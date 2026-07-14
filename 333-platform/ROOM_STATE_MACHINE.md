# 333 Platform: Room Lifecycle State Machine (Visual Reference)
# KG: SPAN_333_RoomStateMachine

> Quick reference for room state transitions. Full context: ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md

---

## State Transition Diagram (Full)

```
                                ┌─────────┐
                                │  INIT   │ (room created, creator only)
                                └────┬────┘
                                     │
                            peer join │
                                     ▼
                            ┌──────────────────┐
                            │   SIGNALING      │ (SDP/ICE exchange)
                            │ (dc opening...) │
                            └────────┬─────────┘
                                     │
                          dc.onopen  │
                                     ▼
           ╔═════════════════════════════════════════════════════╗
           ║             SYNCING                                 ║
           ║  ┌─ CRDT snapshot/delta broadcast                 ║
           ║  ├─ BFT genesis (form validator set)              ║
           ║  └─ Timeout 30s → FROZEN                          ║
           ╚═════════════════════════════════════════════════════╝
                                     │
                      validator-set │ broadcast
                      (peers ≥ 3)  │ received
                                     ▼
           ╔═════════════════════════════════════════════════════╗
           ║             READY                                   ║
           ║  ✓ CRDT live (broadcast delta on change)           ║
           ║  ✓ BFT consensus (HotStuff proposer+votes)         ║
           ║  ✓ Peer can propose, sign, vote, commit            ║
           ║                                                     ║
           ║  ↕ timeout (15s) → FROZEN                          ║
           ║  ↕ peer gone → recalc validatorSet                 ║
           ║  ↕ quorum lost → UNSAFE                            ║
           ║  ↕ peer joins (late) → SYNCING_LATE                ║
           ║  ↕ leave() → DISCONNECTED                          ║
           ╚═════════════════════════════════════════════════════╝
                    │              │               │
       ┌────────────┼──────────────┼───────────────┼─────────────┐
       │            │              │               │             │
       ▼            ▼              ▼               ▼             ▼
  UNSAFE      FROZEN      SYNCING_LATE      quorum OK?      DISC
  (no         (view       (late joiner      (recalc)
   quorum)     change)    catch-up)          │
                  │            │             ├→ READY
                  │            │             │
                  ▼            ▼             ▼ UNSAFE
            newview→       apply→
            READY          READY

   (only leave()
    available)
```

---

## State Details Table

| State | Entry Condition | Actions | Exit Condition |
|-------|-----------------|---------|----------------|
| **INIT** | Room created | Wait for peers | First peer joins |
| **SIGNALING** | Peer joins, peers list received | Exchange SDP/ICE | All DataChannels open |
| **SYNCING** | DataChannel.onopen | Send CRDT snapshot, form BFT genesis | ValidatorSet broadcast received |
| **READY** | ValidatorSet received | CRDT live broadcast, BFT consensus | Timeout / Peer lost / Quorum lost / Late joiner |
| **UNSAFE** | Quorum lost (N peers < quorum) | UI warn user | Leave room |
| **FROZEN** | BFT timeout (15s no consensus) | Initiate view change | Receive newview |
| **SYNCING_LATE** | Peer joins at T > T_genesis | Request catch-up, apply snapshot+blocks | Validator set established |
| **DISCONNECTED** | leave() called | Cleanup (close sockets, clear state) | N/A (terminal) |

---

## Timeout Behavior

```
State         | Timeout    | Condition | Action | Next State
──────────────┼────────────┼───────────┼────────┼───────────
SIGNALING     | 30s        | No dc.open| fail   | INIT (reconnect)
SYNCING       | 30s        | No valSet | freeze | FROZEN
READY         | 15s        | No leader| view   | FROZEN
              |            | proposal | change |
FROZEN        | expo 30→60 | No newview| unsafe| UNSAFE
              | →120s      |           |       |
```

---

## Message Flow: Creating 3-Peer Room (Timeline)

