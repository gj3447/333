// KG: seed-ggrs-bft-adapter-eval-2026-04-15
//! BFT keyring PublicKey ↔ GGRS PlayerHandle mapping.
//!
//! ## Mapping strategy
//!
//! 333's BFT layer uses `NodeId` (u32) as the primary peer identity, backed
//! by Ed25519 `PeerId([u8;32])` for cryptographic verification.  GGRS uses
//! a dense `PlayerHandle` (u32, 0-indexed) to index input arrays.
//!
//! The canonical mapping is:
//!   PlayerHandle = position of NodeId in the sorted validator set.
//!
//! This is deterministic across all peers as long as they share the same
//! ordered validator list — which is already enforced by `ValidatorSet` in
//! bft/crypto.rs (sorted by NodeId at construction time).
//!
//! `BftPlayerMap` is built once at session start from the agreed validator
//! list and is immutable for the duration of a game session.

use std::collections::HashMap;

/// GGRS player handle — u32 index into input arrays (0-indexed).
/// KG: seed-ggrs-bft-adapter-eval-2026-04-15
pub type PlayerHandle = u32;

/// Bidirectional mapping between BFT NodeId and GGRS PlayerHandle.
///
/// Construction:
///   let map = BftPlayerMap::from_sorted_validators(&[node0, node1, node2, node3]);
///   // PlayerHandle 0 = node0, 1 = node1, ...
///
/// KG: seed-ggrs-bft-adapter-eval-2026-04-15
#[derive(Debug, Clone)]
pub struct BftPlayerMap {
    /// NodeId → PlayerHandle
    node_to_handle_map: HashMap<u32, PlayerHandle>,
    /// PlayerHandle → NodeId
    handle_to_node_map: HashMap<PlayerHandle, u32>,
    /// Optional: NodeId → Ed25519 public key bytes (for display / verification)
    peer_keys: HashMap<u32, [u8; 32]>,
}

impl BftPlayerMap {
    /// Build a map from an **already-sorted** validator list.
    ///
    /// The caller must ensure the same ordering on all peers (sort by NodeId).
    /// KG: seed-ggrs-bft-adapter-eval-2026-04-15
    pub fn from_sorted_validators(validators: &[u32]) -> Self {
        let node_to_handle_map = validators
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i as PlayerHandle))
            .collect();
        let handle_to_node_map = validators
            .iter()
            .enumerate()
            .map(|(i, &id)| (i as PlayerHandle, id))
            .collect();
        Self {
            node_to_handle_map,
            handle_to_node_map,
            peer_keys: HashMap::new(),
        }
    }

    /// Build from unsorted list — sorts by NodeId before assigning handles.
    /// Use this when you cannot guarantee caller-side sort.
    pub fn from_validators(mut validators: Vec<u32>) -> Self {
        validators.sort_unstable();
        Self::from_sorted_validators(&validators)
    }

    /// Register the Ed25519 public key for a validator (optional, for audit).
    /// `public_key` is `PeerId.0` from `crypto_real::PeerId`.
    /// KG: seed-ggrs-bft-adapter-eval-2026-04-15
    pub fn register_pubkey(&mut self, node_id: u32, public_key: [u8; 32]) {
        self.peer_keys.insert(node_id, public_key);
    }

    /// Look up GGRS PlayerHandle from BFT NodeId.
    pub fn node_to_handle(&self, node_id: u32) -> Option<PlayerHandle> {
        self.node_to_handle_map.get(&node_id).copied()
    }

    /// Look up BFT NodeId from GGRS PlayerHandle.
    pub fn handle_to_node(&self, handle: PlayerHandle) -> Option<u32> {
        self.handle_to_node_map.get(&handle).copied()
    }

    /// Retrieve stored public key for a node, if registered.
    pub fn pubkey(&self, node_id: u32) -> Option<&[u8; 32]> {
        self.peer_keys.get(&node_id)
    }

    /// Total number of players in this session.
    pub fn len(&self) -> usize {
        self.node_to_handle_map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.node_to_handle_map.is_empty()
    }

    /// All (NodeId, PlayerHandle) pairs, sorted by handle.
    pub fn pairs(&self) -> Vec<(u32, PlayerHandle)> {
        let mut v: Vec<_> = self
            .node_to_handle_map
            .iter()
            .map(|(&n, &h)| (n, h))
            .collect();
        v.sort_by_key(|&(_, h)| h);
        v
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_mapping_is_deterministic() {
        // KG: seed-ggrs-bft-adapter-eval-2026-04-15
        let map_a = BftPlayerMap::from_validators(vec![10, 3, 7, 1]);
        let map_b = BftPlayerMap::from_validators(vec![7, 1, 10, 3]);
        // Both must produce identical mappings
        assert_eq!(map_a.node_to_handle(1), map_b.node_to_handle(1));
        assert_eq!(map_a.node_to_handle(3), map_b.node_to_handle(3));
        assert_eq!(map_a.node_to_handle(7), map_b.node_to_handle(7));
        assert_eq!(map_a.node_to_handle(10), map_b.node_to_handle(10));
    }

    #[test]
    fn round_trip_node_handle() {
        let map = BftPlayerMap::from_sorted_validators(&[100, 200, 300, 400]);
        for (node, handle) in map.pairs() {
            assert_eq!(map.handle_to_node(handle), Some(node));
        }
    }

    #[test]
    fn pubkey_registration() {
        let mut map = BftPlayerMap::from_sorted_validators(&[1, 2]);
        let fake_key = [0xABu8; 32];
        map.register_pubkey(1, fake_key);
        assert_eq!(map.pubkey(1), Some(&fake_key));
        assert_eq!(map.pubkey(2), None);
    }

    #[test]
    fn unknown_node_returns_none() {
        let map = BftPlayerMap::from_sorted_validators(&[1, 2, 3]);
        assert_eq!(map.node_to_handle(99), None);
    }
}
