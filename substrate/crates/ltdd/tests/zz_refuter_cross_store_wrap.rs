//! REFUTER REPRO (temporary, not committed): can an Externalized<T> be minted
//! whose receipt covers none of the wrapped value's trace (cross-store wrap)?
#![cfg(all(feature = "wal", unix))]

use p333_ltdd::wal_store::{WalOptions, WalStore};
use p333_ltdd::{Event, MemoryStore, Store};

fn tmpdir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("refuter_xwrap_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// Claimed defect, path 1: decision's trace shipped to a MemoryStore,
/// wrapped by a fresh (empty, never-shipped) WalStore.
#[test]
fn cross_store_wrap_mints_proof_with_empty_receipt() {
    // The decision and its trace live in a MemoryStore (volatile).
    let mut mem = MemoryStore::default();
    mem.ship(&[
        Event::new("cid-1", "spend_finalized").with("object", "obj").with("version", 1u64),
    ]);
    let decision = true; // computed against `mem`'s state

    // A second, EMPTY WalStore that has shipped nothing.
    let dir = tmpdir("empty_wrapper");
    let mut wrapper = WalStore::create(&dir, b"meta", WalOptions::default()).unwrap();

    // externalize accepts the foreign decision and succeeds.
    let ext = wrapper.externalize(decision).expect("mints despite covering nothing");
    assert_eq!(*ext.value(), true);
    // The receipt proves durability of... nothing: last_seq = 0.
    assert_eq!(ext.receipt().last_seq, 0);
    assert_eq!(wrapper.last_synced(), 0);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Claimed defect, path 2: two WalStores. Trace shipped (un-synced) to A,
/// wrapped by empty B; simulated crash of A loses the trace while the
/// proof-carrying Externalized survives.
#[test]
fn second_walstore_wrap_survives_crash_that_loses_the_trace() {
    let dir_a = tmpdir("store_a");
    let dir_b = tmpdir("store_b");

    let mut a = WalStore::create(&dir_a, b"meta", WalOptions::default()).unwrap();
    let mut b = WalStore::create(&dir_b, b"meta", WalOptions::default()).unwrap();

    // The decision's trace goes to A, never synced.
    a.ship(&[Event::new("cid-2", "spend_finalized").with("object", "obj")]);
    assert_eq!(a.query("cid-2").len(), 1); // visible in-process
    let decision = true;

    // Wrapped by B (which has the events of... nothing).
    let ext = b.externalize(decision).expect("wrong-store wrap succeeds");
    assert_eq!(ext.receipt().last_seq, 0);

    // Simulate crash of A before its sync: drop without sync is not enough
    // (Drop may sync); emulate by just checking what a receipt-demanding API
    // sees — it holds Externalized<bool> while a.last_synced() == 0.
    assert_eq!(a.last_synced(), 0, "trace behind the proof-carrying value is NOT durable");

    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}
