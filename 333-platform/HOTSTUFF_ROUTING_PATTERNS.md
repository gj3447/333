# HotStuff BFT Message Routing Patterns over P2P Mesh Networks

> **Context**: 333 Platform (WebRTC full-mesh, 4-8 validators, browser WASM)  
> **Date**: 2026-04-13  
> **KG**: TASK_HotStuff_Routing_Research  
> **Status**: Research Complete | Practical Implementation Guide

---

## Executive Summary

HotStuff leader-based consensus requires **asymmetric routing**:
- **Leader → All**: Broadcast proposals (1 sender, N receivers)
- **All → Leader**: Send votes (N senders, 1 receiver)
- **View changes**: New leader, new vote routing targets

Over a **full-mesh WebRTC network** (4-8 peers), this translates to:
1. **Leader broadcast** = `send_to(all_peers)` in parallel (no intermediate hops needed)
2. **Validator votes** = `send_to(leader)` directly if connected, else **relay** through mesh
3. **View changes** = Compute new leader index, redirect vote routing
4. **Partial mesh** = Must implement **leader discovery + vote relaying**
5. **Pipelining** = Vote→NewView transitions overlap, keep **separate queues per phase**
6. **Message flow** = 3 phases × (broadcast proposal + collect votes + aggregate) + view change handling

---

## Question 1: How does leader broadcast proposals?

### Pattern: Direct Parallel Send

```
Leader (Node L)
    ├─→ [msg: Proposal{block, phase}] ──→ Node A
    ├─→ [msg: Proposal{block, phase}] ──→ Node B
    ├─→ [msg: Proposal{block, phase}] ──→ Node C
    └─→ (self: store locally, don't send to self)

Time: Single round-trip = 1 RTT + network jitter
Cost: 1 broadcast op = send() called N-1 times in parallel
```

### Implementation Pattern (from src/p2p/mesh.rs)

```rust
impl MeshRoom {
    /// Broadcast data to all peers
    pub fn broadcast(&self, data: &[u8], mode: ChannelMode) -> usize {
        let mut sent = 0;
        for ch in self.channels.values() {  // ← all peers except self
            if ch.send(data, mode).is_ok() {
                sent += 1;
            }
        }
        sent
    }
}
```

**Expected behavior:**
- **WebRTC DataChannel order guarantee**: All proposals reach in FIFO order (ordered=true)
- **No relay needed**: Full mesh = direct connection to every peer
- **Buffering**: Browser OS buffers outbound frames; HotStuff should not wait for ACKs

### Practical Considerations

| Scenario | Handling |
|----------|----------|
| Leader offline | View timeout triggers (15s), triggers view change |
| Slow leader | Validators vote faster than leader sends next proposal → early commit |
| Leader partitioned | Other validators timeout-trigger view change |
| All-to-leader bottleneck | Leader receives N-1 votes, must aggregate quickly (< 1ms for N=7) |

---

## Question 2: How do validators route votes to leader?

### Pattern: Direct Send + Optional Relay

```
Validator V ──→ [msg: Vote{block_hash, phase}] ──→ Leader
                                                     (if direct conn exists)
                                                     ↓
                                                   Direct: Store in vote pool
                                                   ↓
                                                 NOT direct: Relay through mesh?
```

**Key insight**: Vote must reach the leader. If no direct connection, two options:
1. **Establish connection on-demand** (expensive, defeats full-mesh assumption)
2. **Relay through intermediate peer** (requires routing protocol)
3. **Leader fallback**: If vote doesn't arrive, leader times out and advances

### Implementation: Send to Leader

```rust
// From consensus state machine (bft/state.rs conceptual)
impl HotStuffState {
    pub fn process_proposal(&mut self, proposal: &Block) -> ProcessResult {
        // ... validate proposal ...
        
        let vote = HotStuffMsg::Vote {
            block_hash: proposal.hash,
            view: self.view,
            phase: Phase::Prepare,
            signature: sign(self.node_id, proposal.hash),
        };
        
        // ← Route to current leader
        let leader = self.current_leader();
        ProcessResult::SendToLeader(vote)
    }
}

// In main event loop (pseudo-code):
match state.process(incoming_msg) {
    ProcessResult::SendToLeader(vote) => {
        mesh.send_to(leader_id, encode(&vote))?;
        // If send fails (no connection):
        //   Option A: Retry with exponential backoff
        //   Option B: Broadcast vote to all (wasteful, but guarantees delivery)
        //   Option C: Drop and wait for view timeout
    }
    ProcessResult::Broadcast(msg) => {
        mesh.broadcast(encode(&msg))?;
    }
    _ => {}
}
```

