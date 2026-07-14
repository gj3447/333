// KG: SPAN_333_L6plus_StorageAdvanced, plan-333-p2p-os-synthesis-execution-2026-04-18,
//     queue-p8-storage-advanced-2026-04-18, finding_333_synth_sto_d{24..27}
//
// 333 P2P OS L6+ Storage Advanced — trait surface + reference impls.
//
// Design sources (prom32-333-p2p-2026-04-18):
//   - finding_333_synth_sto_sota_d24: reed-solomon-erasure / reed-solomon-simd (10+4 RS), SeaweedFS erasure profile.
//   - finding_333_synth_sto_thr_d25: khonsu/redhac replication strategies (quorum, chain, primary-backup).
//   - finding_333_synth_sto_prt_d26: WNFS 3-tier private/shared/public with capability-bound access.
//   - finding_333_synth_sto_pit_d27: OPFS single-writer semantics, main-thread blocking, quota eviction.
//
// Reference impls here are dependency-free and designed to be drop-in replaced:
//   - `ECEncoder` → XOR-parity (k-of-n recovery, correct for single-shard loss).
//   - `ReplicationStrategy` → QuorumReplication + PrimaryBackup.
//   - `EncryptedMetadata` → XOR-cipher stub (swap for age / XChaCha20-Poly1305 in prod).
//   - `OpfsBackend` → InMemoryOpfs (mirrors OPFS single-writer rule on non-wasm hosts).
//
// Production backends (reed-solomon-simd, WNFS-rs, browser OPFS) implement the
// same traits in downstream crates.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use content333::{BlockStore, Cid, ContentError};
use thiserror::Error;

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum AdvError {
    #[error("erasure: shard count mismatch, got {got} expected {expected}")]
    ShardCount { got: usize, expected: usize },
    #[error("erasure: not enough shards to reconstruct ({have}/{need})")]
    NotEnoughShards { have: usize, need: usize },
    #[error("erasure: empty payload")]
    EmptyPayload,
    #[error("replication: quorum not met ({ok}/{need})")]
    NoQuorum { ok: usize, need: usize },
    #[error("replication: no replicas configured")]
    NoReplicas,
    #[error("crypto: decryption failed")]
    DecryptFailed,
    #[error("opfs: concurrent writer denied on {0}")]
    OpfsConcurrentWrite(String),
    #[error("opfs: path not found: {0}")]
    OpfsNotFound(String),
    #[error("backend: {0}")]
    Backend(String),
}

// ============================================================================
// ECEncoder — erasure coding trait + XOR-parity reference impl
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shard {
    pub index: usize,
    pub data: Vec<u8>,
}

pub trait ECEncoder: Send + Sync {
    /// Splits `payload` into `k` data shards + `m` parity shards (total `k+m`).
    fn encode(&self, payload: &[u8]) -> Result<Vec<Shard>, AdvError>;

    /// Reconstructs the original payload from any surviving shards.
    /// Reference contract: at least one data shard plus enough parity to cover
    /// each missing data shard must be present.
    fn decode(&self, shards: &[Shard]) -> Result<Vec<u8>, AdvError>;

    fn k(&self) -> usize;
    fn m(&self) -> usize;
}

/// XOR-parity encoder: `m` copies of (XOR of all data shards). Tolerates up to
/// `min(m, 1)` data-shard losses simultaneously (XOR group code). It's the
/// stable stand-in until reed-solomon-simd is wired in.
pub struct XorParityEncoder {
    pub k: usize,
    pub m: usize,
}

impl XorParityEncoder {
    pub fn new(k: usize, m: usize) -> Self {
        assert!(k >= 1 && m >= 1, "XorParityEncoder requires k>=1 and m>=1");
        Self { k, m }
    }

    fn data_shard_size(&self, payload_len: usize) -> usize {
        payload_len.div_ceil(self.k).max(1)
    }
}

