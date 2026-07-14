// KG: SPAN_333_Identity, SPAN_333_BFT_Crypto
//! Real Ed25519 cryptography for 333 Platform.
//! Uses ed25519-compact — small, self-contained, WASM-friendly.
//!
//! This replaces the placeholder crypto in bft/crypto.rs for production.
//! Keypair: 32-byte seed → 64-byte keypair (32 secret + 32 public)
//! Signature: 64 bytes
//! Verify: O(1), no external dependencies

use ed25519_compact::{KeyPair, PublicKey, SecretKey, Signature, Seed, Noise};
use serde::{Deserialize, Serialize};

/// A 333 identity — Ed25519 keypair
#[derive(Clone)]
pub struct Identity {
    keypair: KeyPair,
}

/// Public identity (shareable)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerId(pub [u8; 32]);

/// A cryptographic signature
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedMessage {
    pub signer: PeerId,
    /// 64-byte Ed25519 signature as hex string (128 chars)
    pub signature_hex: String,
    pub message: Vec<u8>,
}

impl SignedMessage {
    pub fn signature_bytes(&self) -> Option<[u8; 64]> {
        let bytes: Vec<u8> = (0..self.signature_hex.len())
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(&self.signature_hex[i..i + 2], 16).ok())
            .collect();
        if bytes.len() == 64 {
            let mut arr = [0u8; 64];
            arr.copy_from_slice(&bytes);
            Some(arr)
        } else {
            None
        }
    }
}

impl Identity {
    /// Generate a new random identity
    pub fn generate() -> Self {
        let keypair = KeyPair::generate();
        Self { keypair }
    }

    /// Create from a 32-byte seed (deterministic)
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let seed = Seed::from_slice(seed).expect("seed must be 32 bytes");
        let keypair = KeyPair::from_seed(seed);
        Self { keypair }
    }

    /// Get this identity's public peer ID
    pub fn peer_id(&self) -> PeerId {
        let mut id = [0u8; 32];
        id.copy_from_slice(self.keypair.pk.as_ref());
        PeerId(id)
    }

    /// Sign a message
    pub fn sign(&self, message: &[u8]) -> SignedMessage {
        let sig = self.keypair.sk.sign(message, Some(Noise::generate()));
        let hex: String = sig.as_ref().iter().map(|b| format!("{:02x}", b)).collect();
        SignedMessage {
            signer: self.peer_id(),
            signature_hex: hex,
            message: message.to_vec(),
        }
    }

    /// Export secret key as hex string (for backup)
    pub fn secret_hex(&self) -> String {
        self.keypair.sk.as_ref().iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Import from secret key hex string
    pub fn from_secret_hex(hex: &str) -> Option<Self> {
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
            .collect();
        let sk = SecretKey::from_slice(&bytes).ok()?;
        let pk = sk.public_key();
        Some(Self { keypair: KeyPair { sk, pk } })
    }
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Identity({:?})", self.peer_id())
    }
}

impl PeerId {
    /// Verify a signed message from this peer
    pub fn verify(&self, signed: &SignedMessage) -> bool {
        if signed.signer != *self {
            return false;
        }
        let pk = match PublicKey::from_slice(&self.0) {
            Ok(pk) => pk,
            Err(_) => return false,
        };
        let sig_bytes = match signed.signature_bytes() {
            Some(b) => b,
            None => return false,
        };
        let sig = match Signature::from_slice(&sig_bytes) {
            Ok(sig) => sig,
            Err(_) => return false,
        };
        pk.verify(&signed.message, &sig).is_ok()
    }

    /// Short hex representation (first 8 bytes)
    pub fn short_hex(&self) -> String {
        self.0[..4].iter().map(|b| format!("{:02x}", b)).collect()
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "peer-{}", self.short_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_and_sign() {
        let id = Identity::generate();
        let msg = b"hello 333";
        let signed = id.sign(msg);

        assert!(id.peer_id().verify(&signed), "valid signature must verify");
    }

    #[test]
    fn tampered_message_fails() {
        let id = Identity::generate();
        let mut signed = id.sign(b"original");
        signed.message = b"tampered".to_vec();

        assert!(!id.peer_id().verify(&signed), "tampered message must fail");
    }

    #[test]
    fn wrong_signer_fails() {
        let id1 = Identity::generate();
        let id2 = Identity::generate();

        let signed = id1.sign(b"hello");
        assert!(!id2.peer_id().verify(&signed), "wrong signer must fail");
    }

    #[test]
    fn deterministic_from_seed() {
        let seed = [42u8; 32];
        let id1 = Identity::from_seed(&seed);
        let id2 = Identity::from_seed(&seed);

        assert_eq!(id1.peer_id(), id2.peer_id(), "same seed = same identity");
    }

    #[test]
    fn different_seeds_different_ids() {
        let id1 = Identity::from_seed(&[1u8; 32]);
        let id2 = Identity::from_seed(&[2u8; 32]);

        assert_ne!(id1.peer_id(), id2.peer_id());
    }

    #[test]
    fn export_import_roundtrip() {
        let id = Identity::generate();
        let hex = id.secret_hex();
        let restored = Identity::from_secret_hex(&hex).unwrap();

        assert_eq!(id.peer_id(), restored.peer_id());

        // Sign with original, verify with restored
        let signed = id.sign(b"test");
        assert!(restored.peer_id().verify(&signed));
    }

    #[test]
    fn peer_id_display() {
        let id = Identity::from_seed(&[0xABu8; 32]);
        let display = format!("{}", id.peer_id());
        assert!(display.starts_with("peer-"), "display format: {}", display);
    }

    #[test]
    fn signature_size() {
        let id = Identity::generate();
        let signed = id.sign(b"test");
        assert_eq!(signed.signature_hex.len(), 128, "Ed25519 signature hex must be 128 chars (64 bytes)");
        assert!(signed.signature_bytes().is_some());
    }
}
