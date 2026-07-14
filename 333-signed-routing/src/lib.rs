// KG: SPAN_333_L0_L2_SignedRouting, plan-333-three-way-hybrid-2026-04-17
// Authenticated peer records for Kademlia DHT.
//
// Each record = (node_id, addr, unix_ts, signature). Only the holder of the
// secret key matching `node_id` can produce a valid record. A verifier trusts
// a record iff `Keypair::verify(node_id, canonical_bytes, sig)` succeeds.
//
// Pair-programs with:
//   - identity333: keypair + signature primitives
//   - kademlia333: XOR routing / k-bucket storage
//
// Impersonation resistance: forged author cannot produce a sig that matches the
// claimed NodeId, so verification rejects (see `spoofed_record_rejected` test).

#![forbid(unsafe_code)]

use std::collections::HashMap;

use identity333::{Keypair, NodeId, Signature};
use kademlia333::RoutingTable;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RoutingError {
    #[error("record signature invalid for node {0}")]
    BadSignature(NodeId),
    #[error("serialization: {0}")]
    Ser(String),
    #[error("record not found: {0}")]
    NotFound(NodeId),
    #[error("stale record: incoming ts {incoming} not newer than stored {stored}")]
    Stale { incoming: u64, stored: u64 },
}

/// Payload that actually gets signed. Separating payload from envelope keeps
/// the canonical byte representation stable and easy to hash.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordPayload {
    pub node_id: NodeId,
    pub addr: String,
    pub ts: u64,
}

/// Full signed envelope that travels on the wire.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedPeerRecord {
    pub payload: RecordPayload,
    pub sig: Signature,
}

impl SignedPeerRecord {
    pub fn sign(kp: &Keypair, addr: impl Into<String>, ts: u64) -> Result<Self, RoutingError> {
        let payload = RecordPayload {
            node_id: kp.node_id(),
            addr: addr.into(),
            ts,
        };
        let bytes = serde_json::to_vec(&payload).map_err(|e| RoutingError::Ser(e.to_string()))?;
        let sig = kp.sign(&bytes);
        Ok(Self { payload, sig })
    }

    pub fn verify(&self) -> Result<(), RoutingError> {
        let bytes = serde_json::to_vec(&self.payload).map_err(|e| RoutingError::Ser(e.to_string()))?;
        Keypair::verify(self.payload.node_id.as_bytes(), &bytes, &self.sig)
            .map_err(|_| RoutingError::BadSignature(self.payload.node_id))
    }

    pub fn node_id(&self) -> NodeId {
        self.payload.node_id
    }
}

/// A Kademlia routing table that only stores peers with a valid, fresher-than-known signed record.
/// Newer ts replaces older.
pub struct SignedRoutingTable {
    inner: RoutingTable,
    records: HashMap<NodeId, SignedPeerRecord>,
}

impl SignedRoutingTable {
    pub fn new(local: NodeId) -> Self {
        Self {
            inner: RoutingTable::new(local),
            records: HashMap::new(),
        }
    }

    pub fn local(&self) -> NodeId {
        self.inner.local()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Accept a record iff signature verifies AND ts > stored ts (if any).
    pub fn observe(&mut self, record: SignedPeerRecord) -> Result<(), RoutingError> {
        record.verify()?;
        let nid = record.node_id();
        if let Some(existing) = self.records.get(&nid) {
            if record.payload.ts <= existing.payload.ts {
                return Err(RoutingError::Stale {
                    incoming: record.payload.ts,
                    stored: existing.payload.ts,
                });
            }
        }
        self.inner.observe(nid);
        self.records.insert(nid, record);
        Ok(())
    }

    pub fn get(&self, id: &NodeId) -> Option<&SignedPeerRecord> {
        self.records.get(id)
    }

    /// Find `count` closest peers to `target`, returning their records (all guaranteed verified).
    pub fn find_closest(&self, target: &NodeId, count: usize) -> Vec<&SignedPeerRecord> {
        self.inner
            .find_closest(target, count)
            .into_iter()
            .filter_map(|n| self.records.get(&n))
            .collect()
    }
}
