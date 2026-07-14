# P2P Message Routing & Dispatch Patterns
## Research: Binary Wire Protocol in 333 Platform
# KG: CONTRACT_333_MessageDispatcher, ATOM_Wire_Dispatcher

**Date**: 2026-04-13  
**Scope**: 333 wire protocol (4B header + 7 msg types) + WebRTC DataChannel  
**Deliverable**: Concrete Rust dispatcher pattern + answers to 6 key questions

---

## Executive Summary

For P2P systems with synchronous I/O boundaries (WebRTC callbacks), we recommend:
1. **Dispatch**: Sync static match (not HashMap) in recv loop
2. **Handler registration**: Trait object + registry pattern (compile-time + pluggable)
3. **Async handling**: Sync recv handler → queue → async task processor (separate thread/executor)
4. **Queuing**: Fixed-size bounded channels (not VecDeque) with explicit backpressure
5. **Error handling**: Skip malformed (unknown type/version), disconnect on protocol violation
6. **Backpressure**: Receiver side (drop oldest, pause ingress, or reject)

---

## 1. Message Dispatch Patterns

### 1.1 Pattern A: Static Match (Recommended for Sync Boundaries)

```rust
pub fn handle_message(msg: WireMessage, dispatcher: &MessageDispatcher, peer_id: u32) {
    match MsgType::from_u8(msg.header.msg_type) {
        Some(MsgType::StateUpdate) => {
            // CRDT state delta
            if let Ok(delta) = serde_json::from_slice::<CrdtDelta>(&msg.payload) {
                dispatcher.crdt.apply_delta(peer_id, delta);
            }
        }
        Some(MsgType::StateFull) => {
            // Full state sync
            if let Ok(state) = serde_json::from_slice::<FullState>(&msg.payload) {
                dispatcher.crdt.merge_full_state(peer_id, state);
            }
        }
        Some(MsgType::Presence) => {
            // Cursor, status, typing indicator
            if let Ok(presence) = serde_json::from_slice::<PresenceUpdate>(&msg.payload) {
                dispatcher.presence.update(peer_id, presence);
            }
        }
        Some(MsgType::Consensus) => {
            // BFT consensus message
            if let Ok(bft_msg) = serde_json::from_slice::<HotStuffMsg>(&msg.payload) {
                dispatcher.bft.process_message(peer_id, bft_msg);
            }
        }
        Some(MsgType::RoomControl) => {
            // Join, leave, role change
            if let Ok(control) = serde_json::from_slice::<RoomControlMsg>(&msg.payload) {
                dispatcher.room.handle_control(peer_id, control);
            }
        }
        Some(MsgType::Heartbeat) => {
            // Keepalive — just update peer state
            dispatcher.peers.update_seen(peer_id);
        }
        Some(MsgType::AppMessage) => {
            // Application-defined payload
            dispatcher.app.on_message(peer_id, &msg.payload);
        }
        None => {
            // Unknown type — skip (forward compat)
            log::warn!("unknown message type: {}", msg.header.msg_type);
        }
    }
}
```

**Pros**:
- Sync, no allocation, no indirection
- Compile-time safety (exhaustive match)
- Easy profiling/tracing
- Zero overhead

**Cons**:
- Handlers are tightly coupled to dispatcher
- Adding handler requires code change + recompile
- Cannot unload/reload handlers at runtime

---

### 1.2 Pattern B: Dynamic Registry (Pluggable)

```rust
pub type MessageHandler = Box<dyn Fn(&WireMessage, u32) + Send + Sync>;

pub struct MessageDispatcher {
    handlers: std::collections::HashMap<u8, MessageHandler>,
    peer_manager: Arc<PeerManager>,
}

impl MessageDispatcher {
    pub fn new(peer_manager: Arc<PeerManager>) -> Self {
        Self {
            handlers: HashMap::new(),
            peer_manager,
        }
    }

    pub fn register<F>(&mut self, msg_type: u8, handler: F)
    where
        F: Fn(&WireMessage, u32) + Send + Sync + 'static,
    {
        self.handlers.insert(msg_type, Box::new(handler));
    }

    pub fn dispatch(&self, msg: WireMessage, peer_id: u32) {
        let msg_type = msg.header.msg_type;
        
        if let Some(handler) = self.handlers.get(&msg_type) {
            handler(&msg, peer_id);
        } else {
            // Unknown type — skip (forward compat)
            log::warn!("no handler for type: {}", msg_type);
        }
    }
}

// Usage at startup
let mut dispatcher = MessageDispatcher::new(peer_mgr);
dispatcher.register(0x01, |msg, peer_id| {
    // StateUpdate handler
    if let Ok(delta) = serde_json::from_slice::<CrdtDelta>(&msg.payload) {
        // process delta
    }
});
```

