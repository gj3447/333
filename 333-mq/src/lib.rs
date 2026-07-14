// KG: SPAN_333_L7_MQ, plan-333-three-way-hybrid-2026-04-17
// Observer/PubSub trait + in-memory broker.
// Topic-based 1:N delivery. Subscribers each get their own queue.

#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MqError {
    #[error("unknown subscription id")]
    UnknownSub,
    #[error("backend: {0}")]
    Backend(String),
}

pub type Topic = String;
pub type SubId = u64;

pub trait PubSub: Send + Sync {
    fn publish(&self, topic: &str, payload: Vec<u8>) -> Result<usize, MqError>;
    fn subscribe(&self, topic: &str) -> Result<SubId, MqError>;
    fn unsubscribe(&self, sub: SubId) -> Result<(), MqError>;
    /// Drain buffered messages. Returns shared `Arc` slices — no per-subscriber copy.
    fn poll(&self, sub: SubId) -> Result<Vec<Arc<Vec<u8>>>, MqError>;
    fn topics(&self) -> Vec<Topic>;
}

#[derive(Default)]
struct BrokerState {
    next_sub: SubId,
    topic_of: HashMap<SubId, Topic>,
    queues: HashMap<SubId, VecDeque<Arc<Vec<u8>>>>,
    by_topic: HashMap<Topic, Vec<SubId>>,
}

#[derive(Default)]
pub struct InMemoryBroker {
    inner: Mutex<BrokerState>,
}

impl InMemoryBroker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl PubSub for InMemoryBroker {
    fn publish(&self, topic: &str, payload: Vec<u8>) -> Result<usize, MqError> {
        let mut g = self.inner.lock().map_err(|e| MqError::Backend(e.to_string()))?;
        let ids = g.by_topic.get(topic).cloned().unwrap_or_default();
        let delivered = ids.len();
        // Zero-copy fanout: wrap once, Arc-clone per subscriber (O(N) but shallow).
        let shared = Arc::new(payload);
        for sid in ids {
            if let Some(q) = g.queues.get_mut(&sid) {
                q.push_back(shared.clone());
            }
        }
        Ok(delivered)
    }

    fn subscribe(&self, topic: &str) -> Result<SubId, MqError> {
        let mut g = self.inner.lock().map_err(|e| MqError::Backend(e.to_string()))?;
        g.next_sub += 1;
        let sid = g.next_sub;
        g.topic_of.insert(sid, topic.to_string());
        g.queues.insert(sid, VecDeque::new());
        g.by_topic.entry(topic.to_string()).or_default().push(sid);
        Ok(sid)
    }

    fn unsubscribe(&self, sub: SubId) -> Result<(), MqError> {
        let mut g = self.inner.lock().map_err(|e| MqError::Backend(e.to_string()))?;
        let topic = g.topic_of.remove(&sub).ok_or(MqError::UnknownSub)?;
        g.queues.remove(&sub);
        if let Some(v) = g.by_topic.get_mut(&topic) {
            v.retain(|s| *s != sub);
        }
        Ok(())
    }

    fn poll(&self, sub: SubId) -> Result<Vec<Arc<Vec<u8>>>, MqError> {
        let mut g = self.inner.lock().map_err(|e| MqError::Backend(e.to_string()))?;
        let q = g.queues.get_mut(&sub).ok_or(MqError::UnknownSub)?;
        Ok(q.drain(..).collect())
    }

    fn topics(&self) -> Vec<Topic> {
        self.inner.lock().map(|g| g.by_topic.keys().cloned().collect()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_subscriber_receives_published() {
        let b = InMemoryBroker::new();
        let s = b.subscribe("t1").unwrap();
        let delivered = b.publish("t1", b"msg".to_vec()).unwrap();
        assert_eq!(delivered, 1);
        let got = b.poll(s).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].as_slice(), b"msg");
    }

    #[test]
    fn fanout_to_all_subscribers_of_topic() {
        let b = InMemoryBroker::new();
        let s1 = b.subscribe("x").unwrap();
        let s2 = b.subscribe("x").unwrap();
        let s3 = b.subscribe("y").unwrap();
        let delivered = b.publish("x", b"hi".to_vec()).unwrap();
        assert_eq!(delivered, 2); // only s1 and s2
        assert_eq!(b.poll(s1).unwrap().len(), 1);
        assert_eq!(b.poll(s2).unwrap().len(), 1);
        assert_eq!(b.poll(s3).unwrap().len(), 0);
    }

    #[test]
    fn poll_is_one_shot() {
        let b = InMemoryBroker::new();
        let s = b.subscribe("t").unwrap();
        b.publish("t", b"a".to_vec()).unwrap();
        assert_eq!(b.poll(s).unwrap().len(), 1);
        assert_eq!(b.poll(s).unwrap().len(), 0);
    }

    #[test]
    fn unsubscribe_stops_delivery() {
        let b = InMemoryBroker::new();
        let s = b.subscribe("t").unwrap();
        b.unsubscribe(s).unwrap();
        let delivered = b.publish("t", b"x".to_vec()).unwrap();
        assert_eq!(delivered, 0);
        assert!(matches!(b.poll(s), Err(MqError::UnknownSub)));
    }

    #[test]
    fn topics_lists_active() {
        let b = InMemoryBroker::new();
        b.subscribe("a").unwrap();
        b.subscribe("b").unwrap();
        let mut t = b.topics();
        t.sort();
        assert_eq!(t, vec!["a", "b"]);
    }
}