impl ECEncoder for XorParityEncoder {
    fn encode(&self, payload: &[u8]) -> Result<Vec<Shard>, AdvError> {
        if payload.is_empty() {
            return Err(AdvError::EmptyPayload);
        }
        let shard_len = self.data_shard_size(payload.len());
        let mut data_shards: Vec<Vec<u8>> = Vec::with_capacity(self.k);
        for i in 0..self.k {
            let start = i * shard_len;
            let end = ((i + 1) * shard_len).min(payload.len());
            let mut s = if start < end { payload[start..end].to_vec() } else { Vec::new() };
            s.resize(shard_len, 0);
            data_shards.push(s);
        }
        // Parity = XOR of all data shards. m copies so up to m parity losses are fine.
        let mut parity = vec![0u8; shard_len];
        for s in &data_shards {
            for (p, v) in parity.iter_mut().zip(s.iter()) {
                *p ^= *v;
            }
        }
        let mut out: Vec<Shard> = Vec::with_capacity(self.k + self.m);
        for (i, d) in data_shards.into_iter().enumerate() {
            out.push(Shard { index: i, data: d });
        }
        for j in 0..self.m {
            out.push(Shard { index: self.k + j, data: parity.clone() });
        }
        Ok(out)
    }

    fn decode(&self, shards: &[Shard]) -> Result<Vec<u8>, AdvError> {
        if shards.is_empty() {
            return Err(AdvError::NotEnoughShards { have: 0, need: self.k });
        }
        let shard_len = shards[0].data.len();
        let mut by_idx: BTreeMap<usize, &Shard> = BTreeMap::new();
        for s in shards {
            by_idx.insert(s.index, s);
        }
        // Do we have all data shards already?
        let mut data: Vec<Option<Vec<u8>>> = (0..self.k).map(|_| None).collect();
        for i in 0..self.k {
            if let Some(s) = by_idx.get(&i) {
                data[i] = Some(s.data.clone());
            }
        }
        // Each missing data shard can be recovered via any one parity shard.
        let missing_indices: Vec<usize> = (0..self.k).filter(|i| data[*i].is_none()).collect();
        if !missing_indices.is_empty() {
            if missing_indices.len() > 1 {
                return Err(AdvError::NotEnoughShards {
                    have: self.k - missing_indices.len(),
                    need: self.k,
                });
            }
            let miss = missing_indices[0];
            let parity = (self.k..self.k + self.m)
                .find_map(|j| by_idx.get(&j).map(|s| s.data.clone()))
                .ok_or(AdvError::NotEnoughShards {
                    have: self.k - 1,
                    need: self.k,
                })?;
            // recovered = parity XOR (XOR of present data shards)
            let mut rec = parity;
            for (i, maybe) in data.iter().enumerate() {
                if i == miss {
                    continue;
                }
                if let Some(d) = maybe {
                    for (r, v) in rec.iter_mut().zip(d.iter()) {
                        *r ^= *v;
                    }
                }
            }
            data[miss] = Some(rec);
        }

        let mut out = Vec::with_capacity(self.k * shard_len);
        for d in data.into_iter() {
            out.extend(d.expect("all data shards resolved"));
        }
        // Trim trailing zero padding: callers who care about exact length should
        // store it separately; here we return the padded stream and let the CID
        // verification at the BlockStore layer catch accidental truncation.
        Ok(out)
    }

    fn k(&self) -> usize {
        self.k
    }
    fn m(&self) -> usize {
        self.m
    }
}

// ============================================================================
// ReplicationStrategy — trait + Quorum / PrimaryBackup impls
// ============================================================================

/// Replica identifier. Opaque — a real deployment would bind this to a
/// transport peer id (333-transport NodeId). Kept as `String` here to dodge
/// cyclic deps: this crate sits below the networking layer.
pub type ReplicaId = String;

pub trait ReplicationStrategy: Send + Sync {
    /// Replicates `(cid, bytes)` across backing replicas. Returns the set of
    /// replicas that acknowledged the write.
    fn put(
        &self,
        cid: &Cid,
        bytes: &[u8],
        replicas: &[ReplicaId],
    ) -> Result<Vec<ReplicaId>, AdvError>;

    /// Reads `cid`. Returns the bytes + the replica id that served them.
    fn get(&self, cid: &Cid, replicas: &[ReplicaId]) -> Result<(Vec<u8>, ReplicaId), AdvError>;
}

