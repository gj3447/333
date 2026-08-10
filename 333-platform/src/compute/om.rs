// KG: CONTRACT_333_Compute_OM, SPAN_333_Compute_OM
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-compute-om-NodeResources/OmTaskKind/OmJob/OmJobStatus/OmOrchestrator/OmStats
// Operations Manager: P2P task distribution layer on top of TaskScheduler
// Manages worker pool, resource awareness, task broadcasting

use super::task::*;
use super::scheduler::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Resource snapshot from a browser node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeResources {
    pub node_id: u32,       // KG: ST_NodeId
    pub cpu_cores: u32,
    pub memory_mb: u32,
    pub gpu_available: bool,
    pub battery_pct: Option<u8>,   // None = plugged in
    pub active_workers: u32,
    pub max_workers: u32,
}

impl NodeResources {
    pub fn can_accept_work(&self) -> bool {
        self.active_workers < self.max_workers
            && self.battery_pct.is_none_or(|b| b > 20)
    }

    // KG: sprint6B-wasm-size-opt-2026-04-15 — f64→u32 eliminates flt2dec from WASM
    pub fn compute_capacity(&self) -> u32 {
        let base = self.max_workers.saturating_sub(self.active_workers);
        let gpu_bonus: u32 = if self.gpu_available { 4 } else { 0 };
        base + gpu_bonus
    }
}

/// OM task categories — what kind of compute
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OmTaskKind {
    /// CPU-bound: hash search, Monte Carlo, matrix ops
    CpuIntensive { estimated_ms: u64 },
    /// GPU-bound: AI inference, image processing
    GpuCompute { model_size_mb: u32 },
    /// I/O-bound: data aggregation, text analysis
    DataPipeline { input_size_kb: u32 },
    /// Map-Reduce: split into N subtasks, aggregate
    MapReduce { chunk_count: u32, reduce_fn: String },
}

/// An OM job = collection of tasks with orchestration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmJob {
    pub job_id: String,
    pub kind: OmTaskKind,
    pub submitter: u32,
    pub reward_per_task: u64,
    pub task_ids: Vec<TaskId>,    // KG: ST_TaskId
    pub status: OmJobStatus,
    pub created_at: u64,
    pub completed_at: Option<u64>,
    pub results: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OmJobStatus {
    Submitted,
    Distributing,
    Computing,
    Reducing,
    Completed,
    Failed,
}

/// OM Orchestrator — manages distributed compute across P2P network
pub struct OmOrchestrator {
    pub(crate) scheduler: TaskScheduler,
    nodes: HashMap<u32, NodeResources>,
    jobs: HashMap<String, OmJob>,
    verified_outputs: HashMap<TaskId, Vec<u8>>,
    next_job_num: u64,
    total_compute_ms: u64,
    total_tokens_distributed: u64,
}

impl Default for OmOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl OmOrchestrator {
    pub fn new() -> Self {
        Self {
            scheduler: TaskScheduler::new(),
            nodes: HashMap::new(),
            jobs: HashMap::new(),
            verified_outputs: HashMap::new(),
            next_job_num: 1,
            total_compute_ms: 0,
            total_tokens_distributed: 0,
        }
    }

    /// Register a browser node with its resources
    pub fn register_node(&mut self, resources: NodeResources) {
        let id = resources.node_id;
        self.nodes.insert(id, resources);
        self.scheduler.register_worker(id);
    }

    /// Update node resources (periodic heartbeat)
    pub fn update_resources(&mut self, resources: NodeResources) {
        self.nodes.insert(resources.node_id, resources);
    }

