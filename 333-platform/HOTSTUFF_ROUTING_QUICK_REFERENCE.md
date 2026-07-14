# HotStuff Routing — Quick Reference Card

> One-page cheat sheet for implementation  
> Full details: HOTSTUFF_ROUTING_PATTERNS.md

---

## The 6 Questions (Answers)

| Q | Pattern | Code |
|---|---------|------|
| **Leader broadcast?** | Parallel direct send to all peers | `mesh.broadcast(proposal)` |
| **Vote→leader routing?** | `send_to(leader)` + broadcast fallback | `transport.send_to_leader(leader, vote)` |
| **View change redirect?** | Compute new leader from view# | `leader = validators[(view % N)]` |
| **Partial mesh?** | Relay through any peer | `send_to(relay_peer, msg_with_forward_header)` |
| **Pipelining effect?** | Separate queues per phase | `vote_queues[phase_idx].push(vote)` |
| **4-validator flow?** | 3 phases × 50ms = 150ms round | See PATTERNS.md § Q6 |

---

## Key Data Structures

```rust
// Transport (abstract)
trait Transport {
    fn send(&mut self, to: u32, msg: HotStuffMsg);        // unicast
    fn broadcast(&mut self, msg: HotStuffMsg);            // all-except-self
    fn send_to_leader(&mut self, msg) → send_to(leader)?; // with fallback
    fn recv(&mut self) → Option<(u32, HotStuffMsg)>;
}

// WebRTC Transport (concrete)
struct WebRtcTransport {
    mesh: MeshRoom,
    current_leader: u32,
    proposal_queue: Queue,
    vote_queues: [Queue; 3],  // Prepare, PreCommit, Commit
    newview_queue: Queue,
}

// Vote tracking (prevent equivocation)
pub vote_tracker: HashMap<(NodeId, Phase), u64>;  // (sender, phase) → block_hash
// If (sender, phase) appears twice with different block_hash → equivocation!
```

---

## Main Event Loop (Pseudocode)

```rust
loop {
    // Step 1: Poll network
    while let Some((sender, msg)) = transport.recv() {
        let result = state.process(msg);
        match result {
            SendToLeader(msg) => transport.send_to(leader, msg),
            Broadcast(msg) => transport.broadcast(msg),
            ViewChange(new_view) => {
                state.view = new_view;
                transport.update_leader(state.current_leader());
            }
            _ => {}
        }
    }
    
    // Step 2: Check timeout
    if now - last_progress > 15s {
        state.trigger_view_change();
    }
    
    // Step 3: If leader, propose
    if state.is_leader() && has_txs {
        transport.broadcast(state.propose());
    }
    
    sleep(100ms);  // adjust for network RTT
}
```

---

## Message Flow Diagram (4 validators, Prepare phase)

```
Time │ Node 1 (Leader)      │ Node 2,3,4 (Validators)
─────┼──────────────────────┼──────────────────────────
  0  │ broadcast Proposal   │
     │                      ├─→ receive, validate
  30 │ (gathering votes)    ├─→ create Vote
     │                      ├─→ send_to(1)
  60 │ recv Vote ×3         │
     │ form QC              │
     │ broadcast NewView    │
  90 │                      ├─→ receive NewView
     │ [Prepare done]       ├─→ advance to PreCommit
```

**Latency formula**:
- Broadcast: 1 RTT (~30ms)
- Vote collection: 1 RTT (~30ms)
- Quorum check + NewView: 1 RTT (~30ms)
- Per phase: ~50-100ms
- 4 phases: ~200-400ms per round
- **Practice: 150ms per round, 27K txs/sec**

---

## Critical Rules

1. **Always verify proposal signature** (prevents MITM)
2. **Track (sender, phase)** → detect equivocation
3. **Only vote once per block per phase** (locked_qc)
4. **Timeout must be > max network latency** (15s for RTT<5s)
5. **ViewChange is idempotent** (suppress retransmits)
6. **Separate queues per phase** (prevent head-of-line)
7. **Full mesh recommended** (N ≤ 8 peers)

---

## Failure Scenarios & Recovery

| Failure | Detection | Recovery |
|---------|-----------|----------|
| Vote lost | Leader vote timeout 500ms | Resend proposal or trigger view change |
| Leader down | 15s no progress | View change to next leader |
| Network partition | 2+ view changes | View advances until connected |
| Slow leader (GC) | Vote pool timeout | Increase leader timeout threshold |
| High RTT (10s) | Measure RTT | Scale timeout = 3 × RTT |
| Equivocation | Different block per phase | Reject, report, move on |

---

## Performance Checklist

- [ ] **Latency**: Vote→Leader <30ms (1 RTT)
- [ ] **Throughput**: >1K txs/sec (achievable: 27K)
- [ ] **Finality**: <150ms per round
- [ ] **Network**: <50 Mbps per round
- [ ] **Memory**: <100MB for state + votes at N=7
- [ ] **CPU**: <10% per consensus round (WASM browser)

---

## Files to Modify (7h implementation)

```
src/bft/transport.rs          ← +20 lines (add send_to_leader)
src/bft/state.rs              ← +40 lines (routing logic)
src/p2p/webrtc_transport.rs   ← +150 lines (NEW)
src/bft/consensus_engine.rs   ← +120 lines (NEW)
src/wasm.rs                   ← +30 lines (API)
tests/hotstuff_mesh_*         ← +80 lines (NEW)
```

**Total: ~450 lines of new/modified code**

---

## Debug: What to Log

```rust
// State transitions (INFO)
[INFO] View 0 → 1, leader: 2

// Quorum formation (INFO)
[INFO] Quorum reached: 3/4 votes for block 0x1a2b

// Transient issues (WARN)
[WARN] Send to leader failed, broadcasting instead
[WARN] Vote from old phase, ignoring

// Critical (ERROR)
[ERROR] Equivocation detected from node 3
[ERROR] Proposal signature invalid

// Debug (TRACE)
[TRACE] Received vote from 2, block 0x1a2b, phase Prepare
```

---

## Testing Sequence

1. **Unit**: Transport methods (send, broadcast, recv)
2. **Integration**: 2-node consensus (leader + 1 validator)
3. **Full**: 4-node consensus (3 rounds, no failures)
4. **Resilience**: 1 node crash → view change → recovery
5. **Performance**: Measure latency, throughput, GC pauses
6. **Network**: Simulate packet loss, reordering, asymmetry

---

## One-Liner Decisions

- **Mesh topology**: Full, not partial (N≤8)
- **Vote routing**: Direct to leader, broadcast fallback
- **Leader election**: Round-robin by view number
- **Phase separation**: Separate queues (prevent blocking)
- **Timeout**: 15s (covers network RTT + GC)
- **Quorum size**: 2f+1 (Byzantine fault tolerance, f=1)
- **Message ordering**: FIFO per channel (DataChannel ordered:true)

---

## See Also

- **HOTSTUFF_ROUTING_PATTERNS.md** — Full research (all 6 questions)
- **HOTSTUFF_ROUTING_IMPLEMENTATION.md** — Code patterns & pseudocode
- **HOTSTUFF_ROUTING_PATTERNS_EDGE_CASES.md** — 12 failure scenarios
- **apt-progress.md** — 333 Platform status

---

*Quick reference generated from research 2026-04-13.*
*For full context, see HOTSTUFF_ROUTING_RESEARCH_INDEX.md*