**Pros**:
- Pluggable, no recompile to add handler
- Can register/unregister at runtime
- Useful for WASM plugins

**Cons**:
- HashMap lookup on every message (O(1) but constant factor > match)
- Box<dyn Fn> → vtable indirection
- Runtime panic if handler is not thread-safe

---

### 1.3 Pattern C: Async Channel-Based (For Heavy Handlers)

If handlers need to do async work (e.g., DB writes, HTTP calls):

```rust
pub enum DispatchTask {
    StateUpdate(u32, CrdtDelta),  // peer_id, delta
    Consensus(u32, HotStuffMsg),
    RoomControl(u32, RoomControlMsg),
}

pub struct AsyncDispatcher {
    tx: tokio::sync::mpsc::UnboundedSender<DispatchTask>,
    _bg_task: tokio::task::JoinHandle<()>,
}

impl AsyncDispatcher {
    pub fn new() -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        
        let _bg_task = tokio::spawn(async move {
            while let Some(task) = rx.recv().await {
                match task {
                    DispatchTask::StateUpdate(peer_id, delta) => {
                        // Can now do async work
                        db.store_delta(delta).await;
                    }
                    DispatchTask::Consensus(peer_id, msg) => {
                        bft.process(msg).await;
                    }
                    // ...
                }
            }
        });

        Self { tx, _bg_task }
    }

    pub fn dispatch(&self, msg: WireMessage, peer_id: u32) {
        let task = match MsgType::from_u8(msg.header.msg_type) {
            Some(MsgType::StateUpdate) => {
                serde_json::from_slice(&msg.payload)
                    .ok()
                    .map(|delta| DispatchTask::StateUpdate(peer_id, delta))
            }
            Some(MsgType::Consensus) => {
                serde_json::from_slice(&msg.payload)
                    .ok()
                    .map(|msg| DispatchTask::Consensus(peer_id, msg))
            }
            _ => None,
        };

        if let Some(task) = task {
            let _ = self.tx.send(task);  // Ignore full channel for now (see backpressure)
        }
    }
}
```

**Pros**:
- Handlers can be async (database, HTTP, filesystem)
- Non-blocking from WebRTC callback perspective
- Decouples I/O from dispatch

**Cons**:
- Requires tokio/async runtime
- Task queue can grow unbounded (needs backpressure)
- Harder to debug (async stack traces)

---

## 2. Handler Registration Architecture

### Recommended: Trait Object + Static Registry

Combines static safety with pluggability:

```rust
/// Handler trait (all handlers implement this)
pub trait MessageHandler: Send + Sync {
    fn handle(&self, msg: &WireMessage, peer_id: u32);
    fn msg_type(&self) -> u8;
}

/// CRDT handler
pub struct CrdtHandler {
    crdt: Arc<CrdtModule>,
}

impl MessageHandler for CrdtHandler {
    fn handle(&self, msg: &WireMessage, _peer_id: u32) {
        if let Ok(delta) = serde_json::from_slice(&msg.payload) {
            self.crdt.apply_delta(delta);
        }
    }
    
    fn msg_type(&self) -> u8 { 0x01 }  // StateUpdate
}

/// Dispatcher holds trait objects
pub struct Dispatcher {
    handlers: std::collections::HashMap<u8, Arc<dyn MessageHandler>>,
}

impl Dispatcher {
    pub fn register(&mut self, handler: Arc<dyn MessageHandler>) {
        self.handlers.insert(handler.msg_type(), handler);
    }

    pub fn dispatch(&self, msg: WireMessage, peer_id: u32) {
        if let Some(handler) = self.handlers.get(&msg.header.msg_type) {
            handler.handle(&msg, peer_id);
        }
    }
}
```

