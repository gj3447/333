// KG: seed-rts-integration-wiring-2026-04-15
// KG: prod-wiring-payload-ggrs-serde-2026-04-15
//! RtsSession — Phase 1-3 integration with real payload parsing + GGRS serde.
//!
//! Wires together:
//!   - TacticalState + FrameEpochBridge  (rts_state_tiers)
//!   - frame_state_hash + DesyncDetector (determinism::state_hash)
//!   - BftGgrsSession stub (ggrs-adapter is a separate workspace crate;
//!     RtsSession carries an Option<GgrsStub> to keep the main crate
//!     self-contained until the ggrs-adapter is added as a dependency)
//!
//! # RtsInput Payload Format (prod-wiring-payload-ggrs-serde-2026-04-15)
//!
//! ```text
//! byte[0]      = command type
//!   0x00 = noop
//!   0x01 = move_dx_dy
//!   0x02 = attack_target  (stub — no-op body)
//!   0x03 = select_unit    (stub — no-op body)
//!
//! move_dx_dy (cmd=0x01), 11 bytes total:
//!   [0]      cmd  = 0x01
//!   [1..=2]  unit_id: u16 big-endian
//!   [3..=6]  dx: i32 little-endian (Fixed32 raw bits)
//!   [7..=10] dy: i32 little-endian (Fixed32 raw bits)
//! ```
//!
//! # Type flow
//!
//! ```text
//! advance_frame(inputs: &[RtsInput])
//!   │
//!   ├─ [GGRS stub] save_tactical_state → DeterministicSerialize → bytes + blake3
//!   │
//!   ├─ apply_inputs_to_tactical(inputs) → parse RtsInput payloads, update units
//!   │
//!   ├─ frame_state_hash(frame_n, &tactical) → [u8; 32] hash
//!   │
//!   ├─ bridge.capture(frame_n, TacticalDigest, PersistentDigest, CriticalDigest)
//!   │
//!   └─ desync.record_local(frame_n, hash)
//!
//! observe_peer_digest(peer_id, frame_n, hash)
//!   └─ desync.observe_peer(...) → Option<DesyncEvent>
//! ```
//!
//! # KG: seed-rts-integration-wiring-2026-04-15
//! # KG: prod-wiring-payload-ggrs-serde-2026-04-15

use crate::apps::rts_state_tiers::{
    CriticalDigest, FrameEpochBridge, PersistentDigest, TacticalDigest, TacticalState,
    UnitTactical,
};
use crate::determinism::{
    frame_state_hash, DeterministicSerialize, DeterministicDeserialize, DesyncDetector, DesyncEvent,
};
use crate::hlc::Hlc;
use crate::observability; // KG: sprint6C-observability-2026-04-15
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------
// RtsInput — single-player RTS command for one frame.
//
// Payload encoding (prod-wiring-payload-ggrs-serde-2026-04-15):
//   byte[0] = command type (0=noop, 1=move_dx_dy, 2=attack_target, 3=select_unit)
//   move_dx_dy (11 bytes total):
//     [0]      cmd        = 0x01
//     [1..=2]  unit_id    u16 big-endian
//     [3..=6]  dx         i32 little-endian (Fixed32 raw bits)
//     [7..=10] dy         i32 little-endian (Fixed32 raw bits)
//
// KG: prod-wiring-payload-ggrs-serde-2026-04-15
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Game rules constants
// KG: sprint3-3C-game-rules-graphics-2026-04-15
// ---------------------------------------------------------------------------

/// Map boundary — units are clamped to [0, MAP_MAX] in both axes (Fixed32 raw).
/// MAP_MAX = Fixed32::from_int(10000) = 10000 * 65536 = 655_360_000
pub const MAP_MAX: i32 = 10000 * 65536;

/// Minimum Chebyshev distance between unit centers — move blocked if closer.
/// UNIT_RADIUS = Fixed32 raw 100 (= 100/65536 ≈ 0.00153 world units).
/// Units cannot overlap: if new_pos would bring Chebyshev dist < UNIT_RADIUS → reject.
pub const UNIT_RADIUS: i32 = 100;

/// Command type constants for RtsInput payload dispatch.
/// KG: sprint3-3B-attack-select-2026-04-15
pub mod cmd {
    pub const NOOP: u8 = 0x00;
    pub const MOVE_DX_DY: u8 = 0x01;
    pub const ATTACK_TARGET: u8 = 0x02;
    pub const SELECT_UNIT: u8 = 0x03;
    pub const DESELECT_UNIT: u8 = 0x04;
}

/// One player's input for a single frame — wraps a byte payload.
///
/// Construct with [`RtsInput::noop()`] or [`RtsInput::move_unit()`].
/// The `payload` field is the canonical byte encoding consumed by
/// `apply_inputs_to_tactical`.
///
/// KG: prod-wiring-payload-ggrs-serde-2026-04-15
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtsInput {
    pub payload: Vec<u8>,
}

impl RtsInput {
    /// No-operation input — advance frame without any state change.
    pub fn noop() -> Self {
        Self { payload: vec![cmd::NOOP] }
    }

    /// Move unit `unit_id` by (dx, dy) in Fixed32 raw bits.
    ///
    /// Encoding (11 bytes):
    ///   [0] = 0x01, [1..=2] = unit_id big-endian u16,
    ///   [3..=6] = dx le-i32, [7..=10] = dy le-i32
    pub fn move_unit(unit_id: u16, dx: i32, dy: i32) -> Self {
        let mut payload = Vec::with_capacity(11);
        payload.push(cmd::MOVE_DX_DY);
        payload.extend_from_slice(&unit_id.to_be_bytes());
        payload.extend_from_slice(&dx.to_le_bytes());
        payload.extend_from_slice(&dy.to_le_bytes());
        Self { payload }
    }

    /// Attack command: attacker_id attacks target_id.
    ///
    /// Encoding (5 bytes):
    ///   [0] = 0x02, [1..=2] = attacker_id big-endian u16,
    ///   [3..=4] = target_id big-endian u16
    ///
    /// KG: sprint3-3B-attack-select-2026-04-15
    pub fn attack(attacker_id: u16, target_id: u16) -> Self {
        let mut payload = Vec::with_capacity(5);
        payload.push(cmd::ATTACK_TARGET);
        payload.extend_from_slice(&attacker_id.to_be_bytes());
        payload.extend_from_slice(&target_id.to_be_bytes());
        Self { payload }
    }

    /// Select command: add unit_id to selected_units set.
    ///
    /// Encoding (3 bytes):
    ///   [0] = 0x03, [1..=2] = unit_id big-endian u16
    ///
    /// KG: sprint3-3B-attack-select-2026-04-15
    pub fn select(unit_id: u16) -> Self {
        let mut payload = Vec::with_capacity(3);
        payload.push(cmd::SELECT_UNIT);
        payload.extend_from_slice(&unit_id.to_be_bytes());
        Self { payload }
    }

