//! Oracle: a torn tail is dropped on replay but never truncated on disk, so every
//! record written after it is invisible to the *next* replay — and the
//! crash-restart equivocation P0 comes back.
//!
//! `FileJournal::open` opens with `.append(true)` and validates only the magic
//! (journal.rs, `impl FileJournal`). It never scans the log and never truncates
//! it back to the last good record boundary. `decode_records` stops cleanly at a
//! tear — but `append` keeps writing at EOF, which is *behind* that tear.
//!
//! So the log becomes `magic || R1 || <tear> || R2 || R3 ...`, and every replay
//! from then on returns only `R1`. R2..Rn are durable on disk and unreachable
//! forever. Since a `Locked` record is what stops a restarted authority from
//! signing a second order for a slot, losing it is exactly the P0 the journal
//! was introduced to close (`a088cd2`).
//!
//! The tear tolerance and the append cursor disagree: one treats the tear as the
//! end of the log, the other treats EOF as the end of the log. Truncating to the
//! last good boundary on `open` reconciles them.
//!
//! Found independently by two adversarial agents; reproduced here against the
//! working tree (journal v2, per-record CRC + reserved tag — the CRC does not
//! help, because the tear is *skipped*, not mis-parsed).

use transfer333::{
    Authority, AuthorityError, Committee, FileJournal, Ledger, NetworkId, OwnerRegistry,
    SignedTransfer, SigningKey, Transfer, TransferPolicy,
};

const JOURNAL_MAGIC_LEN: usize = b"transfer333/journal/v2\0".len();

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn policy() -> TransferPolicy {
    TransferPolicy::new(
        NetworkId::new("journal-testnet").unwrap(),
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
    p.push(format!("transfer333-shadow-{}-{tag}.log", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

/// Append raw bytes, as a crashed `append` would leave them.
fn append_raw(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("reopen for tear");
    f.write_all(bytes).expect("write tear");
    f.sync_data().expect("sync tear");
}

#[test]
#[ignore = "RED: documents defect-333-journal-tear-shadows-later-records-2026-07-15 (P0). Un-ignore with the fix."]
fn a_tear_must_not_shadow_records_written_after_it() {
    let (p, c, g) = (policy(), committee(), genesis());
    let path = temp_log("shadow");

    let first = t("bob", 0, 30);
    let conflicting = t("carol", 0, 30);
    assert_ne!(first, conflicting, "the two orders contend for slot (alice, 0)");

    // --- session 1: a power loss mid-append, before any vote landed ----------
    {
        let _journal = FileJournal::open(&path).expect("open"); // writes the magic
    }
    // ext4 `data=ordered`: i_size is replayed, the data blocks never land.
    append_raw(&path, &[0u8; 64]);

    // --- session 2: recovery drops the tear, the node starts, and it votes ---
    {
        let journal = FileJournal::open(&path).expect("reopen 1");
        let mut auth = Authority::recover("a0", key(0), p.clone(), c.id(), g.clone(), journal)
            .expect("the tear is dropped, as designed");
        // `append` writes at EOF — i.e. *after* the 64 zero bytes.
        auth.handle(&first).expect("vote slot0");
        // In-process the lock is honoured, so this session is sound.
        assert!(
            matches!(
                auth.handle(&conflicting),
                Err(AuthorityError::Equivocation { .. })
            ),
            "in-process the lock table still protects the slot"
        );
    }

    // The vote IS durable — the bytes are on disk.
    let on_disk = std::fs::read(&path).expect("read");
    assert!(
        on_disk.len() > JOURNAL_MAGIC_LEN + 64,
        "the Locked record was written past the tear"
    );

    // --- session 3: replay stops at the tear and never reaches the vote ------
    let journal = FileJournal::open(&path).expect("reopen 2");
    let mut auth = Authority::recover("a0", key(0), p, c.id(), g, journal).expect("reopen recover");

    let got = auth.handle(&conflicting);
    let _ = std::fs::remove_file(&path);

    assert!(
        matches!(got, Err(AuthorityError::Equivocation { .. })),
        "REGRESSION OF THE ORIGINAL P0: the Locked record for (alice, 0) is durable \
         on disk but sits behind a torn tail, so replay never reaches it. The \
         authority forgets it voted and signs a conflicting order for the same \
         slot — exactly what a088cd2 exists to prevent. got = {got:?}"
    );
}
