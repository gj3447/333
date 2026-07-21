//! The DUR-6 typestate at the consensus seam: a spend decision only comes
//! back wrapped in `Externalized` (proof of sync), and it survives restart.
#![cfg(all(feature = "wal", unix))]

use p333_consensus::Ledger;
use p333_ltdd::wal_store::{WalOptions, WalStore};
use p333_ltdd::{verify_present, Store, Verdict};

const META: &[u8] = b"p333-consensus-durable";

#[test]
fn spend_durable_receipts_cover_the_events_and_survive_reopen() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("store");
    let mut s = WalStore::create(&dir, META, WalOptions::default()).unwrap();
    let mut l = Ledger::default();
    l.register(&mut s, "tx-1", "coin-A");

    let ok = l.spend_durable(&mut s, "tx-1", "coin-A", 0, "alice->bob").unwrap();
    assert!(*ok.value());
    assert_eq!(ok.receipt().last_seq, 2, "object_registered + spend_finalized both covered");

    // the equivocating spend is REJECTED, and the rejection itself is durable
    let no = l.spend_durable(&mut s, "tx-1", "coin-A", 0, "alice->carol").unwrap();
    assert!(!*no.value());
    assert_eq!(no.receipt().last_seq, 3);
    drop(s);

    let s2 = WalStore::open(&dir, META, WalOptions::default()).unwrap();
    assert_eq!(verify_present(&s2, "tx-1", "object_registered", 1), Verdict::Present);
    assert_eq!(verify_present(&s2, "tx-1", "spend_finalized", 1), Verdict::Present);
    assert_eq!(verify_present(&s2, "tx-1", "equivocation_rejected", 1), Verdict::Present);
    // exactly-once even through the disk: the safety law reads back intact
    let finals = s2.query("tx-1").iter().filter(|e| e.event == "spend_finalized").count();
    assert_eq!(finals, 1);
}
