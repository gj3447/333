# 333 Platform: Room Lifecycle Implementation Patterns
# KG: SPAN_333_RoomImplementationPatterns

> Concrete TypeScript/Rust patterns for SPAN_333_Integration Phase.  
> Context: ROOM_LIFECYCLE_AND_CONSENSUS_RESEARCH.md

---

## 1. Room State Machine Core (TypeScript)

### 1.1 State Enum and Type Guards

```typescript
// src/lib/room-state-machine.ts
// KG: SPAN_333_RoomLifecycle

export enum RoomState {
  INIT = 'init',
  SIGNALING = 'signaling',
  SYNCING = 'syncing',
  READY = 'ready',
  UNSAFE = 'unsafe',
  FROZEN = 'frozen',
  SYNCING_LATE = 'syncing_late',
  DISCONNECTED = 'disconnected'
}

export class RoomStateMachine {
  private currentState: RoomState = RoomState.INIT;
  private readonly state_log: Array<{ time: number; from: RoomState; to: RoomState }> = [];
  
  constructor(
    public roomId: string,
    private onStateChange: (state: RoomState, prev: RoomState) => void
  ) {}
  
  getState(): RoomState {
    return this.currentState;
  }
  
  /**
   * Guards: check if transition is valid before applying
   */
  canTransitionTo(nextState: RoomState): boolean {
    const validTransitions: Record<RoomState, RoomState[]> = {
      [RoomState.INIT]: [RoomState.SIGNALING, RoomState.DISCONNECTED],
      [RoomState.SIGNALING]: [RoomState.SYNCING, RoomState.INIT, RoomState.DISCONNECTED],
      [RoomState.SYNCING]: [RoomState.READY, RoomState.FROZEN, RoomState.SYNCING_LATE, RoomState.DISCONNECTED],
      [RoomState.READY]: [RoomState.FROZEN, RoomState.UNSAFE, RoomState.SYNCING_LATE, RoomState.DISCONNECTED],
      [RoomState.UNSAFE]: [RoomState.DISCONNECTED],
      [RoomState.FROZEN]: [RoomState.READY, RoomState.UNSAFE, RoomState.DISCONNECTED],
      [RoomState.SYNCING_LATE]: [RoomState.READY, RoomState.DISCONNECTED],
      [RoomState.DISCONNECTED]: []
    };
    
    return validTransitions[this.currentState]?.includes(nextState) ?? false;
  }
  
  /**
   * Transition with guard checks
   */
  transitionTo(nextState: RoomState): boolean {
    if (!this.canTransitionTo(nextState)) {
      console.error(`[${this.roomId}] Invalid transition: ${this.currentState} → ${nextState}`);
      return false;
    }
    
    const prev = this.currentState;
    this.currentState = nextState;
    this.state_log.push({ time: Date.now(), from: prev, to: nextState });
    
    console.log(`[${this.roomId}] State: ${prev} → ${nextState}`);
    this.onStateChange(nextState, prev);
    
    return true;
  }
  
  /**
   * Message handlers check: can this peer receive/send certain messages?
   */
  canSendCRDT(): boolean {
    return [RoomState.SYNCING, RoomState.READY, RoomState.SYNCING_LATE].includes(this.currentState);
  }
  
  canProposeBFT(): boolean {
    return this.currentState === RoomState.READY;
  }
  
  canVoteBFT(): boolean {
    return [RoomState.READY, RoomState.FROZEN].includes(this.currentState);
  }
  
  canReceiveCRDTSnapshot(): boolean {
    return [RoomState.SYNCING, RoomState.SYNCING_LATE].includes(this.currentState);
  }
  
  canReceiveValidatorSet(): boolean {
    return this.currentState === RoomState.SYNCING;
  }
  
  canRespondToCatchUpRequest(): boolean {
    return [RoomState.READY, RoomState.FROZEN].includes(this.currentState);
  }
}
```

### 1.2 Timeout Handling

