// KG: SA_333_Platform
//
// Security harness for the certified rail. The critical oracle is forge-first:
// an invalid owner proof must not consume an authority slot, so the registered
// owner can still certify the same (account, sequence) immediately afterward.

use transfer333::{
    certify, Authority, AuthorityError, Certificate, CertificateError, Certified, Committee,
    Ledger, NetworkId, OwnerAuthError, OwnerRegistry, PolicyId, Reject, SignedTransfer,
    SigningKey, Transfer, TransferPolicy,
};

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn policy() -> TransferPolicy {
    TransferPolicy::new(
        NetworkId::new("owner-auth-integration-v1").unwrap(),
        OwnerRegistry::new([
            ("alice", key(42).verifying_key()),
            ("bob", key(43).verifying_key()),
            ("carol", key(44).verifying_key()),
            ("a", key(45).verifying_key()),
            ("b", key(46).verifying_key()),
            ("dave", key(47).verifying_key()),
        ])
        .unwrap(),
    )
}

fn authority_genesis() -> Ledger {
    Ledger::genesis([
        ("alice".to_string(), 100),
        ("bob".to_string(), 0),
        ("carol".to_string(), 0),
        ("a".to_string(), 100),
        ("b".to_string(), 0),
    ])
}

fn raw(from: &str, seq: u64, to: &str, amount: u128) -> Transfer {
    Transfer {
        from: from.into(),
        from_seq: seq,
        to: to.into(),
        amount,
    }
}

fn signed(
    policy: &TransferPolicy,
    owner_seed: u8,
    from: &str,
    seq: u64,
    to: &str,
    amount: u128,
) -> SignedTransfer {
    SignedTransfer::sign(policy, raw(from, seq, to, amount), &key(owner_seed))
}

fn setup() -> (TransferPolicy, Committee, Vec<Authority>) {
    setup_with_authority_key_base(0)
}

fn setup_with_authority_key_base(
    key_base: u8,
) -> (TransferPolicy, Committee, Vec<Authority>) {
    let policy = policy();
    let committee = Committee::new(
        (0..4u8).map(|i| {
            (
                format!("a{i}"),
                key(key_base.wrapping_add(i)).verifying_key(),
            )
        }),
        policy.clone(),
    )
    .unwrap();
    let authorities: Vec<_> = (0..4u8)
        .map(|i| {
            Authority::new(
                format!("a{i}"),
                key(key_base.wrapping_add(i)),
                policy.clone(),
                committee.id(),
                authority_genesis(),
            )
        })
        .collect();
    (policy, committee, authorities)
}

fn assert_owner_rejected_then_recovers(
    mutation: &SignedTransfer,
    recovery: &SignedTransfer,
) {
    let (_, committee, mut authorities) = setup();
    assert!(matches!(
        authorities[0].handle(mutation),
        Err(AuthorityError::OwnerAuth(_))
    ));
    let (verified, status) = certify(mutation, &mut authorities, &committee);
    assert!(verified.is_none());
    assert_eq!(
        status,
        Certified::Failed {
            votes: 0,
            refusals: 4,
            contested: false,
        }
    );

    let (verified, status) = certify(recovery, &mut authorities, &committee);
    assert_eq!(status, Certified::Ok);
    assert!(verified.is_some(), "rejected proof must not poison its slot");
}

#[test]
fn valid_owner_intent_certifies_and_applies() {
    let (policy, committee, mut authorities) = setup();
    let order = signed(&policy, 42, "alice", 0, "bob", 30);

    let (verified, status) = certify(&order, &mut authorities, &committee);
    assert_eq!(status, Certified::Ok);

    let mut ledger = Ledger::genesis([("alice".into(), 100), ("bob".into(), 0)]);
    ledger.apply_verified(&verified.unwrap(), &committee).unwrap();
    assert_eq!(ledger.balance(&"alice".into()), 70);
    assert_eq!(ledger.balance(&"bob".into()), 30);
    assert_eq!(ledger.next_seq(&"alice".into()), 1);
    assert_eq!(ledger.total_supply(), 100);
}