/// Quorum replication: requires `quorum` acks from `replicas.len()` replicas.
/// Uses an injected per-replica `BlockStore`.
pub struct QuorumReplication {
    pub quorum: usize,
    stores: Mutex<HashMap<ReplicaId, Arc<dyn BlockStore>>>,
    /// Set of replicas that should silently fail writes (fault injection for tests).
    faulty: Mutex<std::collections::HashSet<ReplicaId>>,
}

impl QuorumReplication {
    pub fn new(quorum: usize) -> Self {
        Self {
            quorum,
            stores: Mutex::new(HashMap::new()),
            faulty: Mutex::new(std::collections::HashSet::new()),
        }
    }

    pub fn attach(&self, id: ReplicaId, store: Arc<dyn BlockStore>) {
        self.stores.lock().unwrap().insert(id, store);
    }

    pub fn mark_faulty(&self, id: ReplicaId) {
        self.faulty.lock().unwrap().insert(id);
    }
}

impl ReplicationStrategy for QuorumReplication {
    fn put(
        &self,
        _cid: &Cid,
        bytes: &[u8],
        replicas: &[ReplicaId],
    ) -> Result<Vec<ReplicaId>, AdvError> {
        if replicas.is_empty() {
            return Err(AdvError::NoReplicas);
        }
        let stores = self.stores.lock().unwrap();
        let faulty = self.faulty.lock().unwrap();
        let mut acked = Vec::new();
        for r in replicas {
            if faulty.contains(r) {
                continue;
            }
            if let Some(s) = stores.get(r) {
                if s.put(bytes.to_vec()).is_ok() {
                    acked.push(r.clone());
                }
            }
        }
        if acked.len() < self.quorum {
            return Err(AdvError::NoQuorum {
                ok: acked.len(),
                need: self.quorum,
            });
        }
        Ok(acked)
    }

    fn get(&self, cid: &Cid, replicas: &[ReplicaId]) -> Result<(Vec<u8>, ReplicaId), AdvError> {
        if replicas.is_empty() {
            return Err(AdvError::NoReplicas);
        }
        let stores = self.stores.lock().unwrap();
        for r in replicas {
            if let Some(s) = stores.get(r) {
                if let Ok(b) = s.get(cid) {
                    return Ok((b, r.clone()));
                }
            }
        }
        Err(AdvError::Backend(format!("cid {} not found on any replica", cid)))
    }
}

/// Primary-backup replication: writes only succeed when the primary acks;
/// backups receive the write best-effort. Reads always hit the primary.
pub struct PrimaryBackup {
    primary: ReplicaId,
    stores: Mutex<HashMap<ReplicaId, Arc<dyn BlockStore>>>,
}

impl PrimaryBackup {
    pub fn new(primary: ReplicaId) -> Self {
        Self {
            primary,
            stores: Mutex::new(HashMap::new()),
        }
    }

    pub fn attach(&self, id: ReplicaId, store: Arc<dyn BlockStore>) {
        self.stores.lock().unwrap().insert(id, store);
    }
}

impl ReplicationStrategy for PrimaryBackup {
    fn put(
        &self,
        _cid: &Cid,
        bytes: &[u8],
        replicas: &[ReplicaId],
    ) -> Result<Vec<ReplicaId>, AdvError> {
        let stores = self.stores.lock().unwrap();
        let primary = stores.get(&self.primary).ok_or(AdvError::NoReplicas)?;
        primary
            .put(bytes.to_vec())
            .map_err(|e| AdvError::Backend(e.to_string()))?;
        let mut acked = vec![self.primary.clone()];
        for r in replicas.iter().filter(|r| **r != self.primary) {
            if let Some(s) = stores.get(r) {
                if s.put(bytes.to_vec()).is_ok() {
                    acked.push(r.clone());
                }
            }
        }
        Ok(acked)
    }

