//! 333 v2 substrate — consistency lane (Lane B): the CONVERGENCE law, LTDD-verified.
//!
//! Per the OSS survey, Lane B adopts **yrs** (Yjs/Rust CRDT). This crate is the
//! verification half: the convergence-law receipt every state-based CRDT — yrs included —
//! must pass. A minimal **state-based G-Counter** exercises it: per-replica slots, value =
//! their sum, `merge` = elementwise max (commutative, associative, idempotent), so replicas
//! converge regardless of the order they exchange state.
//!
//! HARD RULE (LTDD): the verdict is the *effect*, read back from the store — did the replicas
//! ACTUALLY converge? [`record_states`] emits each replica's final value and a `converged`
//! marker iff they all agree, else a `replica_diverged` event per offender (e.g. a replica that
//! missed a sync message — the silent-loss analog for CRDT sync). The gate then asserts
//! `converged` present and `replica_diverged` ABSENT.

use p333_ltdd::{Event, Store};
use std::collections::BTreeMap;

/// A grow-only counter CRDT. State-based; merge is elementwise max.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GCounter {
    slots: BTreeMap<String, u64>,
}

impl GCounter {
    /// Increment this replica's own slot by `by`.
    pub fn increment(&mut self, replica: &str, by: u64) {
        *self.slots.entry(replica.to_string()).or_insert(0) += by;
    }

    /// The counter's value — the sum of all replica slots.
    pub fn value(&self) -> u64 {
        self.slots.values().sum()
    }

    /// Merge another replica's state in: each slot becomes the max of the two. Commutative,
    /// associative, idempotent — the state-based CRDT (CvRDT) merge law.
    pub fn merge(&mut self, other: &GCounter) {
        for (replica, &v) in &other.slots {
            let slot = self.slots.entry(replica.clone()).or_insert(0);
            if v > *slot {
                *slot = v;
            }
        }
    }
}

/// Full all-to-all state exchange (gossip): every replica merges every replica's current state.
/// Because the merge law holds, the order is irrelevant — all replicas end identical.
pub fn gossip(replicas: &mut [(&str, GCounter)]) {
    let snapshot: Vec<GCounter> = replicas.iter().map(|(_, c)| c.clone()).collect();
    for (_, counter) in replicas.iter_mut() {
        for other in &snapshot {
            counter.merge(other);
        }
    }
}

/// Record each replica's final value to the store and judge convergence. Emits `replica_final`
/// per replica; `converged` iff they all agree; a `replica_diverged` per replica that doesn't.
/// Returns whether they converged. The store — not this bool — is the receipt's judge.
pub fn record_states(store: &mut impl Store, cid: &str, replicas: &[(&str, GCounter)]) -> bool {
    let canonical = replicas.first().map(|(_, c)| c.value()).unwrap_or(0);
    let mut converged = true;
    for (id, counter) in replicas {
        let v = counter.value();
        store.ship(&[Event::new(cid, "replica_final")
            .with("replica", *id)
            .with("value", v)]);
        if v != canonical {
            converged = false;
            store.ship(&[Event::new(cid, "replica_diverged")
                .with("replica", *id)
                .with("value", v)
                .with("expected", canonical)]);
        }
    }
    if converged {
        store.ship(&[Event::new(cid, "converged")
            .with("value", canonical)
            .with("replicas", replicas.len() as u64)]);
    }
    converged
}
