use ed25519_dalek::SigningKey;
use payment333::{
    account_id_from_key, Authority, CommitteeSpec, ControlCertificate, ControlOperation, Domain,
    JobResolution, JobState, PaymentError, PaymentLedger, SignatureBytes, SignedTransferOrder,
    Transfer, TransferCertificate,
};

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn digest(byte: u8) -> [u8; 32] {
    [byte; 32]
}

fn domain(network: u8) -> Domain {
    Domain {
        protocol_version: 1,
        network_id: digest(network),
        asset_id: digest(33),
        genesis_hash: digest(44),
    }
}

fn setup(
    network: u8,
    authority_count: u8,
    alice_balance: u128,
    reward_reserve: u128,
) -> (
    SigningKey,
    SigningKey,
    CommitteeSpec,
    PaymentLedger,
    Vec<Authority>,
) {
    let alice = key(100);
    let provider = key(101);
    let authority_keys: Vec<SigningKey> = (1..=authority_count).map(key).collect();
    let committee = CommitteeSpec::new(
        domain(network),
        0,
        authority_keys
            .iter()
            .map(|key| key.verifying_key().to_bytes()),
    )
    .unwrap();
    let alice_id = account_id_from_key(&alice.verifying_key());
    let provider_id = account_id_from_key(&provider.verifying_key());
    let ledger = PaymentLedger::new(
        committee.clone(),
        [(alice_id, alice_balance), (provider_id, 0)],
        reward_reserve,
    )
    .unwrap();
    let authorities = authority_keys
        .into_iter()
        .map(|key| {
            Authority::new(
                key,
                committee.clone(),
                ledger.control_height(),
                ledger.control_hash(),
            )
            .unwrap()
        })
        .collect();
    (alice, provider, committee, ledger, authorities)
}

fn order(
    ledger: &PaymentLedger,
    owner: &SigningKey,
    to: payment333::AccountId,
    amount: u128,
) -> SignedTransferOrder {
    let from = account_id_from_key(&owner.verifying_key());
    SignedTransferOrder::sign(
        ledger.current_context(),
        Transfer {
            from,
            from_seq: ledger.next_seq(&from),
            to,
            amount,
            valid_until_control_height: ledger.control_height() + 10,
        },
        owner,
    )
    .unwrap()
}

fn certify_transfer(
    order: &SignedTransferOrder,
    ledger: &PaymentLedger,
    authorities: &mut [Authority],
) -> TransferCertificate {
    let votes = authorities
        .iter_mut()
        .map(|authority| authority.handle_transfer(order, ledger).unwrap())
        .collect();
    TransferCertificate::assemble(order.clone(), votes, ledger.current_committee()).unwrap()
}

fn apply_and_confirm_transfer(
    certificate: &TransferCertificate,
    ledger: &mut PaymentLedger,
    authorities: &mut [Authority],
) {
    ledger.apply_fast(certificate).unwrap();
    for authority in authorities {
        authority.confirm_transfer(certificate, ledger).unwrap();
    }
}

fn commit_control(
    operations: Vec<ControlOperation>,
    ledger: &mut PaymentLedger,
    authorities: &mut [Authority],
) -> ControlCertificate {
    let committee = ledger.current_committee().clone();
    let block = ledger.build_control_block(operations).unwrap();
    let validated = ledger.validate_control_block(&block).unwrap();
    let votes = authorities
        .iter_mut()
        .map(|authority| authority.vote_control(&validated).unwrap())
        .collect();
    let certificate = ControlCertificate::assemble(block, votes, &committee).unwrap();
    ledger.apply_control(&certificate).unwrap();
    for authority in authorities {
        authority.confirm_control(&certificate, ledger).unwrap();
    }
    certificate
}

