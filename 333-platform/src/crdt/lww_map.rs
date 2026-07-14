// KG: SPAN_333_CRDT_LwwMap
//! Last-Writer-Wins Map (LWW-Map)
//!
//! CRDT map where each key holds the value with the highest timestamp.
//! Zero tombstones — deletes are represented as None values with a timestamp.
//! Memory: O(unique_keys), NOT O(operations).
//!
//! For Minecraft: `LwwMap<BlockPos, BlockType>` — 1M blocks ≈ 80MB.
//! OR_Set equivalent would be 200MB+ due to tombstones.

use crate::lamport::Lamport;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::Hash;

/// A single entry in the LWW-Map
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LwwEntry<V> {
    pub value: Option<V>,
    pub timestamp: Lamport,
}

/// Last-Writer-Wins Map
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(deserialize = "K: Eq + Hash + serde::Deserialize<'de>, V: serde::Deserialize<'de>"))]
pub struct LwwMap<K, V>
where
    K: Eq + Hash,
{
    entries: HashMap<K, LwwEntry<V>>,
    clock: Lamport,
}

/// Delta — a set of changed entries to send to peers (not the full map)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LwwDelta<K, V>
where
    K: Eq + Hash,
{
    pub changes: Vec<(K, LwwEntry<V>)>,
}

