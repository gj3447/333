// KG: TASK_ATOM_P4OS_WNFS
// KG: CONTRACT_ATOM_P4OS_WNFS_file_put_get
// KG: SPAN_333_Phase4_P2P_OS_MVP
//
// WNFS-lite: encrypted CRDT file put/get with external key.
// One namespace, one file, round-trip encrypt/decrypt.

use crate::{CID, EncryptionKey};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use thiserror::Error;

pub const MAX_FILE_SIZE: usize = 1024 * 1024;

#[derive(Debug, Error, PartialEq)]
pub enum WnfsError {
    #[error("decrypt failed (wrong key or corrupted ciphertext)")]
    DecryptFailed,
    #[error("invalid key length (must be 32 bytes)")]
    InvalidKey,
    #[error("invalid path (empty or non-ASCII)")]
    InvalidPath,
    #[error("size exceeded ({0} > {MAX_FILE_SIZE})")]
    SizeExceeded(usize),
    #[error("storage error: {0}")]
    StorageError(String),
}

/// Derive a deterministic 12-byte nonce from (ns, path).
/// AES-GCM nonces MUST NOT be reused with the same key; since WNFS-lite
/// treats (ns, path) as the storage slot, deriving the nonce from them
/// keeps a given (ns,path,key) round-trip reproducible.
fn derive_nonce(ns: &str, path: &str) -> [u8; 12] {
    let mut h = blake3::Hasher::new();
    h.update(b"wnfs-nonce-v1:");
    h.update(ns.as_bytes());
    h.update(b"/");
    h.update(path.as_bytes());
    let digest = h.finalize();
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&digest.as_bytes()[..12]);
    nonce
}

fn is_valid_path(s: &str) -> bool {
    !s.is_empty() && s.is_ascii()
}

/// Put `bytes` at (ns, path) under `key`. Returns content-addressed CID
/// over the ciphertext so the same inputs produce the same CID.
pub fn put(
    ns: &str,
    path: &str,
    bytes: &[u8],
    key: &EncryptionKey,
) -> Result<CID, WnfsError> {
    if !is_valid_path(ns) || !is_valid_path(path) {
        return Err(WnfsError::InvalidPath);
    }
    if bytes.is_empty() || bytes.len() > MAX_FILE_SIZE {
        return Err(WnfsError::SizeExceeded(bytes.len()));
    }

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce_bytes = derive_nonce(ns, path);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, bytes)
        .map_err(|_| WnfsError::DecryptFailed)?;

    // CID = blake3(ns || path || ciphertext) — deterministic over inputs.
    let mut h = blake3::Hasher::new();
    h.update(ns.as_bytes());
    h.update(b"/");
    h.update(path.as_bytes());
    h.update(b":");
    h.update(&ciphertext);
    let digest = h.finalize();
    let mut cid = [0u8; 32];
    cid.copy_from_slice(digest.as_bytes());

    // Side-effect: persist to the in-process store keyed by CID.
    STORE.with(|s| s.borrow_mut().insert(cid, (ns.to_string(), path.to_string(), ciphertext)));

    Ok(cid)
}

/// Retrieve and decrypt the file at `cid` with `key`.
pub fn get(cid: CID, key: &EncryptionKey) -> Result<Vec<u8>, WnfsError> {
    let (ns, path, ciphertext) = STORE.with(|s| {
        s.borrow()
            .get(&cid)
            .cloned()
            .ok_or_else(|| WnfsError::StorageError(format!("cid not found")))
    })?;

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce_bytes = derive_nonce(&ns, &path);
    let nonce = Nonce::from_slice(&nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| WnfsError::DecryptFailed)
}

// Thread-local in-memory store. MVP only — real WNFS uses DAG-CBOR over IPFS.
thread_local! {
    static STORE: std::cell::RefCell<std::collections::HashMap<CID, (String, String, Vec<u8>)>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> EncryptionKey {
        [7u8; 32]
    }

    #[test]
    fn test_wnfs_round_trip() {
        let key = test_key();
        let cid = put("ns", "path", b"hello world", &key).unwrap();
        let got = get(cid, &key).unwrap();
        assert_eq!(got, b"hello world");
    }

    #[test]
    fn test_wrong_key_fails() {
        let key = test_key();
        let wrong = [8u8; 32];
        let cid = put("ns", "path2", b"secret", &key).unwrap();
        let err = get(cid, &wrong).unwrap_err();
        assert_eq!(err, WnfsError::DecryptFailed);
    }

    #[test]
    fn test_determinism() {
        let key = test_key();
        // Same (ns, path, bytes, key) → same CID.
        // We isolate this from the round-trip test by using a fresh
        // put/get on a distinct path so the store is predictable.
        let cid1 = put("ns", "det", b"same", &key).unwrap();
        let cid2 = put("ns", "det", b"same", &key).unwrap();
        assert_eq!(cid1, cid2);
    }

    #[test]
    fn test_empty_input_rejected() {
        let key = test_key();
        assert!(matches!(
            put("ns", "p", b"", &key),
            Err(WnfsError::SizeExceeded(0))
        ));
    }

    #[test]
    fn test_invalid_path_rejected() {
        let key = test_key();
        assert!(matches!(
            put("", "p", b"x", &key),
            Err(WnfsError::InvalidPath)
        ));
    }
}
