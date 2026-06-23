//! PROM step 0 · receipt #1 — the identity primitive is RFC 8032 Ed25519,
//! and a 333 DID == a libp2p PeerId.
//!
//! Vectors: RFC 8032 §7.1, fetched verbatim from rfc-editor.org (NOT from memory)
//! and length-validated (seed=32B, pub=32B, sig=64B) before use. Test 2's signature
//! arrived with a dropped leading zero (127 hex); it was re-fetched two-line-verbatim
//! and re-validated to 128 hex / 64 bytes. That catch is exactly why we don't trust
//! remembered constants.
//!
//! No-fake-green: every assertion exercises real key derivation, real RFC 8032
//! deterministic signing, and the canonical libp2p PeerId derivation. None is a
//! tautology (we compare against externally-fixed RFC bytes, not against ourselves).

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use libp2p_identity::{KeyType, PeerId};
use p333_identity::{ed25519_did, ed25519_public, ed25519_public_to_peer_id};
use std::str::FromStr;

struct Vec8032 {
    seed: &'static str,
    public: &'static str,
    msg: &'static str,
    sig: &'static str,
}

// RFC 8032 §7.1 Ed25519 — Test 1 (empty msg), Test 2 (1-byte), Test 3 (2-byte).
const RFC8032: &[Vec8032] = &[
    Vec8032 {
        seed: "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        public: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        msg: "",
        sig: "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
    },
    Vec8032 {
        seed: "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
        public: "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
        msg: "72",
        // re-fetched two-line-verbatim, re-validated to 128 hex
        sig: "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
    },
    Vec8032 {
        seed: "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
        public: "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
        msg: "af82",
        sig: "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
    },
];

fn h(s: &str) -> Vec<u8> {
    hex::decode(s).expect("vector must be valid hex")
}

fn arr32(v: &[u8]) -> [u8; 32] {
    v.try_into().expect("expected 32 bytes")
}

fn arr64(v: &[u8]) -> [u8; 64] {
    v.try_into().expect("expected 64 bytes")
}

/// The crypto floor: keygen + deterministic signing must match RFC 8032 byte-for-byte.
#[test]
fn ed25519_rfc8032_keygen_and_sign_conformance() {
    for v in RFC8032 {
        let seed = h(v.seed);
        let public = h(v.public);
        let sig = h(v.sig);
        let msg = h(v.msg);
        assert_eq!(seed.len(), 32, "RFC 8032 seed is 32 bytes");
        assert_eq!(public.len(), 32, "Ed25519 public key is 32 bytes");
        assert_eq!(sig.len(), 64, "Ed25519 signature is 64 bytes");

        let signing = SigningKey::from_bytes(&arr32(&seed));

        // public key derived from the seed must equal the RFC vector
        assert_eq!(
            signing.verifying_key().to_bytes().as_slice(),
            public.as_slice(),
            "derived Ed25519 public key must match RFC 8032 vector"
        );

        // deterministic RFC 8032 signature must equal the vector, byte-for-byte
        let produced: Signature = signing.sign(&msg);
        assert_eq!(
            produced.to_bytes().as_slice(),
            sig.as_slice(),
            "RFC 8032 deterministic signature must match vector"
        );

        // and the vector's own signature must verify against the vector public key
        let vk = VerifyingKey::from_bytes(&arr32(&public)).expect("valid public key");
        vk.verify_strict(&msg, &Signature::from_bytes(&arr64(&sig)))
            .expect("RFC 8032 vector signature must verify");
    }
}

/// The DID contract: a 333 DID == the canonical libp2p PeerId of an Ed25519 key,
/// it round-trips, and the key type is Ed25519 — never secp256k1/BIP-340.
#[test]
fn did_equals_libp2p_peer_id_and_is_ed25519_not_secp256k1() {
    for v in RFC8032 {
        let public = h(v.public);

        // the 333 wrapper accepts the SAME Ed25519 public-key bytes
        let pk = ed25519_public(&public).expect("ed25519 public key");

        // HARD CORE guard: identity curve is Ed25519, not secp256k1
        assert_eq!(
            pk.key_type(),
            KeyType::Ed25519,
            "333 identity MUST be Ed25519, never secp256k1/BIP-340"
        );

        // DID is the PeerId base58btc form, derived canonically by libp2p
        let peer_id = ed25519_public_to_peer_id(&public).expect("peer id");
        assert_eq!(
            peer_id,
            pk.to_peer_id(),
            "DID derivation must match libp2p's own PeerId-from-public-key"
        );

        let did = ed25519_did(&public).expect("did");
        assert_eq!(did, peer_id.to_base58(), "DID is the PeerId base58 string");

        // External libp2p invariant (not self-referential): EVERY Ed25519 PeerId
        // base58btc-encodes with the "12D3KooW" prefix (identity-multihash over the
        // protobuf key envelope). A wrong multihash/envelope would break this.
        assert!(
            did.starts_with("12D3KooW"),
            "Ed25519 DID must carry the canonical libp2p prefix, got {did}"
        );

        // round-trip: DID string parses back to the same PeerId
        assert_eq!(
            PeerId::from_str(&did).expect("DID parses"),
            peer_id,
            "DID must round-trip to the same PeerId"
        );
    }
}

/// Malformed input must be rejected, not silently coerced.
#[test]
fn rejects_wrong_length_public_key() {
    assert!(
        ed25519_public(&[0u8; 31]).is_err(),
        "a 31-byte key must be rejected"
    );
    assert!(
        ed25519_public(&[0u8; 33]).is_err(),
        "a 33-byte key must be rejected"
    );
}
