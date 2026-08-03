// KG: committee-reconfiguration design v1 (docs/design/2026-08-03-committee-reconfiguration.md)
// M2 — authority epoch FSM integration tests (fence → vote → install).
//
// Everything here drives the *public* API only: fence / sign_epoch_vote /
// install_epoch_cert / handle / confirm / recover / compact_journal. The
// two-phase protocol is proven transition by transition: fence blocks votes
// but never confirm, install releases locks and switches the trust root, a
// straggler withholds votes until its frontier covers the committed one,
// conflicting valid certs fail-stop, and the whole change survives a restart
// through the journal.

use ed25519_dalek::Signer;
use transfer333::{
    frontier_digest, Authority, AuthorityError, Certificate, Committee, EpochCert, EpochError,
    EpochProposal, EpochVote, FenceOutcome, FileJournal, InstallOutcome, Ledger, NetworkId,
    OwnerRegistry, SigningKey, Transfer, TransferPolicy, VerifyingKey,
};

fn key(i: u8) -> SigningKey {
    SigningKey::from_bytes(&[i; 32])
}

fn policy() -> TransferPolicy {
    TransferPolicy::new(
        NetworkId::new("epoch-testnet").unwrap(),
        OwnerRegistry::new([
            ("alice", key(42).verifying_key()),
            ("bob", key(43).verifying_key()),
            ("carol", key(44).verifying_key()),
        ])
        .unwrap(),
    )
}

fn committee() -> Committee {
    Committee::new(
        (0..4u8).map(|i| (format!("a{i}"), key(i).verifying_key())),
        policy(),
    )
    .unwrap()
}

fn genesis() -> Ledger {
    Ledger::genesis([
        ("alice".to_string(), 100),
        ("bob".to_string(), 0),
        ("carol".to_string(), 0),
    ])
}

fn authority(id: &str, seed: u8, committee: &Committee) -> Authority {
    Authority::new(id, key(seed), policy(), committee.id(), genesis())
}

fn order(from: &str, seq: u64, to: &str, amount: u128) -> transfer333::SignedTransfer {
    transfer333::SignedTransfer::sign(
        &policy(),
        Transfer {
            from: from.into(),
            from_seq: seq,
            to: to.into(),
            amount,
        },
        &key(42),
    )
}

/// Next roster: a3 retired, b0 (key 100) joins.
fn next_roster() -> Vec<(String, VerifyingKey)> {
    (0..3u8)
        .map(|i| (format!("a{i}"), key(i).verifying_key()))
        .chain(std::iter::once(("b0".to_string(), key(100).verifying_key())))
        .collect()
}

fn next_committee(epoch: u64) -> Committee {
    Committee::with_epoch(next_roster(), policy(), epoch).unwrap()
}

fn proposal(epoch: u64) -> EpochProposal {
    EpochProposal {
        network_id: NetworkId::new("epoch-testnet").unwrap(),
        policy_id: policy().id(),
        epoch,
        next_roster: next_roster(),
    }
}

fn epoch_cert_for(
    committee: &Committee,
    epoch: u64,
    frontier: &[(String, u64)],
    signers: &[(String, u8)],
) -> EpochCert {
    let next = next_committee(epoch);
    let votes = signers
        .iter()
        .map(|(id, seed)| {
            EpochVote::sign(
                id.clone(),
                committee.id(),
                epoch,
                next.id(),
                frontier,
                &key(*seed),
            )
        })
        .collect();
    EpochCert {
        epoch,
        next_roster: next_roster(),
        frontier: frontier.to_vec(),
        votes,
    }
}

fn signers_012() -> Vec<(String, u8)> {
    (0..3u8).map(|i| (format!("a{i}"), i)).collect()
}

fn temp_log(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("t333-epoch-{tag}-{}-{nanos}.wal", std::process::id()))
}

#[test]
fn fence_blocks_user_votes_but_never_confirm() {
    let c = committee();
    let mut a0 = authority("a0", 0, &c);
    let mut rest: Vec<Authority> = (1..4u8).map(|i| authority(&format!("a{i}"), i, &c)).collect();

    let outcome = a0.fence(proposal(1)).unwrap();
    assert!(matches!(outcome, FenceOutcome::Fenced { .. }));

    // User votes are withheld under the fence...
    assert!(matches!(
        a0.handle(&order("alice", 0, "bob", 30)),
        Err(AuthorityError::EpochFencing { epoch: 0 })
    ));

    // ...but confirmation is NOT gated: a pre-fence certificate still applies.
    let o = order("alice", 0, "bob", 30);
    let votes: Vec<_> = rest.iter_mut().map(|a| a.handle(&o).unwrap()).collect();
    let cert = Certificate::assemble(o, votes, &c).unwrap();
    let verified = cert.verify(&c).unwrap();
    assert!(a0.confirm(&verified, &c).is_ok());
    assert_eq!(a0.ledger().balance(&"bob".to_string()), 30);
}

