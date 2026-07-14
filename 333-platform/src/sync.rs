// KG: TASK_333_INT_CrdtSync, CONTRACT_333_INT_CrdtSync
// KG: CONTRACT_SharedType_ProcessWireResult, SPEC_333_WireRouting
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-sync-StateVector/SyncPayload/OutgoingMsg/SyncManager
//! CRDT Sync Manager — delta batching, state vector, postcard serialization.
//!
//! Flow: place_block() → delta → batch queue → 20ms poll → postcard encode
//!       → wire::encode(StateUpdate) → outgoing JSON for TS to send.
//!       recv: wire msg → postcard decode → merge_delta.
//!
//! Taliban F3: Safari produces non-deterministic Ed25519 signatures.
//!   → NEVER use signature bytes as content address or dedup key.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::lww_map::{LwwDelta, LwwMap};
use crate::wire::{self, MsgType};

/// State vector: tracks max Lamport counter seen per peer
/// KG: CONTRACT_333_INT_CrdtSync — state vector piggybacking
pub type StateVector = HashMap<u32, u64>;

/// Sync message payload — postcard serialized
/// KG: SPEC_333_WireRouting — 0x01 StateUpdate, 0x02 StateFull
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncPayload {
    /// Incremental delta with sender's state vector
    Delta {
        sender_id: u32,
        deltas: Vec<(String, crate::lww_map::LwwEntry<String>)>,
        state_vector: StateVector,
    },
    /// Full state snapshot for new peers
    FullState {
        sender_id: u32,
        entries: Vec<(String, crate::lww_map::LwwEntry<String>)>,
        state_vector: StateVector,
    },
}

/// Outgoing message for TS to send
/// KG: CONTRACT_SharedType_ProcessWireResult
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMsg {
    pub msg_type: u8,
    pub channel: String,
    #[serde(with = "base64_bytes")]
    pub payload: Vec<u8>,
}

/// Base64 serde for binary payloads in JSON
mod base64_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        // Simple hex encoding (no base64 dep needed)
        s.serialize_str(&bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(serde::de::Error::custom))
            .collect()
    }
}

/// CRDT Sync Manager
/// KG: CONTRACT_333_INT_CrdtSync
pub struct SyncManager {
    node_id: u32,
    /// Pending deltas to batch-send
    batch_queue: Vec<(String, crate::lww_map::LwwEntry<String>)>,
    /// Our state vector (max Lamport counter per peer)
    state_vector: StateVector,
    /// Counter for delta threshold (reconnection: >1000 → send full state)
    delta_count: u64,
}

impl SyncManager {
    pub fn new(node_id: u32) -> Self {
        Self {
            node_id,
            batch_queue: Vec::new(),
            state_vector: HashMap::new(),
            delta_count: 0,
        }
    }

    /// Called after place_block/delete_block — queue delta for batching
    pub fn on_local_delta(&mut self, delta: &LwwDelta<String, String>) {
        for (k, entry) in &delta.changes {
            // Update our state vector with max timestamp
            let counter = entry.timestamp.counter;
            let sv = self.state_vector.entry(self.node_id).or_insert(0);
            if counter > *sv {
                *sv = counter;
            }
            self.batch_queue.push((k.clone(), entry.clone()));
        }
        self.delta_count += delta.changes.len() as u64;
    }

    /// Poll: flush batch queue → encode → return outgoing wire messages
    /// Called every 20ms by the game loop / TS bridge
    /// KG: CONTRACT_333_INT_CrdtSync — 20ms batch, postcard encode
    pub fn poll_outgoing(&mut self) -> Vec<OutgoingMsg> {
        if self.batch_queue.is_empty() {
            return Vec::new();
        }

        let payload = SyncPayload::Delta {
            sender_id: self.node_id,
            deltas: self.batch_queue.drain(..).collect(),
            state_vector: self.state_vector.clone(),
        };

        match self.encode_sync_msg(MsgType::StateUpdate, &payload) {
            Some(msg) => vec![msg],
            None => Vec::new(),
        }
    }

    /// Generate full state snapshot for a new peer
    /// KG: CONTRACT_333_INT_CrdtSync — 신규 peer → StateFull
    /// Taliban F4 fix: build global state vector by scanning all entries
    pub fn create_full_snapshot(&self, world: &LwwMap<String, String>) -> Option<OutgoingMsg> {
        let entries: Vec<_> = world.snapshot().iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // Taliban F4: build true global state vector from entries, not just self
        let mut global_sv = self.state_vector.clone();
        for (_, entry) in &entries {
            let node = entry.timestamp.node_id;
            let counter = entry.timestamp.counter;
            let sv = global_sv.entry(node).or_insert(0);
            if counter > *sv { *sv = counter; }
        }
        let payload = SyncPayload::FullState {
            sender_id: self.node_id,
            entries,
            state_vector: global_sv,
        };
        self.encode_sync_msg(MsgType::StateFull, &payload)
    }

