//! CRDT 모듈 그룹 — 평면 src/ 에서 span-ancestry 사영으로 묶음.
//! SPAN_333_CRDT grouping span 이 폴더로 보존됨 (materialization discard 정정).
//! KG: lesson-flat-structure-root-decomposition-tree-discarded-at-materialization-2026-05-30
//! KG: finding-333-within-project-grouping-discard-contrast-2026-05-30

pub mod lww_map; // KG: CONTRACT_333_LwwMap
pub mod or_set; // KG: CONTRACT_333_ORSet
pub mod rga; // KG: CONTRACT_333_RGA