```typescript
// src/lib/room-state-timers.ts
// KG: SPAN_333_RoomLifecycle

export interface RoomTimers {
  signalingTimeout: Timer | null;
  syncingTimeout: Timer | null;
  bftViewTimeout: Timer | null;
  bftViewChangeBackoff: number; // milliseconds
}

export class RoomTimerManager {
  private timers: RoomTimers = {
    signalingTimeout: null,
    syncingTimeout: null,
    bftViewTimeout: null,
    bftViewChangeBackoff: 30_000 // 30s, exponential backoff
  };
  
  constructor(
    private roomId: string,
    private onTimeout: (timeoutType: string) => void
  ) {}
  
  /**
   * Start signaling timeout: 30s to open DataChannel
   */
  startSignalingTimeout() {
    this.clearSignalingTimeout();
    this.timers.signalingTimeout = setTimeout(() => {
      console.warn(`[${this.roomId}] Signaling timeout: no DC opened in 30s`);
      this.onTimeout('signaling');
    }, 30_000);
  }
  
  clearSignalingTimeout() {
    if (this.timers.signalingTimeout) clearTimeout(this.timers.signalingTimeout);
    this.timers.signalingTimeout = null;
  }
  
  /**
   * Start syncing timeout: 30s to receive ValidatorSet
   */
  startSyncingTimeout() {
    this.clearSyncingTimeout();
    this.timers.syncingTimeout = setTimeout(() => {
      console.warn(`[${this.roomId}] Syncing timeout: no ValidatorSet in 30s`);
      this.onTimeout('syncing');
    }, 30_000);
  }
  
  clearSyncingTimeout() {
    if (this.timers.syncingTimeout) clearTimeout(this.timers.syncingTimeout);
    this.timers.syncingTimeout = null;
  }
  
  /**
   * Start BFT view timeout: 15s for proposer to produce block
   */
  startBFTViewTimeout() {
    this.clearBFTViewTimeout();
    this.timers.bftViewTimeout = setTimeout(() => {
      console.warn(`[${this.roomId}] BFT view timeout: initiating view change`);
      this.onTimeout('bft-view');
      
      // Exponential backoff for next timeout
      this.timers.bftViewChangeBackoff = Math.min(
        this.timers.bftViewChangeBackoff * 2,
        120_000 // cap at 120s
      );
    }, 15_000);
  }
  
  clearBFTViewTimeout() {
    if (this.timers.bftViewTimeout) clearTimeout(this.timers.bftViewTimeout);
    this.timers.bftViewTimeout = null;
  }
  
  clearAll() {
    this.clearSignalingTimeout();
    this.clearSyncingTimeout();
    this.clearBFTViewTimeout();
  }
}
```

---

## 2. Extended RoomState (TypeScript)

### 2.1 Update room-state.ts with Lifecycle Integration

