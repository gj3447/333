//! Durability receipts for p333-wal (LTDD style: assert what the disk
//! ACTUALLY holds after reopen, not a return value).
//!
//! Honesty note (absorption §5): the kill-based receipt exercises
//! *process-crash* recovery — SIGKILL does not drop the OS page cache, so it
//! proves the recovery invariants (synced prefix intact, no corrupt record
//! ever surfaces, torn tail truncated), not physical power-loss. The
//! byte-level truncation/corruption fixtures below simulate the torn-write
//! shapes a power cut leaves (partial frame, zero sectors), which is the same
//! oracle etcd's TestOpenOnTornWrite uses.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use p333_wal::{DurableReceipt, Entry, Wal, WalError, WalOptions};

const META: &[u8] = b"p333-wal-test";

fn small_opts() -> WalOptions {
    WalOptions { segment_size: 4 * 1024 }
}

fn tmpdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn wal_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().map(|x| x == "wal").unwrap_or(false))
        .collect();
    v.sort();
    v
}

#[test]
fn roundtrip_across_segments() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let mut wal = Wal::create(&dir, META, small_opts()).unwrap();

    let mut want = Vec::new();
    for i in 0..200u64 {
        let payload = vec![i as u8; (i % 97) as usize + 1];
        let seq = wal.append(&payload).unwrap();
        assert_eq!(seq, i + 1, "seq auto-increments from 1");
        want.push(Entry { seq, payload });
    }
    let receipt = wal.sync().unwrap();
    assert_eq!(receipt.last_seq, 200);
    drop(wal);

    assert!(wal_files(&dir).len() > 1, "small segment_size must have forced cuts");

    let (got, wal2) = Wal::open(&dir, META, small_opts()).unwrap();
    assert_eq!(got, want, "reopen replays exactly what was synced (crc handoff across cuts, FMT-4)");
    assert_eq!(wal2.last_synced(), 200);
}

#[test]
fn append_continues_after_reopen() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let mut wal = Wal::create(&dir, META, small_opts()).unwrap();
    wal.append(b"a").unwrap();
    wal.append(b"b").unwrap();
    wal.sync().unwrap();
    drop(wal);

    let (entries, mut wal) = Wal::open(&dir, META, small_opts()).unwrap();
    assert_eq!(entries.len(), 2);
    let seq = wal.append(b"c").unwrap();
    assert_eq!(seq, 3, "seq continues at last+1 after replay");
    wal.sync().unwrap();
    drop(wal);

    let (entries, _wal) = Wal::open(&dir, META, small_opts()).unwrap();
    assert_eq!(entries.last().unwrap().seq, 3);
}

#[test]
fn single_writer_flock() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let wal = Wal::create(&dir, META, small_opts()).unwrap();
    // second writer must bounce while the first is alive (API-7)
    match Wal::open(&dir, META, small_opts()) {
        Err(WalError::Locked) => {}
        other => panic!("expected Locked, got {other:?}", other = other.map(|_| ())),
    }
    drop(wal);
    Wal::open(&dir, META, small_opts()).expect("lock released on drop");
}

#[test]
fn metadata_conflict_detected() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let mut wal = Wal::create(&dir, META, small_opts()).unwrap();
    wal.append(b"x").unwrap();
    wal.sync().unwrap();
    drop(wal);
    match Wal::open(&dir, b"someone-else", small_opts()) {
        Err(WalError::MetadataConflict) => {}
        other => panic!("expected MetadataConflict, got {other:?}", other = other.map(|_| ())),
    }
}

/// Torn-write fixture: truncate the last file at EVERY byte offset inside the
/// tail record. Reopen must always recover exactly the synced prefix — never
/// an error, never a partial record (FMT-7, DUR-8).
#[test]
fn torn_tail_truncation_at_every_offset() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let mut wal = Wal::create(&dir, META, WalOptions::default()).unwrap();
    for i in 0..5u64 {
        wal.append(&vec![i as u8 + 1; 100]).unwrap();
    }
    wal.sync().unwrap();
    let boundary = fs::metadata(wal_files(&dir).pop().unwrap()).unwrap().len();
    wal.append(&vec![0xAB; 100]).unwrap(); // the record we will tear
    wal.sync().unwrap();
    drop(wal);

    let master = td.path().join("master");
    fs::create_dir(&master).unwrap();
    for f in fs::read_dir(&dir).unwrap() {
        let p = f.unwrap().path();
        fs::copy(&p, master.join(p.file_name().unwrap())).unwrap();
    }
    let tail_name = wal_files(&dir).pop().unwrap().file_name().unwrap().to_owned();
    let full = fs::metadata(dir.join(&tail_name)).unwrap().len();

    for cut in boundary..full {
        // restore pristine copy, then tear at `cut`
        for f in fs::read_dir(&master).unwrap() {
            let p = f.unwrap().path();
            fs::copy(&p, dir.join(p.file_name().unwrap())).unwrap();
        }
        let f = fs::OpenOptions::new().write(true).open(dir.join(&tail_name)).unwrap();
        f.set_len(cut).unwrap();
        drop(f);

        let (entries, wal) = Wal::open(&dir, META, WalOptions::default())
            .unwrap_or_else(|e| panic!("torn at {cut}/{full}: reopen failed: {e}"));
        assert_eq!(entries.len(), 5, "torn at {cut}: synced prefix must survive exactly");
        assert!(entries.iter().enumerate().all(|(i, e)| e.payload == vec![i as u8 + 1; 100]));
        drop(wal);
    }
}

