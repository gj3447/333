// KG: CONTRACT_333_Compute_Task, SPAN_333_Compute_Task
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-compute-task-TaskId/TaskType/TaskStatus/TaskAssignment/TaskResult/Task/VerifyResult
// 작업 정의 + 결과 + 검증 레이어

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Task ID (unique) // KG: ST_TaskId
pub type TaskId = u64;

/// Compute task types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskType {
    /// 당혹병렬 — 검증 불필요, 결과 집계만
    Embarrassing {
        /// 작업 설명 (예: "Monte Carlo 100K samples")
        description: String,
        /// 입력 데이터 (직렬화된 바이트)
        input: Vec<u8>,
    },
    /// 퀴럼 검증 — 3노드가 동일 작업 실행, 다수결
    Quorum {
        description: String,
        input: Vec<u8>,
        /// 필요 redundancy (보통 3)
        replicas: u32,
    },
    /// Canary — 답을 이미 아는 검증용 태스크
    Canary {
        description: String,
        input: Vec<u8>,
        /// 정답 해시 (SHA-256)
        expected_hash: String,
    },
}

/// Task status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,      // 대기 중
    Assigned,     // 노드에 배정됨
    Computing,    // 실행 중
    Completed,    // 완료 (결과 있음)
    Verified,     // 검증 완료
    Failed,       // 실패 (타임아웃 or 사기)
}

/// Task assignment to a worker node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAssignment {
    pub task_id: TaskId,
    pub worker_id: u32,
    pub assigned_at: u64,   // timestamp ms
    pub timeout_ms: u64,    // 타임아웃 (기본 30초)
    pub reward: u64,        // 333 토큰 보상
}

/// Worker result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: TaskId,
    pub worker_id: u32,
    pub output: Vec<u8>,
    pub compute_ms: u64,    // 연산 소요 시간
    pub output_hash: String, // SHA-256 of output (for quorum comparison)
}

/// Task with full metadata
#[derive(Debug, Clone)]
pub struct Task {
    pub id: TaskId,
    pub task_type: TaskType,
    pub status: TaskStatus,
    pub created_at: u64,
    pub assignments: Vec<TaskAssignment>,
    pub results: Vec<TaskResult>,
}

/// Verification result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyResult {
    /// 당혹병렬 — 항상 통과
    NoVerificationNeeded,
    /// 퀴럼 — 다수결 일치
    QuorumAgreed { agreed_hash: String, agree_count: u32 },
    /// 퀴럼 — 불일치 (사기 의심)
    QuorumDisagreed { hashes: Vec<String> },
    /// Canary — 정답 일치
    CanaryPassed,
    /// Canary — 정답 불일치 (사기 확인)
    CanaryFailed { worker_id: u32 },
    /// 결과 부족 (타임아웃)
    InsufficientResults,
}

impl Task {
    pub fn new(id: TaskId, task_type: TaskType, now_ms: u64) -> Self {
        Self {
            id,
            task_type,
            status: TaskStatus::Pending,
            created_at: now_ms,
            assignments: Vec::new(),
            results: Vec::new(),
        }
    }

    /// 결과 제출
    pub fn submit_result(&mut self, result: TaskResult) {
        self.results.push(result);
    }