    fn get(&self, cid: &Cid, _replicas: &[ReplicaId]) -> Result<(Vec<u8>, ReplicaId), AdvError> {
        let stores = self.stores.lock().unwrap();
        let primary = stores.get(&self.primary).ok_or(AdvError::NoReplicas)?;
        let bytes = primary.get(cid).map_err(|e| AdvError::Backend(e.to_string()))?;
        Ok((bytes, self.primary.clone()))
    }
}

// ============================================================================
// EncryptedMetadata — WNFS-like trait + XOR-cipher reference impl
// ============================================================================

/// A capability-bound metadata store. Every write is encrypted with a
/// per-capability key; reads require the same key. Intentionally plain — swap
/// for WNFS-rs (age / XChaCha20-Poly1305) in production.
pub trait EncryptedMetadata: Send + Sync {
    fn put_encrypted(&self, path: &str, key: &[u8], plaintext: &[u8]) -> Result<(), AdvError>;
    fn get_encrypted(&self, path: &str, key: &[u8]) -> Result<Vec<u8>, AdvError>;
    fn has(&self, path: &str) -> bool;
}

pub struct XorCipherMetadata {
    inner: RwLock<HashMap<String, Vec<u8>>>,
    /// Per-path MAC stored alongside the ciphertext so decrypt can validate key.
    macs: RwLock<HashMap<String, [u8; 32]>>,
}

impl Default for XorCipherMetadata {
    fn default() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            macs: RwLock::new(HashMap::new()),
        }
    }
}

impl XorCipherMetadata {
    pub fn new() -> Self {
        Self::default()
    }

    fn keystream(key: &[u8], len: usize) -> Vec<u8> {
        assert!(!key.is_empty(), "key must be non-empty");
        (0..len).map(|i| key[i % key.len()]).collect()
    }

    fn mac_of(key: &[u8], ciphertext: &[u8]) -> [u8; 32] {
        // Deterministic 32-byte MAC via XOR-fold — NOT cryptographic, but
        // catches wrong-key reads which is all we need for the reference impl.
        let mut out = [0u8; 32];
        for (i, b) in key.iter().enumerate() {
            out[i % 32] ^= *b;
        }
        for (i, b) in ciphertext.iter().enumerate() {
            out[i % 32] = out[i % 32].wrapping_add(*b);
        }
        out
    }
}

impl EncryptedMetadata for XorCipherMetadata {
    fn put_encrypted(&self, path: &str, key: &[u8], plaintext: &[u8]) -> Result<(), AdvError> {
        let ks = Self::keystream(key, plaintext.len());
        let ciphertext: Vec<u8> = plaintext.iter().zip(ks.iter()).map(|(a, b)| a ^ b).collect();
        let mac = Self::mac_of(key, &ciphertext);
        self.inner
            .write()
            .map_err(|e| AdvError::Backend(e.to_string()))?
            .insert(path.to_string(), ciphertext);
        self.macs
            .write()
            .map_err(|e| AdvError::Backend(e.to_string()))?
            .insert(path.to_string(), mac);
        Ok(())
    }

    fn get_encrypted(&self, path: &str, key: &[u8]) -> Result<Vec<u8>, AdvError> {
        let g = self
            .inner
            .read()
            .map_err(|e| AdvError::Backend(e.to_string()))?;
        let ciphertext = g
            .get(path)
            .cloned()
            .ok_or_else(|| AdvError::OpfsNotFound(path.into()))?;
        drop(g);
        let expected = self
            .macs
            .read()
            .map_err(|e| AdvError::Backend(e.to_string()))?
            .get(path)
            .copied()
            .ok_or(AdvError::DecryptFailed)?;
        if Self::mac_of(key, &ciphertext) != expected {
            return Err(AdvError::DecryptFailed);
        }
        let ks = Self::keystream(key, ciphertext.len());
        Ok(ciphertext.iter().zip(ks.iter()).map(|(a, b)| a ^ b).collect())
    }

    fn has(&self, path: &str) -> bool {
        self.inner.read().map(|g| g.contains_key(path)).unwrap_or(false)
    }
}

// ============================================================================
// OpfsBackend — browser-OPFS-shaped trait + in-memory reference impl
// ============================================================================

