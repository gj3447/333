// KG: transfer333-wal-journal-durable-locks-2026-07-15 (residual: log grows forever)
//
// Compaction must bound the log without weakening anything the log was protecting.
//
// The two things that can go wrong, and are tested here:
//
//  1. DOUBLE APPLY. A snapshot already contains the effect of the records it
//     replaces. If recovery ever sees both, every transfer in the window applies
//     twice and supply inflates. This is why the snapshot is a frame inside the
//     journal and compaction swaps the file by rename, rather than writing a
//     sidecar and truncating.
//  2. LOST PROMISE. A slot that is locked-but-unconfirmed is this authority's
//     irrevocable promise not to sign anything else for it. Dropping such a lock
//     during compaction silently reopens the equivocation window that the whole
//     journal exists to close — the P0 would come back through the GC path.

use transfer333::{
    Authority, AuthorityError, Certificate, Committee, FileJournal, Journal, Ledger, MemJournal,
    NetworkId, OwnerRegistry, SignedTransfer, SigningKey, Transfer, TransferPolicy,
};

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn policy() -> TransferPolicy {
    TransferPolicy::new(
        NetworkId::new("journal-compaction-testnet").unwrap(),
        OwnerRegistry::new([
            ("alice", key(42).verifying_key()),
            ("bob", key(43).verifying_key()),
            ("carol", key(44).verifying_key()),
        ])
        .unwrap(),
    )
}

fn committee() -> Committee {
    Committee::new([("a0", key(0).verifying_key())], policy()).unwrap()
}

fn genesis() -> Ledger {
    Ledger::genesis([
        ("alice".to_string(), 100),
        ("bob".to_string(), 0),
        ("carol".to_string(), 0),
    ])
}

fn t(to: &str, seq: u64, amount: u128) -> SignedTransfer {
    SignedTransfer::sign(
        &policy(),
        Transfer {
            from: "alice".into(),
            from_seq: seq,
            to: to.into(),
            amount,
        },
        &key(42),
    )
}

