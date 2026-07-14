// KG: seed-rts-3tier-state-classification-2026-04-15
// RTS 3-Tier State Classification — struct skeletons + trait sketches.
// Design phase only. No logic implemented. Integration follows in subsequent work.
// Tier 1: TacticalState  → CH_POSITION lockstep, 50 ms
// Tier 2: PersistentState → CH_CRDT LwwMap delta, 20 ms
// Tier 3: CriticalEvent  → CH_BFT HotStuff ordered, per event
// Bridge: FrameEpochBridge — per-frame N HLC snapshot binding all three tiers.

use crate::hlc::Hlc;
use crate::bft::types::OrderedTx;
use crate::determinism::{Fixed32, DeterministicSerialize, DeterministicDeserialize};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------
// Tier 1 — Tactical State
// Owned by lockstep Turn execution. Every field must be deterministic.
// Channel: CH_POSITION | Sync: every Turn (~50 ms) | Desync: state_hash check
// KG: seed-rts-3tier-state-classification-2026-04-15
// ---------------------------------------------------------------------------

/// Per-unit mutable tactical snapshot (fully deterministic).
/// Sourced from RtsGame.units after each process_turn().
/// KG: taliban-fix-C1-2026-04-15
/// KG: sprint3-3B-attack-select-2026-04-15
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitTactical {
    pub unit_id: u32,
    pub owner: u32,
    /// Q16.16 fixed-point x position — replaces f32 for cross-platform determinism.
    /// KG: taliban-fix-C1-2026-04-15
    pub x: Fixed32,
    /// Q16.16 fixed-point y position — replaces f32 for cross-platform determinism.
    /// KG: taliban-fix-C1-2026-04-15
    pub y: Fixed32,
    pub hp: u32,
    pub max_hp: u32,
    /// Animation frame index — driven by last command type
    pub animation_frame: u8,
    /// Remaining cooldown ticks for active ability
    pub ability_cooldown: u16,
    // ── Combat fields (Sprint 3-B) ─────────────────────────────────────────────
    // KG: sprint3-3B-attack-select-2026-04-15
    /// Attack range in Fixed32 units — Chebyshev distance check (max(|dx|,|dy|)).
    /// Default: Fixed32(500) ≈ 0.0076 world units (scaled; use Fixed32::from_int(2) for 2 units).
    pub attack_range: Fixed32,
    /// Flat damage applied to target HP per attack hit.
    pub attack_damage: u32,
    /// Max attack cooldown ticks — reloaded into attack_cooldown_remaining after each hit.
    pub attack_cooldown_max: u32,
    /// Remaining cooldown ticks before this unit can attack again (0 = ready).
    pub attack_cooldown_remaining: u32,
}

/// Complete Tier-1 tactical snapshot for one game turn.
/// Transmitted over CH_POSITION as a lockstep packet.
/// KG: seed-rts-3tier-state-classification-2026-04-15
/// KG: taliban-fix-C1-2026-04-15
/// KG: sprint3-3C-game-rules-graphics-2026-04-15
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TacticalState {
    /// Turn number this snapshot was taken after
    pub turn_number: u64,
    /// HLC at snapshot time — binds to FrameEpochBridge
    pub hlc: Hlc,
    /// All living units
    pub units: Vec<UnitTactical>,
    /// Per-player mineral totals — BTreeMap for deterministic iteration order.
    /// KG: taliban-fix-C1-2026-04-15
    pub player_minerals: BTreeMap<u32, u32>,
    /// Fog-of-war visibility bitmask per player — BTreeMap for deterministic iteration order.
    /// KG: taliban-fix-C1-2026-04-15
    pub visibility: BTreeMap<u32, u64>,
    /// State hash for desync detection — [u8; 32] blake3 digest.
    /// Replaces String for byte-exact comparison without codec ambiguity.
    /// KG: taliban-fix-C1-2026-04-15 (H1 co-fix)
    pub state_hash: [u8; 32],
    /// Currently selected unit IDs — BTreeSet for deterministic iteration.
    /// Select command (0x03) adds to this set; deselect (0x04) removes.
    /// KG: sprint3-3B-attack-select-2026-04-15
    pub selected_units: BTreeSet<u32>,
    /// Victory state — set by check_victory() when game ends.
    /// Some(owner_id) = that owner wins; Some(0) = draw (all dead); None = in progress.
    /// KG: sprint3-3C-game-rules-graphics-2026-04-15
    pub winner: Option<u32>,
}