#[test]
fn fence_ignores_stale_epoch_and_conflicting_proposals() {
    let c = committee();
    let mut a0 = authority("a0", 0, &c);

    // epoch 2 while operating epoch 0: ignored, never fenced.
    assert_eq!(
        a0.fence(proposal(2)).unwrap(),
        FenceOutcome::IgnoredStaleEpoch { expected: 1, got: 2 }
    );
    // Not fenced: user votes still work.
    assert!(a0.handle(&order("alice", 0, "bob", 1)).is_ok());

    assert!(matches!(a0.fence(proposal(1)).unwrap(), FenceOutcome::Fenced { .. }));
    // Idempotent for the identical proposal.
    assert_eq!(a0.fence(proposal(1)).unwrap(), FenceOutcome::AlreadyFenced);
    // A different proposal for the same epoch is ignored (v1: one operator proposal).
    let other = EpochProposal {
        network_id: NetworkId::new("epoch-testnet").unwrap(),
        policy_id: policy().id(),
        epoch: 1,
        next_roster: (0..4u8)
            .map(|i| (format!("c{i}"), key(150 + i).verifying_key()))
            .collect(),
    };
    assert_eq!(
        a0.fence(other).unwrap(),
        FenceOutcome::IgnoredConflictingProposal
    );
}

#[test]
fn sign_epoch_vote_binds_current_frontier() {
    let c = committee();
    let mut a0 = authority("a0", 0, &c);

    // NotFencing outside the Fencing state.
    assert_eq!(a0.sign_epoch_vote().unwrap_err(), EpochError::NotFencing);

    a0.handle(&order("alice", 0, "bob", 30)).unwrap();
    a0.fence(proposal(1)).unwrap();
    let vote = a0.sign_epoch_vote().unwrap();

    assert_eq!(vote.epoch, 1);
    assert_eq!(vote.committee_id, c.id());
    assert_eq!(vote.frontier_digest, frontier_digest(&a0.frontier()));
    assert!(vote.verify_signature(&key(0).verifying_key()).is_ok());
}

#[test]
fn install_epoch_cert_full_flow_switches_trust_root_and_releases_locks() {
    let c = committee();
    let mut auths: Vec<Authority> = (0..4u8).map(|i| authority(&format!("a{i}"), i, &c)).collect();

    // Everyone converges on alice:0 → bob:30.
    let o = order("alice", 0, "bob", 30);
    let votes: Vec<_> = auths[..3].iter_mut().map(|a| a.handle(&o).unwrap()).collect();
    let cert = Certificate::assemble(o.clone(), votes, &c).unwrap();
    let verified = cert.verify(&c).unwrap();
    for a in auths.iter_mut() {
        a.confirm(&verified, &c).unwrap();
    }

    let frontier = auths[0].frontier();
    let ecert = epoch_cert_for(&c, 1, &frontier, &signers_012());

    assert_eq!(
        auths[3].install_epoch_cert(&ecert, &c).unwrap(),
        InstallOutcome::Installed
    );
    assert_eq!(auths[3].epoch(), 1);
    // Locks from epoch 0 were released; the new epoch votes immediately.
    assert!(auths[3].handle(&order("alice", 1, "bob", 1)).is_ok());
}

