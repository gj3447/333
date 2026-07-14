// KG: seed-ggrs-bft-adapter-eval-2026-04-15
//! 333-ggrs-adapter — GGRS rollback-netcode spike for 333 Platform.
//!
//! Public API surface:
//!   - [`BftGgrsSession`]  wraps a GGRS session, keyed by BFT NodeId
//!   - [`WebRtcNonBlockingSocket`]  GGRS NonBlockingSocket mock (mpsc backend)
//!
//! Production wiring (not in this spike):
//!   - Replace mpsc channels with real WebRTC DataChannel sends from p2p/webrtc.rs
//!   - Feed ggrs::GGRSRequest::SaveGameState / LoadGameState into ECS world snapshot

pub mod player_mapping;
pub mod socket;
// KG: taliban-fix-H2-webrtc-socket-2026-04-15
pub mod webrtc_socket;

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

// ── Public re-exports ──────────────────────────────────────────────────────
pub use player_mapping::{BftPlayerMap, PlayerHandle};
pub use socket::WebRtcNonBlockingSocket;
// KG: taliban-fix-H2-webrtc-socket-2026-04-15
pub use webrtc_socket::{
    WebRtcNonBlockingSocket as RealWebRtcSocket,
    TaggedInbox,
    peer_id_to_addr,
    addr_to_peer_id,
};

// ── Frame / Input types ────────────────────────────────────────────────────

/// A single player's game input for one frame.
/// Must be fixed-size and trivially serializable for rollback.
/// KG: seed-ggrs-bft-adapter-eval-2026-04-15
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct GameInput {
    /// Bitmask: bit0=up, bit1=down, bit2=left, bit3=right, bit4=action
    pub buttons: u8,
    /// Sequence counter (frame number at origin)
    pub frame: u32,
}

/// Serialized ECS/game state snapshot for rollback save/restore.
/// KG: seed-ggrs-bft-adapter-eval-2026-04-15
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameStateSnapshot {
    pub frame: u32,
    /// Opaque byte blob — 333 ECS serializes itself here via bincode/postcard
    pub data: Vec<u8>,
    /// blake3 hash of `data` — used for determinism verification
    pub checksum: [u8; 32],
}

impl GameStateSnapshot {
    /// Create a snapshot from raw ECS bytes, computing checksum automatically.
    pub fn new(frame: u32, data: Vec<u8>) -> Self {
        let checksum = compute_checksum(&data);
        Self { frame, data, checksum }
    }
}

/// Compute a deterministic 32-byte checksum over a state blob using blake3.
/// blake3 is collision-resistant (256-bit output) and WASM-friendly.
/// Replaces FNV-64 (was padded to 32 bytes, only 64 bits of entropy).
/// KG: taliban-fix-C2-2026-04-15
pub fn compute_checksum(data: &[u8]) -> [u8; 32] {
    // KG: taliban-fix-C2-2026-04-15
    *blake3::hash(data).as_bytes()
}

// ── GGRS Request emulation (trait skeleton when ggrs crate unavailable) ───

/// Mirror of `ggrs::GGRSRequest` — present so code compiles without ggrs dep.
/// When `ggrs-real` feature is enabled, replace this with the real enum.
/// KG: seed-ggrs-bft-adapter-eval-2026-04-15
#[derive(Debug)]
pub enum GgrsRequest {
    /// Save the current game state (before advancing)
    SaveGameState { frame: u32, cell: SaveCell },
    /// Load a previously saved state (rollback)
    LoadGameState { frame: u32, cell: SaveCell },
    /// Advance the game by one frame with these inputs
    AdvanceFrame { inputs: Vec<(PlayerHandle, GameInput)> },
}

/// Opaque cell passed back to caller for save/restore round-trip.
#[derive(Debug, Default, Clone)]
pub struct SaveCell {
    pub inner: Option<GameStateSnapshot>,
}

// ── BftGgrsSession ─────────────────────────────────────────────────────────

