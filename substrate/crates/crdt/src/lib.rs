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

/// Generic convergence recorder — CRDT-agnostic, so the SAME ooptdd gate judges a G-Counter or a
/// yrs document. Emits `replica_final{state}` per replica; `converged` iff all states agree, else a
/// `replica_diverged{state,expected}` per offender. The store — not this bool — is the judge.
pub fn record_convergence(store: &mut impl Store, cid: &str, states: &[(&str, String)]) -> bool {
    let canonical = states.first().map(|(_, s)| s.clone()).unwrap_or_default();
    let mut converged = true;
    for (id, state) in states {
        store.ship(&[Event::new(cid, "replica_final")
            .with("replica", *id)
            .with("state", state.clone())]);
        if *state != canonical {
            converged = false;
            store.ship(&[Event::new(cid, "replica_diverged")
                .with("replica", *id)
                .with("state", state.clone())
                .with("expected", canonical.clone())]);
        }
    }
    if converged {
        store.ship(&[Event::new(cid, "converged").with("replicas", states.len() as u64)]);
    }
    converged
}

/// Record G-Counter convergence by counter value — a thin wrapper over [`record_convergence`].
pub fn record_states(store: &mut impl Store, cid: &str, replicas: &[(&str, GCounter)]) -> bool {
    let states: Vec<(&str, String)> =
        replicas.iter().map(|(id, c)| (*id, c.value().to_string())).collect();
    record_convergence(store, cid, &states)
}

/// Drive `edits` (one `(replica_id, text)` per replica) as concurrent yrs `Text` inserts at
/// position 0, full-mesh gossip every replica's update to every replica, then record convergence.
/// yrs (Lane B's production CRDT) resolves the interleaving deterministically, so the replicas
/// converge byte-identically — this runs the convergence receipt against the REAL CRDT, not only
/// the minimal G-Counter.
pub fn yrs_text_converge(store: &mut impl Store, cid: &str, edits: &[(&str, &str)]) -> bool {
    use yrs::updates::decoder::Decode;
    use yrs::{Doc, GetString, ReadTxn, StateVector, Text, Transact, Update};

    let docs: Vec<Doc> = edits.iter().map(|_| Doc::new()).collect();
    for (i, (_, text)) in edits.iter().enumerate() {
        let t = docs[i].get_or_insert_text("doc");
        let mut txn = docs[i].transact_mut();
        t.insert(&mut txn, 0, text);
    }
    // full mesh: collect each replica's update, apply ALL to EVERY replica (idempotent re-applies ok)
    let updates: Vec<Vec<u8>> = docs
        .iter()
        .map(|d| d.transact().encode_state_as_update_v1(&StateVector::default()))
        .collect();
    for d in &docs {
        for u in &updates {
            let mut txn = d.transact_mut();
            txn.apply_update(Update::decode_v1(u).unwrap()).unwrap();
        }
    }
    let states: Vec<(&str, String)> = edits
        .iter()
        .enumerate()
        .map(|(i, (id, _))| {
            let t = docs[i].get_or_insert_text("doc");
            (*id, t.get_string(&docs[i].transact()))
        })
        .collect();
    record_convergence(store, cid, &states)
}