```
T=0    A: create room "abc123" → INIT
           ├─ ws.send(join)
           └─ state = INIT
       
T=2    B: join "abc123"
           ├─ ws.send(join)
           └─ state = SIGNALING
           
       A: rcv peer-joined
           ├─ createOffer() → B
           └─ state = SIGNALING
           
       B: rcv peers list
           └─ state = SIGNALING

T=4    A ↔ B: SDP exchange (offer/answer)
       A ↔ B: ICE candidates

T=6    dc.onopen (A→B)
       A: state = SYNCING
       B: state = SYNCING

T=7    A: crdt.toSnapshot()
           broadcast CRDT_SNAPSHOT
           
       B: applySnapshot()
           broadcast SNAPSHOT_ACK

T=8    C: join "abc123"
           ├─ ws.send(join)
           └─ state = SIGNALING
           
       A, B: rcv peer-joined → C
             createOffer() to C

       C: rcv peers list [a, b]
           state = SIGNALING

T=12   C: dc.onopen
        C: state = SYNCING
        
       A: peers.size == 3 && allDCOpen
          form ValidatorSet([a,b,c], q=2)
          broadcast VALIDATOR_SET
          
       B, C: rcv VALIDATOR_SET
             state = READY

T=13   A, B, C: READY ✓
       
       A: propose block_0
          broadcast PREPARE
          
       B, C: vote
             broadcast PREPARE_VOTE
             
       A: qc = collectVotes(2)
          broadcast COMMIT
          
       A, B, C: block_0 = COMMITTED
```

---

## Late Joiner Catch-Up (T=100)

```
T=100  Room state:
       - A, B, C at block 50 (committed)
       - ValidatorSet epoch 1

       D: join "abc123"
           state = SIGNALING
           
       A: rcv peer-joined → D
          createOffer() to D
          
       D: rcv peers [a,b,c]
          ├─ createPeerConn with A, B, C
          └─ state = SIGNALING
          
T=104  D: dc.onopen (with A, B, C)
           state = SYNCING_LATE (auto-detect: peers.size jumped, my block < max)
           
           broadcast CATCH_UP_REQUEST

       A: rcv CATCH_UP_REQUEST from D
          ├─ snapshot = crdt.snapshot()
          ├─ blocks = bft.committedBlocks[0:50]
          ├─ valSet = current_validator_set
          └─ send CATCH_UP_RESPONSE
          
       D: rcv CATCH_UP_RESPONSE
          ├─ crdt.applySnapshot(snap)
          ├─ for block in blocks:
          │    verify(block.sig)
          │    crdt.applyTxs(block)
          ├─ bft.setValidators(valSet)
          └─ state = READY
```

---

## Peer Departure (T=150, B Crashes)

```
T=150  Room state:
       A, B, C: READY, block 100 committed
       ValidatorSet([a,b,c], q=2)

       B: (network partition, no action)

       A, C: DataChannel.onclose() after 30s ICE timeout
             (T=180)
             
T=180  A: peers.delete(b)
          peers = {a, c}
          recalculateValidatorSet()
          ├─ new_quorum = ceil((2+1)/3) = 1
          ├─ alive.size (2) ≥ old_quorum (2) ✓
          └─ state = READY
          
          broadcast VALIDATOR_SET_CHANGE([a,c], q=1, epoch=2)
          
       C: rcv VALIDATOR_SET_CHANGE
          ├─ bft.setValidators([a,c], q=1, epoch=2)
          └─ state = READY
          
       A, C: Continue consensus with N=2
             (single peer can commit: q=1 includes self + 0 others)
```

---

## Unsafe Condition (All Peers Leave Except 1)

```
T=200  A, B, C, D: READY, ValidatorSet([a,b,c,d], q=3)

       B, C, D: crash/leave

T=230  A: peers = {a}
          quorum = 3
          alive = 1 < quorum ✗
          
          state = UNSAFE
          UI: "Not enough peers. Room is unsafe."
          
       A: only option:
          ├─ wait for peers to rejoin, or
          └─ leave()
          
       (No consensus possible, locked out)
```

---

## BFT View Timeout Recovery (T=50)

```
T=50   A, B, C: READY
       Leader A: should propose every ~3s
       (no proposal for 15s → view timeout)

T=65   B, C: BFT timeout (15s passed)
        ├─ increment view (0 → 1)
        ├─ broadcast VIEW_CHANGE(view=1)
        └─ state = FROZEN
        
       A: (might be slow, didn't notice yet)

T=66   B, C: rcv VIEW_CHANGE from each other
          ├─ accumulate 2 ≥ quorum (2)
          ├─ new leader = view % N = 1 % 3 = B
          └─ B: create NEWVIEW(view=1)
               broadcast NEWVIEW
               
       A: (still unaware)
       C: rcv NEWVIEW from B
          state = READY
          
T=67   A: finally detects timeout
       A: rcv VIEW_CHANGE/NEWVIEW
          ├─ update bft.view = 1
          ├─ leader = B
          └─ state = READY
          
       (B proposes next block as new leader)
       
       View change success. Exponential backoff: next timeout = 30s
```

