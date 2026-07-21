//! REFUTER repro: real power loss (byte-level tail loss, not abort()) cutting
//! INSIDE one billed transition's 3 WAL records. Simulation: after an
//! un-synced meter_and_bill, physically truncate the tail segment; open()'s
//! torn-tail repair truncates the torn last record, leaving a record-boundary
//! clean prefix — exactly what writeback + power loss can leave.
#![cfg(all(feature = "wal", unix))]

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use p333_billing::Account;
use p333_ltdd::wal_store::{WalOptions, WalStore};
use p333_ltdd::Store;

const META: &[u8] = b"p333-billing-durable";
const CID: &str = "durable-sess";

fn sum_u64(store: &impl Store, cid: &str, event: &str, field: &str) -> u64 {
    store
        .query(cid)
        .iter()
        .filter(|e| e.event == event)
        .map(|e| e.attrs.get(field).and_then(|v| v.as_u64()).unwrap_or(0))
        .sum()
}

fn count(store: &impl Store, cid: &str, event: &str) -> usize {
    store.query(cid).iter().filter(|e| e.event == event).count()
}

fn seg_file(dir: &Path) -> PathBuf {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().map(|x| x == "wal").unwrap_or(false))
        .expect("one segment file")
}

/// Chop `n` bytes off the tail segment — the byte-level power-loss fixture.
fn lose_tail_bytes(dir: &Path, n: u64) {
    let p = seg_file(dir);
    let f = OpenOptions::new().write(true).open(&p).unwrap();
    let len = f.metadata().unwrap().len();
    f.set_len(len - n).unwrap();
    f.sync_all().unwrap();
}

#[test]
fn power_loss_mid_transition_tears_a_debit_in_half() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("store");
    {
        let mut s = WalStore::create(&dir, META, WalOptions::default()).unwrap();
        let mut acct = Account::new(&mut s, CID, "alice-credits", 100);
        // one RECEIPTED transition: records 3 (spend_finalized), 4
        // (relay_forwarded), 5 (credit_debited) — synced.
        let ext = acct.meter_and_bill_durable(&mut s, CID, 2048, 1).unwrap();
        assert!(*ext.value());
        assert_eq!(ext.receipt().last_seq, 5);
        // one UN-SYNCED transition: records 6/7/8, no receipt exists.
        assert!(acct.meter_and_bill(&mut s, CID, 2048, 1));
        // drop = release the flock; Drop's sync is irrelevant because the
        // power-loss fixture below erases tail bytes regardless.
    }

    // --- power loss #1: the very tail of record 8 (credit_debited) never hit
    // the platter. open() classifies the partial record as TornRecord and
    // truncates it (DUR-8) — a "legal" recovery.
    lose_tail_bytes(&dir, 1);
    {
        let s = WalStore::open(&dir, META, WalOptions::default()).unwrap();
        let finals = count(&s, CID, "spend_finalized");
        let forwards = count(&s, CID, "relay_forwarded");
        let debits = count(&s, CID, "credit_debited");
        let cost_sum = sum_u64(&s, CID, "relay_forwarded", "cost");
        let debit_sum = sum_u64(&s, CID, "credit_debited", "amount");
        println!("cut inside record 8: finals={finals} forwards={forwards} debits={debits} cost_sum={cost_sum} debit_sum={debit_sum}");
        // The commit's own judge (durable_billing.rs L90-98) over this trace:
        assert_ne!(cost_sum, debit_sum, "conservation is BROKEN: forwarded {cost_sum} but debited {debit_sum}");
        assert_ne!(forwards, debits, "a forward exists with no debit");
        assert_ne!(debits, finals, "a finalized spend exists with no debit");
        // The ack boundary itself is intact: the receipted transition survived.
        assert!(s.last_synced() >= 5);
    }

    // --- power loss #2: open() above already re-truncated the file to the
    // record-7 boundary. Lose one more byte: record 7 (relay_forwarded) is now
    // torn too — replay ends right after record 6 (spend_finalized).
    lose_tail_bytes(&dir, 1);
    {
        let s = WalStore::open(&dir, META, WalOptions::default()).unwrap();
        let finals = count(&s, CID, "spend_finalized");
        let forwards = count(&s, CID, "relay_forwarded");
        let debits = count(&s, CID, "credit_debited");
        println!("cut after record 6: finals={finals} forwards={forwards} debits={debits}");
        assert_eq!(finals, 2, "both spend_finalized records replay");
        assert_eq!(forwards, 1);
        assert_eq!(debits, 1);
        assert_ne!(debits, finals, "spend_finalized WITHOUT credit_debited: the half-debit the test comment says can never exist");
    }
}