#[test]
fn install_straggler_withholds_votes_until_frontier_covers() {
    let c = committee();
    let mut converged: Vec<Authority> = (0..3u8).map(|i| authority(&format!("a{i}"), i, &c)).collect();
    let mut straggler = authority("a3", 3, &c);

    // The converged three apply the cert; the straggler never sees it.
    let o = order("alice", 0, "bob", 30);
    let votes: Vec<_> = converged.iter_mut().map(|a| a.handle(&o).unwrap()).collect();
    let cert = Certificate::assemble(o.clone(), votes, &c).unwrap();
    let verified = cert.verify(&c).unwrap();
    for a in converged.iter_mut() {
        a.confirm(&verified, &c).unwrap();
    }

    let frontier = converged[0].frontier(); // alice next_seq = 1
    let ecert = epoch_cert_for(&c, 1, &frontier, &signers_012());

    // Straggler installs directly from Active — its ledger is behind.
    assert_eq!(
        straggler.install_epoch_cert(&ecert, &c).unwrap(),
        InstallOutcome::Installing
    );
    assert_eq!(straggler.epoch(), 0, "epoch advances only at coverage");
    // Idempotent for the same change mid-install.
    assert_eq!(
        straggler.install_epoch_cert(&ecert, &c).unwrap(),
        InstallOutcome::AlreadyInstalled
    );
    assert!(matches!(
        straggler.handle(&order("alice", 1, "bob", 1)),
        Err(AuthorityError::EpochCatchingUp { .. })
    ));

    // Anti-entropy delivers the missing certificate; confirm drives coverage.
    straggler.confirm(&verified, &c).unwrap();
    assert_eq!(straggler.epoch(), 1, "coverage completes the install");
    assert!(straggler.handle(&order("alice", 1, "bob", 1)).is_ok());
}

#[test]
fn install_rejects_invalid_certs() {
    let c = committee();
    let mut a0 = authority("a0", 0, &c);
    let frontier = a0.frontier();

    // Stale epoch.
    assert_eq!(
        a0.install_epoch_cert(&epoch_cert_for(&c, 2, &frontier, &signers_012()), &c)
            .unwrap(),
        InstallOutcome::IgnoredStaleEpoch { expected: 1, got: 2 }
    );

    // Insufficient quorum (2 of 4, need 3).
    let two = vec![("a0".to_string(), 0u8), ("a1".to_string(), 1u8)];
    assert!(a0
        .install_epoch_cert(&epoch_cert_for(&c, 1, &frontier, &two), &c)
        .is_err());

    // Voter outside the old committee.
    let outsiders = vec![
        ("a0".to_string(), 0u8),
        ("a1".to_string(), 1u8),
        ("b9".to_string(), 99u8),
    ];
    assert!(a0
        .install_epoch_cert(&epoch_cert_for(&c, 1, &frontier, &outsiders), &c)
        .is_err());

    // Forged signatures: valid-looking votes signed by the wrong key material.
    let forged_votes: Vec<EpochVote> = (0..3u8)
        .map(|i| EpochVote {
            authority: format!("a{i}"),
            committee_id: c.id(),
            epoch: 1,
            next_committee_id: next_committee(1).id(),
            frontier_digest: frontier_digest(&frontier),
            signature: key(42).sign(&[0xEE; 32]),
        })
        .collect();
    let forged = EpochCert {
        epoch: 1,
        next_roster: next_roster(),
        frontier: frontier.clone(),
        votes: forged_votes,
    };
    assert!(a0.install_epoch_cert(&forged, &c).is_err());

    // Frontier mismatch: cert body digests differently from what votes signed.
    let mismatched = EpochCert {
        epoch: 1,
        next_roster: next_roster(),
        frontier: vec![("alice".to_string(), 99)],
        votes: epoch_cert_for(&c, 1, &frontier, &signers_012()).votes,
    };
    assert!(a0.install_epoch_cert(&mismatched, &c).is_err());

    // And after all that rejection, the authority is still fine.
    assert!(a0.handle(&order("alice", 0, "bob", 1)).is_ok());
}

#[test]
fn conflicting_valid_certs_fail_stop() {
    let c = committee();
    let mut a0 = authority("a0", 0, &c);
    // Committed frontier ahead of local → install parks in Installing, where
    // the conflict check lives.
    let ahead = vec![
        ("alice".to_string(), 1u64),
        ("bob".to_string(), 0u64),
        ("carol".to_string(), 0u64),
    ];

    let cert_a = epoch_cert_for(&c, 1, &ahead, &signers_012());
    // A valid, different change for the same epoch — simulating >= f+1
    // Byzantine double-signers (honest members never sign two changes).
    let other_roster: Vec<(String, VerifyingKey)> = (0..4u8)
        .map(|i| (format!("d{i}"), key(200 + i).verifying_key()))
        .collect();
    let other_next = Committee::with_epoch(other_roster.clone(), policy(), 1).unwrap();
    let votes_b: Vec<EpochVote> = (0..3u8)
        .map(|i| {
            EpochVote::sign(
                format!("a{i}"),
                c.id(),
                1,
                other_next.id(),
                &ahead,
                &key(i),
            )
        })
        .collect();
    let cert_b = EpochCert {
        epoch: 1,
        next_roster: other_roster,
        frontier: ahead.clone(),
        votes: votes_b,
    };

    assert_eq!(
        a0.install_epoch_cert(&cert_a, &c).unwrap(),
        InstallOutcome::Installing
    );
    assert_eq!(
        a0.install_epoch_cert(&cert_b, &c).unwrap_err(),
        EpochError::ConflictingCert
    );
    assert!(a0.is_poisoned(), "conflicting valid certs must fail-stop");
}