/// Trait: anything that can produce a TacticalState snapshot.
/// KG: seed-rts-3tier-state-classification-2026-04-15
pub trait TacticalSnapshot {
    /// Extract current Tier-1 state after turn execution.
    fn tactical_snapshot(&self, hlc: Hlc) -> TacticalState;

    /// Verify a received snapshot hash matches local computation.
    fn verify_tactical_hash(&self, snapshot: &TacticalState) -> bool;
}

// ── DeterministicSerialize implementations ───────────────────────────────────
// BTreeMap iteration is sorted by key → order-independent across all nodes.
// Fixed32 serialized as raw i32 little-endian bytes (platform-stable bit pattern).
// KG: taliban-fix-C1-2026-04-15

impl DeterministicSerialize for Fixed32 {
    /// Serialize as raw i32 little-endian bytes.
    /// Little-endian matches the spec: "raw i32 as little-endian bytes".
    /// KG: taliban-fix-C1-2026-04-15
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.0.to_le_bytes());
    }
}

impl DeterministicSerialize for u16 {
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.to_be_bytes());
    }
}

impl DeterministicSerialize for UnitTactical {
    /// Deterministic byte serialization — no HashMap, no f32.
    /// KG: taliban-fix-C1-2026-04-15
    /// KG: sprint3-3B-attack-select-2026-04-15
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        self.unit_id.det_serialize(buf);
        self.owner.det_serialize(buf);
        self.x.det_serialize(buf);
        self.y.det_serialize(buf);
        self.hp.det_serialize(buf);
        self.max_hp.det_serialize(buf);
        self.animation_frame.det_serialize(buf);
        self.ability_cooldown.det_serialize(buf);
        // Sprint 3-B combat fields — # KG: sprint3-3B-attack-select-2026-04-15
        self.attack_range.det_serialize(buf);
        self.attack_damage.det_serialize(buf);
        self.attack_cooldown_max.det_serialize(buf);
        self.attack_cooldown_remaining.det_serialize(buf);
    }
}

impl DeterministicSerialize for TacticalState {
    /// Deterministic byte serialization — BTreeMap iteration forced, Fixed32 stable.
    /// KG: taliban-fix-C1-2026-04-15
    /// KG: sprint3-3B-attack-select-2026-04-15
    /// KG: sprint3-3C-game-rules-graphics-2026-04-15
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        self.turn_number.det_serialize(buf);
        // HLC: serialize as (wall_ms: u64, counter: u64, node_id: u32)
        self.hlc.wall_ms.det_serialize(buf);
        self.hlc.counter.det_serialize(buf);
        self.hlc.node_id.det_serialize(buf);
        // Vec<UnitTactical>
        self.units.det_serialize(buf);
        // BTreeMap<u32, u32> — sorted iteration
        self.player_minerals.det_serialize(buf);
        // BTreeMap<u32, u64> — sorted iteration
        self.visibility.det_serialize(buf);
        // [u8; 32] state_hash
        self.state_hash.det_serialize(buf);
        // BTreeSet<u32> selected_units — # KG: sprint3-3B-attack-select-2026-04-15
        (self.selected_units.len() as u32).det_serialize(buf);
        for uid in &self.selected_units {
            uid.det_serialize(buf);
        }
        // winner: Option<u32> — tag(1 byte) + value(4 bytes if Some)
        // KG: sprint3-3C-game-rules-graphics-2026-04-15
        match self.winner {
            None => buf.push(0u8),
            Some(owner) => {
                buf.push(1u8);
                owner.det_serialize(buf);
            }
        }
    }
}

// ── DeterministicDeserialize implementations ─────────────────────────────────
// Mirror of DeterministicSerialize — must consume exactly the same bytes.
// KG: prod-wiring-payload-ggrs-serde-2026-04-15

impl DeterministicDeserialize for Fixed32 {
    /// Deserialize from raw i32 little-endian bytes (mirror of det_serialize).
    fn det_deserialize(buf: &[u8]) -> Result<(Self, usize), String> {
        if buf.len() < 4 {
            return Err(format!("Fixed32: need 4 bytes, got {}", buf.len()));
        }
        let val = i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        Ok((Fixed32(val), 4))
    }
}

