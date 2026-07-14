// KG: CONTRACT_333_ORSet, ATOM_Web_ORSet
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-orset-Tag/ORSetDelta/ORSet
// Observed-Remove Set CRDT
// Contract: add→present, remove(tag)→specific add removed, concurrent add+remove→add wins
// Element identity = (value, unique_tag). Tag = (PeerId_u32, seq_u64).

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// Unique tag for each add operation — (node_id, sequence)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Tag {
    pub node_id: u32,
    pub seq: u64,
}

/// Delta for network sync
#[derive(Debug, Clone)]
pub struct ORSetDelta<T: Clone + Eq + Hash> {
    pub adds: Vec<(T, Tag)>,
    pub removes: Vec<Tag>,
}

/// Observed-Remove Set CRDT
/// - add(elem) → generates unique tag, elem present
/// - remove(elem) → removes ALL observed tags for elem
/// - concurrent add+remove → add wins (new tag not in remove set)
#[derive(Debug, Clone)]
pub struct ORSet<T: Clone + Eq + Hash> {
    node_id: u32,
    seq: u64,
    entries: HashMap<T, HashSet<Tag>>,
    tombstones: HashSet<Tag>,
    /// FIX HIGH #10: track when each tombstone was created for time-based GC
    tombstone_times: HashMap<Tag, u64>, // tag → timestamp_ms
}

impl<T: Clone + Eq + Hash> ORSet<T> {
    pub fn new(node_id: u32) -> Self {
        Self {
            node_id,
            seq: 0,
            entries: HashMap::new(),
            tombstones: HashSet::new(),
            tombstone_times: HashMap::new(),
        }
    }

    /// Add element → returns delta for sync
    pub fn add(&mut self, value: T) -> ORSetDelta<T> {
        self.seq += 1;
        let tag = Tag {
            node_id: self.node_id,
            seq: self.seq,
        };
        self.entries
            .entry(value.clone())
            .or_default()
            .insert(tag);

        ORSetDelta {
            adds: vec![(value, tag)],
            removes: vec![],
        }
    }

    /// Remove element → removes ALL current tags (observed-remove)
    pub fn remove(&mut self, value: &T) -> ORSetDelta<T> {
        let removed_tags: Vec<Tag> = if let Some(tags) = self.entries.get(value) {
            tags.iter().copied().collect()
        } else {
            vec![]
        };

        // Remove from entries
        if let Some(tags) = self.entries.get_mut(value) {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            for t in &removed_tags {
                tags.remove(t);
                self.tombstones.insert(*t);
                self.tombstone_times.insert(*t, now_ms);
            }
            if tags.is_empty() {
                self.entries.remove(value);
            }
        }

        ORSetDelta {
            adds: vec![],
            removes: removed_tags,
        }
    }

    /// Check if element is present (has at least one active tag)
    pub fn contains(&self, value: &T) -> bool {
        self.entries
            .get(value)
            .is_some_and(|tags| !tags.is_empty())
    }

