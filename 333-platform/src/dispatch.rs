// KG: CONTRACT_333_MessageDispatcher, ATOM_Wire_Dispatcher
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-dispatch-DispatchTask/DispatchError/MessageDispatcher/MessageProcessor
// Fast sync dispatch → bounded queue → async processing
// Sync boundary: WebRTC callback → decode + enqueue
// Async core: game loop / task processor

use crate::wire::{WireMessage, MsgType, DecodeResult};
use crate::bft::types::HotStuffMsg;
use crate::platform::PlatformCore;
use std::sync::Arc;
use std::sync::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Dispatch task — parsed, typed message ready for processing
///
/// # OCP 설계 결정 (SOLID-OCP-decision-2026-04-16)
///
/// Taliban --lens solid 검증에서 `enqueue_task()`의 `MsgType` enum-match가 OCP ✗로 판정됨.
/// 그러나 Rust enum exhaustive match는 Java/C# OCP와 다른 맥락:
/// 1. **컴파일러 강제**: 새 `MsgType` 추가 → 모든 match 미처리 arm을 컴파일 오류로 차단.
///    이는 버그를 런타임이 아닌 컴파일 타임에 잡는 Rust 고유의 안전 장치.
/// 2. **실제 OCP 충족**: 새 메시지 타입은 `MsgType` + `DispatchTask` variant 추가로만 확장.
///    기존 arm들은 수정 불필요 (Closed to modification).
/// 3. **trait-object 대안의 단점**: `dyn MessageHandler` registry는 heap 할당 + vtable
///    lookup → BFT 메시지 처리 열경로에서 지연 증가.
///
/// **결론**: `dispatch.rs`의 enum-match는 Rust-idiomatic OCP (open via variants, closed via
/// exhaustive match). 수정 없이 유지. 실제 OCP fix는 `wasm_om.rs` string-match에 적용 완료.
/// # KG: SOLID-OCP-decision-2026-04-16, SOLID-OCP-fix-om-dispatch-2026-04-16
#[derive(Debug, Clone)]
pub enum DispatchTask {
    /// CRDT state delta (reliable)
    StateUpdate(u32, serde_json::Value),
    /// Full state sync (reliable)
    StateFull(u32, serde_json::Value),
    /// Presence: cursor, status, typing (unreliable)
    Presence(u32, serde_json::Value),
    /// BFT consensus message (reliable)
    Consensus(u32, HotStuffMsg),
    /// Room management: join, leave, role (reliable)
    RoomControl(u32, serde_json::Value),
    /// Keepalive (unreliable)
    Heartbeat(u32),
    /// Application-defined message (reliable)
    AppMessage(u32, Vec<u8>),
}

/// Dispatch errors
#[derive(Debug, Clone, PartialEq)]
pub enum DispatchError {
    /// Wire decode failed
    DecodeError(String),
    /// Payload serde_json failed
    MalformedPayload(String),
    /// Queue at capacity
    QueueFull,
    /// Peer not registered
    PeerNotRegistered,
    /// Protocol violation (disconnect peer)
    ProtocolViolation(String),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::DecodeError(e) => write!(f, "decode: {}", e),
            Self::MalformedPayload(e) => write!(f, "payload: {}", e),
            Self::QueueFull => write!(f, "queue full"),
            Self::PeerNotRegistered => write!(f, "peer not registered"),
            Self::ProtocolViolation(e) => write!(f, "protocol violation: {}", e),
        }
    }
}

/// Shared bounded queue (Arc<Mutex<VecDeque>> with capacity check)
type BoundedQueue = Arc<Mutex<VecDeque<DispatchTask>>>;

/// Message dispatcher with bounded queues (reliable + unreliable)
/// Fast sync path: decode + enqueue
pub struct MessageDispatcher {
    // Separate queues for reliable vs unreliable
    // Reliable: StateUpdate, StateFull, Consensus, RoomControl, AppMessage (1000 cap)
    reliable_queue: BoundedQueue,
    // Unreliable: Presence, Heartbeat (100 cap)
    unreliable_queue: BoundedQueue,

