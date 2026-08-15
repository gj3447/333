// KG: audit-333-fsm-vs-borg-k8s-2026-07-15 (P0: durable 0/26 → cert uniqueness 붕괴),
//     omc-open-problems-4track-2026-07-15 (Track B 판정 전제 보정)
//
// The falsifier for the durability P0.
//
// Claim under test: an honest authority that crashes and restarts must NOT sign a
// second, conflicting order for a slot it already voted on. Before `journal.rs`,
// `Authority::locked` was in-memory only, so a restart silently converted one
// honest crash into one unit of spent Byzantine budget — more than `f` restarts
// across a slot's voters break certificate uniqueness outright.
//
// Structure uses direct fault injection: the positive tests are
// paired with a run that *omits* the journal and asserts the bug reproduces. A
// green that cannot go red proves nothing.

use transfer333::{
    Authority, AuthorityError, Certificate, Committee, FileJournal, Journal, JournalRecord, Ledger,
    MemJournal, NetworkId, OwnerRegistry, SignedTransfer, SigningKey, Transfer, TransferPolicy,
};

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn policy() -> TransferPolicy {
    TransferPolicy::new(
        NetworkId::new("journal-testnet").unwrap(),
        // Recipients must be registered owners too: the policy rejects a transfer
        // to an unknown account before the slot logic is ever reached.
        OwnerRegistry::new([
            ("alice", key(42).verifying_key()),
            ("bob", key(43).verifying_key()),
            ("carol", key(44).verifying_key()),
        ])
        .unwrap(),
    )
}

/// Single-authority committee: n=1, quorum=1. The slot invariant under test is
/// per-authority, so one honest authority is the minimal witness.
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