#[test]
fn owner_signature_and_pubkey_bound_account_are_mandatory() {
    let (alice, provider, _, ledger, mut authorities) = setup(1, 4, 100, 0);
    let legitimate = order(
        &ledger,
        &alice,
        account_id_from_key(&provider.verifying_key()),
        25,
    );

    let mut tampered_transfer = legitimate.transfer;
    tampered_transfer.amount = 80;
    let forged = SignedTransferOrder::from_untrusted_parts(
        legitimate.context,
        tampered_transfer,
        legitimate.owner_public_key,
        legitimate.owner_signature,
    );
    assert_eq!(
        authorities[0].handle_transfer(&forged, &ledger),
        Err(PaymentError::InvalidOwnerSignature)
    );

    let wrong_account = SignedTransferOrder::from_untrusted_parts(
        legitimate.context,
        Transfer {
            from: account_id_from_key(&provider.verifying_key()),
            ..legitimate.transfer
        },
        legitimate.owner_public_key,
        SignatureBytes(legitimate.owner_signature.0),
    );
    assert_eq!(
        authorities[0].handle_transfer(&wrong_account, &ledger),
        Err(PaymentError::AccountKeyMismatch)
    );
}

#[test]
fn restart_preserves_transfer_and_control_signing_locks() {
    let temp = tempfile::tempdir().unwrap();
    let authority_path = temp.path().join("authority.state");
    let alice = key(100);
    let bob = key(101);
    let authority_key = key(1);
    let committee =
        CommitteeSpec::new(domain(2), 0, [authority_key.verifying_key().to_bytes()]).unwrap();
    let alice_id = account_id_from_key(&alice.verifying_key());
    let bob_id = account_id_from_key(&bob.verifying_key());
    let ledger = PaymentLedger::new(committee.clone(), [(alice_id, 100), (bob_id, 0)], 20).unwrap();
    let first = order(&ledger, &alice, bob_id, 30);
    let conflicting = order(&ledger, &alice, ledger.escrow_account(), 30);

    {
        let mut authority = Authority::create(
            &authority_path,
            authority_key.clone(),
            committee.clone(),
            ledger.control_height(),
            ledger.control_hash(),
        )
        .unwrap();
        authority.handle_transfer(&first, &ledger).unwrap();
        assert!(matches!(
            Authority::open(&authority_path, authority_key.clone()),
            Err(PaymentError::StoreBusy)
        ));
    }
    let mut authority = Authority::open(&authority_path, authority_key.clone()).unwrap();
    assert_eq!(
        authority.handle_transfer(&conflicting, &ledger),
        Err(PaymentError::Equivocation)
    );

    // Use a fresh authority state to prove the same restart rule for the BFT
    // control height lock.
    let control_path = temp.path().join("control-authority.state");
    let mut control_authority = Authority::create(
        &control_path,
        authority_key.clone(),
        committee,
        ledger.control_height(),
        ledger.control_hash(),
    )
    .unwrap();
    let block_a = ledger
        .build_control_block(vec![ControlOperation::DistributeReward {
            reward_epoch: 1,
            allocations: vec![(bob_id, 1)],
        }])
        .unwrap();
    let block_b = ledger
        .build_control_block(vec![ControlOperation::DistributeReward {
            reward_epoch: 2,
            allocations: vec![(bob_id, 1)],
        }])
        .unwrap();
    control_authority
        .vote_control(&ledger.validate_control_block(&block_a).unwrap())
        .unwrap();
    drop(control_authority);
    let mut reopened = Authority::open(&control_path, authority_key).unwrap();
    assert_eq!(
        reopened.vote_control(&ledger.validate_control_block(&block_b).unwrap()),
        Err(PaymentError::Equivocation)
    );
}

#[test]
fn durable_ledger_reopens_without_replay_window() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("ledger.state");
    let alice = key(100);
    let bob = key(101);
    let authority_key = key(1);
    let committee =
        CommitteeSpec::new(domain(3), 0, [authority_key.verifying_key().to_bytes()]).unwrap();
    let alice_id = account_id_from_key(&alice.verifying_key());
    let bob_id = account_id_from_key(&bob.verifying_key());
    let mut ledger =
        PaymentLedger::create(&path, committee.clone(), [(alice_id, 100), (bob_id, 0)], 0).unwrap();
    let mut authority = Authority::new(
        authority_key,
        committee,
        ledger.control_height(),
        ledger.control_hash(),
    )
    .unwrap();
    let signed = order(&ledger, &alice, bob_id, 40);
    let vote = authority.handle_transfer(&signed, &ledger).unwrap();
    let cert =
        TransferCertificate::assemble(signed, vec![vote], ledger.current_committee()).unwrap();
    ledger.apply_fast(&cert).unwrap();
    drop(ledger);

    let mut reopened = PaymentLedger::open(&path).unwrap();
    assert_eq!(reopened.balance(&alice_id), 60);
    assert_eq!(reopened.balance(&bob_id), 40);
    assert_eq!(reopened.next_seq(&alice_id), 1);
    assert_eq!(
        reopened.apply_fast(&cert),
        Err(PaymentError::StaleSequence {
            expected: 1,
            got: 0
        })
    );
}

