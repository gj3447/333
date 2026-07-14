// KG: seed-rts-bft-checkpoint-not-per-frame-2026-04-15
//! Two-layer netcode contract types and implementations.
//!
//! Fast layer: lockstep at 16-33 ms, state_hash every 10 frames.
//! Slow layer: BFT checkpoint every 10-30 s (one checkpoint per N×10 fast frames).
//!
//! # Design decisions
//! - BFT consensus is abstracted behind `BftCheckpointProvider` trait.
//!   Actual HotStuff wiring is deferred to a separate seed.
//! - `rollback_on_desync` window is capped at 5 frames (guardrail: rollback_window ≤ 5).
//! - `restore_from_checkpoint` is the escape hatch when rollback window is exceeded
//!   or a BFT-detected Byzantine fault forces a hard reset.

use crate::determinism::state_hash::{
    frame_state_hash, should_broadcast_hash,
};
use crate::bft::crypto::QuorumCert;
use std::collections::VecDeque;

// ── Constants ────────────────────────────────────────────────────────────────

/// Target fast-layer frame duration (ms): minimum bound.
pub const FAST_FRAME_MIN_MS: u64 = 16;
/// Target fast-layer frame duration (ms): maximum bound.
pub const FAST_FRAME_MAX_MS: u64 = 33;
/// How many fast frames between state_hash broadcasts.
pub const FAST_HASH_INTERVAL: u64 = 10;
/// Rollback window cap — never roll back more than 5 frames.
pub const MAX_ROLLBACK_FRAMES: u64 = 5;
/// How many fast-frame hash intervals between BFT checkpoints.
/// 1 interval = 10 fast frames ≈ 160-330 ms. 100 intervals ≈ 16-33 s.
pub const BFT_CHECKPOINT_INTERVAL_INTERVALS: u64 = 100;

// ── BftCheckpointProvider trait (stub boundary) ──────────────────────────────

/// Opaque handle returned when a checkpoint proposal is submitted to BFT layer.
/// KG: seed-rts-bft-checkpoint-not-per-frame-2026-04-15
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointHandle {
    /// Frame at which the checkpoint was taken.
    pub frame_n: u64,
    /// Blake3 hash of TacticalState at that frame.
    pub tactical_hash: [u8; 32],
}

/// Stub trait — real HotStuff wiring is a separate seed.
///
/// Implementors submit the tactical_hash to BFT consensus and return a
/// `QuorumCert` once 2f+1 validators have signed it.
///
/// # Integration point (Phase 3 / GGRS layer):
///   Replace `StubBftProvider` below with a real HotStuff adapter that wraps
///   `crate::bft::executor::Executor` and `crate::bft::state::HotStuffState`.
///
/// KG: seed-rts-bft-checkpoint-not-per-frame-2026-04-15
pub trait BftCheckpointProvider: Send {
    /// Propose a checkpoint. Returns Ok(qc) when quorum is reached,
    /// Err(reason) if the proposal is rejected or BFT stalls.
    fn propose_checkpoint(
        &mut self,
        handle: &CheckpointHandle,
    ) -> Result<QuorumCert, String>;

    /// Verify that a received checkpoint's QC is valid.
    fn verify_checkpoint_qc(
        &self,
        handle: &CheckpointHandle,
        qc: &QuorumCert,
    ) -> bool;
}

// ── TwoLayerNetcodeContract trait ────────────────────────────────────────────

/// Core behavioral contract for the two-layer netcode system.
///
/// Implementors (`FastLockstepLoop` + `SlowBftCheckpoint`) jointly fulfil this contract.
/// KG: seed-rts-bft-checkpoint-not-per-frame-2026-04-15
pub trait TwoLayerNetcodeContract {
    /// Advance the simulation by one frame, applying `inputs`.
    /// Returns the state_hash if this frame is a broadcast frame (frame_n % 10 == 0).
    fn advance_frame(&mut self, inputs: &[u8]) -> Option<[u8; 32]>;

    /// Queue a local input for the next frame.
    fn submit_input(&mut self, input: u8);

    /// Roll back to `target_frame`. Capped at `MAX_ROLLBACK_FRAMES`.
    /// Returns Err if target is outside the rollback window.
    fn rollback_on_desync(&mut self, target_frame: u64) -> Result<(), String>;

    /// Restore full state from the latest validated BFT checkpoint.
    fn restore_from_checkpoint(&mut self);

