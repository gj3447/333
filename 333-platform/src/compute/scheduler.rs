// KG: CONTRACT_333_Compute_Scheduler, SPAN_333_Compute_Scheduler
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-compute-sched-ReputationTier/WorkerInfo/TaskScheduler/SchedulerStats
// P2P task distribution — no central server
// Uses CRDT for task queue + BFT for reward settlement

use super::task::*;
use std::collections::{HashMap, HashSet, VecDeque};

/// Worker reputation tier (Prometheus research: anti-sybil)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReputationTier {
    /// 신규 — 100% 검증, 낮은 보상
    New = 0,
    /// 100회 성공 — 50% 검증
    Bronze = 1,
    /// 500회 성공 — 20% 검증
    Silver = 2,
    /// 1000회 성공 — Canary만 (10%)
    Gold = 3,
}

/// Worker info
#[derive(Debug, Clone)]
pub struct WorkerInfo {
    pub node_id: u32,       // KG: ST_NodeId
    pub reputation: ReputationTier,
    pub tasks_completed: u64,
    pub tasks_failed: u64,
    pub tokens_earned: u64,
    pub available: bool,
    current_task: Option<TaskId>,
}

impl WorkerInfo {
    pub fn new(node_id: u32) -> Self {
        Self {
            node_id,
            reputation: ReputationTier::New,
            tasks_completed: 0,
            tasks_failed: 0,
            tokens_earned: 0,
            available: true,
            current_task: None,
        }
    }

    /// 평판 업그레이드 체크
    pub fn update_reputation(&mut self) {
        self.reputation = match self.tasks_completed {
            0..=99 => ReputationTier::New,
            100..=499 => ReputationTier::Bronze,
            500..=999 => ReputationTier::Silver,
            _ => ReputationTier::Gold,
        };
    }

