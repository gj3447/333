// KG: SPAN_333_L4_Content, plan-333-three-way-hybrid-2026-04-17
// Content-addressed storage primitive. Cid = SHA-256 over bytes, base58-encoded.
// Pattern = content addressing (Kubo BlockAPI). Write returns Cid, read is idempotent.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContentError {
    #[error("block not found: {0}")]
    NotFound(Cid),
    #[error("integrity: stored hash does not match CID {0}")]
    Integrity(Cid),
    #[error("backend: {0}")]
    Backend(String),
}

/// Content Identifier = 32-byte SHA-256 digest. Self-verifying.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Cid(pub [u8; 32]);

impl Cid {
    pub fn of(bytes: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(bytes);
        let out = h.finalize();
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&out);
        Self(arr)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_base58(&self) -> String {
        bs58::encode(&self.0).into_string()
    }
}

impl std::fmt::Display for Cid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_base58())
    }
}
impl std::fmt::Debug for Cid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Cid({})", self.to_base58())
    }
}

pub trait BlockStore: Send + Sync {
    /// Store bytes, return its CID.
    fn put(&self, bytes: Vec<u8>) -> Result<Cid, ContentError>;
    /// Retrieve bytes by CID. Verifies integrity on the way out.
    fn get(&self, cid: &Cid) -> Result<Vec<u8>, ContentError>;
    fn has(&self, cid: &Cid) -> bool;
    fn len(&self) -> usize;
}

pub struct InMemoryBlockStore {
    inner: RwLock<HashMap<Cid, Vec<u8>>>,
}

impl Default for InMemoryBlockStore {
    fn default() -> Self {
        Self { inner: RwLock::new(HashMap::new()) }
    }
}

impl InMemoryBlockStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl BlockStore for InMemoryBlockStore {
    fn put(&self, bytes: Vec<u8>) -> Result<Cid, ContentError> {
        let cid = Cid::of(&bytes);
        let mut g = self.inner.write().map_err(|e| ContentError::Backend(e.to_string()))?;
        // Idempotent: same bytes → same cid → overwrite with identical content is a no-op.
        g.insert(cid, bytes);
        Ok(cid)
    }

    fn get(&self, cid: &Cid) -> Result<Vec<u8>, ContentError> {
        let g = self.inner.read().map_err(|e| ContentError::Backend(e.to_string()))?;
        let bytes = g.get(cid).cloned().ok_or(ContentError::NotFound(*cid))?;
        // Integrity check: recompute CID and compare. Detects silent backend corruption.
        let recomputed = Cid::of(&bytes);
        if recomputed != *cid {
            return Err(ContentError::Integrity(*cid));
        }
        Ok(bytes)
    }

    fn has(&self, cid: &Cid) -> bool {
        self.inner.read().map(|g| g.contains_key(cid)).unwrap_or(false)
    }

    fn len(&self) -> usize {
        self.inner.read().map(|g| g.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cid_is_deterministic() {
        let a = Cid::of(b"hello");
        let b = Cid::of(b"hello");
        assert_eq!(a, b);
    }

    #[test]
    fn different_bytes_different_cid() {
        assert_ne!(Cid::of(b"hello"), Cid::of(b"world"));
    }

    #[test]
    fn put_then_get_roundtrip() {
        let bs = InMemoryBlockStore::new();
        let cid = bs.put(b"payload".to_vec()).unwrap();
        let back = bs.get(&cid).unwrap();
        assert_eq!(back, b"payload");
    }

    #[test]
    fn get_missing_errors() {
        let bs = InMemoryBlockStore::new();
        let ghost = Cid::of(b"nothing");
        assert!(matches!(bs.get(&ghost), Err(ContentError::NotFound(_))));
    }

    #[test]
    fn put_is_idempotent_same_cid() {
        let bs = InMemoryBlockStore::new();
        let c1 = bs.put(b"x".to_vec()).unwrap();
        let c2 = bs.put(b"x".to_vec()).unwrap();
        assert_eq!(c1, c2);
        assert_eq!(bs.len(), 1);
    }

    #[test]
    fn has_reflects_state() {
        let bs = InMemoryBlockStore::new();
        let cid = Cid::of(b"y");
        assert!(!bs.has(&cid));
        bs.put(b"y".to_vec()).unwrap();
        assert!(bs.has(&cid));
    }

    #[test]
    fn cid_base58_display() {
        let cid = Cid::of(b"xyz");
        let s = cid.to_base58();
        assert!(!s.is_empty());
        assert_eq!(format!("{cid}"), s);
    }
}
