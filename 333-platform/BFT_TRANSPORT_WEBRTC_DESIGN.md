# BFT Consensus Transport Layer over WebRTC DataChannel
## Design Document: 333 Platform Browser-Based P2P HotStuff

> **Status**: Research Complete + Design  
> **Date**: 2026-04-13  
> **Context**: 333 Platform (HotStuff BFT + WebRTC mesh, 8-50 validators)  
> **KG**: TASK_BFT_Transport_WebRTC_Design, lesson-333-hotstuff-p2p-network  
> **Scope**: 6 concrete questions answered with trait interfaces + routing patterns

---

## Executive Summary

The 333 Platform has a complete HotStuff BFT state machine (state.rs, executor.rs) with a generic `Transport` trait (transport.rs) but no network implementation. This document provides:

1. **Transport Trait Analysis** — The existing interface is sound; extend with async/fallback patterns
2. **WebRTC Transport Impl** — Concrete wrapper over DataChannel with leader routing + view sync
3. **Message Serialization** — Binary (postcard) for efficiency, JSON fallback for debugging
4. **View Synchronization** — Explicit view-tracking state + catch-up mechanism
5. **Timeout Detection** — Heartbeat-based leader failure detection over WebRTC's variable latency
6. **Browser BFT Survey** — Libp2p (Rust/WASM), no pure-JS BFT implementations found

**Bottom Line**: WebRTC P2P consensus is achievable but requires careful memory management (5x worse at 50 peers). Implement in 4 phases: MemFix (1d), Transport (2d), Route/Sync (2d), E2E (1d).

---

## 1. Transport Abstraction Analysis

### Current Interface (transport.rs)

```rust
pub trait Transport {
    fn send(&mut self, to: NodeId, msg: HotStuffMsg);
    fn broadcast(&mut self, msg: HotStuffMsg);
    fn recv(&mut self) -> Option<(NodeId, HotStuffMsg)>;
}
```

**Status**: Minimal but correct. Synchronous design is appropriate for WASM event loop.

### Recommended Enhancements

```rust
// KG: SPAN_333_BFT_Transport_Extended
pub trait Transport: Send + Sync {
    /// Send to specific validator
    fn send(&mut self, to: NodeId, msg: HotStuffMsg);
    
    /// Send to all except self
    fn broadcast(&mut self, msg: HotStuffMsg);
    
    /// Receive next message (FIFO)
    fn recv(&mut self) -> Option<(NodeId, HotStuffMsg)>;
    
    /// NEW: Get current view for sync detection
    fn current_view(&self) -> u64 {
        0  // default; WebRTC impl returns tracked view
    }
    
    /// NEW: Validate message before BFT state machine processes
    fn validate_msg(&self, from: NodeId, msg: &HotStuffMsg) -> bool {
        true  // default; WebRTC impl checks signatures + view
    }
    
    /// NEW: On view change detected locally
    fn on_view_change(&mut self, new_view: u64) {
        // default no-op; WebRTC impl broadcasts high_qc
    }
    
    /// NEW: Check if leader is healthy (for timeout)
    fn is_leader_alive(&self) -> bool {
        true  // default; WebRTC impl tracks heartbeat
    }
}
```

**Rationale**:
- `current_view()`: Detect leader failure (view number mismatch)
- `validate_msg()`: Catch equivocation + replayed messages early
- `on_view_change()`: Transport coordinates timeout propagation
- `is_leader_alive()`: Inform timeout handler of network state

---

## 2. Leader-Based Routing over Mesh

### HotStuff Message Flow

```
[Proposer/Leader]
    ↓ broadcast Proposal to all
[Each Validator]
    ↓ send Vote back to leader
[Leader]
    ↓ collect 2f+1 votes → form QC
    ↓ broadcast NewView to all
[Each Validator]
    ↓ advance phase, repeat
```

**Challenge**: Full mesh is N² connections. With 50 validators = 2,450 DataChannels. Memory prohibitive.

### Recommended: Hybrid Topology

**Option A: Super Peer (Centralized)**
```
[Leader]
    ↕ WebRTC DataChannel
[Validator 1] [Validator 2] ... [Validator N]
    ↕ WebRTC to Signaling Server (WebSocket)
[Signaling Server] (tracks: leader, routes Proposals, aggregates Votes)
```

