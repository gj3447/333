//! REFUTER REPRO (temporary, not part of the commit): does the plain
//! (non-durable) meter_and_bill path, called on a WalStore whose ship
//! failure is held, return a confident `true` while every event of the
//! debit is silently dropped — with the ledger version advanced and the
//! balance debited in memory only?
#![cfg(all(feature = "wal", unix))]

use p333_billing::Account;
use p333_ltdd::wal_store::{WalOptions, WalStore, MAX_PAYLOAD};
use p333_ltdd::{Event, Store};

const META: &[u8] = b"refuter-plain-path";
const CID: &str = "refuter-sess";

#[test]
fn plain_path_returns_true_while_held_failure_drops_every_event() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("store");
    let mut s = WalStore::create(&dir, META, WalOptions::default()).unwrap();
    let mut acct = Account::new(&mut s, CID, "alice-credits", 100);
    // Make the pre-failure trace durable so the reopened life has a baseline.
    s.sync().expect("baseline sync must succeed");

    // Induce a held ship failure: one oversized payload -> RecordTooLarge,
    // held in WalStore.pending. From here every subsequent ship is a no-op.
    let huge = "x".repeat(MAX_PAYLOAD + 1);
    s.ship(&[Event::new(CID, "oversized").with("blob", huge.as_str())]);
    assert!(s.sync().is_err(), "the held failure must surface at sync()");

    let events_before = s.query(CID).len();
    let ver_before = acct.version();
    let bal_before = acct.balance();

    // THE CLAIMED DEFECT PATH: plain (non-durable) meter_and_bill on the
    // failed WalStore. cost(2048, 1) = 2 credits.
    let forwarded = acct.meter_and_bill(&mut s, CID, 2048, 1);

    // (1) confident actionable bool
    assert!(forwarded, "plain path hands back true");
    // (2) in-memory state advanced as if the debit were real
    assert_eq!(acct.balance(), bal_before - 2, "balance debited in memory");
    assert_eq!(acct.version(), ver_before + 1, "ledger version advanced");
    // (3) yet EVERY event of the debit was dropped — not even the in-process
    // index saw spend_finalized / relay_forwarded / credit_debited
    assert_eq!(
        s.query(CID).len(),
        events_before,
        "all 3 debit events silently dropped by the held ship"
    );
    // (4) the caller only learns anything if it happens to call sync() later
    assert!(s.sync().is_err(), "failure is only visible via a later sync()");
    drop(s);

    // (5) durability check: the reopened life never saw the debit at all —
    // the account state that returned `true` is pure amnesia-equivocation bait.
    let s2 = WalStore::open(&dir, META, WalOptions::default()).unwrap();
    let finals = s2.query(CID).iter().filter(|e| e.event == "spend_finalized").count();
    assert_eq!(finals, 0, "the debit trace was never appended to the WAL");
    let funded = s2.query(CID).iter().filter(|e| e.event == "account_funded").count();
    assert_eq!(funded, 1, "baseline (pre-failure) trace IS durable");
}

/// Counter-check for the refutation angle: the durable path on the same held
/// store correctly refuses to hand back a decision (Err, no Externalized).
#[test]
fn durable_path_on_held_store_refuses_the_decision() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("store");
    let mut s = WalStore::create(&dir, META, WalOptions::default()).unwrap();
    let mut acct = Account::new(&mut s, CID, "alice-credits", 100);
    let huge = "x".repeat(MAX_PAYLOAD + 1);
    s.ship(&[Event::new(CID, "oversized").with("blob", huge.as_str())]);

    let res = acct.meter_and_bill_durable(&mut s, CID, 2048, 1);
    assert!(res.is_err(), "durable path must surface the held failure");
    // ... but note: even here the in-memory debit already happened before
    // the Err (meter_and_bill ran first), version+balance mutated:
    assert_eq!(acct.balance(), 98);
    assert_eq!(acct.version(), 1);
}