    /// Submit an OM job — breaks into tasks and distributes
    pub fn submit_job(&mut self, kind: OmTaskKind, submitter: u32, now_ms: u64) -> String {
        let job_id = format!("om-{}", self.next_job_num);
        self.next_job_num += 1;

        let mut task_ids = Vec::new();

        match &kind {
            OmTaskKind::MapReduce { chunk_count, reduce_fn } => {
                // Create N sub-tasks
                for i in 0..*chunk_count {
                    let input = serde_json::to_vec(&serde_json::json!({
                        "chunk": i,
                        "total": chunk_count,
                        "reduce_fn": reduce_fn,
                    })).unwrap_or_default();

                    let tid = self.scheduler.submit_task(
                        TaskType::Quorum {
                            description: format!("{} chunk {}/{}", job_id, i, chunk_count),
                            input,
                            replicas: 3,
                        },
                        now_ms,
                    );
                    task_ids.push(tid);
                }
            }
            OmTaskKind::GpuCompute { .. } => {
                // GPU tasks go to nodes with GPU
                let tid = self.scheduler.submit_task(
                    TaskType::Quorum {
                        description: format!("{} gpu-compute", job_id),
                        input: serde_json::to_vec(&kind).unwrap_or_default(),
                        replicas: 2, // GPU tasks: 2-way quorum (expensive)
                    },
                    now_ms,
                );
                task_ids.push(tid);
            }
            _ => {
                // Single task
                let tid = self.scheduler.submit_task(
                    TaskType::Embarrassing {
                        description: format!("{} compute", job_id),
                        input: serde_json::to_vec(&kind).unwrap_or_default(),
                    },
                    now_ms,
                );
                task_ids.push(tid);
            }
        }

        let job = OmJob {
            job_id: job_id.clone(),
            kind,
            submitter,
            reward_per_task: 1,
            task_ids: task_ids.clone(),
            status: OmJobStatus::Submitted,
            created_at: now_ms,
            completed_at: None,
            results: Vec::new(),
        };
        self.jobs.insert(job_id.clone(), job);
        job_id
    }

    /// Distribute pending tasks to available nodes
    pub fn distribute(&mut self, now_ms: u64) -> Vec<TaskAssignment> {
        let mut assignments = Vec::new();
        while let Some(assignment) = self.scheduler.assign_next(now_ms) {
            assignments.push(assignment);
        }
        // Update job status
        for job in self.jobs.values_mut() {
            if job.status == OmJobStatus::Submitted
                && job.task_ids.iter().any(|task_id| {
                    assignments
                        .iter()
                        .any(|assignment| assignment.task_id == *task_id)
                })
            {
                job.status = OmJobStatus::Distributing;
            }
        }
        assignments
    }

    /// Submit task result and check job completion
    pub fn submit_result(&mut self, result: TaskResult, now_ms: u64) -> Option<VerifyResult> {
        let outcome = self.scheduler.submit_result_outcome(result)?;
        self.total_compute_ms = self.total_compute_ms.saturating_add(outcome.compute_ms);
        self.total_tokens_distributed = self
            .total_tokens_distributed
            .saturating_add(outcome.reward_delta);

        let terminal_success = outcome.newly_finalized
            && matches!(
                &outcome.verify,
                VerifyResult::NoVerificationNeeded
                    | VerifyResult::QuorumAgreed { .. }
                    | VerifyResult::CanaryPassed
            );
        let terminal_failure = outcome.newly_finalized
            && matches!(
                &outcome.verify,
                VerifyResult::QuorumDisagreed { .. } | VerifyResult::CanaryFailed { .. }
            );
        let recorded_output = if terminal_success {
            outcome.canonical_output.as_ref().is_some_and(|output| {
                self.verified_outputs
                    .insert(outcome.task_id, output.clone())
                    .is_none()
            })
        } else {
            false
        };

        for job in self.jobs.values_mut() {
            if job.task_ids.contains(&outcome.task_id) {
                if terminal_failure || (terminal_success && !recorded_output) {
                    job.status = OmJobStatus::Failed;
                } else if recorded_output {
                    job.results = job
                        .task_ids
                        .iter()
                        .filter_map(|task_id| self.verified_outputs.get(task_id).cloned())
                        .collect();
                    if job
                        .task_ids
                        .iter()
                        .all(|task_id| self.verified_outputs.contains_key(task_id))
                    {
                        job.status = OmJobStatus::Completed;
                        job.completed_at = Some(now_ms);
                    } else {
                        job.status = OmJobStatus::Computing;
                    }
                } else if !matches!(job.status, OmJobStatus::Completed | OmJobStatus::Failed) {
                    job.status = OmJobStatus::Computing;
                }

                if job.status == OmJobStatus::Completed && job.completed_at.is_none() {
                    job.status = OmJobStatus::Completed;
                    job.completed_at = Some(now_ms);
                }
            }
        }

        Some(outcome.verify)
    }

