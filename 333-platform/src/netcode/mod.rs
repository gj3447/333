// KG: seed-rts-bft-checkpoint-not-per-frame-2026-04-15
//! Two-Layer Netcode — Fast lockstep (16-33 ms) + Slow BFT checkpoint (10-30 s).
//!
//! Architecture:
//! - **Fast layer**: P2P direct lockstep. Every frame advances deterministically.
//!   Every 10 frames, each peer broadcasts a `state_hash`. On mismatch → rollback.
//! - **Slow layer**: BFT checkpoint every 10-30 s. Immutable ledger snapshot.
//!   Byzantine detection delegate to `BftCheckpointProvider` trait (stub here).
//!
//! Input flow:
//!   `submit_input(local)` → fast queue → `advance_frame()` → every 10 frames
//!   fast hash broadcast → every N×10 frames `create_checkpoint()` proposed to BFT layer.
//!
//! Phase 1-3 integration points are marked with `// INTEGRATION:` comments.

pub mod two_layer;
pub mod divergence;
pub mod peer_eject; // KG: seed-rts-verify-bft-vote-peer-eject-2026-04-15
pub mod bft_bridge; // KG: seed-post-rts-bft-bridge-2026-04-15
pub mod bft_multi_node;  // KG: prod-wiring-bft-multi-node-2026-04-15
pub mod transport;       // KG: sprint7G-webrtc-transport-2026-04-16 — BftTransport trait + MpscTransport
pub mod platform_mesh;   // KG: sprint7G-webrtc-transport-2026-04-16 — PlatformCore full-stack mesh
pub mod dc_transport;    // KG: sprint7I-dc-transport-tdd-2026-04-16 — DataChannel → BftTransport bridge

pub use two_layer::{
    TwoLayerNetcodeContract, FastLockstepLoop, SlowBftCheckpoint,
    BftCheckpointProvider, CheckpointHandle,
};
pub use divergence::FrameDivergenceDetector;
