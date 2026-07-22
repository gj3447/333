// SPDX-License-Identifier: AGPL-3.0-only
// Contains modified code based on Garage garage_util::crdt (GNU AGPLv3).
// Modified for 333 in 2026; see ../NOTICE and ../../THIRD_PARTY_NOTICES.md.
// KG: SPAN_333_CRDT_GaragePort, SPAN_333_L5_CRDT_Extensions, plan-333-three-way-hybrid-2026-04-17, plan-333-p2p-os-synthesis-execution-2026-04-18
// 333 P2P OS L5 State layer — CRDT library
// Garage-derived base types are modified under the compatible repository
// license; original 333 extensions use the same AGPL-3.0-only license.
//
// P3 extensions (prom32-333-p2p-2026-04-18):
//   - counter: PNCounter (fix for D11 ORSet≠counter pitfall)
//   - dvv:     Dotted Version Vectors (fix for D11 VC explosion)
//   - delta:   Delta-CRDT wrapper (D8/D10 bandwidth 50-1000x reduction)
//   - rcb:     Reliable Causal Broadcast (D8 Kleppmann causal delivery)
//   - anti_entropy: Merkle-digest reconciliation (D8 60-90% BW saving)

#![forbid(unsafe_code)]

pub mod traits;
pub mod lww;
pub mod map;
pub mod scalar;

pub mod counter;
pub mod dvv;
pub mod delta;
pub mod rcb;
pub mod anti_entropy;

pub use traits::{AutoCrdt, Crdt};
pub use lww::{Lww, LwwMap};
pub use map::Map;
pub use scalar::{Bool, Deletable};

pub use counter::PNCounter;
pub use dvv::{Dot, DvvSet, VersionVector};
pub use delta::{Delta, DeltaCrdt, DeltaWrap};
pub use rcb::{CausalBroadcast, CausalMsg, VectorClockBroadcaster};
pub use anti_entropy::{MerkleDigest, ReconcileHint, reconcile_plan};