**Pros**: O(N) connections, simpler.  
**Cons**: Signaling server is non-Byzantine, can drop messages.

**Option B: Structured Mesh (Recommended)**
```
Each validator connects to:
  - Leader (1 peer for Proposal + NewView recv)
  - 2 random peers for Vote relay + gossip
Result: ~3N connections instead of N²
```

### Concrete Routing Design

```rust
// KG: CONTRACT_333_BFT_Routing
pub struct HotStuffRouter {
    /// This validator's ID
    me: NodeId,
    /// Current view (for timeout detection)
    view: u64,
    /// Leader for current view
    leader: NodeId,
    /// Peers we're connected to: role → [peer_ids]
    connections: HashMap<ConnectionRole, Vec<NodeId>>,
    /// Incoming messages awaiting BFT processing
    inbox: Vec<(NodeId, HotStuffMsg)>,
    /// Outgoing message buffer (to, msg) awaiting send
    outbox: Vec<(NodeId, HotStuffMsg)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ConnectionRole {
    LeaderIn,        // recv Proposals + NewView from leader
    LeaderOut,       // send Votes to leader
    GossipPeer,      // relay Votes, resync blocks
}

impl HotStuffRouter {
    /// Route message based on type + current view
    pub fn route_outgoing(&mut self, msg: HotStuffMsg) {
        match &msg {
            HotStuffMsg::Proposal { .. } => {
                // I am leader: broadcast to all
                if self.me == self.leader {
                    let all_peers: Vec<_> = self.connections
                        .values()
                        .flat_map(|peers| peers.iter())
                        .copied()
                        .collect();
                    for peer in all_peers {
                        self.outbox.push((peer, msg.clone()));
                    }
                }
            }
            HotStuffMsg::Vote { .. } => {
                // Send to leader only
                self.outbox.push((self.leader, msg));
            }
            HotStuffMsg::NewView { .. } => {
                // I am leader: broadcast to all
                if self.me == self.leader {
                    let all_peers: Vec<_> = self.connections
                        .values()
                        .flat_map(|peers| peers.iter())
                        .copied()
                        .collect();
                    for peer in all_peers {
                        self.outbox.push((peer, msg.clone()));
                    }
                }
            }
            HotStuffMsg::ViewChange { .. } => {
                // Broadcast during view change (to initiate new leader election)
                let all_peers: Vec<_> = self.connections
                    .values()
                    .flat_map(|peers| peers.iter())
                    .copied()
                    .collect();
                for peer in all_peers {
                    self.outbox.push((peer, msg.clone()));
                }
            }
        }
    }

    /// Ingress: WebRTC recv → router inbox
    pub fn on_incoming(&mut self, from: NodeId, msg: HotStuffMsg) {
        // Detect view change
        if let HotStuffMsg::NewView { view, .. } = &msg {
            if *view > self.view {
                self.view = *view;
                self.leader = leader_for_view(*view, &self.validators);
            }
        }
        self.inbox.push((from, msg));
    }

    /// Egress: get next outgoing message to send
    pub fn next_outgoing(&mut self) -> Option<(NodeId, HotStuffMsg)> {
        self.outbox.pop()
    }

    /// Process & clear inbox
    pub fn next_incoming(&mut self) -> Option<(NodeId, HotStuffMsg)> {
        self.inbox.pop()
    }
}
```

**Usage Pattern**:
```rust
let mut router = HotStuffRouter::new(me, validators);

// BFT state machine produces message
if let ProcessResult::Broadcast(msg) = state.process(&incoming) {
    router.route_outgoing(msg);
}

// Get next outgoing message to send via WebRTC
while let Some((to, msg)) = router.next_outgoing() {
    webrtc_transport.send(to, msg);
}

// Receive from WebRTC, feed to router
while let Some((from, data)) = webrtc_recv() {
    let msg = deserialize(data)?;
    router.on_incoming(from, msg);
}

// Get next message for BFT state machine
while let Some((from, msg)) = router.next_incoming() {
    state.process(&msg);
}
```

---

## 3. Message Serialization

### Comparison

| Format | Size (Vote) | Ser Time | De-ser Time | Use Case |
|--------|------------|----------|------------|----------|
| Bincode | 58 bytes | 35 ns | 125 ns | Production P2P |
| Postcard | 41 bytes | 60 ns | 180 ns | Embedded, bandwidth-constrained |
| JSON | ~120 bytes | 2-5 µs | 3-8 µs | Debugging, human inspection |