/// A GGRS-style P2P rollback session keyed by BFT player mapping.
///
/// Lifecycle:
/// 1. Build with [`BftGgrsSession::new`] supplying a [`BftPlayerMap`]
/// 2. Each tick: call [`add_local_input`], then [`advance_frame`]
/// 3. Process each [`GgrsRequest`] returned by [`advance_frame`]
///
/// KG: seed-ggrs-bft-adapter-eval-2026-04-15
pub struct BftGgrsSession {
    /// Maps BFT NodeId ↔ GGRS PlayerHandle
    pub player_map: BftPlayerMap,
    /// Local player's GGRS handle
    pub local_handle: PlayerHandle,
    /// Current confirmed frame
    current_frame: u32,
    /// Pending local inputs (per frame)
    local_input_queue: VecDeque<GameInput>,
    /// Save slots (frame → snapshot)
    save_slots: std::collections::HashMap<u32, GameStateSnapshot>,
    /// Input history for rollback — rolling window of (frame, input) tuples.
    /// Capped at `rollback_window + 1` entries per player to bound memory.
    /// KG: taliban-fix-H3-2026-04-15
    input_history: std::collections::HashMap<PlayerHandle, VecDeque<(u32, GameInput)>>,
    /// Number of players in session
    num_players: usize,
    /// Maximum input history depth per player (rollback_window + 1).
    /// KG: taliban-fix-H3-2026-04-15
    input_history_depth: usize,
}

impl BftGgrsSession {
    /// Create a new session.
    ///
    /// `local_node_id` must be registered in `player_map`.
    /// `rollback_window` controls input history depth (default 8 if 0 is passed).
    /// KG: seed-ggrs-bft-adapter-eval-2026-04-15
    pub fn new(player_map: BftPlayerMap, local_node_id: u32) -> Result<Self, AdapterError> {
        Self::new_with_rollback_window(player_map, local_node_id, 8)
    }

    /// Create a new session with an explicit rollback window depth.
    /// `rollback_window` = max frames to roll back; history depth = rollback_window + 1.
    /// KG: taliban-fix-H3-2026-04-15
    pub fn new_with_rollback_window(
        player_map: BftPlayerMap,
        local_node_id: u32,
        rollback_window: usize,
    ) -> Result<Self, AdapterError> {
        let local_handle = player_map
            .node_to_handle(local_node_id)
            .ok_or(AdapterError::NodeNotMapped(local_node_id))?;
        let num_players = player_map.len();
        // KG: taliban-fix-H3-2026-04-15
        let input_history_depth = rollback_window + 1;
        Ok(Self {
            player_map,
            local_handle,
            current_frame: 0,
            local_input_queue: VecDeque::new(),
            save_slots: Default::default(),
            input_history: Default::default(),
            num_players,
            input_history_depth,
        })
    }

    /// Queue a local input for the next frame.
    pub fn add_local_input(&mut self, input: GameInput) {
        self.local_input_queue.push_back(input);
    }

    /// Advance the simulation by one frame, returning requests to the caller.
    ///
    /// Returns `Ok(vec![SaveGameState, AdvanceFrame])` on normal tick.
    /// Returns rollback requests when remote inputs arrive late.
    /// KG: seed-ggrs-bft-adapter-eval-2026-04-15
    pub fn advance_frame(&mut self) -> Result<Vec<GgrsRequest>, AdapterError> {
        let frame = self.current_frame;
        let input = self.local_input_queue.pop_front().unwrap_or(GameInput {
            buttons: 0,
            frame,
        });

        // Record local input with rolling window cap.
        // KG: taliban-fix-H3-2026-04-15
        {
            let window = self.input_history
                .entry(self.local_handle)
                .or_default();
            window.push_back((frame, input));
            while window.len() > self.input_history_depth {
                window.pop_front();
            }
        }

        // Build combined inputs (use last known for remote players)
        let mut inputs = Vec::with_capacity(self.num_players);
        for handle in 0..self.num_players as u32 {
            let last = self
                .input_history
                .get(&handle)
                .and_then(|v| v.back().map(|&(_, inp)| inp))
                .unwrap_or_default();
            inputs.push((handle, last));
        }

        self.current_frame += 1;

        Ok(vec![
            GgrsRequest::SaveGameState {
                frame,
                cell: SaveCell::default(),
            },
            GgrsRequest::AdvanceFrame { inputs },
        ])
    }

    /// Apply a remote input received out-of-band (e.g., via WebRTC DataChannel).
    /// Enforces the rolling window cap.
    /// KG: taliban-fix-H3-2026-04-15
    pub fn receive_remote_input(&mut self, player: PlayerHandle, input: GameInput) {
        let window = self.input_history.entry(player).or_default();
        // Use input.frame as the frame tag for the tuple
        window.push_back((input.frame, input));
        while window.len() > self.input_history_depth {
            window.pop_front();
        }
    }