#[test]
fn foreign_key_cannot_certify_or_poison_owner_slot() {
    let (policy, committee, mut authorities) = setup();
    let forged = signed(&policy, 99, "alice", 0, "bob", 90);

    let (forged_verified, forged_status) = certify(&forged, &mut authorities, &committee);
    assert!(forged_verified.is_none());
    assert_eq!(
        forged_status,
        Certified::Failed {
            votes: 0,
            refusals: 4,
            contested: false,
        }
    );

    // This is the liveness oracle: owner verification happened before slot lock.
    let legitimate = signed(&policy, 42, "alice", 0, "bob", 30);
    let (verified, status) = certify(&legitimate, &mut authorities, &committee);
    assert_eq!(status, Certified::Ok);

    let mut ledger = Ledger::genesis([("alice".into(), 100), ("bob".into(), 0)]);
    ledger.apply_verified(&verified.unwrap(), &committee).unwrap();
    assert_eq!(ledger.balance(&"alice".into()), 70);
    assert_eq!(ledger.balance(&"bob".into()), 30);
    assert_eq!(ledger.total_supply(), 100);
}

#[test]
fn every_signed_field_and_network_are_bound_before_slot_mutation() {
    let (policy, _, _) = setup();
    let original = signed(&policy, 42, "alice", 0, "bob", 30);

    let mut from = original.clone();
    from.transfer.from = "a".into();
    assert!(matches!(
        from.verify(&policy),
        Err(OwnerAuthError::InvalidOwnerSignature { .. })
    ));
    let valid_a_zero = signed(&policy, 45, "a", 0, "b", 30);
    assert_owner_rejected_then_recovers(&from, &valid_a_zero);

    let mut seq = original.clone();
    seq.transfer.from_seq = 1;
    assert!(matches!(
        seq.verify(&policy),
        Err(OwnerAuthError::InvalidOwnerSignature { .. })
    ));
    let (_, committee, mut authorities) = setup();
    let (bad_verified, bad_status) = certify(&seq, &mut authorities, &committee);
    assert!(bad_verified.is_none());
    assert_eq!(
        bad_status,
        Certified::Failed {
            votes: 0,
            refusals: 4,
            contested: false,
        }
    );
    let (zero_verified, zero_status) = certify(&original, &mut authorities, &committee);
    assert_eq!(zero_status, Certified::Ok);
    assert!(zero_verified.is_some());
    let valid_alice_one = signed(&policy, 42, "alice", 1, "bob", 10);
    let (one_verified, one_status) =
        certify(&valid_alice_one, &mut authorities, &committee);
    assert_eq!(one_status, Certified::Ok);
    assert!(one_verified.is_some(), "mutated seq=1 must not lock seq=1");

    let mut to = original.clone();
    to.transfer.to = "carol".into();
    assert_owner_rejected_then_recovers(&to, &original);

    let mut amount = original.clone();
    amount.transfer.amount = 31;
    assert_owner_rejected_then_recovers(&amount, &original);

    let mut network = original.clone();
    network.network_id = NetworkId::new("other-deployment").unwrap();
    assert!(matches!(
        network.verify(&policy),
        Err(OwnerAuthError::WrongNetwork { .. })
    ));
    assert_owner_rejected_then_recovers(&network, &original);

    let policy_mutation = SignedTransfer::from_parts(
        original.network_id.clone(),
        PolicyId::from_bytes([99; 32]),
        original.transfer.clone(),
        original.owner_signature().clone(),
    );
    assert!(matches!(
        policy_mutation.verify(&policy),
        Err(OwnerAuthError::WrongPolicy { .. })
    ));
    assert_owner_rejected_then_recovers(&policy_mutation, &original);
}

#[test]
fn invalid_economic_orders_get_zero_votes_and_leave_seq_zero_available() {
    let policy = policy();
    let bad_orders = [
        ("overspend", signed(&policy, 42, "alice", 0, "bob", 101)),
        ("zero amount", signed(&policy, 42, "alice", 0, "bob", 0)),
    ];

    for (label, bad) in bad_orders {
        let (_, committee, mut authorities) = setup();
        let (verified, status) = certify(&bad, &mut authorities, &committee);
        assert!(verified.is_none(), "{label}");
        assert_eq!(
            status,
            Certified::Failed {
                votes: 0,
                refusals: 4,
                contested: false,
            },
            "{label}"
        );

        let legitimate = signed(&policy, 42, "alice", 0, "bob", 30);
        let (verified, status) = certify(&legitimate, &mut authorities, &committee);
        assert_eq!(status, Certified::Ok, "{label} poisoned seq=0");
        assert!(verified.is_some(), "{label} locked seq=0");
    }
}