```typescript
// src/lib/room-state.ts (EXTENDED)
// KG: SPAN_333_RoomLifecycle, CONTRACT_SharedType_RoomState

import { RoomStateMachine, RoomState } from './room-state-machine';
import { RoomTimerManager } from './room-state-timers';

export interface PeerInfo {
  id: string;
  username: string;
  connectedAt: number;
  dcState: 'pending' | 'open' | 'closed';
  pubKey: Uint8Array;
}

export interface RoomState {
  roomId: string;
  myId: string;
  myPubKey: Uint8Array;
  lifecycleState: RoomState; // NEW: state machine
  peers: Map<string, PeerInfo>;
  validatorSet: ValidatorSet | null;
  crdtStateVector: Record<string, number>;
  bftView: number;
  send: (data: Uint8Array) => void;
  onMessage: (handler: (from: string, data: Uint8Array) => void) => void;
  broadcast: (data: any) => void; // NEW: broadcast to all peers
}

export interface ValidatorSet {
  validators: string[]; // sorted peer IDs
  epoch: number;
  quorum: number;
  timestamp: number;
}

export function createRoomState(roomId: string, myId: string, pubKey: Uint8Array, signalingUrl: string): RoomState {
  let ws: WebSocket | null = null;
  let lifecycleState = RoomState.INIT;
  const peers = new Map<string, PeerInfo>();
  const handlers: MessageHandler[] = [];
  const peerConnections = new Map<string, RTCPeerConnection>();
  const dataChannels = new Map<string, RTCDataChannel>();
  
  // NEW: State machine + timers
  const stateMachine = new RoomStateMachine(roomId, (nextState, prevState) => {
    lifecycleState = nextState;
    
    // Handle state-specific cleanup/startup
    if (nextState === RoomState.SIGNALING) {
      timerManager.startSignalingTimeout();
    }
    if (nextState === RoomState.SYNCING) {
      timerManager.clearSignalingTimeout();
      timerManager.startSyncingTimeout();
      // Trigger CRDT snapshot if we're the leader
      if (isLeader()) {
        const snapshot = getCRDTSnapshot();
        broadcastMessage({
          type: 'crdt-snapshot',
          snap: snapshot,
          epoch: 0
        });
      }
    }
    if (nextState === RoomState.READY) {
      timerManager.clearSyncingTimeout();
      timerManager.startBFTViewTimeout();
    }
    if (nextState === RoomState.FROZEN) {
      timerManager.clearBFTViewTimeout();
      // Initiate view change
      initiateViewChange();
    }
    if (nextState === RoomState.DISCONNECTED) {
      timerManager.clearAll();
      ws?.close();
    }
  });
  
  const timerManager = new RoomTimerManager(roomId, (timeoutType) => {
    switch (timeoutType) {
      case 'signaling':
        // WebRTC connection failed
        console.error(`[${roomId}] WebRTC connection timeout`);
        stateMachine.transitionTo(RoomState.INIT);
        break;
      case 'syncing':
        // CRDT sync or BFT genesis timeout
        console.error(`[${roomId}] Syncing timeout, freezing`);
        stateMachine.transitionTo(RoomState.FROZEN);
        break;
      case 'bft-view':
        // BFT consensus timeout, initiate view change
        if (stateMachine.getState() === RoomState.READY) {
          stateMachine.transitionTo(RoomState.FROZEN);
        }
        break;
    }
  });
  
  function getCRDTSnapshot(): Uint8Array {
    // TODO: call WASM crdt.snapshot()
    return new Uint8Array(0);
  }
  
  function isLeader(): boolean {
    // First peer (lexicographically) is temporary leader
    const sorted = Array.from(peers.keys()).sort();
    return sorted[0] === myId;
  }
  
  function initiateViewChange() {
    // TODO: implement HotStuff view change
  }
  
  function broadcastMessage(msg: any) {
    for (const [, ch] of dataChannels) {
      if (ch.readyState === 'open') {
        ch.send(JSON.stringify(msg));
      }
    }
  }
  
  function recalculateValidatorSet(): boolean {
    const alive = Array.from(peers.values())
      .filter(p => p.dcState === 'open')
      .map(p => p.id)
      .sort();
    
    if (alive.length === 0) {
      stateMachine.transitionTo(RoomState.UNSAFE);
      return false;
    }
    
    const newQuorum = Math.ceil((alive.length + 1) / 3);
    const oldQuorum = validatorSet?.quorum ?? newQuorum;
    
    // Check if we've lost quorum
    if (alive.length < oldQuorum) {
      stateMachine.transitionTo(RoomState.UNSAFE);
      return false;
    }
    
    const newValidatorSet: ValidatorSet = {
      validators: alive,
      epoch: (validatorSet?.epoch ?? 0) + 1,
      quorum: newQuorum,
      timestamp: Date.now()
    };
    
    validatorSet = newValidatorSet;
    broadcastMessage({
      type: 'bft-validator-set-change',
      newSet: newValidatorSet
    });
    
    return true;
  }
  
  const state: RoomState = {
    roomId,
    myId,
    myPubKey: pubKey,
    get lifecycleState() { return lifecycleState; },
    peers,
    validatorSet: null,
    crdtStateVector: {},
    bftView: 0,
    send: (data: Uint8Array) => {
      if (!stateMachine.canSendCRDT()) {
        console.warn(`[${roomId}] Cannot send CRDT in state ${lifecycleState}`);
        return;
      }
      for (const [, ch] of dataChannels) {
        if (ch.readyState === 'open') ch.send(data);
      }
    },
    onMessage: (handler: MessageHandler) => { handlers.push(handler); },
    broadcast: broadcastMessage
  };
  
  function notifyHandlers(from: string, data: Uint8Array) {
    for (const h of handlers) h(from, data);
  }
  
  function createPeerConnection(peerId: string, isInitiator: boolean) {
    const pc = new RTCPeerConnection({
      iceServers: [{ urls: 'stun:stun.l.google.com:19302' }]
    });
    peerConnections.set(peerId, pc);
    
    pc.onicecandidate = (e) => {
      if (e.candidate && ws?.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'ice', to: peerId, candidate: e.candidate }));
      }
    };
    
    pc.ondatachannel = (e) => {
      setupChannel(peerId, e.channel);
    };
    
    if (isInitiator) {
      const ch = pc.createDataChannel('333', { ordered: true });
      setupChannel(peerId, ch);
      pc.createOffer().then(offer => {
        pc.setLocalDescription(offer);
        ws?.send(JSON.stringify({ type: 'offer', to: peerId, sdp: offer }));
      });
    }
    
    return pc;
  }
  
  function setupChannel(peerId: string, channel: RTCDataChannel) {
    dataChannels.set(peerId, channel);
    channel.binaryType = 'arraybuffer';
    
    channel.onopen = () => {
      const peerInfo = peers.get(peerId);
      if (peerInfo) peerInfo.dcState = 'open';
      
      // Transition state machine if ready
      const allOpen = Array.from(peers.values()).every(p => p.dcState === 'open');
      if (allOpen && lifecycleState === RoomState.SIGNALING) {
        stateMachine.transitionTo(RoomState.SYNCING);
      }
    };
    
    channel.onclose = () => {
      const peerInfo = peers.get(peerId);
      if (peerInfo) peerInfo.dcState = 'closed';
      
      dataChannels.delete(peerId);
      peerConnections.get(peerId)?.close();
      peerConnections.delete(peerId);
      
      // Handle peer departure
      if (lifecycleState === RoomState.READY || lifecycleState === RoomState.SYNCING) {
        peers.delete(peerId);
        recalculateValidatorSet();
      }
    };
    
    channel.onmessage = (e) => {
      const msg = JSON.parse(e.data);
      
      // Route different message types
      switch (msg.type) {
        case 'crdt-snapshot':
          if (stateMachine.canReceiveCRDTSnapshot()) {
            // TODO: applyCRDTSnapshot(msg.snap)
            console.log(`[${roomId}] Received CRDT snapshot from ${peerId}`);
          }
          break;
        
        case 'crdt-delta':
          if (stateMachine.canSendCRDT()) {
            // TODO: applyCRDTDelta(msg.delta)
            console.log(`[${roomId}] Received CRDT delta from ${peerId}`);
          }
          break;
        
        case 'bft-validator-set':
          if (stateMachine.canReceiveValidatorSet()) {
            state.validatorSet = msg.set;
            timerManager.startBFTViewTimeout();
            stateMachine.transitionTo(RoomState.READY);
          }
          break;
        
        case 'bft-validator-set-change':
          if (stateMachine.canVoteBFT()) {
            state.validatorSet = msg.newSet;
            console.log(`[${roomId}] ValidatorSet updated to epoch ${msg.newSet.epoch}`);
          }
          break;
        
        case 'catch-up-request':
          if (stateMachine.canRespondToCatchUpRequest()) {
            // TODO: send snapshot + committed blocks
            console.log(`[${roomId}] Late joiner ${peerId} requested catch-up`);
          }
          break;
        
        case 'bft-prepare':
        case 'bft-prepare-vote':
        case 'bft-commit':
        case 'bft-view-change':
        case 'bft-newview':
          if (stateMachine.canVoteBFT()) {
            // TODO: route to BFT module
            console.log(`[${roomId}] BFT message from ${peerId}: ${msg.type}`);
          }
          break;
        
        default:
          // Application data (blocks, transactions, etc.)
          const data = new TextEncoder().encode(JSON.stringify(msg));
          notifyHandlers(peerId, data);
      }
    };
  }
  
  // Connect to signaling
  try {
    stateMachine.transitionTo(RoomState.SIGNALING);
    ws = new WebSocket(signalingUrl);
    
    ws.onopen = () => {
      ws!.send(JSON.stringify({ type: 'join', room: roomId, peerId: myId }));
    };
    
    ws.onmessage = async (e) => {
      const msg = JSON.parse(e.data);
      
      if (msg.type === 'peers') {
        for (const pid of msg.peers) {
          if (pid !== myId && !peerConnections.has(pid)) {
            // NEW: check if we're a late joiner
            const isLateJoiner = msg.peers.length > 2 && validatorSet && validatorSet.epoch > 0;
            if (isLateJoiner && lifecycleState === RoomState.SIGNALING) {
              stateMachine.transitionTo(RoomState.SYNCING_LATE);
            }
            createPeerConnection(pid, true);
          }
        }
      } else if (msg.type === 'peer-joined') {
        createPeerConnection(msg.peerId, true);
      } else if (msg.type === 'peer-left') {
        peers.delete(msg.peerId);
        if (lifecycleState === RoomState.READY || lifecycleState === RoomState.SYNCING) {
          recalculateValidatorSet();
        }
      } else if (msg.type === 'offer' && msg.from !== myId) {
        const pc = createPeerConnection(msg.from, false);
        await pc.setRemoteDescription(msg.sdp);
        const answer = await pc.createAnswer();
        await pc.setLocalDescription(answer);
        ws!.send(JSON.stringify({ type: 'answer', to: msg.from, sdp: answer }));
      } else if (msg.type === 'answer' && msg.from !== myId) {
        await peerConnections.get(msg.from)?.setRemoteDescription(msg.sdp);
      } else if (msg.type === 'ice' && msg.from !== myId) {
        await peerConnections.get(msg.from)?.addIceCandidate(msg.candidate);
      }
    };
    
    ws.onclose = () => {
      if (lifecycleState !== RoomState.DISCONNECTED) {
        stateMachine.transitionTo(RoomState.DISCONNECTED);
      }
    };
  } catch (e) {
    console.error(`[${roomId}] WebSocket error:`, e);
    stateMachine.transitionTo(RoomState.DISCONNECTED);
  }
  
  return state;
}
```

