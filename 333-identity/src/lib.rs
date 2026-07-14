// KG: SPAN_333_Identity_KuboPort, plan-333-three-way-hybrid-2026-04-17
// 333 P2P OS L0 Identity layer — Ed25519 keypair + NodeID + KeyStore
// API shape inspired by IPFS Kubo KeyAPI; implementation uses ed25519-dalek.

#![forbid(unsafe_code)]

pub mod keypair;
pub mod keystore;
pub mod node_id;

pub use keypair::{Keypair, Signature, IdentityError};
pub use keystore::{InMemoryKeyStore, KeyStore};
pub use node_id::NodeId;