    /// Iterate all present elements
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.entries
            .iter()
            .filter(|(_, tags)| !tags.is_empty())
            .map(|(v, _)| v)
    }

    /// Number of present elements
    pub fn len(&self) -> usize {
        self.entries
            .values()
            .filter(|tags| !tags.is_empty())
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Merge remote delta — idempotent, commutative, associative
    pub fn merge_delta(&mut self, delta: &ORSetDelta<T>) {
        // Apply removes first (remove only known tags)
        for tag in &delta.removes {
            self.tombstones.insert(*tag);
            // Remove this tag from all entries
            for tags in self.entries.values_mut() {
                tags.remove(tag);
            }
            // Clean up empty entries
            self.entries.retain(|_, tags| !tags.is_empty());
        }

        // Apply adds (only if tag not in tombstones)
        for (value, tag) in &delta.adds {
            if !self.tombstones.contains(tag) {
                self.entries
                    .entry(value.clone())
                    .or_default()
                    .insert(*tag);
            }
        }
    }

    /// Full state merge (for new peer joining)
    pub fn merge_full(&mut self, other: &ORSet<T>) {
        // Add all entries from other that aren't tombstoned locally
        for (value, tags) in &other.entries {
            for tag in tags {
                if !self.tombstones.contains(tag) {
                    self.entries
                        .entry(value.clone())
                        .or_default()
                        .insert(*tag);
                }
            }
        }
        // Merge tombstones
        for tag in &other.tombstones {
            self.tombstones.insert(*tag);
            // Remove tombstoned tags from our entries
            for tags in self.entries.values_mut() {
                tags.remove(tag);
            }
        }
        self.entries.retain(|_, tags| !tags.is_empty());
    }

    /// Compact tombstones (call after all peers have synced)
    pub fn compact(&mut self) {
        self.tombstones.clear();
    }

    /// Epoch-based GC (Prometheus research: research-333-crdt-gc)
    /// peers_acked: set of node_ids that have acknowledged up to this epoch
    /// Only GC tombstones that ALL peers have seen
    pub fn gc_epoch(&mut self, peers_acked: &HashSet<u32>, total_peers: usize) {
        if peers_acked.len() >= total_peers {
            // All peers have acked → safe to GC all tombstones
            self.tombstones.clear();
        }
        // Otherwise: keep tombstones (not all peers synced yet)
    }

    /// FIX HIGH #10: Time-based GC — remove tombstones older than max_age_ms
    /// Call periodically (e.g. every 60s). Safe even if some peers haven't synced —
    /// worst case: a very late peer re-adds a removed element (acceptable for games).
    pub fn gc_time(&mut self, max_age_ms: u64) -> usize {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let expired: Vec<Tag> = self.tombstone_times.iter()
            .filter(|(_, &ts)| now_ms.saturating_sub(ts) > max_age_ms)
            .map(|(&tag, _)| tag)
            .collect();
        let count = expired.len();
        for tag in &expired {
            self.tombstones.remove(tag);
            self.tombstone_times.remove(tag);
        }
        count
    }

    /// Tombstone count (for monitoring GC pressure)
    pub fn tombstone_count(&self) -> usize {
        self.tombstones.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_contains() {
        let mut set = ORSet::new(1);
        assert!(!set.contains(&"alice"));
        set.add("alice");
        assert!(set.contains(&"alice"));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn remove_element() {
        let mut set = ORSet::new(1);
        set.add("alice");
        set.add("bob");
        assert_eq!(set.len(), 2);
        set.remove(&"alice");
        assert!(!set.contains(&"alice"));
        assert!(set.contains(&"bob"));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn concurrent_add_remove_add_wins() {
        // Node 1 adds "x", Node 2 removes "x" concurrently, Node 1 re-adds "x"
        let mut node1 = ORSet::new(1);
        let mut node2 = ORSet::new(2);

        // Node1 adds "x"
        let d1 = node1.add("x");
        node2.merge_delta(&d1);

        // Both see "x"
        assert!(node1.contains(&"x"));
        assert!(node2.contains(&"x"));

        // Node2 removes "x" (observes node1's tag)
        let d_remove = node2.remove(&"x");

        // Node1 concurrently adds "x" again (new tag!)
        let d_readd = node1.add("x");

        // Merge: node2 gets the re-add, node1 gets the remove
        node2.merge_delta(&d_readd);
        node1.merge_delta(&d_remove);

        // Both should have "x" (new add wins over old remove)
        assert!(node1.contains(&"x"));
        assert!(node2.contains(&"x"));
    }

    #[test]
    fn delta_sync_convergence() {
        let mut a = ORSet::new(1);
        let mut b = ORSet::new(2);

        let d1 = a.add("x");
        let d2 = b.add("y");
        let d3 = a.add("z");

        b.merge_delta(&d1);
        b.merge_delta(&d3);
        a.merge_delta(&d2);

        // Both should have {x, y, z}
        assert_eq!(a.len(), 3);
        assert_eq!(b.len(), 3);
        assert!(a.contains(&"x") && a.contains(&"y") && a.contains(&"z"));
        assert!(b.contains(&"x") && b.contains(&"y") && b.contains(&"z"));
    }

    #[test]
    fn full_merge_new_peer() {
        let mut a = ORSet::new(1);
        a.add("x");
        a.add("y");
        a.remove(&"x");

        let mut newcomer = ORSet::new(3);
        newcomer.merge_full(&a);

        assert!(!newcomer.contains(&"x"));
        assert!(newcomer.contains(&"y"));
        assert_eq!(newcomer.len(), 1);
    }

    #[test]
    fn remove_nonexistent_is_noop() {
        let mut set = ORSet::new(1);
        let delta = set.remove(&"ghost");
        assert!(delta.removes.is_empty());
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn compact_clears_tombstones() {
        let mut set = ORSet::new(1);
        set.add("x");
        set.remove(&"x");
        assert!(!set.tombstones.is_empty());
        set.compact();
        assert!(set.tombstones.is_empty());
    }

    #[test]
    fn idempotent_delta_merge() {
        let mut a = ORSet::new(1);
        let mut b = ORSet::new(2);
        let d = a.add("x");

        b.merge_delta(&d);
        b.merge_delta(&d); // duplicate
        b.merge_delta(&d); // triplicate

        assert_eq!(b.len(), 1); // still 1
    }
}
