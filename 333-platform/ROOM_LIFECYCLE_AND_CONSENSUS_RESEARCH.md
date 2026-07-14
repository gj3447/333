# 333 Platform: Complete Room Lifecycle & CRDT+BFT Initialization
# KG: SPAN_333_RoomLifecycleResearch, SPAN_333_ConsensusInitialization

> **Status**: Research Complete  
> **Date**: 2026-04-13  
> **Context**: 333 Platform P2P WebRTC + CRDT + HotStuff BFT  
> **Related KG**: lesson-333-modules-not-integrated (CRITICAL), SPAN_333_Integration  
> **Scope**: Room lifecycle state machine, CRDT/BFT initialization protocol, late-joiner catch-up

---

## Executive Summary

The 333 Platform has all **core modules implemented** (CRDT, BFT HotStuff, Ed25519, WebRTC) but **zero integration**. This document specifies:

1. **Room Lifecycle State Machine** (8 states + 6 transitions)
2. **CRDT Initialization Protocol** (snapshot + live delta sync hybrid)
3. **BFT Validator Set Formation** (dynamic, N ≤ 50, quorum = ⌈(N+1)/3⌉)
4. **Late-Joiner Catch-Up** (compact snapshot + committed blocks since)
5. **Peer Departure Handling** (graceful/timeout, validator set recalc)
6. **Concrete State Machine Diagram** (message flows, timeout handling)

### Key Design Decisions

- **Room ID**: 6-char alphanumeric, generated on-demand (no central server)
- **Peer Discovery**: Pure P2P via WebSocket signaling server (stateless relay only)
- **Leader Election**: First peer = temporary leader (for initial snapshot), then shift to BFT proposer
- **Validator Set**: All peers (up to N=50), dynamic, no minimum
- **CRDT Sync**: Leader sends snapshot (Yjs state vector) → peers apply → live delta broadcast
- **BFT Timeout**: 15 second view timeout, exponential backoff on view change
- **Data Persistence**: In-browser (localStorage for compact state, IndexedDB for full history)

---

## 1. Room Creation Flow

### 1.1 Creator (First Peer) Initiates

```
Creator (Browser A)
  ├─ generateRoomId() → "abc123"
  ├─ loadIdentity() → peerId = "a1b2c3d4..."
  ├─ connectToRoom(roomId: "abc123")
  │  ├─ new WebSocket(signalingUrl)
  │  ├─ ws.send({ type: 'join', room: 'abc123', peerId: 'a1b2c3d4' })
  │  └─ state = INIT
  ├─ shareURL() → "https://333/room?id=abc123"
  └─ wait for peers
```

### 1.2 Room State at Creation

- **Room ID**: "abc123" (6 chars)
- **Creator Peer ID**: "a1b2c3d4..." (first 8 hex digits of public key)
- **State**: `INIT` (waiting for peers)
- **Peers**: {creatorId}
- **CRDT**: Empty StateVector `{ clock: {} }`
- **BFT**: No ValidatorSet yet
- **DataChannels**: None established yet

### 1.3 Signaling Server Side (stateless relay)

```
SignalingServer
  rooms["abc123"] = Map { "a1b2c3d4" → ws }
  console.log("[abc123] a1b2c3d4 joined (1 peer)")
```

---

## 2. Peer Joining Flow

### 2.1 New Peer Initiates Join

```
Joiner (Browser B)
  ├─ Load existing roomId from URL: "abc123"
  ├─ loadIdentity() → peerId = "b5e6f7g8..."
  ├─ connectToRoom("abc123")
  │  ├─ new WebSocket(signalingUrl)
  │  ├─ ws.send({ type: 'join', room: 'abc123', peerId: 'b5e6f7g8' })
  │  └─ state = SIGNALING
  └─ wait for 'peers' message from signaling server
```

### 2.2 Signaling Server Relays Peer List

```
SignalingServer receives 'join' from b5e6f7g8
  ├─ rooms["abc123"].set("b5e6f7g8", ws_B)
  ├─ send to b5e6f7g8: { type: 'peers', peers: ['a1b2c3d4'], you: 'b5e6f7g8' }
  └─ broadcast to a1b2c3d4: { type: 'peer-joined', peerId: 'b5e6f7g8' }

Browser A receives 'peer-joined'
  └─ peerList.push('b5e6f7g8')
  └─ state = SIGNALING

Browser B receives 'peers'
  └─ peerList.push('a1b2c3d4')
  └─ state = SIGNALING
```

