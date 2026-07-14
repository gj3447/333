// KG: SPAN_333_Identity_KuboPort, integration tests
// End-to-end: generate → store → sign → retrieve → verify

use identity333::{InMemoryKeyStore, KeyStore, Keypair, NodeId};

#[test]
fn end_to_end_sign_verify_across_store() {
    let ks = InMemoryKeyStore::new();

    // Generate and store.
    let kp = Keypair::from_seed([11u8; 32]);
    let node_id = kp.node_id();
    ks.put("peer-A", kp).unwrap();

    // Retrieve and sign.
    let stored = ks.get("peer-A").unwrap();
    assert_eq!(stored.node_id(), node_id);
    let msg = b"hello from peer-A";
    let sig = stored.sign(msg);

    // Verify using the public key alone (no secret needed).
    Keypair::verify(node_id.as_bytes(), msg, &sig).unwrap();
}

#[test]
fn node_id_base58_stable_across_sessions() {
    let kp1 = Keypair::from_seed([7u8; 32]);
    let nid_str = kp1.node_id().to_base58();

    // Simulate new session: reconstruct from base58.
    let nid2 = NodeId::from_base58(&nid_str).unwrap();
    let kp2 = Keypair::from_seed([7u8; 32]);
    assert_eq!(nid2, kp2.node_id());
}

#[test]
fn cross_peer_verify() {
    // Peer B only knows peer A's pubkey (NodeId); verifies sig from A.
    let alice = Keypair::from_seed([100u8; 32]);
    let alice_id = alice.node_id();

    let msg = b"broadcast payload";
    let alice_sig = alice.sign(msg);

    // Bob receives (msg, alice_id, alice_sig).
    Keypair::verify(alice_id.as_bytes(), msg, &alice_sig).unwrap();
}
