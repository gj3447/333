# 333 Platform Room Lifecycle Research — Complete Index
# KG: SPAN_333_RoomLifecycleResearch

**Date**: 2026-04-13  
**Status**: Research Complete  
**Total Pages**: 45 (12 + 8 + 14 + 11 pages)  
**Total Lines**: 1,470 lines of specification + code patterns

---

## Quick Navigation

| Document | Size | Purpose | Read Time |
|----------|------|---------|-----------|
| **ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md** | 12 KB | Complete specification of P2P room lifecycle, CRDT+BFT initialization, late joiner, peer departure | 45 min |
| **ROOM_STATE_MACHINE.md** | 8 KB | Visual FSM diagram, state table, timeouts, timelines, TypeScript guards | 20 min |
| **ROOM_LIFECYCLE_IMPLEMENTATION_PATTERNS.md** | 14 KB | Concrete code: state machine, timers, CRDT sync, BFT validators, catch-up, peer departure | 60 min |
| **333_ROOM_LIFECYCLE_SUMMARY.md** | 11 KB | Executive summary, design rationale, testing strategy, next steps | 30 min |

---

## What Problem Do These Documents Solve?

**From lesson-333-modules-not-integrated (CRITICAL)**:
> 333 Platform has 11,290 lines of working modules (CRDT, BFT, Token, WASM). But modules are never connected via P2P. Room lifecycle is undefined. CRDT/BFT initialization missing. Late joiner catch-up unspecified. Zero end-to-end verification.

**These documents answer:**
1. **What happens when peers join a room?** → 8-state FSM (INIT → SIGNALING → SYNCING → READY)
2. **Who sends the first CRDT snapshot?** → Creator (temporary leader), then BFT proposer rotates
3. **How does BFT start?** → ValidatorSet broadcast after N peers have DataChannels open
4. **Can late joiners catch up?** → Yes: compact snapshot + committed block replay (SYNCING_LATE)
5. **What happens when peers leave?** → Quorum recalculation, may go UNSAFE if quorum lost
6. **What are the timeouts?** → 30s signaling, 30s syncing, 15s BFT (exponential backoff on view change)

---

## Document Overview

### ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md
**The Specification. Read this first.**

- **Section 1-2**: Room creation (generator → broadcast) + Peer joining (SDP/ICE exchange)
- **Section 3**: CRDT initialization (hybrid snapshot + live delta, Yjs)
- **Section 4**: BFT ValidatorSet formation (all peers = validators, quorum = ⌈(N+1)/3⌉, proposer rotation)
- **Section 5**: Late joiner catch-up (compact representation, block replay, SYNCING_LATE state)
- **Section 6**: Peer departure (graceful/ungraceful, quorum recalculation, UNSAFE condition)
- **Section 7**: Complete FSM (8 states, 13 transitions, timeout handling, state transition matrix)
- **Section 8**: Extended message protocol (CRDT, BFT, catch-up, tokens)
- **Section 9**: Implementation checklist (7 phases, 4-week timeline)
- **Section 10**: Example walkthrough (3-peer room T=0s to T=30s, B crashes)
- **Section 11**: Known issues, assumptions, optimization opportunities

**Key Sections to Bookmark**:
- Section 1.2: Room state at creation
- Section 3.3: BFT ValidatorSet formation trigger condition
- Section 6: State diagram (ASCII art)
- Section 10: 3-peer walkthrough

---

### ROOM_STATE_MACHINE.md
**The Quick Reference. Keep this open during coding.**

- **State Transition Diagram**: Full FSM with arrows, timeout paths, recovery paths
- **State Details Table**: 8 states with entry/exit conditions
- **Timeout Behavior**: Summary table for all timeouts
- **Message Flow Timeline**: 3-peer room creation (T=0 to T=13)
- **Late Joiner Timeline**: room at block 50 → D joins → catch-up
- **Peer Departure**: [a,b,c] → B crashes → [a,c] with new quorum
- **Unsafe Condition**: How many peers must leave to break quorum
- **BFT View Timeout Recovery**: timeout → view change → newview → READY
- **TypeScript Guards**: 8 guard functions (canSendCRDT, canProposeBFT, canVoteBFT, etc.)
- **Event Handlers**: onMessage, onDCMessage, onTimeout routing