### Handling Leader Not Directly Connected

**Scenario**: Full mesh has 6 peers, but leader is not in my peer list.

**Solutions** (in order of practicality):

1. **Rely on peer introduction (signaling server)**
   - Signaling server maintains full validator list
   - After connecting to 2+ peers, query signaling server for leader address
   - Establish direct connection to leader before entering consensus
   - **Latency**: 1 signaling server RTT + 1 WebRTC offer/answer exchange (5-10s total)

2. **Broadcast votes to all validators** (fallback)
   - Broadcast vote instead of targeted send
   - Any validator can forward vote to leader if they are connected
   - **Cost**: 5-6× message overhead, but 100% reliable
   - **Use case**: Initial bootstrap when full connectivity not guaranteed

3. **Gossip protocol** (advanced)
   - Each validator gossips votes to random peers
   - Leader eventually receives vote through gossip chain
   - **Latency**: O(log N) hops, ~5-10 hops for N=50
   - **Not recommended for <8 validators** (overhead not justified)

### WebRTC Connection Lifecycle

```
Time │  Peer A                      │  Peer B (Leader)
─────┼──────────────────────────────┼──────────────────
  t0 │ sigserver: who is leader?    │
     │ ← "Node 2 is leader"         │
─────┼──────────────────────────────┼──────────────────
  t1 │ send ICE candidate to sig.   │  wait for offer
     │ create offer                 │
─────┼──────────────────────────────┼──────────────────
  t2 │ sigserver: relay offer       │ get offer
     │                              │ create answer
     │                              │ send answer
─────┼──────────────────────────────┼──────────────────
  t3 │ recv answer                  │
     │ ICE gathering (STUN)         │  ICE gathering
─────┼──────────────────────────────┼──────────────────
  t4 │ connected! send vote         │ recv vote
     │ ready to participate         │ start building QC
```

---

## Question 3: Leader rotation: How to redirect vote routing on view change?

### Pattern: Compute New Leader, Update Routing Target

```rust
// Current view → leader routing
pub fn current_leader(&self) -> NodeId {
    leader_for_view(self.view, &self.validators)
    //                ↑
    //          monotonically increasing
}

// View change triggered → old_leader gone, new_leader elected
impl HotStuffState {
    pub fn on_view_change(&mut self, new_view: u64) {
        let old_leader = leader_for_view(self.view, &self.validators);
        let new_leader = leader_for_view(new_view, &self.validators);
        
        self.view = new_view;
        self.phase = Phase::Prepare;  // reset to prepare phase
        self.pending_block = None;     // discard old proposal
        self.votes.clear();            // clear old votes
        
        // All future votes now target new_leader
        // No code change needed — self.current_leader() automatically returns new leader
    }
}
```

### View Change Trigger: Timeout

```
Time │ Node 1         │ Node 2 (Leader) │ Node 3
─────┼────────────────┼─────────────────┼──────────────
  0s │ send proposal  │ (I'm leader)    │
     │ ← Proposal     │                 │
─────┼────────────────┼─────────────────┼──────────────
  1s │ send vote      │ (collecting...) │ send vote
     │                │                 │
─────┼────────────────┼─────────────────┼──────────────
  2s │ ↓              │ (leader CRASH!) │ ↓
     │ wait for QC    │ (leader OFFLINE)│ wait for QC
─────┼────────────────┼─────────────────┼──────────────
 15s │ TIMEOUT!       │                 │ TIMEOUT!
     │ view_change()  │                 │ view_change()
─────┼────────────────┼─────────────────┼──────────────
 15s │ new leader=N3  │                 │ new leader=N3
     │ send vote→N3   │                 │ (I'm leader!)
     │                │                 │ ready to propose
```

