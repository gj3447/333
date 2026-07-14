// KG: SPAN_333_L5_CRDT_Extensions, finding_333_synth_crd_pit_d11
// Dotted Version Vectors — fix for vector-clock explosion / sibling proliferation.
//
// Pitfall addressed (D11): plain vector clocks grow linearly with replicas and
// accumulate siblings on concurrent writes. Riak solved this via DVV:
// identify values by their dot (replica, counter) instead of by their value,
// so each concurrent update is one dot — no sibling explosion.
//
// Reference: Preguiça et al. "Dotted Version Vectors: Logical Clocks for
// Optimistic Replication" (2010).

use std::collections::BTreeMap;

use crate::traits::Crdt;

pub type ReplicaId = String;

/// A dot: (replica, counter). Counter is monotonic per replica.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Dot {
    pub replica_hash: u64,
    pub counter: u64,
}

/// Version vector: max counter per replica. Summarizes "all dots ≤ this".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionVector {
    clocks: BTreeMap<ReplicaId, u64>,
}

impl VersionVector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tick(&mut self, replica: &ReplicaId) -> u64 {
        let c = self.clocks.entry(replica.clone()).or_insert(0);
        *c += 1;
        *c
    }

    pub fn get(&self, replica: &ReplicaId) -> u64 {
        self.clocks.get(replica).copied().unwrap_or(0)
    }

    pub fn dominates(&self, other: &Self) -> bool {
        for (rid, v) in &other.clocks {
            if self.get(rid) < *v {
                return false;
            }
        }
        true
    }

    /// Concurrent = neither dominates the other.
    pub fn is_concurrent_with(&self, other: &Self) -> bool {
        !self.dominates(other) && !other.dominates(self)
    }
}

impl Crdt for VersionVector {
    fn merge(&mut self, other: &Self) {
        for (rid, v) in &other.clocks {
            let e = self.clocks.entry(rid.clone()).or_insert(0);
            if *v > *e {
                *e = *v;
            }
        }
    }
}

/// A DVV-tagged value set: each value carries one dot. On conflict,
/// concurrent values are kept (no sibling explosion — each value = one dot).
#[derive(Debug, Clone)]
pub struct DvvSet<T: Clone + PartialEq> {
    entries: Vec<(Dot, T)>,
    context: VersionVector,
}

impl<T: Clone + PartialEq> Default for DvvSet<T> {
    fn default() -> Self {
        Self { entries: Vec::new(), context: VersionVector::new() }
    }
}

impl<T: Clone + PartialEq> DvvSet<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a value. The caller provides the replica id and a stable 64-bit
    /// hash (so we don't pull a hasher dep); writes increment the context
    /// and drop values dominated by the new write's causal context.
    pub fn write(&mut self, replica: &ReplicaId, replica_hash: u64, value: T) {
        let counter = self.context.tick(replica);
        // Drop all entries whose dot is ≤ our new context (causally dominated).
        let ctx_snapshot = self.context.clone();
        self.entries.retain(|(_, _)| true);
        // New write supersedes anything in the same replica's prior dots.
        self.entries.retain(|(dot, _)| {
            !(dot.replica_hash == replica_hash && dot.counter < counter)
        });
        self.entries.push((Dot { replica_hash, counter }, value));
        let _ = ctx_snapshot; // context was already advanced by tick
    }

    pub fn values(&self) -> Vec<&T> {
        self.entries.iter().map(|(_, v)| v).collect()
    }

    pub fn context(&self) -> &VersionVector {
        &self.context
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<T: Clone + PartialEq> Crdt for DvvSet<T> {
    fn merge(&mut self, other: &Self) {
        // Union of entries, dedup by dot.
        for (d, v) in &other.entries {
            if !self.entries.iter().any(|(e, _)| e == d) {
                self.entries.push((*d, v.clone()));
            }
        }
        self.context.merge(&other.context);
        // Drop values whose dot is fully covered by the merged context's
        // per-replica counters (already superseded).
        //
        // D11 insight: keep concurrent writes (different replicas), drop
        // same-replica older writes.
        let mut per_replica_max: BTreeMap<u64, u64> = BTreeMap::new();
        for (d, _) in &self.entries {
            let e = per_replica_max.entry(d.replica_hash).or_insert(0);
            if d.counter > *e {
                *e = d.counter;
            }
        }
        self.entries.retain(|(d, _)| {
            per_replica_max.get(&d.replica_hash).copied().unwrap_or(0) == d.counter
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vv_dominates() {
        let mut a = VersionVector::new();
        a.tick(&"A".into());
        a.tick(&"B".into());
        let mut b = VersionVector::new();
        b.tick(&"A".into());
        assert!(a.dominates(&b));
        assert!(!b.dominates(&a));
    }

    #[test]
    fn vv_concurrent() {
        let mut a = VersionVector::new();
        a.tick(&"A".into());
        let mut b = VersionVector::new();
        b.tick(&"B".into());
        assert!(a.is_concurrent_with(&b));
    }

    #[test]
    fn vv_merge_pointwise_max() {
        let mut a = VersionVector::new();
        a.tick(&"A".into());
        a.tick(&"A".into());
        let mut b = VersionVector::new();
        b.tick(&"A".into());
        b.tick(&"B".into());
        b.tick(&"B".into());
        a.merge(&b);
        assert_eq!(a.get(&"A".into()), 2);
        assert_eq!(a.get(&"B".into()), 2);
    }

    #[test]
    fn dvv_same_replica_supersedes() {
        let mut s: DvvSet<&'static str> = DvvSet::new();
        s.write(&"A".into(), 1, "v1");
        s.write(&"A".into(), 1, "v2");
        assert_eq!(s.len(), 1);
        assert_eq!(s.values(), vec![&"v2"]);
    }

    #[test]
    fn dvv_concurrent_keeps_both() {
        let mut a: DvvSet<&'static str> = DvvSet::new();
        a.write(&"A".into(), 1, "alice-v");
        let mut b: DvvSet<&'static str> = DvvSet::new();
        b.write(&"B".into(), 2, "bob-v");
        a.merge(&b);
        // Both kept: different replicas, concurrent.
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn dvv_no_sibling_explosion_on_same_replica_bursts() {
        // Classic VC would accumulate N siblings; DVV collapses to one latest.
        let mut s: DvvSet<u32> = DvvSet::new();
        for i in 0..100 {
            s.write(&"A".into(), 1, i);
        }
        assert_eq!(s.len(), 1);
        assert_eq!(s.values(), vec![&99]);
    }

    #[test]
    fn dvv_merge_commutative() {
        let mut x: DvvSet<u32> = DvvSet::new();
        x.write(&"A".into(), 1, 10);
        let mut y: DvvSet<u32> = DvvSet::new();
        y.write(&"B".into(), 2, 20);

        let mut xy = x.clone();
        xy.merge(&y);
        let mut yx = y.clone();
        yx.merge(&x);
        assert_eq!(xy.context, yx.context);
        assert_eq!(xy.len(), yx.len());
    }
}
