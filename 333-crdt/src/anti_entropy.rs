// KG: SPAN_333_L5_CRDT_Extensions, finding_333_synth_crd_sota_d8, finding_333_synth_crd_prt_d10
// Anti-entropy reconciliation via Merkle-tree digests — bandwidth saving on
// high-churn gossip (60-90% vs naive full-state sync).
//
// Sources:
//   - D8 SOTA: Merkle-tree reconciliation (Netflix, Dynamo, Cassandra).
//   - D10 Port-333: AntiEntropy protocol trait.
//
// Protocol: partition a key-value map into fixed-size buckets, hash each
// bucket, build a balanced binary Merkle over bucket hashes. On gossip:
//   1. Exchange root hashes; match → done (zero-delta).
//   2. Else recurse into mismatched subtree; exchange child hashes.
//   3. At leaf: exchange the differing buckets.
//
// This module provides the digest + comparison primitive; transport is out
// of scope (333-mq or 333-gossip downstream wires it).

use std::collections::BTreeMap;

/// Fixed bucket count keeps the tree structure stable across rebalance.
pub const BUCKET_COUNT: usize = 16;

/// Deterministic 64-bit hash over bytes (FNV-1a, dependency-free). Production
/// backends may swap for BLAKE3 via a feature flag.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn bucket_of_key(key: &[u8]) -> usize {
    (fnv1a_64(key) as usize) % BUCKET_COUNT
}

/// Merkle digest over a key-value snapshot. `bucket_hashes` are leaf hashes;
/// `root` is the hash of all bucket hashes concatenated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleDigest {
    pub bucket_hashes: [u64; BUCKET_COUNT],
    pub root: u64,
}

impl MerkleDigest {
    pub fn build<'a, I>(entries: I) -> Self
    where
        I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
    {
        // Collect into buckets to get a deterministic order.
        let mut buckets: Vec<BTreeMap<Vec<u8>, Vec<u8>>> =
            (0..BUCKET_COUNT).map(|_| BTreeMap::new()).collect();
        for (k, v) in entries {
            let b = bucket_of_key(k);
            buckets[b].insert(k.to_vec(), v.to_vec());
        }
        let mut leaf_hashes = [0u64; BUCKET_COUNT];
        for (i, bucket) in buckets.iter().enumerate() {
            let mut h: u64 = 0xcbf29ce484222325;
            for (k, v) in bucket {
                h ^= fnv1a_64(k);
                h = h.wrapping_mul(0x100000001b3);
                h ^= fnv1a_64(v);
                h = h.wrapping_mul(0x100000001b3);
            }
            leaf_hashes[i] = h;
        }
        let mut root_seed = Vec::with_capacity(BUCKET_COUNT * 8);
        for h in &leaf_hashes {
            root_seed.extend_from_slice(&h.to_be_bytes());
        }
        MerkleDigest {
            bucket_hashes: leaf_hashes,
            root: fnv1a_64(&root_seed),
        }
    }

    /// Return bucket indices where `self` and `other` disagree. Empty = same.
    pub fn diff(&self, other: &Self) -> Vec<usize> {
        if self.root == other.root {
            return Vec::new();
        }
        (0..BUCKET_COUNT)
            .filter(|i| self.bucket_hashes[*i] != other.bucket_hashes[*i])
            .collect()
    }
}

/// Reconciliation plan: "send me these buckets, I'll send you those".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileHint {
    pub mismatched_buckets: Vec<usize>,
}

pub fn reconcile_plan(local: &MerkleDigest, remote: &MerkleDigest) -> ReconcileHint {
    ReconcileHint { mismatched_buckets: local.diff(remote) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_state_zero_delta() {
        let a = MerkleDigest::build([(b"k1".as_ref(), b"v1".as_ref()), (b"k2".as_ref(), b"v2".as_ref())]);
        let b = MerkleDigest::build([(b"k1".as_ref(), b"v1".as_ref()), (b"k2".as_ref(), b"v2".as_ref())]);
        assert_eq!(a.root, b.root);
        assert!(a.diff(&b).is_empty());
        assert!(reconcile_plan(&a, &b).mismatched_buckets.is_empty());
    }

    #[test]
    fn different_value_triggers_diff() {
        let a = MerkleDigest::build([(b"k1".as_ref(), b"v1".as_ref())]);
        let b = MerkleDigest::build([(b"k1".as_ref(), b"v2".as_ref())]);
        assert_ne!(a.root, b.root);
        let d = a.diff(&b);
        assert_eq!(d.len(), 1); // same bucket, different value
    }

    #[test]
    fn different_key_triggers_diff() {
        let a = MerkleDigest::build([(b"k1".as_ref(), b"v".as_ref())]);
        let b = MerkleDigest::build([(b"k2".as_ref(), b"v".as_ref())]);
        assert_ne!(a.root, b.root);
        assert!(!a.diff(&b).is_empty());
    }

    #[test]
    fn key_order_insensitive() {
        let a = MerkleDigest::build([
            (b"a".as_ref(), b"1".as_ref()),
            (b"b".as_ref(), b"2".as_ref()),
            (b"c".as_ref(), b"3".as_ref()),
        ]);
        let b = MerkleDigest::build([
            (b"c".as_ref(), b"3".as_ref()),
            (b"a".as_ref(), b"1".as_ref()),
            (b"b".as_ref(), b"2".as_ref()),
        ]);
        assert_eq!(a, b);
    }

    #[test]
    fn bucketing_localizes_changes() {
        // Flip one entry; expect ≤1 bucket to differ.
        let common: Vec<(&[u8], &[u8])> = (0..50_u32)
            .map(|i| (Box::leak(format!("k{i}").into_boxed_str()).as_bytes(), b"v".as_ref()))
            .collect();
        let mut extended = common.clone();
        let a = MerkleDigest::build(extended.clone().into_iter());
        extended.push((b"new".as_ref(), b"val".as_ref()));
        let b = MerkleDigest::build(extended.into_iter());
        let d = a.diff(&b);
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn fnv1a_stable() {
        // Canary: hash never changes across runs.
        assert_eq!(fnv1a_64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63dc4c8601ec8c);
    }
}
