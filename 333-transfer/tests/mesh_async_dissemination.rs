// KG: transport-plan Steps 5–7 (2026-07-14)
//
// Decoupled multi-round collection, multi-ledger certificate dissemination,
// and remote Authority::confirm over the in-memory mesh. No wall-clock / tokio;
// rounds = mailbox drains only.

use transfer333::{
    authority_handle_round, certify_via_mesh_rounds, collect_until_quorum, confirm_from_mesh,
    disseminate_certificate, Authority, AuthorityMsg, AuthorityNet, Certified, Committee,
    InMemoryAuthorityMesh, Ledger, MeshEndpoint, MeshLedger, NetworkId, OwnerRegistry,
    SignedTransfer, SigningKey, Transfer, TransferPolicy, VoteCollector,
};

fn key(i: u8) -> SigningKey {
    SigningKey::from_bytes(&[i; 32])
}

fn policy() -> TransferPolicy {
    TransferPolicy::new(
        NetworkId::new("mesh-async-testnet").unwrap(),
        OwnerRegistry::new([
            ("alice", key(42).verifying_key()),
            ("bob", key(43).verifying_key()),
            ("carol", key(44).verifying_key()),
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

fn t(from: &str, seq: u64, to: &str, amount: u128) -> SignedTransfer {
    let policy = policy();
    SignedTransfer::sign(
        &policy,
        Transfer {
            from: from.into(),
            from_seq: seq,
            to: to.into(),
            amount,
        },
        &key(42),
    )
}

fn setup_mesh(
    n: u8,
) -> (
    Committee,
    Vec<Authority>,
    std::sync::Arc<InMemoryAuthorityMesh>,
    Vec<MeshEndpoint>,
    MeshEndpoint,
) {
    let policy = policy();
    let committee = Committee::new(
        (0..n).map(|i| (format!("a{i}"), key(i).verifying_key())),
        policy.clone(),
    )
    .unwrap();
    let authorities: Vec<Authority> = (0..n)
        .map(|i| {
            Authority::new(
                format!("a{i}"),
                key(i),
                policy.clone(),
                committee.id(),
                authority_genesis(),
            )
        })
        .collect();
    let mesh = InMemoryAuthorityMesh::new();
    let endpoints: Vec<MeshEndpoint> = authorities
        .iter()
        .map(|a| mesh.join(a.id().clone()))
        .collect();
    let client = mesh.join("client");
    (committee, authorities, mesh, endpoints, client)
}

fn genesis_alice_bob() -> Ledger {
    Ledger::genesis([("alice".to_string(), 100u128), ("bob".to_string(), 0u128)])
}

// --- Step 5 -----------------------------------------------------------------

#[test]
fn step5_votes_across_rounds_reach_quorum() {
    // n=4, quorum=3: inject one vote per round into the collector mailbox.
    let (committee, mut authorities, mesh, _endpoints, client) = setup_mesh(4);
    assert_eq!(committee.quorum(), 3);
    let transfer = t("alice", 0, "bob", 30);

    let mut coll = VoteCollector::new(transfer.clone());
    let mut finished_at = None;
    for i in 0..4 {
        let vote = authorities[i].handle(&transfer).unwrap();
        mesh.deliver_to(client.id(), AuthorityMsg::Vote(vote)).unwrap();
        coll.poll_round(&client, &committee);
        if let Some(cert) = coll.try_assemble(&committee) {
            finished_at = Some(i);
            let verified = cert.verify(&committee).expect("valid Verified");
            assert_eq!(verified.transfer(), &transfer.transfer);
            assert_eq!(coll.vote_count(), 3);
            break;
        }
    }
    // Quorum first reached on the 3rd vote (index 2).
    assert_eq!(finished_at, Some(2));
}

#[test]
fn step5_collect_until_quorum_free_fn_with_preloaded_votes() {
    let (committee, mut authorities, mesh, _endpoints, client) = setup_mesh(4);
    let transfer = t("alice", 0, "bob", 25);
    for a in authorities.iter_mut().take(3) {
        let v = a.handle(&transfer).unwrap();
        mesh.deliver_to(client.id(), AuthorityMsg::Vote(v)).unwrap();
    }
    let (verified, status) = collect_until_quorum(&client, &transfer, &committee, 3);
    assert_eq!(status, Certified::Ok);
    let verified = verified.expect("quorum");
    assert_eq!(verified.transfer(), &transfer.transfer);
}

#[test]
fn step5_sub_quorum_stalls_after_max_rounds() {
    // Only 2 of 4 authorities vote (< quorum 3). Collector stalls to Failed.
    let (committee, mut authorities, mesh, _endpoints, client) = setup_mesh(4);
    let transfer = t("alice", 0, "bob", 10);
    for a in authorities.iter_mut().take(2) {
        let v = a.handle(&transfer).unwrap();
        mesh.deliver_to(client.id(), AuthorityMsg::Vote(v)).unwrap();
    }
    let max_rounds = 5;
    let (verified, status) = collect_until_quorum(&client, &transfer, &committee, max_rounds);
    assert!(verified.is_none());
    assert_eq!(
        status,
        Certified::Failed {
            votes: 2,
            refusals: 0,
            contested: false,
        }
    );
}

#[test]
fn step5_equivocation_split_is_contested() {
    // Half lock T1, half lock T2 (directed inject), then multi-round collect of T1
    // fails with contested=true — same counters as certify_via_mesh contested test.
    let (committee, mut authorities, mesh, endpoints, client) = setup_mesh(4);
    let t1 = t("alice", 0, "bob", 100);
    let t2 = t("alice", 0, "carol", 100);

    mesh.deliver_to("a0", AuthorityMsg::Order(t1.clone())).unwrap();
    mesh.deliver_to("a1", AuthorityMsg::Order(t1.clone())).unwrap();
    mesh.deliver_to("a2", AuthorityMsg::Order(t2.clone())).unwrap();
    mesh.deliver_to("a3", AuthorityMsg::Order(t2.clone())).unwrap();
    for (auth, ep) in authorities.iter_mut().zip(endpoints.iter()) {
        for msg in ep.poll() {
            if let AuthorityMsg::Order(order) = msg {
                let _ = auth.handle(&order);
            }
        }
    }

    // Full-mesh order for T1: a0/a1 re-vote, a2/a3 Equivocation.
    client.broadcast_order(t1.clone()).unwrap();
    let mut coll = VoteCollector::new(t1.clone());
    authority_handle_round(&mut authorities, &endpoints, &mut coll);
    let (cert, status) = coll.collect_until_quorum(&client, &committee, 4);
    assert!(cert.is_none());
    assert_eq!(
        status,
        Certified::Failed {
            votes: 2,
            refusals: 2,
            contested: true,
        }
    );
}

// --- Step 6 -----------------------------------------------------------------

#[test]
fn step6_independent_ledgers_converge_on_same_balances() {
    let (committee, mut authorities, mesh, endpoints, client) = setup_mesh(4);
    let transfer = t("alice", 0, "bob", 40);

    // Join ledgers before certify so the cert fan-out reaches them too; then also
    // exercise explicit disseminate after assemble for a third path.
    let mut ledgers = [
        MeshLedger::new(mesh.join("L1"), genesis_alice_bob()),
        MeshLedger::new(mesh.join("L2"), genesis_alice_bob()),
    ];

    client.broadcast_order(transfer.clone()).unwrap();
    let mut coll = VoteCollector::new(transfer.clone());
    authority_handle_round(&mut authorities, &endpoints, &mut coll);
    let (cert, status) = coll.collect_until_quorum(&client, &committee, 4);
    assert_eq!(status, Certified::Ok);
    let cert = cert.expect("assembled");
    let verified = cert.verify(&committee).expect("Verified");

    // Step 6: broadcast cert; each replica verifies locally + apply_verified.
    let results = disseminate_certificate(&client, &cert, &committee, &mut ledgers).unwrap();
    assert_eq!(results.len(), 2);
    for r in &results {
        assert_eq!(r.len(), 1);
        assert!(r[0].is_ok());
    }

    // Step 7 path also available: authorities confirm from the same cert fan-out.
    confirm_from_mesh(&mut authorities, &endpoints, &committee);

    assert_eq!(ledgers[0].ledger().balance(&"alice".into()), 60);
    assert_eq!(ledgers[0].ledger().balance(&"bob".into()), 40);
    assert_eq!(ledgers[1].ledger().balance(&"alice".into()), 60);
    assert_eq!(ledgers[1].ledger().balance(&"bob".into()), 40);
    assert_eq!(ledgers[0].ledger().total_supply(), 100);
    assert_eq!(ledgers[1].ledger().total_supply(), 100);
    // Convergence: same balances across independent ledgers.
    assert_eq!(
        ledgers[0].ledger().balance(&"alice".into()),
        ledgers[1].ledger().balance(&"alice".into())
    );
    assert_eq!(
        ledgers[0].ledger().balance(&"bob".into()),
        ledgers[1].ledger().balance(&"bob".into())
    );
    // Type-state: we only applied via Verified path.
    assert_eq!(verified.transfer(), &transfer.transfer);
}

#[test]
fn step6_double_spend_yields_no_cert_ledgers_unchanged() {
    let (committee, mut authorities, mesh, endpoints, client) = setup_mesh(4);
    let mut ledgers = [
        MeshLedger::new(
            mesh.join("L1"),
            Ledger::genesis([
                ("alice".to_string(), 100u128),
                ("bob".to_string(), 0u128),
                ("carol".to_string(), 0u128),
            ]),
        ),
        MeshLedger::new(
            mesh.join("L2"),
            Ledger::genesis([
                ("alice".to_string(), 100u128),
                ("bob".to_string(), 0u128),
                ("carol".to_string(), 0u128),
            ]),
        ),
    ];

    // Honest first spend certifies and applies on both ledgers.
    let t1 = t("alice", 0, "bob", 100);
    let (v1, c1, s1) =
        certify_via_mesh_rounds(&t1, &mut authorities, &endpoints, &client, &committee, 4);
    assert_eq!(s1, Certified::Ok);
    let cert1 = c1.expect("cert");
    let _ = v1.expect("verified");
    disseminate_certificate(&client, &cert1, &committee, &mut ledgers).unwrap();
    assert_eq!(ledgers[0].ledger().balance(&"bob".into()), 100);
    assert_eq!(ledgers[1].ledger().balance(&"bob".into()), 100);

    // Equivocating re-use of seq 0 → no valid cert → no ledger applies.
    let t2 = t("alice", 0, "carol", 100);
    let (v2, c2, s2) =
        certify_via_mesh_rounds(&t2, &mut authorities, &endpoints, &client, &committee, 4);
    assert!(v2.is_none());
    assert!(c2.is_none());
    assert!(matches!(s2, Certified::Failed { .. }));

    // Balances unchanged; total_supply conserved on every ledger.
    for led in &ledgers {
        assert_eq!(led.ledger().balance(&"bob".into()), 100);
        assert_eq!(led.ledger().balance(&"carol".into()), 0);
        assert_eq!(led.ledger().balance(&"alice".into()), 0);
        assert_eq!(led.ledger().total_supply(), 100);
    }
}

// --- Step 7 -----------------------------------------------------------------

#[test]
fn step7_sequential_seq0_then_seq1_confirm_over_mesh() {
    let (committee, mut authorities, mesh, endpoints, client) = setup_mesh(4);
    let mut ledgers = [
        MeshLedger::new(mesh.join("L1"), genesis_alice_bob()),
        MeshLedger::new(mesh.join("L2"), genesis_alice_bob()),
    ];

    for (seq, amt) in [(0u64, 40u128), (1, 25)] {
        let transfer = t("alice", seq, "bob", amt);
        let (verified, cert, status) = certify_via_mesh_rounds(
            &transfer,
            &mut authorities,
            &endpoints,
            &client,
            &committee,
            4,
        );
        assert_eq!(status, Certified::Ok, "seq {seq}");
        assert!(verified.is_some());
        let cert = cert.expect("cert");
        // Remote confirm already ran inside certify_via_mesh_rounds.
        disseminate_certificate(&client, &cert, &committee, &mut ledgers).unwrap();
    }

    assert_eq!(ledgers[0].ledger().balance(&"bob".into()), 65);
    assert_eq!(ledgers[1].ledger().balance(&"bob".into()), 65);
    assert_eq!(ledgers[0].ledger().balance(&"alice".into()), 35);
    assert_eq!(ledgers[0].ledger().total_supply(), 100);
}

#[test]
fn step7_skipped_seq_is_out_of_order_not_contested() {
    let (committee, mut authorities, _mesh, endpoints, client) = setup_mesh(4);

    // seq 0 succeeds and confirms next_expected → 1.
    let (v0, _, s0) = certify_via_mesh_rounds(
        &t("alice", 0, "bob", 10),
        &mut authorities,
        &endpoints,
        &client,
        &committee,
        4,
    );
    assert_eq!(s0, Certified::Ok);
    assert!(v0.is_some());

    // Skip seq 1 → try seq 2: all authorities OutOfOrder → Failed, not contested.
    let (v_skip, c_skip, s_skip) = certify_via_mesh_rounds(
        &t("alice", 2, "bob", 5),
        &mut authorities,
        &endpoints,
        &client,
        &committee,
        4,
    );
    assert!(v_skip.is_none());
    assert!(c_skip.is_none());
    assert_eq!(
        s_skip,
        Certified::Failed {
            votes: 0,
            refusals: 4,
            contested: false,
        }
    );

    // seq 1 still certifies after the failed skip.
    let (v1, _, s1) = certify_via_mesh_rounds(
        &t("alice", 1, "bob", 5),
        &mut authorities,
        &endpoints,
        &client,
        &committee,
        4,
    );
    assert_eq!(s1, Certified::Ok);
    assert!(v1.is_some());
}
