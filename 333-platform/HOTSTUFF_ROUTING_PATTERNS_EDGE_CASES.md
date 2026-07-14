# HotStuff Routing: Edge Cases & Production Patterns

> **Companion to**: HOTSTUFF_ROUTING_PATTERNS.md + HOTSTUFF_ROUTING_IMPLEMENTATION.md  
> **Context**: 333 Platform WebRTC mesh, 4-8 validators  
> **Date**: 2026-04-13  
> **KG**: TASK_HotStuff_EdgeCases  
> **Purpose**: Practical patterns for robustness, failure recovery, network anomalies

---

## Part 1: Vote Routing Failure Modes

### Case 1: Vote Lost in Transit (Network Packet Drop)

**Scenario**: Node 3 sends vote to leader (Node 1), packet dropped by OS/network.

```
Node 3: send_to(1, Vote{block: 0x1a2b, phase: Prepare})
        ↓
        [PACKET LOST]  ← Random jitter, congestion, hw fault
        ↓
Node 1: [timeout, never receives vote]
```

**Detection**: Leader's vote collection timer per block.

```rust
pub struct VoteCollectionTimer {
    started_at: Instant,
    timeout: Duration,      // e.g., 500ms
    block_hash: u64,
    phase: Phase,
}

impl HotStuffState {
    pub fn on_timer_tick(&mut self, now: Instant) {
        for timer in &self.vote_timers {
            if now.duration_since(timer.started_at) > timer.timeout {
                eprintln!("[WARN] Vote collection timeout for block {}", timer.block_hash);
                
                // Action 1: Re-broadcast proposal to remind validators
                if let Some(block) = self.blocks.get(&timer.block_hash) {
                    let proposal = HotStuffMsg::Proposal {
                        block: block.clone(),
                        phase: timer.phase,
                        signature: sign(self.node_id, block.hash),
                    };
                    // Don't broadcast() immediately (spammy)
                    // Instead, only broadcast to peers not in vote_set for this block
                    self.resend_proposal_to_missing_voters(&proposal);
                }
                
                // Action 2: If still no quorum after 2 retries, timeout view
                if timer.retry_count > 2 {
                    self.trigger_view_timeout();
                }
            }
        }
    }
    
    fn resend_proposal_to_missing_voters(&mut self, proposal: &HotStuffMsg, transport: &mut dyn Transport) {
        if let HotStuffMsg::Proposal { block, phase, .. } = proposal {
            let voters_set = self.votes.get(&block.hash)
                .map_or(HashSet::new(), |v| v.iter().collect());
            
            for peer in self.validators.all_validators() {
                if peer != self.node_id && !voters_set.contains(&peer) {
                    // Unicast only to non-voters
                    let _ = transport.send(peer, proposal.clone());
                }
            }
        }
    }
}
```

**Recovery Strategy**: Exponential backoff + view timeout.

```
Retry 0: Immediate retry (0ms delay)
Retry 1: After 50ms
Retry 2: After 100ms
Retry 3: TIMEOUT → trigger view change
```

---

### Case 2: Vote Arrives After Leader Decides Quorum

**Scenario**: 3 votes arrived (quorum formed), 4th vote arrives late.

```
Leader's timeline:
  0ms:  Vote from Node 2 arrives
  5ms:  Vote from Node 3 arrives
 10ms:  Vote from Node 4 arrives → quorum = 3, form QC, broadcast NewView
 15ms:  Vote from Node 2 arrives AGAIN (delayed)
        ↓
        Should not double-count!
```

**Prevention**: Equivocation tracking by (sender, phase).

```rust
pub struct HotStuffState {
    pub vote_tracker: HashMap<(NodeId, Phase), u64>,  // (sender, phase) → block_hash
    pub votes: HashMap<u64, Vec<Signature>>,  // block_hash → signatures
}

impl HotStuffState {
    pub fn on_vote(&mut self, vote: &Vote) -> Result<(), VoteError> {
        let key = (vote.sender, vote.phase);
        
        // Check if we already counted a vote from this sender in this phase
        if let Some(prev_hash) = self.vote_tracker.get(&key) {
            if *prev_hash == vote.block_hash {
                // Duplicate, ignore
                eprintln!("[INFO] Duplicate vote from {} for block {}", vote.sender, vote.block_hash);
                return Ok(());
            } else {
                // Equivocation: sender voted for TWO different blocks!
                eprintln!("[ERROR] Equivocation: {} voted for {} AND {}", 
                          vote.sender, prev_hash, vote.block_hash);
                return Err(VoteError::Equivocation);
            }
        }
        
        // First time seeing this sender's vote in this phase
        self.vote_tracker.insert(key, vote.block_hash);
        self.votes.entry(vote.block_hash)
            .or_insert_with(Vec::new)
            .push(vote.signature);
        
        Ok(())
    }
}
```

