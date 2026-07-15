//! Oracle: a crash that tears a record's *header* must be tolerated exactly as
//! a crash that tears its *body* is.
//!
//! `decode_records` already documents the contract for a torn tail:
//!
//! > "A torn tail is expected after a crash mid-append: the record never became
//! >  durable, so the decision it carried never became visible. Stop cleanly
//! >  rather than failing the whole recovery."
//!
//! That tolerance is implemented only for a torn *body* (`bytes.len() < 5 + len`
//! => `break`). A torn *header* (`bytes.len() < 5`) returns
//! `JournalError::Corrupt` instead, which propagates out of `Authority::recover`
//! and stops the node from starting.
//!
//! Both are the same physical event — a power loss while `append` was writing
//! the frame — and both leave a prefix that never became durable. The existing
//! `torn_tail_is_dropped_not_fatal` test only chops 3 bytes off the end, which
//! always lands in the tolerated (body) branch, so the header branch is
//! unexercised.
//!
//! Written as an independent adversarial check of `a088cd2`
//! ("write-ahead journal — durable slot locks close the crash-restart
//! equivocation P0"). Read-only w.r.t. the crate under test.

use transfer333::journal::{decode_records, encode_record};
use transfer333::{
    Authority, Committee, FileJournal, JournalRecord, Ledger, NetworkId, OwnerRegistry,
    SignedTransfer, SigningKey, Transfer, TransferPolicy,
};

const JOURNAL_MAGIC_LEN: usize = b"transfer333/journal/v1\0".len();
const HEADER_LEN: usize = 5; // 1 tag + 4 big-endian length

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
    p.push(format!(
        "transfer333-tornhdr-{}-{tag}.log",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&p);
    p
}

/// Unit level: the two torn-tail shapes are handled inconsistently.
///
/// Same event, same "never became durable" prefix, opposite outcomes.
#[test]
#[ignore = "RED: documents defect-333-journal-tolerates-only-one-torn-tail-shape-2026-07-15 (P2). Un-ignore with the fix."]
fn decode_torn_header_is_inconsistent_with_torn_body() {
    let frame = encode_record(&JournalRecord::Locked(t("bob", 0, 30)));
    assert!(
        frame.len() > HEADER_LEN,
        "frame must be header + body for this test to mean anything"
    );

    // Torn BODY: header complete, body short. Tolerated today.
    let torn_body = &frame[..frame.len() - 3];
    assert_eq!(
        decode_records(torn_body).expect("torn body is tolerated"),
        vec![],
        "a torn body contributes no record"
    );

    // Torn HEADER: only part of the 5-byte header landed. Same class of event.
    for k in 1..HEADER_LEN {
        let torn_header = &frame[..k];
        let got = decode_records(torn_header);
        assert!(
            got.is_ok(),
            "a {k}-byte torn header must be dropped like a torn body, not fail \
             recovery — got {got:?}"
        );
        assert_eq!(got.unwrap(), vec![], "a torn header contributes no record");
    }
}

/// The likeliest real crash artifact: a zero-filled tail.
///
/// On ext4 (`data=ordered`) a power loss can extend a file's size while the
/// data blocks never land, leaving a run of zeros past the last good record.
/// `TAG_LOCKED` is `0` and a zero length field decodes as `len = 0`, so a zero
/// tail parses as a well-formed, zero-length `Locked` frame rather than as the
/// torn tail it is. There is no per-record checksum to catch it.
#[test]
#[ignore = "RED: documents defect-333-journal-tolerates-only-one-torn-tail-shape-2026-07-15 (P2). Un-ignore with the fix."]
fn decode_tolerates_a_zero_filled_tail() {
    let mut body = encode_record(&JournalRecord::Locked(t("bob", 0, 30)));
    let good = decode_records(&body).expect("baseline decodes");
    assert_eq!(good.len(), 1, "one good record before the crash");

    // A page of zeros lands where the next frame was going.
    body.extend(std::iter::repeat(0u8).take(4096));

    let got = decode_records(&body);
    assert!(
        got.is_ok(),
        "a zero-filled tail is a torn tail, not corruption — got {got:?}"
    );
    assert_eq!(
        got.unwrap(),
        good,
        "a zero tail must contribute no record; it must not resurrect as a \
         phantom zero-length Locked frame"
    );
}

/// Integration level: the same tear, through the real API, stops the node.
///
/// This is the reachable consequence — `Authority::recover` is what
/// `node authority --journal <path>` calls at startup.
#[test]
#[ignore = "RED: documents defect-333-journal-tolerates-only-one-torn-tail-shape-2026-07-15 (P2). Un-ignore with the fix."]
fn recover_survives_a_torn_header() {
    let (p, c, g) = (policy(), committee(), genesis());
    let path = temp_log("recover");

    {
        let journal = FileJournal::open(&path).expect("open");
        let mut auth =
            Authority::with_journal("a0", key(0), p.clone(), c.id(), g.clone(), journal);
        auth.handle(&t("bob", 0, 30)).expect("vote");
    }

    // Power loss after 2 of the record's 5 header bytes reached the platter.
    let bytes = std::fs::read(&path).expect("read");
    assert!(
        bytes.len() > JOURNAL_MAGIC_LEN + HEADER_LEN,
        "log must hold magic + a full frame"
    );
    std::fs::write(&path, &bytes[..JOURNAL_MAGIC_LEN + 2]).expect("truncate");

    let journal = FileJournal::open(&path).expect("reopen");
    let recovered = Authority::recover("a0", key(0), p, c.id(), g, journal);

    let _ = std::fs::remove_file(&path);

    let auth = recovered.expect(
        "a torn header must not block recovery: the record never became durable, \
         exactly as for a torn body",
    );
    assert_eq!(
        auth.ledger().total_supply(),
        100,
        "the torn record contributed nothing"
    );
}
