//! WASM 바인딩 모듈 그룹 — 평면 src/wasm_*.rs 에서 묶음.
//! span-ancestry 사영: SPAN_333_Frontend/Runtime grouping 폴더 보존.
//! # KG: lesson-flat-structure-root-decomposition-tree-discarded-at-materialization-2026-05-30

pub mod entry; // KG: sprint5-wasm-ui-2026-04-15 (구 wasm.rs — Platform333 진입점)
pub mod editor; // KG: sprint5C-editor-wasm-ui-2026-04-15
pub mod om; // KG: sprint5B-om-wasm-ui-2026-04-15
pub mod rts; // KG: prod-wiring-wasm-rts-export-2026-04-15
pub mod social; // KG: sprint5A-social-wasm-ui-2026-04-15
pub mod shared; // KG: sprint7H-coop-coep-sharedarraybuffer-2026-04-16 — SAB ring buffer