    /// 실패율 (scaled ×1000 to avoid f64 in WASM) # KG: sprint6B-wasm-size-opt-2026-04-15
    pub fn failure_rate_milli(&self) -> u64 {
        let total = self.tasks_completed.saturating_add(self.tasks_failed);
        if total == 0 { return 0; }
        // Scale to 1000 (1000 = 100%, 500 = 50%, etc.)
        self.tasks_failed.saturating_mul(1000) / total
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAdmissionError {
    ZeroReplicas,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitResultError {
    UnknownTask { task_id: TaskId },
    WorkerIdentityMismatch { authenticated: u32, claimed: u32 },
    Rejected(TaskResultError),
}

/// Distributed task scheduler
pub struct TaskScheduler {
    next_task_id: TaskId,              // KG: ST_TaskId
    pending: VecDeque<TaskId>,          // KG: ST_TaskId
    tasks: HashMap<TaskId, Task>,       // KG: ST_TaskId
    workers: HashMap<u32, WorkerInfo>,
    settled_tasks: HashSet<TaskId>,
    #[allow(dead_code)] // canary task ratio, used when canary scheduling is implemented
    canary_ratio: f64, // Canary 태스크 비율 (기본 0.1 = 10%)
}

impl Default for TaskScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskScheduler {
    pub fn new() -> Self {
        Self {
            next_task_id: 1,
            pending: VecDeque::new(),
            tasks: HashMap::new(),
            workers: HashMap::new(),
            settled_tasks: HashSet::new(),
            canary_ratio: 0.1,
        }
    }

    /// 워커 등록
    pub fn register_worker(&mut self, node_id: u32) {
        self.workers.entry(node_id).or_insert_with(|| WorkerInfo::new(node_id));
    }

    /// 태스크 제출
    pub fn submit_task(&mut self, task_type: TaskType, now_ms: u64) -> TaskId {
        self.try_submit_task(task_type, now_ms)
            .expect("task configuration must be valid")
    }

    pub fn try_submit_task(
        &mut self,
        task_type: TaskType,
        now_ms: u64,
    ) -> Result<TaskId, TaskAdmissionError> {
        if matches!(&task_type, TaskType::Quorum { replicas: 0, .. }) {
            return Err(TaskAdmissionError::ZeroReplicas);
        }
        let id = self.next_task_id;
        self.next_task_id += 1;
        let task = Task::new(id, task_type, now_ms);
        self.tasks.insert(id, task);
        self.pending.push_back(id);
        Ok(id)
    }

    /// 태스크 배정 (가용 워커에게)
    pub fn assign_next(&mut self, now_ms: u64) -> Option<TaskAssignment> {
        self.assign_next_matching(now_ms, |_, _| true)
    }

    pub(crate) fn assign_next_matching<F>(
        &mut self,
        now_ms: u64,
        eligible: F,
    ) -> Option<TaskAssignment>
    where
        F: Fn(TaskId, u32) -> bool,
    {
        let scan_limit = self.pending.len();
        for _ in 0..scan_limit {
            let task_id = self.pending.pop_front()?;
            let Some(task) = self.tasks.get(&task_id) else {
                continue;
            };

            let worker_id = self
                .workers
                .values()
                .filter(|worker| {
                    worker.available
                        && worker.current_task.is_none()
                        && eligible(task_id, worker.node_id)
                        && task.can_assign(worker.node_id)
                })
                .min_by_key(|worker| (worker.failure_rate_milli(), worker.node_id))
                .map(|worker| worker.node_id);

            let Some(worker_id) = worker_id else {
                if task.assignments.len() < task.required_assignments()
                    && matches!(
                        task.status,
                        TaskStatus::Pending | TaskStatus::Assigned | TaskStatus::Computing
                    )
                {
                    self.pending.push_back(task_id);
                }
                continue;
            };

            let assignment = TaskAssignment {
                task_id,
                worker_id,
                assigned_at: now_ms,
                timeout_ms: 30_000,
                reward: 1,
            };

            let task = self.tasks.get_mut(&task_id)?;
            if !task.record_assignment(assignment.clone()) {
                continue;
            }
            if task.assignments.len() < task.required_assignments() {
                self.pending.push_back(task_id);
            }
            if let Some(worker) = self.workers.get_mut(&worker_id) {
                worker.available = false;
                worker.current_task = Some(task_id);
            }
            return Some(assignment);
        }
        None
    }

    /// 결과 제출 + 검증
    pub fn submit_result(&mut self, result: TaskResult) -> Option<VerifyResult> {
        let authenticated_worker_id = result.worker_id;
        self.submit_result_outcome(authenticated_worker_id, result)
            .ok()
            .map(|outcome| outcome.verify)
    }

    pub fn submit_result_from(
        &mut self,
        authenticated_worker_id: u32,
        result: TaskResult,
    ) -> Result<VerifyResult, SubmitResultError> {
        self.submit_result_outcome(authenticated_worker_id, result)
            .map(|outcome| outcome.verify)
    }

    pub(crate) fn submit_result_outcome(
        &mut self,
        authenticated_worker_id: u32,
        result: TaskResult,
    ) -> Result<AcceptedResult, SubmitResultError> {
        let task_id = result.task_id;
        let worker_id = result.worker_id;
        let compute_ms = result.compute_ms;
        if worker_id != authenticated_worker_id {
            return Err(SubmitResultError::WorkerIdentityMismatch {
                authenticated: authenticated_worker_id,
                claimed: worker_id,
            });
        }

        let (verify, newly_finalized, canonical_output, settlements) = {
            let task = self
                .tasks
                .get_mut(&task_id)
                .ok_or(SubmitResultError::UnknownTask { task_id })?;
            let was_terminal = matches!(task.status, TaskStatus::Verified | TaskStatus::Failed);
            task.try_submit_result(result)
                .map_err(SubmitResultError::Rejected)?;
            let verify = if matches!(task.task_type, TaskType::Quorum { .. })
                && task.valid_results().len() < task.required_assignments()
            {
                VerifyResult::InsufficientResults
            } else {
                task.verify()
            };
            let is_terminal = matches!(task.status, TaskStatus::Verified | TaskStatus::Failed);
            let newly_finalized = !was_terminal && is_terminal;
            let canonical_output = if newly_finalized {
                task.canonical_result(&verify)
                    .map(|result| result.output.clone())
            } else {
                None
            };
            let settlements = if newly_finalized {
                Self::settlements_for(task, &verify)
            } else {
                Vec::new()
            };
            (
                verify,
                newly_finalized,
                canonical_output,
                settlements,
            )
        };

        if let Some(worker) = self.workers.get_mut(&authenticated_worker_id) {
            if worker.current_task == Some(task_id) {
                worker.available = true;
                worker.current_task = None;
            }
        }
        if newly_finalized {
            self.pending.retain(|queued_task| *queued_task != task_id);
        }

        let mut reward_delta = 0;
        if newly_finalized && !self.settled_tasks.contains(&task_id) {
            for settlement in settlements {
                if let Some(worker) = self.workers.get_mut(&settlement.worker_id) {
                    if settlement.succeeded {
                        worker.tasks_completed = worker.tasks_completed.saturating_add(1);
                        let previous_tokens = worker.tokens_earned;
                        worker.tokens_earned = worker.tokens_earned.saturating_add(settlement.reward);
                        reward_delta = reward_delta
                            .saturating_add(worker.tokens_earned.saturating_sub(previous_tokens));
                    } else {
                        worker.tasks_failed = worker.tasks_failed.saturating_add(1);
                    }
                    worker.update_reputation();
                }
            }
            self.settled_tasks.insert(task_id);
        }

        Ok(AcceptedResult {
            task_id,
            compute_ms,
            verify,
            newly_finalized,
            canonical_output,
            reward_delta,
        })
    }

    fn settlements_for(task: &Task, verify: &VerifyResult) -> Vec<WorkerSettlement> {
        let valid_results = task.valid_results();
        match verify {
            VerifyResult::NoVerificationNeeded | VerifyResult::CanaryPassed => valid_results
                .first()
                .map(|result| {
                    vec![WorkerSettlement {
                        worker_id: result.worker_id,
                        succeeded: true,
                        reward: Self::assignment_reward(task, result.worker_id),
                    }]
                })
                .unwrap_or_default(),
            VerifyResult::CanaryFailed { worker_id } => vec![WorkerSettlement {
                worker_id: *worker_id,
                succeeded: false,
                reward: 0,
            }],
            VerifyResult::QuorumAgreed { agreed_hash, .. } => valid_results
                .into_iter()
                .map(|result| {
                    let succeeded = result.output_hash == *agreed_hash;
                    WorkerSettlement {
                        worker_id: result.worker_id,
                        succeeded,
                        reward: if succeeded {
                            Self::assignment_reward(task, result.worker_id)
                        } else {
                            0
                        },
                    }
                })
                .collect(),
            VerifyResult::QuorumDisagreed { .. } | VerifyResult::InsufficientResults => Vec::new(),
        }
    }

    fn assignment_reward(task: &Task, worker_id: u32) -> u64 {
        task.assignments
            .iter()
            .find(|assignment| assignment.worker_id == worker_id)
            .map(|assignment| assignment.reward)
            .unwrap_or(0)
    }

    /// 통계
    pub fn stats(&self) -> SchedulerStats {
        SchedulerStats {
            total_tasks: self.tasks.len(),
            pending: self.pending.len(),
            completed: self.tasks.values().filter(|t| t.status == TaskStatus::Verified).count(),
            failed: self.tasks.values().filter(|t| t.status == TaskStatus::Failed).count(),
            workers: self.workers.len(),
            available_workers: self.workers.values().filter(|w| w.available).count(),
        }
    }

    /// 워커 정보
    pub fn worker_info(&self, node_id: u32) -> Option<&WorkerInfo> {
        self.workers.get(&node_id)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AcceptedResult {
    pub task_id: TaskId,
    pub compute_ms: u64,
    pub verify: VerifyResult,
    pub newly_finalized: bool,
    pub canonical_output: Option<Vec<u8>>,
    pub reward_delta: u64,
}

struct WorkerSettlement {
    worker_id: u32,
    succeeded: bool,
    reward: u64,
}

#[derive(Debug, Clone)]
pub struct SchedulerStats {
    pub total_tasks: usize,
    pub pending: usize,
    pub completed: usize,
    pub failed: usize,
    pub workers: usize,
    pub available_workers: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn valid_result(task_id: TaskId, worker_id: u32, output: &[u8]) -> TaskResult {
        TaskResult {
            task_id,
            worker_id,
            output: output.to_vec(),
            compute_ms: 10,
            output_hash: output_hash(output),
        }
    }

    #[test]
    fn register_and_assign() {
        let mut sched = TaskScheduler::new();
        sched.register_worker(1);
        sched.register_worker(2);

        let tid = sched.submit_task(TaskType::Embarrassing {
            description: "test".into(), input: vec![],
        }, 0);

        let assignment = sched.assign_next(100).unwrap();
        assert_eq!(assignment.task_id, tid);
        assert!(assignment.worker_id == 1 || assignment.worker_id == 2);
    }

    #[test]
    fn submit_result_completes() {
        let mut sched = TaskScheduler::new();
        sched.register_worker(1);
        let tid = sched.submit_task(TaskType::Embarrassing {
            description: "pi".into(), input: vec![],
        }, 0);
        sched.assign_next(0);

        let assignment = sched.tasks[&tid].assignments[0].clone();
        let verify = sched.submit_result(valid_result(tid, assignment.worker_id, b"3.14"));
        assert_eq!(verify, Some(VerifyResult::NoVerificationNeeded));

        let stats = sched.stats();
        assert_eq!(stats.completed, 1);
    }

    #[test]
    fn reputation_upgrades() {
        let mut worker = WorkerInfo::new(1);
        assert_eq!(worker.reputation, ReputationTier::New);

        worker.tasks_completed = 100;
        worker.update_reputation();
        assert_eq!(worker.reputation, ReputationTier::Bronze);

        worker.tasks_completed = 500;
        worker.update_reputation();
        assert_eq!(worker.reputation, ReputationTier::Silver);

        worker.tasks_completed = 1000;
        worker.update_reputation();
        assert_eq!(worker.reputation, ReputationTier::Gold);
    }

    #[test]
    fn canary_catches_cheater() {
        let mut sched = TaskScheduler::new();
        sched.register_worker(99);
        let tid = sched.submit_task(TaskType::Canary {
            description: "trap".into(), input: vec![],
            expected_hash: output_hash(b"correct_answer"),
        }, 0);
        sched.assign_next(0);

        let verify = sched.submit_result(valid_result(tid, 99, b"wrong_answer"));
        assert_eq!(verify, Some(VerifyResult::CanaryFailed { worker_id: 99 }));

        // 워커 실패 기록
        let worker = sched.worker_info(99).unwrap();
        assert_eq!(worker.tasks_failed, 1);
    }

    #[test]
    fn multiple_tasks_queue() {
        let mut sched = TaskScheduler::new();
        sched.register_worker(1);
        sched.register_worker(2);

        for i in 0..5 {
            sched.submit_task(TaskType::Embarrassing {
                description: format!("task_{}", i), input: vec![i as u8],
            }, 0);
        }

        let stats = sched.stats();
        assert_eq!(stats.total_tasks, 5);
        assert_eq!(stats.pending, 5);

        // 2 워커 → 2개 배정 가능
        assert!(sched.assign_next(0).is_some());
        assert!(sched.assign_next(0).is_some());
        // 3번째는 워커 없음 (2명 다 busy)
        assert!(sched.assign_next(0).is_none());
        assert_eq!(sched.stats().pending, 3);
    }

    #[test]
    fn no_workers_no_assignment() {
        let mut sched = TaskScheduler::new();
        let task_id = sched.submit_task(TaskType::Embarrassing {
            description: "lonely".into(), input: vec![],
        }, 0);
        assert!(sched.assign_next(0).is_none());
        assert_eq!(sched.stats().pending, 1);

        sched.register_worker(7);
        let assignment = sched.assign_next(1).unwrap();
        assert_eq!(assignment.task_id, task_id);
        assert_eq!(assignment.worker_id, 7);
    }

    // KG: CONTRACT_333_Compute_Scheduler T1 — worker with lower failure_rate gets assigned first
    #[test]
    fn best_worker_assigned_first() {
        let mut sched = TaskScheduler::new();
        sched.register_worker(1);
        sched.register_worker(2);
        // Give worker 1 a bad failure rate
        if let Some(w) = sched.workers.get_mut(&1) {
            w.tasks_completed = 5;
            w.tasks_failed = 5; // 50% failure
        }
        if let Some(w) = sched.workers.get_mut(&2) {
            w.tasks_completed = 10;
            w.tasks_failed = 0; // 0% failure
        }
        sched.submit_task(TaskType::Embarrassing {
            description: "pick best".into(), input: vec![],
        }, 0);
        let assignment = sched.assign_next(0).unwrap();
        assert_eq!(assignment.worker_id, 2); // lower failure rate wins
    }

    // KG: CONTRACT_333_Compute_Scheduler T2 — assign on empty queue returns None
    #[test]
    fn empty_queue_returns_none() {
        let mut sched = TaskScheduler::new();
        sched.register_worker(1);
        assert!(sched.assign_next(0).is_none());
    }

    // KG: CONTRACT_333_Compute_Scheduler T6 — worker becomes available after completing a task
    #[test]
    fn worker_available_after_result() {
        let mut sched = TaskScheduler::new();
        sched.register_worker(1);
        let t1 = sched.submit_task(TaskType::Embarrassing {
            description: "first".into(), input: vec![],
        }, 0);

        // Assign task — worker 1 becomes busy
        let a1 = sched.assign_next(0).unwrap();
        assert_eq!(a1.task_id, t1);
        assert!(!sched.worker_info(1).unwrap().available);

        // Complete task — worker 1 becomes available again
        sched.submit_result(valid_result(t1, 1, &[1]));
        assert!(sched.worker_info(1).unwrap().available);

        // Submit new task — worker 1 can be assigned again
        let t2 = sched.submit_task(TaskType::Embarrassing {
            description: "second".into(), input: vec![],
        }, 100);
        let a2 = sched.assign_next(100).unwrap();
        assert_eq!(a2.task_id, t2);
        assert_eq!(a2.worker_id, 1);
    }

    #[test]
    fn quorum_assignments_use_distinct_workers() {
        let mut sched = TaskScheduler::new();
        for worker_id in [1, 2, 3] {
            sched.register_worker(worker_id);
        }
        let task_id = sched.submit_task(TaskType::Quorum {
            description: "replicated".into(), input: vec![], replicas: 3,
        }, 0);

        let assignments: Vec<_> = (0..3)
            .filter_map(|_| sched.assign_next(0))
            .collect();
        let workers: HashSet<_> = assignments.iter().map(|a| a.worker_id).collect();

        assert_eq!(assignments.len(), 3);
        assert!(assignments.iter().all(|a| a.task_id == task_id));
        assert_eq!(workers.len(), 3);
        assert_eq!(sched.stats().pending, 0);
    }

    #[test]
    fn wrong_worker_result_is_rejected_without_accounting() {
        let mut sched = TaskScheduler::new();
        sched.register_worker(1);
        sched.register_worker(2);
        let task_id = sched.submit_task(TaskType::Embarrassing {
            description: "assignment bound".into(), input: vec![],
        }, 0);
        let assignment = sched.assign_next(0).unwrap();
        let wrong_worker = if assignment.worker_id == 1 { 2 } else { 1 };

        let verify = sched.submit_result(valid_result(task_id, wrong_worker, b"forged"));

        assert!(verify.is_none());
        assert_eq!(sched.worker_info(wrong_worker).unwrap().tasks_completed, 0);
        assert_eq!(sched.worker_info(wrong_worker).unwrap().tokens_earned, 0);
    }

    #[test]
    fn duplicate_result_does_not_double_reward() {
        let mut sched = TaskScheduler::new();
        sched.register_worker(1);
        let task_id = sched.submit_task(TaskType::Embarrassing {
            description: "exactly once".into(), input: vec![],
        }, 0);
        let assignment = sched.assign_next(0).unwrap();
        let result = valid_result(task_id, assignment.worker_id, b"result");

        assert_eq!(
            sched.submit_result(result.clone()),
            Some(VerifyResult::NoVerificationNeeded)
        );
        assert!(sched.submit_result(result).is_none());

        let worker = sched.worker_info(assignment.worker_id).unwrap();
        assert_eq!(worker.tasks_completed, 1);
        assert_eq!(worker.tokens_earned, 1);
    }

    #[test]
    fn zero_replica_quorum_is_rejected_before_queueing() {
        let mut sched = TaskScheduler::new();
        assert_eq!(
            sched.try_submit_task(
                TaskType::Quorum {
                    description: "invalid".into(),
                    input: vec![],
                    replicas: 0,
                },
                0,
            ),
            Err(TaskAdmissionError::ZeroReplicas)
        );
        assert_eq!(sched.stats().total_tasks, 0);
        assert_eq!(sched.stats().pending, 0);
    }

    #[test]
    fn authenticated_worker_cannot_claim_another_assignment() {
        let mut sched = TaskScheduler::new();
        sched.register_worker(1);
        sched.register_worker(2);
        let task_id = sched.submit_task(
            TaskType::Embarrassing {
                description: "identity bound".into(),
                input: vec![],
            },
            0,
        );
        let assignment = sched.assign_next(0).unwrap();
        let other = if assignment.worker_id == 1 { 2 } else { 1 };

        assert_eq!(
            sched.submit_result_from(
                other,
                valid_result(task_id, assignment.worker_id, b"spoof"),
            ),
            Err(SubmitResultError::WorkerIdentityMismatch {
                authenticated: other,
                claimed: assignment.worker_id,
            })
        );
        assert_eq!(sched.stats().completed, 0);
        assert!(!sched.worker_info(assignment.worker_id).unwrap().available);
    }

    #[test]
    fn quorum_waits_for_every_replica_before_majority_settlement() {
        let mut sched = TaskScheduler::new();
        for worker_id in [1, 2, 3] {
            sched.register_worker(worker_id);
        }
        let task_id = sched.submit_task(TaskType::Quorum {
            description: "wait for the full replica set".into(),
            input: vec![],
            replicas: 3,
        }, 0);
        let assignments: Vec<_> = (0..3)
            .map(|_| sched.assign_next(0).unwrap())
            .collect();

        assert_eq!(
            sched.submit_result(valid_result(
                task_id,
                assignments[0].worker_id,
                b"majority",
            )),
            Some(VerifyResult::InsufficientResults)
        );
        assert_eq!(
            sched.submit_result(valid_result(
                task_id,
                assignments[1].worker_id,
                b"majority",
            )),
            Some(VerifyResult::InsufficientResults)
        );
        assert_eq!(sched.stats().completed, 0);

        assert!(matches!(
            sched.submit_result(valid_result(
                task_id,
                assignments[2].worker_id,
                b"minority",
            )),
            Some(VerifyResult::QuorumAgreed { agree_count: 2, .. })
        ));
        assert_eq!(sched.stats().completed, 1);
        assert_eq!(
            assignments
                .iter()
                .map(|assignment| sched.worker_info(assignment.worker_id).unwrap().tokens_earned)
                .sum::<u64>(),
            2
        );
        assert_eq!(
            assignments
                .iter()
                .map(|assignment| sched.worker_info(assignment.worker_id).unwrap().tasks_failed)
                .sum::<u64>(),
            1
        );
    }

    #[test]
    fn finishing_old_quorum_does_not_release_worker_owned_by_new_task() {
        let mut sched = TaskScheduler::new();
        sched.register_worker(1);
        sched.register_worker(2);
        let quorum_id = sched.submit_task(TaskType::Quorum {
            description: "old quorum".into(),
            input: vec![],
            replicas: 2,
        }, 0);
        let first = sched.assign_next(0).unwrap();
        let second = sched.assign_next(0).unwrap();

        assert_eq!(
            sched.submit_result(valid_result(quorum_id, first.worker_id, b"same")),
            Some(VerifyResult::InsufficientResults)
        );
        let next_id = sched.submit_task(TaskType::Embarrassing {
            description: "new task".into(),
            input: vec![],
        }, 1);
        let next = sched.assign_next(1).unwrap();
        assert_eq!(next.task_id, next_id);
        assert_eq!(next.worker_id, first.worker_id);

        assert!(matches!(
            sched.submit_result(valid_result(quorum_id, second.worker_id, b"same")),
            Some(VerifyResult::QuorumAgreed { .. })
        ));
        assert!(!sched.worker_info(first.worker_id).unwrap().available);
        assert!(sched.worker_info(second.worker_id).unwrap().available);
    }

    #[test]
    fn reward_accounting_saturates_without_partial_settlement() {
        let mut sched = TaskScheduler::new();
        sched.register_worker(1);
        let task_id = sched.submit_task(TaskType::Embarrassing {
            description: "counter boundary".into(),
            input: vec![],
        }, 0);
        sched.assign_next(0).unwrap();
        let worker = sched.workers.get_mut(&1).unwrap();
        worker.tasks_completed = u64::MAX;
        worker.tokens_earned = u64::MAX;

        assert_eq!(
            sched.submit_result(valid_result(task_id, 1, b"done")),
            Some(VerifyResult::NoVerificationNeeded)
        );
        let worker = sched.worker_info(1).unwrap();
        assert_eq!(worker.tasks_completed, u64::MAX);
        assert_eq!(worker.tokens_earned, u64::MAX);
    }
}
