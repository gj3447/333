// KG: TASK_333_B_BFTCrypto, CONTRACT_333_B_BFTCrypto
// KG: Taliban F3 fix — FNV placeholder → Ed25519 real signatures
// KG: lesson-333-bft-keyring-exchange-2026-04-14 — ValidatorKeyring::register_remote_key(node_id, verifying_key) 추가 대상
// KG: src-bft-crypto-register_remote_key (PLANNED)
//! BFT Consensus Cryptography — Ed25519 via crypto_real.rs
//!
//! Signature = 64-byte Ed25519 signature (NOT FNV hash).
//! sign() requires Identity (private key). verify() uses PeerId (public key).
//! ValidatorKeyring maps NodeId(u32) → Identity for BFT operations.

use crate::crypto_real::{Identity, PeerId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 32-bit node identity (kept for BFT internal indexing)
pub type NodeId = u32;

/// Ed25519 signature for BFT messages
/// KG: CONTRACT_333_B_BFTCrypto — no longer forgeable
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signature {
    pub signer: NodeId,
    /// 64-byte Ed25519 signature as bytes
    pub sig_bytes: Vec<u8>,
}

impl PartialEq for Signature {
    fn eq(&self, other: &Self) -> bool {
        self.signer == other.signer && self.sig_bytes == other.sig_bytes
    }
}
impl Eq for Signature {}
impl std::hash::Hash for Signature {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.signer.hash(state);
        self.sig_bytes.hash(state);
    }
}

/// Quorum Certificate — proof that 2f+1 nodes voted for a block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuorumCert {
    pub block_hash: u64,
    pub view: u64,
    pub signatures: Vec<Signature>,
}

impl QuorumCert {
    pub fn genesis() -> Self {
        Self { block_hash: 0, view: 0, signatures: vec![] }
    }

    pub fn has_quorum(&self, n: usize) -> bool {
        let f = (n - 1) / 3;
        let quorum = 2 * f + 1;
        let unique_signers: std::collections::HashSet<_> =
            self.signatures.iter().map(|s| s.signer).collect();
        unique_signers.len() >= quorum
    }
}