#[test]
fn verified_from_foreign_committee_is_rejected_by_local_ledger_boundary() {
    let (policy, foreign_committee, mut foreign_authorities) =
        setup_with_authority_key_base(0);
    let (_, local_committee, _) = setup_with_authority_key_base(10);
    assert_ne!(foreign_committee.id(), local_committee.id());

    let order = signed(&policy, 42, "alice", 0, "bob", 30);
    let (verified, status) = certify(
        &order,
        &mut foreign_authorities,
        &foreign_committee,
    );
    assert_eq!(status, Certified::Ok);
    let verified = verified.expect("foreign committee minted a valid Verified");
    assert_eq!(verified.committee_id(), foreign_committee.id());

    let mut ledger = Ledger::genesis([("alice".into(), 100), ("bob".into(), 0)]);
    assert_eq!(
        ledger.apply_verified(&verified, &local_committee),
        Err(Reject::NoCertificate)
    );
    assert_eq!(ledger.balance(&"alice".into()), 100);
    assert_eq!(ledger.balance(&"bob".into()), 0);
    assert_eq!(ledger.next_seq(&"alice".into()), 0);

    ledger.apply_verified(&verified, &foreign_committee).unwrap();
    assert_eq!(ledger.balance(&"alice".into()), 70);
    assert_eq!(ledger.balance(&"bob".into()), 30);
}

#[test]
fn unknown_sender_and_recipient_are_rejected_without_votes() {
    let (policy, _, _) = setup();
    let cases = [
        (
            signed(&policy, 99, "mallory", 0, "bob", 1),
            "unknown sender",
        ),
        (
            signed(&policy, 42, "alice", 0, "mallory", 1),
            "unknown recipient",
        ),
    ];

    for (order, label) in cases {
        let (_, _, mut authorities) = setup();
        assert!(
            matches!(
                authorities[0].handle(&order),
                Err(AuthorityError::OwnerAuth(
                    OwnerAuthError::UnknownSender { .. }
                        | OwnerAuthError::UnknownRecipient { .. }
                ))
            ),
            "{label}"
        );
    }
}

#[test]
fn authority_quorum_cannot_rescue_a_bad_owner_proof() {
    let (policy, committee, mut authorities) = setup();
    let valid = signed(&policy, 42, "alice", 0, "bob", 30);
    let votes = authorities
        .iter_mut()
        .take(committee.quorum())
        .map(|authority| authority.handle(&valid).unwrap())
        .collect();

    let mut bad_owner_proof = valid;
    bad_owner_proof.transfer.amount = 31;
    let cert = Certificate {
        order: bad_owner_proof,
        votes,
    };
    assert!(matches!(
        cert.validate(&committee),
        Err(CertificateError::OwnerAuth(
            OwnerAuthError::InvalidOwnerSignature { .. }
        ))
    ));
    assert!(cert.verify(&committee).is_none());
}

#[test]
fn registered_sender_without_genesis_gets_zero_votes_without_locking_seq_zero() {
    let (policy, committee, mut authorities) = setup();

    // Dave is owner-registered but intentionally absent from authority genesis.
    let unfunded = signed(&policy, 47, "dave", 0, "bob", 1);
    let (verified, status) = certify(&unfunded, &mut authorities, &committee);
    assert!(verified.is_none());
    assert_eq!(
        status,
        Certified::Failed {
            votes: 0,
            refusals: 4,
            contested: false,
        }
    );

    // A confirmed transfer materializes Dave in each authority ledger. Dave's
    // legitimate seq=0 then proves the earlier rejection left the slot free.
    let fund_dave = signed(&policy, 42, "alice", 0, "dave", 10);
    let (funded, status) = certify(&fund_dave, &mut authorities, &committee);
    assert_eq!(status, Certified::Ok);
    assert!(funded.is_some());

    let legitimate = signed(&policy, 47, "dave", 0, "bob", 1);
    let (verified, status) = certify(&legitimate, &mut authorities, &committee);
    assert_eq!(status, Certified::Ok, "unfunded rejection poisoned Dave/seq=0");
    assert!(verified.is_some(), "unfunded rejection locked Dave/seq=0");
}