impl<K, V> LwwMap<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    pub fn new(node_id: u32) -> Self {
        Self {
            entries: HashMap::new(),
            clock: Lamport::new(node_id),
        }
    }

    /// Set a key-value pair. Returns the delta for network sync.
    pub fn set(&mut self, key: K, value: V) -> LwwDelta<K, V> {
        let ts = self.clock.tick();
        let entry = LwwEntry {
            value: Some(value),
            timestamp: ts,
        };
        self.entries.insert(key.clone(), entry.clone());
        LwwDelta {
            changes: vec![(key, entry)],
        }
    }

    /// Delete a key. Stores None with current timestamp (no tombstone accumulation).
    pub fn delete(&mut self, key: K) -> LwwDelta<K, V> {
        let ts = self.clock.tick();
        let entry = LwwEntry {
            value: None,
            timestamp: ts,
        };
        self.entries.insert(key.clone(), entry.clone());
        LwwDelta {
            changes: vec![(key, entry)],
        }
    }

    /// Get a value by key. Returns None if deleted or never set.
    pub fn get(&self, key: &K) -> Option<&V> {
        self.entries
            .get(key)
            .and_then(|e| e.value.as_ref())
    }

    /// Merge a remote delta. Only applies entries with higher timestamps.
    pub fn merge_delta(&mut self, delta: &LwwDelta<K, V>) {
        for (key, remote_entry) in &delta.changes {
            self.clock.recv(&remote_entry.timestamp);
            match self.entries.get(key) {
                Some(local_entry) if local_entry.timestamp > remote_entry.timestamp => {
                    // Local strictly wins — skip
                }
                Some(local_entry) if local_entry.timestamp == remote_entry.timestamp
                    && local_entry.timestamp.node_id >= remote_entry.timestamp.node_id => {
                    // FIX HIGH #12: equal timestamp → higher node_id wins (deterministic)
                }
                _ => {
                    // Remote wins — apply
                    self.entries.insert(key.clone(), remote_entry.clone());
                }
            }
        }
    }

    /// Merge an entire remote map (for new peer sync).
    pub fn merge_full(&mut self, remote: &LwwMap<K, V>) {
        for (key, remote_entry) in &remote.entries {
            self.clock.recv(&remote_entry.timestamp);
            match self.entries.get(key) {
                Some(local_entry) if local_entry.timestamp > remote_entry.timestamp => {}
                Some(local_entry) if local_entry.timestamp == remote_entry.timestamp
                    && local_entry.timestamp.node_id >= remote_entry.timestamp.node_id => {}
                _ => {
                    self.entries.insert(key.clone(), remote_entry.clone());
                }
            }
        }
    }

    /// Number of active (non-deleted) entries
    pub fn len(&self) -> usize {
        self.entries.values().filter(|e| e.value.is_some()).count()
    }

    /// Total entries including deleted (measures actual memory usage)
    pub fn len_total(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Compact: remove entries with value=None to reclaim memory.
    /// Safe only when ALL peers have synced past these timestamps.
    pub fn compact(&mut self) {
        self.entries.retain(|_, e| e.value.is_some());
    }

    /// Snapshot the full map for persistence (IndexedDB, disk)
    pub fn snapshot(&self) -> &HashMap<K, LwwEntry<V>> {
        &self.entries
    }

    /// Iterate over active (non-deleted) entries
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries
            .iter()
            .filter_map(|(k, e)| e.value.as_ref().map(|v| (k, v)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_get() {
        let mut map: LwwMap<String, String> = LwwMap::new(1);
        map.set("hello".into(), "world".into());
        assert_eq!(map.get(&"hello".into()), Some(&"world".into()));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn delete_removes_value() {
        let mut map: LwwMap<String, i32> = LwwMap::new(1);
        map.set("x".into(), 42);
        map.delete("x".into());
        assert_eq!(map.get(&"x".into()), None);
        assert_eq!(map.len(), 0);
        // But memory still holds the entry (until compact)
        assert_eq!(map.len_total(), 1);
    }

    #[test]
    fn compact_reclaims_memory() {
        let mut map: LwwMap<String, i32> = LwwMap::new(1);
        map.set("a".into(), 1);
        map.set("b".into(), 2);
        map.delete("a".into());
        assert_eq!(map.len_total(), 2);

        map.compact();
        assert_eq!(map.len_total(), 1);
        assert_eq!(map.get(&"b".into()), Some(&2));
    }

    #[test]
    fn last_writer_wins() {
        let mut map1: LwwMap<String, i32> = LwwMap::new(1);
        let mut map2: LwwMap<String, i32> = LwwMap::new(2);

        // Node 1 writes first
        let d1 = map1.set("key".into(), 100);
        // Node 2 writes later
        let d2 = map2.set("key".into(), 200);

        // Node 1 receives node 2's delta — node 2 has higher counter
        map1.merge_delta(&d2);
        assert_eq!(map1.get(&"key".into()), Some(&200));

        // Node 2 receives node 1's delta — node 1 has lower counter, ignored
        map2.merge_delta(&d1);
        assert_eq!(map2.get(&"key".into()), Some(&200));
    }

    #[test]
    fn concurrent_writes_deterministic() {
        // Same counter, different node_id → deterministic winner
        let mut map1: LwwMap<String, i32> = LwwMap::new(1);
        let mut map2: LwwMap<String, i32> = LwwMap::new(2);

        // Force same counter
        map1.clock.counter = 10;
        map2.clock.counter = 10;

        let d1 = map1.set("key".into(), 100);
        let d2 = map2.set("key".into(), 200);

        // Both merge — node 2 wins (higher node_id)
        let mut result1 = map1.clone();
        result1.merge_delta(&d2);

        let mut result2 = map2.clone();
        result2.merge_delta(&d1);

        assert_eq!(result1.get(&"key".into()), result2.get(&"key".into()),
            "both nodes must converge to same value");
    }

    #[test]
    fn full_merge_for_new_peer() {
        let mut map1: LwwMap<String, i32> = LwwMap::new(1);
        map1.set("a".into(), 1);
        map1.set("b".into(), 2);
        map1.set("c".into(), 3);

        // New peer joins and receives full state
        let mut map3: LwwMap<String, i32> = LwwMap::new(3);
        map3.merge_full(&map1);

        assert_eq!(map3.get(&"a".into()), Some(&1));
        assert_eq!(map3.get(&"b".into()), Some(&2));
        assert_eq!(map3.get(&"c".into()), Some(&3));
        assert_eq!(map3.len(), 3);
    }

    #[test]
    fn minecraft_block_operations() {
        // Simulate Minecraft block place/break
        type BlockPos = (i32, i32, i32);
        type BlockType = u8; // 0=air, 1=stone, 2=dirt, ...

        let mut world: LwwMap<BlockPos, BlockType> = LwwMap::new(1);

        // Place 1000 blocks
        for i in 0..1000 {
            world.set((i, 64, 0), 1); // stone at y=64
        }
        assert_eq!(world.len(), 1000);

        // Break 500 blocks
        for i in 0..500 {
            world.delete((i, 64, 0));
        }
        assert_eq!(world.len(), 500);

        // Memory: 1000 entries total (500 active + 500 deleted)
        assert_eq!(world.len_total(), 1000);

        // Compact to reclaim
        world.compact();
        assert_eq!(world.len_total(), 500);
    }
}
