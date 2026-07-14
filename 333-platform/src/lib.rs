// KG: 333_Platform (SemanticAnchor), SPAN_333_ROOT
// 333 Platform — P2P state sync with HLC, LWW-Map, CRDT, BFT, WebRTC

// KG: phase-0-allocator-swap-2026-04-14, lesson-333-wasm-memory-oob-2026-04-14
// dlmalloc 0.2.11 heap corruption (4-tab load) 회피 → lol_alloc FreeListAllocator.
// wasm32 전용: native 빌드는 system allocator 유지 → Rust 227 unit test 영향 없음.
#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

pub mod hlc;
pub mod lamport;
pub mod crdt; // KG: SPAN_333_CRDT grouping 보존 (lww_map/or_set/rga)
// Facade — 기존 crate::lww_map / crate::or_set / crate::rga 경로 무수정 보존 (D1 정전).
pub use crdt::{lww_map, or_set, rga};
pub mod wire;
pub mod tokenomics;
pub mod p2p;
pub mod storage;
pub mod sdk;
pub mod compute;
pub mod platform;
pub mod bft;
pub mod crypto_real;
pub mod apps;
pub mod security;
pub mod wasm; // KG: SPAN_333_Frontend/Runtime grouping 보존 (entry/editor/om/rts/social/shared)
pub mod sync;        // KG: CONTRACT_333_INT_CrdtSync, TASK_333_INT_CrdtSync
pub mod dispatch;    // KG: CONTRACT_333_MessageDispatcher, ATOM_Wire_Dispatcher
pub mod determinism; // KG: seed-rts-determinism-fixed-point-state-hash-2026-04-15
pub mod netcode;     // KG: seed-rts-bft-checkpoint-not-per-frame-2026-04-15
pub mod observability; // KG: sprint6C-observability-2026-04-15
pub mod kernel;      // KG: sprint7F-puter-quality-port-2026-04-16 — P1/P2/P3/P4/P5/P10