fn temp_log(tag: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("transfer333-compact-{}-{tag}.log", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

/// Confirm `order` through the single-authority committee.
fn confirm(auth: &mut Authority, c: &Committee, order: &SignedTransfer) {
    let vote = auth.handle(order).expect("vote");
    let cert = Certificate::assemble(order.clone(), vec![vote], c).expect("assemble");
    let verified = cert.verify(c).expect("verify");
    auth.confirm(&verified, c).expect("confirm");
}

/// The point of the exercise: the log stops growing.
#[test]
fn compaction_shrinks_the_log() {
    let (p, c, g) = (policy(), committee(), genesis());
    let path = temp_log("shrinks");

    let journal = FileJournal::open(&path).expect("open");
    let mut auth = Authority::with_journal("a0", key(0), p, c.id(), g, journal);
    for seq in 0..5u64 {
        confirm(&mut auth, &c, &t("bob", seq, 3));
    }
    let before = std::fs::metadata(&path).expect("stat").len();

    auth.compact_journal().expect("compact");
    let after = std::fs::metadata(&path).expect("stat").len();

    assert!(
        after < before,
        "compaction must shrink the log: {before} -> {after}"
    );
    let _ = std::fs::remove_file(&path);
}

/// Supply is the invariant a double-apply would break, so assert on supply.
#[test]
fn recover_after_compaction_does_not_double_apply() {
    let (p, c, g) = (policy(), committee(), genesis());
    let path = temp_log("no-double");

    {
        let journal = FileJournal::open(&path).expect("open");
        let mut auth =
            Authority::with_journal("a0", key(0), p.clone(), c.id(), g.clone(), journal);
        for seq in 0..3u64 {
            confirm(&mut auth, &c, &t("bob", seq, 10));
        }
        assert_eq!(auth.ledger().balance(&"bob".to_string()), 30);
        auth.compact_journal().expect("compact");
    }

    let journal = FileJournal::open(&path).expect("reopen");
    let auth = Authority::recover("a0", key(0), p, c.id(), g, journal).expect("recover");

    assert_eq!(
        auth.ledger().balance(&"bob".to_string()),
        30,
        "bob must not be credited twice"
    );
    assert_eq!(auth.ledger().balance(&"alice".to_string()), 70);
    assert_eq!(
        auth.ledger().total_supply(),
        100,
        "a double apply would inflate supply — this is the assertion that matters"
    );
    let _ = std::fs::remove_file(&path);
}

/// Records appended *after* a snapshot must still apply on top of it.
#[test]
fn records_after_a_snapshot_still_apply() {
    let (p, c, g) = (policy(), committee(), genesis());
    let path = temp_log("after-snap");

    {
        let journal = FileJournal::open(&path).expect("open");
        let mut auth =
            Authority::with_journal("a0", key(0), p.clone(), c.id(), g.clone(), journal);
        confirm(&mut auth, &c, &t("bob", 0, 10));
        auth.compact_journal().expect("compact");
        // Post-compaction activity lands in the fresh log, after the snapshot.
        confirm(&mut auth, &c, &t("carol", 1, 5));
        assert_eq!(auth.ledger().balance(&"carol".to_string()), 5);
    }

    let journal = FileJournal::open(&path).expect("reopen");
    let auth = Authority::recover("a0", key(0), p, c.id(), g, journal).expect("recover");

    assert_eq!(auth.ledger().balance(&"bob".to_string()), 10, "from snapshot");
    assert_eq!(
        auth.ledger().balance(&"carol".to_string()),
        5,
        "from the record appended after the snapshot"
    );
    assert_eq!(auth.ledger().balance(&"alice".to_string()), 85);
    assert_eq!(auth.ledger().total_supply(), 100);
    let _ = std::fs::remove_file(&path);
}

/// THE safety test: compaction must not GC a live promise.
#[test]
fn compaction_preserves_an_unconfirmed_lock() {
    let (p, c, g) = (policy(), committee(), genesis());
    let path = temp_log("keeps-lock");
    let a = t("bob", 0, 30);
    let b = t("carol", 0, 30); // same slot, different order

    {
        let journal = FileJournal::open(&path).expect("open");
        let mut auth =
            Authority::with_journal("a0", key(0), p.clone(), c.id(), g.clone(), journal);
        auth.handle(&a).expect("vote — slot 0 is now promised");
        // Compact while the slot is locked but NOT confirmed.
        auth.compact_journal().expect("compact");
    }

    let journal = FileJournal::open(&path).expect("reopen");
    let mut auth = Authority::recover("a0", key(0), p, c.id(), g, journal).expect("recover");

    assert!(
        matches!(auth.handle(&b), Err(AuthorityError::Equivocation { .. })),
        "compaction must not discard a locked-but-unconfirmed slot: the vote for \
         `a` already left this process and cannot be retracted"
    );
    assert!(auth.handle(&a).is_ok(), "same order still re-votes");
    let _ = std::fs::remove_file(&path);
}

/// A confirmed slot's idempotence survives compaction: a re-delivered certificate
/// must still report AlreadyApplied rather than a stale-sequence error. Gossip
/// re-delivers certificates routinely, so this is the common path, not an edge.
#[test]
fn compaction_preserves_confirmed_idempotence() {
    let (p, c, g) = (policy(), committee(), genesis());
    let path = temp_log("idempotence");
    let a = t("bob", 0, 10);

    let journal = FileJournal::open(&path).expect("open");
    let mut auth = Authority::with_journal("a0", key(0), p, c.id(), g, journal);
    let vote = auth.handle(&a).expect("vote");
    let cert = Certificate::assemble(a.clone(), vec![vote], &c).expect("assemble");
    let verified = cert.verify(&c).expect("verify");
    auth.confirm(&verified, &c).expect("first confirm");

    auth.compact_journal().expect("compact");

    assert_eq!(
        auth.confirm(&verified, &c).expect("re-delivered cert"),
        transfer333::ConfirmOutcome::AlreadyApplied,
        "the confirmed slot must survive compaction as AlreadyApplied, not become \
         a stale-sequence error"
    );
    let _ = std::fs::remove_file(&path);
}

/// Snapshot codec round-trip, including the awkward shapes.
#[test]
fn snapshot_codec_roundtrips() {
    use transfer333::journal::{decode_snapshot, encode_snapshot};
    use transfer333::SnapshotData;

    let sn = SnapshotData {
        accounts: vec![
            ("alice".to_string(), u128::MAX, u64::MAX),
            ("".to_string(), 0, 0), // empty id, zero balance
            ("가나다".to_string(), 7, 3), // non-ascii
        ],
        locked: vec![t("bob", 0, 30)],
        confirmed: vec![("bob".to_string(), 9, [0xAB; 32])],
    };
    let bytes = encode_snapshot(&sn);
    assert_eq!(decode_snapshot(&bytes).expect("roundtrip"), sn);

    // Trailing garbage must not be silently ignored.
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(
        decode_snapshot(&extra).is_err(),
        "trailing bytes mean the frame is not what we wrote"
    );
    // Truncation must fail closed, not produce a partial snapshot.
    assert!(decode_snapshot(&bytes[..bytes.len() - 1]).is_err());
}

/// A snapshot frame is CRC-covered like any other frame.
#[test]
fn corrupt_snapshot_frame_fails_closed() {
    let (p, c, g) = (policy(), committee(), genesis());
    let path = temp_log("corrupt-snap");

    {
        let journal = FileJournal::open(&path).expect("open");
        let mut auth =
            Authority::with_journal("a0", key(0), p.clone(), c.id(), g.clone(), journal);
        confirm(&mut auth, &c, &t("bob", 0, 10));
        auth.compact_journal().expect("compact");
    }

    // Flip a byte deep inside the snapshot body (past magic + frame header).
    let mut bytes = std::fs::read(&path).expect("read");
    let victim = bytes.len() - 1;
    bytes[victim] ^= 0xFF;
    std::fs::write(&path, &bytes).expect("write");

    let journal = FileJournal::open(&path).expect("reopen");
    let got = Authority::recover("a0", key(0), p, c.id(), g, journal);
    assert!(
        got.is_err(),
        "a complete-but-corrupt snapshot is corruption, not a tear: it must fail \
         closed rather than silently restore wrong balances"
    );
    let _ = std::fs::remove_file(&path);
}

/// MemJournal implements the same contract, so the recover path can be exercised
/// without a filesystem.
#[test]
fn mem_journal_compaction_matches() {
    let (p, c, g) = (policy(), committee(), genesis());
    let mut journal = MemJournal::new();
    let a = t("bob", 0, 30);
    journal
        .append(&transfer333::JournalRecord::Locked(a.clone()))
        .expect("append");
    assert_eq!(journal.len(), 1);

    let snapshot = transfer333::SnapshotData {
        accounts: vec![
            ("alice".to_string(), 100, 0),
            ("bob".to_string(), 0, 0),
            ("carol".to_string(), 0, 0),
        ],
        locked: vec![a.clone()],
        confirmed: vec![],
    };
    journal.compact(&snapshot).expect("compact");
    assert_eq!(journal.len(), 1, "compaction replaces the log with one frame");

    let mut auth = Authority::recover("a0", key(0), p, c.id(), g, journal).expect("recover");
    assert!(
        matches!(
            auth.handle(&t("carol", 0, 30)),
            Err(AuthorityError::Equivocation { .. })
        ),
        "the lock carried by the snapshot must still protect the slot"
    );
}

/// Characterization: compaction cuts the constant, it does **not** bound growth.
///
/// Measured 2026-07-15 (see printed output): the log is ~440 B per confirmed
/// transfer, the compacted file ~49 B. An ~8x cut, but both are O(transfers).
///
/// The residual is `SnapshotData::confirmed`: one `(account, seq, order_id)` per
/// slot ever applied, kept so a re-delivered certificate reports `AlreadyApplied`
/// instead of a stale-sequence error.
///
/// Bounding it is a *semantic* decision, not a coding one, which is why this test
/// pins the honest status quo rather than papering over it. Certificate
/// uniqueness means at most one valid certificate can exist per slot, so
/// `seq < next_seq` would already imply "this is the one we applied" and the map
/// could collapse to O(accounts). But that reasoning *assumes* the security
/// model: today a conflicting certificate for a spent slot is caught and
/// reported, and the collapse would silently call it `AlreadyApplied`. Trading
/// that detector for memory is a call to make deliberately.
///
/// If a future change actually bounds the snapshot, this test SHOULD fail. That
/// is its job.
#[test]
fn compaction_cuts_the_constant_but_does_not_bound_growth() {
    let (p, c, g) = (policy(), committee(), genesis());
    let mut sizes = Vec::new();
    for n in [4u64, 16, 40] {
        let path = temp_log(&format!("scale-{n}"));
        let journal = FileJournal::open(&path).expect("open");
        let mut auth =
            Authority::with_journal("a0", key(0), p.clone(), c.id(), g.clone(), journal);
        for seq in 0..n {
            confirm(&mut auth, &c, &t("bob", seq, 1));
        }
        let before = std::fs::metadata(&path).expect("stat").len();
        auth.compact_journal().expect("compact");
        let after = std::fs::metadata(&path).expect("stat").len();
        println!(
            "n={n:3}  log={before:6}B ({:.0}B/tx)  compacted={after:5}B ({:.0}B/tx)",
            before as f64 / n as f64,
            after as f64 / n as f64
        );
        sizes.push((n, before, after));
        let _ = std::fs::remove_file(&path);
    }
    let (n_small, log_small, snap_small) = sizes[0];
    let (n_big, log_big, snap_big) = sizes[2];

    // What compaction DOES buy: a large constant-factor cut.
    assert!(
        snap_big * 5 < log_big,
        "compaction must cut the log by a wide margin: {log_big} -> {snap_big}"
    );

    // What it does NOT buy, asserted so nobody reads the above as "bounded".
    assert!(
        snap_big > snap_small,
        "documented residual: the snapshot is O(confirmed slots), so it still \
         grows with N ({n_small} -> {n_big} gave {snap_small} -> {snap_big}). If \
         this now fails, growth was actually bounded — update this test and the \
         residual note in journal.rs."
    );
}
