// KG: SPAN_333_L5_CRDT_Extensions, finding_333_synth_crd_pit_d11
// PNCounter — positive/negative counter CRDT.
//
// Pitfall addressed (D11): ORSet is NOT a correct counter. Using OR-Set for
// increment counts converges on set-semantics, not integer-sum. PNCounter
// (Shapiro 2011) maintains per-replica (P, N) monotonic counters; value = ΣP - ΣN.
//
// Merge is pointwise max on each replica's (P, N) → commutative, associative,
// idempotent. Unlike GCounter, supports decrements.

use std::collections::BTreeMap;

use crate::traits::Crdt;

pub type ReplicaId = String;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PNCounter {
    p: BTreeMap<ReplicaId, u64>,
    n: BTreeMap<ReplicaId, u64>,
}

impl PNCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn increment(&mut self, replica: &ReplicaId, delta: u64) {
        let entry = self.p.entry(replica.clone()).or_insert(0);
        *entry = entry.saturating_add(delta);
    }

    pub fn decrement(&mut self, replica: &ReplicaId, delta: u64) {
        let entry = self.n.entry(replica.clone()).or_insert(0);
        *entry = entry.saturating_add(delta);
    }

    /// Value = ΣP - ΣN. Signed (i128) to avoid underflow on large negatives.
    pub fn value(&self) -> i128 {
        let pos: u128 = self.p.values().map(|v| *v as u128).sum();
        let neg: u128 = self.n.values().map(|v| *v as u128).sum();
        pos as i128 - neg as i128
    }
}

impl Crdt for PNCounter {
    fn merge(&mut self, other: &Self) {
        for (rid, v) in &other.p {
            let e = self.p.entry(rid.clone()).or_insert(0);
            if *v > *e {
                *e = *v;
            }
        }
        for (rid, v) in &other.n {
            let e = self.n.entry(rid.clone()).or_insert(0);
            if *v > *e {
                *e = *v;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increment_decrement_value() {
        let mut c = PNCounter::new();
        let a: ReplicaId = "A".into();
        let b: ReplicaId = "B".into();
        c.increment(&a, 10);
        c.increment(&b, 5);
        c.decrement(&a, 3);
        assert_eq!(c.value(), 12);
    }

    #[test]
    fn merge_commutative() {
        let a1: ReplicaId = "A".into();
        let b1: ReplicaId = "B".into();
        let mut x = PNCounter::new();
        x.increment(&a1, 7);
        let mut y = PNCounter::new();
        y.increment(&b1, 4);
        y.decrement(&a1, 2);

        let mut xy = x.clone();
        xy.merge(&y);
        let mut yx = y.clone();
        yx.merge(&x);
        assert_eq!(xy, yx);
        assert_eq!(xy.value(), 7 + 4 - 2);
    }

    #[test]
    fn merge_idempotent() {
        let a: ReplicaId = "A".into();
        let mut c = PNCounter::new();
        c.increment(&a, 100);
        let c2 = c.clone();
        c.merge(&c2);
        c.merge(&c2);
        assert_eq!(c.value(), 100);
    }

    #[test]
    fn concurrent_increments_both_counted() {
        // D11 pitfall: if we had used ORSet, concurrent increments would collapse.
        // PNCounter keeps both.
        let a: ReplicaId = "A".into();
        let b: ReplicaId = "B".into();
        let mut x = PNCounter::new();
        x.increment(&a, 5);
        let mut y = PNCounter::new();
        y.increment(&b, 5);
        x.merge(&y);
        assert_eq!(x.value(), 10);
    }

    #[test]
    fn saturating_increment_no_panic() {
        let a: ReplicaId = "A".into();
        let mut c = PNCounter::new();
        c.increment(&a, u64::MAX);
        c.increment(&a, 100);
        // Saturates; no overflow panic.
        assert_eq!(c.p[&a], u64::MAX);
    }
}
