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
//! A transient ENOSPC is not exotic here: the log grows with transfer volume and
//! compaction (added later, same day) only cuts the constant — it does not bound
//! growth. A full disk is a reachable end state; freeing space is the operator's
//! obvious response, and that is exactly what opens the hole.
//!
//! Independent adversarial check of `a088cd2`; the defect it found was real and
//! is fixed by the poisoned guard now at the head of `confirm()`. The assertions
//! below were flipped from documenting the bug to pinning the fixed contract.

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
    /// This fake models a flaky *device*, not a mis-pointed journal, so the claim
    /// always succeeds. Deliberately does not touch `calls`: binding is not an
    /// append, and counting it here would shift which append `fail_on` hits.
    /// # KG: fix-333-journal-identity-binding-2026-07-15
    fn bind(&mut self, _identity: &[u8; 32]) -> Result<(), JournalError> {
        Ok(())
    }
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
    fn compact(&mut self, snapshot: &transfer333::SnapshotData) -> Result<(), JournalError> {
        *self.records.lock().unwrap() = vec![JournalRecord::Snapshot(snapshot.clone())];
        Ok(())
    }
}

fn certify(auth: &mut Authority, order: &SignedTransfer, c: &Committee) -> transfer333::Verified {
    let vote = auth.handle(order).expect("vote");
    let cert = Certificate::assemble(order.clone(), vec![vote], c).expect("assemble");
    cert.verify(c).expect("verify")
}

#[test]
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

    // FIXED 2026-07-15: confirm() now carries the same poisoned guard as handle().
    // The authority refuses rather than journalling seq1 over the gap left by the
    // failed seq0 append.
    let r1 = auth.confirm(&v1, &c);
    assert!(
        matches!(r1, Err(transfer333::ConfirmError::Poisoned)),
        "a poisoned authority must refuse to confirm, not journal past the hole: {r1:?}"
    );

    // NO HOLE: nothing was written after the failure.
    let durable = probe.replay().expect("replay");
    let confirmed: Vec<_> = durable
        .iter()
        .filter_map(|r| match r {
            JournalRecord::Confirmed(o) => Some(o.transfer.from_seq),
            _ => None,
        })
        .collect();
    assert!(
        confirmed.is_empty(),
        "the failed seq0 append left no Confirmed record, and the guard stopped \
         seq1 from being written on top of the gap — got {confirmed:?}"
    );

    // Restart. This is the whole point of the journal.
    let recovered = Authority::recover("a0", key(0), p, c.id(), g, probe.reopen());
    let auth = recovered.expect(
        "a transient disk-full must not permanently brick the authority: the log \
         has no hole, so recovery folds cleanly",
    );
    // seq0's ledger.apply was in-memory only and correctly did not survive. The
    // certificate is public evidence and can simply be re-presented.
    assert_eq!(
        auth.ledger().balance(&"bob".to_string()),
        0,
        "the un-journalled apply must not resurrect: recovery restores the durable \
         state, and the cert can be replayed"
    );
    assert_eq!(auth.ledger().total_supply(), 100);
    assert!(!auth.is_poisoned(), "a fresh process starts clean");
}