impl DeterministicDeserialize for UnitTactical {
    /// KG: sprint3-3B-attack-select-2026-04-15
    fn det_deserialize(buf: &[u8]) -> Result<(Self, usize), String> {
        let mut pos = 0usize;

        let (unit_id, n) = u32::det_deserialize(&buf[pos..])?; pos += n;
        let (owner, n)   = u32::det_deserialize(&buf[pos..])?; pos += n;
        let (x, n)       = Fixed32::det_deserialize(&buf[pos..])?; pos += n;
        let (y, n)       = Fixed32::det_deserialize(&buf[pos..])?; pos += n;
        let (hp, n)      = u32::det_deserialize(&buf[pos..])?; pos += n;
        let (max_hp, n)  = u32::det_deserialize(&buf[pos..])?; pos += n;
        let (animation_frame, n) = u8::det_deserialize(&buf[pos..])?; pos += n;
        let (ability_cooldown, n) = u16::det_deserialize(&buf[pos..])?; pos += n;
        // Sprint 3-B combat fields
        let (attack_range, n)              = Fixed32::det_deserialize(&buf[pos..])?; pos += n;
        let (attack_damage, n)             = u32::det_deserialize(&buf[pos..])?; pos += n;
        let (attack_cooldown_max, n)       = u32::det_deserialize(&buf[pos..])?; pos += n;
        let (attack_cooldown_remaining, n) = u32::det_deserialize(&buf[pos..])?; pos += n;

        Ok((UnitTactical {
            unit_id, owner, x, y, hp, max_hp, animation_frame, ability_cooldown,
            attack_range, attack_damage, attack_cooldown_max, attack_cooldown_remaining,
        }, pos))
    }
}

impl DeterministicDeserialize for TacticalState {
    /// KG: sprint3-3B-attack-select-2026-04-15
    fn det_deserialize(buf: &[u8]) -> Result<(Self, usize), String> {
        use crate::hlc::Hlc;
        let mut pos = 0usize;

        let (turn_number, n) = u64::det_deserialize(&buf[pos..])?; pos += n;
        // HLC: wall_ms(u64 big-endian) + counter(u32 big-endian) + node_id(u32 big-endian)
        let (wall_ms, n)  = u64::det_deserialize(&buf[pos..])?; pos += n;
        let (counter, n)  = u32::det_deserialize(&buf[pos..])?; pos += n;
        let (node_id, n)  = u32::det_deserialize(&buf[pos..])?; pos += n;
        let hlc = Hlc { wall_ms, counter, node_id };
        let (units, n)            = Vec::<UnitTactical>::det_deserialize(&buf[pos..])?; pos += n;
        let (player_minerals, n)  = std::collections::BTreeMap::<u32,u32>::det_deserialize(&buf[pos..])?; pos += n;
        let (visibility, n)       = std::collections::BTreeMap::<u32,u64>::det_deserialize(&buf[pos..])?; pos += n;
        let (state_hash, n)       = <[u8; 32]>::det_deserialize(&buf[pos..])?; pos += n;
        // selected_units: BTreeSet<u32> — # KG: sprint3-3B-attack-select-2026-04-15
        let (sel_len, n) = u32::det_deserialize(&buf[pos..])?; pos += n;
        let mut selected_units = std::collections::BTreeSet::new();
        for _ in 0..sel_len {
            let (uid, n) = u32::det_deserialize(&buf[pos..])?; pos += n;
            selected_units.insert(uid);
        }

        // winner: Option<u32> — # KG: sprint3-3C-game-rules-graphics-2026-04-15
        if pos >= buf.len() {
            return Err("TacticalState: missing winner tag byte".to_string());
        }
        let winner_tag = buf[pos]; pos += 1;
        let winner = if winner_tag == 0 {
            None
        } else {
            let (owner, n) = u32::det_deserialize(&buf[pos..])?; pos += n;
            Some(owner)
        };

        Ok((TacticalState { turn_number, hlc, units, player_minerals, visibility, state_hash, selected_units, winner }, pos))
    }
}

// ---------------------------------------------------------------------------
// Tier 2 — Persistent State
// CRDT LwwMap-backed. Eventual consistency. Conflict = LWW timestamp.
// Channel: CH_CRDT | Sync: 20 ms batch via SyncManager | Model: LwwMap<String,String>
// KG: seed-rts-3tier-state-classification-2026-04-15
// ---------------------------------------------------------------------------