    /// 검증 수행
    pub fn verify(&mut self) -> VerifyResult {
        match &self.task_type {
            TaskType::Embarrassing { .. } => {
                if !self.results.is_empty() {
                    self.status = TaskStatus::Verified;
                    VerifyResult::NoVerificationNeeded
                } else {
                    VerifyResult::InsufficientResults
                }
            }
            TaskType::Quorum { replicas, .. } => {
                if self.results.len() < *replicas as usize {
                    return VerifyResult::InsufficientResults;
                }
                // 해시 다수결
                let mut hash_count: HashMap<&str, u32> = HashMap::new();
                for r in &self.results {
                    *hash_count.entry(&r.output_hash).or_default() += 1;
                }
                // 가장 많은 해시
                if let Some((hash, count)) = hash_count.iter().max_by_key(|(_, c)| *c) {
                    let threshold = (*replicas).div_ceil(2); // 과반수
                    if *count >= threshold {
                        self.status = TaskStatus::Verified;
                        VerifyResult::QuorumAgreed {
                            agreed_hash: hash.to_string(),
                            agree_count: *count,
                        }
                    } else {
                        self.status = TaskStatus::Failed;
                        VerifyResult::QuorumDisagreed {
                            hashes: hash_count.keys().map(|h| h.to_string()).collect(),
                        }
                    }
                } else {
                    VerifyResult::InsufficientResults
                }
            }
            TaskType::Canary { expected_hash, .. } => {
                if let Some(result) = self.results.first() {
                    if result.output_hash == *expected_hash {
                        self.status = TaskStatus::Verified;
                        VerifyResult::CanaryPassed
                    } else {
                        self.status = TaskStatus::Failed;
                        VerifyResult::CanaryFailed { worker_id: result.worker_id }
                    }
                } else {
                    VerifyResult::InsufficientResults
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embarrassing_no_verification() {
        let mut task = Task::new(1, TaskType::Embarrassing {
            description: "Monte Carlo".into(),
            input: vec![1, 2, 3],
        }, 0);
        task.submit_result(TaskResult {
            task_id: 1, worker_id: 10, output: vec![42],
            compute_ms: 100, output_hash: "abc".into(),
        });
        assert_eq!(task.verify(), VerifyResult::NoVerificationNeeded);
        assert_eq!(task.status, TaskStatus::Verified);
    }

    #[test]
    fn quorum_3_agree() {
        let mut task = Task::new(2, TaskType::Quorum {
            description: "hash search".into(), input: vec![], replicas: 3,
        }, 0);
        for w in [10, 20, 30] {
            task.submit_result(TaskResult {
                task_id: 2, worker_id: w, output: vec![1],
                compute_ms: 50, output_hash: "same_hash".into(),
            });
        }
        match task.verify() {
            VerifyResult::QuorumAgreed { agree_count, .. } => assert_eq!(agree_count, 3),
            other => panic!("expected QuorumAgreed, got {:?}", other),
        }
    }

    #[test]
    fn quorum_2_of_3_agree() {
        let mut task = Task::new(3, TaskType::Quorum {
            description: "test".into(), input: vec![], replicas: 3,
        }, 0);
        task.submit_result(TaskResult { task_id: 3, worker_id: 10, output: vec![], compute_ms: 0, output_hash: "good".into() });
        task.submit_result(TaskResult { task_id: 3, worker_id: 20, output: vec![], compute_ms: 0, output_hash: "good".into() });
        task.submit_result(TaskResult { task_id: 3, worker_id: 30, output: vec![], compute_ms: 0, output_hash: "bad".into() });
        match task.verify() {
            VerifyResult::QuorumAgreed { agree_count, .. } => assert_eq!(agree_count, 2),
            other => panic!("expected QuorumAgreed, got {:?}", other),
        }
    }

    #[test]
    fn quorum_disagree() {
        let mut task = Task::new(4, TaskType::Quorum {
            description: "test".into(), input: vec![], replicas: 3,
        }, 0);
        task.submit_result(TaskResult { task_id: 4, worker_id: 10, output: vec![], compute_ms: 0, output_hash: "a".into() });
        task.submit_result(TaskResult { task_id: 4, worker_id: 20, output: vec![], compute_ms: 0, output_hash: "b".into() });
        task.submit_result(TaskResult { task_id: 4, worker_id: 30, output: vec![], compute_ms: 0, output_hash: "c".into() });
        match task.verify() {
            VerifyResult::QuorumDisagreed { .. } => {} // 3개 다 다르면 불일치
            other => panic!("expected QuorumDisagreed, got {:?}", other),
        }
    }

    #[test]
    fn canary_pass() {
        let mut task = Task::new(5, TaskType::Canary {
            description: "trap".into(), input: vec![], expected_hash: "correct".into(),
        }, 0);
        task.submit_result(TaskResult { task_id: 5, worker_id: 10, output: vec![], compute_ms: 0, output_hash: "correct".into() });
        assert_eq!(task.verify(), VerifyResult::CanaryPassed);
    }

    #[test]
    fn canary_fail_catches_cheater() {
        let mut task = Task::new(6, TaskType::Canary {
            description: "trap".into(), input: vec![], expected_hash: "correct".into(),
        }, 0);
        task.submit_result(TaskResult { task_id: 6, worker_id: 99, output: vec![], compute_ms: 0, output_hash: "wrong".into() });
        assert_eq!(task.verify(), VerifyResult::CanaryFailed { worker_id: 99 });
    }

    #[test]
    fn insufficient_results() {
        let mut task = Task::new(7, TaskType::Quorum {
            description: "test".into(), input: vec![], replicas: 3,
        }, 0);
        assert_eq!(task.verify(), VerifyResult::InsufficientResults);
    }

    // KG: CONTRACT_333_Compute_Task T1 — task creation sets Pending status
    #[test]
    fn create_task_pending() {
        let task = Task::new(100, TaskType::Embarrassing {
            description: "new".into(), input: vec![],
        }, 42);
        assert_eq!(task.id, 100);
        assert_eq!(task.status, TaskStatus::Pending);
        assert_eq!(task.created_at, 42);
        assert!(task.results.is_empty());
        assert!(task.assignments.is_empty());
    }

    // KG: CONTRACT_333_Compute_Task T2 — submitting result to unassigned task still stores result
    #[test]
    fn submit_result_without_assignment() {
        let mut task = Task::new(200, TaskType::Embarrassing {
            description: "no assign".into(), input: vec![],
        }, 0);
        // Submit result directly (no assignment step)
        task.submit_result(TaskResult {
            task_id: 200, worker_id: 5,
            output: vec![1, 2, 3], compute_ms: 10,
            output_hash: "h".into(),
        });
        assert_eq!(task.results.len(), 1);
        // Verify still works — Embarrassing accepts any result
        assert_eq!(task.verify(), VerifyResult::NoVerificationNeeded);
    }

    // KG: CONTRACT_333_Compute_Task T4 — verify on already-Verified task returns same result
    #[test]
    fn verify_completed_task_idempotent() {
        let mut task = Task::new(300, TaskType::Embarrassing {
            description: "done".into(), input: vec![],
        }, 0);
        task.submit_result(TaskResult {
            task_id: 300, worker_id: 1,
            output: vec![42], compute_ms: 5,
            output_hash: "h".into(),
        });
        let v1 = task.verify();
        assert_eq!(v1, VerifyResult::NoVerificationNeeded);
        assert_eq!(task.status, TaskStatus::Verified);
        // Calling verify again is idempotent
        let v2 = task.verify();
        assert_eq!(v2, VerifyResult::NoVerificationNeeded);
    }

    // KG: CONTRACT_333_Compute_Task T7 — all three TaskType variants can be created
    #[test]
    fn all_task_types_create() {
        let t1 = Task::new(401, TaskType::Embarrassing {
            description: "e".into(), input: vec![],
        }, 0);
        let t2 = Task::new(402, TaskType::Quorum {
            description: "q".into(), input: vec![], replicas: 3,
        }, 0);
        let t3 = Task::new(403, TaskType::Canary {
            description: "c".into(), input: vec![], expected_hash: "h".into(),
        }, 0);
        assert_eq!(t1.status, TaskStatus::Pending);
        assert_eq!(t2.status, TaskStatus::Pending);
        assert_eq!(t3.status, TaskStatus::Pending);
    }
}
