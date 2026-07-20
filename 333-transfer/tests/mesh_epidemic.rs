// KG: transport-plan Step 4 (2026-07-14)
//
// Plumtree epidemic over InMemoryAuthorityMesh targeted delivery.
// Reuses gossip333::InMemoryPlumtree via transfer333::epidemic.

use transfer333::{
    certify_via_mesh_rounds, encode_authority_msg, mesh_all_eager, mesh_with_dropped_eager_edge,
    msg_id_of, pump, Authority, AuthorityMsg, AuthorityNet, Certified, Committee, EpidemicEndpoint,
    EpidemicNetwork, Ledger, NetworkId, OwnerRegistry, SignedTransfer, SigningKey, Transfer,
    TransferPolicy,
};

fn key(i: u8) -> SigningKey {
    SigningKey::from_bytes(&[i; 32])
}

fn policy() -> TransferPolicy {
    TransferPolicy::new(
        NetworkId::new("mesh-epidemic-testnet").unwrap(),
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

fn committee(policy: &TransferPolicy) -> Committee {
    Committee::new(
        (0..4u8).map(|i| (format!("a{i}"), key(i).verifying_key())),
        policy.clone(),
    )
    .unwrap()
}

fn authority(auth_idx: u8) -> Authority {
    let policy = policy();
    let committee = committee(&policy);
    Authority::new(
        format!("a{auth_idx}"),
        key(auth_idx),
        policy,
        committee.id(),
        authority_genesis(),
    )
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

fn vote_msg(auth_idx: u8, transfer: &SignedTransfer) -> (Authority, AuthorityMsg) {
    let mut authority = authority(auth_idx);
    let v = authority.handle(transfer).unwrap();
    (authority, AuthorityMsg::Vote(v))
}

/// 1. Full eager mesh: a broadcast Vote reaches all N peers' inboxes.
#[test]
fn full_eager_mesh_vote_reaches_all() {
    let net = EpidemicNetwork::new();
    let peers: Vec<EpidemicEndpoint> = (0..4u8).map(|i| net.join(format!("a{i}"))).collect();
    let refs: Vec<&EpidemicEndpoint> = peers.iter().collect();
    mesh_all_eager(&refs);

    let transfer = t("alice", 0, "bob", 10);
    let (_auth, msg) = vote_msg(0, &transfer);
    let payload = encode_authority_msg(&msg);
    let id = msg_id_of(&payload);

    // a0 originates the vote via AuthorityNet.
    if let AuthorityMsg::Vote(v) = msg {
        peers[0].broadcast_vote(v).unwrap();
    }

    // One process round: eager push lands in every peer mailbox → app inbox.
    pump(&refs, 1);

    for (i, ep) in peers.iter().enumerate() {
        assert!(
            ep.has_msg(&id),
            "peer a{i} must have message via eager push"
        );
        assert_eq!(
            ep.deliver_count(&id),
            1,
            "peer a{i} should see exactly one app delivery"
        );
    }
}

/// 2. RESILIENCE: dropped eager edge A→D; message still reaches D via IHAVE→GRAFT.
#[test]
fn resilience_dropped_eager_edge_recovers_via_ihave_graft() {
    let net = EpidemicNetwork::new();
    let a = net.join("a");
    let b = net.join("b");
    let c = net.join("c");
    let d = net.join("d");
    let refs = [&a, &b, &c, &d];

    // A→D is lazy only; B/C also only lazy to D so multi-hop eager cannot bypass
    // the lazy recovery path. Core {A,B,C} is full-eager.
    mesh_with_dropped_eager_edge(&refs, "a", "d");

    assert!(a.lazy_peers().contains(&"d".to_string()) || !a.eager_peers().contains(&"d".to_string()));
    assert!(a.eager_peers().contains(&"b".to_string()));
    assert!(a.eager_peers().contains(&"c".to_string()));
    assert!(!a.eager_peers().contains(&"d".to_string()));

    let transfer = t("alice", 0, "bob", 7);
    let mut auth = authority(0);
    let vote = auth.handle(&transfer).unwrap();
    let id = msg_id_of(&encode_authority_msg(&AuthorityMsg::Vote(vote.clone())));

    a.broadcast_vote(vote).unwrap();

    // Immediately after broadcast: D must NOT have the body yet (only IHAVE queued).
    assert!(
        !d.has_msg(&id),
        "D must not receive eager push when A→D eager edge is missing"
    );

    // A few deterministic rounds: IHAVE lands → tick GRAFT → full body to D.
    pump(&refs, 6);

    assert!(
        d.has_msg(&id),
        "D must eventually receive the message via lazy IHAVE→GRAFT"
    );
    assert_eq!(d.deliver_count(&id), 1);

    // B and C (eager from A) already had it from the first push.
    assert!(b.has_msg(&id));
    assert!(c.has_msg(&id));
}

/// 2b. GRAFT double purpose: retransmission trigger AND tree-edge repair.
///
/// Same partition-heal scenario as test 2 — but Plumtree GRAFT is not only a
/// pull request for the body: it also re-promotes the grafting peer back into
/// the receiver's eager set (dual of PRUNE). Without that, the partition heals
/// payload-wise but the healed edge stays lazy forever, so every future
/// broadcast to D pays the IHAVE→tick→GRAFT round-trip again — permanent
/// latency/fanout degradation that silently accumulates.
#[test]
fn graft_heals_tree_edge_grafting_peer_back_in_eager() {
    let net = EpidemicNetwork::new();
    let a = net.join("a");
    let b = net.join("b");
    let c = net.join("c");
    let d = net.join("d");
    let refs = [&a, &b, &c, &d];
    mesh_with_dropped_eager_edge(&refs, "a", "d");

    // Pre-heal: nobody has an eager edge to D.
    for ep in &refs {
        if ep.id() != "d" {
            assert!(!ep.eager_peers().contains(&"d".to_string()));
            assert!(ep.lazy_peers().contains(&"d".to_string()));
        }
    }

    let transfer = t("alice", 0, "bob", 9);
    let mut auth = authority(0);
    let vote = auth.handle(&transfer).unwrap();
    let id = msg_id_of(&encode_authority_msg(&AuthorityMsg::Vote(vote.clone())));

    a.broadcast_vote(vote).unwrap();
    pump(&refs, 6);

    // Existing guarantee (test 2): the payload healed via IHAVE→GRAFT pull.
    assert!(d.has_msg(&id), "D must receive the body via GRAFT pull");
    assert_eq!(d.deliver_count(&id), 1);

    // THE DEFECT: D grafted onto its IHAVE announcers, so at least one of them
    // must have promoted D from lazy back to eager (edge repair). Duplicate
    // eager retransmissions then PRUNE the extras, so the tree converges to
    // ≥1 eager edge into D — never zero.
    let healed: Vec<&str> = refs
        .iter()
        .filter(|ep| ep.id() != "d" && ep.eager_peers().contains(&"d".to_string()))
        .map(|ep| ep.id())
        .collect();
    assert!(
        !healed.is_empty(),
        "GRAFT must repair the tree edge: some announcer must hold D in its \
         eager set after healing, got none (edge stayed lazy forever)"
    );
    // Deterministic pump order: A's IHAVE is queued first, so A's retransmission
    // lands first and survives D's duplicate-PRUNE trimming.
    assert!(
        a.eager_peers().contains(&"d".to_string()),
        "the original sender A must hold the healed eager edge to D"
    );
    assert!(
        !a.lazy_peers().contains(&"d".to_string()),
        "healed peer D must leave A's lazy view"
    );
}

/// 3. No duplicate app delivery; Plumtree prune on second full receive.
#[test]
fn no_duplicate_delivery_prune_on_second_eager() {
    let net = EpidemicNetwork::new();
    let a = net.join("a");
    let b = net.join("b");
    mesh_all_eager(&[&a, &b]);

    let transfer = t("alice", 0, "bob", 3);
    let mut auth = authority(0);
    let vote = auth.handle(&transfer).unwrap();
    let msg = AuthorityMsg::Vote(vote.clone());
    let id = msg_id_of(&encode_authority_msg(&msg));

    a.broadcast_vote(vote).unwrap();
    pump(&[&a, &b], 2);
    assert_eq!(b.deliver_count(&id), 1);
    assert!(b.eager_peers().contains(&"a".to_string()));

    // Inject a second full copy of the same message from A → B.
    net.mesh()
        .deliver_to_from("b", Some("a"), msg)
        .unwrap();
    b.process().unwrap();

    // Still a single app delivery; A demoted to lazy (PRUNE).
    assert_eq!(b.deliver_count(&id), 1, "duplicate must not re-deliver to app");
    assert!(
        !b.eager_peers().contains(&"a".to_string()),
        "duplicate eager receive demotes sender to lazy"
    );
    assert!(b.lazy_peers().contains(&"a".to_string()));
}

/// 4. End-to-end certify over the epidemic layer matches full-mesh certify semantics.
#[test]
fn certify_over_epidemic_layer_reaches_quorum() {
    let policy = policy();
    let committee = committee(&policy);
    let authorities: Vec<Authority> = (0..4u8)
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
    assert_eq!(committee.quorum(), 3);

    let net = EpidemicNetwork::new();
    let endpoints: Vec<EpidemicEndpoint> = authorities
        .iter()
        .map(|a| net.join(a.id().clone()))
        .collect();
    let client = net.join("client");
    let mut all_refs: Vec<&EpidemicEndpoint> = endpoints.iter().collect();
    all_refs.push(&client);
    mesh_all_eager(&all_refs);

    let mut authorities = authorities;
    let transfer = t("alice", 0, "bob", 30);

    // Multi-round certify; epidemic process runs inside each poll().
    // Extra pumps between conceptual rounds are not required for full eager mesh,
    // but we allow several rounds for interleaving handle + collect.
    let (verified, cert, status) = certify_via_mesh_rounds(
        &transfer,
        &mut authorities,
        &endpoints,
        &client,
        &committee,
        8,
    );
    assert_eq!(status, Certified::Ok);
    let verified = verified.expect("quorum over epidemic");
    assert!(cert.is_some());

    let mut ledger =
        Ledger::genesis([("alice".to_string(), 100u128), ("bob".to_string(), 0u128)]);
    ledger.apply_verified(&verified, &committee).unwrap();
    assert_eq!(ledger.balance(&"bob".to_string()), 30);
    assert_eq!(ledger.balance(&"alice".to_string()), 70);
    assert_eq!(ledger.total_supply(), 100);

    // Authorities confirmed (next_expected advanced) via cert dissemination:
    // a second seq-0 transfer must be OutOfOrder, not a fresh lock.
    let (v2, _c2, s2) = certify_via_mesh_rounds(
        &t("alice", 0, "carol", 5),
        &mut authorities,
        &endpoints,
        &client,
        &committee,
        8,
    );
    assert!(v2.is_none());
    assert!(matches!(s2, Certified::Failed { contested: false, .. }));
}