---

## TypeScript State Guard Example

```typescript
interface RoomStateGuard {
  canSendCRDT(): boolean {
    return state === SYNCING || state === READY || state === SYNCING_LATE;
  }
  
  canProposeBFT(): boolean {
    return state === READY && amProposer();
  }
  
  canVoteBFT(): boolean {
    return state === READY || state === FROZEN; // vote in view change too
  }
  
  canReceiveSnapshot(): boolean {
    return state === SYNCING || state === SYNCING_LATE;
  }
  
  canChangeState(from: State, to: State): boolean {
    const transitions: Record<State, State[]> = {
      INIT: [SIGNALING],
      SIGNALING: [SYNCING, INIT],
      SYNCING: [READY, FROZEN, SYNCING_LATE, INIT],
      READY: [FROZEN, UNSAFE, SYNCING_LATE, DISCONNECTED],
      FROZEN: [READY, UNSAFE, DISCONNECTED],
      UNSAFE: [DISCONNECTED],
      SYNCING_LATE: [READY, INIT],
      DISCONNECTED: []
    };
    return transitions[from]?.includes(to) ?? false;
  }
  
  transitionTo(nextState: State) {
    if (!this.canChangeState(state, nextState)) {
      throw new Error(`Invalid: ${state} → ${nextState}`);
    }
    state = nextState;
    console.log(`[${roomId}] ${state}`);
  }
}
```

---

## Room Lifecycle Summary (What Each Peer Sees)

### Creator's View
```
INIT (alone)
  → SIGNALING (B joins)
  → SYNCING (send snapshot, form ValidatorSet)
  → READY (consensus begins)
     ↕ (FROZEN if timeout, back to READY on newview)
     ↕ (SYNCING_LATE if C joins late)
  → DISCONNECTED (leave)
```

### Joiner's View (Normal)
```
(outside)
  → SIGNALING (receive peers list, exchange SDP/ICE)
  → SYNCING (dc opens, receive snapshot)
  → READY (receive ValidatorSet)
     ↕ (same as creator)
  → DISCONNECTED (leave)
```

### Late Joiner's View
```
(outside)
  → SIGNALING (receive peers list)
  → SYNCING_LATE (send catch-up request, receive snapshot+blocks)
  → READY (apply blocks, join consensus)
     ↕ (same as creator)
  → DISCONNECTED (leave)
```

---

## Quick Reference: Event Handlers

```typescript
// onMessageFromSignaling
onMessage(msg):
  if (msg.type === 'peer-joined'):
    createOffer() → send offer
    state = SIGNALING
  elif (msg.type === 'peers'):
    for each peer in msg.peers:
      createPeerConnection()
    state = SIGNALING
  elif (msg.type === 'peer-left'):
    peers.delete(msg.peerId)
    recalculateValidatorSet()
    state = (quorumOK() ? READY : UNSAFE)

// onDataChannelMessage
onDCMessage(msg):
  if (msg.type === 'crdt-snapshot'):
    applySnapshot(msg.snap)
  elif (msg.type === 'crdt-delta'):
    applyDelta(msg.delta)
  elif (msg.type === 'bft-validator-set'):
    setValidators(msg.set)
    state = READY
  elif (msg.type === 'bft-prepare'):
    vote(msg.block) → send prepare-vote
  elif (msg.type === 'bft-commit'):
    commit(msg.lockQC)
  elif (msg.type === 'bft-view-change'):
    accumulateViewChange() → newview if quorum
  elif (msg.type === 'catch-up-request'):
    sendSnapshot + committedBlocks

// Timeouts
onSignalingTimeout(30s):
  if (state === SIGNALING): state = INIT; reconnect()

onSyncingTimeout(30s):
  if (state === SYNCING): state = FROZEN

onBFTTimeout(15s):
  if (state === READY): incrementView(); state = FROZEN
```

---

*For full context, see ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md*
