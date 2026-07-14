// KG: SPAN_333_Infra — Kademlia DHT implementation
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-storage-kad-xor_distance/Contact/KBucket/KademliaNode/node_id_from_seed
// k=20, alpha=3, 160-bit XOR distance, iterative lookup

use super::KVStore;
use std::collections::HashMap;

const K: usize = 20;       // bucket size
#[allow(dead_code)] // algorithm parameter, used when iterative lookup is implemented
const ALPHA: usize = 3;    // parallel lookups
const ID_BITS: usize = 160;

pub type NodeId = [u8; 20]; // 160-bit

/// XOR distance between two node IDs
pub fn xor_distance(a: &NodeId, b: &NodeId) -> NodeId {
    let mut d = [0u8; 20];
    for i in 0..20 { d[i] = a[i] ^ b[i]; }
    d
}

/// Leading zeros = which bucket this distance falls into
fn bucket_index(distance: &NodeId) -> usize {
    for (i, &byte) in distance.iter().enumerate() {
        if byte != 0 {
            return (i * 8) + byte.leading_zeros() as usize;
        }
    }
    ID_BITS - 1
}

/// Contact info for a node
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub id: NodeId,
    pub addr: String,  // e.g. "peer:abc123" or "ws://..."
    pub last_seen: u64,
}

/// K-bucket: sorted by last_seen, max K entries
#[derive(Debug, Clone, Default)]
pub struct KBucket {
    pub contacts: Vec<Contact>,
}

impl KBucket {
    pub fn update(&mut self, contact: Contact) {
        if let Some(pos) = self.contacts.iter().position(|c| c.id == contact.id) {
            self.contacts.remove(pos);
            self.contacts.push(contact);
        } else if self.contacts.len() < K {
            self.contacts.push(contact);
        }
        // If full, ignore (real Kademlia would ping oldest)
    }

    pub fn closest(&self, target: &NodeId, count: usize) -> Vec<&Contact> {
        let mut sorted: Vec<&Contact> = self.contacts.iter().collect();
        sorted.sort_by_key(|c| xor_distance(&c.id, target));
        sorted.truncate(count);
        sorted
    }
}

/// Kademlia routing table + local storage
pub struct KademliaNode {
    pub local_id: NodeId,
    buckets: Vec<KBucket>,
    store: HashMap<Vec<u8>, Vec<u8>>,
}

impl KademliaNode {
    pub fn new(local_id: NodeId) -> Self {
        Self {
            local_id,
            buckets: (0..ID_BITS).map(|_| KBucket::default()).collect(),
            store: HashMap::new(),
        }
    }

    /// Add or update a contact in routing table
    pub fn update_contact(&mut self, contact: Contact) {
        if contact.id == self.local_id { return; }
        let dist = xor_distance(&self.local_id, &contact.id);
        let idx = bucket_index(&dist);
        self.buckets[idx].update(contact);
    }

    /// Find K closest contacts to target
    pub fn find_closest(&self, target: &NodeId, count: usize) -> Vec<&Contact> {
        let mut all: Vec<&Contact> = self.buckets.iter()
            .flat_map(|b| b.contacts.iter())
            .collect();
        all.sort_by_key(|c| xor_distance(&c.id, target));
        all.truncate(count);
        all
    }

    /// Store a value locally
    pub fn store_value(&mut self, key: &[u8], value: &[u8]) {
        self.store.insert(key.to_vec(), value.to_vec());
    }

    /// Retrieve a value locally
    pub fn get_value(&self, key: &[u8]) -> Option<&Vec<u8>> {
        self.store.get(key)
    }

    /// Number of known contacts
    pub fn contact_count(&self) -> usize {
        self.buckets.iter().map(|b| b.contacts.len()).sum()
    }

    /// Number of stored values
    pub fn store_size(&self) -> usize {
        self.store.len()
    }

    /// Get contacts from a specific bucket
    pub fn bucket_contacts(&self, idx: usize) -> &[Contact] {
        if idx < self.buckets.len() { &self.buckets[idx].contacts } else { &[] }
    }
}