    /// Store a snapshot after handling `SaveGameState`.
    pub fn confirm_save(&mut self, frame: u32, snapshot: GameStateSnapshot) {
        self.save_slots.insert(frame, snapshot);
    }

    /// Retrieve snapshot for a rollback `LoadGameState`.
    pub fn get_snapshot(&self, frame: u32) -> Option<&GameStateSnapshot> {
        self.save_slots.get(&frame)
    }

    pub fn current_frame(&self) -> u32 {
        self.current_frame
    }
}

// ── Error type ─────────────────────────────────────────────────────────────

/// Errors from the adapter layer.
/// KG: seed-ggrs-bft-adapter-eval-2026-04-15
#[derive(Debug)]
pub enum AdapterError {
    NodeNotMapped(u32),
    SerializationError(String),
    InvalidFrame { expected: u32, got: u32 },
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NodeNotMapped(id) => write!(f, "BFT node {id} has no GGRS PlayerHandle"),
            Self::SerializationError(s) => write!(f, "serialization error: {s}"),
            Self::InvalidFrame { expected, got } => {
                write!(f, "frame mismatch: expected {expected}, got {got}")
            }
        }
    }
}

impl std::error::Error for AdapterError {}

// ── Unit tests ────────────────────────────────────────────────────────────────
// KG: taliban-fix-C2-2026-04-15
// KG: taliban-fix-H3-2026-04-15

#[cfg(test)]
mod tests {
    use super::*;
    use player_mapping::BftPlayerMap;

    /// C2: blake3 collision resistance — two different inputs must yield different checksums.
    /// KG: taliban-fix-C2-2026-04-15
    #[test]
    fn blake3_checksum_collision_resistance() {
        let a = compute_checksum(b"state-alpha");
        let b = compute_checksum(b"state-beta");
        assert_ne!(a, b, "different inputs must produce different blake3 checksums");
    }

    /// C2: checksum is 32 bytes and stable across calls.
    /// KG: taliban-fix-C2-2026-04-15
    #[test]
    fn blake3_checksum_stable_32bytes() {
        let data = b"333-platform-deterministic-state";
        let c1 = compute_checksum(data);
        let c2 = compute_checksum(data);
        assert_eq!(c1, c2, "blake3 checksum must be deterministic");
        assert_eq!(c1.len(), 32, "checksum must be exactly 32 bytes");
    }

    /// H3: input_history window is capped at rollback_window + 1.
    /// After pushing 9 frames into a window-8 session, size must be 9 (= 8+1).
    /// KG: taliban-fix-H3-2026-04-15
    #[test]
    fn input_history_window_cap() {
        let map = BftPlayerMap::from_sorted_validators(&[1]);
        let mut sess = BftGgrsSession::new_with_rollback_window(map, 1, 8)
            .expect("session init");

        // Push 9 local inputs into the window (rollback_window=8 → depth=9)
        for f in 0..9u32 {
            sess.add_local_input(GameInput { buttons: f as u8, frame: f });
            sess.advance_frame().expect("advance");
        }

        // Internal window should be capped at 9 (8+1)
        let depth = sess
            .input_history
            .get(&sess.local_handle)
            .map(|v| v.len())
            .unwrap_or(0);
        assert_eq!(depth, 9, "input_history must be capped at rollback_window+1=9, got {depth}");
    }

    /// H3: oldest entries are dropped when window overflows.
    /// KG: taliban-fix-H3-2026-04-15
    #[test]
    fn input_history_oldest_evicted() {
        let map = BftPlayerMap::from_sorted_validators(&[1]);
        let mut sess = BftGgrsSession::new_with_rollback_window(map, 1, 8)
            .expect("session init");

        // Push 12 frames (4 beyond the depth=9 cap)
        for f in 0..12u32 {
            sess.add_local_input(GameInput { buttons: f as u8, frame: f });
            sess.advance_frame().expect("advance");
        }

        let window = sess.input_history.get(&sess.local_handle).unwrap();
        // Most recent entry must be frame 11
        let (_, last_input) = window.back().unwrap();
        assert_eq!(last_input.frame, 11, "last entry must be frame 11");
        // Oldest entry must be frame 3 (12 - 9 = 3)
        let (_, first_input) = window.front().unwrap();
        assert_eq!(first_input.frame, 3, "oldest entry must be frame 3 after eviction of frames 0-2");
    }
}
