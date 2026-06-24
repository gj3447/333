//! PROM · receipt — Lane A owned-object safety, LTDD-verified. An object version finalizes at most
//! once; equivocating (conflicting) spends are rejected. The verdict is read back from the store.

use p333_consensus::Ledger;
use p333_ltdd::{verify_present, MemoryStore, Store, Verdict};

#[test]
fn a_valid_spend_finalizes_and_advances_the_version() {
    let mut s = MemoryStore::default();
    let mut l = Ledger::default();
    l.register("coin-A");
    assert!(l.spend(&mut s, "tx-1", "coin-A", 0, "alice->bob"));
    assert_eq!(l.current_version("coin-A"), Some(1));
    assert_eq!(verify_present(&s, "tx-1", "spend_finalized", 1), Verdict::Present);
}

#[test]
fn equivocation_is_rejected_so_no_double_spend_finalizes() {
    let mut s = MemoryStore::default();
    let mut l = Ledger::default();
    l.register("coin-A");
    // two conflicting spends of the SAME version 0 (the owner equivocates)
    assert!(l.spend(&mut s, "tx-eq", "coin-A", 0, "alice->bob"));
    assert!(!l.spend(&mut s, "tx-eq", "coin-A", 0, "alice->carol"));

    // the safety property: the object version finalized AT MOST ONCE.
    let finals = s.query("tx-eq").iter().filter(|e| e.event == "spend_finalized").count();
    assert_eq!(finals, 1, "an owned object version must finalize at most once");
    assert_eq!(verify_present(&s, "tx-eq", "equivocation_rejected", 1), Verdict::Present);
    assert_eq!(l.current_version("coin-A"), Some(1)); // advanced exactly once
}

#[test]
fn a_stale_spend_after_finalize_is_also_rejected() {
    let mut s = MemoryStore::default();
    let mut l = Ledger::default();
    l.register("coin-A");
    l.spend(&mut s, "tx-stale", "coin-A", 0, "v0"); // finalizes, version -> 1
    assert!(!l.spend(&mut s, "tx-stale", "coin-A", 0, "replay-v0")); // stale replay rejected
    assert!(l.spend(&mut s, "tx-stale", "coin-A", 1, "v1")); // the fresh version finalizes
    let finals = s.query("tx-stale").iter().filter(|e| e.event == "spend_finalized").count();
    assert_eq!(finals, 2, "two distinct versions finalize once each; the replay did not");
}
