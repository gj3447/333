// KG: transport-plan Step 8 (2026-07-14)
//
// Real TCP I/O integration: same certify flow as Steps 5–7, over sockets.
// Wall-clock sleeps / retries ARE allowed here (only Steps 1–7 were deterministic).

use std::net::SocketAddr;
use std::time::Duration;

use transfer333::{
    certify_via_mesh_rounds_with_pause, disseminate_certificate_with_pause, Authority, Certified,
    Committee, Ledger, MeshLedger, NetworkId, OwnerRegistry, SignedTransfer, SigningKey,
    TcpAuthorityNet, TcpEndpoint, Transfer, TransferPolicy,
};

const PAUSE: Duration = Duration::from_millis(10);
const MAX_ROUNDS: usize = 80;

fn key(i: u8) -> SigningKey {
    SigningKey::from_bytes(&[i; 32])
}

fn policy() -> TransferPolicy {
    TransferPolicy::new(
        NetworkId::new("tcp-transport-testnet").unwrap(),
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

fn genesis_alice_bob() -> Ledger {
    Ledger::genesis([("alice".to_string(), 100u128), ("bob".to_string(), 0u128)])
}

fn genesis_abc() -> Ledger {
    Ledger::genesis([
        ("alice".to_string(), 100u128),
        ("bob".to_string(), 0u128),
        ("carol".to_string(), 0u128),
    ])
}

/// N=4 authorities + 1 client (+ optional ledger peers), full mesh, ephemeral ports.
struct TcpMesh {
    authorities: Vec<Authority>,
    committee: Committee,
    auth_nets: Vec<TcpAuthorityNet>,
    auth_eps: Vec<TcpEndpoint>,
    client_net: TcpAuthorityNet,
    client: TcpEndpoint,
    ledger_nets: Vec<TcpAuthorityNet>,
    ledgers: Vec<MeshLedger<TcpEndpoint>>,
}

impl TcpMesh {
    fn boot(n_auth: u8, n_ledgers: usize, ledger_genesis: impl Fn() -> Ledger) -> Self {
        let policy = policy();
        let committee = Committee::new(
            (0..n_auth).map(|i| (format!("a{i}"), key(i).verifying_key())),
            policy.clone(),
        )
        .unwrap();
        let authorities: Vec<Authority> = (0..n_auth)
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

        let auth_nets: Vec<TcpAuthorityNet> = authorities
            .iter()
            .map(|a| TcpAuthorityNet::bind(a.id().clone()).expect("bind authority"))
            .collect();
        let client_net = TcpAuthorityNet::bind("client").expect("bind client");
        let ledger_nets: Vec<TcpAuthorityNet> = (0..n_ledgers)
            .map(|i| TcpAuthorityNet::bind(format!("L{i}")).expect("bind ledger"))
            .collect();

        let mut addrs: Vec<SocketAddr> = auth_nets.iter().map(|n| n.addr()).collect();
        addrs.push(client_net.addr());
        for n in &ledger_nets {
            addrs.push(n.addr());
        }

        // Full mesh with bounded connect retries (listeners may still be ramping).
        for n in &auth_nets {
            n.connect_all(&addrs).expect("auth connect_all");
        }
        client_net.connect_all(&addrs).expect("client connect_all");
        for n in &ledger_nets {
            n.connect_all(&addrs).expect("ledger connect_all");
        }

        // Brief settle so accept threads have streams before traffic.
        std::thread::sleep(Duration::from_millis(20));

        let auth_eps: Vec<TcpEndpoint> = auth_nets.iter().map(|n| n.endpoint()).collect();
        let client = client_net.endpoint();
        let ledgers: Vec<MeshLedger<TcpEndpoint>> = ledger_nets
            .iter()
            .map(|n| MeshLedger::new(n.endpoint(), ledger_genesis()))
            .collect();

        Self {
            authorities,
            committee,
            auth_nets,
            auth_eps,
            client_net,
            client,
            ledger_nets,
            ledgers,
        }
    }

    fn shutdown(self) {
        for n in self.auth_nets {
            n.shutdown();
        }
        self.client_net.shutdown();
        for n in self.ledger_nets {
            n.shutdown();
        }
    }
}

#[test]
fn tcp_honest_transfer_certifies_and_ledgers_agree() {
    let mut mesh = TcpMesh::boot(4, 2, genesis_alice_bob);
    assert_eq!(mesh.committee.quorum(), 3);

    let transfer = t("alice", 0, "bob", 40);
    let (verified, cert, status) = certify_via_mesh_rounds_with_pause(
        &transfer,
        &mut mesh.authorities,
        &mesh.auth_eps,
        &mesh.client,
        &mesh.committee,
        MAX_ROUNDS,
        PAUSE,
    );
    assert_eq!(status, Certified::Ok, "honest path must certify over TCP");
    let verified = verified.expect("Verified");
    let cert = cert.expect("Certificate");
    assert_eq!(verified.transfer(), &transfer.transfer);

    // Independent ledger replicas apply via the same cert dissemination path.
    let results = disseminate_certificate_with_pause(
        &mesh.client,
        &cert,
        &mesh.committee,
        &mut mesh.ledgers,
        MAX_ROUNDS,
        PAUSE,
    )
    .expect("disseminate");
    assert_eq!(results.len(), 2);
    for r in &results {
        assert!(
            r.iter().any(|x| x.is_ok()),
            "each ledger should apply at least once: {r:?}"
        );
    }

    assert_eq!(mesh.ledgers[0].ledger().balance(&"bob".into()), 40);
    assert_eq!(mesh.ledgers[1].ledger().balance(&"bob".into()), 40);
    assert_eq!(mesh.ledgers[0].ledger().balance(&"alice".into()), 60);
    assert_eq!(mesh.ledgers[1].ledger().balance(&"alice".into()), 60);
    assert_eq!(mesh.ledgers[0].ledger().total_supply(), 100);
    assert_eq!(mesh.ledgers[1].ledger().total_supply(), 100);

    // Authorities advanced next_expected via remote confirm (seq 0 done → expect 1).
    for a in &mut mesh.authorities {
        assert_eq!(
            a.handle(&t("alice", 0, "bob", 1)),
            Err(transfer333::AuthorityError::OutOfOrder {
                account: "alice".into(),
                expected: 1,
                got: 0,
            }),
            "authority {} should have confirmed seq 0",
            a.id()
        );
    }

    mesh.shutdown();
}

#[test]
fn tcp_double_spend_yields_no_cert_ledgers_unchanged() {
    let mut mesh = TcpMesh::boot(4, 2, genesis_abc);

    let t1 = t("alice", 0, "bob", 100);
    let (v1, c1, s1) = certify_via_mesh_rounds_with_pause(
        &t1,
        &mut mesh.authorities,
        &mesh.auth_eps,
        &mesh.client,
        &mesh.committee,
        MAX_ROUNDS,
        PAUSE,
    );
    assert_eq!(s1, Certified::Ok);
    let cert1 = c1.expect("cert");
    let _ = v1.expect("verified");
    disseminate_certificate_with_pause(
        &mesh.client,
        &cert1,
        &mesh.committee,
        &mut mesh.ledgers,
        MAX_ROUNDS,
        PAUSE,
    )
    .unwrap();
    assert_eq!(mesh.ledgers[0].ledger().balance(&"bob".into()), 100);
    assert_eq!(mesh.ledgers[1].ledger().balance(&"bob".into()), 100);

    // Reuse seq 0 for a different recipient → no cert, ledgers stay put.
    let t2 = t("alice", 0, "carol", 100);
    let (v2, c2, s2) = certify_via_mesh_rounds_with_pause(
        &t2,
        &mut mesh.authorities,
        &mesh.auth_eps,
        &mesh.client,
        &mesh.committee,
        MAX_ROUNDS,
        PAUSE,
    );
    assert!(v2.is_none());
    assert!(c2.is_none());
    assert!(matches!(s2, Certified::Failed { .. }));

    for led in &mesh.ledgers {
        assert_eq!(led.ledger().balance(&"bob".into()), 100);
        assert_eq!(led.ledger().balance(&"carol".into()), 0);
        assert_eq!(led.ledger().balance(&"alice".into()), 0);
        assert_eq!(led.ledger().total_supply(), 100);
    }

    mesh.shutdown();
}

#[test]
fn tcp_skipped_seq_is_out_of_order_not_contested() {
    let mut mesh = TcpMesh::boot(4, 0, genesis_alice_bob);

    let (v0, _, s0) = certify_via_mesh_rounds_with_pause(
        &t("alice", 0, "bob", 10),
        &mut mesh.authorities,
        &mesh.auth_eps,
        &mesh.client,
        &mesh.committee,
        MAX_ROUNDS,
        PAUSE,
    );
    assert_eq!(s0, Certified::Ok);
    assert!(v0.is_some());

    // Skip seq 1 → try seq 2: OutOfOrder refusals, not contested.
    let (v_skip, c_skip, s_skip) = certify_via_mesh_rounds_with_pause(
        &t("alice", 2, "bob", 5),
        &mut mesh.authorities,
        &mesh.auth_eps,
        &mesh.client,
        &mesh.committee,
        MAX_ROUNDS,
        PAUSE,
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
    let (v1, _, s1) = certify_via_mesh_rounds_with_pause(
        &t("alice", 1, "bob", 5),
        &mut mesh.authorities,
        &mesh.auth_eps,
        &mesh.client,
        &mesh.committee,
        MAX_ROUNDS,
        PAUSE,
    );
    assert_eq!(s1, Certified::Ok);
    assert!(v1.is_some());

    mesh.shutdown();
}