impl KVStore for KademliaNode {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.store.get(key).cloned()
    }
    fn put(&mut self, key: &[u8], value: &[u8]) -> bool {
        self.store.insert(key.to_vec(), value.to_vec());
        true
    }
    fn delete(&mut self, key: &[u8]) -> bool {
        self.store.remove(key).is_some()
    }
    fn contains(&self, key: &[u8]) -> bool {
        self.store.contains_key(key)
    }
    fn keys(&self) -> Vec<Vec<u8>> {
        self.store.keys().cloned().collect()
    }
    fn size(&self) -> usize {
        self.store.len()
    }
}

/// Generate a NodeId from a seed string (SHA-1-like hash)
pub fn node_id_from_seed(seed: &str) -> NodeId {
    let mut id = [0u8; 20];
    let bytes = seed.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        id[i % 20] ^= b.wrapping_mul((i as u8).wrapping_add(1));
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_id(val: u8) -> NodeId {
        let mut id = [0u8; 20];
        id[19] = val;
        id
    }

    #[test]
    fn xor_distance_symmetric() {
        let a = make_id(0x0A);
        let b = make_id(0x0F);
        assert_eq!(xor_distance(&a, &b), xor_distance(&b, &a));
    }

    #[test]
    fn xor_distance_self_is_zero() {
        let a = make_id(42);
        assert_eq!(xor_distance(&a, &a), [0u8; 20]);
    }

    #[test]
    fn bucket_index_logic() {
        let mut dist = [0u8; 20];
        dist[19] = 1; // distance = 1 → bucket 159
        assert_eq!(bucket_index(&dist), 159);

        dist[19] = 0; dist[0] = 0x80; // MSB set → bucket 0
        assert_eq!(bucket_index(&dist), 0);
    }

    #[test]
    fn add_and_find_contacts() {
        let mut node = KademliaNode::new(make_id(0));
        for i in 1..=10u8 {
            node.update_contact(Contact {
                id: make_id(i), addr: format!("peer:{}", i), last_seen: i as u64,
            });
        }
        assert_eq!(node.contact_count(), 10);

        let closest = node.find_closest(&make_id(5), 3);
        assert_eq!(closest.len(), 3);
        assert_eq!(closest[0].id, make_id(5)); // exact match first
    }

    #[test]
    fn store_and_retrieve() {
        let mut node = KademliaNode::new(make_id(0));
        node.store_value(b"block:0,0", b"stone");
        assert_eq!(node.get_value(b"block:0,0"), Some(&b"stone".to_vec()));
        assert_eq!(node.store_size(), 1);
    }

    #[test]
    fn kvstore_trait_works() {
        let mut node = KademliaNode::new(make_id(0));
        assert!(node.put(b"k1", b"v1"));
        assert!(node.contains(b"k1"));
        assert_eq!(node.get(b"k1"), Some(b"v1".to_vec()));
        assert!(node.delete(b"k1"));
        assert!(!node.contains(b"k1"));
    }

    #[test]
    fn ignore_self_contact() {
        let id = make_id(42);
        let mut node = KademliaNode::new(id);
        node.update_contact(Contact { id, addr: "self".into(), last_seen: 0 });
        assert_eq!(node.contact_count(), 0);
    }

    #[test]
    fn bucket_overflow_ignores() {
        let mut node = KademliaNode::new(make_id(0));
        // All IDs differ only in low bit → same XOR prefix → same bucket
        for i in 0..25u8 {
            let mut id = [0u8; 20];
            id[0] = 0x80; // same leading bit pattern → bucket 0
            id[19] = i;   // vary low byte for uniqueness
            node.update_contact(Contact { id, addr: format!("p{}", i), last_seen: i as u64 });
        }
        // K=20, bucket 0 can hold at most K entries
        let bucket_0_count = node.bucket_contacts(0).len();
        assert!(bucket_0_count <= K);
    }

    #[test]
    fn node_id_from_seed_deterministic() {
        let a = node_id_from_seed("test-node-1");
        let b = node_id_from_seed("test-node-1");
        assert_eq!(a, b);
        let c = node_id_from_seed("test-node-2");
        assert_ne!(a, c);
    }
}