---

### Case 3: Leader Crash After Broadcasting Proposal (Before Aggregating Votes)

**Scenario**: Leader sends proposal, then crashes. Validators have proposal but leader never collects votes.

```
Timeline:
  0ms:  Leader broadcasts Proposal(B0)
  5ms:  Validators receive, send votes → Leader
 10ms:  CRASH! Leader process dies
        ↓
        Validators waiting for NewView that never comes
```

**Recovery**: Validators timeout, trigger view change.

```rust
// In validator's state machine
pub fn on_timeout(&mut self) -> ProcessResult {
    let old_view = self.view;
    self.view += 1;
    self.phase = Phase::Prepare;
    
    eprintln!("[TIMEOUT] No progress in view {}, advancing to {}", old_view, self.view);
    
    let viewchange = HotStuffMsg::ViewChange {
        new_view: self.view,
        sender: self.node_id,
        high_qc: self.high_qc.clone(),
        signature: sign(self.node_id, self.view),
    };
    
    ProcessResult::Broadcast(viewchange)
}

// Timer loop (in event loop)
pub fn check_timeouts(&mut self, now: Instant) {
    if now.duration_since(self.last_progress) > Duration::from_secs(15) {
        let result = self.on_timeout();
        self.apply_result(result);
        self.last_progress = now;
    }
}
```

---

### Case 4: Vote Arrives from Unknown Validator

**Scenario**: Vote from Node ID 99, but we only expect [1, 2, 3, 4].

```rust
pub fn validate_vote(&self, vote: &Vote) -> Result<(), VoteError> {
    // Check sender is in validator set
    if !self.validators.is_member(vote.sender) {
        eprintln!("[WARN] Vote from unknown validator {}", vote.sender);
        return Err(VoteError::UnknownValidator);
    }
    
    // Verify signature
    if !verify_signature(&vote.signature, vote.sender, vote.block_hash) {
        eprintln!("[WARN] Invalid signature from {}", vote.sender);
        return Err(VoteError::InvalidSignature);
    }
    
    Ok(())
}
```

---

## Part 2: Leader Rotation Edge Cases

### Case 5: Validators Disagree on View (Safety Violation)

**Scenario**: Network partition causes diverging view numbers.

```
Partition A: Nodes 1, 2 → view = 5 (thinking Node 1 is leader)
Partition B: Nodes 3, 4 → view = 6 (thinking Node 3 is leader)
             ↓ Merge
Nodes 1,2 have Prepare phase votes for Node 1's block
Nodes 3,4 have Prepare phase votes for Node 3's block
```

**Prevention**: Never vote for two different blocks in same phase.

```rust
pub fn on_vote_request(&mut self, proposal: &Block, phase: Phase) -> Result<(), SafetyViolation> {
    // Rule 1: Don't vote for conflicting blocks
    if let Some(pending) = &self.pending_block {
        if pending.hash != proposal.hash && pending.view == proposal.view && pending.parent_hash == proposal.parent_hash {
            return Err(SafetyViolation::ConflictingVote);
        }
    }
    
    // Rule 2: Locked QC prevents voting for sibling blocks
    // (higher rounds can unlock, but not backwards)
    if self.view < proposal.view && self.locked_qc.block_hash != proposal.parent_hash {
        return Err(SafetyViolation::LockedQcViolation);
    }
    
    Ok(())
}
```

---

### Case 6: View Change Triggered Simultaneously by Multiple Nodes

**Scenario**: All 4 validators timeout at same time (network hiccup). All broadcast ViewChange{5}.

```
Time: 15000ms (all timeout)
  Node 1: broadcast ViewChange{5}
  Node 2: broadcast ViewChange{5}
  Node 3: broadcast ViewChange{5}
  Node 4: broadcast ViewChange{5}
            ↓ (4 messages × 4 receivers = 16 messages in network)
            Can cause feedback loop!
```

**Prevention**: ViewChange idempotence + deduplication.