---

## 3. CRDT Snapshot + Delta Protocol (TypeScript/Rust Bridge)

### 3.1 CRDT Message Handlers

```typescript
// src/lib/crdt-sync.ts
// KG: SPAN_333_RoomLifecycle

export interface CRDTSnapshot {
  stateVector: Record<string, number>; // peer → lamport clock
  data: Uint8Array; // Yjs encoded state
}

export interface CRDTDelta {
  changes: Uint8Array; // Yjs encoded update
  vectorClock: Record<string, number>;
  epoch: number;
}

export class CRDTSyncManager {
  private crdtModule: any; // WASM binding to Yjs
  private stateVector: Record<string, number> = {};
  
  constructor(private peerId: string) {}
  
  /**
   * Called by leader to send initial snapshot
   */
  createSnapshot(): CRDTSnapshot {
    const stateVector = this.crdtModule.getStateVector();
    const data = this.crdtModule.encodeState();
    
    return {
      stateVector,
      data
    };
  }
  
  /**
   * Called by joiner to apply received snapshot
   */
  applySnapshot(snapshot: CRDTSnapshot) {
    this.crdtModule.applyState(snapshot.data);
    this.stateVector = snapshot.stateVector;
    console.log(`Applied CRDT snapshot. State vector:`, this.stateVector);
  }
  
  /**
   * Called whenever CRDT changes (e.g., block placed)
   */
  onCRDTChange(delta: Uint8Array) {
    // Increment our lamport clock
    this.stateVector[this.peerId] = (this.stateVector[this.peerId] ?? 0) + 1;
    
    const msg: CRDTDelta = {
      changes: delta,
      vectorClock: { ...this.stateVector },
      epoch: 0
    };
    
    return msg;
  }
  
  /**
   * Apply received delta from peer
   */
  applyDelta(delta: CRDTDelta) {
    // Check if already applied (idempotent)
    const peerClock = this.stateVector[delta.vectorClock[this.peerId]] ?? 0;
    if (delta.vectorClock[this.peerId] <= peerClock) {
      console.log(`Delta from ${this.peerId} already applied, skipping`);
      return;
    }
    
    // Merge lamport clocks
    for (const [peer, ts] of Object.entries(delta.vectorClock)) {
      this.stateVector[peer] = Math.max(this.stateVector[peer] ?? 0, ts);
    }
    
    // Apply to CRDT
    this.crdtModule.applyUpdate(delta.changes);
    console.log(`Applied CRDT delta. New state vector:`, this.stateVector);
  }
}
```

