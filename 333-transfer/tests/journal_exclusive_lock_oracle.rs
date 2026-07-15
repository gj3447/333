//! Oracle: two live holders of one journal each keep their own lock table, so
//! both sign for the same slot.
//!
//! `Authority.locked` is per-process and is only ever read back at `recover`.
//! Nothing stops two `FileJournal::open` calls on one path from succeeding, and
//! nothing makes one holder see the other's appends. Each therefore believes the
//! slot is free and votes — the crash-restart equivocation, without the crash.
//!
//! This is not exotic operations. `systemd Restart=always` firing while the old
//! process still drains its poll loop, a k8s rolling update overlapping old and
//! new pods on one PVC, or an operator starting the node twice all produce it.
//!
//! The log afterwards holds two `Locked` records for one slot — durable,
//! self-authenticating proof this authority broke certificate uniqueness — and
//! `recover`'s fold silently keeps the last one.
//!
//! Found by two independent adversarial agents (2026-07-15); reproduced here.

use transfer333::{
    Authority, AuthorityError, Committee, FileJournal, JournalRecord, Journal, Ledger, NetworkId,
    OwnerRegistry, SignedTransfer, SigningKey, Transfer, TransferPolicy,
};

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
    p.push(format!("transfer333-excl-{}-{tag}.log", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

/// A second holder must be refused while the first is alive.
#[test]
fn a_second_holder_of_one_journal_is_refused() {
    let path = temp_log("second-holder");
    let _first = FileJournal::open(&path).expect("first holder opens");

    let second = FileJournal::open(&path);
    let refused = second.is_err();
    drop(_first);
    let _ = std::fs::remove_file(&path);

    assert!(
        refused,
        "a journal held by a live process must not be opened again: each holder \
         keeps an independent in-memory lock table, so both would vote for the \
         same slot"
    );
}

/// The consequence, spelled out: two holders, one slot, two signatures.
#[test]
fn two_holders_of_one_journal_both_sign_the_same_slot() {
    let (p, c, g) = (policy(), committee(), genesis());
    let path = temp_log("double-sign");

    let a_order = t("bob", 0, 30);
    let b_order = t("carol", 0, 30);
    assert_ne!(a_order, b_order, "the two orders contend for slot (alice, 0)");

    // Two processes, one path. Neither open is refused today.
    let ja = FileJournal::open(&path).expect("holder A");
    let jb = match FileJournal::open(&path) {
        Ok(j) => j,
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            return; // Refused — the exclusion this file argues for is in place.
        }
    };

    let mut a = Authority::recover("a0", key(0), p.clone(), c.id(), g.clone(), ja)
        .expect("A recovers");
    let mut b = Authority::recover("a0", key(0), p.clone(), c.id(), g.clone(), jb)
        .expect("B recovers");

    let va = a.handle(&a_order);
    let vb = b.handle(&b_order);

    // Read the log back through a third handle before anyone drops.
    let both_signed = va.is_ok() && vb.is_ok();

    drop(a);
    drop(b);

    let durable: Vec<u64> = {
        let j = FileJournal::open(&path).expect("read back");
        j.replay()
            .expect("replay")
            .iter()
            .filter(|r| matches!(r, JournalRecord::Locked(_)))
            .map(|_| 0u64)
            .collect()
    };
    let _ = std::fs::remove_file(&path);

    assert!(
        !both_signed,
        "EQUIVOCATION WITHOUT A CRASH: two holders of one journal each saw an \
         empty lock table and both signed for (alice, 0). va={va:?} vb={vb:?}; \
         the log now holds {} Locked records for that slot, which is durable \
         proof the invariant broke.",
        durable.len()
    );
    let _ = AuthorityError::Poisoned; // keep the import honest
}

/// The refusal must be observable, not silent — a `return` on `Ok` would make
/// `two_holders_of_one_journal_both_sign_the_same_slot` vacuously green.
#[test]
fn the_refusal_names_the_live_holder() {
    let path = temp_log("names-holder");
    let _first = FileJournal::open(&path).expect("first holder");
    let err = FileJournal::open(&path).expect_err("second holder must be refused");
    let msg = format!("{err}");
    drop(_first);
    let _ = std::fs::remove_file(&path);
    assert!(
        msg.contains("already held by a live process"),
        "the error must say why, so an operator does not read it as corruption: {msg}"
    );
}

/// Compaction must not drop the exclusion.
///
/// `compact` replaces the file by rename and reopens for appends. The lock lives
/// on the *inode*, so the old one dies with the unlinked file — if the reopen
/// does not retake it, the journal is unheld from that moment and a second
/// process walks in.
#[test]
fn compaction_keeps_the_exclusive_lock() {
    use transfer333::{Certificate, SnapshotData};
    let (p, c, g) = (policy(), committee(), genesis());
    let path = temp_log("compact-lock");

    let journal = FileJournal::open(&path).expect("holder A");
    let mut a = Authority::with_journal("a0", key(0), p.clone(), c.id(), g.clone(), journal);

    // One confirmed transfer, so there is state worth snapshotting.
    let order = t("bob", 0, 30);
    let vote = a.handle(&order).expect("vote");
    let cert = Certificate::assemble(order, vec![vote], &c).expect("assemble");
    let verified = cert.verify(&c).expect("verify");
    a.confirm(&verified, &c).expect("confirm");
    a.compact_journal().expect("compact");

    // A is still alive and still holds the journal.
    let second = FileJournal::open(&path);
    let refused = second.is_err();
    drop(a);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("compact-tmp"));

    assert!(
        refused,
        "compaction renamed a new inode over the log and reopened it without \
         retaking the lock, so the live holder silently stopped holding it"
    );
    let _ = std::mem::size_of::<SnapshotData>();
}
