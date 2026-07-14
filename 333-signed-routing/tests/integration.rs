// KG: SPAN_333_L0_L2_SignedRouting integration
// Proves identity333 × kademlia333 compose into Byzantine-resistant routing.

use identity333::Keypair;
use signed_routing333::{RoutingError, SignedPeerRecord, SignedRoutingTable};

fn kp(seed: u8) -> Keypair {
    Keypair::from_seed([seed; 32])
}

#[test]
fn genuine_record_stored_and_retrievable() {
    let me = kp(0).node_id();
    let mut rt = SignedRoutingTable::new(me);

    let alice = kp(1);
    let rec = SignedPeerRecord::sign(&alice, "/ip4/1.2.3.4/tcp/4001", 100).unwrap();
    rt.observe(rec).unwrap();
    assert_eq!(rt.len(), 1);

    let got = rt.get(&alice.node_id()).unwrap();
    assert_eq!(got.payload.addr, "/ip4/1.2.3.4/tcp/4001");
    assert_eq!(got.payload.ts, 100);
}

#[test]
fn spoofed_record_rejected() {
    // Eve forges a record claiming to be Alice by copying Alice's node_id and signing
    // with her own key. The signature cannot satisfy Keypair::verify against Alice's pubkey.
    let me = kp(0).node_id();
    let mut rt = SignedRoutingTable::new(me);
    let alice = kp(1);
    let eve = kp(99);

    // Eve signs her own record legitimately.
    let mut forged = SignedPeerRecord::sign(&eve, "/ip4/6.6.6.6/tcp/6666", 100).unwrap();
    // Swap the author field to impersonate Alice — sig no longer matches.
    forged.payload.node_id = alice.node_id();

    let err = rt.observe(forged).unwrap_err();
    assert!(matches!(err, RoutingError::BadSignature(_)));
    assert_eq!(rt.len(), 0);
}

#[test]
fn newer_ts_replaces_older() {
    let me = kp(0).node_id();
    let mut rt = SignedRoutingTable::new(me);
    let alice = kp(1);

    rt.observe(SignedPeerRecord::sign(&alice, "/ip4/1.1.1.1/tcp/1", 100).unwrap()).unwrap();
    rt.observe(SignedPeerRecord::sign(&alice, "/ip4/2.2.2.2/tcp/2", 200).unwrap()).unwrap();

    assert_eq!(rt.get(&alice.node_id()).unwrap().payload.addr, "/ip4/2.2.2.2/tcp/2");
    assert_eq!(rt.len(), 1);
}

#[test]
fn stale_record_rejected_as_replay_defense() {
    let me = kp(0).node_id();
    let mut rt = SignedRoutingTable::new(me);
    let alice = kp(1);

    let old = SignedPeerRecord::sign(&alice, "/old", 50).unwrap();
    let new = SignedPeerRecord::sign(&alice, "/new", 100).unwrap();
    rt.observe(new).unwrap();

    let err = rt.observe(old).unwrap_err();
    assert!(matches!(err, RoutingError::Stale { .. }));
    assert_eq!(rt.get(&alice.node_id()).unwrap().payload.addr, "/new");
}

#[test]
fn find_closest_returns_only_verified() {
    let me = kp(0).node_id();
    let mut rt = SignedRoutingTable::new(me);

    // 5 legit peers.
    for seed in 1..=5u8 {
        let k = kp(seed);
        let rec = SignedPeerRecord::sign(&k, format!("/peer/{seed}"), 10).unwrap();
        rt.observe(rec).unwrap();
    }

    let target = kp(3).node_id();
    let closest = rt.find_closest(&target, 3);
    assert_eq!(closest.len(), 3);
    // Nearest should be target itself (peer seed=3).
    assert_eq!(closest[0].node_id(), target);
    // Every returned record verifies.
    for r in &closest {
        r.verify().unwrap();
    }
}

#[test]
fn wire_roundtrip_preserves_sig() {
    let alice = kp(1);
    let rec = SignedPeerRecord::sign(&alice, "/ip/10", 5).unwrap();
    let wire = serde_json::to_string(&rec).unwrap();
    let parsed: SignedPeerRecord = serde_json::from_str(&wire).unwrap();
    parsed.verify().unwrap();
}

#[test]
fn hundred_peer_routing_simulation() {
    let me = kp(0).node_id();
    let mut rt = SignedRoutingTable::new(me);

    for i in 1u8..=100 {
        let k = kp(i);
        let rec = SignedPeerRecord::sign(&k, format!("/p/{i}"), i as u64).unwrap();
        rt.observe(rec).unwrap();
    }
    assert_eq!(rt.len(), 100);

    let target = kp(50).node_id();
    let closest = rt.find_closest(&target, 10);
    assert!(!closest.is_empty());
    // All returned records must verify (network-grade guarantee).
    for r in closest {
        r.verify().unwrap();
    }
}