### 3.2 WASM CRDT Binding (Rust)

```rust
// src/crdt/mod.rs (EXTENDED)
// KG: SPAN_333_RoomLifecycle

use wasm_bindgen::prelude::*;
use yrs::{Doc, Transact, Text};
use std::collections::HashMap;

#[wasm_bindgen]
pub struct CRDTState {
  doc: Doc,
  state_vector: HashMap<String, u32>,
}

#[wasm_bindgen]
impl CRDTState {
  #[wasm_bindgen(constructor)]
  pub fn new() -> CRDTState {
    CRDTState {
      doc: Doc::new(),
      state_vector: HashMap::new(),
    }
  }
  
  /// Get current state vector (lamport clocks per peer)
  #[wasm_bindgen]
  pub fn get_state_vector(&self) -> JsValue {
    serde_wasm_bindgen::to_value(&self.state_vector).unwrap()
  }
  
  /// Encode entire document state to bytes
  #[wasm_bindgen]
  pub fn encode_state(&self) -> Vec<u8> {
    let mut encoder = Vec::new();
    let state_vec = yrs::encoding::read_var_u32(&mut std::io::Cursor::new(&[]));
    self.doc.get_state(Vec::new())
  }
  
  /// Apply full state (from snapshot)
  #[wasm_bindgen]
  pub fn apply_state(&mut self, state: &[u8]) -> Result<(), JsValue> {
    self.doc = Doc::from_bytes(state)?;
    Ok(())
  }
  
  /// Apply incremental update (delta)
  #[wasm_bindgen]
  pub fn apply_update(&mut self, update: &[u8]) -> Result<(), JsValue> {
    let mut txn = self.doc.transact_mut();
    txn.apply_update(update.to_vec()).ok();
    Ok(())
  }
  
  /// Get current CRDT as JSON (for app serialization)
  #[wasm_bindgen]
  pub fn to_json(&self) -> String {
    // Serialize doc to JSON
    "{}".to_string()
  }
}
```

