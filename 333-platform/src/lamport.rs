// KG: SPAN_333_CRDT_Lamport
//! Lamport Timestamp
//!
//! 12 bytes per message (u64 counter + u32 node_id).
//! Provides total ordering for conflict resolution (Last-Writer-Wins).
//! Does NOT detect concurrency — use HLC for causal ordering.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lamport {
    pub counter: u64,
    pub node_id: u32,
}

impl Lamport {
    pub fn new(node_id: u32) -> Self {
        Self { counter: 0, node_id }
    }

    /// Increment on local event
    pub fn tick(&mut self) -> Self {
        // FIX CRITICAL #6: saturating_add prevents u64 overflow
        self.counter = self.counter.saturating_add(1);
        *self
    }

    /// Update on receiving remote timestamp, then increment
    pub fn recv(&mut self, remote: &Lamport) {
        self.counter = self.counter.max(remote.counter).saturating_add(1);
    }

    /// Serialize to 12 bytes
    pub fn to_bytes(&self) -> [u8; 12] {
        let mut buf = [0u8; 12];
        buf[0..8].copy_from_slice(&self.counter.to_be_bytes());
        buf[8..12].copy_from_slice(&self.node_id.to_be_bytes());
        buf
    }

    pub fn from_bytes(buf: &[u8; 12]) -> Self {
        Self {
            counter: u64::from_be_bytes(buf[0..8].try_into().unwrap()),
            node_id: u32::from_be_bytes(buf[8..12].try_into().unwrap()),
        }
    }
}

impl Ord for Lamport {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.counter
            .cmp(&other.counter)
            .then(self.node_id.cmp(&other.node_id))
    }
}

impl PartialOrd for Lamport {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_increments() {
        let mut l = Lamport::new(1);
        l.tick();
        assert_eq!(l.counter, 1);
        l.tick();
        assert_eq!(l.counter, 2);
    }

    #[test]
    fn recv_takes_max() {
        let mut local = Lamport { counter: 5, node_id: 1 };
        let remote = Lamport { counter: 10, node_id: 2 };
        local.recv(&remote);
        assert_eq!(local.counter, 11);
    }

    #[test]
    fn total_order() {
        let a = Lamport { counter: 5, node_id: 1 };
        let b = Lamport { counter: 5, node_id: 2 };
        assert!(a < b);
    }

    #[test]
    fn serialization() {
        let l = Lamport { counter: 999, node_id: 42 };
        let restored = Lamport::from_bytes(&l.to_bytes());
        assert_eq!(l, restored);
    }
}