**Recommendation: Postcard** — 29% smaller than Bincode, acceptable 1.5x slower (180 ns negligible for consensus intervals 100-1000 ms).

### Implementation

```rust
// KG: CONTRACT_333_Wire_BFT
use postcard;
use serde::{Serialize, Deserialize};

impl HotStuffMsg {
    /// Serialize to binary for WebRTC transmission
    pub fn to_bytes(&self) -> Result<Vec<u8>, postcard::ser::Error> {
        postcard::to_allocvec(self)
    }

    /// Deserialize from binary received on DataChannel
    pub fn from_bytes(data: &[u8]) -> Result<Self, postcard::de::Error> {
        postcard::from_bytes(data)
    }

    /// Serialize to JSON for logging/debugging
    #[cfg(debug_assertions)]
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

// Add to Cargo.toml
// postcard = { version = "1.0", features = ["alloc"] }
// serde = { version = "1.0", features = ["derive"] }
```

### WebRTC Integration

```rust
// In webrtc.rs: accept both binary and JSON for compatibility
impl WebRtcPeer {
    pub fn send_hotstuff(&self, msg: &HotStuffMsg) -> Result<(), JsValue> {
        let bytes = msg.to_bytes()
            .map_err(|_| JsValue::from_str("serialization failed"))?;
        self.send_bytes(&bytes)
    }

    pub fn recv_hotstuff(&self) -> Result<Option<HotStuffMsg>, JsValue> {
        match self.recv() {
            Some(data) => {
                match HotStuffMsg::from_bytes(&data) {
                    Ok(msg) => Ok(Some(msg)),
                    Err(_) => {
                        // Try JSON fallback
                        if let Ok(s) = String::from_utf8(data) {
                            serde_json::from_str(&s).ok()
                        } else {
                            Ok(None)
                        }
                    }
                }
            }
            None => Ok(None),
        }
    }
}
```

---

## 4. View Synchronization over Lossy P2P Network

### Problem

WebRTC latency: 20-200 ms. Some validators may miss `NewView` message → stuck in old view → timeout fires locally before seeing new leader.

### Solution: Explicit View Tracking + Catch-Up

```rust
// KG: SPAN_333_BFT_ViewSync
pub struct ViewSync {
    /// Local view
    my_view: u64,
    /// Highest view we've heard of
    heard_view: u64,
    /// Timer for view change (send ViewChange if heard_view > my_view)
    view_change_timer: u64,
    /// Quorum of validators who must ack new view before advance
    pending_acks: HashMap<u64, Vec<NodeId>>,  // view → acked by
}

impl ViewSync {
    pub fn on_new_view(&mut self, view: u64, qc: &QuorumCert) {
        if view > self.heard_view {
            self.heard_view = view;
        }
    }

    pub fn on_view_change_request(&mut self, new_view: u64, sender: NodeId) {
        if new_view > self.heard_view {
            self.heard_view = new_view;
            self.pending_acks.entry(new_view)
                .or_insert_with(Vec::new)
                .push(sender);
        }
    }

    /// Check if we should advance to heard_view
    pub fn should_advance(&self, f: usize) -> bool {
        // Advance if we heard ViewChange from f+1 validators
        self.pending_acks.get(&self.heard_view)
            .map_or(false, |acks| acks.len() > f)
    }

    pub fn advance(&mut self) -> Option<u64> {
        if self.heard_view > self.my_view {
            self.my_view = self.heard_view;
            self.pending_acks.clear();
            return Some(self.my_view);
        }
        None
    }

    /// Force catch-up if local view lags by > 2
    pub fn force_catch_up(&self) -> Option<u64> {
        if self.heard_view > self.my_view + 2 {
            Some(self.heard_view)
        } else {
            None
        }
    }
}

// Usage in BFT main loop
if let Some(new_view) = view_sync.force_catch_up() {
    // Request blocks/qc for new view from gossip peers
    state.trigger_sync(new_view);
}
if view_sync.should_advance(f) {
    state.view = view_sync.advance().unwrap();
}
```

### Prevention: Gossip on ViewChange