/// Simple block hash (SHA-256 would be better but FNV is fine for block ID)
pub fn hash_block(view: u64, parent_hash: u64, payload: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    h = h.wrapping_mul(0x100000001b3);
    h ^= view;
    h = h.wrapping_mul(0x100000001b3);
    h ^= parent_hash;
    for &b in payload {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Validator Keyring — maps NodeId → Identity for signing, PeerId for verification
/// KG: TASK_333_B_BFTCrypto, lesson-333-bft-keyring-exchange-2026-04-14
pub struct ValidatorKeyring {
    keys: HashMap<NodeId, Identity>,
    /// Remote peers: only public key stored (for verify, not sign)
    peer_ids: HashMap<NodeId, PeerId>,
}

impl Default for ValidatorKeyring {
    fn default() -> Self {
        Self::new()
    }
}

impl ValidatorKeyring {
    pub fn new() -> Self {
        Self { keys: HashMap::new(), peer_ids: HashMap::new() }
    }

    /// Register a validator with a RANDOM keypair (production)
    /// KG: Taliban BFTCrypto F1 fix — random key, not deterministic from node_id
    pub fn register_random(&mut self, node_id: NodeId) -> PeerId {
        let identity = Identity::generate();
        let peer_id = identity.peer_id();
        self.keys.insert(node_id, identity);
        peer_id
    }

    /// Register a validator with an existing Identity (from key exchange)
    pub fn register_identity(&mut self, node_id: NodeId, identity: Identity) {
        self.keys.insert(node_id, identity);
    }

    /// Register with deterministic seed — TESTING ONLY, NOT SECURE
    #[cfg(test)]
    pub fn register_deterministic(&mut self, node_id: NodeId) {
        let mut seed = [0u8; 32];
        seed[..4].copy_from_slice(&node_id.to_be_bytes());
        let identity = Identity::from_seed(&seed);
        self.keys.insert(node_id, identity);
    }

    /// Register a remote peer's public key only (for verification, not signing)
    /// KG: lesson-333-bft-keyring-exchange-2026-04-14
    pub fn register_peer_id(&mut self, node_id: NodeId, peer_id: PeerId) {
        self.peer_ids.insert(node_id, peer_id);
    }

    pub fn get(&self, node_id: &NodeId) -> Option<&Identity> {
        self.keys.get(node_id)
    }

    /// Get PeerId from either full Identity or remote-only PeerId
    pub fn peer_id(&self, node_id: &NodeId) -> Option<PeerId> {
        if let Some(id) = self.keys.get(node_id) {
            return Some(id.peer_id());
        }
        self.peer_ids.get(node_id).cloned()
    }

    /// Total registered peers (local keys + remote-only peer_ids)
    /// KG: seed-phase-B-sdk-test-page-2026-04-15 — diagnostics_json용
    pub fn peer_count(&self) -> usize {
        self.keys.len() + self.peer_ids.len()
    }
}

/// Sign a block hash with Ed25519
/// KG: CONTRACT_333_B_BFTCrypto — real Ed25519 signature, not forgeable
pub fn sign(node_id: NodeId, block_hash: u64, keyring: &ValidatorKeyring) -> Signature {
    let identity = keyring.get(&node_id).expect("validator not in keyring");
    let msg = block_hash.to_be_bytes();
    let signed = identity.sign(&msg);
    Signature {
        signer: node_id,
        sig_bytes: signed.signature_bytes()
            .map(|b| b.to_vec())
            .unwrap_or_default(),
    }
}

/// Verify a signature against a block hash
/// KG: CONTRACT_333_B_BFTCrypto — Ed25519 verify, forgery detected
pub fn verify(sig: &Signature, block_hash: u64, keyring: &ValidatorKeyring) -> bool {
    let peer_id = match keyring.peer_id(&sig.signer) {
        Some(p) => p,
        None => return false,
    };
    let msg = block_hash.to_be_bytes();
    // Reconstruct SignedMessage for verification
    let hex: String = sig.sig_bytes.iter().map(|b| format!("{:02x}", b)).collect();
    let signed_msg = crate::crypto_real::SignedMessage {
        signer: peer_id.clone(),
        signature_hex: hex,
        message: msg.to_vec(),
    };
    peer_id.verify(&signed_msg)
}

/// Sign with Identity directly (no keyring lookup needed)
/// KG: Taliban BFTCrypto F1 fix — caller must provide Identity (random key)
pub fn sign_with_identity(node_id: NodeId, block_hash: u64, identity: &Identity) -> Signature {
    let msg = block_hash.to_be_bytes();
    let signed = identity.sign(&msg);
    Signature {
        signer: node_id,
        sig_bytes: signed.signature_bytes()
            .map(|b| b.to_vec())
            .unwrap_or_default(),
    }
}

/// Verify with PeerId directly (no keyring lookup needed)
pub fn verify_with_peer_id(sig: &Signature, block_hash: u64, peer_id: &PeerId) -> bool {
    let msg = block_hash.to_be_bytes();
    let hex: String = sig.sig_bytes.iter().map(|b| format!("{:02x}", b)).collect();
    let signed_msg = crate::crypto_real::SignedMessage {
        signer: peer_id.clone(),
        signature_hex: hex,
        message: msg.to_vec(),
    };
    peer_id.verify(&signed_msg)
}

/// ⚠️ INSECURE — deterministic seed from node_id. For BFT state machine internal use only.
/// Production security is enforced at the WASM layer (wasm.rs uses sign_with_identity + random Identity).
/// Taliban BFTCrypto F1: This function exists because HotStuffState doesn't hold Identity.
/// TODO: Refactor HotStuffState to accept Identity, then remove this.
pub fn sign_standalone(node_id: NodeId, block_hash: u64) -> Signature {
    let mut seed = [0u8; 32];
    seed[..4].copy_from_slice(&node_id.to_be_bytes());
    sign_with_identity(node_id, block_hash, &Identity::from_seed(&seed))
}

/// ⚠️ INSECURE — same caveat as sign_standalone.
pub fn verify_standalone(sig: &Signature, block_hash: u64) -> bool {
    let mut seed = [0u8; 32];
    seed[..4].copy_from_slice(&sig.signer.to_be_bytes());
    let identity = Identity::from_seed(&seed);
    verify_with_peer_id(sig, block_hash, &identity.peer_id())
}

/// Validator set — tracks who the current validators are
#[derive(Debug, Clone)]
pub struct ValidatorSet {
    pub validators: Vec<NodeId>,
    index: HashMap<NodeId, usize>,
}

impl ValidatorSet {
    pub fn new(validators: Vec<NodeId>) -> Self {
        let index = validators.iter().enumerate().map(|(i, &id)| (id, i)).collect();
        Self { validators, index }
    }

    pub fn n(&self) -> usize { self.validators.len() }
    pub fn f(&self) -> usize { (self.n() - 1) / 3 }
    pub fn quorum_size(&self) -> usize { 2 * self.f() + 1 }
    pub fn contains(&self, id: &NodeId) -> bool { self.index.contains_key(id) }
    pub fn position(&self, id: &NodeId) -> Option<usize> { self.index.get(id).copied() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_keyring(nodes: &[NodeId]) -> ValidatorKeyring {
        let mut kr = ValidatorKeyring::new();
        for &n in nodes { kr.register_deterministic(n); }
        kr
    }

    #[test]
    fn sign_verify_roundtrip() {
        let kr = make_keyring(&[1, 2, 3]);
        let sig = sign(1, 12345, &kr);
        assert!(verify(&sig, 12345, &kr));
    }

    #[test]
    fn wrong_hash_fails() {
        let kr = make_keyring(&[1]);
        let sig = sign(1, 12345, &kr);
        assert!(!verify(&sig, 99999, &kr)); // wrong hash
    }

    #[test]
    fn forgery_detected() {
        // KG: CONTRACT_333_B_BFTCrypto — forgery test
        // Try to sign as node 2 using node 1's identity → verify as node 2 should fail
        let kr = make_keyring(&[1, 2]);
        let sig_by_1 = sign(1, 42, &kr);

        // Attacker tries: take sig from node 1, claim it's from node 2
        let forged = Signature {
            signer: 2, // lie about signer
            sig_bytes: sig_by_1.sig_bytes.clone(),
        };
        assert!(!verify(&forged, 42, &kr), "forgery must be detected");
    }

    #[test]
    fn standalone_sign_verify() {
        let sig = sign_standalone(5, 777);
        assert!(verify_standalone(&sig, 777));
        assert!(!verify_standalone(&sig, 888));
    }

    #[test]
    fn quorum_7_validators() {
        let vs = ValidatorSet::new(vec![1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(vs.n(), 7);
        assert_eq!(vs.f(), 2);
        assert_eq!(vs.quorum_size(), 5);
    }

    #[test]
    fn qc_quorum_check() {
        let kr = make_keyring(&[1, 2, 3, 4, 5, 6, 7]);
        let mut qc = QuorumCert::genesis();
        qc.signatures = vec![
            sign(1, 0, &kr), sign(2, 0, &kr), sign(3, 0, &kr),
            sign(4, 0, &kr), sign(5, 0, &kr),
        ];
        assert!(qc.has_quorum(7));

        qc.signatures = vec![sign(1, 0, &kr), sign(2, 0, &kr), sign(3, 0, &kr), sign(4, 0, &kr)];
        assert!(!qc.has_quorum(7));
    }

    #[test]
    fn hash_deterministic() {
        let h1 = hash_block(1, 0, b"hello");
        let h2 = hash_block(1, 0, b"hello");
        assert_eq!(h1, h2);
        assert_ne!(h1, hash_block(1, 0, b"world"));
    }
}