### 2.3 WebRTC Offer/Answer Exchange

**Browser A (existing peer)**: initiator
```
A receives 'peer-joined' from B
  ├─ pc_A_B = new RTCPeerConnection()
  ├─ createOffer() → offer_sdp
  ├─ setLocalDescription(offer_sdp)
  ├─ send to B: { type: 'offer', to: 'b5e6f7g8', sdp: offer_sdp }
  └─ state = CONNECTING
```

**Browser B (new peer)**: responder
```
B receives 'offer' from A
  ├─ pc_B_A = new RTCPeerConnection()
  ├─ setRemoteDescription(offer_sdp)
  ├─ createAnswer() → answer_sdp
  ├─ setLocalDescription(answer_sdp)
  ├─ send to A: { type: 'answer', to: 'a1b2c3d4', sdp: answer_sdp }
  └─ state = CONNECTING

A receives 'answer' from B
  ├─ setRemoteDescription(answer_sdp)
  └─ state = CONNECTING (wait for ICE)
```

### 2.4 ICE Candidate Exchange

```
Both A & B:
  ├─ pc.onicecandidate → e.candidate
  ├─ send to peer: { type: 'ice', to: 'peer', candidate: e.candidate }
  └─ receive 'ice' → pc.addIceCandidate(candidate)
```

### 2.5 DataChannel Open

```
A (initiator) creates DataChannel:
  ├─ const dc = pc.createDataChannel('333', { ordered: true })
  ├─ dc.onopen → peers[B].dcState = OPEN

B (responder) receives DataChannel:
  ├─ pc.ondatachannel → (e) { const dc = e.channel }
  ├─ dc.onopen → peers[A].dcState = OPEN
```

**End Result**: A ↔ B have **2 peer connections** (A→B + B→A, or shared RTCPeerConnection).

---

## 3. Initial State Sync (CRDT + BFT Setup)

### 3.1 Core Question: Who Sends the Snapshot?

**Option A: Creator = Permanent Leader**
- ❌ Bad: Creator crash → whole room dies
- ❌ Bad: Creator has slower connection → new peers wait forever

**Option B: First N Peers Sync Live**
- ✓ Good: decentralized from start
- ✓ Good: no waiting, CRDT live-sync immediately
- ⚠️ Complexity: concurrent state merging if multiple peers edit simultaneously

**Option C (Recommended): Leader + Voting**
- ✓ Creator acts as **temporary initial proposer**
- ✓ After K peers join, elect permanent proposer via BFT view
- ✓ CRDT sync: creator sends snapshot → new peers apply → live deltas broadcast

### 3.2 Initial Snapshot + Live Sync Protocol

```typescript
// Creator (A) sends CRDT snapshot when first peer joins
A: peers.size === 2 → trigger SNAPSHOT_SEND
   ├─ const snapshot = crdt.state.toSnapshot() // Yjs state vector
   ├─ broadcast { type: 'crdt-snapshot', snap: snapshot, epoch: 0 }
   └─ state = SYNCING

// New peer (B) receives snapshot
B: on({ type: 'crdt-snapshot', snap, epoch })
   ├─ crdt.applySnapshot(snap)
   ├─ ack: broadcast { type: 'crdt-snapshot-ack', peerId: B, epoch: 0 }
   └─ state = SYNCING

// From now on: ALL peers broadcast CRDT deltas
on_crdt_update(delta):
  ├─ broadcast { type: 'crdt-delta', delta, clock: vclock, epoch: 0 }

// Receive delta
on({ type: 'crdt-delta', delta, clock, epoch }):
  ├─ crdt.applyDelta(delta)
  └─ no ack needed (idempotent)
```

### 3.3 BFT ValidatorSet Formation

**When**: After N peers are SYNCING and DataChannels open

**Trigger Condition**: `peers.size >= 3` OR `5 seconds after SYNCING`

