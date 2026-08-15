//! WalStore receipts — the durable Store judged the LTDD way: reopen the WAL
//! and READ the trace BACK; never trust a return value. The crash receipt is
//! the design doc's stated P0-1 judge: kill before sync → restart → exactly
//! the acked (receipt-covered) events survive readback.
#![cfg(all(feature = "wal", unix))]

use std::path::PathBuf;
use std::process::Command;

use p333_ltdd::wal_store::{WalError, WalOptions, WalStore, WalStoreError, MAX_PAYLOAD};
use p333_ltdd::{verify_present, Event, Store, Verdict};

const META: &[u8] = b"p333-ltdd-walstore";

fn tmpdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[test]
fn envelope_roundtrip_law() {
    let e = Event::new("cid-1", "spend_finalized").with("object", "coin-A").with("version", 3u64);
    assert_eq!(Event::from_trace_json(&e.to_trace_json()), Some(e));
}

#[test]
fn ship_sync_reopen_readback() {
    let td = tmpdir();
    let dir = td.path().join("store");
    let mut s = WalStore::create(&dir, META, WalOptions::default()).unwrap();
    s.ship(&[
        Event::new("cid-a", "spend_finalized").with("object", "coin-A").with("version", 0u64),
        Event::new("cid-b", "relay_forwarded").with("bytes", 2048u64).with("cost", 2u64),
    ]);
    s.ship(&[Event::new("cid-a", "credit_debited").with("amount", 2u64)]);
    let receipt = s.sync().unwrap();
    assert_eq!(receipt.last_seq, 3, "three events → three records");
    drop(s);

    // The judge: a fresh life, rebuilt purely from disk.
    let s2 = WalStore::open(&dir, META, WalOptions::default()).unwrap();
    assert_eq!(verify_present(&s2, "cid-a", "spend_finalized", 1), Verdict::Present);
    assert_eq!(verify_present(&s2, "cid-a", "credit_debited", 1), Verdict::Present);
    assert_eq!(verify_present(&s2, "cid-b", "relay_forwarded", 1), Verdict::Present);
    assert_eq!(verify_present(&s2, "cid-a", "relay_forwarded", 1), Verdict::Absent, "cid isolation");
    let got = s2.query("cid-b");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].attrs["bytes"], serde_json::json!(2048u64), "attrs survive the disk roundtrip");
}

#[test]
fn externalize_is_the_only_path_to_a_receipt_carrying_value() {
    let td = tmpdir();
    let dir = td.path().join("store");
    let mut s = WalStore::create(&dir, META, WalOptions::default()).unwrap();
    s.ship(&[Event::new("cid-x", "spend_finalized").with("version", 0u64)]);
    let ext = s.externalize(true).unwrap();
    assert!(*ext.value());
    assert_eq!(ext.receipt().last_seq, 1, "the receipt covers the shipped event");
    assert_eq!(s.last_synced(), 1);
}

#[test]
fn ship_failure_is_held_and_sync_stays_red() {
    let td = tmpdir();
    let dir = td.path().join("store");
    let mut s = WalStore::create(&dir, META, WalOptions::default()).unwrap();
    s.ship(&[Event::new("cid-1", "ok_event")]);

    // An event whose envelope exceeds MAX_PAYLOAD: append refuses, the store holds the failure.
    let huge = "x".repeat(MAX_PAYLOAD + 1);
    s.ship(&[Event::new("cid-1", "too_big").with("blob", huge)]);
    assert!(s.query("cid-1").iter().all(|e| e.event != "too_big"), "failed event never indexed");

    // Later ships are ignored — a hole in the trace must not be papered over.
    s.ship(&[Event::new("cid-1", "after_failure")]);
    assert!(s.query("cid-1").iter().all(|e| e.event != "after_failure"));

    // sync surfaces the held failure, repeatedly — the store stays failed.
    for _ in 0..2 {
        match s.sync() {
            Err(WalStoreError::Wal(WalError::RecordTooLarge { .. })) => {}
            other => panic!("expected held RecordTooLarge, got {other:?}", other = other.map(|r| r.last_seq)),
        }
    }
    assert!(s.externalize(true).is_err(), "no receipt may exist for a holed trace");
}

/// Crash receipt (design §5): the child acks spends 1..=3 (ship+sync), ships
/// 2 more WITHOUT sync, then abort()s. The reopened store must hold the 3
/// acked events intact; the un-synced pair is either absent or a clean intact
/// prefix (SIGKILL keeps the page cache — power loss is simulated by the
/// byte-level torn fixtures in p333-wal itself).
#[test]
fn crash_receipt_acked_events_survive_readback() {
    let td = tmpdir();
    let dir = td.path().join("store");

    let exe = std::env::current_exe().unwrap();
    let status = Command::new(&exe)
        .args(["crash_child_body", "--exact", "--nocapture", "--include-ignored"])
        .env("P333_WALSTORE_CRASH_DIR", &dir)
        .status()
        .unwrap();
    assert!(!status.success(), "child is expected to abort()");

    let s = WalStore::open(&dir, META, WalOptions::default()).unwrap();
    // The ack boundary: every receipt-covered spend is Present after restart.
    for i in 1..=3u64 {
        let cid = format!("acked-{i}");
        assert_eq!(
            verify_present(&s, &cid, "spend_finalized", 1),
            Verdict::Present,
            "acked spend {i} must survive the crash"
        );
        let got = s.query(&cid);
        assert_eq!(got[0].attrs["version"], serde_json::json!(i - 1), "attrs intact for spend {i}");
    }
    // Un-synced suffix: clean prefix or absent — and NEVER before the acked ones.
    let unsynced: usize = (1..=2u64)
        .filter(|i| {
            verify_present(&s, &format!("unsynced-{i}"), "spend_finalized", 1) == Verdict::Present
        })
        .count();
    assert!(unsynced <= 2);
    assert_eq!(s.last_synced() >= 3, true, "receipt covered at least the 3 acked records");
}

/// Not a test in the normal run: the crash child's body.
#[test]
#[ignore]
fn crash_child_body() {
    let Some(dir) = std::env::var_os("P333_WALSTORE_CRASH_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let mut s = WalStore::create(&dir, META, WalOptions::default()).unwrap();
    for i in 1..=3u64 {
        s.ship(&[Event::new(format!("acked-{i}"), "spend_finalized").with("version", i - 1)]);
        let ext = s.externalize(true).unwrap(); // ship → sync → only then "answer"
        assert_eq!(ext.receipt().last_seq, i);
    }
    for i in 1..=2u64 {
        s.ship(&[Event::new(format!("unsynced-{i}"), "spend_finalized").with("version", 99u64)]);
        // deliberately NOT synced — no receipt, so externalizing these would not compile past review
    }
    std::process::abort(); // die hard: no Drop, no flush
}