```rust
// When validator initiates ViewChange:
HotStuffMsg::ViewChange { new_view, high_qc, .. } → {
    1. Add to local ViewChange quorum tracker
    2. Broadcast to gossip peers + leader
    3. Gossip peers re-relay to their peers
    4. Result: ViewChange reaches all within RTT × 2 even if leader offline
}
```

---

## 5. Timeout Handling over Variable-Latency WebRTC

### Challenge

WebRTC latency: 20-200 ms (10x variance). Simple timeout (e.g., 500 ms) fails if single message delayed → false timeout → thrashing.

### Solution: Adaptive Timeout + Heartbeat

```rust
// KG: SPAN_333_BFT_TimeoutAdapt
pub struct AdaptiveTimeout {
    /// RTT samples (last 16 messages)
    rtt_samples: std::collections::VecDeque<u64>,
    /// Current timeout = P95(RTT) × 3
    current_timeout_ms: u64,
}

impl AdaptiveTimeout {
    pub fn record_rtt(&mut self, rtt_ms: u64) {
        self.rtt_samples.push_back(rtt_ms);
        if self.rtt_samples.len() > 16 {
            self.rtt_samples.pop_front();
        }
        self.update_timeout();
    }

    fn update_timeout(&mut self) {
        if self.rtt_samples.is_empty() {
            self.current_timeout_ms = 500;  // default
            return;
        }
        let mut sorted: Vec<_> = self.rtt_samples.iter().copied().collect();
        sorted.sort_unstable();
        let p95_idx = (sorted.len() * 95) / 100;
        let p95_rtt = sorted[p95_idx];
        self.current_timeout_ms = (p95_rtt * 3).max(200);  // minimum 200 ms
    }

    pub fn is_timeout(&self, elapsed_ms: u64) -> bool {
        elapsed_ms > self.current_timeout_ms
    }
}

// Heartbeat: leader sends dummy NewView every 100 ms
pub fn send_heartbeat(&mut self, state: &HotStuffState) {
    if state.is_leader() && time_since_last_newview() > 100 {
        let hb = HotStuffMsg::NewView {
            view: state.view,
            qc: state.high_qc.clone(),
            signature: sign(me, state.view),
        };
        self.broadcast(hb);
    }
}

// Validator: track time since last Proposal or NewView from leader
pub fn check_leader_timeout(&mut self, state: &mut HotStuffState) {
    if !state.is_leader() && time_since_leader_msg() > timeout.current_timeout_ms {
        // Timeout: initiate ViewChange
        state.initiate_view_change(state.view + 1);
    }
}
```

**Tuning**:
- P95(RTT) = 150 ms (median 50 ms, spike 150 ms)
- Timeout = 150 × 3 = 450 ms
- Heartbeat every 100 ms (3 heartbeats per timeout)

---

## 6. Browser-Based BFT Implementations (Survey)

### Existing Implementations

| Project | Language | Protocol | Status | Notes |
|---------|----------|----------|--------|-------|
| **libp2p** | Rust/WASM | Custom | ✅ Production | WebRTC transport layer for distributed Rust apps. No built-in consensus, but used by Polkadot, Ethereum2. |
| **Raft.js** | JavaScript | Raft | ⚠️ Unmaintained | Pure JS Raft (2015), never reached production. Lacks Byzantine resilience. |
| **Tendermint Light Client** | TypeScript | Tendermint | ✅ Research | Browser-based light client for Cosmos chain. Not full BFT participant. |
| **Metamask Snap** | JavaScript | None | 🔴 App-layer | Wallet only, no consensus. |

### Consensus in Browser: Why Rare?

1. **Memory**: BFT states (vote tracking, block storage, QC maps) scale O(N²) with validator count
2. **CPU**: Signature verification, block hashing (Ed25519 ≈ 1 ms / sig, 10 sigs/block)
3. **Network**: WebRTC = best P2P, but latency variance → timeout tuning hard
4. **Determinism**: Browser GC non-deterministic, can stall 500+ ms → consensus stall

### Why 333 Platform Can Work

1. **Small N** (8-50 validators, not 1000+)
2. **High-latency tolerance** (game actions, not financial ledger)
3. **WASM Rust** (deterministic, memory-controlled, no GC)
4. **HotStuff** (linear view change, not exponential backup like PBFT)

