# Room Lifecycle & Consensus Initialization Research — Summary
# KG: SPAN_333_RoomLifecycleResearch (Complete)

> **Deliverables**: 4 markdown files + concrete TypeScript/Rust patterns  
> **Date**: 2026-04-13  
> **Status**: Research Complete, Ready for SPAN_333_Integration Phase  
> **Context**: 333 Platform P2P WebRTC + CRDT + HotStuff BFT

---

## Problem Statement (lesson-333-modules-not-integrated)

333 Platform has **11,290 lines** of **working modules** (CRDT, BFT HotStuff, Token, Wire, WASM):
- ✅ Core compiled, 209 tests pass
- ✅ WASM 153KB optimized
- ❌ **Modules never connected via P2P**
- ❌ Room lifecycle undefined
- ❌ CRDT/BFT initialization missing
- ❌ Late joiner catch-up unspecified
- ❌ Zero end-to-end verification

**Why It Matters**: Distributed systems fail catastrophically without clear state machines.
- **What happens when 2nd peer joins?** State machine says: SIGNALING → SYNCING
- **How does CRDT start?** Leader sends snapshot when peers ≥ 3
- **When does BFT begin?** After ValidatorSet is broadcast
- **Can late joiners catch up?** Yes: snapshot + committed block replay

---

## Deliverables (4 Documents)

### 1. ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md (12 KB, 370 lines)

**Complete specification of P2P room lifecycle:**

- **Section 1-2**: Room creation + joining (who joins first, WebRTC flow)
- **Section 3**: CRDT initialization (snapshot + live delta)
- **Section 4**: BFT ValidatorSet formation (quorum, epoch, proposer rotation)
- **Section 5**: Late joiner catch-up (compact representation, replay)
- **Section 6**: Peer departure (graceful/ungraceful, quorum recalculation)
- **Section 7**: Complete room state machine (8 states, 13 transitions, timeouts)
- **Section 8**: Message types (extended protocol: CRDT, BFT, catch-up, tokens)
- **Section 9-11**: Implementation checklist, rationale, references

**Key Findings**:
- Room ID: 6-char alphanumeric (no server needed)
- Leader: creator (temporary), then BFT proposer rotates per view
- CRDT: hybrid snapshot + live delta (Yjs state vector)
- BFT: ALL peers = validators (up to N=50), quorum = ⌈(N+1)/3⌉
- Late Joiner: state = SYNCING_LATE, receives snapshot + blocks
- Peer Departure: dynamic ValidatorSet recalc, check quorum > old_quorum
- Timeout: 30s signaling, 30s syncing, 15s BFT (exponential backoff on view change)

**Example**: 3-peer room walkthrough (T=0s to T=13s, READY state achieved)

---

### 2. ROOM_STATE_MACHINE.md (8 KB, 250 lines)

**Visual + tabular reference for state transitions:**

- **State Diagram**: ASCII art FSM with 8 states + 6 main transitions
- **State Details Table**: entry condition, actions, exit for each state
- **Timeout Behavior**: 30s SIGNALING, 30s SYNCING, 15s BFT, exponential backoff
- **Message Flow Timeline**: 3-peer room creation (T=0 to T=13)
- **Late Joiner Timeline**: room at block 50, D joins, catch-up applied
- **Peer Departure**: B crashes, quorum recalc from [a,b,c] q=2 to [a,c] q=2
- **Unsafe Condition**: N=4 → 1 peer left → quorum lost → UNSAFE state
- **BFT View Timeout Recovery**: timeout → FROZEN → view change → NEWVIEW → READY
- **TypeScript Guards**: canSendCRDT(), canProposeBFT(), canVoteBFT(), etc.

**Quick Reference**: print and post near desk during implementation.

---

### 3. ROOM_LIFECYCLE_IMPLEMENTATION_PATTERNS.md (14 KB, 450 lines)

**Concrete TypeScript + Rust implementation code:**

