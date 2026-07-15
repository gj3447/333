// A journal must be usable only by the authority, committee and genesis that
// wrote it.
//
// Both cases below were ACCEPTED before the identity block existed (measured
// 2026-07-15), and both turn an operator mistake into a correctness failure that
// recovery cannot notice:
//
//   * wrong file    — the authority adopts a peer's locks and loses its own, so
//                     it re-votes a slot it already voted on. That is Byzantine
//                     equivocation, reached by a path typo rather than a crash,
//                     and it is exactly what the journal exists to prevent.
//   * wrong genesis — replay re-drives `Ledger::apply` from whatever the caller
//                     supplied, so balances are silently minted.
//
// # KG: fix-333-journal-identity-binding-2026-07-15
// # KG: review-transfer333-journal-vs-fastpay-sui-2026-07-15 (F7)

use transfer333::{
    Authority, Committee, FileJournal, Journal, Ledger, MemJournal, NetworkId, OwnerRegistry,
    SignedTransfer, SigningKey, Transfer, TransferPolicy,
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
        ])
        .unwrap(),
    )
}

fn committee_a() -> Committee {
    Committee::new([("a0", key(0).verifying_key())], policy()).unwrap()
}

fn committee_b() -> Committee {
    Committee::new([("a1", key(1).verifying_key())], policy()).unwrap()
}

fn genesis() -> Ledger {
    Ledger::genesis([("alice".to_string(), 100), ("bob".to_string(), 0)])
}

fn t(seq: u64, amount: u128) -> SignedTransfer {
    SignedTransfer::sign(
        &policy(),
        Transfer {
            from: "alice".into(),
            from_seq: seq,
            to: "bob".into(),
            amount,
        },
        &key(42),
    )
}

fn temp_log(tag: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("transfer333-identity-{}-{tag}.log", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

/// Write one real lock into a journal at `path`, as authority a0.
fn seed_a0_journal(path: &std::path::Path) {
    let journal = FileJournal::open(path).expect("open");
    let mut a = Authority::with_journal("a0", key(0), policy(), committee_a().id(), genesis(), journal);
    a.handle(&t(0, 30)).expect("a0 votes");
    assert!(!a.is_poisoned(), "claiming a fresh journal must succeed");
}

#[test]
fn another_authority_cannot_recover_from_this_journal() {
    let path = temp_log("foreign-authority");
    seed_a0_journal(&path);

    let journal = FileJournal::open(&path).expect("reopen");
    let err = Authority::recover("a1", key(1), policy(), committee_b().id(), genesis(), journal)
        .err()
        .expect("a1 must not be able to recover from a0's journal");
    assert!(
        format!("{err}").contains("different authority"),
        "expected an identity rejection, got: {err}"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_wrong_genesis_cannot_recover_from_this_journal() {
    let path = temp_log("wrong-genesis");
    seed_a0_journal(&path);

    // Same authority, same committee, but a genesis that never wrote this log.
    // Previously this recovered and reported alice's balance as 999.
    let wrong = Ledger::genesis([("alice".to_string(), 999), ("bob".to_string(), 0)]);
    let journal = FileJournal::open(&path).expect("reopen");
    let err = Authority::recover("a0", key(0), policy(), committee_a().id(), wrong, journal)
        .err()
        .expect("a mismatched genesis must not be silently adopted");
    assert!(
        format!("{err}").contains("different authority"),
        "expected an identity rejection, got: {err}"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_rightful_owner_still_recovers() {
    // The guard must reject impostors without locking out the authority itself:
    // its own lock survives the restart and still blocks a conflicting order.
    let path = temp_log("rightful-owner");
    seed_a0_journal(&path);

    let journal = FileJournal::open(&path).expect("reopen");
    let mut a = Authority::recover("a0", key(0), policy(), committee_a().id(), genesis(), journal)
        .expect("the authority that wrote the journal must recover from it");
    assert!(!a.is_poisoned());

    assert!(
        a.handle(&t(0, 30)).is_ok(),
        "the identical order must still re-vote idempotently after recovery"
    );
    assert!(
        a.handle(&t(0, 55)).is_err(),
        "the recovered lock must still refuse a conflicting order for the slot"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_reclaimed_journal_survives_compaction() {
    // Compaction rewrites the file; if it dropped the claim, the next open would
    // see an unclaimed journal and hand it to whoever asked first.
    let path = temp_log("compaction");
    seed_a0_journal(&path);
    {
        let journal = FileJournal::open(&path).expect("reopen");
        let mut a =
            Authority::recover("a0", key(0), policy(), committee_a().id(), genesis(), journal)
                .expect("recover");
        a.compact_journal().expect("compact");
    }

    let journal = FileJournal::open(&path).expect("reopen after compaction");
    let err = Authority::recover("a1", key(1), policy(), committee_b().id(), genesis(), journal)
        .err()
        .expect("a compacted journal must still be claimed by a0");
    assert!(format!("{err}").contains("different authority"), "got: {err}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn with_journal_poisons_rather_than_writing_into_a_foreign_log() {
    // `with_journal` cannot return an error without churning every call site, so
    // a rejected claim must at least leave an authority that refuses to act.
    let path = temp_log("poison");
    seed_a0_journal(&path);

    let journal = FileJournal::open(&path).expect("reopen");
    let mut a1 =
        Authority::with_journal("a1", key(1), policy(), committee_b().id(), genesis(), journal);
    assert!(
        a1.is_poisoned(),
        "an authority handed a foreign journal must be born poisoned"
    );
    assert!(
        a1.handle(&t(0, 30)).is_err(),
        "a poisoned authority must not vote"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn mem_journal_enforces_the_same_rule() {
    // The in-memory port cannot be mis-pointed by a path typo, but it must not
    // quietly diverge from the rule the file port enforces.
    let mut j = MemJournal::new();
    assert!(j.bind(&[7u8; 32]).is_ok(), "first claim wins");
    assert!(j.bind(&[7u8; 32]).is_ok(), "re-claiming with the same identity is idempotent");
    assert!(j.bind(&[9u8; 32]).is_err(), "a different identity must be refused");
}
