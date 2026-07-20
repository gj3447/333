//! Lifecycle receipts for the step-4 surface: `release_older` (LIF-5/API-9),
//! read-only `verify` (API-10), and backup-first `repair` (FMT-10). Same LTDD
//! discipline as durability.rs: every assertion reads the DISK back through
//! reopen/verify, never a return value alone.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};

use p333_wal::{repair, verify, Wal, WalError, WalOptions};

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

/// Build a multi-segment log of 200 entries (payload = seq byte repeated).
fn build_log(dir: &Path) -> Wal {
    let mut wal = Wal::create(dir, META, small_opts()).unwrap();
    for i in 1..=200u64 {
        wal.append(&vec![i as u8; (i % 97) as usize + 1]).unwrap();
    }
    wal.sync().unwrap();
    wal
}

#[test]
fn release_older_drops_covered_segments_and_keeps_the_rest() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let mut wal = build_log(&dir);
    let before = wal_files(&dir).len();
    assert!(before > 2, "need several segments for a meaningful release");

    let removed = wal.release_older(100).unwrap();
    assert!(removed > 0, "half the log is covered — something must go");
    assert_eq!(wal_files(&dir).len(), before - removed);
    drop(wal);

    // Reopen: the surviving run must still be contiguous and cover 101..=200,
    // and may start at or before 101 (a segment survives if it holds ANY
    // entry > upto).
    let (entries, wal2) = Wal::open(&dir, META, small_opts()).unwrap();
    assert_eq!(entries.last().unwrap().seq, 200);
    assert!(entries.first().unwrap().seq <= 101, "everything above upto must survive");
    for w in entries.windows(2) {
        assert_eq!(w[1].seq, w[0].seq + 1, "no seq gaps after release");
    }
    for e in &entries {
        assert_eq!(e.payload, vec![e.seq as u8; (e.seq % 97) as usize + 1], "entry {} intact", e.seq);
    }
    assert_eq!(wal2.last_synced(), 200);
}

#[test]
fn release_older_never_removes_the_tail_segment() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let mut wal = Wal::create(&dir, META, small_opts()).unwrap();
    wal.append(b"only").unwrap();
    wal.sync().unwrap();
    assert_eq!(wal.release_older(u64::MAX).unwrap(), 0, "single (tail) segment must survive");
    assert_eq!(wal_files(&dir).len(), 1);

    // Multi-segment: even upto=MAX keeps the active tail.
    drop(wal);
    let mut wal = build_log(&dir.parent().unwrap().join("wal2"));
    let removed = wal.release_older(u64::MAX).unwrap();
    let left = wal_files(&td.path().join("wal2")).len();
    assert!(removed > 0);
    assert_eq!(left, 1, "everything but the tail goes");
    // and the handle still appends fine
    let seq = wal.append(b"after-release").unwrap();
    assert_eq!(seq, 201);
    wal.sync().unwrap();
    drop(wal);
    let (entries, _w) = Wal::open(&td.path().join("wal2"), META, small_opts()).unwrap();
    assert_eq!(entries.last().unwrap().seq, 201);
}

#[test]
fn release_older_zero_is_a_noop() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let mut wal = build_log(&dir);
    let before = wal_files(&dir).len();
    assert_eq!(wal.release_older(0).unwrap(), 0);
    assert_eq!(wal_files(&dir).len(), before);
}

#[test]
fn verify_reports_a_clean_log() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let wal = build_log(&dir);
    let segments = wal_files(&dir).len();
    drop(wal);

    let report = verify(&dir, META).unwrap();
    assert_eq!(report.segments, segments);
    assert_eq!(report.entries, 200);
    assert_eq!(report.first_seq, 1);
    assert_eq!(report.last_seq, 200);
    assert!(!report.torn_tail);
}