---

## 4. BFT Validator Set Formation (Rust)

### 4.1 ValidatorSet Initialization

```rust
// src/bft/validator_set.rs (NEW)
// KG: SPAN_333_RoomLifecycle

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidatorSet {
  pub validators: Vec<String>, // sorted peer IDs
  pub epoch: u32,
  pub quorum: u32,
  pub timestamp: u64,
}

impl ValidatorSet {
  pub fn new(peers: Vec<String>, epoch: u32) -> Self {
    let mut validators = peers;
    validators.sort();
    
    let quorum = ((validators.len() as u32 + 1) / 3).max(1);
    
    ValidatorSet {
      validators,
      epoch,
      quorum,
      timestamp: std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs(),
    }
  }
  
  pub fn is_valid_quorum(&self, votes: &BTreeSet<String>) -> bool {
    votes.len() >= self.quorum as usize
  }
  
  pub fn get_proposer(&self, view: u32) -> Option<&str> {
    let idx = (view as usize) % self.validators.len();
    Some(&self.validators[idx])
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  
  #[test]
  fn test_quorum_calculation() {
    let vs = ValidatorSet::new(vec!["a".into(), "b".into(), "c".into()], 0);
    assert_eq!(vs.quorum, 2);
    
    let vs = ValidatorSet::new(vec!["a".into(), "b".into(), "c".into(), "d".into()], 0);
    assert_eq!(vs.quorum, 2);
    
    let vs = ValidatorSet::new(vec!["a".into(), "b".into()], 0);
    assert_eq!(vs.quorum, 1);
  }
  
  #[test]
  fn test_proposer_rotation() {
    let vs = ValidatorSet::new(vec!["a".into(), "b".into(), "c".into()], 0);
    
    assert_eq!(vs.get_proposer(0), Some("a"));
    assert_eq!(vs.get_proposer(1), Some("b"));
    assert_eq!(vs.get_proposer(2), Some("c"));
    assert_eq!(vs.get_proposer(3), Some("a")); // wrap around
  }
}
```

### 4.2 BFT State Integration

```rust
// src/bft/hotstuff.rs (EXTENDED)
// KG: SPAN_333_RoomLifecycle

use crate::bft::validator_set::ValidatorSet;

pub struct HotStuffState {
  pub validator_set: Option<ValidatorSet>,
  pub view: u32,
  pub lock_qc: Option<QuorumCert>,
  pub committed_blocks: Vec<Block>,
}

impl HotStuffState {
  pub fn new() -> Self {
    HotStuffState {
      validator_set: None,
      view: 0,
      lock_qc: None,
      committed_blocks: vec![],
    }
  }
  
  pub fn set_validators(&mut self, vs: ValidatorSet) {
    println!("[BFT] ValidatorSet epoch {}: {:?}", vs.epoch, vs.validators);
    self.validator_set = Some(vs);
  }
  
  pub fn get_proposer(&self) -> Option<String> {
    self.validator_set.as_ref().and_then(|vs| {
      vs.get_proposer(self.view).map(|s| s.to_string())
    })
  }
  
  pub fn advance_view(&mut self) {
    if let Some(ref vs) = self.validator_set {
      let old_proposer = vs.get_proposer(self.view);
      self.view += 1;
      let new_proposer = vs.get_proposer(self.view);
      println!("[BFT] View {} → {} (proposer: {:?} → {:?})",
        self.view - 1, self.view, old_proposer, new_proposer);
    }
  }
}

#[wasm_bindgen]
impl Platform333 {
  /// Called when room receives ValidatorSet
  pub fn set_bft_validators(&mut self, validators_json: &str) -> Result<(), JsValue> {
    let vs: ValidatorSet = serde_json::from_str(validators_json)
      .map_err(|e| JsValue::from_str(&e.to_string()))?;
    
    self.bft.set_validators(vs);
    Ok(())
  }
  
  /// Propose a new block (only if we're proposer)
  pub fn propose_block(&mut self, transactions: &[u8]) -> Result<Vec<u8>, JsValue> {
    let Some(proposer) = self.bft.get_proposer() else {
      return Err(JsValue::from_str("No proposer in ValidatorSet"));
    };
    
    if proposer != self.peer_id {
      return Err(JsValue::from_str("Not our turn to propose"));
    }
    
    let block = Block {
      view: self.bft.view,
      parent_qc: self.bft.lock_qc.clone(),
      txs: transactions.to_vec(),
    };
    
    let encoded = bincode::serialize(&block)?;
    Ok(encoded)
  }
}
```