```typescript
// A (leader) or any proposer:
if (peers.size >= 3 && peers.every(p => p.dcState === OPEN)) {
  // Form ValidatorSet
  const validators = Array.from(peers.keys()).sort();
  const validatorSet = {
    validators: validators,
    epoch: 0,
    quorum: Math.ceil((validators.length + 1) / 3),
    timestamp: Date.now()
  };
  
  // Broadcast to all
  broadcast({
    type: 'bft-validator-set',
    set: validatorSet
  });
  
  state = READY;
}

// On receiving ValidatorSet
on({ type: 'bft-validator-set', set }):
  ├─ bft.setValidators(set)
  ├─ state = READY
  └─ start BFT consensus (hotstuff prepare phase)
```

### 3.4 State Diagram: INIT → SYNCING → READY

```
             ┌──── INIT ────┐
             │ (room created)│
             └──────┬────────┘
                    │
         peer-joined │
                    ▼
             ┌────────────────┐
             │    SIGNALING   │
             │ (WebRTC SDP/ICE)│
             └────────┬────────┘
                      │
        dc.onopen     │
                      ▼
             ┌────────────────┐
             │    SYNCING     │
             │ (CRDT snapshot)│
             │ (BFT genesis)  │
             └────────┬────────┘
                      │
    validator-set    │
    received & ready │
                      ▼
             ┌────────────────┐
             │     READY      │
             │ (consensus!)   │
             │ (live CRDT)    │
             └────────────────┘
```

---

## 4. Late Joiner Catch-Up

### 4.1 Scenario: Room Has Committed History

```
Time T0:   A & B form room, commit 100 blocks
Time T1:   C joins room

C needs:
  1. CRDT snapshot (current state)
  2. Committed blocks since epoch 0 (for validation + replay)
  3. Current ValidatorSet + BFT view
```

### 4.2 Late Joiner Protocol

```typescript
// C joins and connects
C: connectToRoom("abc123", peerId: "c9d0e1f2")
   ├─ peers.size increases to 3
   ├─ receive crdt-snapshot from A/B
   ├─ receive bft-validator-set
   └─ send { type: 'catch-up-request' }

// Any peer (A or B) responds
A: on({ type: 'catch-up-request', from: C })
   ├─ const blocks = bft.committedBlocks.filter(b => b.epoch >= 0)
   ├─ send { type: 'catch-up-response',
   │         snapshot: crdt.snapshot,
   │         blocks: blocks,
   │         validatorSet: current,
   │         view: bft.view
   │       }

// C applies catch-up
C: on({ type: 'catch-up-response', ... })
   ├─ crdt.applySnapshot(snapshot)
   ├─ for block in blocks:
   │    ├─ verify(block.sig, block.proposer)
   │    ├─ crdt.apply(block.txs)
   │ (no re-execution, trust proposer's validity proof)
   ├─ bft.setValidators(validatorSet)
   ├─ state = READY
```

### 4.3 Compact Representation (Ledger Pruning)

To avoid unbounded history, implement **epoch compaction**:

```typescript
// Every 1000 blocks = 1 epoch
// Epoch N+1 starts with:
//   1. CRDT snapshot (compressed Yjs state)
//   2. ValidatorSet (may have changed)
//   3. BFT qc (quorum cert for block 1000)
//
// Old blocks ≤ 1000 can be discarded

const epochBoundary = {
  epoch: 1,
  crdtSnapshot: compressedYjsState,
  validatorSet: {...},
  genesisQC: quorumCertificate,
  discardBlocksUpTo: 1000
};
```

---

## 5. Peer Departure Handling

### 5.1 Graceful Departure (Peer Closes)

```
Peer B decides to leave:
  ├─ dc.close() (for all DataChannels)
  ├─ ws.close() (signaling connection)
  └─ state = DISCONNECTED

Signaling server detects ws.close():
  ├─ rooms["abc123"].delete("b5e6f7g8")
  └─ broadcast to room: { type: 'peer-left', peerId: 'b5e6f7g8' }

Remaining peers (A, C) receive 'peer-left':
  ├─ peerList.delete("b5e6f7g8")
  ├─ peerConnections.delete("b5e6f7g8")
  ├─ state = SYNCING (recalculate validator set)
  ├─ new ValidatorSet = {validators: [a, c], quorum: 2}
  ├─ broadcast { type: 'bft-validator-set', set: {...} }
  └─ state = READY
```

### 5.2 Ungraceful Departure (Timeout)