**Use During Implementation**:
- Print the FSM diagram (section 1, ASCII art)
- Implement guards in RoomStateMachine class
- Use message flow timelines to validate your message routing

---

### ROOM_LIFECYCLE_IMPLEMENTATION_PATTERNS.md
**The Code. Copy, paste, adjust.**

- **Section 1**: RoomStateMachine class (state enum, canTransitionTo guards, transitionTo)
- **Section 2**: RoomTimerManager (start/clear signaling/syncing/BFT timeouts, exponential backoff)
- **Section 3**: Extended room-state.ts (full integration: FSM + timers + message routing + peer management)
- **Section 4**: CRDT sync (CRDTSyncManager: snapshot creation, delta application, vector clocks, WASM binding)
- **Section 5**: BFT ValidatorSet (Rust types, quorum calculation, proposer rotation, tests)
- **Section 6**: Late joiner (LateJoinerManager: catch-up request/response, block replay, view setting)
- **Section 7**: Peer departure (PeerDepartureManager: quorum recalculation, new ValidatorSet, UNSAFE detection)
- **Section 8**: Updated +page.svelte (UI state dots: init/signaling/syncing/ready/frozen/unsafe/late/disconnected)

**What to Do**:
1. Copy RoomStateMachine class → src/lib/room-state-machine.ts
2. Copy RoomTimerManager class → src/lib/room-state-timers.ts
3. Update room-state.ts with FSM integration (see section 3)
4. Copy CRDT manager → src/lib/crdt-sync.ts
5. Copy BFT types → src/bft/validator_set.rs
6. Copy late joiner manager → src/lib/late-joiner.ts
7. Copy peer departure manager → src/lib/peer-departure.ts
8. Update +page.svelte with state display

---

### 333_ROOM_LIFECYCLE_SUMMARY.md
**The Entry Point. Start here if you're new.**

- **Problem Statement**: What's broken and why
- **Deliverables**: 4-document overview
- **Key Design Decisions**: 5 Q&A pairs with rationale
- **Message Protocol**: Signaling (WebSocket) + DataChannel (CRDT, BFT, catch-up, token)
- **State Machine Quick Reference**: 8 states, 6 transitions, 3 timeout types
- **Implementation Path**: Week-by-week 4-week plan (foundation → CRDT+BFT → consensus → advanced)
- **Critical Path**: What must be done first (state machine → CRDT → BFT validators → E2E test)
- **Testing Strategy**: Unit + integration + E2E tests
- **Known Limitations**: Out of scope (DHT, Sybil resistance, Byzantine, mobile, BLS, persistence)
- **Next Steps**: 7-day immediate action plan

**Best For**:
- Onboarding new team member
- Executive summary for stakeholders
- 30-second overview

---

## How to Read These (Recommended Order)

### Scenario 1: You're Starting Implementation
1. **Read 333_ROOM_LIFECYCLE_SUMMARY.md** (30 min) — understand the problem
2. **Study ROOM_STATE_MACHINE.md** (20 min) — internalize the FSM
3. **Read ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md section 1-6** (45 min) — understand design
4. **Skim ROOM_LIFECYCLE_IMPLEMENTATION_PATTERNS.md** (30 min) — see what code looks like
5. **Start coding** — copy patterns from section 3 into room-state.ts

### Scenario 2: You're Reviewing Architecture
1. **Read ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md section 9 (design rationale)** (20 min)
2. **Study ROOM_STATE_MACHINE.md** (20 min) — validate transitions
3. **Check ROOM_LIFECYCLE_IMPLEMENTATION_PATTERNS.md section 1** (15 min) — check state guards

### Scenario 3: You're Writing Tests
1. **Read ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md section 10 (example walkthrough)** (20 min)
2. **Check ROOM_STATE_MACHINE.md message flow timelines** (15 min) — see what messages should arrive when
3. **Reference ROOM_LIFECYCLE_IMPLEMENTATION_PATTERNS.md** — use guards in test assertions

