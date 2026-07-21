//! PROM · receipt — the consistency lane's CONVERGENCE law, LTDD-verified.
//!
//! A state-based CRDT's merge is commutative/associative/idempotent, so replicas that exchange
//! state converge regardless of order. The LTDD question is the *effect*: did the replicas
//! ACTUALLY converge? The receipt reads each replica's recorded final state back from the store
//! and asserts `converged` present + `replica_diverged` ABSENT (the `absent`/forbid primitive).
//! A replica that missed a sync message ends at a different value — surfaced as `replica_diverged`
//! (the silent-loss analog for CRDT sync). This is the law any adopted CRDT (Lane B = yrs) must
//! pass; a minimal G-Counter exercises it here.

use p333_ltdd::{verify_present, MemoryStore, Store, Verdict};
use p333_crdt::{gossip, record_convergence, record_states, yrs_text_converge, GCounter};

#[test]
fn merge_is_commutative_associative_idempotent() {
    let mut a = GCounter::default();
    a.increment("a", 3);
    let mut b = GCounter::default();
    b.increment("b", 5);

    let mut ab = a.clone();
    ab.merge(&b);
    let mut ba = b.clone();
    ba.merge(&a);
    assert_eq!(ab, ba, "merge must be commutative");

    let mut ab_again = ab.clone();
    ab_again.merge(&b);
    assert_eq!(ab_again, ab, "merge must be idempotent");
    assert_eq!(ab.value(), 8);
}

#[test]
fn replicas_converge_after_gossip_and_no_divergence_is_recorded() {
    let mut s = MemoryStore::default();
    let mut r = [
        ("a", GCounter::default()),
        ("b", GCounter::default()),
        ("c", GCounter::default()),
    ];
    r[0].1.increment("a", 2);
    r[1].1.increment("b", 3);
    r[2].1.increment("c", 4);

    gossip(&mut r); // full all-to-all state exchange
    assert!(record_states(&mut s, "doc-1", &r), "all replicas should agree after gossip");

    assert_eq!(verify_present(&s, "doc-1", "converged", 1), Verdict::Present);
    assert_eq!(verify_present(&s, "doc-1", "replica_diverged", 1), Verdict::Absent); // forbid
    for (_, c) in &r {
        assert_eq!(c.value(), 9); // 2+3+4
    }
}

#[test]
fn a_replica_that_missed_sync_is_recorded_as_diverged() {
    // a real failure mode: a sync message was dropped, so one replica never saw the others'
    // updates. Its value differs — the receipt catches it (a green return value would not).
    let mut s = MemoryStore::default();
    let mut r = [
        ("a", GCounter::default()),
        ("b", GCounter::default()),
    ];
    r[0].1.increment("a", 2);
    r[1].1.increment("b", 3);
    // NO gossip for replica b (it missed the sync); record the divergent states as-is.
    let only_a_gossiped = {
        let snapshot: Vec<GCounter> = r.iter().map(|(_, c)| c.clone()).collect();
        for other in &snapshot {
            r[0].1.merge(other); // only a merges everyone -> a=5, b still 3
        }
        record_states(&mut s, "doc-2", &r)
    };
    assert!(!only_a_gossiped, "replicas disagree (5 vs 3) -> not converged");
    assert_eq!(verify_present(&s, "doc-2", "replica_diverged", 1), Verdict::Present);
    assert_eq!(verify_present(&s, "doc-2", "converged", 1), Verdict::Absent);
}

// ── the SAME receipt, run against the REAL production CRDT (yrs / Lane B) ──────────

#[test]
fn yrs_replicas_converge_under_concurrent_edits() {
    // three replicas each insert different text at position 0 concurrently (a real conflict),
    // then full-mesh gossip their updates. yrs resolves the interleaving deterministically, so
    // all replicas converge byte-identically — asserted via the SAME trace + gate as the G-Counter.
    let mut s = MemoryStore::default();
    let converged =
        yrs_text_converge(&mut s, "ydoc-1", &[("a", "hello "), ("b", "world"), ("c", "!!!")]);
    assert!(converged, "yrs replicas must converge under concurrent edits");

    assert_eq!(verify_present(&s, "ydoc-1", "converged", 1), Verdict::Present);
    assert_eq!(verify_present(&s, "ydoc-1", "replica_diverged", 1), Verdict::Absent);

    let finals: Vec<String> = s
        .query("ydoc-1")
        .iter()
        .filter(|e| e.event == "replica_final")
        .map(|e| e.attrs.get("state").unwrap().as_str().unwrap().to_string())
        .collect();
    assert_eq!(finals.len(), 3);
    assert!(finals.windows(2).all(|w| w[0] == w[1]), "all three replicas byte-identical");
    assert!(!finals[0].is_empty(), "and non-trivial (the edits actually merged)");
}

#[test]
fn a_yrs_replica_that_missed_an_update_is_recorded_as_diverged() {
    // model a dropped sync: `a` receives `b`'s update, but `b` never receives `a`'s -> they end
    // at different states. The receipt catches it (a "merged ok" return value would not).
    use yrs::updates::decoder::Decode;
    use yrs::{Doc, GetString, ReadTxn, StateVector, Text, Transact, Update};

    let mut s = MemoryStore::default();
    let a = Doc::new();
    let b = Doc::new();
    let ta = a.get_or_insert_text("doc");
    let tb = b.get_or_insert_text("doc");
    {
        let mut txn = a.transact_mut();
        ta.insert(&mut txn, 0, "hello ");
    }
    {
        let mut txn = b.transact_mut();
        tb.insert(&mut txn, 0, "world");
    }
    let ub = b.transact().encode_state_as_update_v1(&StateVector::default());
    {
        let mut txn = a.transact_mut();
        txn.apply_update(Update::decode_v1(&ub).unwrap()).unwrap(); // a sees b
    }
    // b's apply of a's update is DROPPED -> b stays "world", a has both.
    let sa = ta.get_string(&a.transact());
    let sb = tb.get_string(&b.transact());
    assert_ne!(sa, sb, "the dropped sync left them divergent");

    let converged = record_convergence(&mut s, "ydoc-2", &[("a", sa), ("b", sb)]);
    assert!(!converged);
    assert_eq!(verify_present(&s, "ydoc-2", "replica_diverged", 1), Verdict::Present);
    assert_eq!(verify_present(&s, "ydoc-2", "converged", 1), Verdict::Absent);
}