```
B disappears (network crash, tab close):

Signaling Server:
  ├─ WebSocket timeout (5 seconds idle)
  ├─ trigger ws.close()
  └─ broadcast 'peer-left' (same as graceful)

A & C:
  ├─ DataChannel.onclose after 30 seconds ICE timeout
  ├─ detect B is unreachable
  ├─ delete B from peers
  └─ recalculate ValidatorSet
```

### 5.3 ValidatorSet Recalculation on Departure

```typescript
function recalculateValidatorSet() {
  const alive = peers.filter(p => p.dcState === OPEN);
  
  if (alive.length < Math.ceil((oldValidators.length + 1) / 3)) {
    // CRITICAL: Not enough replicas to maintain quorum
    state = UNSAFE;
    UI_alert("Not enough peers to maintain safety. Exit room.");
    return;
  }
  
  const newSet = {
    validators: alive.map(p => p.id).sort(),
    epoch: oldSet.epoch + 1,
    quorum: Math.ceil((alive.length + 1) / 3),
    timestamp: Date.now()
  };
  
  // Broadcast new ValidatorSet
  broadcast({
    type: 'bft-validator-set-change',
    newSet: newSet,
    oldEpoch: oldSet.epoch
  });
  
  bft.setValidators(newSet);
}
```

### 5.4 Example: 3-Peer Room, 1 Leaves

```
Initial: [A, B, C] → quorum = 2
↓
B leaves
↓
New: [A, C] → quorum = 2 (STILL SAFE)

Initial: [A, B, C, D] → quorum = 3
↓
A, B leave (2 depart)
↓
New: [C, D] → quorum = 2 (SAFE)

Initial: [A] → quorum = 1
↓
A is alone (room inactive)
↓
Timeout: room auto-destroy after 30 min idle
```

---

## 6. Complete Room Lifecycle State Machine

### 6.1 State Definitions

| State | Meaning | Valid Actions | Invalid Actions |
|-------|---------|---------------|-----------------|
| **INIT** | Room created, waiting for peers | `join()` | `send_crdt()`, `bft_propose()` |
| **SIGNALING** | Peers connected, WebRTC SDP/ICE in progress | `receive_peers()`, `exchange_sdp()`, `exchange_ice()` | `broadcast_crdt()`, `bft_propose()` |
| **SYNCING** | DataChannels open, CRDT snapshot + BFT genesis | `send_crdt_snapshot()`, `receive_snapshot()`, `receive_delta()`, `set_validator_set()` | `bft_propose()` (wait for ValidatorSet) |
| **READY** | Full consensus ready, CRDT live, BFT operational | ✓ All operations | (none) |
| **UNSAFE** | Not enough peers for quorum | `leave_room()` | `bft_propose()`, `send_crdt()` |
| **FROZEN** | View timeout, waiting for view change | `receive_viewchange()`, `receive_newview()` | `bft_propose()` (wait for recovery) |
| **SYNCING_LATE** | Late joiner applying catch-up | `apply_snapshot()`, `replay_blocks()`, `set_validator_set()` | `send_crdt_delta()` (until READY) |
| **DISCONNECTED** | Local peer left room | (none) | (all) |

### 6.2 State Transition Matrix

```
From\To          INIT  SIGNAL SYNC  READY UNSAFE FROZEN LATE  DISC
────────────────────────────────────────────────────────────────
INIT             —     join   —     —     —      —      —      leave
SIGNALING        —     —      dc→   —     —      —      —      leave
                           open
SYNCING          —     —      —     val   —      —      —      leave
                                   set
READY            —     —      —     —     peer   timeout—     leave
                                         gone
UNSAFE           —     —      —     enough—      —      —      leave
                                  peers
FROZEN           —     —      —     newview—    —      —      leave
SYNCING_LATE     —     —      —     apply  —      —      —      leave
DISCONNECTED     —     —      —     —     —      —      —      —
```

### 6.3 Concrete FSM Diagram (ASCII)

