// KG: seed-rts-determinism-fixed-point-state-hash-2026-04-15
//! Determinism module — RTS-grade lockstep state synchronization.
//!
//! # Overview
//!
//! This module provides three pillars of deterministic simulation:
//!
//! 1. **`fixed_point`** — Q16.16 fixed-point arithmetic (`Fixed32`).
//!    All game-logic calculations use `Fixed32` instead of `f32`/`f64`.
//!    Bitwise identical across all platforms and Rust versions.
//!
//! 2. **`rng`** — Per-frame seeded RNG (`DeterministicRng`).
//!    `seed_for_frame(global_seed, frame_n)` always produces the same sequence.
//!    Based on PCG32 — fast, small, and statistically excellent.
//!
//! 3. **`state_hash`** — Frame state hashing and desync detection.
//!    `frame_state_hash(frame_n, state)` emits a Blake3 256-bit digest.
//!    `DesyncDetector` watches peer hashes and raises `DesyncEvent` on mismatch.
//!    Broadcast cadence: every `HASH_BROADCAST_INTERVAL` (10) frames.
//!
//! # Determinism Contract
//! - No `f32`/`f64` in simulation code (use `Fixed32`)
//! - No `HashMap`/`HashSet` in serialized state (use `BTreeMap`/`BTreeSet`)
//! - All RNG seeded via `DeterministicRng::seed_for_frame`
//! - All iteration over entity collections must be sorted
//! - On desync: request full snapshot from the majority of peers (BFT checkpoint)
//!
//! # KG: seed-rts-determinism-fixed-point-state-hash-2026-04-15

pub mod fixed_point;
pub mod rng;
pub mod state_hash;

// Re-exports for ergonomic use
pub use fixed_point::Fixed32;
pub use rng::DeterministicRng;
pub use state_hash::{
    frame_state_hash, should_broadcast_hash, DesyncDetector, DesyncEvent,
    DeterministicSerialize, DeterministicDeserialize, HASH_BROADCAST_INTERVAL,
};