    /// Deselect command: remove unit_id from selected_units set.
    ///
    /// Encoding (3 bytes):
    ///   [0] = 0x04, [1..=2] = unit_id big-endian u16
    ///
    /// KG: sprint3-3B-attack-select-2026-04-15
    pub fn deselect(unit_id: u16) -> Self {
        let mut payload = Vec::with_capacity(3);
        payload.push(cmd::DESELECT_UNIT);
        payload.extend_from_slice(&unit_id.to_be_bytes());
        Self { payload }
    }

    /// Build from raw bytes — used in tests to construct arbitrary payloads.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self { payload: bytes.to_vec() }
    }
}

// ---------------------------------------------------------------------------
// TacticalHashView — deterministic hash over game-logic fields only.
//
// TacticalState.hlc carries a node_id and wall_ms that differ across peers
// (HLC is a local clock, not a shared game-state field).  Including it in
// the hash would cause false desync events between sessions with different
// node_ids even when the game state is identical.
//
// This newtype wraps &TacticalState and serializes only the game-logic fields:
//   turn_number, units, player_minerals, visibility.
// It intentionally excludes hlc and state_hash (the latter is the output, not input).
//
// KG: seed-rts-integration-wiring-2026-04-15
// ---------------------------------------------------------------------------

struct TacticalHashView<'a>(&'a TacticalState);

impl<'a> DeterministicSerialize for TacticalHashView<'a> {
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        let s = self.0;
        // turn_number
        s.turn_number.det_serialize(buf);
        // units
        s.units.det_serialize(buf);
        // player_minerals (BTreeMap — sorted iteration)
        s.player_minerals.det_serialize(buf);
        // visibility (BTreeMap — sorted iteration)
        s.visibility.det_serialize(buf);
        // selected_units (BTreeSet — sorted iteration) — # KG: sprint3-3B-attack-select-2026-04-15
        (s.selected_units.len() as u32).det_serialize(buf);
        for uid in &s.selected_units {
            uid.det_serialize(buf);
        }
        // NOTE: hlc and state_hash intentionally excluded — they are per-node
        // metadata, not shared game-logic state.
    }
}

// ---------------------------------------------------------------------------
// GGRS Stub — inline mock used while ggrs-adapter is not a direct dependency
// of the main crate.  Mirrors the minimum surface of BftGgrsSession needed
// by RtsSession.  Replace with real type when workspace dep is wired.
// KG: seed-rts-integration-wiring-2026-04-15
// ---------------------------------------------------------------------------

/// Opaque save cell passed to the caller for save/restore round-trips.
#[derive(Debug, Clone, Default)]
pub struct GgrsSaveCell {
    pub data: Vec<u8>,
    pub checksum: [u8; 32],
    pub frame: u32,
}

/// Minimal GGRS stub — replicates the BftGgrsSession API surface that
/// RtsSession needs, without pulling in the adapter crate as a dep.
/// KG: seed-rts-integration-wiring-2026-04-15
pub struct GgrsStub {
    current_frame: u32,
    save_slots: BTreeMap<u32, GgrsSaveCell>,
}

impl GgrsStub {
    pub fn new() -> Self {
        Self {
            current_frame: 0,
            save_slots: BTreeMap::new(),
        }
    }

    /// Maximum save slots retained simultaneously (rollback window + margin).
    /// KG: taliban2-H3-fix-2026-04-15
    const MAX_SAVE_SLOTS: usize = 8;

    /// Save state snapshot before advancing. Returns the cell for the caller to fill.
    /// Older slots are evicted once MAX_SAVE_SLOTS is reached (rolling window).
    /// KG: taliban2-H3-fix-2026-04-15
    pub fn save_state(&mut self, data: Vec<u8>) -> GgrsSaveCell {
        let checksum = *blake3::hash(&data).as_bytes();
        let cell = GgrsSaveCell {
            data,
            checksum,
            frame: self.current_frame,
        };
        // Evict oldest entry before inserting so the map stays bounded.
        // KG: sprint6C-observability-2026-04-15
        if self.save_slots.len() >= Self::MAX_SAVE_SLOTS {
            if let Some(&oldest_key) = self.save_slots.keys().next() {
                self.save_slots.remove(&oldest_key);
                observability::GGRS_SAVE_SLOT_EVICTED_TOTAL.inc();
            }
        }
        self.save_slots.insert(self.current_frame, cell.clone());
        cell
    }

    /// Serialize a TacticalState snapshot and save it for the current frame.
    ///
    /// Uses DeterministicSerialize → bytes + blake3 checksum.
    /// KG: prod-wiring-payload-ggrs-serde-2026-04-15
    pub fn save_tactical_state(&mut self, tactical: &TacticalState) -> GgrsSaveCell {
        let mut buf = Vec::with_capacity(256);
        tactical.det_serialize(&mut buf);
        self.save_state(buf)
    }

    /// Retrieve a previously saved snapshot for rollback.
    pub fn load_state(&self, frame: u32) -> Option<&GgrsSaveCell> {
        self.save_slots.get(&frame)
    }

    /// Load and deserialize a TacticalState from the saved snapshot at `frame`.
    ///
    /// Returns None if the frame was never saved or the bytes are corrupt.
    /// KG: prod-wiring-payload-ggrs-serde-2026-04-15
    pub fn load_tactical_state(&self, frame: u32) -> Option<TacticalState> {
        let cell = self.save_slots.get(&frame)?;
        // Verify checksum before deserializing
        let computed = *blake3::hash(&cell.data).as_bytes();
        if computed != cell.checksum {
            return None; // corrupted
        }
        match TacticalState::det_deserialize(&cell.data) {
            Ok((state, consumed)) if consumed == cell.data.len() => Some(state),
            _ => None,
        }
    }

    pub fn advance(&mut self) {
        self.current_frame += 1;
    }

    pub fn current_frame(&self) -> u32 {
        self.current_frame
    }
}

