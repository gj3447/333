//! PROM · receipt — credits-as-owned-objects: billing composes Lane A (no-double-debit) and the
//! metering conservation. Read back from the store: each debit is a finalized owned-object spend,
//! the costs conserve, and a replayed debit does NOT double-charge.

use p333_billing::Account;
use p333_ltdd::{verify_present, MemoryStore, Store, Verdict};

fn sum_u64(store: &impl Store, cid: &str, event: &str, field: &str) -> u64 {
    store
        .query(cid)
        .iter()
        .filter(|e| e.event == event)
        .map(|e| e.attrs.get(field).and_then(|v| v.as_u64()).unwrap_or(0))
        .sum()
}

#[test]
fn billing_conserves_and_each_debit_is_a_finalized_spend() {
    let mut s = MemoryStore::default();
    let mut acct = Account::new("alice-credits", 100);
    assert!(acct.meter_and_bill(&mut s, "sess-1", 2048, 1)); // cost 2
    assert!(acct.meter_and_bill(&mut s, "sess-1", 1024, 1)); // cost 1
    assert!(acct.meter_and_bill(&mut s, "sess-1", 5000, 1)); // cost 5

    assert_eq!(acct.balance(), 100 - 8);
    assert_eq!(acct.version(), 3, "three finalized spends advanced the owned-object version 3x");
    // conservation: every metered cost was actually debited
    assert_eq!(
        sum_u64(&s, "sess-1", "relay_forwarded", "cost"),
        sum_u64(&s, "sess-1", "credit_debited", "amount"),
    );
    // and every debit is a finalized owned-object spend (no free debit)
    let finals = s.query("sess-1").iter().filter(|e| e.event == "spend_finalized").count();
    assert_eq!(finals, 3);
}

#[test]
fn a_replayed_debit_message_does_not_double_charge() {
    let mut s = MemoryStore::default();
    let mut acct = Account::new("alice-credits", 100);
    acct.meter_and_bill(&mut s, "sess-2", 2048, 1); // version 0 -> 1, balance 98
    let before = acct.balance();
    // a replayed debit at the now-STALE version 0 must be rejected, not re-charged
    assert!(!acct.replay_debit_at(&mut s, "sess-2", 0));
    assert_eq!(acct.balance(), before, "a replay must not charge the account again");
    assert_eq!(verify_present(&s, "sess-2", "equivocation_rejected", 1), Verdict::Present);
}

#[test]
fn an_underfunded_account_gates() {
    let mut s = MemoryStore::default();
    let mut acct = Account::new("poor", 1);
    assert!(!acct.meter_and_bill(&mut s, "sess-3", 4096, 1)); // cost 4 > 1
    assert_eq!(verify_present(&s, "sess-3", "relay_gated", 1), Verdict::Present);
    assert_eq!(acct.balance(), 1); // never overdrawn
}
