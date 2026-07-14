// KG: SPAN_333_CRDT_HLC
//! Hybrid Logical Clock (HLC)
//!
//! 16 bytes per message. O(1) regardless of peer count.
//! Preserves happens-before causality: a→b ⟹ HLC(a) < HLC(b)
//!
//! Based on: Kulkarni et al. "Logical Physical Clocks" (2014)
//! Production use: MongoDB MVCC, CockroachDB

use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};

/// A Hybrid Logical Clock timestamp.
/// Total size: 16 bytes (u64 wall + u32 counter + u32 node_id)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hlc {
    /// Physical wall-clock time in milliseconds since epoch
    pub wall_ms: u64,
    /// Logical counter — disambiguates events at same wall time
    pub counter: u32,
    /// Node identifier — deterministic tiebreaker
    pub node_id: u32, // KG: ST_NodeId
}

impl Hlc {
    pub fn new(node_id: u32) -> Self {
        Self {
            wall_ms: now_ms(),
            counter: 0,
            node_id,
        }
    }

    /// Create a zero-valued HLC (for initialization)
    pub fn zero(node_id: u32) -> Self {
        Self {
            wall_ms: 0,
            counter: 0,
            node_id,
        }
    }

    /// Tick on local event — call before sending a message
    pub fn tick(&mut self) {
        let now = now_ms();
        if now > self.wall_ms {
            self.wall_ms = now;
            self.counter = 0;
        } else {
            // FIX Issue 4.1: saturating_add prevents u32 overflow
            self.counter = self.counter.saturating_add(1);
        }
    }

    /// Update on receiving a remote HLC — merges local and remote
    pub fn recv(&mut self, remote: &Hlc) {
        let now = now_ms();
        let max_wall = now.max(self.wall_ms).max(remote.wall_ms);

        if max_wall == now && max_wall > self.wall_ms && max_wall > remote.wall_ms {
            self.wall_ms = max_wall;
            self.counter = 0;
        } else if self.wall_ms == remote.wall_ms {
            self.wall_ms = max_wall;
            // FIX Issue 4.1: saturating_add
            self.counter = self.counter.max(remote.counter).saturating_add(1);
        } else if self.wall_ms > remote.wall_ms {
            self.wall_ms = max_wall;
            self.counter = self.counter.saturating_add(1);
        } else {
            self.wall_ms = max_wall;
            self.counter = remote.counter.saturating_add(1);
        }
    }

    /// Serialize to 16 bytes (network wire format)
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut buf = [0u8; 16];
        buf[0..8].copy_from_slice(&self.wall_ms.to_be_bytes());
        buf[8..12].copy_from_slice(&self.counter.to_be_bytes());
        buf[12..16].copy_from_slice(&self.node_id.to_be_bytes());
        buf
    }

    /// Deserialize from 16 bytes
    pub fn from_bytes(buf: &[u8; 16]) -> Self {
        Self {
            wall_ms: u64::from_be_bytes(buf[0..8].try_into().unwrap()),
            counter: u32::from_be_bytes(buf[8..12].try_into().unwrap()),
            node_id: u32::from_be_bytes(buf[12..16].try_into().unwrap()),
        }
    }
}

impl Ord for Hlc {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.wall_ms
            .cmp(&other.wall_ms)
            .then(self.counter.cmp(&other.counter))
            .then(self.node_id.cmp(&other.node_id))
    }
}

impl PartialOrd for Hlc {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn now_ms() -> u64 {
    // WASM: js_sys::Date가 더 안정적. Native: SystemTime.
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_advances() {
        let mut clock = Hlc::zero(1);
        clock.tick();
        assert!(clock.wall_ms > 0);
    }

    #[test]
    fn ordering_preserved() {
        let mut a = Hlc::zero(1);
        a.tick();
        let snap_a = a;

        a.tick();
        let snap_b = a;

        assert!(snap_a < snap_b, "later tick must be greater");
    }

    #[test]
    fn recv_merges_same_wall() {
        let far_future = now_ms() + 1_000_000;
        let mut local = Hlc { wall_ms: far_future, counter: 5, node_id: 1 };
        let remote = Hlc { wall_ms: far_future, counter: 10, node_id: 2 };

        local.recv(&remote);
        assert_eq!(local.counter, 11, "should be max(5,10)+1");
        assert_eq!(local.wall_ms, far_future);
    }

    #[test]
    fn remote_ahead_adopted() {
        let far_future = now_ms() + 1_000_000;
        let mut local = Hlc { wall_ms: far_future, counter: 3, node_id: 1 };
        let remote = Hlc { wall_ms: far_future + 100, counter: 7, node_id: 2 };

        local.recv(&remote);
        assert_eq!(local.wall_ms, far_future + 100);
        assert_eq!(local.counter, 8);
    }

    #[test]
    fn serialization_roundtrip() {
        let clock = Hlc {
            wall_ms: 1234567890,
            counter: 42,
            node_id: 7,
        };
        let bytes = clock.to_bytes();
        let restored = Hlc::from_bytes(&bytes);
        assert_eq!(clock, restored);
    }

    #[test]
    fn deterministic_tiebreak() {
        let a = Hlc { wall_ms: 100, counter: 5, node_id: 1 };
        let b = Hlc { wall_ms: 100, counter: 5, node_id: 2 };
        assert!(a < b, "lower node_id should be less");
    }

    #[test]
    fn size_is_16_bytes() {
        let clock = Hlc::new(1);
        assert_eq!(clock.to_bytes().len(), 16);
    }
}
