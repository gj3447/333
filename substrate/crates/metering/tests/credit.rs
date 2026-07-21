//! PROM · receipt — relay metering -> credit-bucket, LTDD-verified.
//!
//! The billing question is an ARRIVAL question: the relay returning "forwarded ok" is a claim;
//! the receipt asserts, read back from the store, that the metered cost was ACTUALLY debited
//! (no free-riding) and that the bucket GATES when it can't pay. The load-bearing check is the
//! conservation invariant `sum(cost@relay_forwarded) == sum(amount@credit_debited)` — the
//! "queryable causal cycle" rung, a value-consistency relation, not just event existence.

use p333_ltdd::{verify_present, Event, MemoryStore, Store, Verdict};
use p333_metering::{cost, relay_forward, CreditBucket};

fn sum_u64(store: &impl Store, cid: &str, event: &str, field: &str) -> u64 {
    store
        .query(cid)
        .iter()
        .filter(|e| e.event == event)
        .map(|e| e.attrs.get(field).and_then(|v| v.as_u64()).unwrap_or(0))
        .sum()
}

#[test]
fn cost_is_ceil_kib_times_rate() {
    assert_eq!(cost(2048, 1), 2); // 2 KiB
    assert_eq!(cost(1024, 1), 1);
    assert_eq!(cost(5000, 1), 5); // ceil(5000/1024)=5
    assert_eq!(cost(1, 3), 3); // ceil(1/1024)=1 KiB * rate 3
    assert_eq!(cost(0, 5), 0);
}

#[test]
fn metered_forwards_debit_credit_and_conserve() {
    let mut s = MemoryStore::default();
    let mut bucket = CreditBucket::new(&mut s, "sess-1", 100);
    assert!(relay_forward(&mut s, "sess-1", &mut bucket, 2048, 1)); // cost 2
    assert!(relay_forward(&mut s, "sess-1", &mut bucket, 1024, 1)); // cost 1
    assert!(relay_forward(&mut s, "sess-1", &mut bucket, 5000, 1)); // cost 5

    // arrival: both event types actually landed
    assert_eq!(verify_present(&s, "sess-1", "relay_forwarded", 3), Verdict::Present);
    assert_eq!(verify_present(&s, "sess-1", "credit_debited", 3), Verdict::Present);
    // CONSERVATION: every metered cost was actually debited (queryable-causal tier)
    assert_eq!(
        sum_u64(&s, "sess-1", "relay_forwarded", "cost"),
        sum_u64(&s, "sess-1", "credit_debited", "amount"),
    );
    assert_eq!(bucket.balance(), 100 - (2 + 1 + 5));
}

#[test]
fn empty_bucket_gates_the_relay_and_never_overdraws() {
    let mut s = MemoryStore::default();
    let mut bucket = CreditBucket::new(&mut s, "sess-2", 3);
    assert!(relay_forward(&mut s, "sess-2", &mut bucket, 2048, 1)); // cost 2 -> ok, 1 left
    assert!(!relay_forward(&mut s, "sess-2", &mut bucket, 4096, 1)); // cost 4 > 1 -> GATED, refused

    assert_eq!(verify_present(&s, "sess-2", "relay_gated", 1), Verdict::Present); // the gate fired
    assert_eq!(bucket.balance(), 1); // never negative, never over-debited
    // conservation holds over the SUCCESSFUL forwards (the gated one debited nothing)
    assert_eq!(
        sum_u64(&s, "sess-2", "relay_forwarded", "cost"),
        sum_u64(&s, "sess-2", "credit_debited", "amount"),
    );
}

#[test]
fn a_leaky_relay_that_forwards_without_debiting_breaks_conservation() {
    // ADVERSARIAL: a buggy/cheating relay meters the forward but the debit silently never lands
    // (free-riding). Its return value would say "ok"; the conservation invariant is what catches it.
    let mut s = MemoryStore::default();
    let mut bucket = CreditBucket::new(&mut s, "sess-3", 100);
    relay_forward(&mut s, "sess-3", &mut bucket, 2048, 1); // honest: cost 2, debited 2
    s.ship(&[Event::new("sess-3", "relay_forwarded").with("bytes", 9000u64).with("cost", 9u64)]); // leaked: no debit

    // metered cost (2+9=11) no longer equals debited (2): the receipt catches the free ride.
    assert_ne!(
        sum_u64(&s, "sess-3", "relay_forwarded", "cost"),
        sum_u64(&s, "sess-3", "credit_debited", "amount"),
    );
}