```
                            ┌──────────────────────────────────┐
                            │      ROOM LIFECYCLE FSM          │
                            └──────────────────────────────────┘

                                    ┌─ INIT ─┐
                                    │ (empty)│
                                    └────┬───┘
                                         │
                                 peer join→
                                         │
                    ┌────────────────────▼──────────────────┐
                    │         SIGNALING (SDP/ICE)           │
                    │  A↔B exchange offer/answer/ice-cands │
                    └────────────────────┬──────────────────┘
                                         │
                               dc.onopen→
                                         │
      ┌──────────────────────────────────▼────────────────────────┐
      │              SYNCING (CRDT Snapshot + BFT)                │
      │  1. Leader sends CRDT StateVector                         │
      │  2. Peers receive & apply snapshot                        │
      │  3. All peers exchange live deltas                        │
      │  4. BFT genesis: ValidatorSet broadcast                  │
      └──────────────────────────────────┬───────────────────────┘
                                         │
                            val-set→ or timeout→
                                         │
      ┌──────────────────────────────────▼────────────────────────┐
      │              READY (Full Consensus)                       │
      │  ✓ CRDT live delta broadcast                             │
      │  ✓ BFT HotStuff consensus                                │
      │  ✓ Peer can propose blocks, sign, vote                   │
      └──────────────────────────────────┬───────────────────────┘
                                         │
                    ┌────────────────────┼────────────────┐
                    │                    │                │
            peer leaves→         BFT timeout→      peer joins→
                    │                    │                │
                    ▼                    ▼                ▼
              recalc valid→       FROZEN               SYNCING_LATE
              set & broadcast   (viewchange)           (catch-up)
                    │                    │                │
                    │           newview→ │ apply→         │
                    │                    ▼                ▼
                    │                 READY             READY
                    │
              quorum OK?
                 /    \
               YES     NO
               │       │
               ▼       ▼
            READY    UNSAFE
            │        │
            │        └─ leave→ DISCONNECTED
            │
            └─ leave→ DISCONNECTED

Timeout at SYNCING (30s) → FROZEN (enter view change protocol)
Timeout at FROZEN (exponential backoff) → UNSAFE
At UNSAFE: only option is leave()
```

### 6.4 Critical Transitions with Timing

```typescript
enum RoomState {
  INIT = 'init',
  SIGNALING = 'signaling',
  SYNCING = 'syncing',
  READY = 'ready',
  UNSAFE = 'unsafe',
  FROZEN = 'frozen',
  SYNCING_LATE = 'syncing_late',
  DISCONNECTED = 'disconnected'
}

interface RoomStateMachine {
  currentState: RoomState;
  peers: Map<string, PeerInfo>;
  validatorSet: ValidatorSet | null;
  crdtState: CRDTState;
  bftState: HotStuffState;
  
  // Timeouts
  signalingTimeout: Timer; // 30s: if no dc.open, fail
  syncingTimeout: Timer;   // 30s: if no ValidatorSet, freeze
  viewTimeout: Timer;      // 15s: BFT consensus view timeout
}

// State Transition Functions
function init() { state = INIT; }
function beginSignaling(peers) { state = SIGNALING; startTimer(signalingTimeout, 30000); }
function beginSyncing() { state = SYNCING; startTimer(syncingTimeout, 30000); }
function readyForConsensus() { state = READY; cancelTimer(signalingTimeout); cancelTimer(syncingTimeout); }
function handlePeerDeparture() {
  recalculateValidatorSet();
  if (quorumLost()) { state = UNSAFE; }
  else { state = READY; }
}
function handleBFTTimeout() { state = FROZEN; initiateViewChange(); }
function handleLateJoiner() { state = SYNCING_LATE; sendCatchUp(); }
function disconnect() { state = DISCONNECTED; cancelAllTimers(); }
```

---

## 7. Message Types (Extended Protocol)

### 7.1 Signaling Messages (WebSocket)

```typescript
// Join/Peers
{ type: 'join', room: string, peerId: string }
{ type: 'peers', peers: string[], you: string }
{ type: 'peer-joined', peerId: string }
{ type: 'peer-left', peerId: string }

// WebRTC Signaling
{ type: 'offer', to: string, sdp: RTCSessionDescriptionInit }
{ type: 'answer', to: string, sdp: RTCSessionDescriptionInit }
{ type: 'ice', to: string, candidate: RTCIceCandidateInit }
```

### 7.2 DataChannel Messages (Binary or JSON)