#[test]
fn network_asset_genesis_and_committee_epoch_are_signature_bound() {
    let (alice, provider, _, mut ledger_a, mut authorities) = setup(4, 4, 100, 0);
    let provider_id = account_id_from_key(&provider.verifying_key());
    let signed = order(&ledger_a, &alice, provider_id, 10);
    let cert = certify_transfer(&signed, &ledger_a, &mut authorities);

    let authority_public_keys: Vec<[u8; 32]> =
        authorities.iter().map(Authority::verifying_key).collect();
    let committee_b = CommitteeSpec::new(domain(5), 0, authority_public_keys.clone()).unwrap();
    let mut ledger_b = PaymentLedger::new(
        committee_b,
        [
            (account_id_from_key(&alice.verifying_key()), 100),
            (provider_id, 0),
        ],
        0,
    )
    .unwrap();
    assert_eq!(
        ledger_b.apply_fast(&cert),
        Err(PaymentError::InvalidTransferCertificate)
    );

    apply_and_confirm_transfer(&cert, &mut ledger_a, &mut authorities);
    let old_epoch_cert = cert.clone();
    let successor = ledger_a
        .current_committee()
        .successor(1, authority_public_keys)
        .unwrap();
    commit_control(
        vec![ControlOperation::RotateCommittee { next: successor }],
        &mut ledger_a,
        &mut authorities,
    );
    assert_eq!(ledger_a.current_context().committee_epoch, 1);
    assert_eq!(
        ledger_a.apply_fast(&old_epoch_cert),
        Err(PaymentError::InvalidTransferCertificate)
    );
}

#[test]
fn rotation_cannot_cross_an_unsettled_fastpay_lock() {
    let (alice, provider, _, ledger, mut authorities) = setup(6, 4, 100, 0);
    let signed = order(
        &ledger,
        &alice,
        account_id_from_key(&provider.verifying_key()),
        10,
    );
    let _certificate = certify_transfer(&signed, &ledger, &mut authorities);
    let keys: Vec<[u8; 32]> = authorities.iter().map(Authority::verifying_key).collect();
    let next = ledger.current_committee().successor(1, keys).unwrap();
    let block = ledger
        .build_control_block(vec![ControlOperation::RotateCommittee { next }])
        .unwrap();
    let validated = ledger.validate_control_block(&block).unwrap();
    assert_eq!(
        authorities[0].vote_control(&validated),
        Err(PaymentError::PendingTransfersAtRotation)
    );
}

#[test]
fn settled_escrow_deposit_survives_committee_rotation() {
    let (alice, _provider, _, mut ledger, mut authorities) = setup(8, 4, 100, 0);
    let alice_id = account_id_from_key(&alice.verifying_key());
    let deposit_order = order(&ledger, &alice, ledger.escrow_account(), 20);
    let deposit = certify_transfer(&deposit_order, &ledger, &mut authorities);
    apply_and_confirm_transfer(&deposit, &mut ledger, &mut authorities);

    let keys: Vec<[u8; 32]> = authorities.iter().map(Authority::verifying_key).collect();
    let next = ledger.current_committee().successor(1, keys).unwrap();
    commit_control(
        vec![ControlOperation::RotateCommittee { next }],
        &mut ledger,
        &mut authorities,
    );
    assert_eq!(ledger.current_context().committee_epoch, 1);
    assert_eq!(
        ledger.apply_fast(&deposit),
        Err(PaymentError::InvalidTransferCertificate)
    );

    let job_id = digest(88);
    commit_control(
        vec![ControlOperation::OpenJob {
            job_id,
            buyer: alice_id,
            budget: 20,
            funding: deposit,
        }],
        &mut ledger,
        &mut authorities,
    );
    assert_eq!(ledger.job(&job_id).unwrap().state, JobState::Open);
}

