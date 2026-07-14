// KG: SPAN_333_L2_Kademlia, ATOM_L2_Kademlia_KBucket
// K-bucket with LRU eviction policy. Each bucket holds up to K entries.

use identity333::NodeId;
use std::collections::VecDeque;

pub const K_DEFAULT: usize = 20;

/// K-bucket: recency-ordered NodeId queue. Front = least recently seen, back = most recent.
#[derive(Debug, Clone)]
pub struct KBucket {
    k: usize,
    entries: VecDeque<NodeId>,
}

impl KBucket {
    pub fn new(k: usize) -> Self {
        Self { k, entries: VecDeque::with_capacity(k) }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn is_full(&self) -> bool {
        self.entries.len() >= self.k
    }

    pub fn contains(&self, n: &NodeId) -> bool {
        self.entries.iter().any(|e| e == n)
    }

    pub fn entries(&self) -> impl Iterator<Item = &NodeId> {
        self.entries.iter()
    }

    /// Observe a NodeId. If present, move to back (most recent). If absent and bucket not full, push back.
    /// If bucket is full, drop the least-recently-seen (front) — simple replacement policy.
    /// Returns true if the bucket changed.
    pub fn observe(&mut self, n: NodeId) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| *e == n) {
            if pos == self.entries.len() - 1 {
                return false; // already most recent
            }
            self.entries.remove(pos);
            self.entries.push_back(n);
            return true;
        }
        if self.is_full() {
            self.entries.pop_front();
        }
        self.entries.push_back(n);
        true
    }

    pub fn remove(&mut self, n: &NodeId) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| e == n) {
            self.entries.remove(pos);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nid(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    #[test]
    fn new_observation_inserts() {
        let mut kb = KBucket::new(3);
        assert!(kb.observe(nid(1)));
        assert_eq!(kb.len(), 1);
    }

    #[test]
    fn reobserving_existing_bumps_to_back() {
        let mut kb = KBucket::new(3);
        kb.observe(nid(1));
        kb.observe(nid(2));
        assert!(kb.observe(nid(1))); // bump 1 to back
        let order: Vec<_> = kb.entries().copied().collect();
        assert_eq!(order[0], nid(2));
        assert_eq!(order[1], nid(1));
    }

    #[test]
    fn full_bucket_evicts_lru_on_new() {
        let mut kb = KBucket::new(2);
        kb.observe(nid(1));
        kb.observe(nid(2));
        kb.observe(nid(3)); // evicts nid(1)
        assert!(!kb.contains(&nid(1)));
        assert!(kb.contains(&nid(2)));
        assert!(kb.contains(&nid(3)));
        assert_eq!(kb.len(), 2);
    }

    #[test]
    fn reobserve_most_recent_is_noop() {
        let mut kb = KBucket::new(3);
        kb.observe(nid(1));
        kb.observe(nid(2));
        assert!(!kb.observe(nid(2))); // already at back
    }

    #[test]
    fn remove_shrinks() {
        let mut kb = KBucket::new(3);
        kb.observe(nid(1));
        kb.observe(nid(2));
        assert!(kb.remove(&nid(1)));
        assert_eq!(kb.len(), 1);
        assert!(!kb.remove(&nid(99))); // nonexistent
    }
}