### Implementation: View Change Message

```rust
// From bft/types.rs
pub enum HotStuffMsg {
    ViewChange {
        new_view: u64,
        sender: NodeId,
        high_qc: QuorumCert,  // ← proves you know highest finalized block
        signature: Signature,
    },
}

// Handling view change from other validators
impl HotStuffState {
    pub fn on_view_change_request(&mut self, new_view: u64, high_qc: &QuorumCert) {
        if new_view > self.view {
            self.on_view_change(new_view);
            
            // If I'm new leader, start proposing
            if self.is_leader() {
                // Create proposal using high_qc as justify
                let msg = self.propose_with_justify(high_qc);
                return ProcessResult::Broadcast(msg);
            }
        }
    }
}
```

### Network-Level: Mesh Must Support Multi-Leader Discovery

```rust
// In mesh.rs: keep leader address up-to-date
pub struct MeshRoom {
    pub config: RoomConfig,
    local_id: u32,
    peers: HashMap<u32, PeerInfo>,
    channels: HashMap<u32, Box<dyn DataChannel>>,
    current_leader: u32,  // ← NEW: cache current leader ID
}

impl MeshRoom {
    pub fn update_leader(&mut self, new_leader_id: u32) {
        self.current_leader = new_leader_id;
        // All future votes route to new_leader_id
    }
    
    pub fn send_to_leader(&self, data: &[u8], mode: ChannelMode) -> bool {
        self.send_to(self.current_leader, data, mode)
    }
}
```

---

## Question 4: Partial mesh — What if leader not directly connected?

### Scenario: 4 validators, only 5 edges (not full-mesh)

```
Node 1 (Leader) ──┬──→ Node 2
                  └──→ Node 3
Node 4 ──→ Node 2
```

**Problem**: Node 4 cannot send vote directly to Node 1.

### Solution Pattern: Fallback to Broadcast

```rust
impl MeshRoom {
    pub fn send_to_with_fallback(&self, to: u32, data: &[u8]) -> bool {
        // Try direct send
        if self.send_to(to, data, ChannelMode::Reliable).is_ok() {
            return true;  // ← success
        }
        
        // Fallback: broadcast (any peer may relay)
        web_sys::console_log_1(&"[WARN] No direct path to leader, broadcasting vote".into());
        self.broadcast(data, ChannelMode::Reliable);
        true  // ← relay will eventually deliver
    }
}
```

### Solution Pattern: Smart Relay

```rust
// Pre-compute routing table from mesh topology
pub struct RoutingTable {
    routes: HashMap<u32, Vec<u32>>,  // to → [via, via, ...]
}

impl RoutingTable {
    pub fn best_path(&self, from: u32, to: u32) -> Option<Vec<u32>> {
        // BFS to find shortest path
        // If to == from: direct
        // If peer in channels: direct
        // Else: [via_peer1, via_peer2, ...]
    }
}

// Usage in consensus
match mesh.send_to(leader, &vote_msg) {
    Ok(_) => {
        // Direct send worked
    }
    Err(_) => {
        // Use routing table to find relay path
        if let Some(path) = mesh.routing_table.best_path(my_id, leader) {
            // Send via first hop: path[0]
            mesh.send_to(path[0], &vote_with_forward_header(vote, leader))?;
        }
    }
}
```

**Practical Note**: For 4-8 validators, a **full mesh is strongly recommended**. With 7 nodes, only 21 direct connections needed. Cost of partial mesh routing > cost of establishing 4-5 extra peer connections.

### Heartbeat + Leader Discovery

```rust
// Periodic heartbeat to discover new peers
pub fn heartbeat(&mut self, now_ms: u64) {
    for (peer_id, info) in &mut self.peers {
        if now_ms - info.last_heartbeat > self.config.heartbeat_interval_ms {
            // Send ping
            self.send_to(*peer_id, b"PING", ChannelMode::Reliable).ok();
        }
    }
    
    // If not connected to leader, attempt to establish connection
    if !self.is_connected_to(self.current_leader) {
        web_sys::console_log_1(&format!("Not connected to leader {}, requesting peer list...", 
                                         self.current_leader).into());
        // Query signaling server for leader's address
        self.query_signaling_server(self.current_leader);
    }
}
```