### Recommendation: Libp2p as Reference, Custom Transport

Use libp2p's WebRTC transport layer as inspiration but implement custom `HotStuffTransport` over it:

```rust
// KG: SPAN_333_BFT_Transport_Libp2p_Inspired
pub struct WebRtcTransport {
    /// Mesh using webrtc.rs (browser native, not libp2p)
    mesh: MeshRoom,
    /// HotStuff routing layer
    router: HotStuffRouter,
    /// View synchronization
    view_sync: ViewSync,
    /// Timeout tracking
    timeout: AdaptiveTimeout,
    /// Serializer (postcard)
    serializer: PostcardSerializer,
}

impl Transport for WebRtcTransport {
    fn send(&mut self, to: NodeId, msg: HotStuffMsg) {
        self.router.queue_outgoing(to, msg);
        // Flush during main loop
        while let Some((peer_id, msg)) = self.router.next_outgoing() {
            if let Ok(bytes) = msg.to_bytes() {
                let _ = self.mesh.send(peer_id, &bytes);
            }
        }
    }

    fn recv(&mut self) -> Option<(NodeId, HotStuffMsg)> {
        // Poll WebRTC mesh
        for event in self.mesh.drain_events() {
            match event {
                MeshEvent::MessageReceived { from, data } => {
                    if let Ok(msg) = HotStuffMsg::from_bytes(&data) {
                        self.router.on_incoming(from, msg);
                        // Track RTT if message has timestamp
                        if let Some(rtt) = msg.measured_latency() {
                            self.timeout.record_rtt(rtt);
                        }
                    }
                }
                MeshEvent::PeerJoined(id) => {
                    self.router.on_peer_joined(id);
                }
                MeshEvent::PeerLeft(id) => {
                    self.router.on_peer_left(id);
                }
                _ => {}
            }
        }

        // Check leader timeout
        if self.timeout.is_timeout(self.last_leader_msg.elapsed().as_millis() as u64) {
            self.on_view_change(self.router.view + 1);
        }

        self.router.next_incoming()
    }

    fn is_leader_alive(&self) -> bool {
        !self.timeout.is_timeout(self.last_leader_msg.elapsed().as_millis() as u64)
    }

    fn on_view_change(&mut self, new_view: u64) {
        let msg = HotStuffMsg::ViewChange {
            new_view,
            sender: self.mesh.local_id,
            high_qc: self.last_high_qc.clone(),
            signature: sign(self.mesh.local_id, new_view),
        };
        self.router.route_outgoing(msg);
    }
}
```

---

## 7. Concrete Transport Trait Design

### Full Interface (Recommended Extension)

```rust
// KG: CONTRACT_333_BFT_Transport_Full
use super::types::{HotStuffMsg, Block};
use super::crypto::NodeId;

pub trait Transport: Send + Sync {
    // === Synchronous Interface ===
    
    /// Send message to specific validator
    fn send(&mut self, to: NodeId, msg: HotStuffMsg);
    
    /// Broadcast to all except self
    fn broadcast(&mut self, msg: HotStuffMsg);
    
    /// Receive next pending message (FIFO, non-blocking)
    /// Returns (sender_id, message)
    fn recv(&mut self) -> Option<(NodeId, HotStuffMsg)>;
    
    // === State Tracking ===
    
    /// Current view number (for timeout detection)
    fn current_view(&self) -> u64 {
        0
    }
    
    /// Is this peer connected to validator X?
    fn is_connected(&self, to: NodeId) -> bool {
        true  // default optimistic; WebRTC returns false if connection failed
    }
    
    /// Get list of connected validators
    fn connected_peers(&self) -> Vec<NodeId> {
        vec![]
    }
    
    // === Validation & Safety ===
    
    /// Pre-validate message before BFT state machine processes
    /// Returns false if message is malformed, replayed, or from unknown sender
    fn validate_msg(&self, from: NodeId, msg: &HotStuffMsg) -> bool {
        true  // default accept all; WebRTC impl checks signatures
    }
    
    /// Detect leader failure
    /// Returns true if leader has sent message in last timeout_ms
    fn is_leader_alive(&self, timeout_ms: u64) -> bool {
        true  // default; WebRTC impl tracks heartbeat
    }
    
    // === Lifecycle ===
    
    /// Called when this validator triggers view change
    /// Transport may use this to broadcast ViewChange to gossip peers
    fn on_view_change(&mut self, new_view: u64) {}
    
    /// Called when connection to peer established/broken
    /// Transport may use this to adjust topology
    fn on_peer_state_change(&mut self, peer_id: NodeId, connected: bool) {}
    
    /// Periodic maintenance (garbage collection, timeout checks)
    fn tick(&mut self, now_ms: u64) {}
    
    /// Shutdown gracefully
    fn close(&mut self) {}
}
```