    /// Process incoming wire message (StateUpdate or StateFull)
    /// Returns: true if world was modified
    /// KG: CONTRACT_333_INT_CrdtSync — recv → decode → merge
    pub fn process_incoming(
        &mut self,
        _msg_type: u8,
        wire_payload: &[u8],
        world: &mut LwwMap<String, String>,
    ) -> bool {
        let sync_payload: SyncPayload = match postcard::from_bytes(wire_payload) {
            Ok(p) => p,
            Err(_) => {
                // Fallback: try serde_json (backward compat with pre-postcard messages)
                match serde_json::from_slice(wire_payload) {
                    Ok(p) => p,
                    Err(_) => return false,
                }
            }
        };

        match sync_payload {
            SyncPayload::Delta { sender_id: _sender_id, deltas, state_vector } => {
                // Merge remote state vector
                self.merge_state_vector(&state_vector);
                // Apply deltas
                let delta = LwwDelta { changes: deltas };
                world.merge_delta(&delta);
                self.delta_count += delta.changes.len() as u64;
                true
            }
            SyncPayload::FullState { sender_id: _sender_id, entries, state_vector } => {
                self.merge_state_vector(&state_vector);
                // Merge each entry
                for (k, entry) in entries {
                    let delta = LwwDelta { changes: vec![(k, entry)] };
                    world.merge_delta(&delta);
                }
                true
            }
        }
    }

    /// Should we send full state (>1000 deltas since last snapshot)?
    pub fn needs_full_snapshot(&self) -> bool {
        self.delta_count > 1000
    }

    /// Reset delta counter after sending full snapshot
    pub fn reset_delta_count(&mut self) {
        self.delta_count = 0;
    }

    pub fn state_vector(&self) -> &StateVector {
        &self.state_vector
    }

    pub fn pending_count(&self) -> usize {
        self.batch_queue.len()
    }

    // --- Private ---

    fn merge_state_vector(&mut self, remote: &StateVector) {
        for (&peer_id, &counter) in remote {
            let entry = self.state_vector.entry(peer_id).or_insert(0);
            if counter > *entry {
                *entry = counter;
            }
        }
    }

    fn encode_sync_msg(&self, msg_type: MsgType, payload: &SyncPayload) -> Option<OutgoingMsg> {
        let bytes = postcard::to_allocvec(payload).ok()?;
        let wire_bytes = wire::encode(msg_type, &bytes).ok()?;

        Some(OutgoingMsg {
            msg_type: msg_type as u8,
            channel: "crdt".to_string(),
            payload: wire_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lww_map::LwwMap;

    #[test]
    fn delta_batch_roundtrip() {
        let mut world_a = LwwMap::<String, String>::new(1);
        let mut world_b = LwwMap::<String, String>::new(2);
        let mut sync_a = SyncManager::new(1);
        let mut sync_b = SyncManager::new(2);

        // A places a block
        let delta = world_a.set("0,0".into(), "stone".into());
        sync_a.on_local_delta(&delta);

        // A polls outgoing
        let msgs = sync_a.poll_outgoing();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].channel, "crdt");

        // Decode wire msg to get payload
        let wire_msg = &msgs[0].payload;
        match wire::decode(wire_msg) {
            wire::DecodeResult::Ok(msg) => {
                // B processes incoming
                let modified = sync_b.process_incoming(
                    msg.header.msg_type, &msg.payload, &mut world_b
                );
                assert!(modified);
                assert_eq!(world_b.get(&"0,0".to_string()), Some(&"stone".to_string()));
            }
            _ => panic!("decode failed"),
        }
    }

    #[test]
    fn full_snapshot_sync() {
        let mut world_a = LwwMap::<String, String>::new(1);
        let mut world_b = LwwMap::<String, String>::new(2);
        let sync_a = SyncManager::new(1);
        let mut sync_b = SyncManager::new(2);

        // A has several blocks
        world_a.set("0,0".into(), "stone".into());
        world_a.set("1,1".into(), "grass".into());
        world_a.set("2,2".into(), "water".into());

        // A sends full snapshot
        let msg = sync_a.create_full_snapshot(&world_a).unwrap();
        match wire::decode(&msg.payload) {
            wire::DecodeResult::Ok(wire_msg) => {
                let modified = sync_b.process_incoming(
                    wire_msg.header.msg_type, &wire_msg.payload, &mut world_b
                );
                assert!(modified);
                assert_eq!(world_b.len(), 3);
                assert_eq!(world_b.get(&"2,2".to_string()), Some(&"water".to_string()));
            }
            _ => panic!("decode failed"),
        }
    }

    #[test]
    fn state_vector_merge() {
        let mut sync = SyncManager::new(1);
        let remote_sv: StateVector = [(2, 10), (3, 20)].into();
        sync.merge_state_vector(&remote_sv);
        assert_eq!(sync.state_vector[&2], 10);
        assert_eq!(sync.state_vector[&3], 20);

        // Higher value wins
        let remote_sv2: StateVector = [(2, 5), (3, 30)].into();
        sync.merge_state_vector(&remote_sv2);
        assert_eq!(sync.state_vector[&2], 10); // 10 > 5
        assert_eq!(sync.state_vector[&3], 30); // 30 > 20
    }

    #[test]
    fn empty_poll_no_messages() {
        let mut sync = SyncManager::new(1);
        assert!(sync.poll_outgoing().is_empty());
    }
}
