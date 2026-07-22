// SPDX-License-Identifier: AGPL-3.0-only
// Modified from Garage src/util/crdt/map.rs (GNU AGPLv3) for 333 in 2026:
// reduced API, deterministic two-pointer merge, and local tests.
// See ../NOTICE and ../../THIRD_PARTY_NOTICES.md.
// KG: TASK_ATOM_L5_GP_Map, CONTRACT_ATOM_L5_GP_Map
// Composite CRDT Map: key → V where V is itself a CRDT. Per-key recursive merge.
// Ported from garage::util::crdt::map.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::traits::Crdt;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Map<K, V> {
    vals: Vec<(K, V)>,
}

impl<K, V> Default for Map<K, V>
where
    K: Ord + Clone,
    V: Crdt + Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> Map<K, V>
where
    K: Ord + Clone,
    V: Crdt + Clone,
{
    pub fn new() -> Self {
        Self { vals: vec![] }
    }

    pub fn put_mutator(k: K, v: V) -> Self {
        Self { vals: vec![(k, v)] }
    }

    pub fn put(&mut self, k: K, v: V) {
        self.merge(&Self::put_mutator(k, v));
    }

    pub fn get(&self, k: &K) -> Option<&V> {
        self.vals
            .binary_search_by(|(k2, _)| k2.cmp(k))
            .ok()
            .map(|i| &self.vals[i].1)
    }

    pub fn items(&self) -> &[(K, V)] {
        &self.vals
    }

    pub fn len(&self) -> usize {
        self.vals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vals.is_empty()
    }

    pub fn clear(&mut self) {
        self.vals.clear();
    }
}

impl<K, V> Crdt for Map<K, V>
where
    K: Ord + Clone,
    V: Crdt + Clone,
{
    fn merge(&mut self, other: &Self) {
        // Two-pointer merge of sorted vecs.
        let mut out: Vec<(K, V)> = Vec::with_capacity(self.vals.len() + other.vals.len());
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
                    let mut v = a[i].1.clone();
                    v.merge(&b[j].1);
                    out.push((a[i].0.clone(), v));
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
    use crate::lww::Lww;

    #[test]
    fn map_inserts_and_merges_recursively() {
        let mut a: Map<String, Lww<u32>> = Map::new();
        a.put("alpha".into(), Lww::raw(100, 5));

        let mut b: Map<String, Lww<u32>> = Map::new();
        b.put("alpha".into(), Lww::raw(200, 10));
        b.put("beta".into(), Lww::raw(50, 3));

        a.merge(&b);
        assert_eq!(a.get(&"alpha".into()).map(|l| *l.get()), Some(10));
        assert_eq!(a.get(&"beta".into()).map(|l| *l.get()), Some(3));
    }

    #[test]
    fn map_empty_identity() {
        let mut a: Map<u32, u32> = Map::new();
        a.put(1, 42);
        let empty = Map::<u32, u32>::new();
        let snapshot = a.clone();
        a.merge(&empty);
        assert_eq!(a, snapshot);
    }
}