    /// Current frame number.
    fn current_frame(&self) -> u64;
}

// ── FastLockstepLoop ─────────────────────────────────────────────────────────

/// Minimal in-memory state snapshot for rollback.
#[derive(Clone, Debug)]
struct FrameSnapshot {
    frame_n: u64,
    /// Serialised state bytes (opaque; real impl holds TacticalState clone).
    state_bytes: Vec<u8>,
    /// Input applied to reach this frame (stored for replay/rollback).
    #[allow(dead_code)]
    input: u8,
}

/// Fast layer: lockstep loop with speculative execution and hash-based desync detection.
///
/// # Invariants
/// - `current_frame` advances monotonically.
/// - `snapshots` never exceeds `MAX_ROLLBACK_FRAMES` entries.
/// - A state_hash is broadcast every `FAST_HASH_INTERVAL` frames.
///
/// KG: seed-rts-bft-checkpoint-not-per-frame-2026-04-15
pub struct FastLockstepLoop {
    /// Current simulation frame.
    current_frame: u64,
    /// Ring buffer of recent snapshots for rollback.
    snapshots: VecDeque<FrameSnapshot>,
    /// Accumulated "state" — u64 stub; real impl holds TacticalState.
    state_accumulator: u64,
    /// Pending local inputs.
    input_queue: VecDeque<u8>,
    /// Latest BFT-restored frame (floor for rollback).
    checkpoint_floor: u64,
    /// Cached hash at checkpoint_floor.
    checkpoint_hash: [u8; 32],
}

impl FastLockstepLoop {
    /// Create a new loop starting at frame 0.
    pub fn new() -> Self {
        Self {
            current_frame: 0,
            snapshots: VecDeque::with_capacity(MAX_ROLLBACK_FRAMES as usize + 1),
            state_accumulator: 0,
            input_queue: VecDeque::new(),
            checkpoint_floor: 0,
            checkpoint_hash: [0u8; 32],
        }
    }
}

impl Default for FastLockstepLoop {
    fn default() -> Self {
        Self::new()
    }
}

impl FastLockstepLoop {

    /// Update checkpoint floor (called by `SlowBftCheckpoint` after quorum).
    pub fn set_checkpoint_floor(&mut self, frame_n: u64, hash: [u8; 32]) {
        self.checkpoint_floor = frame_n;
        self.checkpoint_hash = hash;
        // Evict snapshots older than the new floor.
        while self
            .snapshots
            .front()
            .map(|s| s.frame_n < frame_n)
            .unwrap_or(false)
        {
            self.snapshots.pop_front();
        }
    }

    /// Compute a deterministic hash of current stub state.
    fn compute_hash(&self) -> [u8; 32] {
        // INTEGRATION: replace `state_accumulator` with real TacticalState.
        frame_state_hash(self.current_frame, &self.state_accumulator)
    }
}

impl TwoLayerNetcodeContract for FastLockstepLoop {
    fn submit_input(&mut self, input: u8) {
        self.input_queue.push_back(input);
    }

    fn advance_frame(&mut self, inputs: &[u8]) -> Option<[u8; 32]> {
        let frame_n = self.current_frame;
        // Save snapshot before applying (enables rollback).
        let snap = FrameSnapshot {
            frame_n,
            state_bytes: self.state_accumulator.to_be_bytes().to_vec(),
            input: inputs.first().copied().unwrap_or(0),
        };
        self.snapshots.push_back(snap);
        if self.snapshots.len() > MAX_ROLLBACK_FRAMES as usize {
            self.snapshots.pop_front();
        }

        // Apply inputs (stub: XOR accumulator with each input byte).
        for &b in inputs {
            self.state_accumulator ^= (b as u64).wrapping_mul(0x9e3779b97f4a7c15);
        }
        // Also drain pending queue.
        while let Some(q) = self.input_queue.pop_front() {
            self.state_accumulator ^= (q as u64).wrapping_mul(0x9e3779b97f4a7c15);
        }

        self.current_frame += 1;

        // Broadcast hash every FAST_HASH_INTERVAL frames.
        if should_broadcast_hash(self.current_frame) {
            Some(self.compute_hash())
        } else {
            None
        }
    }