    /// Get network-wide stats
    pub fn stats(&self) -> OmStats {
        let sched_stats = self.scheduler.stats();
        // KG: sprint6B-wasm-size-opt-2026-04-15 — u32 sum, no f64 needed
        let total_capacity: u32 = self.nodes.values()
            .filter(|n| n.can_accept_work())
            .map(|n| n.compute_capacity())
            .sum();

        OmStats {
            nodes_online: self.nodes.len(),
            nodes_available: self.nodes.values().filter(|n| n.can_accept_work()).count(),
            gpu_nodes: self.nodes.values().filter(|n| n.gpu_available).count(),
            total_capacity,
            total_jobs: self.jobs.len(),
            active_jobs: self.jobs.values().filter(|j| j.status == OmJobStatus::Computing || j.status == OmJobStatus::Distributing).count(),
            completed_jobs: self.jobs.values().filter(|j| j.status == OmJobStatus::Completed).count(),
            total_compute_ms: self.total_compute_ms,
            total_tokens_distributed: self.total_tokens_distributed,
            pending_tasks: sched_stats.pending,
        }
    }

    /// Get job status
    pub fn job_status(&self, job_id: &str) -> Option<&OmJob> {
        self.jobs.get(job_id)
    }

    /// List available compute nodes
    pub fn available_nodes(&self) -> Vec<&NodeResources> {
        self.nodes.values().filter(|n| n.can_accept_work()).collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmStats {
    pub nodes_online: usize,
    pub nodes_available: usize,
    pub gpu_nodes: usize,
    // KG: sprint6B-wasm-size-opt-2026-04-15 — was f64, now u32 (int capacity units)
    pub total_capacity: u32,
    pub total_jobs: usize,
    pub active_jobs: usize,
    pub completed_jobs: usize,
    pub total_compute_ms: u64,
    pub total_tokens_distributed: u64,
    pub pending_tasks: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn make_node(id: u32, cores: u32, gpu: bool) -> NodeResources {
        NodeResources {
            node_id: id, cpu_cores: cores, memory_mb: 4096,
            gpu_available: gpu, battery_pct: None,
            active_workers: 0, max_workers: cores,
        }
    }

    fn valid_result(
        task_id: TaskId,
        worker_id: u32,
        output: &[u8],
        compute_ms: u64,
    ) -> TaskResult {
        TaskResult {
            task_id,
            worker_id,
            output: output.to_vec(),
            compute_ms,
            output_hash: output_hash(output),
        }
    }

    #[test]
    fn register_nodes_and_submit_job() {
        let mut om = OmOrchestrator::new();
        om.register_node(make_node(1, 4, false));
        om.register_node(make_node(2, 8, true));

        let job_id = om.submit_job(
            OmTaskKind::CpuIntensive { estimated_ms: 1000 },
            1, 0,
        );
        assert!(job_id.starts_with("om-"));

        let stats = om.stats();
        assert_eq!(stats.nodes_online, 2);
        assert_eq!(stats.total_jobs, 1);
    }

    #[test]
    fn distribute_and_complete() {
        let mut om = OmOrchestrator::new();
        om.register_node(make_node(1, 4, false));

        let job_id = om.submit_job(
            OmTaskKind::CpuIntensive { estimated_ms: 500 },
            1, 0,
        );

        let assignments = om.distribute(100);
        assert_eq!(assignments.len(), 1);

        let verify = om.submit_result(
            valid_result(
                assignments[0].task_id,
                assignments[0].worker_id,
                b"result",
                500,
            ),
            600,
        );
        assert!(verify.is_some());

        let job = om.job_status(&job_id).unwrap();
        assert_eq!(job.status, OmJobStatus::Completed);
    }

    #[test]
    fn map_reduce_creates_multiple_tasks() {
        let mut om = OmOrchestrator::new();
        om.register_node(make_node(1, 4, false));
        om.register_node(make_node(2, 4, false));
        om.register_node(make_node(3, 4, false));

        let job_id = om.submit_job(
            OmTaskKind::MapReduce { chunk_count: 4, reduce_fn: "sum".into() },
            1, 0,
        );

        let job = om.job_status(&job_id).unwrap();
        assert_eq!(job.task_ids.len(), 4); // 4 chunks = 4 tasks
    }

    #[test]
    fn battery_low_rejects_work() {
        let mut node = make_node(1, 4, false);
        node.battery_pct = Some(15); // 15% < 20% threshold
        assert!(!node.can_accept_work());

        node.battery_pct = Some(50);
        assert!(node.can_accept_work());

        node.battery_pct = None; // plugged in
        assert!(node.can_accept_work());
    }

    #[test]
    fn gpu_node_has_higher_capacity() {
        let cpu_node = make_node(1, 4, false);
        let gpu_node = make_node(2, 4, true);
        assert!(gpu_node.compute_capacity() > cpu_node.compute_capacity());
    }

    #[test]
    fn stats_track_compute_time() {
        let mut om = OmOrchestrator::new();
        om.register_node(make_node(1, 4, false));

        om.submit_job(OmTaskKind::CpuIntensive { estimated_ms: 100 }, 1, 0);
        let assignments = om.distribute(0);

        om.submit_result(
            valid_result(
                assignments[0].task_id,
                assignments[0].worker_id,
                b"",
                250,
            ),
            250,
        );

        assert_eq!(om.stats().total_compute_ms, 250);
    }

    // KG: CONTRACT_333_Compute_OM T2 — duplicate register overwrites, no crash
    #[test]
    fn duplicate_register_node() {
        let mut om = OmOrchestrator::new();
        om.register_node(make_node(1, 4, false));
        om.register_node(make_node(1, 8, true)); // same id, different resources
        assert_eq!(om.stats().nodes_online, 1);
        // Second registration overwrites — GPU now available
        assert_eq!(om.stats().gpu_nodes, 1);
    }

    // KG: CONTRACT_333_Compute_OM T3 — no workers, distribute returns empty
    #[test]
    fn no_workers_distribute_empty() {
        let mut om = OmOrchestrator::new();
        let job_id = om.submit_job(OmTaskKind::CpuIntensive { estimated_ms: 100 }, 1, 0);
        let task_id = om.job_status(&job_id).unwrap().task_ids[0];
        let assignments = om.distribute(0);
        assert!(assignments.is_empty());
        assert_eq!(om.stats().pending_tasks, 1);

        om.register_node(make_node(7, 4, false));
        let assignments = om.distribute(1);
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].task_id, task_id);
    }