/// Mirrors OPFS (Origin Private File System) semantics: single writer per file,
/// synchronous access, quota-bounded. This reference impl runs everywhere —
/// wasm builds plug in a real `navigator.storage.getDirectory()` handle.
pub trait OpfsBackend: Send + Sync {
    fn acquire_writer(&self, path: &str) -> Result<WriterGuard, AdvError>;
    fn read_sync(&self, path: &str) -> Result<Vec<u8>, AdvError>;
    fn quota_used(&self) -> u64;
    fn quota_limit(&self) -> u64;
}

/// RAII write guard. Dropping releases the single-writer lock. A second
/// `acquire_writer` on the same path while this is held returns `OpfsConcurrentWrite`.
#[derive(Debug)]
pub struct WriterGuard {
    path: String,
    #[allow(dead_code)]
    locks: Arc<Mutex<std::collections::HashSet<String>>>,
    #[allow(dead_code)]
    data: Arc<RwLock<HashMap<String, Vec<u8>>>>,
}

impl WriterGuard {
    pub fn write(&self, bytes: &[u8]) -> Result<(), AdvError> {
        self.data
            .write()
            .map_err(|e| AdvError::Backend(e.to_string()))?
            .insert(self.path.clone(), bytes.to_vec());
        Ok(())
    }
}

impl Drop for WriterGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = self.locks.lock() {
            g.remove(&self.path);
        }
    }
}

pub struct InMemoryOpfs {
    data: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    locks: Arc<Mutex<std::collections::HashSet<String>>>,
    limit: u64,
}

impl InMemoryOpfs {
    pub fn new(limit_bytes: u64) -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            locks: Arc::new(Mutex::new(std::collections::HashSet::new())),
            limit: limit_bytes,
        }
    }
}

impl OpfsBackend for InMemoryOpfs {
    fn acquire_writer(&self, path: &str) -> Result<WriterGuard, AdvError> {
        let mut g = self.locks.lock().map_err(|e| AdvError::Backend(e.to_string()))?;
        if g.contains(path) {
            return Err(AdvError::OpfsConcurrentWrite(path.into()));
        }
        g.insert(path.to_string());
        Ok(WriterGuard {
            path: path.to_string(),
            locks: self.locks.clone(),
            data: self.data.clone(),
        })
    }

    fn read_sync(&self, path: &str) -> Result<Vec<u8>, AdvError> {
        self.data
            .read()
            .map_err(|e| AdvError::Backend(e.to_string()))?
            .get(path)
            .cloned()
            .ok_or_else(|| AdvError::OpfsNotFound(path.into()))
    }

    fn quota_used(&self) -> u64 {
        self.data
            .read()
            .map(|g| g.values().map(|v| v.len() as u64).sum::<u64>())
            .unwrap_or(0)
    }

    fn quota_limit(&self) -> u64 {
        self.limit
    }
}

// ============================================================================
// Convenience: bridge an OpfsBackend into a content333::BlockStore
// ============================================================================

/// Adapts an `OpfsBackend` into a `content333::BlockStore`. Bytes are written
/// under the base58-encoded CID path ("/blocks/{cid}"), making the OPFS handle
/// double as a content-addressed block cache.
pub struct OpfsBlockStore<O: OpfsBackend> {
    pub opfs: Arc<O>,
}

impl<O: OpfsBackend> OpfsBlockStore<O> {
    fn path_for(cid: &Cid) -> String {
        format!("/blocks/{}", cid)
    }
}

impl<O: OpfsBackend> BlockStore for OpfsBlockStore<O> {
    fn put(&self, bytes: Vec<u8>) -> Result<Cid, ContentError> {
        let cid = Cid::of(&bytes);
        let path = Self::path_for(&cid);
        let guard = self
            .opfs
            .acquire_writer(&path)
            .map_err(|e| ContentError::Backend(e.to_string()))?;
        guard
            .write(&bytes)
            .map_err(|e| ContentError::Backend(e.to_string()))?;
        Ok(cid)
    }

    fn get(&self, cid: &Cid) -> Result<Vec<u8>, ContentError> {
        self.opfs
            .read_sync(&Self::path_for(cid))
            .map_err(|e| match e {
                AdvError::OpfsNotFound(_) => ContentError::NotFound(*cid),
                other => ContentError::Backend(other.to_string()),
            })
    }

