// KG: SPAN_333_Phase4_P2P_OS_MVP
// Meta-validation cycle #1
//
// Three modules, three atoms:
//   wnfs   — encrypted CRDT file put/get
//   ucan   — capability auth check
//   plan9  — 9P op → CRDT op translator
//
// All three are deliberately pure functions with no I/O so the Rust
// type system fully captures the Contract's pre/postconditions.

pub mod wnfs;
pub mod ucan;
pub mod plan9;

/// CID = 32-byte blake3 content hash.
pub type CID = [u8; 32];

/// Externally-provided symmetric encryption key (AES-256-GCM).
pub type EncryptionKey = [u8; 32];