/// Typed wrapper over a LwwMap key-value entry for persistent game state.
/// Keys use namespaced patterns (see RTS_3TIER_STATE_MATRIX.md).
/// KG: seed-rts-3tier-state-classification-2026-04-15
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentEntry {
    pub key: String,
    pub value: String,
    /// HLC of the last write (embedded in LwwEntry but surfaced here for routing)
    pub hlc: Hlc,
}

/// Tier-2 persistent state domain categories — for routing/filtering.
/// KG: seed-rts-3tier-state-classification-2026-04-15
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistentDomain {
    Inventory,
    Alliance,
    Chat,
    Resource,
    PlayerProfile,
    BuildProgress,
    Annotation,
}

impl PersistentDomain {
    /// Returns the key prefix for this domain.
    pub fn key_prefix(self) -> &'static str {
        match self {
            PersistentDomain::Inventory => "inv:",
            PersistentDomain::Alliance => "alliance:",
            PersistentDomain::Chat => "chat:",
            PersistentDomain::Resource => "resource:",
            PersistentDomain::PlayerProfile => "player:",
            PersistentDomain::BuildProgress => "build:",
            PersistentDomain::Annotation => "annotation:",
        }
    }
}

/// Full Tier-2 persistent state — a typed view over a LwwMap snapshot.
/// Not transmitted directly; built from SyncManager full snapshot on join.
/// KG: seed-rts-3tier-state-classification-2026-04-15
#[derive(Debug, Clone)]
pub struct PersistentState {
    pub node_id: u32,
    /// Snapshot of all CRDT entries at a point in time
    pub entries: Vec<PersistentEntry>,
    /// HLC at snapshot generation
    pub snapshot_hlc: Hlc,
}

/// Trait: routing layer that classifies a LwwMap key into a PersistentDomain.
/// KG: seed-rts-3tier-state-classification-2026-04-15
pub trait PersistentRouter {
    /// Classify a CRDT key into its domain for selective sync/replay.
    fn classify_key(key: &str) -> Option<PersistentDomain>;

    /// Build a PersistentState view for a specific domain only.
    fn domain_snapshot(&self, domain: PersistentDomain) -> PersistentState;
}

// ---------------------------------------------------------------------------
// Tier 3 — Critical Events
// HotStuff BFT. Total order. Irreversible. Quorum-committed.
// Channel: CH_BFT | Sync: event-driven HotStuffMsg | Model: OrderedTx log
// KG: seed-rts-3tier-state-classification-2026-04-15
// ---------------------------------------------------------------------------

/// RTS-specific action type codes for OrderedTx::RankedAction.payload routing.
/// Values match the CH_BFT dispatch table in FRAME_EPOCH_BRIDGE_CONTRACT.md.
/// KG: seed-rts-3tier-state-classification-2026-04-15
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CriticalActionType {
    VictoryDeclaration = 0x01,
    DesyncProof = 0x02,
    ReplayAnchor = 0x03,
    ConsensusKick = 0x04,
    MatchAttestation = 0x05,
}

/// A committed critical event — output of BFT Decide phase.
/// Wraps OrderedTx with RTS metadata for post-commit dispatch.
/// KG: seed-rts-3tier-state-classification-2026-04-15
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriticalEvent {
    /// BFT block view number at commit
    pub bft_view: u64,
    /// Position in committed block (block_hash + tx_index = globally unique)
    pub block_hash: u64,
    pub tx_index: u32,
    /// The committed transaction
    pub tx: OrderedTx,
    /// HLC at time of local commit — for FrameEpochBridge anchoring
    pub commit_hlc: Hlc,
}

/// Trait: post-commit handler for critical events.
/// Each variant of OrderedTx routes to a specific handler.
/// KG: seed-rts-3tier-state-classification-2026-04-15
pub trait CriticalEventHandler {
    /// Dispatch a committed critical event to the appropriate subsystem.
    fn handle_critical(&mut self, event: CriticalEvent);

    /// True if the given action type is handled by this handler.
    fn handles(action_type: CriticalActionType) -> bool;
}

// ---------------------------------------------------------------------------
// FrameEpochBridge
// Per-frame N HLC snapshot — binds all three tiers at a single causally-ordered
// point. Desync detection, replay anchoring, and cross-tier audit trail.
// KG: seed-rts-3tier-state-classification-2026-04-15
// ---------------------------------------------------------------------------