    // KG: CONTRACT_333_Compute_OM T4 — submit result for unassigned task returns None
    #[test]
    fn unauthorized_complete_ignored() {
        let mut om = OmOrchestrator::new();
        om.register_node(make_node(1, 4, false));
        // Submit result for task_id 999 that was never created
        let verify = om.submit_result(valid_result(999, 1, b"x", 10), 100);
        assert!(verify.is_none());
    }

    // KG: CONTRACT_333_Compute_OM T5 — failed canary slashes worker reputation via scheduler
    #[test]
    fn slash_reputation_on_canary_fail() {
        let mut om = OmOrchestrator::new();
        om.register_node(make_node(1, 4, false));
        // Directly use scheduler to submit a canary
        let tid = om.scheduler.submit_task(TaskType::Canary {
            description: "trap".into(), input: vec![],
            expected_hash: output_hash(b"correct"),
        }, 0);
        om.scheduler.assign_next(0);
        om.scheduler.submit_result(valid_result(tid, 1, b"wrong", 1));
        let worker = om.scheduler.worker_info(1).unwrap();
        assert_eq!(worker.tasks_failed, 1);
    }

    // KG: CONTRACT_333_Compute_OM T6 — newly registered node has default (New) reputation
    #[test]
    fn default_reputation_new() {
        let mut om = OmOrchestrator::new();
        om.register_node(make_node(42, 2, false));
        let worker = om.scheduler.worker_info(42).unwrap();
        assert_eq!(worker.reputation, ReputationTier::New);
        assert_eq!(worker.tasks_completed, 0);
    }