---

## Question 5: Pipelining — How does Prepare/PreCommit/Commit affect message flow?

### Background: HotStuff 3-Phase Pipeline

HotStuff **pipelines 3 phases**, so blocks are simultaneously in different phases:

```
Block B0 (Prepare phase)
Block B1 (PreCommit phase, waits for Prepare QC of B0)
Block B2 (Commit phase, waits for PreCommit QC of B1)
Block B3 (Decide phase, waits for Commit QC of B2)
                        ↓
Block B3 is FINALIZED (irreversible)
```

### Message Flow Per Block

```
Round │ Proposal                    │ Vote Collection              │ Phase Advance
──────┼─────────────────────────────┼──────────────────────────────┼──────────────────
  1   │ L→All: Proposal{B0, Prep}   │ All→L: Vote{B0.hash, Prep}   │ L→All: NewView{Prep QC}
      │                             │ L collects 2f+1 votes        │
──────┼─────────────────────────────┼──────────────────────────────┼──────────────────
  2   │ L→All: Proposal{B1, PreC}   │ All→L: Vote{B1.hash, PreC}   │ L→All: NewView{PreC QC}
      │                             │ (B1 justify = Prep QC)       │
──────┼─────────────────────────────┼──────────────────────────────┼──────────────────
  3   │ L→All: Proposal{B2, Comm}   │ All→L: Vote{B2.hash, Comm}   │ L→All: NewView{Comm QC}
      │                             │ (B2 justify = PreC QC)       │
──────┼─────────────────────────────┼──────────────────────────────┼──────────────────
  4   │ (B3 commit decided)         │                              │ (B3 finalized, execute)
      │ L→All: Proposal{B4, Prep}   │                              │
```

**Key**: Each round simultaneously sends + receives + aggregates

### Routing Impact: Separate Queues per Phase

```rust
impl Transport for WebRtcTransport {
    /// Messages must be queued by (sender, phase, block_hash)
    fn send(&mut self, to: NodeId, msg: HotStuffMsg) {
        match &msg {
            HotStuffMsg::Vote { phase, .. } => {
                // Queue votes separately per phase
                self.vote_queues[phase as usize].push((to, msg));
                // ← Prevents head-of-line blocking
            }
            HotStuffMsg::NewView { .. } => {
                self.newview_queue.push((to, msg));
            }
            HotStuffMsg::Proposal { .. } => {
                self.proposal_queue.push((to, msg));
            }
            _ => {}
        }
    }
    
    fn recv(&mut self) -> Option<(NodeId, HotStuffMsg)> {
        // Poll queues in priority order
        self.proposal_queue.pop_front()
            .or_else(|| self.newview_queue.pop_front())
            .or_else(|| self.vote_queues[0].pop_front())  // Prepare
            .or_else(|| self.vote_queues[1].pop_front())  // PreCommit
            .or_else(|| self.vote_queues[2].pop_front())  // Commit
    }
}
```

### Network Throughput Per Phase

```
1 Block = 128 KB (1024 txs × 128 bytes each)

Per Round:
  Proposal broadcast:     N × 128 KB = 7 × 128 KB = 896 KB
  Vote unicast:           N × 1 KB = 7 KB (vote is ~100 bytes)
  NewView broadcast:      N × 2 KB = 14 KB (includes QC)
  ─────────────────────────────────────────────────
  Total per round:        ~920 KB

At 4 rounds/second:
  Throughput = 920 KB × 4 = 3.68 MB/s
  
Browser WebRTC: 10-50 MB/s typical (depends on network)
Conclusion: Pipelined HotStuff is throughput-safe for N=7, 100 txs/block
```

### Backpressure: If Votes Arrive Faster Than Leader Can Aggregate