/// Tier 1 digest included in each bridge epoch.
/// Minimal subset needed for desync detection without transmitting full snapshot.
/// KG: seed-rts-3tier-state-classification-2026-04-15
/// KG: taliban-fix-C1-2026-04-15 (H1 co-fix)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TacticalDigest {
    /// Matches TacticalState.state_hash — [u8; 32] blake3 digest.
    /// Changed from String to [u8; 32] for byte-exact comparison without codec ambiguity.
    /// KG: taliban-fix-C1-2026-04-15 (H1 co-fix)
    pub state_hash: [u8; 32],
    /// Number of living units at turn end
    pub unit_count: u32,
    /// Total minerals across all players (integrity sanity check)
    pub total_minerals: u64,
}

/// Tier 2 digest included in each bridge epoch.
/// KG: seed-rts-3tier-state-classification-2026-04-15
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentDigest {
    /// Number of LwwMap entries at this snapshot
    pub entry_count: u64,
    /// Highest HLC counter seen across all CRDT entries (from StateVector max)
    pub max_hlc_counter: u64,
}

/// Tier 3 digest included in each bridge epoch.
/// KG: seed-rts-3tier-state-classification-2026-04-15
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriticalDigest {
    /// BFT view number at time of epoch (0 if no BFT activity this frame)
    pub bft_view: u64,
    /// Hash of the last committed BFT block (0 if none committed this frame)
    pub last_committed_block_hash: u64,
    /// Count of OrderedTx committed since last bridge epoch
    pub committed_tx_count: u32,
}

/// Per-frame epoch bridge — the single causal anchor for all three tiers.
///
/// Created once per turn N by FrameEpochBridge::capture().
/// Stored in replay buffer. Used by desync detection, reconnect recovery,
/// and BFT ReplayAnchor transactions.
///
/// HLC contract: `epoch_hlc` is ticked AFTER all tier state is frozen
/// for turn N, ensuring epoch_hlc > any HLC embedded in that turn's
/// TacticalState, CRDT deltas, or BFT commits.
///
/// KG: seed-rts-3tier-state-classification-2026-04-15
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameEpoch {
    /// Turn number this epoch covers
    pub frame: u64,
    /// HLC ticked after all tier state frozen — causal upper bound for frame N
    pub epoch_hlc: Hlc,
    /// Tier 1 digest (deterministic, must match all peers)
    pub tactical: TacticalDigest,
    /// Tier 2 digest (convergent, used for drift detection)
    pub persistent: PersistentDigest,
    /// Tier 3 digest (ordered, used for replay anchoring)
    pub critical: CriticalDigest,
}

/// Builder/coordinator that captures a FrameEpoch at the end of each turn.
/// KG: seed-rts-3tier-state-classification-2026-04-15
pub struct FrameEpochBridge {
    #[allow(dead_code)] // used indirectly via hlc construction; retained for future epoch attribution
    node_id: u32,
    hlc: Hlc,
    /// Rolling buffer of recent epochs for reconnect / replay
    epoch_buffer: Vec<FrameEpoch>,
    /// Max epochs to retain before pruning (keep last N for replay window)
    buffer_capacity: usize,
}

impl FrameEpochBridge {
    /// Create a new bridge for this node.
    /// KG: seed-rts-3tier-state-classification-2026-04-15
    pub fn new(node_id: u32, buffer_capacity: usize) -> Self {
        Self {
            node_id,
            hlc: Hlc::new(node_id),
            epoch_buffer: Vec::with_capacity(buffer_capacity),
            buffer_capacity,
        }
    }

    /// Capture a FrameEpoch for turn `frame`.
    /// Call this AFTER process_turn() and AFTER CRDT poll_outgoing() for the frame.
    /// HLC is ticked here to guarantee epoch_hlc is causally after all frame events.
    /// KG: seed-rts-3tier-state-classification-2026-04-15
    pub fn capture(
        &mut self,
        frame: u64,
        tactical: TacticalDigest,
        persistent: PersistentDigest,
        critical: CriticalDigest,
    ) -> FrameEpoch {
        // Tick HLC after all tier state is frozen for this frame
        self.hlc.tick();
        let epoch = FrameEpoch {
            frame,
            epoch_hlc: self.hlc,
            tactical,
            persistent,
            critical,
        };
        self.push_epoch(epoch.clone());
        epoch
    }