```rust
pub struct HotStuffState {
    pub last_viewchange_broadcast: HashMap<u64, Instant>,  // view → time
}

impl HotStuffState {
    pub fn on_timeout(&mut self, now: Instant) -> ProcessResult {
        let old_view = self.view;
        self.view += 1;
        
        // Check if we already broadcasted ViewChange for this view
        if let Some(last_time) = self.last_viewchange_broadcast.get(&self.view) {
            if now.duration_since(*last_time) < Duration::from_millis(100) {
                // Too soon, suppress retransmit
                eprintln!("[INFO] Suppressing duplicate ViewChange{}", self.view);
                return ProcessResult::None;
            }
        }
        
        // Broadcast ViewChange
        let vc = HotStuffMsg::ViewChange { /* ... */ };
        self.last_viewchange_broadcast.insert(self.view, now);
        
        ProcessResult::Broadcast(vc)
    }
}
```

---

## Part 3: WebRTC Mesh Issues

### Case 7: Peer Connection Establishment Fails (Signaling Error)

**Scenario**: Node 3 tries to establish connection to Node 1, but signaling server unreachable.

```
Node 3 to Signaling Server: "Tell Node 1 to connect to me"
         ↓ [TIMEOUT]
         [Connection unreachable]
         ↓
Node 3 can't vote → timeout → view change

But with full mesh assumption, Node 3 should be pre-connected to all nodes!
```

**Mitigation**: Establish all connections during initialization.

```rust
pub async fn bootstrap_full_mesh(
    local_id: u32,
    peer_ids: &[u32],
    signaling_url: &str,
) -> Result<MeshRoom, Box<dyn std::error::Error>> {
    let mut mesh = MeshRoom::new(local_id, RoomConfig::default());
    
    for &peer_id in peer_ids {
        if peer_id == local_id {
            continue;  // Don't connect to self
        }
        
        match establish_webrtc_connection(local_id, peer_id, signaling_url).await {
            Ok(channel) => {
                mesh.add_peer(peer_id, Box::new(channel), now_ms());
                eprintln!("[OK] Connected to peer {}", peer_id);
            }
            Err(e) => {
                eprintln!("[WARN] Failed to connect to peer {}: {}", peer_id, e);
                // Don't crash; mesh can operate with partial connectivity
                // But if peer_id is the leader and we can't reach it, vote→broadcast fallback
            }
        }
    }
    
    // Check minimum connectivity
    if mesh.peer_count() < peer_ids.len() / 2 {
        eprintln!("[ERROR] Insufficient connectivity (< N/2)");
        return Err("Insufficient connectivity".into());
    }
    
    Ok(mesh)
}
```

---

### Case 8: DataChannel Suddenly Closes (Peer Disconnect)

**Scenario**: Node 2's connection to Node 1 is closed (browser tab closed, network card fail).

```
Before: Node 2 ↔ Node 1 (connected)
Event: Node 1 closes browser tab
         ↓
After: Node 2 ↔ Node 1 (ERROR: channel closed)

Node 2 tries: send_to(1, vote)
              ↓ [CHANNEL CLOSED]
              ↓ Falls back to broadcast
              ↓ [OK]
```

**Handling**: Try-catch + fallback pattern.

```rust
impl Transport for WebRtcTransport {
    fn send(&mut self, to: u32, msg: HotStuffMsg) -> Result<(), TransportError> {
        let encoded = serde_json::to_vec(&msg)?;
        
        match self.mesh.send_to(to, &encoded, ChannelMode::Reliable) {
            Ok(_) => {
                // Direct send successful
                Ok(())
            }
            Err(_) => {
                // Direct send failed
                eprintln!("[WARN] Send to {} failed, no direct connection", to);
                
                // Fallback 1: Broadcast (any peer can forward)
                match self.mesh.broadcast(&encoded, ChannelMode::Reliable) {
                    Ok(_) => {
                        eprintln!("[OK] Broadcasted message, peer will forward");
                        Ok(())
                    }
                    Err(_) => {
                        eprintln!("[ERROR] Broadcast also failed");
                        Err(TransportError::SendFailed)
                    }
                }
            }
        }
    }
}
```

**Mesh repair**: Periodically attempt to re-establish broken connections.

```rust
pub fn mesh_health_check(&mut self, now_ms: u64) {
    if now_ms - self.last_health_check_ms > 5000 {  // every 5 seconds
        for &peer_id in &self.all_peer_ids {
            if !self.is_connected(peer_id) {
                eprintln!("[WARN] Not connected to {}, attempting reconnect...", peer_id);
                // Spawn async task to reconnect
                self.attempt_reconnect(peer_id);
            }
        }
        self.last_health_check_ms = now_ms;
    }
}
```

---

## Part 4: Pipelined Phase Interactions

### Case 9: Votes Arrive Out of Phase Order

**Scenario**: Validators send votes in order [Prepare, PreCommit, Commit], but leader receives them as [PreCommit, Prepare, Commit].