    fn rollback_on_desync(&mut self, target_frame: u64) -> Result<(), String> {
        let distance = self.current_frame.saturating_sub(target_frame);
        if distance > MAX_ROLLBACK_FRAMES {
            return Err(format!(
                "rollback distance {} exceeds cap {}",
                distance, MAX_ROLLBACK_FRAMES
            ));
        }
        if target_frame < self.checkpoint_floor {
            return Err(format!(
                "target_frame {} is below checkpoint floor {}",
                target_frame, self.checkpoint_floor
            ));
        }
        // Restore the snapshot at or just before target_frame.
        if let Some(pos) = self
            .snapshots
            .iter()
            .rposition(|s| s.frame_n <= target_frame)
        {
            let snap = self.snapshots[pos].clone();
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&snap.state_bytes[..8.min(snap.state_bytes.len())]);
            self.state_accumulator = u64::from_be_bytes(bytes);
            self.current_frame = snap.frame_n;
            self.snapshots.truncate(pos + 1);
            Ok(())
        } else {
            Err(format!("no snapshot available for frame {}", target_frame))
        }
    }

    fn restore_from_checkpoint(&mut self) {
        // Hard-reset to checkpoint floor state.
        self.state_accumulator = u64::from_be_bytes(self.checkpoint_hash[..8].try_into().unwrap_or([0u8; 8]));
        self.current_frame = self.checkpoint_floor;
        self.snapshots.clear();
    }

    fn current_frame(&self) -> u64 {
        self.current_frame
    }
}

// ── SlowBftCheckpoint ─────────────────────────────────────────────────────────

/// Slow layer: BFT checkpoint management.
///
/// Proposes a checkpoint every `BFT_CHECKPOINT_INTERVAL_INTERVALS` × `FAST_HASH_INTERVAL` frames
/// (≈ 10-30 s). After a quorum signature is received, the checkpoint becomes immutable.
///
/// # Byzantine detection
/// - A Byzantine node that submits a different `tactical_hash` will fail QC verification.
/// - `verify_checkpoint_qc` on the provider stub signals the mismatch.
///
/// KG: seed-rts-bft-checkpoint-not-per-frame-2026-04-15
pub struct SlowBftCheckpoint {
    /// Last committed checkpoint.
    pub last_checkpoint: Option<CheckpointHandle>,
    /// Last checkpoint's quorum certificate (None until first quorum).
    pub last_qc: Option<QuorumCert>,
    /// BFT provider stub — real HotStuff adapter plugged here.
    provider: Box<dyn BftCheckpointProvider>,
    /// Frame cadence: propose checkpoint every this many frames.
    checkpoint_frame_interval: u64,
}

impl SlowBftCheckpoint {
    /// Create with a BFT provider. `checkpoint_frame_interval` defaults to
    /// `BFT_CHECKPOINT_INTERVAL_INTERVALS * FAST_HASH_INTERVAL`.
    pub fn new(provider: Box<dyn BftCheckpointProvider>) -> Self {
        Self {
            last_checkpoint: None,
            last_qc: None,
            provider,
            checkpoint_frame_interval: BFT_CHECKPOINT_INTERVAL_INTERVALS
                * FAST_HASH_INTERVAL,
        }
    }

    /// Create a new checkpoint proposal for `frame_n` with `tactical_hash`.
    /// Returns the new `CheckpointHandle` on success.
    ///
    /// KG: seed-rts-bft-checkpoint-not-per-frame-2026-04-15
    pub fn create_checkpoint(
        &mut self,
        frame_n: u64,
        tactical_hash: [u8; 32],
    ) -> Result<CheckpointHandle, String> {
        let handle = CheckpointHandle { frame_n, tactical_hash };
        match self.provider.propose_checkpoint(&handle) {
            Ok(qc) => {
                self.last_qc = Some(qc);
                self.last_checkpoint = Some(handle.clone());
                Ok(handle)
            }
            Err(e) => Err(e),
        }
    }

    /// Returns true if `frame_n` is a checkpoint proposal frame.
    pub fn is_checkpoint_frame(&self, frame_n: u64) -> bool {
        frame_n > 0 && frame_n.is_multiple_of(self.checkpoint_frame_interval)
    }