    /// Update HLC on receipt of a remote epoch (causal merge).
    /// KG: seed-rts-3tier-state-classification-2026-04-15
    pub fn recv_remote_epoch(&mut self, remote_epoch: &FrameEpoch) {
        self.hlc.recv(&remote_epoch.epoch_hlc);
    }

    /// Retrieve the most recent epoch, if any.
    pub fn latest_epoch(&self) -> Option<&FrameEpoch> {
        self.epoch_buffer.last()
    }

    /// Find an epoch by frame number for replay/reconnect.
    pub fn epoch_for_frame(&self, frame: u64) -> Option<&FrameEpoch> {
        self.epoch_buffer.iter().find(|e| e.frame == frame)
    }

    /// Check if Tier-1 hashes match between local and remote epoch.
    /// Returns false → desync detected → escalate to CH_BFT DesyncProof.
    /// Byte comparison on [u8; 32] — no string codec ambiguity.
    /// KG: seed-rts-3tier-state-classification-2026-04-15
    /// KG: taliban-fix-C1-2026-04-15 (H1 co-fix)
    pub fn check_desync(&self, local: &FrameEpoch, remote: &FrameEpoch) -> bool {
        local.frame == remote.frame
            && local.tactical.state_hash == remote.tactical.state_hash  // [u8;32] == [u8;32]
    }

    // --- private ---

    fn push_epoch(&mut self, epoch: FrameEpoch) {
        if self.epoch_buffer.len() >= self.buffer_capacity {
            self.epoch_buffer.remove(0);
        }
        self.epoch_buffer.push(epoch);
    }
}