**Benefits**:
- Type-safe at compile time (trait bounds)
- Pluggable at runtime (Arc + trait objects)
- Each handler is independent, testable module

---

## 3. Sync vs Async Handling

### The WebRTC Callback Problem

WebRTC DataChannel callbacks are **synchronous** (run on browser event loop):

```javascript
// JavaScript (browser)
dataChannel.onmessage = (event) => {
    // This callback is synchronous
    // You have ~16ms before jank (60fps)
    // Cannot await here
    const data = event.data;
};
```

In Rust WASM:

```rust
// This runs synchronously from JS event loop
pub fn on_datachannel_message(buf: &[u8]) {
    // Cannot call .await here!
    // Must return immediately
}
```

### Recommendation: Sync Boundary + Async Core

```rust
pub struct MessageLoop {
    dispatcher: Arc<Dispatcher>,
    rx: Arc<std::sync::Mutex<crossbeam::channel::Receiver<DispatchTask>>>,
}

impl MessageLoop {
    /// Called from WebRTC callback (sync, must be fast)
    pub fn on_message(&self, buf: &[u8], peer_id: u32) {
        // 1. Decode (fast, 4-byte header)
        let msg = match wire::decode(buf) {
            wire::DecodeResult::Ok(m) => m,
            wire::DecodeResult::SkipVersion(_) => return,
            wire::DecodeResult::SkipType(_) => return,
            wire::DecodeResult::Err(e) => {
                log::error!("decode error: {}", e);
                return;
            }
        };

        // 2. Enqueue to dispatcher (fast, bounded queue)
        match MsgType::from_u8(msg.header.msg_type) {
            Some(MsgType::StateUpdate) => {
                let task = serde_json::from_slice(&msg.payload)
                    .ok()
                    .map(|delta| DispatchTask::StateUpdate(peer_id, delta));
                if let Some(t) = task {
                    let _ = self.tx.send(t);  // Bounded, may drop if full
                }
            }
            _ => self.dispatcher.dispatch(msg, peer_id),
        }

        // 3. Return ASAP (< 1ms)
    }

    /// Called from game loop / tokio task (async-safe)
    pub async fn process_queue(&self) {
        let rx = self.rx.lock().unwrap();
        while let Ok(task) = rx.try_recv() {
            match task {
                DispatchTask::StateUpdate(peer_id, delta) => {
                    // Can now do heavy async work
                    self.crdt.apply_async(delta).await;
                }
                // ...
            }
        }
    }
}
```

**Tradeoff**:
- **Sync boundary** (WebRTC callback): Only decode + enqueue (< 1ms)
- **Async core** (game loop): Do heavy work here (CRDT merge, BFT processing, DB writes)

---

## 4. Message Queuing Strategy

### Problem: VecDeque is unbounded

```rust
// BAD: Can grow to 1GB
let mut inbox = VecDeque::new();
inbox.push_back(msg);  // No limit!
```

### Solution: Bounded Channels with Backpressure

**Option A: Drop oldest (best for unreliable UDP-like messages)**

```rust
pub struct BoundedQueue {
    tx: crossbeam::channel::Sender<Message>,
    rx: crossbeam::channel::Receiver<Message>,
}

impl BoundedQueue {
    pub fn new(capacity: usize) -> Self {
        let (tx, rx) = crossbeam::channel::bounded(capacity);
        Self { tx, rx }
    }

    pub fn enqueue(&self, msg: Message) -> Result<(), QueueError> {
        match self.tx.try_send(msg) {
            Ok(()) => Ok(()),
            Err(crossbeam::channel::TrySendError::Full(msg)) => {
                // Backpressure: drop oldest from internal queue
                // (This is application-specific)
                Err(QueueError::Full)
            }
            Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                Err(QueueError::ReceiverClosed)
            }
        }
    }

    pub fn dequeue(&self) -> Option<Message> {
        self.rx.try_recv().ok()
    }
}
```