    /// Verify an incoming checkpoint QC from a peer (Byzantine audit).
    pub fn verify_peer_checkpoint(
        &self,
        handle: &CheckpointHandle,
        qc: &QuorumCert,
    ) -> bool {
        self.provider.verify_checkpoint_qc(handle, qc)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bft::crypto::QuorumCert;

    /// Minimal stub provider for testing.
    struct OkProvider;
    impl BftCheckpointProvider for OkProvider {
        fn propose_checkpoint(&mut self, _h: &CheckpointHandle) -> Result<QuorumCert, String> {
            Ok(QuorumCert::genesis())
        }
        fn verify_checkpoint_qc(&self, _h: &CheckpointHandle, _qc: &QuorumCert) -> bool {
            true
        }
    }

    struct RejectProvider;
    impl BftCheckpointProvider for RejectProvider {
        fn propose_checkpoint(&mut self, _h: &CheckpointHandle) -> Result<QuorumCert, String> {
            Err("Byzantine quorum refused".into())
        }
        fn verify_checkpoint_qc(&self, _h: &CheckpointHandle, _qc: &QuorumCert) -> bool {
            false
        }
    }

    // ── Fast layer tests ──────────────────────────────────────────────────────

    #[test]
    fn fast_advance_frame_increments_counter() {
        let mut fast = FastLockstepLoop::new();
        assert_eq!(fast.current_frame(), 0);
        fast.advance_frame(&[1]);
        assert_eq!(fast.current_frame(), 1);
        fast.advance_frame(&[2]);
        assert_eq!(fast.current_frame(), 2);
    }

    #[test]
    fn fast_hash_broadcast_every_10_frames() {
        let mut fast = FastLockstepLoop::new();
        let mut broadcast_count = 0usize;
        for i in 0..30u8 {
            if fast.advance_frame(&[i]).is_some() {
                broadcast_count += 1;
            }
        }
        // frames 10, 20, 30 → 3 broadcasts
        assert_eq!(broadcast_count, 3);
    }

    #[test]
    fn fast_rollback_within_window_succeeds() {
        let mut fast = FastLockstepLoop::new();
        for i in 0..5u8 {
            fast.advance_frame(&[i]);
        }
        let frame_before_rollback = fast.current_frame();
        assert_eq!(frame_before_rollback, 5);
        // Roll back 3 frames
        let result = fast.rollback_on_desync(2);
        assert!(result.is_ok(), "rollback within window must succeed");
        assert!(fast.current_frame() <= frame_before_rollback);
    }

    #[test]
    fn fast_rollback_beyond_cap_fails() {
        let mut fast = FastLockstepLoop::new();
        for i in 0..10u8 {
            fast.advance_frame(&[i]);
        }
        // Try to roll back 8 frames (> MAX_ROLLBACK_FRAMES=5)
        let result = fast.rollback_on_desync(2);
        assert!(result.is_err(), "rollback beyond cap must fail");
    }

    #[test]
    fn fast_restore_from_checkpoint_resets_frame() {
        let mut fast = FastLockstepLoop::new();
        fast.set_checkpoint_floor(0, [0u8; 32]);
        for i in 0..5u8 {
            fast.advance_frame(&[i]);
        }
        assert_eq!(fast.current_frame(), 5);
        fast.restore_from_checkpoint();
        assert_eq!(fast.current_frame(), 0, "must reset to checkpoint floor");
    }

    // ── Slow layer tests ──────────────────────────────────────────────────────

    #[test]
    fn slow_checkpoint_ok_provider_stores_handle() {
        let mut slow = SlowBftCheckpoint::new(Box::new(OkProvider));
        let result = slow.create_checkpoint(1000, [0xAA; 32]);
        assert!(result.is_ok());
        let h = result.unwrap();
        assert_eq!(h.frame_n, 1000);
        assert_eq!(h.tactical_hash, [0xAA; 32]);
        assert!(slow.last_checkpoint.is_some());
    }

    #[test]
    fn slow_checkpoint_reject_provider_returns_err() {
        let mut slow = SlowBftCheckpoint::new(Box::new(RejectProvider));
        let result = slow.create_checkpoint(1000, [0xBB; 32]);
        assert!(result.is_err(), "Byzantine quorum refusal must propagate as Err");
    }

    #[test]
    fn slow_is_checkpoint_frame_cadence() {
        let slow = SlowBftCheckpoint::new(Box::new(OkProvider));
        let interval = BFT_CHECKPOINT_INTERVAL_INTERVALS * FAST_HASH_INTERVAL;
        assert!(slow.is_checkpoint_frame(interval));
        assert!(slow.is_checkpoint_frame(interval * 2));
        assert!(!slow.is_checkpoint_frame(interval - 1));
        assert!(!slow.is_checkpoint_frame(0));
    }
}