- **Section 1**: RoomStateMachine class (state enum, canTransitionTo, transitionTo guards)
- **Section 2**: RoomTimerManager (signaling/syncing/BFT timeouts, exponential backoff)
- **Section 3**: Extended room-state.ts (full integration with FSM, timers, state-aware handlers)
- **Section 4**: CRDT sync (snapshot creation, delta application, vector clocks)
- **Section 5**: BFT ValidatorSet (Rust types, quorum calc, proposer rotation)
- **Section 6**: Late joiner (catch-up request/response, block replay)
- **Section 7**: Peer departure (quorum recalculation, new ValidatorSet broadcast)
- **Section 8**: Updated +page.svelte (UI state display with color dots)

**Ready to Copy**: can be pasted directly into project files (minor adjustments).

---

### 4. 333_ROOM_LIFECYCLE_SUMMARY.md (This File)

**Ties everything together, provides entry points.**

---

## Key Design Decisions (Rationale)

### Q1: Creator = Permanent Leader?
**No.** Creator = temporary initial leader (sends CRDT snapshot), then BFT proposer rotates.
- ✅ Decentralized: no permanent leader
- ✅ Fault-tolerant: creator crash doesn't kill room
- ⚠️ Complexity: but worth it for robustness

### Q2: All Peers = Validators?
**Yes.** N ≤ 50, quorum = ⌈(N+1)/3⌉, no minimum stake.
- ✅ Fair: late joiners become validators immediately
- ✅ Simple: no hierarchical roles
- ✅ Scalable: HotStuff linearity up to 50 peers
- ⚠️ Cost: N=50 → 50 signatures per block (later: BLS aggregation)

### Q3: Snapshot or Live Sync?
**Hybrid: Snapshot + Live Delta.**
- ✅ Snapshot: deterministic, fast, memory-bounded
- ✅ Live: low latency after genesis
- ⚠️ Complexity: two sync modes (but manages well)

### Q4: How to Handle Late Joiners?
**Compact snapshot + committed block replay.**
- ✅ Fast: no need to replay all deltas
- ✅ Memory: epoch compaction every 1000 blocks
- ✓ Verify: check proposer signatures, trust block's validity
- (Future: Merkle tree for proof instead of full replay)

### Q5: Peer Departure = Quorum Recalculation?
**Yes, immediately.** Check: `alive_peers >= old_quorum`.
- ✅ Safe: quorum increases if peers drop
- ✅ Responsive: no waiting for epoch boundary
- ⚠️ Could go UNSAFE if too many leave (then only option is leave room)

---

## Message Protocol (Extended)

### Signaling (WebSocket, stateless relay)
```
join, peers, peer-joined, peer-left
offer, answer, ice
```

### DataChannel (Binary, ordered)
```
CRDT:
  crdt-snapshot, crdt-snapshot-ack, crdt-delta

BFT:
  bft-validator-set, bft-validator-set-change
  bft-prepare, bft-prepare-vote, bft-commit, bft-commit-vote
  bft-view-change, bft-newview

Catch-up:
  catch-up-request, catch-up-response

Token (Future):
  token-tx, token-balance
```

---

## State Machine Quick Reference

### 8 States
| # | State | Meaning |
|---|-------|---------|
| 1 | INIT | Room created, waiting for peers |
| 2 | SIGNALING | Peers exchanging SDP/ICE |
| 3 | SYNCING | DataChannels open, CRDT snapshot + BFT genesis |
| 4 | READY | ✓ Consensus running, CRDT live |
| 5 | UNSAFE | Quorum lost, no consensus possible |
| 6 | FROZEN | BFT timeout, view change in progress |
| 7 | SYNCING_LATE | Late joiner applying catch-up |
| 8 | DISCONNECTED | Terminal: peer left room |

### 6 Main Transitions
1. INIT → SIGNALING: peer joins
2. SIGNALING → SYNCING: DataChannel opens
3. SYNCING → READY: ValidatorSet broadcast
4. READY ↔ FROZEN: BFT timeout / view change recover
5. READY → UNSAFE: quorum lost
6. Any → DISCONNECTED: leave()