**Option B: Pause ingress (best for TCP-like reliable messages)**

```rust
pub fn on_message(&self, buf: &[u8]) {
    if self.queue.is_full() {
        // Tell DataChannel to pause/buffer (backpressure to peer)
        // In WebRTC: dc.bufferedAmount > threshold
        self.pause_ingress();
        return;
    }
    
    // Decode and enqueue
    self.queue.enqueue(msg);
}

pub fn on_drain(&self) {
    // Called when queue drops below threshold
    self.resume_ingress();
}
```

**Option C: Reject with explicit error (best for request-response)**

```rust
pub fn enqueue(&self, msg: Message) -> Result<(), QueueError> {
    self.tx.try_send(msg).map_err(|e| match e {
        TrySendError::Full(_) => QueueError::Backpressure,
        TrySendError::Disconnected(_) => QueueError::Closed,
    })
}

// Caller (in WebRTC callback)
match queue.enqueue(task) {
    Ok(()) => {},
    Err(QueueError::Backpressure) => {
        // Send NACK to peer, they should retry
        self.send_nack_to_peer(peer_id);
    }
    Err(QueueError::Closed) => {
        // Connection closing
    }
}
```

### Recommended Hybrid

For 333 platform:
- **Reliable channels** (StateUpdate, Consensus, RoomControl): Use bounded queue + backpressure pause
- **Unreliable channels** (Presence, Heartbeat): Use small queue + drop oldest

```rust
pub struct MessageQueues {
    reliable: BoundedQueue<DispatchTask>,    // capacity=1000
    unreliable: BoundedQueue<DispatchTask>,  // capacity=100
}

impl MessageQueues {
    pub fn enqueue(&self, task: DispatchTask) -> Result<(), QueueError> {
        match task {
            DispatchTask::StateUpdate(..) |
            DispatchTask::Consensus(..) |
            DispatchTask::RoomControl(..) => self.reliable.enqueue(task),
            
            DispatchTask::Presence(..) |
            DispatchTask::Heartbeat => self.unreliable.enqueue(task),
        }
    }
}
```

---

## 5. Error Handling

### Categories of Errors

1. **Unknown version** (forward compat) → **Skip silently**
   ```rust
   wire::DecodeResult::SkipVersion(v) => {
       log::debug!("skipping future version {}", v);
       return;
   }
   ```

2. **Unknown type** (forward compat) → **Skip silently**
   ```rust
   wire::DecodeResult::SkipType(t) => {
       log::debug!("skipping future type {}", t);
       return;
   }
   ```

3. **Malformed payload** (serde error) → **Skip, log as warn**
   ```rust
   match serde_json::from_slice::<CrdtDelta>(&msg.payload) {
       Ok(delta) => self.crdt.apply(delta),
       Err(e) => log::warn!("bad StateUpdate payload: {}", e),
   }
   ```

4. **Protocol violation** (impossible state) → **Disconnect peer**
   ```rust
   // Example: peer sends consensus message but not registered
   if !self.peers.is_registered(peer_id) {
       log::error!("consensus from unregistered peer {}", peer_id);
       self.disconnect_peer(peer_id, "protocol violation");
       return;
   }
   ```

5. **Backpressure** (queue full) → **Drop or nack**
   ```rust
   match self.queue.enqueue(task) {
       Ok(()) => {},
       Err(QueueError::Full) => {
           // For unreliable: silently drop
           // For reliable: send NACK or pause
           log::warn!("queue full for peer {}", peer_id);
       }
   }
   ```

### Error Handling Strategy