---

## 5. Late Joiner Catch-Up (TypeScript)

### 5.1 Catch-Up Request/Response

```typescript
// src/lib/late-joiner.ts
// KG: SPAN_333_RoomLifecycle

export interface CatchUpRequest {
  type: 'catch-up-request';
  peerId: string;
}

export interface CatchUpResponse {
  type: 'catch-up-response';
  snapshot: CRDTSnapshot;
  committedBlocks: Block[];
  validatorSet: ValidatorSet;
  currentView: number;
}

export class LateJoinerManager {
  constructor(
    private peerId: string,
    private bftModule: any,
    private crdtModule: CRDTSyncManager
  ) {}
  
  /**
   * Late joiner sends catch-up request
   */
  sendCatchUpRequest(roomBroadcast: (msg: any) => void) {
    const msg: CatchUpRequest = {
      type: 'catch-up-request',
      peerId: this.peerId
    };
    roomBroadcast(msg);
  }
  
  /**
   * Peer responds with catch-up data
   */
  createCatchUpResponse(validatorSet: ValidatorSet): CatchUpResponse {
    const snapshot = this.crdtModule.createSnapshot();
    const committedBlocks = this.bftModule.get_committed_blocks();
    
    return {
      type: 'catch-up-response',
      snapshot,
      committedBlocks,
      validatorSet,
      currentView: this.bftModule.get_view()
    };
  }
  
  /**
   * Late joiner applies catch-up
   */
  applyCatchUp(response: CatchUpResponse) {
    console.log(`[${this.peerId}] Applying catch-up: snapshot + ${response.committedBlocks.length} blocks`);
    
    // 1. Apply CRDT snapshot
    this.crdtModule.applySnapshot(response.snapshot);
    
    // 2. Replay committed blocks (verify signatures, but don't re-execute)
    for (const block of response.committedBlocks) {
      console.log(`  Replaying block view ${block.view}`);
      // TODO: verify(block.signature, block.proposer)?
      // Block's transactions already applied to CRDT by proposer
    }
    
    // 3. Set validator set and view
    console.log(`Setting view to ${response.currentView}, validator set epoch ${response.validatorSet.epoch}`);
    this.bftModule.set_validators(response.validatorSet);
    this.bftModule.set_view(response.currentView);
  }
}
```

---

## 6. Peer Departure Detection (TypeScript)

### 6.1 Quorum Recalculation

```typescript
// src/lib/peer-departure.ts
// KG: SPAN_333_RoomLifecycle

export class PeerDepartureManager {
  constructor(
    private roomId: string,
    private stateMachine: RoomStateMachine,
    private bftModule: any
  ) {}
  
  /**
   * Handle peer disconnection: recalculate ValidatorSet
   */
  onPeerDeparture(
    peers: Map<string, PeerInfo>,
    oldValidatorSet: ValidatorSet | null,
    broadcastMessage: (msg: any) => void
  ): ValidatorSet | null {
    const alive = Array.from(peers.values())
      .filter(p => p.dcState === 'open')
      .map(p => p.id)
      .sort();
    
    console.log(`[${this.roomId}] Peer departed. Alive peers: ${alive.length}`);
    
    if (alive.length === 0) {
      console.error(`[${this.roomId}] No peers alive, room is unsafe`);
      this.stateMachine.transitionTo(RoomState.UNSAFE);
      return null;
    }
    
    const oldQuorum = oldValidatorSet?.quorum ?? 1;
    const newQuorum = Math.ceil((alive.length + 1) / 3);
    
    // Safety check: do we have enough replicas?
    if (alive.length < oldQuorum) {
      console.error(`[${this.roomId}] Quorum lost: ${alive.length} < ${oldQuorum}`);
      this.stateMachine.transitionTo(RoomState.UNSAFE);
      return oldValidatorSet; // Keep old validator set for reference
    }
    
    // Form new validator set
    const newValidatorSet: ValidatorSet = {
      validators: alive,
      epoch: (oldValidatorSet?.epoch ?? 0) + 1,
      quorum: newQuorum,
      timestamp: Date.now()
    };
    
    // Broadcast new validator set
    broadcastMessage({
      type: 'bft-validator-set-change',
      newSet: newValidatorSet,
      oldEpoch: oldValidatorSet?.epoch ?? 0
    });
    
    // Update BFT module
    this.bftModule.set_validators(newValidatorSet);
    
    console.log(`[${this.roomId}] New ValidatorSet epoch ${newValidatorSet.epoch}: ` +
      `${alive.length} validators, quorum=${newQuorum}`);
    
    // Stay in READY (if we have quorum)
    this.stateMachine.transitionTo(RoomState.READY);
    
    return newValidatorSet;
  }
}
```