### 3 Timeout Behaviors
- **SIGNALING (30s)**: no DataChannel → fail, reconnect
- **SYNCING (30s)**: no ValidatorSet → FROZEN
- **BFT (15s)**: no leader proposal → view change (exponential backoff)

---

## Implementation Path (SPAN_333_Integration Phase)

### Week 1: Foundation
- [ ] Implement RoomStateMachine class (state enum + guards)
- [ ] Add RoomTimerManager (signaling/syncing/BFT timeouts)
- [ ] Extend room-state.ts with FSM integration
- [ ] Add console logging for state transitions
- [ ] Test: 2-peer room reaches READY state

### Week 2: CRDT + BFT
- [ ] Implement CRDT snapshot creation (Yjs)
- [ ] Implement CRDT delta broadcast
- [ ] Implement leader detection (first N peers)
- [ ] Implement BFT ValidatorSet formation
- [ ] Test: 3-peer room forms ValidatorSet

### Week 3: Consensus
- [ ] Wire BFT prepare/vote/commit messages
- [ ] Implement view timeout + exponential backoff
- [ ] Implement view change protocol
- [ ] Test: propose block → collect votes → commit

### Week 4: Advanced
- [ ] Late joiner catch-up (request/response)
- [ ] Peer departure (quorum recalc)
- [ ] E2E test: 4→3→2 safe, 2→1 unsafe
- [ ] E2E test: 2-peer → late joiner join → 3-peer consensus
- [ ] E2E test: BFT timeout → view change → recovery

---

## Critical Path (Must Complete First)

1. **Room State Machine** (blocks everything else)
2. **CRDT Snapshot + Live Sync** (blocks BFT initialization)
3. **BFT ValidatorSet Formation** (blocks consensus)
4. **E2E Test (2-peer → 3-peer → READY)** (validates core flow)

**Then** (optional, can parallelize):
- Late joiner catch-up
- Peer departure handling
- View timeout recovery

---

## Testing Strategy

### Unit Tests
- [ ] State machine: valid/invalid transitions
- [ ] Timeouts: fire at correct intervals
- [ ] Quorum calculation: N → q = ⌈(N+1)/3⌉
- [ ] Vector clocks: increment on CRDT change

### Integration Tests
- [ ] 2-peer room: INIT → SIGNALING → SYNCING → READY (4 steps)
- [ ] 3-peer room: same + third peer joins + ValidatorSet formed
- [ ] Late joiner: room at block 50 → D joins → catch-up + READY
- [ ] Peer crash: [A,B,C] → B crashes → [A,C] with new quorum

### E2E Tests (Browser)
- [ ] Open room1 on browser A (READY)
- [ ] Open room1 on browser B (READY)
- [ ] A places block 1 (CRDT broadcast)
- [ ] B sees block 1
- [ ] A proposes BFT block (both vote) → commit
- [ ] Open room1 on browser C (late joiner, SYNCING_LATE)
- [ ] C receives snapshot + block 1
- [ ] C catches up, reaches READY
- [ ] A, B, C now 3-peer consensus
- [ ] Close B (peer-left broadcast)
- [ ] A, C recalc quorum (still 2 ≥ 2 = safe)
- [ ] A, C continue consensus

---

## Known Limitations

### Not Addressed (Out of Scope)
1. **DHT peer discovery**: requires IPFS/Kademlia (later)
2. **Sybil resistance**: no stake/reputation yet
3. **Byzantine detection**: assume mostly honest peers
4. **Mobile reconnect**: no connection recovery (rejoin only)
5. **BLS aggregation**: individual Ed25519 only (50+ peers later)
6. **Persistence**: state lost on page reload (IndexedDB later)

### Assumptions
- Network: mostly reliable, <30s partitions
- Browser: WebRTC + WebSocket + localStorage/IndexedDB
- Peer count: N ≤ 50 (can scale with optimization)
- Clocks: not synchronized (vector clocks handle)
- Identities: Ed25519 = permanent (no rotation)

---

## File Locations