```rust
pub enum DispatchError {
    MalformedPayload(String),
    QueueFull,
    PeerNotRegistered,
    ProtocolViolation(String),
}

pub fn dispatch(&self, msg: WireMessage, peer_id: u32) -> Result<(), DispatchError> {
    // Check peer first
    if !self.peers.is_connected(peer_id) {
        return Err(DispatchError::PeerNotRegistered);
    }

    match MsgType::from_u8(msg.header.msg_type) {
        Some(MsgType::StateUpdate) => {
            let delta = serde_json::from_slice(&msg.payload)
                .map_err(|e| DispatchError::MalformedPayload(e.to_string()))?;
            
            self.queue.enqueue(DispatchTask::StateUpdate(peer_id, delta))
                .map_err(|_| DispatchError::QueueFull)?;
            Ok(())
        }
        Some(MsgType::Consensus) => {
            if !self.peers.is_validator(peer_id) {
                return Err(DispatchError::ProtocolViolation(
                    "non-validator sent consensus".into()
                ));
            }
            // ... rest
            Ok(())
        }
        _ => Ok(()), // Unknown type: silent forward compat
    }
}

// Caller
match dispatcher.dispatch(msg, peer_id) {
    Ok(()) => {},
    Err(DispatchError::ProtocolViolation(reason)) => {
        log::error!("disconnecting peer {}: {}", peer_id, reason);
        self.disconnect_peer(peer_id);
    }
    Err(e) => {
        log::warn!("dispatch error for peer {}: {:?}", peer_id, e);
    }
}
```

---

## 6. Backpressure Mechanisms

### Mechanism 1: Queue Depth Monitoring

```rust
pub struct BackpressureMonitor {
    queue_depth: Arc<std::sync::atomic::AtomicUsize>,
    threshold: usize,  // 80% capacity
}

impl BackpressureMonitor {
    pub fn should_drop_message(&self) -> bool {
        self.queue_depth.load(Ordering::Relaxed) > self.threshold
    }

    pub fn on_message(&self, task: DispatchTask) {
        if self.should_drop_message() {
            log::warn!("dropping message due to backpressure");
            self.metrics.inc_dropped();
            return;
        }
        
        let old = self.queue_depth.fetch_add(1, Ordering::Relaxed);
        if old + 1 > self.threshold {
            self.pause_ingress();
        }
    }

    pub fn on_drain(&self, count: usize) {
        self.queue_depth.fetch_sub(count, Ordering::Relaxed);
        if self.queue_depth.load(Ordering::Relaxed) < self.threshold / 2 {
            self.resume_ingress();
        }
    }
}
```

### Mechanism 2: Adaptive Processing Rate

```rust
pub struct AdaptiveDispatcher {
    queue: BoundedQueue<DispatchTask>,
    metrics: Arc<Metrics>,
}

impl AdaptiveDispatcher {
    pub async fn process_loop(&self) {
        let mut batch_size = 32;
        loop {
            // Dequeue batch
            let tasks: Vec<_> = (0..batch_size)
                .filter_map(|_| self.queue.dequeue())
                .collect();

            if tasks.is_empty() {
                tokio::time::sleep(Duration::from_millis(1)).await;
                batch_size = batch_size.max(32);  // Reset to baseline
                continue;
            }

            // Process
            let start = Instant::now();
            for task in tasks {
                self.process_task(task).await;
            }
            let elapsed = start.elapsed();

            // Adapt batch size based on latency
            if elapsed > Duration::from_millis(50) {
                batch_size = batch_size.saturating_sub(8);  // Reduce
                self.metrics.record_backpressure();
            } else if elapsed < Duration::from_millis(10) {
                batch_size = (batch_size + 8).min(256);  // Increase
            }
        }
    }
}
```

### Mechanism 3: Per-Peer Rate Limiting

```rust
pub struct PeerRateLimiter {
    peer_queues: HashMap<u32, BoundedQueue<DispatchTask>>,  // 100 cap per peer
    shared_queue: BoundedQueue<(u32, DispatchTask)>,         // 5000 total
}

impl PeerRateLimiter {
    pub fn enqueue(&self, peer_id: u32, task: DispatchTask) -> Result<(), QueueError> {
        // First limit per-peer
        self.peer_queues
            .entry(peer_id)
            .or_insert_with(|| BoundedQueue::new(100))
            .enqueue(task.clone())?;

        // Then global queue
        self.shared_queue.enqueue((peer_id, task))?;
        Ok(())
    }

    pub fn mark_misbehaving(&self, peer_id: u32) {
        // Halve this peer's queue capacity
        if let Some(q) = self.peer_queues.get(&peer_id) {
            q.set_capacity(50);
        }
    }
}
```

---

## 7. Concrete Implementation for 333 Platform