```rust
// In leader's vote aggregation loop
impl HotStuffState {
    pub fn on_vote(&mut self, block_hash: u64, phase: Phase, from: NodeId) {
        self.votes[phase as usize].entry(block_hash)
            .or_insert_with(Vec::new)
            .push(from);
        
        // Check if we have 2f+1 votes
        let vote_count = self.votes[phase as usize][&block_hash].len();
        if vote_count >= self.quorum_size() {
            // ← Form QC and broadcast NewView immediately
            // If votes are queued faster than this check,
            // excess votes are just stored (safe, but uses memory)
        }
    }
}

// To implement backpressure:
pub fn recv_backpressured(&mut self) -> Option<(NodeId, HotStuffMsg)> {
    // Only return message if vote pool < threshold
    if self.pending_votes.len() > MAX_PENDING_VOTES {
        return None;  // ← Caller will retry
    }
    self.recv()
}
```

---

## Question 6: Concrete Message Flow — 4-Validator HotStuff over WebRTC

### Setup

```
4 validators (node IDs: 1, 2, 3, 4)
Full mesh = 6 connections

View 0 Leader: Node 1
View 1 Leader: Node 2 (if Node 1 times out)
View 2 Leader: Node 3
View 3 Leader: Node 4

Quorum = 2f+1 = 3 validators (f=1, tolerate 1 failure)
```

### Successful Round (No Timeout)

```
Time    │ Node 1 (Leader)              │ Nodes 2,3,4 (Validators)        │ Network
────────┼──────────────────────────────┼─────────────────────────────────┼──────────
  0ms   │ [Proposal Phase]             │                                 │
        │ block = {hash: 0x1a2b,       │                                 │
        │          parent: 0x0000,     │                                 │
        │          view: 0}            │                                 │
        │ msg = Proposal{block}        │                                 │
        │                              │                                 │
        │ broadcast()                  │                                 │
        │                              ├→ Node 2 receives Proposal      │ 1 RTT
        │                              ├→ Node 3 receives Proposal      │ ~30ms
        │                              ├→ Node 4 receives Proposal      │
────────┼──────────────────────────────┼─────────────────────────────────┼──────────
 30ms   │                              │ [All validate block]            │
        │                              │ Check parent hash ✓             │
        │                              │ Check signature ✓               │
        │                              │ Create vote                     │
        │                              │ vote = Vote{                   │
        │                              │   block_hash: 0x1a2b,         │
        │                              │   view: 0,                     │
        │                              │   phase: Prepare,              │
        │                              │   sig: sign(node_id, hash)     │
        │                              │ }                               │
        │                              │                                 │
        │                              │ send_to(1, vote)               │ 1 RTT
        │                              │                                 │ ~30ms
        │ Node 2 vote arrives          │←──────────────────────────────│
        │ Node 3 vote arrives          │←──────────────────────────────│
        │ Node 4 vote arrives          │←──────────────────────────────│
────────┼──────────────────────────────┼─────────────────────────────────┼──────────
 60ms   │ [Aggregate votes]            │                                 │
        │ Check: 3 votes >= quorum(3)  │                                 │
        │ ✓ Quorum reached!            │                                 │
        │ Form QC = QuorumCert{        │                                 │
        │   block_hash: 0x1a2b,        │                                 │
        │   signers: [1,2,3],          │                                 │
        │   aggregate_sig: ...         │                                 │
        │ }                             │                                 │
        │                              │                                 │
        │ Advance to PreCommit phase   │                                 │
        │ newview_msg = NewView{       │                                 │
        │   view: 0,                   │                                 │
        │   qc: QC,                    │                                 │
        │ }                             │                                 │
        │ broadcast()                  │                                 │ 1 RTT
        │                              ├→ All nodes receive QC           │ ~30ms
        │                              │                                 │
────────┼──────────────────────────────┼─────────────────────────────────┼──────────
 90ms   │ [PreCommit Phase]            │ [All receive QC]               │
        │ Propose B1 using QC as       │ Vote on B1                     │
        │ justify                      │ send_to(1, vote_b1)            │
        │                              │                                 │
        │ broadcast Proposal(B1)       │                                 │
        │                              ├→ receive B1 proposal            │
        │                              │ send_to(1, vote_b1)            │ 1 RTT
        │ Node 2,3,4 votes arrive      │←──────────────────────────────│
        │                              │                                 │
────────┼──────────────────────────────┼─────────────────────────────────┼──────────
120ms   │ Quorum on B1 ✓               │                                 │
        │ Broadcast NewView{PreCommit} │                                 │
        │                              ├→ Advance to Commit phase        │
        │                              │ send_to(1, vote_b2)            │
────────┼──────────────────────────────┼─────────────────────────────────┼──────────
150ms   │ Quorum on B2 ✓               │                                 │
        │ Broadcast NewView{Commit}    │                                 │
        │                              ├→ Advance to Decide phase        │
        │                              │ (B0 is now FINALIZED)           │
        │                              │ Execute transactions in B0      │
────────┼──────────────────────────────┼─────────────────────────────────┼──────────

Total latency: 150ms (proof-of-work: 0ms, networking: 150ms)
Throughout: 4 phases/150ms = 26.7 blocks/sec × 1024 txs = 27,306 txs/sec
```

