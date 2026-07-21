//! PROM · receipt — `did:key` (W3C) interop, a KAT (log-free zone, like RFC-8032). The W3C
//! `did:key` and the libp2p PeerId DID are two encodings of ONE Ed25519 key; this pins the
//! encoding, the round-trip, and that both forms decode to the same key.

use p333_identity::{
    ed25519_did, ed25519_did_key, ed25519_from_did_key, ed25519_public_to_peer_id,
};

// RFC 8032 §7.1 Test 1 public key — shared with receipts #1/#2, so all three tie to one identity.
const T1_PUBLIC: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

fn t1() -> [u8; 32] {
    hex::decode(T1_PUBLIC).unwrap().try_into().unwrap()
}

#[test]
fn ed25519_did_key_is_z6mk_and_round_trips() {
    let pk = t1();
    let dk = ed25519_did_key(&pk).unwrap();
    // every Ed25519 did:key is `did:key:z6Mk…` (the 0xed01 multicodec base58btc-encodes to z6Mk…)
    assert!(dk.starts_with("did:key:z6Mk"), "ed25519 did:key must be z6Mk…, got {dk}");
    assert_eq!(ed25519_from_did_key(&dk).unwrap(), pk, "did:key must round-trip to the same key");
}

#[test]
fn did_key_and_peer_id_did_are_two_encodings_of_one_key() {
    let pk = t1();
    let recovered = ed25519_from_did_key(&ed25519_did_key(&pk).unwrap()).unwrap();
    // the PeerId derived via the did:key path == the PeerId derived directly
    assert_eq!(
        ed25519_public_to_peer_id(&recovered).unwrap(),
        ed25519_public_to_peer_id(&pk).unwrap(),
        "did:key decodes to the same key as the PeerId DID",
    );
    assert!(ed25519_did(&pk).unwrap().starts_with("12D3KooW")); // the PeerId DID is unchanged
}

#[test]
fn a_malformed_or_wrong_curve_did_key_is_rejected() {
    assert!(ed25519_from_did_key("nope").is_err()); // not did:key:z
    assert!(ed25519_from_did_key("did:key:z0OIl").is_err()); // invalid base58btc alphabet
    assert!(ed25519_did_key(&[0u8; 31]).is_err()); // wrong key length
    // a secp256k1 did:key (multicodec 0xe7 0x01) must NOT decode as ed25519 (curve-trap)
    let secp = format!("did:key:z{}", bs58::encode([0xe7u8, 0x01, 0xAA, 0xBB]).into_string());
    assert!(ed25519_from_did_key(&secp).is_err());
}
