// KG: SPAN_333_L5_L0_SignedCRDT_Integration
// Signed CRDT ops: each state mutation is authored by a NodeId and carries a signature.
// Composes crdt333 (Crdt trait) + identity333 (Keypair/NodeId/Signature).
//
// Open design question: `SignedLwwMap.authorized` is a plain `BTreeMap<NodeId,()>`
// local to each replica. Distributed deployments MUST share the ACL via an
// external channel (e.g., a separate Crdt-based Or-Set sync) — this crate
// does NOT solve ACL replication.

#![forbid(unsafe_code)]
//
// Design: SignedOp<T> = (author: NodeId, payload: T, sig: Signature over canonical bytes of payload).
// A SignedLwwMap<K,V> accepts only SignedOps whose sig verifies against the stated author.

use std::collections::BTreeMap;

use crdt333::{Crdt, LwwMap};
use identity333::{Keypair, NodeId, Signature};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SignedError {
    #[error("signature verification failed for author {0}")]
    BadSignature(NodeId),
    #[error("unknown author: {0}")]
    UnknownAuthor(NodeId),
    #[error("serialization: {0}")]
    Ser(String),
}

/// A CRDT op authored and signed by one NodeId. Generic over payload type P.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedOp<P> {
    pub author: NodeId,
    pub payload: P,
    pub sig: Signature,
}

impl<P: Serialize> SignedOp<P> {
    pub fn new(kp: &Keypair, payload: P) -> Result<Self, SignedError> {
        let bytes = serde_json::to_vec(&payload).map_err(|e| SignedError::Ser(e.to_string()))?;
        let sig = kp.sign(&bytes);
        Ok(Self { author: kp.node_id(), payload, sig })
    }

    pub fn verify(&self) -> Result<(), SignedError>
    where
        P: Serialize,
    {
        let bytes = serde_json::to_vec(&self.payload).map_err(|e| SignedError::Ser(e.to_string()))?;
        Keypair::verify(self.author.as_bytes(), &bytes, &self.sig)
            .map_err(|_| SignedError::BadSignature(self.author))
    }
}

/// Signed LWW map. Accepts ops only from authors listed in `authorized` and only if sig verifies.
/// Internal state is a plain LwwMap<K, V>; signatures are a membrane on writes, not stored in the CRDT itself.
pub struct SignedLwwMap<K, V> {
    authorized: BTreeMap<NodeId, ()>,
    inner: LwwMap<K, V>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LwwPut<K, V> {
    pub key: K,
    pub ts: u64,
    pub value: V,
}

impl<K, V> Default for SignedLwwMap<K, V>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de>,
    V: Crdt + Clone + Serialize + for<'de> Deserialize<'de>,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> SignedLwwMap<K, V>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de>,
    V: Crdt + Clone + Serialize + for<'de> Deserialize<'de>,
{
    pub fn new() -> Self {
        Self { authorized: BTreeMap::new(), inner: LwwMap::new() }
    }

    pub fn authorize(&mut self, n: NodeId) {
        self.authorized.insert(n, ());
    }

    pub fn is_authorized(&self, n: &NodeId) -> bool {
        self.authorized.contains_key(n)
    }

    pub fn get(&self, k: &K) -> Option<&V> {
        self.inner.get(k)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Apply a signed put. Rejects unknown authors or invalid signatures.
    pub fn apply(&mut self, op: &SignedOp<LwwPut<K, V>>) -> Result<(), SignedError> {
        if !self.is_authorized(&op.author) {
            return Err(SignedError::UnknownAuthor(op.author));
        }
        op.verify()?;
        self.inner.put(op.payload.key.clone(), op.payload.ts, op.payload.value.clone());
        Ok(())
    }
}
