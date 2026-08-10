// KG: CONTRACT_333_Compute_Task, SPAN_333_Compute_Task
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-compute-task-TaskId/TaskType/TaskStatus/TaskAssignment/TaskResult/Task/VerifyResult
// 작업 정의 + 결과 + 검증 레이어

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

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

/// Canonical SHA-256 digest of exact task output bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OutputDigest([u8; 32]);

impl OutputDigest {
    pub const ALGORITHM: &'static str = "sha256";

    pub fn of(output: &[u8]) -> Self {
        let digest = Sha256::digest(output);
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&digest);
        Self(bytes)
    }

    /// Parse the wire representation. Only canonical lowercase 64-byte hex is accepted.
    pub fn from_hex(value: &str) -> Option<Self> {
        let encoded = value.as_bytes();
        if encoded.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            let high = decode_hex_nibble(encoded[index * 2])?;
            let low = decode_hex_nibble(encoded[index * 2 + 1])?;
            *byte = (high << 4) | low;
        }
        Some(Self(bytes))
    }

    pub fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(64);
        for byte in self.0 {
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
        encoded
    }
}

fn decode_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Canonical wire hash for a task output.
pub fn output_hash(output: &[u8]) -> String {
    OutputDigest::of(output).to_hex()
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskResultError {
    WrongTask { expected: TaskId, actual: TaskId },
    NotAcceptingResults { status: TaskStatus },
    UnassignedWorker { worker_id: u32 },
    DuplicateWorker { worker_id: u32 },
    OutputHashMismatch { claimed: String, actual: String },
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

    pub fn required_assignments(&self) -> usize {
        match &self.task_type {
            TaskType::Quorum { replicas, .. } => *replicas as usize,
            TaskType::Embarrassing { .. } | TaskType::Canary { .. } => 1,
        }
    }

    pub fn can_assign(&self, worker_id: u32) -> bool {
        matches!(self.status, TaskStatus::Pending | TaskStatus::Assigned | TaskStatus::Computing)
            && self.assignments.len() < self.required_assignments()
            && !self.assignments.iter().any(|assignment| {
                assignment.task_id == self.id && assignment.worker_id == worker_id
            })
    }

    pub fn record_assignment(&mut self, assignment: TaskAssignment) -> bool {
        if assignment.task_id != self.id || !self.can_assign(assignment.worker_id) {
            return false;
        }
        self.assignments.push(assignment);
        if self.status == TaskStatus::Pending {
            self.status = TaskStatus::Assigned;
        }
        true
    }

    /// Compatibility wrapper. New callers should use `try_submit_result` so a
    /// rejected result cannot be mistaken for an accepted state transition.
    pub fn submit_result(&mut self, result: TaskResult) {
        let _ = self.try_submit_result(result);
    }

    pub fn try_submit_result(&mut self, result: TaskResult) -> Result<(), TaskResultError> {
        if result.task_id != self.id {
            return Err(TaskResultError::WrongTask {
                expected: self.id,
                actual: result.task_id,
            });
        }
        if !matches!(self.status, TaskStatus::Assigned | TaskStatus::Computing) {
            return Err(TaskResultError::NotAcceptingResults {
                status: self.status,
            });
        }
        if !self.assignments.iter().any(|assignment| {
            assignment.task_id == self.id && assignment.worker_id == result.worker_id
        }) {
            return Err(TaskResultError::UnassignedWorker {
                worker_id: result.worker_id,
            });
        }
        if self.results.iter().any(|existing| existing.worker_id == result.worker_id) {
            return Err(TaskResultError::DuplicateWorker {
                worker_id: result.worker_id,
            });
        }
        let actual = OutputDigest::of(&result.output);
        if OutputDigest::from_hex(&result.output_hash) != Some(actual) {
            return Err(TaskResultError::OutputHashMismatch {
                claimed: result.output_hash,
                actual: actual.to_hex(),
            });
        }
        self.results.push(result);
        self.status = TaskStatus::Computing;
        Ok(())
    }

    pub(crate) fn valid_results(&self) -> Vec<&TaskResult> {
        let mut seen_workers = HashSet::new();
        self.results
            .iter()
            .filter(|result| {
                result.task_id == self.id
                    && self.assignments.iter().any(|assignment| {
                        assignment.task_id == self.id
                            && assignment.worker_id == result.worker_id
                    })
                    && OutputDigest::from_hex(&result.output_hash)
                        == Some(OutputDigest::of(&result.output))
                    && seen_workers.insert(result.worker_id)
            })
            .collect()
    }

    pub(crate) fn canonical_result(&self, verify: &VerifyResult) -> Option<&TaskResult> {
        let valid_results = self.valid_results();
        match verify {
            VerifyResult::NoVerificationNeeded | VerifyResult::CanaryPassed => {
                valid_results.first().copied()
            }
            VerifyResult::QuorumAgreed { agreed_hash, .. } => {
                let agreed = OutputDigest::from_hex(agreed_hash)?;
                valid_results
                    .into_iter()
                    .find(|result| OutputDigest::of(&result.output) == agreed)
            }
            _ => None,
        }
    }

    /// 검증 수행
    pub fn verify(&mut self) -> VerifyResult {
        let verify = {
            let valid_results = self.valid_results();
            match &self.task_type {
            TaskType::Embarrassing { .. } => {
                if !valid_results.is_empty() {
                    VerifyResult::NoVerificationNeeded
                } else {
                    VerifyResult::InsufficientResults
                }
            }
            TaskType::Quorum { replicas, .. } => {
                let replicas = *replicas as usize;
                let threshold = replicas / 2 + 1;
                if valid_results.len() < threshold {
                    return VerifyResult::InsufficientResults;
                }
                let mut hash_count: BTreeMap<OutputDigest, u32> = BTreeMap::new();
                for result in &valid_results {
                    *hash_count
                        .entry(OutputDigest::of(&result.output))
                        .or_default() += 1;
                }
                if let Some((hash, count)) = hash_count.iter().max_by_key(|(_, c)| *c) {
                    if *count as usize >= threshold {
                        VerifyResult::QuorumAgreed {
                            agreed_hash: hash.to_hex(),
                            agree_count: *count,
                        }
                    } else if valid_results.len() >= replicas {
                        VerifyResult::QuorumDisagreed {
                            hashes: hash_count.keys().map(|hash| hash.to_hex()).collect(),
                        }
                    } else {
                        VerifyResult::InsufficientResults
                    }
                } else {
                    VerifyResult::InsufficientResults
                }
            }
            TaskType::Canary { expected_hash, .. } => {
                if let Some(result) = valid_results.first() {
                    if OutputDigest::from_hex(expected_hash)
                        == Some(OutputDigest::of(&result.output))
                    {
                        VerifyResult::CanaryPassed
                    } else {
                        VerifyResult::CanaryFailed { worker_id: result.worker_id }
                    }
                } else {
                    VerifyResult::InsufficientResults
                }
            }
            }
        };

        match verify {
            VerifyResult::NoVerificationNeeded
            | VerifyResult::QuorumAgreed { .. }
            | VerifyResult::CanaryPassed => self.status = TaskStatus::Verified,
            VerifyResult::QuorumDisagreed { .. } | VerifyResult::CanaryFailed { .. } => {
                self.status = TaskStatus::Failed;
            }
            VerifyResult::InsufficientResults => {}
        }
        verify
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assign_worker(task: &mut Task, worker_id: u32) {
        assert!(task.record_assignment(TaskAssignment {
            task_id: task.id,
            worker_id,
            assigned_at: 0,
            timeout_ms: 30_000,
            reward: 1,
        }));
    }

    fn result(task_id: TaskId, worker_id: u32, output: &[u8]) -> TaskResult {
        TaskResult {
            task_id,
            worker_id,
            output: output.to_vec(),
            compute_ms: 10,
            output_hash: output_hash(output),
        }
    }

    #[test]
    fn embarrassing_no_verification() {
        let mut task = Task::new(1, TaskType::Embarrassing {
            description: "Monte Carlo".into(),
            input: vec![1, 2, 3],
        }, 0);
        assign_worker(&mut task, 10);
        task.submit_result(result(1, 10, &[42]));
        assert_eq!(task.verify(), VerifyResult::NoVerificationNeeded);
        assert_eq!(task.status, TaskStatus::Verified);
    }

    #[test]
    fn quorum_3_agree() {
        let mut task = Task::new(2, TaskType::Quorum {
            description: "hash search".into(), input: vec![], replicas: 3,
        }, 0);
        for w in [10, 20, 30] {
            assign_worker(&mut task, w);
            task.submit_result(result(2, w, &[1]));
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
        for worker_id in [10, 20, 30] {
            assign_worker(&mut task, worker_id);
        }
        task.submit_result(result(3, 10, b"good"));
        task.submit_result(result(3, 20, b"good"));
        task.submit_result(result(3, 30, b"bad"));
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
        for worker_id in [10, 20, 30] {
            assign_worker(&mut task, worker_id);
        }
        task.submit_result(result(4, 10, b"a"));
        task.submit_result(result(4, 20, b"b"));
        task.submit_result(result(4, 30, b"c"));
        match task.verify() {
            VerifyResult::QuorumDisagreed { .. } => {} // 3개 다 다르면 불일치
            other => panic!("expected QuorumDisagreed, got {:?}", other),
        }
    }

    #[test]
    fn canary_pass() {
        let expected = output_hash(b"correct");
        let mut task = Task::new(5, TaskType::Canary {
            description: "trap".into(), input: vec![], expected_hash: expected,
        }, 0);
        assign_worker(&mut task, 10);
        task.submit_result(result(5, 10, b"correct"));
        assert_eq!(task.verify(), VerifyResult::CanaryPassed);
    }

    #[test]
    fn canary_fail_catches_cheater() {
        let expected = output_hash(b"correct");
        let mut task = Task::new(6, TaskType::Canary {
            description: "trap".into(), input: vec![], expected_hash: expected,
        }, 0);
        assign_worker(&mut task, 99);
        task.submit_result(result(6, 99, b"wrong"));
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

    // KG: CONTRACT_333_Compute_Task T2 — unassigned results are rejected
    #[test]
    fn submit_result_without_assignment_is_rejected() {
        let mut task = Task::new(200, TaskType::Embarrassing {
            description: "no assign".into(), input: vec![],
        }, 0);
        task.submit_result(result(200, 5, &[1, 2, 3]));
        assert!(task.results.is_empty());
        assert_eq!(task.verify(), VerifyResult::InsufficientResults);
    }

    #[test]
    fn result_for_wrong_task_id_is_rejected() {
        let mut task = Task::new(201, TaskType::Embarrassing {
            description: "wrong task".into(), input: vec![],
        }, 0);
        assign_worker(&mut task, 5);

        task.submit_result(result(999, 5, &[1]));

        assert!(task.results.is_empty());
    }

    #[test]
    fn duplicate_worker_cannot_form_quorum() {
        let mut task = Task::new(202, TaskType::Quorum {
            description: "distinct workers".into(), input: vec![], replicas: 3,
        }, 0);
        for worker_id in [5, 6, 7] {
            assign_worker(&mut task, worker_id);
        }

        for _ in 0..3 {
            task.submit_result(result(202, 5, &[1]));
        }

        assert_eq!(task.results.len(), 1);
        assert_eq!(task.verify(), VerifyResult::InsufficientResults);
    }

    #[test]
    fn claimed_output_hash_must_match_output_bytes() {
        let mut task = Task::new(203, TaskType::Embarrassing {
            description: "hash binding".into(), input: vec![],
        }, 0);
        assign_worker(&mut task, 5);

        task.submit_result(TaskResult {
            task_id: 203, worker_id: 5,
            output: b"actual bytes".to_vec(), compute_ms: 10,
            output_hash: "forged".into(),
        });

        assert!(task.results.is_empty());
    }

    #[test]
    fn even_replica_split_is_not_a_quorum() {
        let mut task = Task::new(204, TaskType::Quorum {
            description: "strict majority".into(), input: vec![], replicas: 2,
        }, 0);
        assign_worker(&mut task, 5);
        assign_worker(&mut task, 6);
        task.submit_result(result(204, 5, &[1]));
        task.submit_result(result(204, 6, &[2]));

        assert!(matches!(task.verify(), VerifyResult::QuorumDisagreed { .. }));
    }

    #[test]
    fn terminal_task_rejects_late_result() {
        let mut task = Task::new(205, TaskType::Quorum {
            description: "terminal".into(), input: vec![], replicas: 3,
        }, 0);
        assign_worker(&mut task, 5);
        assign_worker(&mut task, 6);
        assign_worker(&mut task, 7);
        task.submit_result(result(205, 5, &[1]));
        task.submit_result(result(205, 6, &[1]));
        assert!(matches!(task.verify(), VerifyResult::QuorumAgreed { .. }));

        task.submit_result(result(205, 7, &[2]));

        assert_eq!(task.results.len(), 2);
        assert_eq!(task.status, TaskStatus::Verified);
    }

    #[test]
    fn verify_ignores_forged_duplicate_results() {
        let mut task = Task::new(206, TaskType::Quorum {
            description: "defensive verify".into(), input: vec![], replicas: 3,
        }, 0);
        for worker_id in [5, 6, 7] {
            assign_worker(&mut task, worker_id);
        }
        for _ in 0..3 {
            task.results.push(result(206, 5, &[1]));
        }

        assert_eq!(task.verify(), VerifyResult::InsufficientResults);
    }

    // KG: CONTRACT_333_Compute_Task T4 — verify on already-Verified task returns same result
    #[test]
    fn verify_completed_task_idempotent() {
        let mut task = Task::new(300, TaskType::Embarrassing {
            description: "done".into(), input: vec![],
        }, 0);
        assign_worker(&mut task, 1);
        task.submit_result(result(300, 1, &[42]));
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
