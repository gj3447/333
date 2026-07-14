// KG: CONTRACT_333_Compute_OM, CONTRACT_333_Compute_Scheduler, CONTRACT_333_Compute_Task
// 333 Distributed Compute — 브라우저 분산 컴퓨팅 엔진
// Phase 1: 당혹병렬 (embarrassingly parallel) — 검증 불필요
// Phase 2: 3-way 퀴럼 검증 (BOINC 방식)
// Phase 3: Canary 태스크 (10% 함정)

pub mod task;
pub mod scheduler;
pub mod om;