```
/Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform/

ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md (12 KB)
  └─ Complete spec: creation, joining, sync, BFT, late joiner, departure
  
ROOM_STATE_MACHINE.md (8 KB)
  └─ Visual FSM diagram, state table, timelines, TypeScript guards
  
ROOM_LIFECYCLE_IMPLEMENTATION_PATTERNS.md (14 KB)
  └─ Concrete code: RoomStateMachine, timers, CRDT, BFT, catch-up
  
333_ROOM_LIFECYCLE_SUMMARY.md (this file)
  └─ Executive summary, design rationale, testing strategy

Also reference:
  apt-progress.md — overall 333 Platform status
  WEBRTC_MEMORY_ANALYSIS.md — WebRTC optimization (closure leaks)
```

---

## How to Use These Documents

### During Coding (Week 1-4)
1. **Bookmark ROOM_STATE_MACHINE.md** — state diagram
2. **Copy patterns from ROOM_LIFECYCLE_IMPLEMENTATION_PATTERNS.md** into files
3. **Refer to ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md** section-by-section as you implement each phase

### For Communication
- **"What states exist?"** → ROOM_STATE_MACHINE.md state table
- **"How does late joiner work?"** → ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md section 4
- **"When does ValidatorSet form?"** → ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md section 3.3
- **"Show me the code pattern"** → ROOM_LIFECYCLE_IMPLEMENTATION_PATTERNS.md

### For Validation
- **"Is peer departure correct?"** → ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md section 5 + tests
- **"Check state transitions are safe"** → ROOM_STATE_MACHINE.md FSM diagram
- **"Verify timeout handling"** → ROOM_LIFECYCLE_IMPLEMENTATION_PATTERNS.md RoomTimerManager

---

## Next Steps (Immediate)

1. **Read ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md section 1-6** (30 min)
   - Understand room creation, joining, CRDT sync, BFT formation
   
2. **Study ROOM_STATE_MACHINE.md** (15 min)
   - Internalize state diagram
   
3. **Copy ROOM_LIFECYCLE_IMPLEMENTATION_PATTERNS.md patterns** into project
   - Start with RoomStateMachine + RoomTimerManager
   - Integrate into existing room-state.ts
   
4. **Run unit tests** (day 1-2)
   - Validate state transitions
   - Validate timeouts
   
5. **Build 2-peer E2E test** (day 3-4)
   - INIT → SIGNALING → SYNCING → READY
   - Celebrate first successful state transition!
   
6. **Expand to 3-peer** (day 5-6)
   - Add ValidatorSet formation
   - Add BFT message routing
   
7. **Late joiner + peer departure** (week 2)
   - Once core is solid

---

## KG References

All content is KG-bound to:
- **SPAN_333_RoomLifecycleResearch** (this entire research)
- **SPAN_333_RoomStateMachine** (ROOM_STATE_MACHINE.md)
- **SPAN_333_RoomImplementationPatterns** (ROOM_LIFECYCLE_IMPLEMENTATION_PATTERNS.md)
- **SPAN_333_ConsensusInitialization** (CRDT + BFT sections)
- **lesson-333-modules-not-integrated** (CRITICAL, what we're solving)
- **SPAN_333_Integration** (parent phase)

---

## Conclusion

The 333 Platform has **all the pieces**. This research provides the **blueprint** for assembling them into a working P2P system.

**Key Insight**: A room is not a blob of WebRTC connections. It's a **state machine** with clear transitions, timeouts, and invariants (e.g., "always maintain quorum").

**Bottom Line**:
- ✅ Know what states a room goes through (8 states)
- ✅ Know who sends the first CRDT snapshot (leader/creator)
- ✅ Know when BFT starts (ValidatorSet broadcast)
- ✅ Know how late joiners catch up (snapshot + replay)
- ✅ Know what happens when peers leave (quorum recalc)
- ✅ Ready to implement

**Go build.** Questions? Refer to section numbers in the research docs.

---

*Research complete. Integration phase begins 2026-04-14. KG: work-buffer-2026-04-13-333-room-lifecycle.*