    #[test]
    fn quorum_job_finalizes_only_once_after_strict_majority() {
        let mut om = OmOrchestrator::new();
        om.register_node(make_node(1, 4, true));
        om.register_node(make_node(2, 4, true));
        let job_id = om.submit_job(OmTaskKind::GpuCompute { model_size_mb: 128 }, 9, 0);
        let assignments = om.distribute(0);
        let workers: HashSet<_> = assignments.iter().map(|a| a.worker_id).collect();
        assert_eq!(assignments.len(), 2);
        assert_eq!(workers.len(), 2);

        let first = valid_result(
            assignments[0].task_id,
            assignments[0].worker_id,
            b"agreed output",
            100,
        );
        let second = valid_result(
            assignments[1].task_id,
            assignments[1].worker_id,
            b"agreed output",
            120,
        );

        assert_eq!(
            om.submit_result(first, 100),
            Some(VerifyResult::InsufficientResults)
        );
        let job = om.job_status(&job_id).unwrap();
        assert_eq!(job.status, OmJobStatus::Computing);
        assert!(job.results.is_empty());
        assert_eq!(om.stats().total_tokens_distributed, 0);

        assert!(matches!(
            om.submit_result(second.clone(), 120),
            Some(VerifyResult::QuorumAgreed { agree_count: 2, .. })
        ));
        let job = om.job_status(&job_id).unwrap();
        assert_eq!(job.status, OmJobStatus::Completed);
        assert_eq!(job.results, vec![b"agreed output".to_vec()]);
        assert_eq!(om.stats().total_compute_ms, 220);
        assert_eq!(om.stats().total_tokens_distributed, 2);

        assert!(om.submit_result(second, 130).is_none());
        let job = om.job_status(&job_id).unwrap();
        assert_eq!(job.results.len(), 1);
        assert_eq!(om.stats().total_compute_ms, 220);
        assert_eq!(om.stats().total_tokens_distributed, 2);
    }

    #[test]
    fn failed_mapreduce_job_is_absorbing_when_other_chunks_finish() {
        let mut om = OmOrchestrator::new();
        for worker_id in 1..=6 {
            om.register_node(make_node(worker_id, 4, false));
        }
        let job_id = om.submit_job(
            OmTaskKind::MapReduce {
                chunk_count: 2,
                reduce_fn: "sum".into(),
            },
            99,
            0,
        );
        let assignments = om.distribute(0);
        assert_eq!(assignments.len(), 6);
        let job = om.job_status(&job_id).unwrap();
        let failed_task = job.task_ids[0];
        let successful_task = job.task_ids[1];
        let failed_workers: Vec<_> = assignments
            .iter()
            .filter(|assignment| assignment.task_id == failed_task)
            .map(|assignment| assignment.worker_id)
            .collect();
        let successful_workers: Vec<_> = assignments
            .iter()
            .filter(|assignment| assignment.task_id == successful_task)
            .map(|assignment| assignment.worker_id)
            .collect();

        for (index, worker_id) in failed_workers.into_iter().enumerate() {
            om.submit_result(
                valid_result(
                    failed_task,
                    worker_id,
                    format!("different-{index}").as_bytes(),
                    10,
                ),
                10,
            );
        }
        assert_eq!(om.job_status(&job_id).unwrap().status, OmJobStatus::Failed);

        for worker_id in successful_workers {
            om.submit_result(valid_result(successful_task, worker_id, b"same", 10), 20);
        }
        assert_eq!(om.job_status(&job_id).unwrap().status, OmJobStatus::Failed);
        assert_eq!(om.job_status(&job_id).unwrap().completed_at, None);
    }

    #[test]
    fn distribute_excludes_low_battery_nodes() {
        let mut om = OmOrchestrator::new();
        let mut low_battery = make_node(1, 4, false);
        low_battery.battery_pct = Some(15);
        om.register_node(low_battery);
        om.submit_job(OmTaskKind::CpuIntensive { estimated_ms: 10 }, 1, 0);

        assert!(om.distribute(0).is_empty());
        assert_eq!(om.stats().pending_tasks, 1);
    }

    #[test]
    fn distribute_routes_gpu_jobs_only_to_gpu_nodes() {
        let mut om = OmOrchestrator::new();
        om.register_node(make_node(1, 4, false));
        om.register_node(make_node(2, 4, false));
        om.submit_job(OmTaskKind::GpuCompute { model_size_mb: 8 }, 1, 0);

        assert!(om.distribute(0).is_empty());
        assert_eq!(om.stats().pending_tasks, 1);
    }
}