### Scenario 4: You're Debugging a Peer Departure Bug
1. **Go to ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md section 5** (15 min) — read peer departure spec
2. **Check ROOM_STATE_MACHINE.md peer departure timeline** (10 min) — trace message flow
3. **Look at ROOM_LIFECYCLE_IMPLEMENTATION_PATTERNS.md section 7** (10 min) — read PeerDepartureManager code

---

## Key Findings Summary

### Room States (8 Total)
```
INIT → SIGNALING → SYNCING → READY ↔ FROZEN
                            ↓      ↓
                       SYNCING_LATE UNSAFE
                            ↓
                        DISCONNECTED
```

### Timeouts
- **SIGNALING**: 30s (no DataChannel → fail)
- **SYNCING**: 30s (no ValidatorSet → freeze)
- **BFT**: 15s (no proposal → view change)
- **View Change Backoff**: 30s → 60s → 120s (exponential)

### Quorum Calculation
```
N peers → quorum = ⌈(N+1)/3⌉
N=2 → q=1
N=3 → q=2
N=4 → q=2
N=5 → q=2
N=50 → q=17
```

### Message Routing
```
WebSocket (Signaling Server, stateless)
  ├─ join → peers list
  ├─ peer-joined → broadcast
  ├─ peer-left → broadcast
  └─ offer/answer/ice → P2P SDP exchange

DataChannel (CRDT + BFT)
  ├─ crdt-snapshot → leader sends initial state
  ├─ crdt-delta → broadcast on every CRDT change
  ├─ bft-validator-set → trigger consensus start
  ├─ bft-prepare/vote/commit → consensus messages
  ├─ bft-view-change/newview → timeout recovery
  └─ catch-up-request/response → late joiner replay
```

---

## Implementation Checklist (7 Phases, 4 Weeks)

### Phase 1: Room State Machine (Week 1)
- [ ] RoomStateMachine class (state enum + guards)
- [ ] RoomTimerManager (timeouts)
- [ ] Extend room-state.ts with FSM
- [ ] Add console logging
- [ ] Test: 2-peer READY

### Phase 2: CRDT Initialization (Week 2)
- [ ] CRDT snapshot creation
- [ ] Leader detection
- [ ] Snapshot broadcast
- [ ] Live delta broadcast
- [ ] Test: 3-peer snapshot applied

### Phase 3: BFT ValidatorSet (Week 2)
- [ ] ValidatorSet formation (N peers → [sorted, quorum, epoch])
- [ ] Broadcast to all
- [ ] State machine: SYNCING → READY
- [ ] Test: 3-peer ValidatorSet epoch 0

### Phase 4: HotStuff Consensus (Week 3)
- [ ] Wire BFT prepare/vote/commit messages
- [ ] Implement view timeout (15s)
- [ ] Implement view change protocol
- [ ] Test: block proposal → votes → commit

### Phase 5: Late Joiner (Week 3)
- [ ] Detect late joiner (peers.size changes)
- [ ] Catch-up request/response
- [ ] Block replay (verify signatures)
- [ ] Test: join at block 50 → catch-up → block 51

### Phase 6: Peer Departure (Week 4)
- [ ] Handle dc.onclose (ungraceful)
- [ ] Handle peer-left message (graceful)
- [ ] Quorum recalculation
- [ ] New ValidatorSet broadcast
- [ ] Check UNSAFE condition
- [ ] Test: [A,B,C] → B leaves → [A,C] safe

### Phase 7: E2E Validation (Week 4)
- [ ] 2-peer consensus
- [ ] 3-peer consensus
- [ ] 4-peer → late joiner (5-peer)
- [ ] Peer crash (5 → 4 safe, 4 → 3 safe, 3 → 2 safe, 2 → 1 unsafe)
- [ ] View timeout → newview recovery

---

## Critical Success Factors

### Must Have (Blocking)
1. **State machine guard checks** — prevent invalid message routing
2. **Quorum recalculation logic** — safety depends on this
3. **BFT timeout** — without it, frozen rooms stay frozen
4. **CRDT snapshot** — without it, new peers get broken state