```typescript
// CRDT
{ type: 'crdt-snapshot', snap: Uint8Array, epoch: number }
{ type: 'crdt-snapshot-ack', peerId: string, epoch: number }
{ type: 'crdt-delta', delta: Uint8Array, clock: VClock, epoch: number }

// BFT
{ type: 'bft-validator-set', set: ValidatorSet }
{ type: 'bft-validator-set-change', newSet: ValidatorSet, oldEpoch: number }

// HotStuff Consensus
{ type: 'bft-prepare', block: Block, qc: QuorumCert, view: number }
{ type: 'bft-prepare-vote', sig: Signature, blockHash: string, view: number }
{ type: 'bft-commit', lockQC: QuorumCert }
{ type: 'bft-commit-vote', sig: Signature, blockHash: string }
{ type: 'bft-view-change', view: number, qc: QuorumCert }
{ type: 'bft-newview', view: number, newBlock: Block }

// Late Joiner
{ type: 'catch-up-request' }
{ type: 'catch-up-response', snapshot: Uint8Array, blocks: Block[], validatorSet: ValidatorSet, view: number }

// Token Transfer
{ type: 'token-tx', tx: Transaction, sig: Signature }
```

---

## 8. Implementation Checklist (for SPAN_333_Integration)

### Phase 1: Room State Machine (Week 1)
- [ ] Extend `room-state.ts`: add RoomState enum + FSM methods
- [ ] Add state transition guards (prevent invalid transitions)
- [ ] Add console logging for state changes (debug visibility)
- [ ] Update UI: show current room state in header

### Phase 2: CRDT Initialization (Week 2)
- [ ] Leader detection: `if (peers.size === 2 && myId < other.id) { isLeader = true }`
- [ ] Snapshot creation: `crdt.toSnapshot()` (Yjs API)
- [ ] Snapshot broadcast: `broadcast({ type: 'crdt-snapshot', ... })`
- [ ] Snapshot application: `crdt.applySnapshot(snap)`
- [ ] Delta broadcasting: on any CRDT change, `broadcast({ type: 'crdt-delta', ... })`

### Phase 3: BFT Validator Set Formation (Week 2)
- [ ] Trigger condition: `peers.size >= 3 && allDCsOpen()`
- [ ] Form validator set: sort peers by ID
- [ ] Compute quorum: `ceil((N+1)/3)`
- [ ] Broadcast ValidatorSet
- [ ] Transition to READY

### Phase 4: HotStuff Integration (Week 3)
- [ ] Wire `bft/hotstuff.rs` → `on_bft_prepare`, `on_bft_vote`, `on_bft_commit`
- [ ] Implement view timeout (15s)
- [ ] Implement view change protocol
- [ ] Send prepared blocks over DataChannel
- [ ] Verify signatures on received votes

### Phase 5: Late Joiner Catch-Up (Week 3)
- [ ] Detect late joiner: `peers.size increases && peer.epoch < current.epoch`
- [ ] Leader sends catch-up: committed blocks + snapshot
- [ ] Late joiner replays: `for block in blocks { applyTxs() }`
- [ ] Verify proposer signatures

### Phase 6: Peer Departure (Week 4)
- [ ] Handle graceful: `on(type: 'peer-left')`
- [ ] Handle ungraceful: DataChannel timeout + onclose
- [ ] Recalculate ValidatorSet + quorum
- [ ] Broadcast new ValidatorSet
- [ ] Check UNSAFE condition

### Phase 7: E2E Tests (Week 4)
- [ ] Test 2-peer room: create → sync → READY
- [ ] Test 3-peer consensus: propose block → votes → commit
- [ ] Test late joiner: join after 10 blocks, catch up correctly
- [ ] Test peer departure: 4→3 recalc, 3→2 safe, 2→1 unsafe
- [ ] Test view timeout: BFT timeout → viewchange → newview

---

## 9. Key Design Rationale

### 9.1 Why All Peers = Validators?

**Option A: All peers (chosen)**
- ✓ Decentralized: no hierarchical roles
- ✓ Simple: N=3 → quorum=2, N=50 → quorum=34
- ✓ Scalable: up to 50 peers (HotStuff linearity)
- ⚠️ Cost: 50 peers → 50 signatures per block (solvable with BLS aggregation later)

**Option B: Only first N peers**
- ❌ Unfair: late joiners are second-class
- ❌ Complex: who decides N? sliding window?

**Option C: Stake-based validators**
- ❌ Requires token state from day 1
- ❌ Bootstrapping problem: all peers start with 0 stake

### 9.2 Why Leader = Creator (Not BFT Proposer)?