/// Zero-sector torn shape: garbage-free partial record followed by a zeroed
/// region (what a power cut typically leaves on a preallocated file).
#[test]
fn zero_sector_tail_is_torn_not_fatal() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let mut wal = Wal::create(&dir, META, WalOptions::default()).unwrap();
    wal.append(b"survivor").unwrap();
    wal.sync().unwrap();
    drop(wal);

    let tail = wal_files(&dir).pop().unwrap();
    let mut bytes = fs::read(&tail).unwrap();
    // append a plausible-length frame header then a zeroed sector
    bytes.extend_from_slice(&64u64.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 512]);
    fs::write(&tail, &bytes).unwrap();

    let (entries, _wal) = Wal::open(&dir, META, WalOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].payload, b"survivor");
}

/// NEGATIVE ORACLE (the RED that proves the gate can fail): flip one byte
/// inside a SYNCED record that has valid records after it. This is real
/// corruption, not a torn tail — reopen must refuse with CrcMismatch and must
/// NOT silently repair (FMT-8).
#[test]
fn corrupt_synced_record_is_fatal() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let mut wal = Wal::create(&dir, META, WalOptions::default()).unwrap();
    wal.append(&[0x11; 64]).unwrap();
    let first_end = {
        wal.sync().unwrap();
        fs::metadata(wal_files(&dir).pop().unwrap()).unwrap().len()
    };
    wal.append(&[0x22; 64]).unwrap();
    wal.sync().unwrap();
    drop(wal);

    let tail = wal_files(&dir).pop().unwrap();
    let mut bytes = fs::read(&tail).unwrap();
    let victim = (first_end as usize) - 20; // inside the first record's payload
    bytes[victim] ^= 0xFF;
    fs::write(&tail, &bytes).unwrap();

    match Wal::open(&dir, META, WalOptions::default()) {
        Err(WalError::CrcMismatch { .. }) => {}
        other => panic!(
            "corruption inside the synced region must be fatal, got {other:?}",
            other = other.map(|(e, _)| e.len())
        ),
    }
}

/// Crash receipt: a child process appends+syncs 1..=5, appends 6..=8 WITHOUT
/// sync, then dies by abort(). The parent reopens and asserts the ack
/// boundary: everything covered by the receipt survives intact; anything
/// after it is either absent or a clean intact prefix (never corruption).
#[test]
fn crash_receipt_synced_prefix_survives() {
    let td = tmpdir();
    let dir = td.path().join("wal");

    let exe = std::env::current_exe().unwrap();
    let status = Command::new(&exe)
        .args(["crash_child_body", "--exact", "--nocapture", "--include-ignored"])
        .env("P333_WAL_CRASH_DIR", &dir)
        .status()
        .unwrap();
    assert!(!status.success(), "child is expected to abort()");

    let (entries, wal) = Wal::open(&dir, META, WalOptions::default()).unwrap();
    assert!(entries.len() >= 5, "receipt covered 1..=5; got only {}", entries.len());
    for (i, e) in entries.iter().enumerate() {
        assert_eq!(e.seq, i as u64 + 1, "no gaps, no reordering");
        let want = if e.seq <= 5 { vec![e.seq as u8; 64] } else { vec![0xEE; 64] };
        assert_eq!(e.payload, want, "record {} intact", e.seq);
    }
    assert_eq!(wal.last_synced(), entries.len() as u64);
}

/// Not a test in the normal run: the crash child's body. Runs only when the
/// parent invokes this binary with P333_WAL_CRASH_DIR set.
#[test]
#[ignore]
fn crash_child_body() {
    let Some(dir) = std::env::var_os("P333_WAL_CRASH_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let mut wal = Wal::create(&dir, META, WalOptions::default()).unwrap();
    let mut receipt: Option<DurableReceipt> = None;
    for i in 1..=5u64 {
        wal.append(&vec![i as u8; 64]).unwrap();
        receipt = Some(wal.sync().unwrap());
    }
    assert_eq!(receipt.unwrap().last_seq, 5);
    for _ in 6..=8u64 {
        wal.append(&vec![0xEE; 64]).unwrap(); // deliberately NOT synced
    }
    // Die hard: no Drop, no flush. (SIGKILL semantics via abort.)
    std::process::abort();
}