### Should Have (Quality)
1. **Timeout exponential backoff** — prevents view change storms
2. **Late joiner SYNCING_LATE** — prevents race conditions
3. **Console logging per transition** — debugging visibility
4. **UI state visualization** — user sees progress

### Nice to Have (Optimization)
1. **Epoch compaction** — unbounded history pruning
2. **BLS aggregation** — signature overhead for 50+ peers
3. **Merkle tree proofs** — reduce late joiner data
4. **IndexedDB persistence** — survive page reload

---

## Validation Checklist

- [ ] All 8 states defined
- [ ] All 13 transitions valid (no dead states)
- [ ] All 3 timeout paths covered
- [ ] Message routing guards implemented
- [ ] Quorum calculation correct (test N=2,3,4,50)
- [ ] Late joiner flow works (join → catch-up → READY)
- [ ] Peer departure safe (quorum check before READY)
- [ ] E2E test passes (2-peer → 3-peer → consensus)

---

## Glossary

- **CRDT**: Conflict-free Replicated Data Type (Yjs)
- **BFT**: Byzantine Fault Tolerance (HotStuff)
- **Quorum**: Min votes needed for consensus (⌈(N+1)/3⌉)
- **Validator**: Peer authorized to vote in BFT
- **ValidatorSet**: Set of validators + quorum + epoch
- **Epoch**: Period of fixed validator set
- **Proposer**: Validator proposing next block (rotates per view)
- **View**: Round of consensus (view 0, 1, 2, ...)
- **Lock QC**: Quorum cert for highest locked block
- **Commit**: Block written to ledger (irreversible)
- **Late Joiner**: Peer joining after genesis block
- **Catch-Up**: Snapshot + blocks sent to late joiner
- **Peer Departure**: Peer disconnect (graceful or timeout)
- **UNSAFE**: State when quorum lost (no consensus possible)

---

## Cross-References

### lesson-333-modules-not-integrated
- **Solved by**: All 4 documents
- **Evidence**: Sections 1-6 of ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md show how to wire modules

### SPAN_333_Integration (parent)
- **Decomposed into**: 4 AtomicSpans (MemFix, CrdtSync, ConsensusNet, E2E)
- **MemFix**: WEBRTC_MEMORY_ANALYSIS.md (closure leak fixes)
- **CrdtSync**: Section 3-5 of ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md
- **ConsensusNet**: Section 7 + ROOM_LIFECYCLE_IMPLEMENTATION_PATTERNS.md
- **E2E**: Section 10 (example walkthrough)

### apt-progress.md
- **Context**: 333 Platform at Integration Phase (SA complete)
- **Next**: Use these docs for SP (SemanticPyramid) decomposition

---

## Document Maintenance

**Last Updated**: 2026-04-13  
**Next Review**: After implementation starts (2026-04-14)  
**Owner**: KG work-buffer-2026-04-13-333-room-lifecycle  
**Status**: COMPLETE (no changes until implementation feedback)

---

## Questions?

**Q: "What's the difference between SYNCING and SYNCING_LATE?"**  
A: SYNCING = first peers syncing snapshot + forming genesis ValidatorSet. SYNCING_LATE = peer joining after genesis, needs catch-up (block replay).

**Q: "What happens if all peers are FROZEN at the same time?"**  
A: All peers increment view, broadcast VIEW_CHANGE, accumulate quorum of 2/3 view changes, one becomes new leader, broadcasts NEWVIEW, all transition to READY. (See ROOM_STATE_MACHINE.md BFT View Timeout Recovery section)

**Q: "Can a room go from UNSAFE back to READY?"**  
A: Only if new peers join and quorum > old_quorum. Otherwise, only option is leave(). (See ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md section 5.3)

**Q: "Do late joiners participate in BFT immediately?"**  
A: Yes, after catch-up completes and ValidatorSet is applied, they're full validators in next view. (See section 5)

**Q: "How long can block replay take?"**  
A: O(number of blocks). With 1000-block epochs, late joiner can rejoin at any epoch boundary. (See section 5.3, epoch compaction)

---

*Research complete. Integration begins 2026-04-14. Ask for clarification during implementation.*
