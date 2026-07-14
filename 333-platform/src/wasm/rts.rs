// KG: prod-wiring-wasm-rts-export-2026-04-15
//! WASM bindings for RtsSession — exposes lockstep RTS session to JavaScript.
//!
//! Wraps `RtsSession` (and its helpers) behind a `#[wasm_bindgen]` facade.
//! All JSON serialization stays in this layer; the inner crate types remain
//! free of JS coupling.
//!
//! # API surface
//! - `RtsSessionWasm::new(seed, node_id)` — create session with 4 default units
//! - `RtsSessionWasm::advance_frame(inputs)` — step simulation, return JSON result
//! - `RtsSessionWasm::state_hash_hex()` — current frame hash as 64-char hex string
//! - `RtsSessionWasm::observe_peer_digest(peer_id, frame, hash_hex)` — check peer hash
//! - `RtsSessionWasm::tactical_summary()` — units positions as JSON
//! - `rts_peer_eject_stub(peer_id)` — stub for peer eject vote (UI section 5)
//! - `rts_bft_checkpoint_stub(frame)` — stub for BFT checkpoint QC (UI section 4)
//! - `rts_ggrs_rollback_stub(frame)` — stub for GGRS rollback (UI section 6)
//!
//! # KG: prod-wiring-wasm-rts-export-2026-04-15

use wasm_bindgen::prelude::*;

use crate::apps::rts_session::{RtsSession, RtsInput};
use crate::apps::rts_state_tiers::UnitTactical;
use crate::determinism::Fixed32;
use crate::observability; // KG: sprint6C-observability-2026-04-15

// ---------------------------------------------------------------------------
// RtsSessionWasm
// ---------------------------------------------------------------------------

/// WASM-exposed wrapper around `RtsSession`.
///
/// Created via JS: `new wasm.RtsSessionWasm(seed, nodeId)`
///
/// KG: prod-wiring-wasm-rts-export-2026-04-15
#[wasm_bindgen]
pub struct RtsSessionWasm {
    inner: RtsSession,
}

#[wasm_bindgen]
impl RtsSessionWasm {
    /// Create a new RTS session.
    ///
    /// `seed` is used to derive deterministic initial unit positions across
    /// all peers.  All peers with the same `seed` start in identical state.
    /// `node_id` identifies this local node (used in DesyncDetector logs).
    ///
    /// KG: prod-wiring-wasm-rts-export-2026-04-15
    #[wasm_bindgen(constructor)]
    pub fn new(seed: u32, node_id: u32) -> Self {
        console_error_panic_hook::set_once();

        // Derive 4 initial units from seed for determinism across peers.
        let units: Vec<UnitTactical> = (0u32..4).map(|i| {
            // Fixed-point positions derived from seed+i — same for all peers with same seed.
            let base = seed.wrapping_add(i * 1337);
            UnitTactical {
                unit_id: i,
                owner: i % 2,
                x: Fixed32((base.wrapping_mul(1009) as i32) & 0x007F_FFFF), // bounded range
                y: Fixed32((base.wrapping_mul(2003) as i32) & 0x007F_FFFF),
                hp: 100,
                max_hp: 100,
                animation_frame: 0,
                ability_cooldown: 0,
                attack_range: Fixed32::from_int(10),
                attack_damage: 10,
                attack_cooldown_max: 3,
                attack_cooldown_remaining: 0,
            }
        }).collect();

        let inner = RtsSession::new(
            node_id,
            units,
            64,   // bridge_capacity
            64,   // desync_retention
            true, // with_ggrs = rollback-capable
        );

        Self { inner }
    }

