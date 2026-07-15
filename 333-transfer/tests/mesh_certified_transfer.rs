// KG: transport-plan Step 3 (2026-07-14)
//
// End-to-end certification over isolated authority mailboxes (public API).
// Ports the scenarios from `certified_transfer.rs` onto `certify_via_mesh` —
// the first true "transport" green (no shared `certify(&mut [Authority])` loop).

use transfer333::{
    certify_via_mesh, Authority, Certified, Committee, InMemoryAuthorityMesh, Ledger, MeshEndpoint,
    NetworkId, OwnerRegistry, SignedTransfer, SigningKey, Transfer, TransferPolicy,
};

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn policy() -> TransferPolicy {
    TransferPolicy::new(
        NetworkId::new("mesh-certified-testnet").unwrap(),
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

fn setup() -> (Committee, Vec<Authority>, Vec<MeshEndpoint>, MeshEndpoint) {
    let policy = policy();
    let committee = Committee::new(
        (0..4u8).map(|i| (format!("a{i}"), key(i).verifying_key())),
        policy.clone(),
    )
    .unwrap();
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
    let mesh = InMemoryAuthorityMesh::new();
    let endpoints: Vec<MeshEndpoint> = authorities
        .iter()
        .map(|a| mesh.join(a.id().clone()))
        .collect();
    let client = mesh.join("client");
    (committee, authorities, endpoints, client)
}

#[test]
fn mesh_end_to_end_certified_transfer_applies_only_via_verified() {
    let (committee, mut authorities, endpoints, client) = setup();
    let mut ledger = Ledger::genesis([("alice".to_string(), 100u128), ("bob".to_string(), 0u128)]);

    let (verified, status) = certify_via_mesh(
        &t("alice", 0, "bob", 30),
        &mut authorities,
        &endpoints,
        &client,
        &committee,
    );
    assert_eq!(status, Certified::Ok);
    let verified = verified.expect("quorum reached over mailboxes");

    ledger.apply_verified(&verified, &committee).unwrap();
    assert_eq!(ledger.balance(&"bob".to_string()), 30);
    assert_eq!(ledger.balance(&"alice".to_string()), 70);
    assert_eq!(ledger.total_supply(), 100);
}

#[test]
fn mesh_end_to_end_byzantine_equivocation_cannot_double_apply() {
    let (committee, mut authorities, endpoints, client) = setup();
    let mut ledger = Ledger::genesis([
        ("alice".to_string(), 100u128),
        ("bob".to_string(), 0u128),
        ("carol".to_string(), 0u128),
    ]);

    let (v1, s1) = certify_via_mesh(
        &t("alice", 0, "bob", 100),
        &mut authorities,
        &endpoints,
        &client,
        &committee,
    );
    assert_eq!(s1, Certified::Ok);
    ledger.apply_verified(&v1.unwrap(), &committee).unwrap();

    let (v2, s2) = certify_via_mesh(
        &t("alice", 0, "carol", 100),
        &mut authorities,
        &endpoints,
        &client,
        &committee,
    );
    assert!(v2.is_none(), "no Verified for double-spend over mesh");
    assert!(matches!(s2, Certified::Failed { .. }));

    assert_eq!(ledger.balance(&"bob".to_string()), 100);
    assert_eq!(ledger.balance(&"carol".to_string()), 0);
    assert_eq!(ledger.total_supply(), 100);
}

#[test]
fn mesh_sequential_transfers_certify_in_order_only() {
    let (committee, mut authorities, endpoints, client) = setup();
    let mut ledger = Ledger::genesis([("alice".to_string(), 100u128), ("bob".to_string(), 0u128)]);

    for (seq, amt) in [(0u64, 40u128), (1, 25)] {
        let (v, s) = certify_via_mesh(
            &t("alice", seq, "bob", amt),
            &mut authorities,
            &endpoints,
            &client,
            &committee,
        );
        assert_eq!(s, Certified::Ok, "seq {seq}");
        ledger.apply_verified(&v.unwrap(), &committee).unwrap();
    }
    assert_eq!(ledger.balance(&"bob".to_string()), 65);

    let (v3, s3) = certify_via_mesh(
        &t("alice", 3, "bob", 5),
        &mut authorities,
        &endpoints,
        &client,
        &committee,
    );
    assert!(v3.is_none());
    assert!(matches!(s3, Certified::Failed { contested: false, .. }));
}
