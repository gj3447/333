// KG: SPAN_333_Consensus
//! HotStuff BFT Consensus
//!
//! Simplified HotStuff for 333 Platform super-peer validators.
//! Only used for ordered operations (token transfer, auction, ranking).
//! Non-ordered ops use CRDT (LWW-Map).
//!
//! Based on: "HotStuff: BFT Consensus with Linear Communication"
//! (Yin, Malkhi, Reiter, Gueta, Abraham — 2019)
//!
//! Properties:
//! - Safety: f < n/3 Byzantine tolerance
//! - Liveness: optimistic responsiveness (leader-based)
//! - Finality: 3 phases × 1 round-trip = <2s practical
//! - Message complexity: O(n) per phase (linear, not O(n²))

pub mod crypto;
pub mod types;
pub mod state;
pub mod leader;
pub mod executor;
pub mod viewchange;
pub mod transport;

pub use types::*;
pub use state::HotStuffState;
pub use executor::Executor;
