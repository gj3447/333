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
use p333_crdt::{gossip, record_states, GCounter};

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