    // Queue capacity limits
    reliable_cap: usize,
    unreliable_cap: usize,

    // Monitor queue depth for backpressure
    queue_depths: Arc<(AtomicUsize, AtomicUsize)>,
    // Backpressure thresholds (reliable, unreliable)
    thresholds: (usize, usize),
}

impl MessageDispatcher {
    /// Create new dispatcher with default capacities
    /// Returns (dispatcher, processor) pair
    pub fn new() -> (Self, MessageProcessor) {
        Self::with_capacities(1000, 100)
    }

    /// Create with custom capacities
    pub fn with_capacities(reliable_cap: usize, unreliable_cap: usize) -> (Self, MessageProcessor) {
        let reliable_queue = Arc::new(Mutex::new(VecDeque::new()));
        let unreliable_queue = Arc::new(Mutex::new(VecDeque::new()));
        let queue_depths = Arc::new((AtomicUsize::new(0), AtomicUsize::new(0)));

        let dispatcher = Self {
            reliable_queue: Arc::clone(&reliable_queue),
            unreliable_queue: Arc::clone(&unreliable_queue),
            reliable_cap,
            unreliable_cap,
            queue_depths: Arc::clone(&queue_depths),
            thresholds: (reliable_cap * 80 / 100, unreliable_cap * 80 / 100),
        };

        let processor = MessageProcessor {
            reliable_queue,
            unreliable_queue,
            queue_depths,
        };

        (dispatcher, processor)
    }

    /// Fast sync dispatch (called from WebRTC DataChannel.onmessage)
    /// Must return quickly (< 1ms target)
    ///
    /// 1. Decode wire format (4-byte header)
    /// 2. Parse payload (JSON)
    /// 3. Enqueue task
    pub fn on_message(&self, buf: &[u8], peer_id: u32) -> Result<(), DispatchError> {
        // 1. Decode wire frame
        let msg = match crate::wire::decode(buf) {
            DecodeResult::Ok(m) => m,
            // Forward compat: unknown version → skip silently
            DecodeResult::SkipVersion(_v) => {
                return Ok(());
            }
            // Forward compat: unknown message type → skip silently
            DecodeResult::SkipType(_t) => {
                return Ok(());
            }
            // Protocol error: decode failed
            DecodeResult::Err(e) => {
                return Err(DispatchError::DecodeError(e.to_string()));
            }
        };

        // 2. Route to appropriate queue based on message type
        self.enqueue_task(MsgType::from_u8(msg.header.msg_type), &msg, peer_id)
    }

    /// Internal: route and enqueue task
    fn enqueue_task(
        &self,
        msg_type: Option<MsgType>,
        msg: &WireMessage,
        peer_id: u32,
    ) -> Result<(), DispatchError> {
        match msg_type {
            Some(MsgType::StateUpdate) => {
                let delta = serde_json::from_slice(&msg.payload)
                    .map_err(|e| DispatchError::MalformedPayload(format!("StateUpdate: {}", e)))?;
                let task = DispatchTask::StateUpdate(peer_id, delta);
                self.enqueue_reliable(task)?;
                Ok(())
            }

            Some(MsgType::StateFull) => {
                let state = serde_json::from_slice(&msg.payload)
                    .map_err(|e| DispatchError::MalformedPayload(format!("StateFull: {}", e)))?;
                let task = DispatchTask::StateFull(peer_id, state);
                self.enqueue_reliable(task)?;
                Ok(())
            }

            Some(MsgType::Presence) => {
                let presence = serde_json::from_slice(&msg.payload)
                    .map_err(|e| DispatchError::MalformedPayload(format!("Presence: {}", e)))?;
                let task = DispatchTask::Presence(peer_id, presence);

                // Unreliable: silently drop if queue full (don't fail)
                let _ = self.enqueue_unreliable(task);
                Ok(())
            }

            Some(MsgType::Consensus) => {
                let bft_msg = serde_json::from_slice(&msg.payload)
                    .map_err(|e| DispatchError::MalformedPayload(format!("Consensus: {}", e)))?;
                let task = DispatchTask::Consensus(peer_id, bft_msg);
                self.enqueue_reliable(task)?;
                Ok(())
            }

            Some(MsgType::RoomControl) => {
                let control = serde_json::from_slice(&msg.payload)
                    .map_err(|e| DispatchError::MalformedPayload(format!("RoomControl: {}", e)))?;
                let task = DispatchTask::RoomControl(peer_id, control);
                self.enqueue_reliable(task)?;
                Ok(())
            }

            Some(MsgType::Heartbeat) => {
                let task = DispatchTask::Heartbeat(peer_id);
                // Heartbeat is optional: drop if queue full
                let _ = self.enqueue_unreliable(task);
                Ok(())
            }

            Some(MsgType::AppMessage) => {
                let task = DispatchTask::AppMessage(peer_id, msg.payload.clone());
                self.enqueue_reliable(task)?;
                Ok(())
            }

            None => {
                // Unknown type (future version): silent skip (forward compat)
                Ok(())
            }
        }
    }

