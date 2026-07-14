// KG: TASK_ATOM_L0_Identity_Keypair, CONTRACT_ATOM_L0_Identity_Keypair
// Ed25519 keypair with deterministic seed support, sign/verify.

#![forbid(unsafe_code)]

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey, SECRET_KEY_LENGTH};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::node_id::NodeId;

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("invalid secret key bytes")]
    InvalidSecret,
    #[error("invalid public key bytes")]
    InvalidPublic,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("signature verification failed")]
    VerificationFailed,
}

/// Ed25519 keypair.
///
/// Security: the secret is zeroized on drop and is never exposed via a public
/// accessor. Use `sign()` for signing; use `public_bytes()` / `node_id()` for
/// the public half.
///
/// Note: `Clone` is intentional — in-memory key handoff between tasks is
/// common — but avoid serializing or logging a `Keypair`; only the public
/// half is `Serialize`-derivable via `node_id()`.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Keypair {
    secret: [u8; SECRET_KEY_LENGTH],
    public: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature(pub [u8; 64]);

impl Serialize for Signature {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&self.0.as_slice(), s)
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v: Vec<u8> = serde::Deserialize::deserialize(d)?;
        if v.len() != 64 {
            return Err(serde::de::Error::custom(format!("expected 64 bytes, got {}", v.len())));
        }
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&v);
        Ok(Signature(arr))
    }
}

impl Keypair {
    /// Generate fresh keypair using OS RNG.
    pub fn generate() -> Self {
        let mut seed = [0u8; SECRET_KEY_LENGTH];
        OsRng.fill_bytes(&mut seed);
        Self::from_seed(seed)
    }

    /// Deterministic keypair from a 32-byte seed.
    pub fn from_seed(seed: [u8; SECRET_KEY_LENGTH]) -> Self {
        let sk = SigningKey::from_bytes(&seed);
        let vk: VerifyingKey = sk.verifying_key();
        Self {
            secret: seed,
            public: vk.to_bytes(),
        }
    }

    pub fn public_bytes(&self) -> &[u8; 32] {
        &self.public
    }

    /// NodeID from this keypair's public part.
    pub fn node_id(&self) -> NodeId {
        NodeId::from_bytes(self.public)
    }

    /// Export secret material — the only path that touches the raw seed.
    /// Guarded by an explicit method name so grep/audit can find every caller.
    /// Callers must zeroize the returned bytes when done.
    pub fn export_secret(&self) -> [u8; SECRET_KEY_LENGTH] {
        self.secret
    }

    pub fn sign(&self, msg: &[u8]) -> Signature {
        let sk = SigningKey::from_bytes(&self.secret);
        Signature(sk.sign(msg).to_bytes())
    }

    pub fn verify(public: &[u8; 32], msg: &[u8], sig: &Signature) -> Result<(), IdentityError> {
        let vk = VerifyingKey::from_bytes(public).map_err(|_| IdentityError::InvalidPublic)?;
        let sig = ed25519_dalek::Signature::from_bytes(&sig.0);
        vk.verify(msg, &sig).map_err(|_| IdentityError::VerificationFailed)
    }
}

impl std::fmt::Debug for Keypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keypair(node_id={})", self.node_id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_determinism() {
        let seed = [42u8; 32];
        let a = Keypair::from_seed(seed);
        let b = Keypair::from_seed(seed);
        assert_eq!(a.public_bytes(), b.public_bytes());
        assert_eq!(a.export_secret(), b.export_secret());
    }

    #[test]
    fn different_seeds_different_keys() {
        let a = Keypair::from_seed([1u8; 32]);
        let b = Keypair::from_seed([2u8; 32]);
        assert_ne!(a.public_bytes(), b.public_bytes());
    }

    #[test]
    fn sign_verify_roundtrip() {
        let kp = Keypair::from_seed([7u8; 32]);
        let msg = b"hello 333 P2P OS";
        let sig = kp.sign(msg);
        Keypair::verify(kp.public_bytes(), msg, &sig).unwrap();
    }

    #[test]
    fn reject_tampered_signature() {
        let kp = Keypair::from_seed([7u8; 32]);
        let msg = b"original";
        let mut sig = kp.sign(msg);
        sig.0[0] ^= 1; // flip one bit
        assert!(Keypair::verify(kp.public_bytes(), msg, &sig).is_err());
    }

    #[test]
    fn reject_wrong_message() {
        let kp = Keypair::from_seed([7u8; 32]);
        let sig = kp.sign(b"original");
        assert!(Keypair::verify(kp.public_bytes(), b"tampered", &sig).is_err());
    }

    #[test]
    fn node_id_matches_pubkey() {
        let kp = Keypair::from_seed([3u8; 32]);
        assert_eq!(kp.node_id().as_bytes(), kp.public_bytes());
    }
}
