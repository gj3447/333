// KG: TASK_ATOM_L5_GP_Lww_Family, CONTRACT_ATOM_L5_GP_Lww_Family
// Last-Writer-Wins register + LwwMap. Ported from garage::util::crdt::{lww,lww_map}.
// Timestamps are user-provided (clock injection for testability). Tie-break via inner CRDT merge.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::traits::Crdt;

/// Last-Writer-Wins register: `(ts, value)`. Larger ts wins. Tie-break via inner merge.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Lww<T> {
    ts: u64,
    v: T,
}

impl<T: Crdt + Clone> Lww<T> {
    pub fn raw(ts: u64, value: T) -> Self {
        Self { ts, v: value }
    }

    pub fn timestamp(&self) -> u64 {
        self.ts
    }

    pub fn get(&self) -> &T {
        &self.v
    }

    /// Set value with an explicit timestamp that must exceed previous.
    /// Returns `false` if `new_ts` is not strictly greater (silent no-op — caller
    /// should check if a write was meant to be guaranteed).
    pub fn update(&mut self, new_ts: u64, new_v: T) -> bool {
        if new_ts > self.ts {
            self.ts = new_ts;
            self.v = new_v;
            true
        } else {
            false
        }
    }
}

impl<T: Crdt + Clone> Crdt for Lww<T> {
    fn merge(&mut self, other: &Self) {
        match self.ts.cmp(&other.ts) {
            Ordering::Less => {
                self.ts = other.ts;
                self.v = other.v.clone();
            }
            Ordering::Equal => {
                // Tie: merge inner CRDT.
                self.v.merge(&other.v);
            }
            Ordering::Greater => {} // keep self
        }
    }
}

// --------------------------------------------------------------------------
// LwwMap: key → (ts, V) with V: Crdt for conflict resolution.

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LwwMap<K, V> {
    // Sorted by ascending key for deterministic serialization and binary search.
    vals: Vec<(K, u64, V)>,
}

impl<K, V> Default for LwwMap<K, V>
where
    K: Ord + Clone,
    V: Crdt + Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> LwwMap<K, V>
where
    K: Ord + Clone,
    V: Crdt + Clone,
{
    pub fn new() -> Self {
        Self { vals: vec![] }
    }

    /// Single-entry map, useful as a delta-mutator.
    pub fn raw_item(k: K, ts: u64, v: V) -> Self {
        Self { vals: vec![(k, ts, v)] }
    }

    pub fn len(&self) -> usize {
        self.vals.len()
    }
    pub fn is_empty(&self) -> bool {
        self.vals.is_empty()
    }

    pub fn get(&self, k: &K) -> Option<&V> {
        self.vals
            .binary_search_by(|(k2, _, _)| k2.cmp(k))
            .ok()
            .map(|i| &self.vals[i].2)
    }

    pub fn get_with_ts(&self, k: &K) -> Option<(u64, &V)> {
        self.vals
            .binary_search_by(|(k2, _, _)| k2.cmp(k))
            .ok()
            .map(|i| (self.vals[i].1, &self.vals[i].2))
    }

    pub fn items(&self) -> &[(K, u64, V)] {
        &self.vals
    }

    /// Put with explicit timestamp (monotonic enforced on merge).
    pub fn put(&mut self, k: K, ts: u64, v: V) {
        self.merge(&Self::raw_item(k, ts, v));
    }
}

impl<K, V> Crdt for LwwMap<K, V>
where
    K: Ord + Clone,
    V: Crdt + Clone,
{
    fn merge(&mut self, other: &Self) {
        // Merge two sorted vecs. Conflicts resolved by: larger ts wins; equal ts → V::merge.
        let mut out: Vec<(K, u64, V)> = Vec::with_capacity(self.vals.len() + other.vals.len());
        let (mut i, mut j) = (0usize, 0usize);
        let a = &self.vals;
        let b = &other.vals;

        while i < a.len() && j < b.len() {
            match a[i].0.cmp(&b[j].0) {
                Ordering::Less => {
                    out.push(a[i].clone());
                    i += 1;
                }
                Ordering::Greater => {
                    out.push(b[j].clone());
                    j += 1;
                }
                Ordering::Equal => {
                    let (k, ts_a, va) = a[i].clone();
                    let (_, ts_b, vb) = b[j].clone();
                    let merged = match ts_a.cmp(&ts_b) {
                        Ordering::Less => (ts_b, vb),
                        Ordering::Greater => (ts_a, va),
                        Ordering::Equal => {
                            let mut v = va.clone();
                            v.merge(&vb);
                            (ts_a, v)
                        }
                    };
                    out.push((k, merged.0, merged.1));
                    i += 1;
                    j += 1;
                }
            }
        }
        while i < a.len() {
            out.push(a[i].clone());
            i += 1;
        }
        while j < b.len() {
            out.push(b[j].clone());
            j += 1;
        }
        self.vals = out;
    }
}

#[cfg(test)]
mod smoke {
    use super::*;

    #[test]
    fn lww_later_ts_wins() {
        let mut a: Lww<u32> = Lww::raw(100, 5);
        let b: Lww<u32> = Lww::raw(200, 10);
        a.merge(&b);
        assert_eq!(*a.get(), 10);
        assert_eq!(a.timestamp(), 200);
    }

    #[test]
    fn lww_earlier_ts_loses() {
        let mut a: Lww<u32> = Lww::raw(200, 10);
        let b: Lww<u32> = Lww::raw(100, 5);
        a.merge(&b);
        assert_eq!(*a.get(), 10);
    }

    #[test]
    fn lww_tie_uses_inner_max() {
        // Tie at ts=100, inner CRDT merge (u32 max).
        let mut a: Lww<u32> = Lww::raw(100, 5);
        let b: Lww<u32> = Lww::raw(100, 10);
        a.merge(&b);
        assert_eq!(*a.get(), 10);
    }

    #[test]
    fn lww_map_insert_and_merge() {
        let mut a: LwwMap<String, u32> = LwwMap::new();
        a.put("k1".into(), 100, 5);
        a.put("k2".into(), 100, 7);
        let mut b: LwwMap<String, u32> = LwwMap::new();
        b.put("k1".into(), 200, 50);
        b.put("k3".into(), 100, 9);
        a.merge(&b);
        assert_eq!(a.get(&"k1".into()), Some(&50));
        assert_eq!(a.get(&"k2".into()), Some(&7));
        assert_eq!(a.get(&"k3".into()), Some(&9));
    }
}