#[test]
fn verify_is_read_only_on_a_torn_tail() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let mut wal = Wal::create(&dir, META, WalOptions::default()).unwrap();
    wal.append(b"survivor").unwrap();
    wal.sync().unwrap();
    wal.append(&vec![0xAB; 100]).unwrap();
    wal.sync().unwrap();
    drop(wal);

    // tear the tail record
    let tail = wal_files(&dir).pop().unwrap();
    let full = fs::metadata(&tail).unwrap().len();
    let f = fs::OpenOptions::new().write(true).open(&tail).unwrap();
    f.set_len(full - 30).unwrap();
    drop(f);
    let torn_len = fs::metadata(&tail).unwrap().len();

    let report = verify(&dir, META).unwrap();
    assert!(report.torn_tail, "the torn suffix must be reported");
    assert_eq!(report.entries, 1, "only the synced prefix verifies");
    assert_eq!(report.last_seq, 1);
    assert_eq!(
        fs::metadata(&tail).unwrap().len(),
        torn_len,
        "verify must not modify the file (that is repair's job)"
    );
}

#[test]
fn verify_rejects_corruption_of_the_synced_region() {
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
    bytes[first_end as usize - 20] ^= 0xFF;
    fs::write(&tail, &bytes).unwrap();

    match verify(&dir, META) {
        Err(WalError::CrcMismatch { .. }) => {}
        other => panic!("expected CrcMismatch, got {other:?}"),
    }
}

#[test]
fn verify_bounces_while_a_writer_is_alive() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let wal = Wal::create(&dir, META, WalOptions::default()).unwrap();
    match verify(&dir, META) {
        Err(WalError::Locked) => {}
        other => panic!("expected Locked, got {other:?}"),
    }
    drop(wal);
    verify(&dir, META).expect("lock released on drop");
}

#[test]
fn repair_backs_up_everything_then_truncates_the_torn_tail() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let mut wal = Wal::create(&dir, META, WalOptions::default()).unwrap();
    wal.append(b"survivor").unwrap();
    wal.sync().unwrap();
    wal.append(&vec![0xAB; 100]).unwrap();
    wal.sync().unwrap();
    drop(wal);

    let tail = wal_files(&dir).pop().unwrap();
    let full = fs::metadata(&tail).unwrap().len();
    let f = fs::OpenOptions::new().write(true).open(&tail).unwrap();
    f.set_len(full - 30).unwrap();
    drop(f);
    let torn_bytes = fs::read(&tail).unwrap();

    let report = repair(&dir, META, WalOptions::default()).unwrap();
    assert!(report.truncated);
    assert_eq!(report.entries, 1);
    assert_eq!(report.last_seq, 1);

    // The backup preserves the PRE-repair (torn) bytes (FMT-10).
    let backed = fs::read(report.backup.join(tail.file_name().unwrap())).unwrap();
    assert_eq!(backed, torn_bytes, "backup must be the untouched pre-repair image");
    assert!(report.backup.join("LOCK").exists(), "every file is backed up");

    // And the live dir now reopens clean.
    let rep2 = verify(&dir, META).unwrap();
    assert!(!rep2.torn_tail);
    assert_eq!(rep2.entries, 1);
}

#[test]
fn repair_refuses_fatal_corruption_without_touching_anything() {
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
    bytes[first_end as usize - 20] ^= 0xFF;
    fs::write(&tail, &bytes).unwrap();
    let before = fs::read(&tail).unwrap();

    match repair(&dir, META, WalOptions::default()) {
        Err(WalError::CrcMismatch { .. }) => {}
        other => panic!("expected CrcMismatch, got {other:?}", other = other.map(|r| r.entries)),
    }
    assert_eq!(fs::read(&tail).unwrap(), before, "fatal corruption: bytes untouched");
    assert!(!dir.parent().unwrap().join("wal.walbak").exists(), "no backup for a refused repair");
}

#[test]
fn repair_never_clobbers_an_existing_backup() {
    let td = tmpdir();
    let dir = td.path().join("wal");
    let mut wal = Wal::create(&dir, META, WalOptions::default()).unwrap();
    wal.append(b"x").unwrap();
    wal.sync().unwrap();
    drop(wal);

    fs::create_dir(td.path().join("wal.walbak")).unwrap();
    match repair(&dir, META, WalOptions::default()) {
        Err(WalError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists),
        other => panic!("expected AlreadyExists, got {other:?}", other = other.map(|r| r.entries)),
    }
}