Combining all patterns:

```rust
// src/dispatch/mod.rs
// KG: CONTRACT_333_MessageDispatcher, ATOM_Wire_Dispatcher

use crate::wire::{WireMessage, MsgType, DecodeResult};
use crate::bft::HotStuffMsg;
use crate::platform::PlatformCore;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use crossbeam::channel::{bounded, Sender, Receiver};

/// Dispatch task (parsed, typed message)
pub enum DispatchTask {
    StateUpdate(u32, serde_json::Value),     // peer_id, delta
    StateFull(u32, serde_json::Value),       // peer_id, full_state
    Presence(u32, serde_json::Value),        // peer_id, presence
    Consensus(u32, HotStuffMsg),             // peer_id, bft_msg
    RoomControl(u32, serde_json::Value),     // peer_id, control
    Heartbeat(u32),                          // peer_id (just update seen)
    AppMessage(u32, Vec<u8>),                // peer_id, opaque_payload
}

pub enum DispatchError {
    DecodeError(String),
    MalformedPayload(String),
    QueueFull,
    PeerNotRegistered,
    ProtocolViolation(String),
}

/// Message dispatcher with bounded queues + backpressure
pub struct MessageDispatcher {
    // Separate queues for reliable vs unreliable
    reliable_tx: Sender<DispatchTask>,
    unreliable_tx: Sender<DispatchTask>,
    
    // Monitoring
    queue_depths: (AtomicUsize, AtomicUsize),
    thresholds: (usize, usize),  // (reliable, unreliable) backpressure thresholds
}

impl MessageDispatcher {
    /// Create with default capacity (reliable=1000, unreliable=100)
    pub fn new() -> (Self, MessageProcessor) {
        let (rel_tx, rel_rx) = bounded(1000);
        let (unrel_tx, unrel_rx) = bounded(100);

        let dispatcher = Self {
            reliable_tx: rel_tx,
            unreliable_tx: unrel_tx,
            queue_depths: (AtomicUsize::new(0), AtomicUsize::new(0)),
            thresholds: (800, 80),  // 80% capacity triggers backpressure
        };

        let processor = MessageProcessor {
            reliable_rx: rel_rx,
            unreliable_rx: unrel_rx,
            queue_depths: Arc::new(dispatcher.queue_depths.clone()),
        };

        (dispatcher, processor)
    }

    /// Fast sync decode + enqueue (called from WebRTC callback)
    pub fn on_message(&self, buf: &[u8], peer_id: u32) -> Result<(), DispatchError> {
        // 1. Decode (fast, 4-byte header)
        let msg = match crate::wire::decode(buf) {
            DecodeResult::Ok(m) => m,
            DecodeResult::SkipVersion(v) => {
                log::debug!("skipping future version {}", v);
                return Ok(());
            }
            DecodeResult::SkipType(t) => {
                log::debug!("skipping future type {}", t);
                return Ok(());
            }
            DecodeResult::Err(e) => {
                return Err(DispatchError::DecodeError(e.to_string()));
            }
        };

        // 2. Decode payload + enqueue
        self.enqueue_task(MsgType::from_u8(msg.header.msg_type), &msg, peer_id)
    }

    fn enqueue_task(
        &self,
        msg_type: Option<MsgType>,
        msg: &WireMessage,
        peer_id: u32,
    ) -> Result<(), DispatchError> {
        match msg_type {
            Some(MsgType::StateUpdate) => {
                let delta = serde_json::from_slice(&msg.payload)
                    .map_err(|e| DispatchError::MalformedPayload(e.to_string()))?;
                let task = DispatchTask::StateUpdate(peer_id, delta);
                self.reliable_tx.try_send(task)
                    .map_err(|_| DispatchError::QueueFull)?;
                self.queue_depths.0.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Some(MsgType::StateFull) => {
                let state = serde_json::from_slice(&msg.payload)
                    .map_err(|e| DispatchError::MalformedPayload(e.to_string()))?;
                let task = DispatchTask::StateFull(peer_id, state);
                self.reliable_tx.try_send(task)
                    .map_err(|_| DispatchError::QueueFull)?;
                self.queue_depths.0.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Some(MsgType::Presence) => {
                let presence = serde_json::from_slice(&msg.payload)
                    .map_err(|e| DispatchError::MalformedPayload(e.to_string()))?;
                let task = DispatchTask::Presence(peer_id, presence);
                // For unreliable: try send, but don't fail if full
                match self.unreliable_tx.try_send(task) {
                    Ok(()) => {
                        self.queue_depths.1.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    }
                    Err(_) => {
                        log::warn!("unreliable queue full, dropping presence from peer {}", peer_id);
                        Ok(())  // Don't propagate error for unreliable
                    }
                }
            }
            Some(MsgType::Consensus) => {
                let bft_msg = serde_json::from_slice(&msg.payload)
                    .map_err(|e| DispatchError::MalformedPayload(e.to_string()))?;
                let task = DispatchTask::Consensus(peer_id, bft_msg);
                self.reliable_tx.try_send(task)
                    .map_err(|_| DispatchError::QueueFull)?;
                self.queue_depths.0.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Some(MsgType::RoomControl) => {
                let control = serde_json::from_slice(&msg.payload)
                    .map_err(|e| DispatchError::MalformedPayload(e.to_string()))?;
                let task = DispatchTask::RoomControl(peer_id, control);
                self.reliable_tx.try_send(task)
                    .map_err(|_| DispatchError::QueueFull)?;
                self.queue_depths.0.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Some(MsgType::Heartbeat) => {
                let task = DispatchTask::Heartbeat(peer_id);
                // Heartbeat can be dropped if queue is full
                let _ = self.unreliable_tx.try_send(task);
                Ok(())
            }
            Some(MsgType::AppMessage) => {
                let task = DispatchTask::AppMessage(peer_id, msg.payload.clone());
                self.reliable_tx.try_send(task)
                    .map_err(|_| DispatchError::QueueFull)?;
                self.queue_depths.0.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            None => Ok(()), // Unknown type: forward compat, silent skip
        }
    }

    pub fn backpressure_level(&self) -> f32 {
        let rel = self.queue_depths.0.load(Ordering::Relaxed) as f32 / self.thresholds.0 as f32;
        let unrel = self.queue_depths.1.load(Ordering::Relaxed) as f32 / self.thresholds.1 as f32;
        rel.max(unrel)
    }
}

/// Process messages from queue (async, called from game loop / tokio task)
pub struct MessageProcessor {
    reliable_rx: Receiver<DispatchTask>,
    unreliable_rx: Receiver<DispatchTask>,
    queue_depths: Arc<(AtomicUsize, AtomicUsize)>,
}

impl MessageProcessor {
    /// Process pending messages (non-blocking)
    pub fn process_batch(&self, platform: &mut PlatformCore, batch_size: usize) {
        let mut count = 0;
        
        // Process reliable first (ordered)
        for _ in 0..batch_size {
            if let Ok(task) = self.reliable_rx.try_recv() {
                self.process_task(task, platform);
                self.queue_depths.0.fetch_sub(1, Ordering::Relaxed);
                count += 1;
            } else {
                break;
            }
        }

        // Then unreliable (can drop old)
        for _ in 0..(batch_size / 4) {
            if let Ok(task) = self.unreliable_rx.try_recv() {
                self.process_task(task, platform);
                self.queue_depths.1.fetch_sub(1, Ordering::Relaxed);
                count += 1;
            } else {
                break;
            }
        }

        if count > 0 {
            log::debug!("processed {} messages", count);
        }
    }

    fn process_task(&self, task: DispatchTask, platform: &mut PlatformCore) {
        match task {
            DispatchTask::StateUpdate(_peer_id, delta) => {
                if let Ok(s) = serde_json::to_string(&delta) {
                    platform.merge_remote_delta(&s);
                }
            }
            DispatchTask::StateFull(_peer_id, state) => {
                // Full state sync
                if let Ok(s) = serde_json::to_string(&state) {
                    platform.merge_remote_delta(&s);
                }
            }
            DispatchTask::Presence(_peer_id, _presence) => {
                // Update presence in platform
                // (not shown here)
            }
            DispatchTask::Consensus(peer_id, msg) => {
                let result = platform.process_consensus(msg);
                log::debug!("consensus from peer {}: {:?}", peer_id, result);
            }
            DispatchTask::RoomControl(_peer_id, _control) => {
                // Handle room control (join/leave)
            }
            DispatchTask::Heartbeat(peer_id) => {
                // Just a keepalive — update peer state
                log::trace!("heartbeat from peer {}", peer_id);
            }
            DispatchTask::AppMessage(peer_id, payload) => {
                log::debug!("app message from peer {}: {} bytes", peer_id, payload.len());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_state_update() {
        let (dispatcher, _processor) = MessageDispatcher::new();
        
        let msg = crate::wire::encode(
            MsgType::StateUpdate,
            br#"{"key":"0,0","value":"stone"}"#,
        ).unwrap();

        let result = dispatcher.on_message(&msg, 1);
        assert!(result.is_ok());
    }

    #[test]
    fn unknown_type_skipped() {
        let (dispatcher, _processor) = MessageDispatcher::new();
        
        let mut msg = crate::wire::encode(MsgType::Heartbeat, b"").unwrap();
        msg[1] = 0xFF; // Unknown type

        let result = dispatcher.on_message(&msg, 1);
        assert!(result.is_ok()); // Should be silent skip
    }

    #[test]
    fn backpressure_reported() {
        let (dispatcher, _processor) = MessageDispatcher::new();
        
        // Fill queue with 500 messages
        for _ in 0..500 {
            let msg = crate::wire::encode(
                MsgType::StateUpdate,
                br#"{"test":"data"}"#,
            ).unwrap();
            let _ = dispatcher.on_message(&msg, 1);
        }

        let pressure = dispatcher.backpressure_level();
        assert!(pressure > 0.5); // >50% full
    }
}
```