```
Timeline:
  Node 1 (Leader): waiting for Prepare votes
  0ms:  Node 2 sends Vote{B0, PreCommit}  (early! belongs to next phase)
 10ms:  Node 3 sends Vote{B0, Prepare}
 20ms:  Node 4 sends Vote{B0, Prepare}
            ↓
Leader has Prepare votes [2, 3, but not 4 yet]
Leader hasn't even finished Prepare phase
Vote from Node 2 (PreCommit) is stored in PreCommit queue, not lost
```

**Handling**: Strict phase checking before aggregation.

```rust
impl HotStuffState {
    pub fn on_vote(&mut self, vote: &Vote) -> Result<(), VoteError> {
        // Reject votes from future phases
        if vote.phase > self.phase {
            eprintln!("[WARN] Vote from phase {:?}, currently in {:?}, storing for later",
                      vote.phase, self.phase);
            // Store in forward queue (not in vote_tracker yet)
            self.future_votes.push(vote.clone());
            return Err(VoteError::FuturePhase);  // Don't count yet
        }
        
        // Reject votes from past phases (we moved on)
        if vote.phase < self.phase {
            eprintln!("[INFO] Ignoring vote from old phase {:?}", vote.phase);
            return Err(VoteError::OldPhase);
        }
        
        // vote.phase == self.phase: process normally
        self.vote_tracker.insert((vote.sender, vote.phase), vote.block_hash);
        self.votes.entry(vote.block_hash).or_insert_with(Vec::new).push(vote.signature);
        
        Ok(())
    }
    
    pub fn on_phase_advance(&mut self, new_phase: Phase) {
        // Re-process future votes that are now current
        let applicable_votes = self.future_votes.drain_filter(|v| v.phase == new_phase).collect::<Vec<_>>();
        for vote in applicable_votes {
            self.on_vote(&vote).ok();  // Process now that phase matches
        }
    }
}
```

---

### Case 10: Slow Leader (Takes >500ms to Aggregate Votes)

**Scenario**: Leader is overloaded, takes 1 second to form QC. Validators timeout and trigger view change.

```
Timeline:
  0ms:    Proposal broadcast
  30ms:   Validators vote
  60ms:   Leader starts receiving votes
 100ms:   Leader has quorum, but...
 500ms:   Leader busy (GC pause, heavy computation), not checking vote pool
         ↓
 515ms:  Validator timeout triggers (configured as 500ms)
         ↓ Validators broadcast ViewChange
         
 520ms:  Leader finally checks votes, has quorum now, but too late
         ↓ Broadcasts NewView to old view
         ↓ Ignored by validators (already moved to new view)
```

**Prevention**: Separate leader timeout from validator timeout.

```rust
pub struct TimeoutConfig {
    pub leader_aggregate_timeout_ms: u64,   // 100ms (leader is more powerful)
    pub validator_view_timeout_ms: u64,     // 15000ms (long timeout, tolerates GC)
}

impl HotStuffState {
    pub fn on_timer(&mut self, now: Instant) {
        // If I'm leader, check if I'm slow aggregating votes
        if self.is_leader() {
            if now.duration_since(self.last_vote_received) > Duration::from_millis(100) {
                eprintln!("[WARN] Slow to aggregate votes, checking if quorum reached...");
                self.force_check_quorum_and_advance();
            }
        } else {
            // If I'm validator, check view progress
            if now.duration_since(self.last_progress) > Duration::from_millis(15000) {
                eprintln!("[TIMEOUT] No progress in view {}", self.view);
                self.trigger_view_change();
            }
        }
    }
}
```

---

## Part 5: Network Anomalies

### Case 11: High-Latency, Low-Bandwidth Network (10+ second RTT)

**Scenario**: Browser in rural area, 10s RTT to peers.

```
Timeline (with 10s RTT):
  0ms:    Proposal sent
 10s:    Proposal arrives
 10s:    Validator sends vote
 20s:    Vote arrives at leader
 20s:    Leader broadcasts NewView
 30s:    NewView arrives
 30s:    Total latency = 30s to finalize one block
 
At this rate: 1 block / 30s = ~0.033 blocks/sec
With 1024 txs/block: 33 txs/sec (acceptable for game, not for high-frequency trades)
```

**Adaptation**: Increase timeout proportionally.

```rust
let timeout_ms = if network.estimated_rtt_ms > 5000 {
    // High-latency network: scale timeout = 3 × RTT
    3 * network.estimated_rtt_ms
} else {
    // Normal network: fixed 15s timeout
    15000
};

consensus_engine.set_view_timeout(timeout_ms);
```