### View Change Scenario (Leader Timeout)

```
Time    │ Node 1 (Leader)              │ Nodes 2,3,4 (Validators)
────────┼──────────────────────────────┼──────────────────────────────
  0ms   │ Broadcast Proposal(B0)       │
 30ms   │                              │ Receive Proposal
        │                              │ Send vote→Node 1
 60ms   │ [CRASH / NETWORK PARTITION]  │ (Leader unreachable)
        │ (Node 1 goes offline)        │
────────┼──────────────────────────────┼──────────────────────────────
15000ms │ (offline)                    │ TIMEOUT TRIGGERED!
        │                              │ view_change() → view = 1
        │                              │ New leader = Node 2
        │                              │
        │                              │ Broadcast ViewChange{
        │                              │   new_view: 1,
        │                              │   high_qc: highest QC I've seen
        │                              │ }
────────┼──────────────────────────────┼──────────────────────────────
15030ms │ (offline)                    │ Node 2 receives ViewChange{1}
        │                              │ Node 3 receives ViewChange{1}
        │                              │ Node 4 receives ViewChange{1}
        │                              │
        │                              │ Node 2: "I'm new leader!"
        │                              │ Create proposal B0'
        │                              │ broadcast Proposal(B0')
────────┼──────────────────────────────┼──────────────────────────────
15060ms │                              │ Node 3,4 receive Proposal
        │                              │ Vote on B0'
        │                              │ send_to(Node 2)
────────┼──────────────────────────────┼──────────────────────────────
15090ms │                              │ Node 2 gets 3 votes
        │                              │ Broadcast NewView{Prepare QC}
        │                              │
        │                              │ Continue with view 1...
```

### Mesh Topology: How Votes Route to Leader

```
Scenario: Node 1 is leader, but connection to Node 3 temporarily broken

Topology:
  1 ←→ 2
  1 ←→ 4
  2 ←→ 3
  2 ←→ 4
  3 ←→ 4

Node 3's vote routing:
  Option A (direct): send_to(1) → fails (no connection)
  Option B (fallback): broadcast() → Node 2 will forward to Node 1
                                  → Node 4 will forward to Node 1
  
Result: Vote eventually reaches Node 1 through relays (higher latency)
Alternative: Node 3 initiates connection to Node 1 (through signaling server)
```

---

## Implementation Checklist: 333 Platform Integration

### Phase 1: Basic Routing (Current)

- [x] Transport trait abstraction (InMemoryNetwork, WebRTC stubs)
- [x] Mesh topology (MeshRoom)
- [x] Broadcast primitive
- [ ] **Direct send-to-peer** (TODO: implement in MeshRoom, test with 4 nodes)
- [ ] **Leader routing** (vote→leader) in consensus state machine
- [ ] Unit tests: vote reaches leader in <100ms

### Phase 2: View Change + Resilience

- [ ] View change detection (15s timeout)
- [ ] New leader computation (round-robin by view)
- [ ] Fallback: broadcast votes if direct send fails
- [ ] Integration tests: 4 nodes, 1 leader crash → failover in <16s

### Phase 3: Pipelining

- [ ] Separate vote queues per phase
- [ ] Head-of-line unblocking (can receive Prepare votes while sending PreCommit votes)
- [ ] End-to-end test: 4 nodes, 3-phase pipeline, measure throughput