**Option A: Creator = initial leader, then BFT proposer (chosen)**
- ✓ Fast bootstrap: creator can send snapshot immediately
- ✓ Fallback: if creator slow, BFT proposer takes over at view change
- ✓ Flexibility: proposer role rotates in HotStuff

**Option B: All peers sync live from day 1**
- ⚠️ Slower: concurrent deltas must merge (CRDT handles, but order unknown)
- ⚠️ Risk: conflicting snapshots if multiple peers claim leadership

### 9.3 Why Hybrid Snapshot + Live Delta?

**Option A: Snapshot then live (chosen)**
- ✓ Deterministic: snapshot = known state vector
- ✓ Fast: new peer doesn't wait for all past deltas
- ✓ Memory: can compact old history at epoch boundaries

**Option B: Live delta only**
- ❌ Late joiner must replay all deltas from genesis
- ❌ Unbounded: history grows forever

**Option C: Snapshot then replay blocks (no live delta)**
- ❌ High latency: late joiner waits for block execution
- ✓ Benefit: single source of truth (blocks)
- (Later optimization: combine both)

### 9.4 Why 15-Second BFT Timeout?

- Typical network latency (P2P): 50-200ms
- Block time (propose→vote→commit): 1-3 seconds
- Slack for slow replicas: 5x = 5-15 seconds
- **Chosen: 15 seconds** (reasonable for home networks)
- **Backoff**: view change 1 → 30s, 2 → 60s, 3 → 120s (exponential)

---

## 10. Example: 3-Peer Room Walkthrough

### T=0s: A Creates Room "abc123"

```
A: generateRoomId() → "abc123"
   connectToRoom("abc123")
   ws.send({ type: 'join', room: 'abc123', peerId: 'a1b2c3d4' })
   state = INIT
```

### T=2s: B Joins

```
B: connectToRoom("abc123")
   ws.send({ type: 'join', room: 'abc123', peerId: 'b5e6f7g8' })
   state = SIGNALING

A: receive { type: 'peer-joined', peerId: 'b5e6f7g8' }
   createOffer() → send to B
   state = SIGNALING

B: receive { type: 'peers', peers: ['a1b2c3d4'], you: 'b5e6f7g8' }
   state = SIGNALING
```

### T=4s: A & B Complete SDP Exchange

```
A: receive answer from B
   setRemoteDescription(answer)
   state = SIGNALING (wait for ICE)

B: receive offer from A
   setRemoteDescription(offer)
   createAnswer() → send to A
   state = SIGNALING
```

### T=6s: A & B ICE Complete, DataChannel Opens

```
A: dc.onopen (A→B channel open)
   state = SYNCING
   isLeader = true (2 peers, A < B lexicographically)

B: dc.onopen (B←A channel open)
   state = SYNCING
```

### T=7s: A Sends CRDT Snapshot

```
A: crdt.toSnapshot() → stateVector = { clock: { a: 1, b: 0 } }
   broadcast { type: 'crdt-snapshot', snap: stateVector, epoch: 0 }

B: receive snapshot
   crdt.applySnapshot(snap)
   broadcast { type: 'crdt-snapshot-ack', peerId: 'b5e6f7g8', epoch: 0 }
```

### T=8s: C Joins

```
C: connectToRoom("abc123")
   ws.send({ type: 'join', room: 'abc123', peerId: 'c9d0e1f2' })
   state = SIGNALING

A, B: receive { type: 'peer-joined', peerId: 'c9d0e1f2' }
      A & B create offers to C (A is initiator with C, B is initiator with C)
```

### T=12s: All 3 have DataChannels Open

```
A, B, C: peers = {a, b, c}, allDCsOpen = true
         state = SYNCING

A: peers.size >= 3 → form ValidatorSet
   validators = [a, b, c], quorum = 2
   broadcast { type: 'bft-validator-set', set: {...} }

B, C: receive ValidatorSet
      bft.setValidators({...})
      state = READY
```

### T=13s: Ready for Consensus