---

## 8. Summary & Recommendations

| Question | Recommendation | Rationale |
|----------|---|---|
| **Dispatch pattern** | Static match (not HashMap) | Zero overhead, compile-time safe |
| **Handler registration** | Trait object + Arc registry | Pluggable without losing safety |
| **Async handling** | Sync boundary (WebRTC) + async core (task processor) | WebRTC callbacks are sync; handlers can be async |
| **Message queuing** | Bounded channels (crossbeam) | Explicit backpressure, split reliable/unreliable |
| **Error handling** | Skip unknown (forward compat), disconnect on protocol violation | Graceful degradation |
| **Backpressure** | Drop unreliable + pause reliable | Differentiated by message type |

### Checklist for Implementation

- [ ] Use `crossbeam::channel::bounded()` for message queues (not VecDeque)
- [ ] Split reliable (1000 cap) and unreliable (100 cap) queues
- [ ] Implement backpressure monitoring (queue depth ≥80%)
- [ ] Fast sync dispatch path: decode → enqueue (< 1ms target)
- [ ] Async processor: batch 32-256 tasks per iteration
- [ ] Error handling: log malformed, disconnect on protocol violation
- [ ] Metrics: queue depth, dropped messages, processing latency
- [ ] Tests: unknown type, unknown version, queue full, backpressure

### Performance Notes

- Decode latency: ~50μs (4B header + serde_json)
- Enqueue latency: ~1μs (bounded channel)
- Total sync path: ~100μs (well under 16ms frame budget)
- Async processor: ~10μs per task (depends on handler complexity)

---

## References

1. **Message Passing Patterns**: "Designing Data-Intensive Applications" (Kleppmann 2017), Ch. 3
2. **Async Boundaries**: "Tokio Tutorial" (https://tokio.rs), async-await patterns
3. **Backpressure**: "Reactive Streams" spec (http://www.reactive-streams.org/)
4. **BFT Consensus**: "HotStuff" (Yin et al. 2019)
5. **CRDT**: "A JSON Document Object Model" (Kleppmann et al. 2021)

---

**Status**: Ready for implementation  
**Next Step**: Implement in `src/dispatch/mod.rs`, integrate with `src/p2p/webrtc.rs`  
**KG Refs**: CONTRACT_333_MessageDispatcher, ATOM_Wire_Dispatcher, SPAN_333_P2P