// ---------------------------------------------------------------------------
// Tests — compile-only skeletons verify types are coherent.
// KG: seed-rts-3tier-state-classification-2026-04-15
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // KG: taliban-fix-C1-2026-04-15 — updated tests for [u8;32] state_hash

    #[test]
    fn frame_epoch_bridge_captures_epoch() {
        let mut bridge = FrameEpochBridge::new(1, 16);
        let expected_hash = [0xdeu8, 0xad, 0xbe, 0xef, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                             0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let tactical = TacticalDigest {
            state_hash: expected_hash,
            unit_count: 4,
            total_minerals: 2000,
        };
        let persistent = PersistentDigest {
            entry_count: 12,
            max_hlc_counter: 7,
        };
        let critical = CriticalDigest {
            bft_view: 0,
            last_committed_block_hash: 0,
            committed_tx_count: 0,
        };
        let epoch = bridge.capture(1, tactical, persistent, critical);
        assert_eq!(epoch.frame, 1);
        assert_eq!(epoch.tactical.state_hash, expected_hash);
        assert_eq!(epoch.tactical.unit_count, 4);
        assert!(bridge.latest_epoch().is_some());
    }

    #[test]
    fn desync_detection_same_hash_passes() {
        let mut bridge = FrameEpochBridge::new(1, 4);
        let td = TacticalDigest { state_hash: [0xabu8; 32], unit_count: 1, total_minerals: 100 };
        let pd = PersistentDigest { entry_count: 0, max_hlc_counter: 0 };
        let cd = CriticalDigest { bft_view: 0, last_committed_block_hash: 0, committed_tx_count: 0 };
        let local = bridge.capture(1, td.clone(), pd.clone(), cd.clone());
        let remote = bridge.capture(1, td, pd, cd);
        assert!(bridge.check_desync(&local, &remote));
    }

    #[test]
    fn desync_detection_different_hash_fails() {
        let mut bridge = FrameEpochBridge::new(1, 4);
        let make_td = |byte: u8| TacticalDigest { state_hash: [byte; 32], unit_count: 1, total_minerals: 100 };
        let pd = PersistentDigest { entry_count: 0, max_hlc_counter: 0 };
        let cd = CriticalDigest { bft_view: 0, last_committed_block_hash: 0, committed_tx_count: 0 };
        let local = bridge.capture(1, make_td(0xaa), pd.clone(), cd.clone());
        let remote = bridge.capture(1, make_td(0xbb), pd, cd);
        assert!(!bridge.check_desync(&local, &remote));
    }

    #[test]
    fn persistent_domain_prefixes_correct() {
        assert_eq!(PersistentDomain::Inventory.key_prefix(), "inv:");
        assert_eq!(PersistentDomain::Chat.key_prefix(), "chat:");
        assert_eq!(PersistentDomain::Alliance.key_prefix(), "alliance:");
    }

    #[test]
    fn epoch_buffer_prunes_oldest() {
        let mut bridge = FrameEpochBridge::new(1, 2);
        let pd = PersistentDigest { entry_count: 0, max_hlc_counter: 0 };
        let cd = CriticalDigest { bft_view: 0, last_committed_block_hash: 0, committed_tx_count: 0 };
        for i in 0u64..3 {
            let mut hash = [0u8; 32];
            hash[0] = i as u8;
            let td = TacticalDigest { state_hash: hash, unit_count: 0, total_minerals: 0 };
            bridge.capture(i, td, pd.clone(), cd.clone());
        }
        // capacity=2, frame 0 should be pruned
        assert!(bridge.epoch_for_frame(0).is_none());
        assert!(bridge.epoch_for_frame(2).is_some());
    }

    // ── New tests for C1 / H1 fixes ─────────────────────────────────────────
    // KG: taliban-fix-C1-2026-04-15

    #[test]
    fn fixed32_xy_byte_identity() {
        // Fixed32 x/y must produce identical bytes when raw values are equal.
        use crate::determinism::DeterministicSerialize;
        let unit_a = UnitTactical {
            unit_id: 1, owner: 0,
            x: Fixed32::from_int(3), y: Fixed32::from_int(7),
            hp: 100, max_hp: 100, animation_frame: 0, ability_cooldown: 0,
            attack_range: Fixed32::from_int(5), attack_damage: 10,
            attack_cooldown_max: 3, attack_cooldown_remaining: 0,
        };
        let unit_b = UnitTactical {
            unit_id: 1, owner: 0,
            x: Fixed32::from_int(3), y: Fixed32::from_int(7),
            hp: 100, max_hp: 100, animation_frame: 0, ability_cooldown: 0,
            attack_range: Fixed32::from_int(5), attack_damage: 10,
            attack_cooldown_max: 3, attack_cooldown_remaining: 0,
        };
        let mut buf_a = Vec::new();
        let mut buf_b = Vec::new();
        unit_a.det_serialize(&mut buf_a);
        unit_b.det_serialize(&mut buf_b);
        assert_eq!(buf_a, buf_b, "UnitTactical with same Fixed32 x/y must serialize identically");
    }

    #[test]
    fn state_hash_is_u8_32_byte_comparison() {
        // TacticalDigest.state_hash is [u8;32]; check_desync uses == on byte arrays.
        let mut bridge = FrameEpochBridge::new(1, 4);
        let hash_a: [u8; 32] = {
            let mut h = [0u8; 32]; h[0] = 0x11; h
        };
        let hash_b: [u8; 32] = {
            let mut h = [0u8; 32]; h[0] = 0x22; h
        };
        let pd = PersistentDigest { entry_count: 0, max_hlc_counter: 0 };
        let cd = CriticalDigest { bft_view: 0, last_committed_block_hash: 0, committed_tx_count: 0 };
        let td_a = TacticalDigest { state_hash: hash_a, unit_count: 1, total_minerals: 0 };
        let td_b = TacticalDigest { state_hash: hash_b, unit_count: 1, total_minerals: 0 };
        let local = bridge.capture(5, td_a, pd.clone(), cd.clone());
        let remote = bridge.capture(5, td_b, pd, cd);
        assert!(!bridge.check_desync(&local, &remote), "different [u8;32] must detect desync");
        // Same hash
        let hash_same = [0x42u8; 32];
        let pd2 = PersistentDigest { entry_count: 0, max_hlc_counter: 0 };
        let cd2 = CriticalDigest { bft_view: 0, last_committed_block_hash: 0, committed_tx_count: 0 };
        let td_s1 = TacticalDigest { state_hash: hash_same, unit_count: 1, total_minerals: 0 };
        let td_s2 = TacticalDigest { state_hash: hash_same, unit_count: 1, total_minerals: 0 };
        let l2 = bridge.capture(6, td_s1, pd2.clone(), cd2.clone());
        let r2 = bridge.capture(6, td_s2, pd2, cd2);
        assert!(bridge.check_desync(&l2, &r2), "identical [u8;32] must not detect desync");
    }
}