#[test]
fn unified_fast_and_bft_lanes_preserve_supply_and_are_idempotent() {
    let (alice, provider, _, mut ledger, mut authorities) = setup(7, 4, 100, 50);
    let alice_id = account_id_from_key(&alice.verifying_key());
    let provider_id = account_id_from_key(&provider.verifying_key());
    let initial_supply = ledger.total_supply();

    // Fast path: the buyer deposits into a protocol-owned account. The account
    // has no owner key, so only a later BFT voucher can spend it.
    let deposit_order = order(&ledger, &alice, ledger.escrow_account(), 70);
    let deposit = certify_transfer(&deposit_order, &ledger, &mut authorities);
    apply_and_confirm_transfer(&deposit, &mut ledger, &mut authorities);

    let job_id = digest(77);
    commit_control(
        vec![
            ControlOperation::OpenJob {
                job_id,
                buyer: alice_id,
                budget: 70,
                funding: deposit.clone(),
            },
            ControlOperation::PlaceBid {
                job_id,
                provider: provider_id,
                price: 40,
            },
            ControlOperation::AcceptBid {
                job_id,
                provider: provider_id,
            },
        ],
        &mut ledger,
        &mut authorities,
    );
    assert_eq!(ledger.job(&job_id).unwrap().state, JobState::Leased);

    // A naked control block has no authority to mutate shared state.
    let naked = ledger
        .build_control_block(vec![ControlOperation::OpenDispute {
            job_id,
            actor: alice_id,
        }])
        .unwrap();
    let no_votes = ControlCertificate {
        block: naked,
        votes: vec![],
    };
    assert_eq!(
        ledger.apply_control(&no_votes),
        Err(PaymentError::InvalidControlCertificate)
    );

    commit_control(
        vec![
            ControlOperation::OpenDispute {
                job_id,
                actor: alice_id,
            },
            ControlOperation::ResolveJob {
                job_id,
                resolution: JobResolution::PayProvider,
            },
        ],
        &mut ledger,
        &mut authorities,
    );
    let voucher_ids = ledger.job(&job_id).unwrap().payout_vouchers.clone();
    assert_eq!(voucher_ids.len(), 2);
    for voucher in &voucher_ids {
        ledger.redeem_voucher(voucher).unwrap();
    }
    assert_eq!(ledger.balance(&provider_id), 40);
    assert_eq!(ledger.balance(&alice_id), 60); // 30 never deposited + 30 refund
    assert_eq!(ledger.total_supply(), initial_supply);
    assert_eq!(
        ledger.redeem_voucher(&voucher_ids[0]),
        Err(PaymentError::VoucherAlreadyRedeemed)
    );

    // The same certified deposit cannot fund a second escrow object.
    assert_eq!(
        ledger.build_control_block(vec![ControlOperation::OpenJob {
            job_id: digest(78),
            buyer: alice_id,
            budget: 70,
            funding: deposit,
        }]),
        Err(PaymentError::FundingAlreadyConsumed)
    );

    let before_rewards: std::collections::BTreeSet<_> = ledger.voucher_ids().into_iter().collect();
    commit_control(
        vec![ControlOperation::DistributeReward {
            reward_epoch: 9,
            allocations: vec![(provider_id, 10)],
        }],
        &mut ledger,
        &mut authorities,
    );
    let reward_vouchers: Vec<_> = ledger
        .voucher_ids()
        .into_iter()
        .filter(|id| !before_rewards.contains(id))
        .collect();
    assert_eq!(reward_vouchers.len(), 1);
    ledger.redeem_voucher(&reward_vouchers[0]).unwrap();
    assert_eq!(ledger.balance(&provider_id), 50);
    assert_eq!(
        ledger.build_control_block(vec![ControlOperation::DistributeReward {
            reward_epoch: 9,
            allocations: vec![(provider_id, 10)],
        }]),
        Err(PaymentError::RewardEpochAlreadyDistributed)
    );
    assert_eq!(ledger.total_supply(), initial_supply);

    // Routine payout remains on the fast path after control operations.
    let payout_order = order(&ledger, &alice, provider_id, 5);
    let payout = certify_transfer(&payout_order, &ledger, &mut authorities);
    apply_and_confirm_transfer(&payout, &mut ledger, &mut authorities);
    assert_eq!(ledger.balance(&alice_id), 55);
    assert_eq!(ledger.balance(&provider_id), 55);
    assert_eq!(ledger.total_supply(), initial_supply);
}
