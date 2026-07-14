// KG: SPAN_333_L2_Kademlia, ATOM_L2_Kademlia_RoutingTable
// 256 k-buckets indexed by leading-zero count of XOR distance to local NodeId.

use identity333::NodeId;

use crate::distance::Distance;
use crate::kbucket::{KBucket, K_DEFAULT};

const KEY_BITS: usize = 256;

pub struct RoutingTable {
    local: NodeId,
    k: usize,
    buckets: Vec<KBucket>,
}

impl RoutingTable {
    pub fn new(local: NodeId) -> Self {
        Self::with_k(local, K_DEFAULT)
    }

    pub fn with_k(local: NodeId, k: usize) -> Self {
        let buckets = (0..KEY_BITS).map(|_| KBucket::new(k)).collect();
        Self { local, k, buckets }
    }

    pub fn local(&self) -> NodeId {
        self.local
    }

    pub fn k(&self) -> usize {
        self.k
    }

    /// Bucket index for a peer = leading_zeros(XOR(local, peer)).
    /// Index KEY_BITS (256) means the peer is ourselves — skipped.
    pub fn bucket_index(&self, peer: &NodeId) -> Option<usize> {
        let d = Distance::between(&self.local, peer);
        let lz = d.leading_zeros() as usize;
        if lz >= KEY_BITS {
            None // self
        } else {
            Some(lz)
        }
    }

    /// Observe a peer. Self is ignored.
    pub fn observe(&mut self, peer: NodeId) -> bool {
        match self.bucket_index(&peer) {
            Some(i) => self.buckets[i].observe(peer),
            None => false,
        }
    }

    /// Number of known peers across all buckets.
    pub fn len(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Find the `count` peers closest (by XOR distance) to `target`.
    pub fn find_closest(&self, target: &NodeId, count: usize) -> Vec<NodeId> {
        let mut all: Vec<NodeId> = self
            .buckets
            .iter()
            .flat_map(|b| b.entries().copied())
            .collect();
        all.sort_by_key(|n| Distance::between(n, target));
        all.truncate(count);
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nid(first: u8) -> NodeId {
        let mut b = [0u8; 32];
        b[0] = first;
        NodeId::from_bytes(b)
    }

    #[test]
    fn self_is_not_added() {
        let me = nid(0x00);
        let mut rt = RoutingTable::with_k(me, 3);
        assert!(!rt.observe(me));
        assert_eq!(rt.len(), 0);
    }

    #[test]
    fn peer_goes_to_correct_bucket() {
        let me = nid(0x00);
        let mut rt = RoutingTable::with_k(me, 3);
        // peer 0x80..00 → XOR with me = 0x80..00 → 0 leading zeros → bucket 0
        let peer = nid(0x80);
        rt.observe(peer);
        assert_eq!(rt.bucket_index(&peer), Some(0));
        assert_eq!(rt.len(), 1);
    }

    #[test]
    fn find_closest_orders_by_xor() {
        let me = nid(0x00);
        let mut rt = RoutingTable::with_k(me, 5);
        let peers = [nid(0x01), nid(0x80), nid(0x02), nid(0x40), nid(0x10)];
        for p in peers {
            rt.observe(p);
        }
        let target = nid(0x02);
        let closest = rt.find_closest(&target, 3);
        assert_eq!(closest.len(), 3);
        // 0x02 (dist 0), 0x01 (0x03 = 3), 0x10 (0x12 = 18) — in that order.
        assert_eq!(closest[0], nid(0x02));
        assert_eq!(closest[1], nid(0x01));
        assert_eq!(closest[2], nid(0x10));
    }

    #[test]
    fn find_closest_on_empty_table() {
        let me = nid(0x00);
        let rt = RoutingTable::with_k(me, 3);
        assert!(rt.find_closest(&nid(0x80), 5).is_empty());
    }

    #[test]
    fn many_peers_spread_across_buckets() {
        let me = nid(0x00);
        let mut rt = RoutingTable::with_k(me, 2);
        // Force 3 peers in same bucket 0 (first byte 0x80..0xFF all LZ=0).
        rt.observe(nid(0x80));
        rt.observe(nid(0x90));
        rt.observe(nid(0xA0));
        // Bucket capacity 2 → 0x80 evicted (LRU).
        let in_bucket: Vec<_> = rt.find_closest(&me, 10);
        assert_eq!(rt.len(), 2);
        assert!(!in_bucket.contains(&nid(0x80)));
    }
}
