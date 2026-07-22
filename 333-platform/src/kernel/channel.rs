// SPDX-License-Identifier: AGPL-3.0-only
// 333 modification notice (2026): typed Rust event-channel adaptation of the
// Puter observer/pub-sub pattern. See ../../../THIRD_PARTY_NOTICES.md.
// KG: puter-tpa-P7-event-pubsub-2026-04-16, SPAN_333_Kernel
// P7: Typed event channels — Puter Observer(0.90) pattern ported to Rust
// Type-safe pub/sub without tokio (WASM-compatible).
// Wraps the untyped EventBus with a typed Channel<T> facade.

use std::collections::HashMap;
use std::marker::PhantomData;

/// A typed message on a channel
pub struct Message<T> {
    pub payload: T,
    pub sender: Option<u32>,
    pub seq: u64,
}

/// Type-erased callback
type AnyCallback = Box<dyn Fn(&[u8]) + Send>;

/// Subscription handle
pub type SubId = u64;

/// A typed channel — subscribers receive messages of type T.
/// Serialisation is done via serde_json internally to maintain type safety
/// across the subscription boundary without needing GATs.
pub struct Channel<T: serde::Serialize + serde::de::DeserializeOwned + 'static> {
    name: String,
    subscribers: HashMap<SubId, AnyCallback>,
    next_id: SubId,
    history: Vec<Vec<u8>>,
    max_history: usize,
    seq: u64,
    _phantom: PhantomData<T>,
}

impl<T: serde::Serialize + serde::de::DeserializeOwned + 'static> Channel<T> {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            subscribers: HashMap::new(),
            next_id: 1,
            history: Vec::new(),
            max_history: 64,
            seq: 0,
            _phantom: PhantomData,
        }
    }

    pub fn name(&self) -> &str { &self.name }

    /// Subscribe to this channel. Returns a SubId for unsubscription.
    pub fn subscribe<F>(&mut self, callback: F) -> SubId
    where
        F: Fn(T) + Send + 'static,
    {
        let id = self.next_id;
        self.next_id += 1;
        self.subscribers.insert(id, Box::new(move |bytes: &[u8]| {
            if let Ok(msg) = serde_json::from_slice::<T>(bytes) {
                callback(msg);
            }
        }));
        id
    }

    /// Unsubscribe
    pub fn unsubscribe(&mut self, id: SubId) -> bool {
        self.subscribers.remove(&id).is_some()
    }

    /// Publish a message to all subscribers
    pub fn publish(&mut self, msg: T) -> usize {
        let bytes = match serde_json::to_vec(&msg) {
            Ok(b) => b,
            Err(_) => return 0,
        };
        self.seq += 1;
        let called = self.subscribers.len();
        for cb in self.subscribers.values() {
            cb(&bytes);
        }
        // Maintain history ring
        self.history.push(bytes);
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }
        called
    }

    /// Replay recent history to a late subscriber (returns count replayed)
    pub fn replay<F>(&self, count: usize, mut callback: F) -> usize
    where
        F: FnMut(T),
    {
        let start = self.history.len().saturating_sub(count);
        let mut replayed = 0;
        for bytes in &self.history[start..] {
            if let Ok(msg) = serde_json::from_slice::<T>(bytes) {
                callback(msg);
                replayed += 1;
            }
        }
        replayed
    }

    pub fn subscriber_count(&self) -> usize { self.subscribers.len() }
    pub fn history_len(&self) -> usize { self.history.len() }
    pub fn seq(&self) -> u64 { self.seq }
}

/// Multi-channel broker — holds heterogeneous channels by name.
/// Each channel is type-erased at the broker level; callers get typed handles.
///
/// Usage pattern (Puter Observer):
///   1. Create channel: broker registers under a string name
///   2. Subscribe: consumer receives typed callback
///   3. Publish: producer dispatches typed messages
///
/// This is deliberately NOT using trait objects for channels — instead,
/// callers own Channel<T> instances and the broker is just for naming/discovery.
#[derive(Default)]
pub struct ChannelBroker {
    names: Vec<String>,
}

impl ChannelBroker {
    pub fn new() -> Self { Self::default() }

    /// Announce a channel (records its name for discovery)
    pub fn announce(&mut self, name: impl Into<String>) {
        let n = name.into();
        if !self.names.contains(&n) {
            self.names.push(n);
        }
    }

    pub fn known_channels(&self) -> &[String] {
        &self.names
    }

    pub fn len(&self) -> usize { self.names.len() }
    pub fn is_empty(&self) -> bool { self.names.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use serde::{Serialize, Deserialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct GameEvent { kind: String, data: u32 }

    #[test]
    fn subscribe_and_receive() {
        let mut ch: Channel<GameEvent> = Channel::new("game_events");
        let received = Arc::new(Mutex::new(Vec::new()));
        let r = Arc::clone(&received);

        ch.subscribe(move |e: GameEvent| {
            r.lock().unwrap().push(e);
        });

        ch.publish(GameEvent { kind: "spawn".into(), data: 42 });
        ch.publish(GameEvent { kind: "move".into(), data: 99 });

        let msgs = received.lock().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].kind, "spawn");
        assert_eq!(msgs[1].data, 99);
    }

    #[test]
    fn unsubscribe_stops_delivery() {
        let mut ch: Channel<GameEvent> = Channel::new("ch");
        let counter = Arc::new(Mutex::new(0u32));
        let c = Arc::clone(&counter);

        let id = ch.subscribe(move |_| { *c.lock().unwrap() += 1; });
        ch.publish(GameEvent { kind: "a".into(), data: 0 });
        assert_eq!(*counter.lock().unwrap(), 1);

        ch.unsubscribe(id);
        ch.publish(GameEvent { kind: "b".into(), data: 0 });
        assert_eq!(*counter.lock().unwrap(), 1); // unchanged
    }

    #[test]
    fn late_subscriber_replay() {
        let mut ch: Channel<GameEvent> = Channel::new("ch");
        for i in 0..10u32 {
            ch.publish(GameEvent { kind: "e".into(), data: i });
        }
        assert_eq!(ch.history_len(), 10);

        let mut replayed = vec![];
        ch.replay(3, |e: GameEvent| replayed.push(e.data));
        assert_eq!(replayed, vec![7, 8, 9]);
    }

    #[test]
    fn multiple_subscribers_all_receive() {
        let mut ch: Channel<u32> = Channel::new("nums");
        let a = Arc::new(Mutex::new(0u32));
        let b = Arc::new(Mutex::new(0u32));
        let (ca, cb) = (Arc::clone(&a), Arc::clone(&b));
        ch.subscribe(move |n: u32| { *ca.lock().unwrap() += n; });
        ch.subscribe(move |n: u32| { *cb.lock().unwrap() += n; });

        let called = ch.publish(5);
        assert_eq!(called, 2);
        assert_eq!(*a.lock().unwrap(), 5);
        assert_eq!(*b.lock().unwrap(), 5);
    }

    #[test]
    fn seq_increments() {
        let mut ch: Channel<u32> = Channel::new("seq_ch");
        assert_eq!(ch.seq(), 0);
        ch.publish(1);
        ch.publish(2);
        assert_eq!(ch.seq(), 2);
    }

    #[test]
    fn broker_announces_channels() {
        let mut broker = ChannelBroker::new();
        broker.announce("rts:game_events");
        broker.announce("social:posts");
        broker.announce("rts:game_events"); // duplicate ignored
        assert_eq!(broker.len(), 2);
        assert!(broker.known_channels().contains(&"rts:game_events".to_string()));
    }
}
