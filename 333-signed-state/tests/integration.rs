// KG: SPAN_333_L5_L0_SignedCRDT_Integration, integration tests
// Proves crdt333 (Lww map semantics) and identity333 (ed25519 sig) compose.

use identity333::Keypair;
use signed_state333::{LwwPut, SignedLwwMap, SignedOp, SignedError};

fn alice() -> Keypair {
    Keypair::from_seed([1u8; 32])
}
fn bob() -> Keypair {
    Keypair::from_seed([2u8; 32])
}
fn eve() -> Keypair {
    Keypair::from_seed([9u8; 32])
}

#[test]
fn authorized_author_can_write() {
    let alice = alice();
    let mut m: SignedLwwMap<String, u32> = SignedLwwMap::new();
    m.authorize(alice.node_id());

    let op = SignedOp::new(&alice, LwwPut { key: "volume".into(), ts: 100, value: 42u32 }).unwrap();
    m.apply(&op).unwrap();
    assert_eq!(m.get(&"volume".into()), Some(&42));
}

#[test]
fn unknown_author_rejected() {
    let alice = alice();
    let eve = eve();
    let mut m: SignedLwwMap<String, u32> = SignedLwwMap::new();
    m.authorize(alice.node_id()); // eve NOT authorized

    let op = SignedOp::new(&eve, LwwPut { key: "volume".into(), ts: 100, value: 99 }).unwrap();
    assert!(matches!(m.apply(&op), Err(SignedError::UnknownAuthor(_))));
    assert!(m.is_empty());
}

#[test]
fn tampered_payload_rejected() {
    let alice = alice();
    let mut m: SignedLwwMap<String, u32> = SignedLwwMap::new();
    m.authorize(alice.node_id());

    let mut op = SignedOp::new(&alice, LwwPut { key: "x".into(), ts: 1, value: 5 }).unwrap();
    op.payload.value = 999; // tamper without re-signing
    assert!(matches!(m.apply(&op), Err(SignedError::BadSignature(_))));
}

#[test]
fn multi_author_lww_resolves_by_ts() {
    let alice = alice();
    let bob = bob();
    let mut m: SignedLwwMap<String, u32> = SignedLwwMap::new();
    m.authorize(alice.node_id());
    m.authorize(bob.node_id());

    let a_op = SignedOp::new(&alice, LwwPut { key: "k".into(), ts: 100, value: 1 }).unwrap();
    let b_op = SignedOp::new(&bob,   LwwPut { key: "k".into(), ts: 200, value: 2 }).unwrap();
    m.apply(&a_op).unwrap();
    m.apply(&b_op).unwrap();
    // Bob's later ts wins.
    assert_eq!(m.get(&"k".into()), Some(&2));
}

#[test]
fn op_is_idempotent() {
    let alice = alice();
    let mut m: SignedLwwMap<String, u32> = SignedLwwMap::new();
    m.authorize(alice.node_id());

    let op = SignedOp::new(&alice, LwwPut { key: "k".into(), ts: 50, value: 7 }).unwrap();
    m.apply(&op).unwrap();
    m.apply(&op).unwrap(); // replay
    m.apply(&op).unwrap();
    assert_eq!(m.get(&"k".into()), Some(&7));
    assert_eq!(m.len(), 1);
}

#[test]
fn wire_roundtrip_preserves_signature() {
    let alice = alice();
    let op: SignedOp<LwwPut<String, u32>> = SignedOp::new(&alice, LwwPut { key: "k".into(), ts: 1, value: 42u32 }).unwrap();

    // Simulate network: serialize + deserialize.
    let wire = serde_json::to_string(&op).unwrap();
    let parsed: SignedOp<LwwPut<String, u32>> = serde_json::from_str(&wire).unwrap();

    parsed.verify().unwrap(); // sig still valid after roundtrip
}

#[test]
fn signature_binds_to_author_not_interchangeable() {
    let alice = alice();
    let bob = bob();
    let mut m: SignedLwwMap<String, u32> = SignedLwwMap::new();
    m.authorize(bob.node_id());

    // Alice signs, but op pretends to be from Bob.
    let mut op = SignedOp::new(&alice, LwwPut { key: "k".into(), ts: 1, value: 1 }).unwrap();
    op.author = bob.node_id(); // forge author field

    assert!(matches!(m.apply(&op), Err(SignedError::BadSignature(_))));
}