### Phase 4: Production Hardening

- [ ] Backpressure: reject incoming votes if vote pool > threshold
- [ ] Heartbeat + leader discovery (re-establish broken connections)
- [ ] Network partition simulation (random packet loss, high latency)
- [ ] Metrics: vote latency, QC formation time, finality latency

---

## Technical Deep Dives by Concern

### Vote Aggregation: Equivocation Detection

```rust
// From bft/state.rs: prevent validator from voting twice in same phase
pub fn on_vote(&mut self, vote: &Vote) -> Result<(), VoteError> {
    let key = (vote.signature, vote.phase);
    
    if let Some(prev_hash) = self.vote_tracker.get(&key) {
        if *prev_hash != vote.block_hash {
            // ← EQUIVOCATION: validator voted for two different blocks
            return Err(VoteError::Equivocation);
        }
    }
    
    self.vote_tracker.insert(key, vote.block_hash);
    Ok(())
}
```

### Message Ordering: Total Order on Broadcast

WebRTC DataChannel with `ordered: true` guarantees:
- **Within one channel (peer-to-peer)**: FIFO
- **Across channels**: No guarantee (Node 2 proposal arrives before Node 3 vote)

This is **safe for HotStuff**:
- Proposals don't depend on previous proposals (each refers to parent via hash)
- Votes don't depend on each other (leader aggregates, finds quorum)
- NewView broadcasts don't depend on specific arrival order

### Signature Verification: Reduce per-message overhead

Current: Every proposal/vote signed individually.
Optimized: BLS aggregate signatures (combine N signatures into 1).

For N=7, BLS saves: 7×64 bytes = 448 bytes per QC.
At 27K txs/sec: ~100 MB/s → 95 MB/s saved (3.8% throughput gain).

**Recommendation**: Skip BLS for N≤7 (verification CPU cost > network savings).

---

## Edge Cases & Mitigations

| Case | Detection | Recovery |
|------|-----------|----------|
| Leader votes for self | Signature verification fails | Reject, move to next validator |
| Vote arrives after quorum formed | Stored in vote pool | Used in next phase's QC |
| NewView arrives before Proposal | Store in queue | Process once Proposal arrives |
| Partition: validators split 2-1 | Minority times out, merges with majority | View advances until all connected |
| Network reordering | Out-of-order votes received | HashMap-based vote storage handles any order |
| Duplicate vote | Same (sender, phase, block_hash) | De-duplicate before storing in vote pool |

---

## References & Further Reading

### HotStuff Original Paper
- Moniz, Yin, et al. "HotStuff: BFT Consensus with Linearity and Responsiveness" (PODC 2019)
- Emphasis on **pipelined phases** and **leader-driven aggregation**
- Section 3.2: "Vote collection and aggregation"

### WebRTC Mesh Challenges
- Full-mesh limits: N*(N-1)/2 connections. At N=50: 1,225 connections.
- Recommendation: N≤8 for browser, N≤50 with super-peer relays.

### 333 Platform Status
- BFT consensus: ✅ Types, state machine, leader election implemented
- WebRTC mesh: ✅ Peer lifecycle, broadcast primitive
- **Gap**: No message routing between BFT and mesh (TODO: INT_ConsensusNet)

---

## Summary: Rules of Thumb

| Rule | Rationale |
|------|-----------|
| **Full mesh for N≤8** | Reduces routing complexity, lowest latency |
| **Direct send-to-leader** | No queuing, <30ms latency |
| **Broadcast fallback** | If direct send fails, broadcast as last resort |
| **Separate queues per phase** | Prevent head-of-line blocking in pipelined protocol |
| **15s timeout for view change** | Empirically: ~3s for leader recovery, 12s buffer |
| **Verify proposal signature** | Prevents malicious leader injection |
| **Equivocation tracking** | Detect validator voting for >1 block/phase |
| **Heartbeat every 5s** | Detect dead peers, repair broken connections |

**Practical: For 333 Platform with 4-7 validators in browser, focus on Direct Send → Leader with Broadcast Fallback. Pipelining is secondary.**