    /// Advance the simulation by one frame.
    ///
    /// `inputs` — encoded byte slice from JS.  The first byte is a command flag:
    ///   0x00 = noop, 0x01 = move_dx_dy (11 bytes total per RtsInput payload).
    ///
    /// For the WASM bridge we accept a raw byte buffer and decode it into one
    /// `RtsInput` per frame: non-empty buffer with first byte != 0 → `move_unit(0, 1, 1)`.
    ///
    /// Returns a JSON string:
    /// ```json
    /// { "frame": 42, "hash": "aabbcc...", "units": [{"id":0,"x":1000,"y":2000}, ...] }
    /// ```
    ///
    /// KG: prod-wiring-wasm-rts-export-2026-04-15
    pub fn advance_frame(&mut self, inputs: &[u8]) -> String {
        // Decode raw byte buffer → Vec<RtsInput> (one entry per frame).
        // Protocol: empty or [0x00, ...] → noop; anything else → move_unit(0, dx, dy).
        // Full multi-player input batching is a Phase 4 concern; for now unit 0 is
        // controlled by the single connected client.
        let rts_inputs: Vec<RtsInput> = if inputs.is_empty() || inputs[0] == 0x00 {
            vec![RtsInput::noop()]
        } else if inputs.len() >= 11 && inputs[0] == 0x01 {
            // Full move payload forwarded verbatim
            vec![RtsInput::from_bytes(inputs)]
        } else {
            // Simple flag: any non-zero byte → move unit 0 by (1, 1) Fixed32 steps
            vec![RtsInput::move_unit(0, 65536, 65536)] // Fixed32(1.0) = 65536
        };

        let hash = self.inner.advance_frame(&rts_inputs);
        let hash_hex: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
        let frame = self.inner.tactical.turn_number;
        let units: Vec<_> = self.inner.tactical.units.iter().map(|u| {
            serde_json::json!({
                "id": u.unit_id,
                "owner": u.owner,
                "x": u.x.0,
                "y": u.y.0,
                "hp": u.hp,
                "maxHp": u.max_hp,
            })
        }).collect();

        let winner = self.inner.tactical.winner;
        serde_json::json!({
            "frame": frame,
            "hash": hash_hex,
            "units": units,
            "winner": winner, // KG: sprint3-3C-game-rules-graphics-2026-04-15
        }).to_string()
    }

    /// Return the current frame's state hash as a 64-character lowercase hex string.
    ///
    /// Broadcast this over the position channel after each `advance_frame` call
    /// so peers can detect desyncs.
    ///
    /// KG: prod-wiring-wasm-rts-export-2026-04-15
    pub fn state_hash_hex(&self) -> String {
        self.inner.tactical.state_hash
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }

