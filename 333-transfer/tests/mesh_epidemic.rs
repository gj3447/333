// KG: transport-plan Step 4 (2026-07-14)
//
// Plumtree epidemic over InMemoryAuthorityMesh targeted delivery.
// Reuses gossip333::InMemoryPlumtree via transfer333::epidemic.

use transfer333::{
    certify_via_mesh_rounds, encode_authority_msg, mesh_all_eager, mesh_with_dropped_eager_edge,
    msg_id_of, pump, Authority, AuthorityMsg, AuthorityNet, Certified, Committee, EpidemicEndpoint,
    EpidemicNetwork, Ledger, SigningKey, Transfer,
};

fn t(from: &str, seq: u64, to: &str, amount: u128) -> Transfer {
    Transfer {
        from: from.into(),
        from_seq: seq,
        to: to.into(),
        amount,
    }
}

fn key(i: u8) -> SigningKey {
    SigningKey::from_bytes(&[i; 32])
}

fn vote_msg(auth_idx: u8, transfer: &Transfer) -> (Authority, AuthorityMsg) {
    let mut a = Authority::new(format!("a{auth_idx}"), key(auth_idx));
    let v = a.handle(transfer).unwrap();
    (a, AuthorityMsg::Vote(v))
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
    let mut auth = Authority::new("a0", key(0));
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

/// 3. No duplicate app delivery; Plumtree prune on second full receive.
#[test]
fn no_duplicate_delivery_prune_on_second_eager() {
    let net = EpidemicNetwork::new();
    let a = net.join("a");
    let b = net.join("b");
    mesh_all_eager(&[&a, &b]);

    let transfer = t("alice", 0, "bob", 3);
    let mut auth = Authority::new("a0", key(0));
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
    let authorities: Vec<Authority> = (0..4u8)
        .map(|i| Authority::new(format!("a{i}"), key(i)))
        .collect();
    let committee =
        Committee::new(authorities.iter().map(|a| (a.id().clone(), a.verifying_key()))).unwrap();
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
    ledger.apply_verified(&verified).unwrap();
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