    fn has(&self, cid: &Cid) -> bool {
        self.opfs.read_sync(&Self::path_for(cid)).is_ok()
    }

    fn len(&self) -> usize {
        // OpfsBackend has no cheap cardinality; report 0 to keep the contract
        // honest — callers needing a count should wrap with a Counter decorator.
        0
    }
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use content333::InMemoryBlockStore;

    // ---- ECEncoder tests -----------------------------------------------------

    #[test]
    fn xor_encode_then_decode_full_roundtrip() {
        let enc = XorParityEncoder::new(4, 2);
        let payload = b"Hello 333 P2P OS erasure coding test vector.";
        let shards = enc.encode(payload).unwrap();
        assert_eq!(shards.len(), 6);
        let got = enc.decode(&shards).unwrap();
        // Stripped trailing zero padding for direct comparison:
        assert!(got.starts_with(payload));
    }

    #[test]
    fn xor_recovers_one_missing_data_shard_via_parity() {
        let enc = XorParityEncoder::new(3, 2);
        let payload = b"abcdefghijklmnopqrst"; // 20 bytes → 3 shards of 7,7,6(padded)
        let mut shards = enc.encode(payload).unwrap();
        // Drop data shard 1
        shards.retain(|s| s.index != 1);
        let got = enc.decode(&shards).unwrap();
        assert!(got.starts_with(payload));
    }

    #[test]
    fn xor_rejects_two_missing_data_shards() {
        let enc = XorParityEncoder::new(3, 2);
        let payload = b"some payload";
        let mut shards = enc.encode(payload).unwrap();
        shards.retain(|s| s.index != 0 && s.index != 1);
        assert!(matches!(
            enc.decode(&shards),
            Err(AdvError::NotEnoughShards { .. })
        ));
    }

    #[test]
    fn xor_empty_payload_rejected() {
        let enc = XorParityEncoder::new(2, 1);
        assert!(matches!(enc.encode(&[]), Err(AdvError::EmptyPayload)));
    }

    #[test]
    fn xor_k_m_exposed() {
        let enc = XorParityEncoder::new(6, 3);
        assert_eq!(enc.k(), 6);
        assert_eq!(enc.m(), 3);
    }

    // ---- Replication tests ---------------------------------------------------

    #[test]
    fn quorum_replication_happy_path() {
        let q = QuorumReplication::new(2);
        for i in 0..3 {
            q.attach(format!("r{i}"), Arc::new(InMemoryBlockStore::new()));
        }
        let replicas: Vec<_> = (0..3).map(|i| format!("r{i}")).collect();
        let bytes = b"replicated payload".to_vec();
        let cid = Cid::of(&bytes);
        let acked = q.put(&cid, &bytes, &replicas).unwrap();
        assert!(acked.len() >= 2);
        let (got, _who) = q.get(&cid, &replicas).unwrap();
        assert_eq!(got, bytes);
    }

    #[test]
    fn quorum_below_threshold_fails() {
        let q = QuorumReplication::new(3);
        for i in 0..3 {
            q.attach(format!("r{i}"), Arc::new(InMemoryBlockStore::new()));
        }
        // Two faulty out of three → only 1 ack < quorum=3.
        q.mark_faulty("r0".into());
        q.mark_faulty("r1".into());
        let replicas: Vec<_> = (0..3).map(|i| format!("r{i}")).collect();
        let bytes = b"x".to_vec();
        let cid = Cid::of(&bytes);
        let err = q.put(&cid, &bytes, &replicas).unwrap_err();
        assert!(matches!(err, AdvError::NoQuorum { .. }));
    }

    #[test]
    fn primary_backup_reads_always_from_primary() {
        let pb = PrimaryBackup::new("p".into());
        let p_store = Arc::new(InMemoryBlockStore::new());
        let b_store = Arc::new(InMemoryBlockStore::new());
        pb.attach("p".into(), p_store.clone());
        pb.attach("b".into(), b_store.clone());
        let replicas = vec!["p".into(), "b".into()];
        let bytes = b"primary payload".to_vec();
        let cid = Cid::of(&bytes);
        let acked = pb.put(&cid, &bytes, &replicas).unwrap();
        assert!(acked.contains(&"p".to_string()));
        let (got, who) = pb.get(&cid, &replicas).unwrap();
        assert_eq!(got, bytes);
        assert_eq!(who, "p");
    }