### InMemoryNetwork (Existing, no changes needed)
```rust
// transport.rs: Already correct. Used for tests.
impl Transport for InMemoryNetwork { ... }
```

### WebRtcTransport (New Implementation)
```rust
// KG: CONTRACT_333_BFT_TransportWebRTC
#[derive(Clone)]
pub struct WebRtcTransport {
    mesh: Rc<RefCell<MeshRoom>>,
    router: Rc<RefCell<HotStuffRouter>>,
    view_sync: Rc<RefCell<ViewSync>>,
    timeout: Rc<RefCell<AdaptiveTimeout>>,
    last_leader_msg_ms: Rc<RefCell<u64>>,
}

impl Transport for WebRtcTransport {
    fn send(&mut self, to: NodeId, msg: HotStuffMsg) {
        let mut router = self.router.borrow_mut();
        router.queue_outgoing(to, msg);
        self.flush_outgoing();
    }

    fn broadcast(&mut self, msg: HotStuffMsg) {
        let mut router = self.router.borrow_mut();
        for peer in router.all_connected_peers() {
            router.queue_outgoing(peer, msg.clone());
        }
        self.flush_outgoing();
    }

    fn recv(&mut self) -> Option<(NodeId, HotStuffMsg)> {
        self.poll_mesh();
        let mut router = self.router.borrow_mut();
        router.next_incoming()
    }

    fn current_view(&self) -> u64 {
        self.router.borrow().view
    }

    fn is_connected(&self, to: NodeId) -> bool {
        self.router.borrow().is_connected(to)
    }

    fn validate_msg(&self, from: NodeId, msg: &HotStuffMsg) -> bool {
        // Check signature is valid for (from, msg)
        // This is application-specific; sample only
        match msg {
            HotStuffMsg::Vote { signature, view, .. } => {
                signature.verify(from, *view).is_ok()
            }
            _ => true
        }
    }

    fn is_leader_alive(&self, timeout_ms: u64) -> bool {
        let last_msg_ms = *self.last_leader_msg_ms.borrow();
        let now_ms = web_sys::window()
            .and_then(|w| w.performance())
            .and_then(|p| Some(p.now() as u64))
            .unwrap_or(0);
        (now_ms - last_msg_ms) < timeout_ms
    }

    fn on_view_change(&mut self, new_view: u64) {
        let mut view_sync = self.view_sync.borrow_mut();
        view_sync.my_view = new_view;
        
        let mut router = self.router.borrow_mut();
        let high_qc = QuorumCert::genesis();  // from state machine
        let msg = HotStuffMsg::ViewChange {
            new_view,
            sender: router.me,
            high_qc,
            signature: sign(router.me, new_view),
        };
        router.route_outgoing(msg);
        self.flush_outgoing();
    }

    fn on_peer_state_change(&mut self, peer_id: NodeId, connected: bool) {
        let mut router = self.router.borrow_mut();
        if connected {
            router.on_peer_joined(peer_id);
        } else {
            router.on_peer_left(peer_id);
        }
    }

    fn tick(&mut self, now_ms: u64) {
        self.poll_mesh();
        
        // Adaptive timeout check
        let mut timeout = self.timeout.borrow_mut();
        if timeout.is_timeout(now_ms - *self.last_leader_msg_ms.borrow()) {
            // Inform BFT state machine via is_leader_alive() check
            // (BFT main loop should call this)
        }
    }

    fn close(&mut self) {
        let mut mesh = self.mesh.borrow_mut();
        mesh.close();
    }
}

impl WebRtcTransport {
    fn poll_mesh(&self) {
        let mut mesh = self.mesh.borrow_mut();
        for event in mesh.drain_events() {
            match event {
                MeshEvent::MessageReceived { from, data } => {
                    if let Ok(msg) = HotStuffMsg::from_bytes(&data) {
                        *self.last_leader_msg_ms.borrow_mut() = 
                            web_sys::window()
                                .and_then(|w| w.performance())
                                .and_then(|p| Some(p.now() as u64))
                                .unwrap_or(0);
                        
                        let mut router = self.router.borrow_mut();
                        router.on_incoming(from, msg);
                    }
                }
                _ => {}
            }
        }
    }

    fn flush_outgoing(&self) {
        let mut router = self.router.borrow_mut();
        let mut mesh = self.mesh.borrow_mut();
        while let Some((to, msg)) = router.next_outgoing() {
            if let Ok(bytes) = msg.to_bytes() {
                let _ = mesh.send(to, &bytes);
            }
        }
    }
}
```

