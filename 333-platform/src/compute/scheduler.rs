// KG: CONTRACT_333_Compute_Scheduler, SPAN_333_Compute_Scheduler
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-compute-sched-ReputationTier/WorkerInfo/TaskScheduler/SchedulerStats
// P2P task distribution — no central server
// Uses CRDT for task queue + BFT for reward settlement

use super::task::*;
use std::collections::{HashMap, VecDeque};

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
        let total = self.tasks_completed + self.tasks_failed;
        if total == 0 { return 0; }
        // Scale to 1000 (1000 = 100%, 500 = 50%, etc.)
        self.tasks_failed * 1000 / total
    }
}

/// Distributed task scheduler
pub struct TaskScheduler {
    next_task_id: TaskId,              // KG: ST_TaskId
    pending: VecDeque<TaskId>,          // KG: ST_TaskId
    tasks: HashMap<TaskId, Task>,       // KG: ST_TaskId
    workers: HashMap<u32, WorkerInfo>,
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
            canary_ratio: 0.1,
        }
    }

    /// 워커 등록
    pub fn register_worker(&mut self, node_id: u32) {
        self.workers.entry(node_id).or_insert_with(|| WorkerInfo::new(node_id));
    }

    /// 태스크 제출
    pub fn submit_task(&mut self, task_type: TaskType, now_ms: u64) -> TaskId {
        let id = self.next_task_id;
        self.next_task_id += 1;
        let task = Task::new(id, task_type, now_ms);
        self.tasks.insert(id, task);
        self.pending.push_back(id);
        id
    }

    /// 태스크 배정 (가용 워커에게)
    pub fn assign_next(&mut self, now_ms: u64) -> Option<TaskAssignment> {
        let task_id = self.pending.pop_front()?;

        // 가용 워커 찾기 (실패율 낮은 순)
        let worker_id = self.workers.values()
            .filter(|w| w.available)
            .min_by_key(|w| w.failure_rate_milli())
            .map(|w| w.node_id)?;

        let assignment = TaskAssignment {
            task_id,
            worker_id,
            assigned_at: now_ms,
            timeout_ms: 30_000,
            reward: 1, // 기본 1 토큰
        };

        if let Some(task) = self.tasks.get_mut(&task_id) {
            task.status = TaskStatus::Assigned;
            task.assignments.push(assignment.clone());
        }

        if let Some(worker) = self.workers.get_mut(&worker_id) {
            worker.available = false;
        }

        Some(assignment)
    }

    /// 결과 제출 + 검증
    pub fn submit_result(&mut self, result: TaskResult) -> Option<VerifyResult> {
        let task = self.tasks.get_mut(&result.task_id)?;
        let worker_id = result.worker_id;
        task.submit_result(result);

        let verify = task.verify();

        // 워커 상태 업데이트
        if let Some(worker) = self.workers.get_mut(&worker_id) {
            worker.available = true;
            match &verify {
                VerifyResult::NoVerificationNeeded |
                VerifyResult::QuorumAgreed { .. } |
                VerifyResult::CanaryPassed => {
                    worker.tasks_completed += 1;
                    worker.tokens_earned += 1;
                    worker.update_reputation();
                }
                VerifyResult::QuorumDisagreed { .. } |
                VerifyResult::CanaryFailed { .. } => {
                    worker.tasks_failed += 1;
                    worker.update_reputation();
                }
                _ => {}
            }
        }

        Some(verify)
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

        let verify = sched.submit_result(TaskResult {
            task_id: tid, worker_id: 1, output: b"3.14".to_vec(),
            compute_ms: 100, output_hash: "abc".into(),
        });
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
            expected_hash: "correct_answer".into(),
        }, 0);
        sched.assign_next(0);

        let verify = sched.submit_result(TaskResult {
            task_id: tid, worker_id: 99, output: vec![],
            compute_ms: 1, output_hash: "wrong_answer".into(),
        });
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
    }

    #[test]
    fn no_workers_no_assignment() {
        let mut sched = TaskScheduler::new();
        sched.submit_task(TaskType::Embarrassing {
            description: "lonely".into(), input: vec![],
        }, 0);
        assert!(sched.assign_next(0).is_none());
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
        sched.submit_result(TaskResult {
            task_id: t1, worker_id: 1,
            output: vec![1], compute_ms: 10,
            output_hash: "h".into(),
        });
        assert!(sched.worker_info(1).unwrap().available);

        // Submit new task — worker 1 can be assigned again
        let t2 = sched.submit_task(TaskType::Embarrassing {
            description: "second".into(), input: vec![],
        }, 100);
        let a2 = sched.assign_next(100).unwrap();
        assert_eq!(a2.task_id, t2);
        assert_eq!(a2.worker_id, 1);
    }
}