---

## 7. Integration into +page.svelte (UI Updates)

### 7.1 Update Room Page with State Display

```svelte
<!-- 333-app/src/routes/room/+page.svelte (EXTENDED) -->
<!-- KG: SPAN_333_RoomLifecycle, CONTRACT_333_FE_PeerDiscovery -->

<script lang="ts">
  // ... existing imports ...
  import { RoomState } from '$lib/room-state-machine';
  
  let lifecycleState: RoomState = $state(RoomState.INIT);
  
  function connectToRoom(id: string) {
    const sigUrl = getSignalingUrl();
    room = createRoomState(id, myId, pubKey, sigUrl);
    status = 'connecting';
    
    // NEW: listen to lifecycle state changes
    let previousState = lifecycleState;
    const stateMonitor = setInterval(() => {
      if (!room) { clearInterval(stateMonitor); return; }
      
      lifecycleState = room.lifecycleState;
      status = room.status;
      peerList = Array.from(room.peers.values());
      
      // Log state transitions
      if (lifecycleState !== previousState) {
        console.log(`State transition: ${previousState} → ${lifecycleState}`);
        log(`State: ${previousState} → ${lifecycleState}`);
        previousState = lifecycleState;
      }
    }, 200);
  }
</script>

<h2 class="page-title"><span class="accent">~</span> P2P Room</h2>

{#if !roomId}
  <!-- Create/Join UI -->
{:else}
  <!-- Room header with state display -->
  <div class="card room-header">
    <div class="room-header__row">
      <div class="room-header__status">
        <span class="dot"
          class:dot--init={lifecycleState === 'init'}
          class:dot--signaling={lifecycleState === 'signaling'}
          class:dot--syncing={lifecycleState === 'syncing'}
          class:dot--ready={lifecycleState === 'ready'}
          class:dot--frozen={lifecycleState === 'frozen'}
          class:dot--unsafe={lifecycleState === 'unsafe'}
          class:dot--late={lifecycleState === 'syncing_late'}
        ></span>
        <strong>{lifecycleState}</strong>
      </div>
      <!-- ... rest of UI ... -->
    </div>
  </div>
  
  <!-- Block world (only when READY) -->
  {#if lifecycleState === 'ready'}
    <div class="card" style="margin-top:1rem">
      <h3 class="card-heading">Block World -- Live CRDT</h3>
      <!-- ... block grid ... -->
    </div>
  {:else if lifecycleState === 'syncing' || lifecycleState === 'syncing_late'}
    <div class="card" style="margin-top:1rem">
      <p>Syncing CRDT snapshot and BFT genesis...</p>
    </div>
  {/if}
{/if}

<style>
  .dot--init { background: #6b7280; }
  .dot--signaling { background: #fbbf24; }
  .dot--syncing { background: #60a5fa; }
  .dot--ready { background: #34d399; }
  .dot--frozen { background: #f87171; }
  .dot--unsafe { background: #dc2626; }
  .dot--late { background: #a78bfa; }
</style>
```

---

## Summary Checklist

- [x] State machine enum + transition guards
- [x] Timer manager (signaling/syncing/BFT timeouts)
- [x] Extended room-state.ts with FSM integration
- [x] CRDT snapshot + delta protocol
- [x] BFT ValidatorSet formation + rotation
- [x] Late joiner catch-up manager
- [x] Peer departure + quorum recalculation
- [x] Svelte UI for state visualization

**Next Steps**:
1. Copy these patterns into actual files
2. Integrate with existing WASM module (bft/hotstuff.rs, crdt/mod.rs)
3. Add unit tests for state transitions + timeouts
4. E2E test: 2-peer room → 3-peer room → late joiner → peer crash

---

*KG-bound to SPAN_333_RoomLifecycleResearch. Implementation begins in Phase 2 of SPAN_333_Integration.*