/// Deterministic temp path: process id + call-site tag. The crate forbids
/// wall-clock, and adding a tempfile dependency for four tests is not worth it.
fn temp_log(tag: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("transfer333-journal-{}-{tag}.log", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

// ---------------------------------------------------------------------------
// NEGATIVE ORACLE — without durability the defect is real
// ---------------------------------------------------------------------------

/// Reproduces the audited P0. A fresh `Authority` (what a restart produced before
/// the journal) has an empty lock table and signs a conflicting order for a slot
/// it already voted on. If this ever fails, the defect model is wrong and the
/// positive tests below prove nothing.
#[test]
fn negative_oracle_restart_without_journal_double_signs() {
    let (p, c, g) = (policy(), committee(), genesis());
    let a = t("bob", 0, 30);
    let b = t("carol", 0, 30);
    assert_ne!(a, b, "the two orders must contend for the same slot");

    let mut before = Authority::new("a0", key(0), p.clone(), c.id(), g.clone());
    before.handle(&a).expect("first vote");
    assert!(
        matches!(before.handle(&b), Err(AuthorityError::Equivocation { .. })),
        "in-process, the lock table still protects the slot"
    );

    // "Crash": drop the process state and rebuild from genesis — exactly what the
    // pre-journal node did on restart.
    let mut after = Authority::new("a0", key(0), p, c.id(), g);
    assert!(
        after.handle(&b).is_ok(),
        "NEGATIVE ORACLE: a restart forgets the lock and signs the conflicting \
         order. This is the P0 the journal exists to close."
    );
}

// ---------------------------------------------------------------------------
// POSITIVE — recovery from a durable journal blocks it
// ---------------------------------------------------------------------------

#[test]
fn recover_from_file_journal_blocks_equivocation_after_restart() {
    let (p, c, g) = (policy(), committee(), genesis());
    let path = temp_log("blocks-equivocation");
    let a = t("bob", 0, 30);
    let b = t("carol", 0, 30);

    {
        let journal = FileJournal::open(&path).expect("open journal");
        let mut auth =
            Authority::with_journal("a0", key(0), p.clone(), c.id(), g.clone(), journal);
        auth.handle(&a).expect("first vote is durable");
    } // drop == crash

    let journal = FileJournal::open(&path).expect("reopen journal");
    let mut auth =
        Authority::recover("a0", key(0), p, c.id(), g, journal).expect("recover");

    assert!(
        matches!(auth.handle(&b), Err(AuthorityError::Equivocation { .. })),
        "a recovered authority must refuse the conflicting order"
    );
    // The promise is unchanged, so re-sending the *same* order stays valid.
    assert!(
        auth.handle(&a).is_ok(),
        "idempotent re-vote for the same order must still succeed"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn recover_replays_confirmed_ledger_state() {
    let (p, c, g) = (policy(), committee(), genesis());
    let path = temp_log("replays-ledger");
    let a = t("bob", 0, 30);

    {
        let journal = FileJournal::open(&path).expect("open");
        let mut auth =
            Authority::with_journal("a0", key(0), p.clone(), c.id(), g.clone(), journal);
        let vote = auth.handle(&a).expect("vote");
        let cert = Certificate::assemble(a.clone(), vec![vote], &c).expect("assemble");
        let verified = cert.verify(&c).expect("verify");
        auth.confirm(&verified, &c).expect("confirm");
        assert_eq!(auth.ledger().balance(&"bob".to_string()), 30);
    } // crash after a confirmed transfer

    let journal = FileJournal::open(&path).expect("reopen");
    let auth = Authority::recover("a0", key(0), p, c.id(), g, journal).expect("recover");

    assert_eq!(
        auth.ledger().balance(&"bob".to_string()),
        30,
        "a confirmed transfer must survive the restart"
    );
    assert_eq!(
        auth.ledger().balance(&"alice".to_string()),
        70,
        "the debit must survive too"
    );
    assert_eq!(
        auth.ledger().total_supply(),
        100,
        "replay must neither mint nor burn"
    );

    let _ = std::fs::remove_file(&path);
}

/// The confirmed slot must stay closed across a restart: once applied, a later
/// conflicting order for the same sequence is stale, not a fresh slot.
#[test]
fn recover_keeps_confirmed_slot_closed() {
    let (p, c, g) = (policy(), committee(), genesis());
    let path = temp_log("slot-closed");
    let a = t("bob", 0, 30);
    let b = t("carol", 0, 30);

    {
        let journal = FileJournal::open(&path).expect("open");
        let mut auth =
            Authority::with_journal("a0", key(0), p.clone(), c.id(), g.clone(), journal);
        let vote = auth.handle(&a).expect("vote");
        let cert = Certificate::assemble(a.clone(), vec![vote], &c).expect("assemble");
        let verified = cert.verify(&c).expect("verify");
        auth.confirm(&verified, &c).expect("confirm");
    }

    let journal = FileJournal::open(&path).expect("reopen");
    let mut auth = Authority::recover("a0", key(0), p, c.id(), g, journal).expect("recover");

    // seq 0 is spent; the ledger has advanced to seq 1.
    assert!(
        matches!(auth.handle(&b), Err(AuthorityError::OutOfOrder { .. })),
        "a spent sequence must not reopen after recovery"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn mem_journal_recovery_matches_file_journal() {
    let (p, c, g) = (policy(), committee(), genesis());
    let a = t("bob", 0, 30);
    let b = t("carol", 0, 30);

    // Drive the record through the port directly: MemJournal is a fixture for the
    // recover path, not a durability claim.
    let mut journal = MemJournal::new();
    journal
        .append(&JournalRecord::Locked(a.clone()))
        .expect("append");

    let mut auth = Authority::recover("a0", key(0), p, c.id(), g, journal).expect("recover");
    assert!(
        matches!(auth.handle(&b), Err(AuthorityError::Equivocation { .. })),
        "MemJournal recovery must protect the slot identically to FileJournal"
    );
    assert!(auth.handle(&a).is_ok(), "same order still re-votes");
}

#[test]
fn journal_rejects_foreign_file() {
    let path = temp_log("bad-magic");
    std::fs::write(&path, b"not a transfer333 journal").expect("write");
    assert!(
        FileJournal::open(&path).is_err(),
        "a file without the magic header must fail closed, not be silently adopted"
    );
    let _ = std::fs::remove_file(&path);
}

/// A crash mid-append leaves a torn frame. That record never became durable, so
/// the decision it carried never became externally visible: recovery must drop
/// the tail cleanly rather than refuse to start.
#[test]
fn torn_tail_is_dropped_not_fatal() {
    let (p, c, g) = (policy(), committee(), genesis());
    let path = temp_log("torn-tail");
    let a = t("bob", 0, 30);

    {
        let journal = FileJournal::open(&path).expect("open");
        let mut auth =
            Authority::with_journal("a0", key(0), p.clone(), c.id(), g.clone(), journal);
        auth.handle(&a).expect("vote");
    }
    // Simulate a partial write: chop the last few bytes off the log.
    let mut bytes = std::fs::read(&path).expect("read");
    bytes.truncate(bytes.len() - 3);
    std::fs::write(&path, &bytes).expect("truncate");

    let journal = FileJournal::open(&path).expect("reopen");
    let auth = Authority::recover("a0", key(0), p, c.id(), g, journal)
        .expect("a torn tail must not block recovery");
    assert_eq!(
        auth.ledger().total_supply(),
        100,
        "the torn record contributed nothing"
    );

    let _ = std::fs::remove_file(&path);
}