---

### Case 12: Asymmetric Connectivity (Partition, Non-Uniform Delays)

**Scenario**: Node 1 → {2, 3, 4} OK, but 4 → 1 blocked by firewall (ISP, geographic).

```
Node 4's perspective:
  Can receive from: 1, 2, 3 (inbound OK)
  Can send to: 2, 3 (outbound OK)
  Cannot send to: 1 (firewall blocks)
  ↓
  vote_to(1, vote) fails
  ↓ Fallback: broadcast()
  ↓ Node 2 or 3 forwards vote to Node 1
  ↓ OK, but with extra hop
```

**Handling**: Relay routing (already described in Questions).

```rust
pub fn send_with_relay_fallback(&mut self, to: u32, msg: &[u8]) -> Result<(), String> {
    // Try direct
    if self.send_direct(to, msg).is_ok() {
        return Ok(());
    }
    
    // Try relay through each peer
    for &relay_peer in self.connected_peers() {
        if relay_peer == to || relay_peer == self.local_id {
            continue;
        }
        
        // Send message wrapped with relay header: "forward to X"
        let wrapped = wrap_relay_message(to, msg);
        if self.send_direct(relay_peer, &wrapped).is_ok() {
            eprintln!("[OK] Relayed message to {} via {}", to, relay_peer);
            return Ok(());
        }
    }
    
    Err("No route to peer".into())
}
```

---

## Part 6: Production Checklist

### Monitoring & Metrics

```rust
pub struct ConsensusMetrics {
    pub votes_received: u64,
    pub votes_duplicate: u64,
    pub votes_equivocation: u64,
    pub quorum_time_ms: u64,
    pub view_changes: u64,
    pub blocks_committed: u64,
    pub txs_committed: u64,
}

impl ConsensusEngine {
    pub fn report_metrics(&self) -> ConsensusMetrics {
        ConsensusMetrics {
            votes_received: self.state.vote_count,
            votes_duplicate: self.state.duplicate_votes,
            votes_equivocation: self.state.equivocation_attempts,
            quorum_time_ms: self.state.last_quorum_time_ms,
            view_changes: self.state.view_changes,
            blocks_committed: self.state.committed_count(),
            txs_committed: self.state.committed_txs_count(),
        }
    }
}
```

### Logging Strategy

```rust
// Level 0: INFO — state transitions
eprintln!("[INFO] View {} → {}, leader: {}", old_view, new_view, new_leader);

// Level 1: WARN — transient issues (will be recovered)
eprintln!("[WARN] Vote from {} lost, retrying...", peer_id);

// Level 2: ERROR — critical but survived
eprintln!("[ERROR] Equivocation detected from {}", peer_id);

// Level 3: PANIC — unrecoverable
panic!("[FATAL] Quorum size = {} < f+1, safety violated!", quorum_size);
```

---

## References

- **HotStuff Paper** (Moniz et al.): Section 4 (safety conditions), Section 5 (liveness conditions)
- **PBFT** (Castro & Liskov): View change protocol (basis for HotStuff)
- **Tendermint**: Round-robin view change (BFT in production)
- **Algorand**: Value proposal before voting (prevents ballot veto)

---

## Summary Table: Edge Cases + Mitigations

| Edge Case | Symptom | Root Cause | Detection | Fix |
|-----------|---------|-----------|-----------|-----|
| Vote lost in transit | Leader never gets votes | Network drop | Vote timeout | Resend proposal or timeout |
| Duplicate vote | Double-count vote | Delayed retransmit | Track (sender, phase) | Deduplicate |
| Equivocation | Two votes different blocks | Malicious node or bug | Different block_hash | Reject, move to next view |
| Leader crash | No progress in view | Leader offline | 15s timeout | View change to next leader |
| Simultaneous timeouts | Feedback loop | All nodes timeout together | View change count spike | Suppress duplicate ViewChange |
| DataChannel closes | Send fails | Peer disconnect | send() returns error | Broadcast fallback |
| Out-of-order votes | Phase mismatch | Slow network, pipelining | vote.phase != state.phase | Forward queue for later phase |
| Slow leader | Validators timeout early | Leader GC pause | Leader agg time > 500ms | Separate leader/validator timeouts |
| High RTT (10s) | Slow finality | Geographic | Measure network | Scale timeout = 3 × RTT |
| Asymmetric connect | Can't send to peer | Firewall, ISP | send_direct() fails | Relay through other peer |

---

**End of Edge Cases Guide.**

Use alongside HOTSTUFF_ROUTING_PATTERNS.md for full context.

