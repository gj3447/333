// KG: CONTRACT_333_Events, ATOM_Web_Events
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-sdk-events-EventType/Event/CallbackId/EventBus
// Event system — PubSub + Presence
// Contract: on→callback registered. emit→all listeners called. off→removed.

use std::collections::HashMap;

/// Event types emitted by the platform
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EventType {
    PeerJoined,
    PeerLeft,
    PeerTimedOut,
    StateChanged,    // CRDT state updated
    MessageReceived, // PubSub message
    RoomCreated,
    RoomDestroyed,
    Custom(String),  // App-specific events
}

/// Event payload
#[derive(Debug, Clone)]
pub struct Event {
    pub event_type: EventType,
    pub data: Vec<u8>,
    pub source: Option<u32>, // peer ID
    pub timestamp_ms: u64,
}

/// Callback ID for unsubscription
pub type CallbackId = u64;

/// Event callback (boxed closure)
type Callback = Box<dyn Fn(&Event) + Send>;

/// Event Bus — subscribe/emit/unsubscribe
pub struct EventBus {
    listeners: HashMap<EventType, Vec<(CallbackId, Callback)>>,
    next_id: CallbackId,
    event_log: Vec<Event>, // recent events for late joiners
    max_log: usize,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            listeners: HashMap::new(),
            next_id: 1,
            event_log: Vec::new(),
            max_log: 100,
        }
    }

    /// Subscribe to an event type → returns callback ID for unsubscription
    pub fn on<F: Fn(&Event) + Send + 'static>(&mut self, event_type: EventType, callback: F) -> CallbackId {
        let id = self.next_id;
        self.next_id += 1;
        self.listeners
            .entry(event_type)
            .or_default()
            .push((id, Box::new(callback)));
        id
    }

    /// Emit an event → calls all registered listeners (panic-safe)
    pub fn emit(&mut self, event: Event) -> usize {
        let mut called = 0;
        if let Some(listeners) = self.listeners.get(&event.event_type) {
            for (_, cb) in listeners {
                // FIX HIGH #11: catch_unwind prevents one bad callback from killing all listeners
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(&event)));
                called += 1;
            }
        }
        // Also check Custom wildcard listeners
        if let Some(listeners) = self.listeners.get(&EventType::Custom("*".into())) {
            for (_, cb) in listeners {
                cb(&event);
                called += 1;
            }
        }
        // Log
        self.event_log.push(event);
        if self.event_log.len() > self.max_log {
            self.event_log.remove(0);
        }
        called
    }

    /// Unsubscribe by callback ID
    pub fn off(&mut self, callback_id: CallbackId) -> bool {
        for listeners in self.listeners.values_mut() {
            if let Some(pos) = listeners.iter().position(|(id, _)| *id == callback_id) {
                let _ = listeners.remove(pos);
                return true;
            }
        }
        false
    }

    /// Count listeners for an event type
    pub fn listener_count(&self, event_type: &EventType) -> usize {
        self.listeners.get(event_type).map_or(0, |v| v.len())
    }

    /// Recent event log (for late joiners)
    pub fn recent_events(&self, count: usize) -> &[Event] {
        let start = self.event_log.len().saturating_sub(count);
        &self.event_log[start..]
    }

    /// Total emitted events
    pub fn total_emitted(&self) -> usize {
        self.event_log.len()
    }
}

impl Default for EventBus {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicU32, Ordering}};

    #[test]
    fn subscribe_and_emit() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&counter);

        let mut bus = EventBus::new();
        bus.on(EventType::PeerJoined, move |_e| {
            c.fetch_add(1, Ordering::SeqCst);
        });

        bus.emit(Event {
            event_type: EventType::PeerJoined,
            data: vec![],
            source: Some(42),
            timestamp_ms: 1000,
        });

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn multiple_listeners() {
        let c1 = Arc::new(AtomicU32::new(0));
        let c2 = Arc::new(AtomicU32::new(0));
        let cc1 = Arc::clone(&c1);
        let cc2 = Arc::clone(&c2);

        let mut bus = EventBus::new();
        bus.on(EventType::StateChanged, move |_| { cc1.fetch_add(1, Ordering::SeqCst); });
        bus.on(EventType::StateChanged, move |_| { cc2.fetch_add(1, Ordering::SeqCst); });

        let called = bus.emit(Event {
            event_type: EventType::StateChanged,
            data: b"delta".to_vec(),
            source: None,
            timestamp_ms: 0,
        });
        assert_eq!(called, 2);
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unsubscribe() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&counter);

        let mut bus = EventBus::new();
        let id = bus.on(EventType::PeerLeft, move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        });

        assert_eq!(bus.listener_count(&EventType::PeerLeft), 1);
        bus.off(id);
        assert_eq!(bus.listener_count(&EventType::PeerLeft), 0);

        bus.emit(Event {
            event_type: EventType::PeerLeft,
            data: vec![], source: None, timestamp_ms: 0,
        });
        assert_eq!(counter.load(Ordering::SeqCst), 0); // not called
    }

    #[test]
    fn event_log() {
        let mut bus = EventBus::new();
        for i in 0..5 {
            bus.emit(Event {
                event_type: EventType::Custom(format!("evt_{}", i)),
                data: vec![i as u8], source: None, timestamp_ms: i as u64 * 100,
            });
        }
        assert_eq!(bus.total_emitted(), 5);
        assert_eq!(bus.recent_events(3).len(), 3);
    }

    #[test]
    fn no_listeners_emits_zero() {
        let mut bus = EventBus::new();
        let called = bus.emit(Event {
            event_type: EventType::RoomCreated,
            data: vec![], source: None, timestamp_ms: 0,
        });
        assert_eq!(called, 0);
    }

    #[test]
    fn wrong_event_type_not_called() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&counter);

        let mut bus = EventBus::new();
        bus.on(EventType::PeerJoined, move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        });

        bus.emit(Event {
            event_type: EventType::PeerLeft, // different type
            data: vec![], source: None, timestamp_ms: 0,
        });
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
}
