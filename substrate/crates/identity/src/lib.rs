//! 333 v2 substrate identity — PROM step 0.
//!
//! HARD CORE: the identity primitive is **Ed25519 (RFC 8032)**, NOT secp256k1 /
//! BIP-340. A 333 DID is exactly the canonical **libp2p PeerId** derived from an
//! Ed25519 public key. This keeps the substrate's identity curve identical to
//! `rust-libp2p`'s PeerId curve (the survey's "lucky curve match" with Pubky/PKARR),
//! and keeps the Bitcoin/Nostr secp256k1 world strictly out of the DID layer.

use libp2p_identity::{ed25519, PeerId, PublicKey};

/// Errors from decoding raw identity material.
#[derive(Debug)]
pub enum IdentityError {
    /// The supplied bytes are not a valid Ed25519 public key.
    Decode(libp2p_identity::DecodingError),
    /// A `did:key` (or key) string/bytes are malformed (wrong length, multibase, or multicodec).
    Malformed(&'static str),
}

impl From<libp2p_identity::DecodingError> for IdentityError {
    fn from(e: libp2p_identity::DecodingError) -> Self {
        IdentityError::Decode(e)
    }
}

/// Wrap raw Ed25519 public-key bytes (32) into a libp2p [`PublicKey`].
pub fn ed25519_public(bytes: &[u8]) -> Result<PublicKey, IdentityError> {
    let ed = ed25519::PublicKey::try_from_bytes(bytes)?;
    Ok(PublicKey::from(ed))
}

/// Derive the canonical libp2p [`PeerId`] from raw Ed25519 public-key bytes.
pub fn ed25519_public_to_peer_id(bytes: &[u8]) -> Result<PeerId, IdentityError> {
    Ok(ed25519_public(bytes)?.to_peer_id())
}

/// A 333 DID is the PeerId's canonical base58btc string form.
pub fn ed25519_did(bytes: &[u8]) -> Result<String, IdentityError> {
    Ok(ed25519_public_to_peer_id(bytes)?.to_base58())
}

/// The multicodec varint prefix for an Ed25519 public key (`ed25519-pub` 0xed → varint `0xed 0x01`).
const ED25519_MULTICODEC: [u8; 2] = [0xed, 0x01];

/// Encode the SAME Ed25519 key as a W3C [`did:key`](https://w3c-ccg.github.io/did-method-key/)
/// (`did:key:z6Mk…`): the `ed25519-pub` multicodec prefix + the 32-byte key, base58btc with the
/// `z` multibase. One key, two DID encodings — the libp2p PeerId form ([`ed25519_did`]) and this
/// interoperable W3C form both decode to the same public key. The interop decision: DID stays
/// PeerId-canonical internally; `did:key` is the export/interop form, not a second identity.
pub fn ed25519_did_key(bytes: &[u8]) -> Result<String, IdentityError> {
    if bytes.len() != 32 {
        return Err(IdentityError::Malformed("ed25519 public key must be 32 bytes"));
    }
    let mut buf = Vec::with_capacity(34);
    buf.extend_from_slice(&ED25519_MULTICODEC);
    buf.extend_from_slice(bytes);
    Ok(format!("did:key:z{}", bs58::encode(buf).into_string()))
}

/// Recover the 32-byte Ed25519 public key from a `did:key:z…` string — the inverse of
/// [`ed25519_did_key`]. Rejects a non-`did:key:z` string, non-base58btc body, or a multicodec
/// that is not `ed25519-pub` (so a secp256k1 `did:key` can never sneak into the DID layer).
pub fn ed25519_from_did_key(did: &str) -> Result<[u8; 32], IdentityError> {
    let z = did
        .strip_prefix("did:key:z")
        .ok_or(IdentityError::Malformed("not a did:key:z… multibase string"))?;
    let raw = bs58::decode(z)
        .into_vec()
        .map_err(|_| IdentityError::Malformed("did:key body is not valid base58btc"))?;
    if raw.len() != 34 || raw[..2] != ED25519_MULTICODEC {
        return Err(IdentityError::Malformed("did:key is not an ed25519-pub multicodec"));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&raw[2..]);
    Ok(key)
}
