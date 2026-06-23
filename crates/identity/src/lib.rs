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