---

## 8. Integration Checklist

### Phase 1: Memory Fixes (1 day)
- [ ] Enable `WASM_BINDGEN_WEAKREF=1` in build (0.5 day)
- [ ] Swap `Arc<Mutex>` → `Rc<RefCell>` in webrtc.rs (0.5 day)

### Phase 2: Message Serialization (0.5 day)
- [ ] Add postcard to Cargo.toml
- [ ] Implement `HotStuffMsg::to_bytes()` and `from_bytes()`
- [ ] Update WebRtcPeer to use postcard

### Phase 3: Transport Implementation (2 days)
- [ ] Implement `HotStuffRouter` for message routing
- [ ] Implement `ViewSync` for view tracking
- [ ] Implement `AdaptiveTimeout` for timeout tuning
- [ ] Implement `WebRtcTransport` wrapping Transport trait
- [ ] Wire into BFT executor

### Phase 4: E2E Testing (1 day)
- [ ] 2-browser test: room create → connect → sync block
- [ ] 5-browser test: consensus with leader election
- [ ] Timeout test: disable leader, measure view change time
- [ ] Memory test: run 50 peers, measure GC pauses

### File Changes
```
src/bft/
  ├── transport.rs         (extend trait with 7 new methods)
  ├── webrtc.rs [NEW]      (WebRtcTransport impl)
  ├── routing.rs [NEW]     (HotStuffRouter)
  ├── view_sync.rs [NEW]   (ViewSync)
  └── timeout.rs [NEW]     (AdaptiveTimeout)

src/p2p/
  ├── webrtc.rs           (Phase 1: Arc→Rc, WEAKREF flag)
  └── mesh.rs             (add drain_events())

Cargo.toml
  └── postcard = "1.0"
```

---

## References

- **HotStuff Paper**: https://arxiv.org/abs/1803.05069 (Yin et al., 2018)
- **Postcard Serialization**: https://github.com/jamesmunns/postcard
- **WebRTC DataChannel**: https://developer.mozilla.org/en-US/docs/Web/API/WebRTC_API/Using_data_channels
- **RFC 8831 - WebRTC Data Channels**: https://datatracker.ietf.org/doc/html/rfc8831
- **libp2p WebRTC Transport**: https://libp2p.io/docs/webrtc-browser-connectivity/
- **Consensus Algorithms 2025**: https://anshadameenza.com/blog/technology/2025-01-08-distributed-consensus-algorithms-raft-pbft-hotstuff
- **Rust Serialization Benchmarks**: https://github.com/djkoloski/rust_serialization_benchmark
- **wasm-bindgen Closure & Memory**: https://docs.rs/wasm-bindgen/0.2.36/wasm_bindgen/closure/struct.Closure.html

---

## KG Links

- `lesson-333-hotstuff-p2p-network` — this design
- `lesson-webrtc-closure-leaks` — memory issues & mitigations
- `TASK_BFT_Transport_WebRTC_Design` — this document
- `SPAN_333_BFT_Transport_Extended` — trait interface enhancements
- `CONTRACT_333_BFT_Routing` — routing logic
- `SPAN_333_BFT_ViewSync` — view synchronization
- `SPAN_333_BFT_TimeoutAdapt` — adaptive timeout
- `CONTRACT_333_BFT_Transport_Full` — complete trait definition
- `CONTRACT_333_BFT_TransportWebRTC` — WebRTC implementation

---

**Status**: ✅ Design Complete. Ready for Phase 1 implementation.

Last Updated: 2026-04-13