```
A, B, C: state = READY
         ✓ CRDT live: any peer can do blocks.set()
         ✓ BFT: A proposes first block (round-robin or stake)

A: crdt.set("1,1", "grass")
   broadcast { type: 'crdt-delta', delta: ..., clock: { a: 2 }, epoch: 0 }

B, C: receive delta, apply to CRDT

A: bft_propose(block_0: txs=[delta_1], parent=genesis_qc)
   broadcast { type: 'bft-prepare', block: block_0, qc: genesis_qc, view: 0 }

B, C: verify block, vote
      broadcast { type: 'bft-prepare-vote', sig: ..., blockHash: h0, view: 0 }

A: collect votes (A's vote + B's vote = quorum 2/3)
   create QC(block_0)
   broadcast { type: 'bft-commit', lockQC: qc_0 }
   state[block_0] = COMMITTED
```

### T=30s: B Crashes (Network Partition)

```
B: (no action, just dies)

A, C: DataChannel.onclose() after 30s ICE timeout
      peers.delete('b5e6f7g8')
      peers = {a, c}
      recalculateValidatorSet()
      new_quorum = ceil((2+1)/3) = 1 (still safe!)

A, C: broadcast { type: 'bft-validator-set-change', newSet: {[a,c], q:1, epoch:1} }
      state = READY
      ✓ Can continue consensus with 2 peers
```

---

## 11. Known Issues & Future Work

### 11.1 Not Addressed (Out of Scope)

1. **DHT for peer discovery**: Currently requires sharing room URL manually
2. **Sybil resistance**: No stake/reputation yet (early integration)
3. **Byzantine peer**: No detection of malicious consensus votes
4. **Mobile network**: No connection recovery (reconnect = rejoin)
5. **BLS signature aggregation**: Current design uses individual Ed25519
6. **Persistence**: Room state lost on page reload (IndexedDB needed)

### 11.2 Assumptions

1. **Network**: Mostly reliable, no Byzantine adversaries, <30s partitions
2. **Browser environment**: WebRTC+WebSocket available, localStorage/IndexedDB
3. **Peer count**: N ≤ 50 (HotStuff linearity, BLS aggregation at 50+)
4. **Clock sync**: Peers' clocks ≠ synchronized (using vector clocks for CRDT)
5. **Identities**: Ed25519 key = permanent peer identity (no key rotation)

### 11.3 Optimization Opportunities

1. **View timeout adaptive**: 15s → measure network latency, adjust
2. **Batch CRDT deltas**: instead of broadcast per change, batch every 100ms
3. **Epoch compaction**: every 1000 blocks, create new snapshot + discard old
4. **BLS aggregate signatures**: 50+ peers → 1 aggregate sig instead of 50
5. **Merkle tree for blocks**: prove history without full replay

---

## 12. References

- **KG**: SPAN_333_Integration, lesson-333-modules-not-integrated, SPAN_333_ConsensusInitialization
- **Codebase**:
  - `333-app/src/lib/room-state.ts` — WebRTC peer connection
  - `333-app/src/routes/room/+page.svelte` — UI
  - `signaling/server.mjs` — Stateless relay
  - `src/p2p/webrtc.rs` — Rust WebRTC bindings
  - `src/crdt/mod.rs` — Yjs wrapper
  - `src/bft/hotstuff.rs` — HotStuff consensus
- **External**: HotStuff (PODC 2019), Yjs CRDT, WebRTC Data Channels RFC

---

## Summary for SPAN_333_Integration Phase

**You need to implement these 5 message handlers:**

1. `on_crdt_snapshot(snapshot)` — leader sends initial state
2. `on_bft_validator_set(set)` — trigger consensus start
3. `on_crdt_delta(delta)` — live CRDT broadcast
4. `on_bft_*` (prepare/vote/commit/viewchange) — HotStuff consensus
5. `on_catch_up_request()` — late joiner replay

**State machine gates:**
- INIT → SIGNALING: peer joins
- SIGNALING → SYNCING: DataChannel opens
- SYNCING → READY: ValidatorSet broadcast
- READY ↔ FROZEN: BFT timeout
- READY → UNSAFE: lost quorum

**Tests needed** (E2E):
- [ ] 2-peer sync
- [ ] 3-peer consensus + block commit
- [ ] Late joiner (join at block 10, replay)
- [ ] Peer crash (recalc quorum)
- [ ] 4→3→2 safe, 2→1 unsafe

---

*This document is KG-bound via SPAN_333_RoomLifecycleResearch + SPAN_333_ConsensusInitialization. Update KG work-buffer when implementation begins.*