#[test]
fn epoch_change_survives_restart_via_journal() {
    let c = committee();

    // Crash mid-fence: the fence must be durable.
    let path = temp_log("fence");
    {
        let journal = FileJournal::open(&path).expect("open");
        let mut a0 = Authority::with_journal("a0", key(0), policy(), c.id(), genesis(), journal);
        a0.handle(&order("alice", 0, "bob", 30)).unwrap();
        a0.fence(proposal(1)).unwrap();
    }
    {
        let journal = FileJournal::open(&path).expect("reopen");
        let mut a0 = Authority::recover("a0", key(0), policy(), c.id(), genesis(), journal).unwrap();
        assert!(
            a0.sign_epoch_vote().is_ok(),
            "a recovered Fencing authority can still sign its epoch vote"
        );
        assert!(
            matches!(
                a0.handle(&order("alice", 1, "bob", 1)),
                Err(AuthorityError::EpochFencing { .. })
            ),
            "a recovered Fencing authority still withholds user votes"
        );
    }
    let _ = std::fs::remove_file(&path);

    // Crash post-install: epoch, trust root, and lock release must survive.
    let path2 = temp_log("installed");
    let frontier;
    let alice0 = order("alice", 0, "bob", 30);
    {
        let journal = FileJournal::open(&path2).expect("open");
        let mut a0 = Authority::with_journal("a0", key(0), policy(), c.id(), genesis(), journal);
        let votes: Vec<_> = (0..3u8)
            .map(|i| {
                let mut a = authority(&format!("a{i}"), i, &c);
                a.handle(&alice0).unwrap()
            })
            .collect();
        let cert = Certificate::assemble(alice0.clone(), votes, &c).unwrap();
        let verified = cert.verify(&c).unwrap();
        a0.confirm(&verified, &c).unwrap();
        frontier = a0.frontier();
        let ecert = epoch_cert_for(&c, 1, &frontier, &signers_012());
        assert_eq!(
            a0.install_epoch_cert(&ecert, &c).unwrap(),
            InstallOutcome::Installed
        );
        assert_eq!(a0.epoch(), 1);
    }
    {
        let journal = FileJournal::open(&path2).expect("reopen");
        let mut a0 = Authority::recover("a0", key(0), policy(), c.id(), genesis(), journal).unwrap();
        assert_eq!(a0.epoch(), 1, "installed epoch survives restart");
        assert_eq!(a0.frontier(), frontier, "confirmed state survives restart");
        assert!(
            a0.handle(&order("alice", 1, "bob", 1)).is_ok(),
            "new epoch votes after restart (locks stayed released)"
        );
        // Epoch-0 certificates are foreign to the switched trust root.
        let votes: Vec<_> = (0..3u8)
            .map(|i| {
                let mut a = authority(&format!("a{i}"), i, &c);
                a.handle(&alice0).unwrap()
            })
            .collect();
        let old_cert = Certificate::assemble(alice0.clone(), votes, &c).unwrap();
        let old_verified = old_cert.verify(&c).unwrap();
        assert!(
            a0.confirm(&old_verified, &c).is_err(),
            "epoch-0 certs stay foreign after the switch"
        );
    }
    let _ = std::fs::remove_file(&path2);
}

#[test]
fn compact_refused_mid_change_allowed_after() {
    let c = committee();
    let path = temp_log("compact");
    let journal = FileJournal::open(&path).expect("open");
    let mut a0 = Authority::with_journal("a0", key(0), policy(), c.id(), genesis(), journal);

    a0.fence(proposal(1)).unwrap();
    assert!(
        a0.compact_journal().is_err(),
        "compaction mid-change would erase the in-flight epoch transition"
    );

    let ecert = epoch_cert_for(&c, 1, &a0.frontier(), &signers_012());
    assert_eq!(
        a0.install_epoch_cert(&ecert, &c).unwrap(),
        InstallOutcome::Installed
    );
    assert!(a0.compact_journal().is_ok(), "Active again — compaction resumes");

    let _ = std::fs::remove_file(&path);
}