    /// Enqueue to reliable queue (fail if full)
    fn enqueue_reliable(&self, task: DispatchTask) -> Result<(), DispatchError> {
        let mut queue = self.reliable_queue.lock().unwrap();
        if queue.len() >= self.reliable_cap {
            return Err(DispatchError::QueueFull);
        }
        queue.push_back(task);
        self.queue_depths.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Enqueue to unreliable queue (silently drop if full)
    fn enqueue_unreliable(&self, task: DispatchTask) -> Result<(), ()> {
        let mut queue = self.unreliable_queue.lock().unwrap();
        if queue.len() >= self.unreliable_cap {
            return Err(());  // Silent drop
        }
        queue.push_back(task);
        self.queue_depths.1.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Check backpressure level scaled ×1000 (0 = empty, 1000 = full). # KG: sprint6B-wasm-size-opt-2026-04-15
    pub fn backpressure_level(&self) -> u32 {
        let rel_num = self.queue_depths.0.load(Ordering::Relaxed);
        let unrel_num = self.queue_depths.1.load(Ordering::Relaxed);
        // Scale ×1000 to avoid f32
        let rel = if self.thresholds.0 == 0 { 0 } else { rel_num * 1000 / self.thresholds.0 };
        let unrel = if self.thresholds.1 == 0 { 0 } else { unrel_num * 1000 / self.thresholds.1 };
        rel.max(unrel) as u32
    }

    /// Get queue depths (reliable, unreliable)
    pub fn queue_depths(&self) -> (usize, usize) {
        (
            self.queue_depths.0.load(Ordering::Relaxed),
            self.queue_depths.1.load(Ordering::Relaxed),
        )
    }
}

impl Default for MessageDispatcher {
    fn default() -> Self {
        let (dispatcher, _) = Self::new();
        dispatcher
    }
}

/// Message processor — async/deferred execution
/// Called from game loop or tokio task
pub struct MessageProcessor {
    reliable_queue: BoundedQueue,
    unreliable_queue: BoundedQueue,
    queue_depths: Arc<(AtomicUsize, AtomicUsize)>,
}

impl MessageProcessor {
    /// Process pending tasks (non-blocking, batch style)
    ///
    /// # Arguments
    /// * `platform` - mutable reference to PlatformCore
    /// * `batch_size` - max tasks to process per call (default 32-256)
    ///
    /// Processes reliable (ordered) first, then unreliable.
    /// For unreliable, only process batch_size/4 to prioritize reliable.
    pub fn process_batch(&self, platform: &mut PlatformCore, batch_size: usize) {
        // Process reliable (ordered)
        {
            let mut queue = self.reliable_queue.lock().unwrap();
            for _ in 0..batch_size {
                match queue.pop_front() {
                    Some(task) => {
                        drop(queue);  // Release lock before processing
                        self.process_task(task, platform);
                        self.queue_depths.0.fetch_sub(1, Ordering::Relaxed);
                        queue = self.reliable_queue.lock().unwrap();
                    }
                    None => break,
                }
            }
        }

        // Process unreliable (don't let them starve reliable)
        let unrel_batch = batch_size / 4;
        {
            let mut queue = self.unreliable_queue.lock().unwrap();
            for _ in 0..unrel_batch {
                match queue.pop_front() {
                    Some(task) => {
                        drop(queue);
                        self.process_task(task, platform);
                        self.queue_depths.1.fetch_sub(1, Ordering::Relaxed);
                        queue = self.unreliable_queue.lock().unwrap();
                    }
                    None => break,
                }
            }
        }
    }

    /// Process all pending tasks (blocking until queues empty)
    /// Warning: may block for a long time if queue is large
    pub fn drain_all(&self, platform: &mut PlatformCore) {
        loop {
            let mut drained = 0;

            {
                let mut queue = self.reliable_queue.lock().unwrap();
                while let Some(task) = queue.pop_front() {
                    drop(queue);
                    self.process_task(task, platform);
                    self.queue_depths.0.fetch_sub(1, Ordering::Relaxed);
                    queue = self.reliable_queue.lock().unwrap();
                    drained += 1;
                }
            }

            {
                let mut queue = self.unreliable_queue.lock().unwrap();
                while let Some(task) = queue.pop_front() {
                    drop(queue);
                    self.process_task(task, platform);
                    self.queue_depths.1.fetch_sub(1, Ordering::Relaxed);
                    queue = self.unreliable_queue.lock().unwrap();
                    drained += 1;
                }
            }

            if drained == 0 {
                break;
            }
        }
    }

    /// Process single task (internal)
    fn process_task(&self, task: DispatchTask, platform: &mut PlatformCore) {
        match task {
            DispatchTask::StateUpdate(_peer_id, delta) => {
                // CRDT delta from remote peer
                if let Ok(delta_str) = serde_json::to_string(&delta) {
                    platform.merge_remote_delta(&delta_str);
                }
            }

            DispatchTask::StateFull(_peer_id, state) => {
                // Full state sync (e.g., new peer joining)
                if let Ok(state_str) = serde_json::to_string(&state) {
                    platform.merge_remote_delta(&state_str);
                }
            }

            DispatchTask::Presence(_peer_id, _presence) => {
                // Cursor, status, typing indicator
                // TODO: update presence in platform
            }

            DispatchTask::Consensus(_peer_id, msg) => {
                // BFT consensus message
                let _result = platform.process_consensus(msg);
            }

            DispatchTask::RoomControl(_peer_id, _control) => {
                // Room management: join, leave, role change
                // TODO: update room state
            }

            DispatchTask::Heartbeat(_peer_id) => {
                // Just a keepalive — update peer last_seen time
            }

            DispatchTask::AppMessage(_peer_id, _payload) => {
                // Application-defined payload
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{encode, MsgType};

    #[test]
    fn new_dispatcher() {
        let (dispatcher, _processor) = MessageDispatcher::new();
        assert_eq!(dispatcher.backpressure_level(), 0);
    }

    #[test]
    fn dispatch_state_update() {
        let (dispatcher, _processor) = MessageDispatcher::new();

        let payload = br#"{"key":"0,0","value":"stone"}"#;
        let buf = encode(MsgType::StateUpdate, payload).unwrap();

        let result = dispatcher.on_message(&buf, 1);
        assert!(result.is_ok());

        let (rel, unrel) = dispatcher.queue_depths();
        assert_eq!(rel, 1);
        assert_eq!(unrel, 0);
    }

    #[test]
    fn unknown_type_skipped() {
        let (dispatcher, _processor) = MessageDispatcher::new();

        let mut buf = encode(MsgType::Heartbeat, b"").unwrap();
        buf[1] = 0xFF; // Unknown type

        let result = dispatcher.on_message(&buf, 1);
        assert!(result.is_ok()); // Should be silent skip

        let (rel, unrel) = dispatcher.queue_depths();
        assert_eq!(rel, 0);
        assert_eq!(unrel, 0);
    }

    #[test]
    fn unknown_version_skipped() {
        let (dispatcher, _processor) = MessageDispatcher::new();

        let mut buf = encode(MsgType::Heartbeat, b"").unwrap();
        buf[0] = 99; // Unknown version

        let result = dispatcher.on_message(&buf, 1);
        assert!(result.is_ok());

        let (rel, unrel) = dispatcher.queue_depths();
        assert_eq!(rel, 0);
        assert_eq!(unrel, 0);
    }

    #[test]
    fn malformed_payload() {
        let (dispatcher, _processor) = MessageDispatcher::new();

        let buf = encode(MsgType::StateUpdate, b"not json").unwrap();
        let result = dispatcher.on_message(&buf, 1);

        assert!(matches!(result, Err(DispatchError::MalformedPayload(_))));
    }

    #[test]
    fn presence_unreliable_queue() {
        let (dispatcher, _processor) = MessageDispatcher::new();

        let payload = br#"{"x":10.5,"y":20.3}"#;
        let buf = encode(MsgType::Presence, payload).unwrap();

        dispatcher.on_message(&buf, 1).unwrap();

        let (rel, unrel) = dispatcher.queue_depths();
        assert_eq!(rel, 0);
        assert_eq!(unrel, 1);
    }

    #[test]
    fn heartbeat_unreliable() {
        let (dispatcher, _processor) = MessageDispatcher::new();

        let buf = encode(MsgType::Heartbeat, b"").unwrap();
        dispatcher.on_message(&buf, 1).unwrap();

        // Heartbeat should go to unreliable queue
        let (_rel, unrel) = dispatcher.queue_depths();
        assert_eq!(unrel, 1);
    }

    #[test]
    fn backpressure_monitoring() {
        let (dispatcher, _processor) = MessageDispatcher::with_capacities(10, 10);

        // Fill reliable queue
        for i in 0..8 {
            let payload = format!(r#"{{"id":{}}}"#, i);
            let buf = encode(MsgType::StateUpdate, payload.as_bytes()).unwrap();
            dispatcher.on_message(&buf, 1).unwrap();
        }

        let pressure = dispatcher.backpressure_level();
        // backpressure_level is ×1000: 0.7 → 700
        assert!(pressure > 700, "expected pressure > 700‰, got {}", pressure);
    }

    #[test]
    fn queue_full_error() {
        let (dispatcher, _processor) = MessageDispatcher::with_capacities(2, 2);

        // Fill reliable queue (capacity 2)
        for i in 0..2 {
            let payload = format!(r#"{{"id":{}}}"#, i);
            let buf = encode(MsgType::StateUpdate, payload.as_bytes()).unwrap();
            dispatcher.on_message(&buf, 1).unwrap();
        }

        // Third should fail
        let payload = br#"{"id":3}"#;
        let buf = encode(MsgType::StateUpdate, payload).unwrap();
        let result = dispatcher.on_message(&buf, 1);

        assert!(matches!(result, Err(DispatchError::QueueFull)));
    }

    #[test]
    fn process_batch() {
        let (dispatcher, processor) = MessageDispatcher::new();

        // Enqueue 5 messages
        for i in 0..5 {
            let payload = format!(r#"{{"seq":{}}}"#, i);
            let buf = encode(MsgType::StateUpdate, payload.as_bytes()).unwrap();
            dispatcher.on_message(&buf, 1).unwrap();
        }

        let mut platform = PlatformCore::new(1, &[1, 2, 3]);

        // Process batch of 3
        processor.process_batch(&mut platform, 3);

        let (rel, _) = dispatcher.queue_depths();
        assert_eq!(rel, 2, "should have 2 tasks left");
    }
}