impl Default for GgrsStub {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// apply_inputs_to_tactical — command dispatcher
//
// Iterates all RtsInput entries in deterministic order (slice order = peer order).
// Each payload is parsed and dispatched by command byte.
// Identical byte sequences always produce identical state — determinism contract.
//
// Payload format (see module-level doc):
//   0x00 noop          — skip
//   0x01 move_dx_dy    — unit_id u16 BE, dx i32 LE (Fixed32), dy i32 LE (Fixed32)
//   0x02 attack_target — skeleton stub, no state change
//   0x03 select_unit   — skeleton stub, no state change
//
// KG: prod-wiring-payload-ggrs-serde-2026-04-15
// ---------------------------------------------------------------------------

/// Apply a slice of RTS inputs to the tactical state.
///
/// This is the game-logic heart of the lockstep simulation.  All operations
/// use Fixed32 saturating arithmetic to prevent overflow divergence.
///
/// KG: prod-wiring-payload-ggrs-serde-2026-04-15
fn apply_inputs_to_tactical(inputs: &[RtsInput], tactical: &mut TacticalState) {
    use crate::determinism::Fixed32;

    for input in inputs {
        let payload = &input.payload;
        if payload.is_empty() {
            continue; // treat empty as noop
        }
        match payload[0] {
            cmd::NOOP => {
                // no-op: skip
            }
            cmd::MOVE_DX_DY => {
                // Expect 11 bytes: [cmd, unit_id_hi, unit_id_lo, dx×4, dy×4]
                if payload.len() < 11 {
                    continue; // malformed — skip silently for determinism
                }
                let unit_id = u16::from_be_bytes([payload[1], payload[2]]) as u32;
                let dx_raw = i32::from_le_bytes([payload[3], payload[4], payload[5], payload[6]]);
                let dy_raw = i32::from_le_bytes([payload[7], payload[8], payload[9], payload[10]]);
                let dx = Fixed32(dx_raw);
                let dy = Fixed32(dy_raw);

                // Compute new position and apply boundary clamp + collision check.
                // KG: sprint3-3C-game-rules-graphics-2026-04-15

                // Find current position of the unit being moved.
                let current_pos = tactical.units.iter()
                    .find(|u| u.unit_id == unit_id)
                    .map(|u| (u.x.0, u.y.0));

                let Some((cx, cy)) = current_pos else { continue; };

                // Compute tentative new position with saturation.
                let new_x_raw = cx.saturating_add(dx.0);
                let new_y_raw = cy.saturating_add(dy.0);

                // Clamp to map boundaries [0, MAP_MAX].
                let clamped_x = new_x_raw.clamp(0, MAP_MAX);
                let clamped_y = new_y_raw.clamp(0, MAP_MAX);

                // Collision check: Chebyshev distance between clamped new pos and every
                // other alive unit must be >= UNIT_RADIUS. BTreeMap ordering via unit_id
                // ensures deterministic iteration.
                let blocked = tactical.units.iter().any(|other| {
                    if other.unit_id == unit_id { return false; }
                    let cdx = (clamped_x - other.x.0).abs();
                    let cdy = (clamped_y - other.y.0).abs();
                    let cheb = cdx.max(cdy);
                    cheb < UNIT_RADIUS
                });

                if !blocked {
                    for unit in tactical.units.iter_mut() {
                        if unit.unit_id == unit_id {
                            unit.x = Fixed32(clamped_x);
                            unit.y = Fixed32(clamped_y);
                            break;
                        }
                    }
                }
                // If blocked: silent skip — original position retained (deterministic).
            }
            cmd::ATTACK_TARGET => {
                // Attack: [cmd, attacker_id u16 BE, target_id u16 BE] = 5 bytes
                // KG: sprint3-3B-attack-select-2026-04-15
                if payload.len() < 5 {
                    continue; // malformed — skip silently for determinism
                }
                let attacker_id = u16::from_be_bytes([payload[1], payload[2]]) as u32;
                let target_id   = u16::from_be_bytes([payload[3], payload[4]]) as u32;

                // Snapshot positions to avoid simultaneous borrow
                let attacker_pos = tactical.units.iter().find(|u| u.unit_id == attacker_id)
                    .map(|u| (u.x, u.y, u.attack_range, u.attack_damage, u.attack_cooldown_max, u.attack_cooldown_remaining));
                let target_pos = tactical.units.iter().find(|u| u.unit_id == target_id)
                    .map(|u| (u.x, u.y));

                if let (Some((ax, ay, range, damage, cd_max, cd_rem)), Some((tx, ty))) =
                    (attacker_pos, target_pos)
                {
                    // Cooldown check: only attack when cd_remaining == 0
                    if cd_rem == 0 {
                        // Chebyshev distance: max(|dx|, |dy|) — no sqrt, fully deterministic
                        let dx = Fixed32((ax.0 - tx.0).abs());
                        let dy = Fixed32((ay.0 - ty.0).abs());
                        let dist = if dx.0 >= dy.0 { dx } else { dy };

                        if dist <= range {
                            // Apply damage to target
                            if let Some(target) = tactical.units.iter_mut().find(|u| u.unit_id == target_id) {
                                target.hp = target.hp.saturating_sub(damage);
                            }
                            // Set attacker cooldown
                            if let Some(attacker) = tactical.units.iter_mut().find(|u| u.unit_id == attacker_id) {
                                attacker.attack_cooldown_remaining = cd_max;
                            }
                        }
                    }
                }

                // Remove dead units (hp == 0) after attack resolution
                tactical.units.retain(|u| u.hp > 0);
            }
            cmd::SELECT_UNIT => {
                // Select: [cmd, unit_id u16 BE] = 3 bytes — add to selected_units set.
                // KG: sprint3-3B-attack-select-2026-04-15
                if payload.len() < 3 {
                    continue;
                }
                let unit_id = u16::from_be_bytes([payload[1], payload[2]]) as u32;
                // Only select units that exist
                if tactical.units.iter().any(|u| u.unit_id == unit_id) {
                    tactical.selected_units.insert(unit_id);
                }
            }
            cmd::DESELECT_UNIT => {
                // Deselect: [cmd, unit_id u16 BE] = 3 bytes — remove from selected_units set.
                // KG: sprint3-3B-attack-select-2026-04-15
                if payload.len() < 3 {
                    continue;
                }
                let unit_id = u16::from_be_bytes([payload[1], payload[2]]) as u32;
                tactical.selected_units.remove(&unit_id);
            }
            _ => {
                // Unknown command — skip for forward compatibility.
            }
        }
    }
}

// ---------------------------------------------------------------------------
// check_victory — victory condition evaluation
//
// Called at the end of each advance_frame after attack resolution.
// Rules:
//   - If all alive units belong to exactly one owner → that owner wins.
//   - If all units are dead (0 alive) → draw: winner = Some(0) sentinel.
//   - Otherwise → None (game in progress).
//
// Once winner is Some(_), subsequent advance_frame calls are no-ops.
//
// KG: sprint3-3C-game-rules-graphics-2026-04-15
// ---------------------------------------------------------------------------

/// Evaluate victory condition from current unit list.
/// Returns Some(owner_id) for a winner, Some(0) for a draw, None for in-progress.
///
/// Victory requires `initial_owner_count > 1` to avoid premature game-over
/// in single-owner scenarios (e.g. tutorial / test sessions with owner=0 only).
///
/// KG: sprint3-3C-game-rules-graphics-2026-04-15
fn check_victory(tactical: &TacticalState, initial_owner_count: usize) -> Option<u32> {
    // Only evaluate victory when the game started with multiple owners.
    if initial_owner_count <= 1 {
        return None;
    }

    if tactical.units.is_empty() {
        // All units dead — draw sentinel
        return Some(0);
    }

    // Collect unique owner IDs from alive units (BTreeSet → deterministic).
    let mut owners = std::collections::BTreeSet::new();
    for unit in &tactical.units {
        owners.insert(unit.owner);
    }

    if owners.len() == 1 {
        // Only one owner's units remain — that owner wins.
        Some(*owners.iter().next().unwrap())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// RtsSession
// ---------------------------------------------------------------------------

/// RTS Phase 1-3 integration session.
///
/// Holds Tier-1 tactical state, the FrameEpochBridge, a DesyncDetector, and
/// an optional GGRS stub (None = headless / unit-test mode).
///
/// `node_id` identifies this local node across all subsystems.
///
/// KG: seed-rts-integration-wiring-2026-04-15
pub struct RtsSession {
    /// Node identity
    pub node_id: u32,
    /// Tier-1 deterministic game state
    pub tactical: TacticalState,
    /// Frame-epoch bridge (all three tiers anchored per frame)
    pub bridge: FrameEpochBridge,
    /// Desync detector — watches local vs peer hashes
    pub desync: DesyncDetector,
    /// Optional GGRS stub (Some = rollback-capable mode, None = headless)
    pub ggrs: Option<GgrsStub>,
    /// Number of distinct owners in the initial unit list — victory requires
    /// exactly 1 surviving owner when initial_owner_count > 1.
    /// KG: sprint3-3C-game-rules-graphics-2026-04-15
    pub initial_owner_count: usize,
}

impl RtsSession {
    /// Create a new session with initial units.
    ///
    /// `bridge_capacity` sets how many frame epochs to retain for replay.
    /// `desync_retention` sets how many per-direction hashes to retain.
    /// Pass `with_ggrs = true` to enable the rollback stub.
    ///
    /// KG: seed-rts-integration-wiring-2026-04-15
    pub fn new(
        node_id: u32,
        initial_units: Vec<UnitTactical>,
        bridge_capacity: usize,
        desync_retention: usize,
        with_ggrs: bool,
    ) -> Self {
        let hlc = Hlc::zero(node_id);
        let tactical = TacticalState {
            turn_number: 0,
            hlc,
            units: initial_units,
            player_minerals: BTreeMap::new(),
            visibility: BTreeMap::new(),
            state_hash: [0u8; 32],
            selected_units: BTreeSet::new(),
            winner: None, // KG: sprint3-3C-game-rules-graphics-2026-04-15
        };
        // Count distinct owners in initial units for victory check.
        // KG: sprint3-3C-game-rules-graphics-2026-04-15
        let initial_owner_count = {
            let mut owners = std::collections::BTreeSet::new();
            for u in &tactical.units { owners.insert(u.owner); }
            owners.len()
        };
        let bridge = FrameEpochBridge::new(node_id, bridge_capacity);
        let desync = DesyncDetector::new(desync_retention);
        let ggrs = if with_ggrs { Some(GgrsStub::new()) } else { None };
        Self { node_id, tactical, bridge, desync, ggrs, initial_owner_count }
    }

    /// Advance the simulation by one frame.
    ///
    /// Steps:
    /// 1. GGRS save_tactical_state — serialize TacticalState via DeterministicSerialize.
    /// 2. apply_inputs_to_tactical — dispatch RtsInput payloads to update unit state.
    /// 3. Compute blake3 hash via frame_state_hash.
    /// 4. bridge.capture with fresh digests.
    /// 5. desync.record_local.
    ///
    /// Returns the blake3 hash for this frame (useful in tests / broadcast).
    ///
    /// KG: seed-rts-integration-wiring-2026-04-15
    /// KG: prod-wiring-payload-ggrs-serde-2026-04-15
    pub fn advance_frame(&mut self, inputs: &[RtsInput]) -> [u8; 32] {
        // KG: sprint6C-observability-2026-04-15
        // Record frame advance counter.  Latency is measured only on native targets
        // where std::time::Instant is available; on WASM the histogram is skipped.
        observability::RTS_FRAME_ADVANCE_TOTAL.inc();

        #[cfg(not(target_arch = "wasm32"))]
        let _frame_start = std::time::Instant::now();

        // If game is already over, inputs are ignored and state is frozen.
        // KG: sprint3-3C-game-rules-graphics-2026-04-15
        if self.tactical.winner.is_some() {
            return self.tactical.state_hash;
        }

        let frame_n = self.tactical.turn_number;

        // ── Step 1: GGRS save_tactical_state ─────────────────────────────────
        // Serialize TacticalState deterministically before mutating it.
        // KG: prod-wiring-payload-ggrs-serde-2026-04-15
        if let Some(ref mut stub) = self.ggrs {
            // Temporarily extract tactical to avoid borrow conflict
            let mut buf = Vec::with_capacity(256);
            self.tactical.det_serialize(&mut buf);
            stub.save_state(buf);
        }

        // ── Step 2: Apply inputs ─────────────────────────────────────────────
        // Parse RtsInput payloads and dispatch commands to update TacticalState.
        // KG: prod-wiring-payload-ggrs-serde-2026-04-15
        apply_inputs_to_tactical(inputs, &mut self.tactical);

        // ── Step 2b: Decrement attack cooldowns ───────────────────────────────
        // After all inputs are applied, tick down each unit's attack cooldown.
        // cd_remaining reaches 0 = unit is ready to attack next frame.
        // KG: sprint3-3B-attack-select-2026-04-15
        for unit in self.tactical.units.iter_mut() {
            if unit.attack_cooldown_remaining > 0 {
                unit.attack_cooldown_remaining -= 1;
            }
        }

        // ── Step 2c: Check victory condition ─────────────────────────────────
        // KG: sprint3-3C-game-rules-graphics-2026-04-15
        self.tactical.winner = check_victory(&self.tactical, self.initial_owner_count);

        self.tactical.turn_number += 1;

        // ── Step 3: Compute blake3 hash ──────────────────────────────────────
        // Hash only game-logic fields via TacticalHashView (excludes node-local hlc).
        let hash = frame_state_hash(frame_n, &TacticalHashView(&self.tactical));

        // Update tactical.state_hash field for downstream digest consumers.
        self.tactical.state_hash = hash;

        // ── Step 4: bridge.capture ───────────────────────────────────────────
        let total_minerals: u64 = self.tactical.player_minerals.values().map(|&v| v as u64).sum();
        let tactical_digest = TacticalDigest {
            state_hash: hash,
            unit_count: self.tactical.units.len() as u32,
            total_minerals,
        };
        let persistent_digest = PersistentDigest {
            entry_count: 0,
            max_hlc_counter: 0,
        };
        let critical_digest = CriticalDigest {
            bft_view: 0,
            last_committed_block_hash: 0,
            committed_tx_count: 0,
        };
        self.bridge.capture(frame_n, tactical_digest, persistent_digest, critical_digest);

        // ── Step 5: desync.record_local ──────────────────────────────────────
        self.desync.record_local(frame_n, hash);

        // Advance GGRS stub frame counter
        if let Some(ref mut stub) = self.ggrs {
            stub.advance();
        }

        // Record frame advance latency (native only; WASM has no std::time::Instant).
        // KG: sprint6C-observability-2026-04-15
        #[cfg(not(target_arch = "wasm32"))]
        {
            let elapsed_ms = _frame_start.elapsed().as_secs_f64() * 1000.0;
            observability::RTS_FRAME_ADVANCE_LATENCY_MS.observe(elapsed_ms);
        }

        hash
    }

    /// Observe a hash received from a peer.
    ///
    /// Returns `Some(DesyncEvent)` if the peer hash mismatches local for that frame.
    /// On desync, caller should log and escalate to BFT DesyncProof (Phase 4).
    ///
    /// KG: seed-rts-integration-wiring-2026-04-15
    pub fn observe_peer_digest(
        &self,
        peer_id: u32,
        frame_n: u64,
        peer_hash: [u8; 32],
    ) -> Option<DesyncEvent> {
        let event = self.desync.observe_peer(peer_id, frame_n, peer_hash);
        if let Some(ref ev) = event {
            // Log the desync — in production, escalate to BFT DesyncProof tx.
            // KG: seed-rts-integration-wiring-2026-04-15
            eprintln!(
                "[RtsSession] DESYNC frame={} peer={} local={:?} peer={:?}",
                ev.frame_n,
                ev.peer_id,
                &ev.local_hash[..4],
                &ev.peer_hash[..4],
            );
            // KG: sprint6C-observability-2026-04-15
            observability::RTS_STATE_HASH_MISMATCHES_TOTAL.inc();
        }
        event
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// KG: seed-rts-integration-wiring-2026-04-15
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::determinism::Fixed32;

    /// Helper: build two identical RtsSessions with n units.
    /// All units have default combat stats: range=10, damage=10, cooldown_max=3.
    /// KG: sprint3-3B-attack-select-2026-04-15
    fn make_pair(n: u32, with_ggrs: bool) -> (RtsSession, RtsSession) {
        let units: Vec<UnitTactical> = (0..n)
            .map(|i| UnitTactical {
                unit_id: i,
                owner: 0,
                x: Fixed32::from_int(i as i32),
                y: Fixed32::from_int(0),
                hp: 100,
                max_hp: 100,
                animation_frame: 0,
                ability_cooldown: 0,
                attack_range: Fixed32::from_int(10),
                attack_damage: 10,
                attack_cooldown_max: 3,
                attack_cooldown_remaining: 0,
            })
            .collect();
        let a = RtsSession::new(1, units.clone(), 64, 64, with_ggrs);
        let b = RtsSession::new(2, units, 64, 64, with_ggrs);
        (a, b)
    }

    // ── Test 1: Two sessions, same inputs for 30 frames → identical hashes ──
    // KG: seed-rts-integration-wiring-2026-04-15
    #[test]
    fn two_sessions_same_inputs_produce_same_hash() {
        let (mut sess_a, mut sess_b) = make_pair(4, false);
        let inputs = [RtsInput::noop()]; // non-empty → step = 1

        for frame in 0..30u64 {
            let hash_a = sess_a.advance_frame(&inputs);
            let hash_b = sess_b.advance_frame(&inputs);
            assert_eq!(
                hash_a, hash_b,
                "frame {frame}: sessions with identical inputs must produce identical hashes"
            );
        }
    }

    // ── Test 2: Diverged input → observe_peer_digest returns DesyncEvent ────
    // KG: seed-rts-integration-wiring-2026-04-15
    #[test]
    fn diverged_input_triggers_desync_event() {
        let (mut sess_a, mut sess_b) = make_pair(2, false);

        // Advance both identically for 5 frames
        for _ in 0..5 {
            sess_a.advance_frame(&[RtsInput::noop()]);
            sess_b.advance_frame(&[RtsInput::noop()]);
        }

        // On frame 5: sess_a advances with a move input, sess_b advances with no inputs
        // (move_unit changes unit position → different state → different hash)
        let _hash_a = sess_a.advance_frame(&[RtsInput::move_unit(0, 65536, 65536)]);
        let hash_b = sess_b.advance_frame(&[]); // deliberately diverged

        // sess_a observes sess_b's hash for frame 5 (turn_number was 5 when captured)
        // Both sessions captured frame 5 (before increment → frame_n = 5)
        let diverge_frame: u64 = 5;
        let event = sess_a.observe_peer_digest(2, diverge_frame, hash_b);
        assert!(
            event.is_some(),
            "differing inputs must produce different hashes → DesyncEvent"
        );
        let ev = event.unwrap();
        assert_eq!(ev.frame_n, diverge_frame);
        assert_eq!(ev.peer_id, 2);
        assert_ne!(ev.local_hash, ev.peer_hash);
    }

    // ── Test 3: Same hash for same frame → no desync event ──────────────────
    #[test]
    fn same_hash_no_desync_event() {
        let (mut sess_a, _sess_b) = make_pair(2, false);
        let hash = sess_a.advance_frame(&[RtsInput::noop()]);
        // Same hash reported by peer → no event
        let event = sess_a.observe_peer_digest(99, 0, hash);
        assert!(event.is_none(), "matching peer hash must not trigger desync");
    }

    // ── Test 4: GGRS stub save_state round-trip ──────────────────────────────
    #[test]
    fn ggrs_stub_save_and_load() {
        let (mut sess, _) = make_pair(2, true);
        sess.advance_frame(&[RtsInput::noop()]);
        let stub = sess.ggrs.as_ref().expect("stub must be Some");
        // Frame 0 was saved during the first advance_frame call
        let saved = stub.load_state(0);
        assert!(saved.is_some(), "frame 0 snapshot must be stored in stub");
        let cell = saved.unwrap();
        assert_eq!(cell.frame, 0);
        assert!(!cell.data.is_empty(), "snapshot data must be non-empty");
        assert_ne!(cell.checksum, [0u8; 32], "checksum must be non-zero");
    }

    // ── Test 5: Bridge captures epoch per frame ──────────────────────────────
    #[test]
    fn bridge_records_epoch_each_frame() {
        let (mut sess, _) = make_pair(1, false);
        for _ in 0..5 {
            sess.advance_frame(&[RtsInput::noop()]);
        }
        assert!(sess.bridge.latest_epoch().is_some(), "bridge must hold at least one epoch");
    }

    // ── Test 6: payload_move_command_updates_unit_position ───────────────────
    // move_dx_dy with dx=+0.5 (Fixed32) → unit x increases by 0.5 units.
    // Using dx=0.5 instead of 1.0 to avoid landing exactly on unit 1 at x=1.0,
    // which would trigger the collision guard (Chebyshev dist=0 < UNIT_RADIUS).
    // KG: prod-wiring-payload-ggrs-serde-2026-04-15
    // KG: sprint3-3C-game-rules-graphics-2026-04-15
    #[test]
    fn payload_move_command_updates_unit_position() {
        let (mut sess, _) = make_pair(3, false);

        // Record initial x for unit 0
        let initial_x = sess.tactical.units[0].x;

        // Issue move_dx_dy for unit_id=0, dx=+0.5 (32768 raw), dy=0.
        // Unit 1 is at x=65536; landing at 32768 has Chebyshev=32768 >> UNIT_RADIUS(100).
        let dx = 32768i32; // Fixed32(0.5) = 32768
        let dy = 0i32;
        let inp = [RtsInput::move_unit(0, dx, dy)];
        sess.advance_frame(&inp);

        let new_x = sess.tactical.units[0].x;
        let expected_x = Fixed32(initial_x.0.saturating_add(32768));
        assert_eq!(
            new_x, expected_x,
            "unit x must increase by 32768 raw (0.5 world unit) after move_dx_dy command"
        );
        // y must be unchanged (dy=0)
        assert_eq!(
            sess.tactical.units[0].y,
            Fixed32::from_int(0),
            "unit y must not change when dy=0"
        );
    }

    // ── Test 7: payload_different_bytes_different_hash ───────────────────────
    // Two inputs with different dx → produce different state → different hash.
    // KG: prod-wiring-payload-ggrs-serde-2026-04-15
    #[test]
    fn payload_different_bytes_different_hash() {
        let (mut sess_a, mut sess_b) = make_pair(2, false);

        // Session A: move unit 0 by dx=+1
        let inp_a = [RtsInput::move_unit(0, Fixed32::from_int(1).0, 0)];
        // Session B: move unit 0 by dx=+2
        let inp_b = [RtsInput::move_unit(0, Fixed32::from_int(2).0, 0)];

        let hash_a = sess_a.advance_frame(&inp_a);
        let hash_b = sess_b.advance_frame(&inp_b);

        assert_ne!(
            hash_a, hash_b,
            "different dx bytes must produce different state → different hash"
        );
    }

    // ── Test 8: ggrs_save_load_round_trip ────────────────────────────────────
    // save_tactical_state → load_tactical_state == original TacticalState.
    // KG: prod-wiring-payload-ggrs-serde-2026-04-15
    #[test]
    fn ggrs_save_load_round_trip() {
        let (mut sess, _) = make_pair(4, true);
        // Advance a few frames to get non-trivial state
        for _ in 0..5 {
            sess.advance_frame(&[RtsInput::move_unit(
                0,
                Fixed32::from_int(3).0,
                Fixed32::from_int(7).0,
            )]);
        }

        // Save the current tactical state via save_tactical_state
        let original = sess.tactical.clone();
        let stub = sess.ggrs.as_mut().expect("stub must be Some");
        let saved_cell = stub.save_tactical_state(&original);
        let saved_frame = saved_cell.frame;

        // Load it back and compare field-by-field
        let stub_ref = sess.ggrs.as_ref().unwrap();
        let restored = stub_ref
            .load_tactical_state(saved_frame)
            .expect("load_tactical_state must return Some for a just-saved frame");

        assert_eq!(restored.turn_number, original.turn_number, "turn_number round-trip");
        assert_eq!(restored.units.len(), original.units.len(), "units len round-trip");
        for (i, (orig, rest)) in original.units.iter().zip(restored.units.iter()).enumerate() {
            assert_eq!(orig.unit_id, rest.unit_id, "unit[{i}] unit_id round-trip");
            assert_eq!(orig.x, rest.x, "unit[{i}] x round-trip");
            assert_eq!(orig.y, rest.y, "unit[{i}] y round-trip");
            assert_eq!(orig.hp, rest.hp, "unit[{i}] hp round-trip");
        }
        assert_eq!(restored.state_hash, original.state_hash, "state_hash round-trip");
        assert_eq!(restored.player_minerals, original.player_minerals, "minerals round-trip");
        assert_eq!(restored.visibility, original.visibility, "visibility round-trip");
    }

    // ── Test 9: ggrs_load_unknown_frame_returns_none ─────────────────────────
    // KG: prod-wiring-payload-ggrs-serde-2026-04-15
    #[test]
    fn ggrs_load_unknown_frame_returns_none() {
        let stub = GgrsStub::new();
        let result = stub.load_tactical_state(9999);
        assert!(result.is_none(), "load_tactical_state for unsaved frame must return None");
    }

    // ── Test 10: ggrs_load_corrupted_bytes_returns_none ──────────────────────
    // KG: prod-wiring-payload-ggrs-serde-2026-04-15
    #[test]
    fn ggrs_load_corrupted_bytes_returns_none() {
        let mut stub = GgrsStub::new();
        // Save a valid state first (any non-empty bytes)
        let data = vec![0u8; 32];
        let cell = stub.save_state(data);
        let frame = cell.frame;

        // Corrupt the saved cell's data — flip a byte so checksum mismatches
        if let Some(saved_cell) = stub.save_slots.get_mut(&frame) {
            saved_cell.data[0] ^= 0xFF;
            // checksum still points to original bytes → mismatch detected on load
        }

        let result = stub.load_tactical_state(frame);
        assert!(result.is_none(), "corrupted bytes must return None from load_tactical_state");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Sprint 3-B: Attack + Select tests
    // KG: sprint3-3B-attack-select-2026-04-15
    // ═══════════════════════════════════════════════════════════════════════════

    // ── Test 11: attack_in_range_reduces_hp ──────────────────────────────────
    // Unit 0 attacks unit 1 at distance 0 — HP must decrease.
    // KG: sprint3-3B-attack-select-2026-04-15
    #[test]
    fn attack_in_range_reduces_hp() {
        let (mut sess, _) = make_pair(2, false);
        let initial_hp = sess.tactical.units[1].hp;

        // Units start at x=0 and x=1 (within default range=10)
        sess.advance_frame(&[RtsInput::attack(0, 1)]);

        let new_hp = sess.tactical.units.iter().find(|u| u.unit_id == 1).map(|u| u.hp);
        assert!(
            new_hp.is_some(),
            "target unit must still be alive (HP > 0 after one hit)"
        );
        assert!(
            new_hp.unwrap() < initial_hp,
            "target HP must decrease after in-range attack: initial={} final={}",
            initial_hp, new_hp.unwrap()
        );
    }

    // ── Test 12: attack_out_of_range_no_effect ───────────────────────────────
    // Units placed far apart — attack must have no effect.
    // KG: sprint3-3B-attack-select-2026-04-15
    #[test]
    fn attack_out_of_range_no_effect() {
        let (mut sess, _) = make_pair(2, false);
        // Move unit 1 far away (range=10, distance=1000 >> range)
        sess.tactical.units[1].x = Fixed32::from_int(1000);
        let initial_hp = sess.tactical.units[1].hp;

        sess.advance_frame(&[RtsInput::attack(0, 1)]);

        let new_hp = sess.tactical.units.iter().find(|u| u.unit_id == 1)
            .expect("unit 1 must still exist after out-of-range attack")
            .hp;
        assert_eq!(new_hp, initial_hp, "HP must not change for out-of-range attack");
    }

    // ── Test 13: attack_respects_cooldown ────────────────────────────────────
    // After a successful attack, the second attack within cooldown window must be skipped.
    // KG: sprint3-3B-attack-select-2026-04-15
    #[test]
    fn attack_respects_cooldown() {
        let (mut sess, _) = make_pair(2, false);
        let initial_hp = sess.tactical.units[1].hp;

        // First attack (cd=0 → fires)
        sess.advance_frame(&[RtsInput::attack(0, 1)]);
        let hp_after_first = sess.tactical.units.iter().find(|u| u.unit_id == 1)
            .expect("unit 1 alive after first attack").hp;
        assert!(hp_after_first < initial_hp, "first attack must land");

        // Immediately second attack (cd_remaining > 0 → should not fire)
        sess.advance_frame(&[RtsInput::attack(0, 1)]);
        let hp_after_second = sess.tactical.units.iter().find(|u| u.unit_id == 1)
            .expect("unit 1 alive after second attack").hp;
        assert_eq!(
            hp_after_second, hp_after_first,
            "second immediate attack must not land due to cooldown"
        );
    }

    // ── Test 14: attack_kills_unit ────────────────────────────────────────────
    // Attack a unit with 1 HP — it must be removed from units list.
    // KG: sprint3-3B-attack-select-2026-04-15
    #[test]
    fn attack_kills_unit() {
        let (mut sess, _) = make_pair(2, false);
        // Set target HP to 1 (less than attack_damage=10)
        sess.tactical.units[1].hp = 1;

        sess.advance_frame(&[RtsInput::attack(0, 1)]);

        let target_alive = sess.tactical.units.iter().any(|u| u.unit_id == 1);
        assert!(!target_alive, "unit with HP=1 hit by 10 damage must be removed");
    }

    // ── Test 15: select_adds_to_selected_set ─────────────────────────────────
    // Select command must add unit_id to TacticalState.selected_units.
    // KG: sprint3-3B-attack-select-2026-04-15
    #[test]
    fn select_adds_to_selected_set() {
        let (mut sess, _) = make_pair(3, false);

        assert!(sess.tactical.selected_units.is_empty(), "selected_units must start empty");

        sess.advance_frame(&[RtsInput::select(1)]);
        assert!(
            sess.tactical.selected_units.contains(&1u32),
            "unit 1 must appear in selected_units after select command"
        );

        // Select a second unit
        sess.advance_frame(&[RtsInput::select(2)]);
        assert!(sess.tactical.selected_units.contains(&2u32), "unit 2 must be selected");
        assert!(sess.tactical.selected_units.contains(&1u32), "unit 1 must remain selected");
    }

    // ── Test 16: cooldown_decrements_each_frame ───────────────────────────────
    // After an attack (cd_max=3), three advance_frame calls must bring cd_remaining to 0.
    // KG: sprint3-3B-attack-select-2026-04-15
    #[test]
    fn cooldown_decrements_each_frame() {
        let (mut sess, _) = make_pair(2, false);

        // Fire attack (cd_max=3 → cd_remaining set to 3)
        sess.advance_frame(&[RtsInput::attack(0, 1)]);
        // After advance_frame, cooldown decrements by 1 → 3-1 = 2
        let cd_after_attack = sess.tactical.units.iter().find(|u| u.unit_id == 0)
            .expect("attacker must exist").attack_cooldown_remaining;
        assert_eq!(cd_after_attack, 2, "cd must be 2 after first frame tick (set to 3 then -1)");

        sess.advance_frame(&[RtsInput::noop()]);
        let cd2 = sess.tactical.units.iter().find(|u| u.unit_id == 0).unwrap().attack_cooldown_remaining;
        assert_eq!(cd2, 1);

        sess.advance_frame(&[RtsInput::noop()]);
        let cd3 = sess.tactical.units.iter().find(|u| u.unit_id == 0).unwrap().attack_cooldown_remaining;
        assert_eq!(cd3, 0, "cd must reach 0 after 3 frames of decrement");
    }

    // ══════════════════════════════════════════════════════════════════════════
    // Sprint 3-C: Collision, Boundary, Victory tests
    // KG: sprint3-3C-game-rules-graphics-2026-04-15
    // ══════════════════════════════════════════════════════════════════════════

    /// Helper: build a session with units placed at custom positions.
    fn make_session_custom(positions: &[(u32, u32, i32, i32)]) -> RtsSession {
        // positions: (unit_id, owner, x_raw, y_raw) — Fixed32 raw values
        let units: Vec<UnitTactical> = positions.iter().map(|&(uid, owner, xr, yr)| UnitTactical {
            unit_id: uid,
            owner,
            x: Fixed32(xr),
            y: Fixed32(yr),
            hp: 100,
            max_hp: 100,
            animation_frame: 0,
            ability_cooldown: 0,
            attack_range: Fixed32::from_int(10),
            attack_damage: 10,
            attack_cooldown_max: 3,
            attack_cooldown_remaining: 0,
        }).collect();
        RtsSession::new(1, units, 64, 64, false)
    }

    // ── Test 18: move_blocked_by_collision ───────────────────────────────────
    // Two units at distance < UNIT_RADIUS after move → second move rejected.
    // KG: sprint3-3C-game-rules-graphics-2026-04-15
    #[test]
    fn move_blocked_by_collision() {
        // Unit 0 at (0, 0), unit 1 at (1000, 0) — far apart.
        // Move unit 1 to x=50 (raw 50) — within UNIT_RADIUS(100) of unit 0.
        let mut sess = make_session_custom(&[
            (0, 0, 0, 0),
            (1, 0, 1000, 0),
        ]);

        // Attempt to move unit 1 to raw x=50 — Chebyshev(|50-0|, |0-0|) = 50 < 100 → blocked
        let dx = 50 - 1000; // raw delta to land at x=50
        let inp = RtsInput::move_unit(1, dx, 0);
        sess.advance_frame(&[inp]);

        // Unit 1 must remain at x=1000 (blocked)
        let u1_x = sess.tactical.units.iter().find(|u| u.unit_id == 1).unwrap().x.0;
        assert_eq!(u1_x, 1000, "move into collision zone must be rejected: unit stays at x=1000, got {}", u1_x);
    }

    // ── Test 19: move_clamped_at_boundary ────────────────────────────────────
    // Attempting to move a unit beyond MAP_MAX must clamp to MAP_MAX.
    // KG: sprint3-3C-game-rules-graphics-2026-04-15
    #[test]
    fn move_clamped_at_boundary() {
        // Unit at x=MAP_MAX - 1000, unit 1 far away so no collision
        let start_x = MAP_MAX - 1000;
        let mut sess = make_session_custom(&[
            (0, 0, start_x, 0),
            (1, 0, 0, MAP_MAX), // far away, no collision
        ]);

        // Try to move unit 0 by +2000 raw — would land at MAP_MAX+1000 → clamped.
        let inp = RtsInput::move_unit(0, 2000, 0);
        sess.advance_frame(&[inp]);

        let u0_x = sess.tactical.units.iter().find(|u| u.unit_id == 0).unwrap().x.0;
        assert_eq!(u0_x, MAP_MAX, "x exceeding MAP_MAX must be clamped to MAP_MAX, got {}", u0_x);
    }

    // ── Test 20: victory_single_owner_remaining ───────────────────────────────
    // When all non-owner-0 units die, owner 0 is declared winner.
    // KG: sprint3-3C-game-rules-graphics-2026-04-15
    #[test]
    fn victory_single_owner_remaining() {
        // Two units: owner 0 at (0,0) with hp=100, owner 1 at (0,0) with hp=1.
        let mut sess = make_session_custom(&[
            (0, 0, 0, 0),
            (1, 1, 0, 0),
        ]);
        // Set owner-1 unit hp to 1 so one attack kills it.
        sess.tactical.units[1].hp = 1;

        // Unit 0 attacks unit 1 — one-shot kill.
        sess.advance_frame(&[RtsInput::attack(0, 1)]);

        assert_eq!(
            sess.tactical.winner,
            Some(0),
            "owner 0 must be declared winner after all owner-1 units die"
        );
    }

    // ── Test 21: victory_draw_all_dead ────────────────────────────────────────
    // When all units die in the same frame, winner = Some(0) sentinel (draw).
    // Setup: unit 0 (owner 0, hp=1) and unit 2 (owner 0, hp=1) attack-kill each
    // other across owners in sequence within one frame, leaving no alive units.
    // attack(0→1) kills unit 1; attack(2→0) kills unit 0 → both owners have zero alive.
    // KG: sprint3-3C-game-rules-graphics-2026-04-15
    #[test]
    fn victory_draw_all_dead() {
        // 3 units: owner-0 (unit 0), owner-1 (unit 1, unit 2).
        // Unit 0 hp=1 — will be killed by unit 2's attack.
        // Unit 1 hp=1 — will be killed by unit 0's attack.
        // Attack sequence: attack(0→1) kills unit 1; attack(2→0) kills unit 0.
        // Result: only unit 2 from owner-1 remains? No — unit 2 survives.
        // We need a scenario where NO unit survives.
        // Instead: use 2 units; bypass to make both dead via direct state manipulation.
        // Direct approach: set units to empty before advance_frame, then noop.
        let mut sess = make_session_custom(&[
            (0, 0, 0, 0),
            (1, 1, 0, 0),
        ]);
        // Directly empty the unit list to simulate all units dying.
        // Then advance_frame (noop) → check_victory sees empty units → draw.
        sess.tactical.units.clear();
        sess.advance_frame(&[RtsInput::noop()]);

        assert!(sess.tactical.units.is_empty(), "units must remain empty");
        assert_eq!(
            sess.tactical.winner,
            Some(0),
            "draw (all dead) must set winner = Some(0) sentinel"
        );
    }

    // ── Test 22: winner_set_then_no_more_inputs ───────────────────────────────
    // After winner is set, advance_frame must be a no-op (state frozen).
    // KG: sprint3-3C-game-rules-graphics-2026-04-15
    #[test]
    fn winner_set_then_no_more_inputs() {
        let mut sess = make_session_custom(&[
            (0, 0, 0, 0),
            (1, 1, 0, 0),
        ]);
        sess.tactical.units[1].hp = 1; // one-shot

        // Kill owner-1 unit → winner = Some(0)
        sess.advance_frame(&[RtsInput::attack(0, 1)]);
        assert_eq!(sess.tactical.winner, Some(0));

        let frozen_turn = sess.tactical.turn_number;
        let frozen_hash = sess.tactical.state_hash;
        let frozen_unit_count = sess.tactical.units.len();

        // Try to advance more frames — must be no-ops.
        sess.advance_frame(&[RtsInput::move_unit(0, Fixed32::from_int(100).0, 0)]);
        sess.advance_frame(&[RtsInput::noop()]);

        assert_eq!(sess.tactical.turn_number, frozen_turn, "turn_number must not advance after game over");
        assert_eq!(sess.tactical.state_hash, frozen_hash, "state_hash must be frozen after winner set");
        assert_eq!(sess.tactical.units.len(), frozen_unit_count, "unit count must not change after game over");
        assert_eq!(sess.tactical.winner, Some(0), "winner must remain Some(0)");
    }

    // ── Test 17: attack_deterministic ────────────────────────────────────────
    // Same input sequence on two sessions must yield identical state_hash.
    // KG: sprint3-3B-attack-select-2026-04-15
    #[test]
    fn attack_deterministic() {
        let (mut sess_a, mut sess_b) = make_pair(4, false);

        let sequence = [
            RtsInput::attack(0, 1),
            RtsInput::noop(),
            RtsInput::noop(),
            RtsInput::noop(),
            RtsInput::attack(0, 1), // cd should be 0 again after 3 noop frames
            RtsInput::select(2),
        ];

        for input in &sequence {
            let ha = sess_a.advance_frame(std::slice::from_ref(input));
            let hb = sess_b.advance_frame(std::slice::from_ref(input));
            assert_eq!(
                ha, hb,
                "determinism violated: sessions diverged after input {:?}",
                input
            );
        }
    }

    /// KG: taliban2-H3-fix-2026-04-15
    /// GgrsStub save_slots must behave as a rolling window capped at MAX_SAVE_SLOTS=8.
    /// Inserting more than 8 snapshots must evict the oldest entry so the map stays bounded.
    #[test]
    fn ggrs_save_slots_ring_buffer_cap() {
        let mut stub = GgrsStub::new();
        let cap = GgrsStub::MAX_SAVE_SLOTS;

        // Insert cap+4 snapshots sequentially.
        for i in 0..(cap + 4) as u32 {
            stub.save_state(vec![i as u8; 16]);
            stub.advance();
        }

        // Map must never exceed cap.
        assert!(
            stub.save_slots.len() <= cap,
            "save_slots exceeded cap: len={} cap={}",
            stub.save_slots.len(),
            cap
        );

        // Oldest frames (0..3) must have been evicted; most-recent (cap..cap+3) must exist.
        assert!(
            stub.load_state(0).is_none(),
            "frame 0 should have been evicted"
        );
        let last_saved = (cap + 3) as u32;
        assert!(
            stub.load_state(last_saved).is_some(),
            "most-recent frame {} should still be present",
            last_saved
        );
    }
}

