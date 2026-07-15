//! Oracle: a *transient* journal failure punches a permanent hole in the log.
//!
//! `handle()` guards on `poisoned` (authority.rs:506) so it can never append
//! after a durability failure. `confirm()` does not, and neither does
//! `journal_append` itself (authority.rs:447-453) — it only *sets* the flag.
//!
//! So when the failure is transient, later appends succeed and the log ends up
//! with `Confirmed(seq N+1)` but no `Confirmed(seq N)`. `Authority::recover`
//! folds the log onto genesis, reaches N+1 against a ledger still at N, and
//! returns `Corrupt("replay: confirmed record ... does not apply")`. The node
//! never starts again, and there is no repair tool.
//!
//! A transient ENOSPC is not exotic here: journal.rs:30-32 states the log has
//! "no compaction, no snapshot, no segment rotation. A confirmed slot's records
//! stay in the log forever." A full disk is the design's own end state; freeing
//! space is the operator's obvious response, and that is exactly what opens the
//! hole.
//!
//! Independent adversarial check of `a088cd2`. Read-only w.r.t. the crate.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use transfer333::{
    Authority, Certificate, Committee, Journal, JournalError, JournalRecord, Ledger, NetworkId,
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

/// A journal whose Nth append fails and whose later appends succeed — a full
/// disk that the operator then makes room on, or a transient EIO/NFS blip.
#[derive(Debug, Clone)]
struct FlakyJournal {
    records: Arc<std::sync::Mutex<Vec<JournalRecord>>>,
    calls: Arc<AtomicUsize>,
    fail_on: usize,
}

impl FlakyJournal {
    fn new(fail_on: usize) -> Self {
        Self {
            records: Arc::new(std::sync::Mutex::new(Vec::new())),
            calls: Arc::new(AtomicUsize::new(0)),
            fail_on,
        }
    }
    /// A second handle onto the same log — the "restart reads what was durable".
    fn reopen(&self) -> Self {
        Self {
            records: Arc::clone(&self.records),
            calls: Arc::new(AtomicUsize::new(usize::MAX / 2)), // never fails again
            fail_on: usize::MAX,
        }
    }
}

impl Journal for FlakyJournal {
    fn append(&mut self, record: &JournalRecord) -> Result<(), JournalError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n == self.fail_on {
            return Err(JournalError::Io("ENOSPC: no space left on device".into()));
        }
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }
    fn replay(&self) -> Result<Vec<JournalRecord>, JournalError> {
        Ok(self.records.lock().unwrap().clone())
    }
}

fn certify(auth: &mut Authority, order: &SignedTransfer, c: &Committee) -> transfer333::Verified {
    let vote = auth.handle(order).expect("vote");
    let cert = Certificate::assemble(order.clone(), vec![vote], c).expect("assemble");
    cert.verify(c).expect("verify")
}

#[test]
#[ignore = "RED: documents defect-333-transient-journal-io-permanently-bricks-authority-2026-07-15 (P1). Un-ignore with the fix."]
fn transient_journal_failure_must_not_brick_the_authority() {
    let (p, c, g) = (policy(), committee(), genesis());

    // Appends in order: 0=Locked(seq0), 1=Confirmed(seq0), 2=Locked(seq1), ...
    // Fail exactly the first Confirmed. The disk is full at that instant only.
    let journal = FlakyJournal::new(1);
    let probe = journal.clone();

    let mut auth = Authority::with_journal("a0", key(0), p.clone(), c.id(), g.clone(), journal);

    // seq 0 — vote is durable, confirm's ledger.apply lands, its journal append
    // hits ENOSPC. The authority is now poisoned but the process keeps running:
    // node.rs:571 maps this to a `cert_state_rejected` event and loops on.
    let v0 = certify(&mut auth, &t("bob", 0, 30), &c);
    let r0 = auth.confirm(&v0, &c);
    assert!(r0.is_err(), "the ENOSPC append must surface");
    assert!(auth.is_poisoned(), "durability failure fail-stops the authority");
    assert_eq!(
        auth.ledger().balance(&"bob".to_string()),
        30,
        "but ledger.apply already committed, ahead of stable storage"
    );

    // Operator frees disk space. handle() stays shut (guarded at :506), but
    // confirm() is not, so the next certificate walks straight through.
    let v1_order = t("carol", 1, 20);
    let vote1 = match auth.handle(&v1_order) {
        Ok(_) => panic!("handle must stay shut once poisoned"),
        Err(e) => e,
    };
    let _ = vote1; // Poisoned, as designed.

    // The cert does not need this authority's vote to exist: any replica's
    // quorum certificate is public, self-authenticating evidence. Build it from
    // a healthy peer and present it — exactly what confirm_from_mesh does.
    let mut healthy = Authority::new("a0", key(0), p.clone(), c.id(), g.clone());
    let peer_v0 = certify(&mut healthy, &t("bob", 0, 30), &c);
    healthy.confirm(&peer_v0, &c).expect("peer confirm seq0");
    let v1 = certify(&mut healthy, &v1_order, &c);

    let r1 = auth.confirm(&v1, &c);
    assert!(
        r1.is_ok(),
        "confirm has no poisoned guard, so it accepts and journals seq1: {r1:?}"
    );

    // The log now records seq1 but never recorded seq0.
    let durable = probe.replay().expect("replay");
    let confirmed: Vec<_> = durable
        .iter()
        .filter_map(|r| match r {
            JournalRecord::Confirmed(o) => Some(o.transfer.from_seq),
            _ => None,
        })
        .collect();
    assert_eq!(
        confirmed,
        vec![1],
        "THE HOLE: seq0 is missing, seq1 is durable"
    );

    // Restart. This is the whole point of the journal.
    let recovered = Authority::recover("a0", key(0), p, c.id(), g, probe.reopen());
    assert!(
        recovered.is_ok(),
        "a transient disk-full must not permanently brick the authority — \
         recover() folds seq1 onto a genesis ledger still at seq0 and gives up: \
         {:?}",
        recovered.err()
    );
}