    /// Observe a hash digest received from a peer.
    ///
    /// Returns `None` if hashes match (no desync), or a JSON string describing
    /// the desync event if hashes diverge:
    /// ```json
    /// {
    ///   "peerId": 2,
    ///   "frame": 42,
    ///   "localHash": "aabb...",
    ///   "peerHash": "ccdd..."
    /// }
    /// ```
    ///
    /// KG: prod-wiring-wasm-rts-export-2026-04-15
    pub fn observe_peer_digest(&self, peer_id: u32, frame: u32, hash_hex: &str) -> Option<String> {
        // Decode hex string → [u8; 32]
        let bytes: Vec<u8> = (0..hash_hex.len())
            .step_by(2)
            .filter_map(|i| {
                hash_hex.get(i..i + 2)
                    .and_then(|s| u8::from_str_radix(s, 16).ok())
            })
            .collect();

        if bytes.len() != 32 {
            return Some(r#"{"error":"invalid hash length"}"#.to_string());
        }

        let mut peer_hash = [0u8; 32];
        peer_hash.copy_from_slice(&bytes);

        match self.inner.observe_peer_digest(peer_id, frame as u64, peer_hash) {
            None => None,
            Some(ev) => {
                let local_hex: String = ev.local_hash.iter().map(|b| format!("{:02x}", b)).collect();
                let peer_hex: String = ev.peer_hash.iter().map(|b| format!("{:02x}", b)).collect();
                Some(serde_json::json!({
                    "peerId": ev.peer_id,
                    "frame": ev.frame_n,
                    "localHash": local_hex,
                    "peerHash": peer_hex,
                }).to_string())
            }
        }
    }

    /// Return a JSON summary of all unit positions in the current frame.
    ///
    /// ```json
    /// {"frame":42,"units":[{"id":0,"x":1000,"y":2000},...],"unitCount":4}
    /// ```
    ///
    /// KG: prod-wiring-wasm-rts-export-2026-04-15
    pub fn tactical_summary(&self) -> String {
        let frame = self.inner.tactical.turn_number;
        let units: Vec<_> = self.inner.tactical.units.iter().map(|u| {
            serde_json::json!({
                "id": u.unit_id,
                "owner": u.owner,
                "x": u.x.0,
                "y": u.y.0,
                "hp": u.hp,
                "maxHp": u.max_hp,
            })
        }).collect();

        let winner = self.inner.tactical.winner;
        serde_json::json!({
            "frame": frame,
            "unitCount": units.len(),
            "units": units,
            "winner": winner, // KG: sprint3-3C-game-rules-graphics-2026-04-15
        }).to_string()
    }

    /// Return the current frame number.
    pub fn current_frame(&self) -> u32 {
        self.inner.tactical.turn_number as u32
    }

    /// Return node_id for this session.
    pub fn node_id(&self) -> u32 {
        self.inner.node_id
    }
}

// ---------------------------------------------------------------------------
// Standalone stubs — UI sections 4, 5, 6
// These stubs are wired into the UI but perform no real BFT / GGRS work.
// They return a minimal JSON response that lets the UI display "approved"
// status without a live BFT cluster.  Phase 4 will replace them.
// KG: prod-wiring-wasm-rts-export-2026-04-15
// ---------------------------------------------------------------------------

/// BFT Checkpoint stub — simulates a quorum-check result.
///
/// Returns JSON: `{"frame":N,"qcStatus":"approved","validators":4,"approved":3}`
///
/// Replace with real BFT `submit_checkpoint_proof` when Phase 4 lands.
/// KG: prod-wiring-wasm-rts-export-2026-04-15
#[wasm_bindgen]
pub fn rts_bft_checkpoint_stub(frame: u32) -> String {
    serde_json::json!({
        "frame": frame,
        "qcStatus": "approved",
        "validators": 4,
        "approved": 3,
        "stub": true,
    }).to_string()
}

/// Peer eject stub — simulates AFK detection vote.
///
/// Returns JSON: `{"peerId":N,"votes":1,"threshold":3,"ejected":false}`
///
/// Replace with real BFT `submit_eject_vote` when Phase 4 lands.
/// KG: prod-wiring-wasm-rts-export-2026-04-15
#[wasm_bindgen]
pub fn rts_peer_eject_stub(peer_id: u32) -> String {
    serde_json::json!({
        "peerId": peer_id,
        "votes": 1,
        "threshold": 3,
        "ejected": false,
        "stub": true,
    }).to_string()
}

/// GGRS rollback stub — simulates rollback acknowledgement.
///
/// Returns JSON: `{"frame":N,"rolledBack":true,"resimulated":0}`
///
/// Replace with real GGRS `BftGgrsSession::load_state` when adapter is wired.
/// KG: prod-wiring-wasm-rts-export-2026-04-15
#[wasm_bindgen]
pub fn rts_ggrs_rollback_stub(frame: u32) -> String {
    // KG: sprint6C-observability-2026-04-15 — count rollback stubs as rollback events
    observability::GGRS_ROLLBACK_EVENTS_TOTAL.inc();
    serde_json::json!({
        "frame": frame,
        "rolledBack": true,
        "resimulated": 0,
        "stub": true,
    }).to_string()
}

/// Return a JSON snapshot of all observable metrics from the Rust core.
///
/// Poll this from JS every 5 seconds for a live metrics overlay.
///
/// ```js
/// const metrics = JSON.parse(wasm.get_metrics_json());
/// console.log(metrics.rts_frame_advance_total);
/// ```
///
/// KG: sprint6C-observability-2026-04-15
#[wasm_bindgen]
pub fn get_metrics_json() -> String {
    observability::json_snapshot()
}