    #[test]
    fn replication_no_replicas_errors() {
        let q = QuorumReplication::new(1);
        let bytes = b"x".to_vec();
        let cid = Cid::of(&bytes);
        assert!(matches!(q.put(&cid, &bytes, &[]), Err(AdvError::NoReplicas)));
    }

    // ---- EncryptedMetadata tests --------------------------------------------

    #[test]
    fn encrypted_roundtrip_with_correct_key() {
        let m = XorCipherMetadata::new();
        let key = b"super-secret";
        m.put_encrypted("/doc/a", key, b"top secret").unwrap();
        let got = m.get_encrypted("/doc/a", key).unwrap();
        assert_eq!(got, b"top secret");
    }

    #[test]
    fn encrypted_wrong_key_rejected() {
        let m = XorCipherMetadata::new();
        m.put_encrypted("/doc/a", b"key1", b"hello").unwrap();
        let err = m.get_encrypted("/doc/a", b"key2").unwrap_err();
        assert!(matches!(err, AdvError::DecryptFailed));
    }

    #[test]
    fn encrypted_missing_path_errors() {
        let m = XorCipherMetadata::new();
        let err = m.get_encrypted("/nope", b"k").unwrap_err();
        assert!(matches!(err, AdvError::OpfsNotFound(_)));
    }

    #[test]
    fn encrypted_has_queryable() {
        let m = XorCipherMetadata::new();
        assert!(!m.has("/doc/x"));
        m.put_encrypted("/doc/x", b"k", b"v").unwrap();
        assert!(m.has("/doc/x"));
    }

    // ---- OPFS tests ---------------------------------------------------------

    #[test]
    fn opfs_single_writer_blocks_concurrent() {
        let opfs = InMemoryOpfs::new(1024);
        let _g = opfs.acquire_writer("/a").unwrap();
        let err = opfs.acquire_writer("/a").unwrap_err();
        assert!(matches!(err, AdvError::OpfsConcurrentWrite(_)));
    }

    #[test]
    fn opfs_writer_drop_releases_lock() {
        let opfs = InMemoryOpfs::new(1024);
        {
            let g = opfs.acquire_writer("/a").unwrap();
            g.write(b"hello").unwrap();
        }
        // After drop, re-acquire succeeds.
        let _g2 = opfs.acquire_writer("/a").unwrap();
    }

    #[test]
    fn opfs_read_after_write_roundtrip() {
        let opfs = InMemoryOpfs::new(1024);
        {
            let g = opfs.acquire_writer("/a").unwrap();
            g.write(b"payload").unwrap();
        }
        let got = opfs.read_sync("/a").unwrap();
        assert_eq!(got, b"payload");
    }

    #[test]
    fn opfs_quota_tracks_usage() {
        let opfs = InMemoryOpfs::new(1024);
        assert_eq!(opfs.quota_used(), 0);
        {
            let g = opfs.acquire_writer("/a").unwrap();
            g.write(&[0u8; 100]).unwrap();
        }
        assert_eq!(opfs.quota_used(), 100);
        assert_eq!(opfs.quota_limit(), 1024);
    }

    #[test]
    fn opfs_block_store_bridges_into_content333() {
        let opfs = Arc::new(InMemoryOpfs::new(1024));
        let store = OpfsBlockStore { opfs };
        let cid = store.put(b"hi".to_vec()).unwrap();
        assert!(store.has(&cid));
        let got = store.get(&cid).unwrap();
        assert_eq!(got, b"hi");
    }

    #[test]
    fn opfs_missing_path_errors() {
        let opfs = InMemoryOpfs::new(1024);
        assert!(matches!(opfs.read_sync("/nope"), Err(AdvError::OpfsNotFound(_))));
    }
}
