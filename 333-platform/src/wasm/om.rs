// KG: sprint5B-om-wasm-ui-2026-04-15
//! WASM bindings for OmOrchestrator — exposes distributed compute orchestration
//! (Operations Manager) to JavaScript.
//!
//! Wraps `OmOrchestrator` (P2P task distribution, worker pool, resource awareness,
//! reward settlement) behind a `#[wasm_bindgen]` facade.
//! All JSON serialisation stays in this layer; inner crate types stay JS-free.
//!
//! # API surface
//! - `OmSessionWasm::new(peer_id)` — create orchestrator session for local node
//! - `register_node(resources_json)` → register this node's capabilities
//! - `update_node_resources(resources_json)` → heartbeat update
//! - `submit_job(kind_json)` → submit a compute job, returns job_id
//! - `distribute(now_ms)` → assign pending tasks, returns JSON array of assignments
//! - `submit_result(result_json, now_ms)` → submit task result, returns verify JSON
//! - `job_status_json(job_id)` → get job status as JSON
//! - `stats_json()` → network-wide stats snapshot
//! - `available_nodes_json()` → list of nodes that can accept work
//! - `serialize_state()` → full orchestrator snapshot

use wasm_bindgen::prelude::*;
use crate::compute::om::{NodeResources, OmOrchestrator, OmTaskKind};
use crate::compute::scheduler::SubmitResultError;
use crate::compute::task::{TaskResult, TaskResultError, VerifyResult};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Wire types (private — JSON boundary only)
// KG: sprint5B-om-wasm-ui-2026-04-15
// ---------------------------------------------------------------------------

/// JSON-friendly node registration payload.
#[derive(Deserialize)]
struct NodeResourcesWire {
    node_id: u32,
    cpu_cores: u32,
    memory_mb: u32,
    gpu_available: bool,
    battery_pct: Option<u8>,
    active_workers: u32,
    max_workers: u32,
}

/// JSON-friendly task result payload.
#[derive(Deserialize)]
struct TaskResultWire {
    task_id: u64,   // TaskId = u64
    worker_id: u32,
    output: Vec<u8>,
    compute_ms: u64,
    output_hash: String,
}

/// OmTaskKind wire format — serde-tagged enum for OCP-compliant dispatch.
///
/// OCP 원칙: 새 compute 종류 추가 시 이 enum + OmTaskKind + From impl에만 arm 추가.
/// `submit_job()` 본문은 수정 불필요 — serde가 tag 기반으로 자동 dispatch.
/// KG: SOLID-OCP-fix-om-dispatch-2026-04-16
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum OmTaskKindWire {
    Cpu {
        #[serde(default = "default_estimated_ms")]
        estimated_ms: u64,
    },
    Gpu {
        #[serde(default)]
        model_size_mb: u32,
    },
    Data {
        #[serde(default)]
        input_size_kb: u32,
    },
    Mapreduce {
        #[serde(default = "default_chunk_count")]
        chunk_count: u32,
        #[serde(default = "default_reduce_fn")]
        reduce_fn: String,
    },
}

fn default_estimated_ms() -> u64 { 1000 }
fn default_chunk_count() -> u32 { 4 }
fn default_reduce_fn() -> String { "sum".into() }

impl From<OmTaskKindWire> for OmTaskKind {
    fn from(w: OmTaskKindWire) -> Self {
        match w {
            OmTaskKindWire::Cpu { estimated_ms } =>
                OmTaskKind::CpuIntensive { estimated_ms },
            OmTaskKindWire::Gpu { model_size_mb } =>
                OmTaskKind::GpuCompute { model_size_mb },
            OmTaskKindWire::Data { input_size_kb } =>
                OmTaskKind::DataPipeline { input_size_kb },
            OmTaskKindWire::Mapreduce { chunk_count, reduce_fn } =>
                OmTaskKind::MapReduce { chunk_count, reduce_fn },
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a typed core outcome to a compact and stable JSON envelope.
/// KG: sprint5B-om-wasm-ui-2026-04-15
pub(crate) fn submit_result_to_json(
    outcome: Result<VerifyResult, SubmitResultError>,
) -> String {
    let value = match outcome {
        Ok(v) => verify_result_value(&v),
        Err(error) => submit_result_error_value(&error),
    };
    value.to_string()
}

fn verify_result_value(v: &VerifyResult) -> serde_json::Value {
    match v {
        VerifyResult::NoVerificationNeeded =>
            serde_json::json!({"verdict": "ok", "kind": "no_verification"}),
        VerifyResult::QuorumAgreed { agreed_hash, agree_count } =>
            serde_json::json!({
                "verdict": "ok",
                "kind": "quorum_agreed",
                "agreed_hash": agreed_hash,
                "agree_count": agree_count,
            }),
        VerifyResult::QuorumDisagreed { hashes } => serde_json::json!({
            "verdict": "fail",
            "kind": "quorum_disagreed",
            "hashes": hashes,
        }),
        VerifyResult::CanaryPassed =>
            serde_json::json!({"verdict": "ok", "kind": "canary_passed"}),
        VerifyResult::CanaryFailed { worker_id } =>
            serde_json::json!({
                "verdict": "fail",
                "kind": "canary_failed",
                "worker_id": worker_id,
            }),
        VerifyResult::InsufficientResults =>
            serde_json::json!({"verdict": "pending", "kind": "insufficient_results"}),
    }
}

fn submit_result_error_value(error: &SubmitResultError) -> serde_json::Value {
    match error {
        SubmitResultError::UnknownTask { task_id } => serde_json::json!({
            "verdict": "rejected",
            "kind": "unknown_task",
            "task_id": task_id,
        }),
        SubmitResultError::WorkerIdentityMismatch { authenticated, claimed } => {
            serde_json::json!({
                "verdict": "rejected",
                "kind": "worker_identity_mismatch",
                "authenticated_worker_id": authenticated,
                "claimed_worker_id": claimed,
            })
        }
        SubmitResultError::Rejected(reason) => match reason {
            TaskResultError::WrongTask { expected, actual } => serde_json::json!({
                "verdict": "rejected",
                "kind": "wrong_task",
                "expected_task_id": expected,
                "actual_task_id": actual,
            }),
            TaskResultError::NotAcceptingResults { status } => serde_json::json!({
                "verdict": "rejected",
                "kind": "task_not_accepting_results",
                "status": format!("{status:?}"),
            }),
            TaskResultError::UnassignedWorker { worker_id } => serde_json::json!({
                "verdict": "rejected",
                "kind": "unassigned_worker",
                "worker_id": worker_id,
            }),
            TaskResultError::DuplicateWorker { worker_id } => serde_json::json!({
                "verdict": "rejected",
                "kind": "duplicate_worker_result",
                "worker_id": worker_id,
            }),
            TaskResultError::OutputHashMismatch { .. } => serde_json::json!({
                "verdict": "rejected",
                "kind": "output_hash_mismatch",
            }),
        },
    }
}

// ---------------------------------------------------------------------------
// OmSessionWasm
// KG: sprint5B-om-wasm-ui-2026-04-15
// ---------------------------------------------------------------------------

/// WASM-exposed wrapper around `OmOrchestrator`.
///
/// Created via JS: `new wasm.OmSessionWasm(peerId)`
///
/// KG: sprint5B-om-wasm-ui-2026-04-15
#[wasm_bindgen]
pub struct OmSessionWasm {
    inner: OmOrchestrator,
    peer_id: u32,
}

#[wasm_bindgen]
impl OmSessionWasm {
    /// Create a new OM session for `peer_id` and auto-register this node.
    ///
    /// KG: sprint5B-om-wasm-ui-2026-04-15
    #[wasm_bindgen(constructor)]
    pub fn new(peer_id: u32) -> Self {
        console_error_panic_hook::set_once();
        let mut inner = OmOrchestrator::new();
        // Auto-register local node with sensible browser defaults
        inner.register_node(NodeResources {
            node_id: peer_id,
            cpu_cores: 4,
            memory_mb: 4096,
            gpu_available: false,
            battery_pct: None,
            active_workers: 0,
            max_workers: 4,
        });
        Self { inner, peer_id }
    }

    // -------------------------------------------------------------------------
    // Node management
    // -------------------------------------------------------------------------

    /// Register (or update) a node with its resource snapshot JSON.
    ///
    /// JSON shape: `{ node_id, cpu_cores, memory_mb, gpu_available, battery_pct, active_workers, max_workers }`
    ///
    /// KG: sprint5B-om-wasm-ui-2026-04-15
    pub fn register_node(&mut self, resources_json: &str) -> Result<(), JsValue> {
        let w: NodeResourcesWire = serde_json::from_str(resources_json)
            .map_err(|e| JsValue::from_str(&format!("parse error: {e}")))?;
        self.inner.register_node(NodeResources {
            node_id: w.node_id,
            cpu_cores: w.cpu_cores,
            memory_mb: w.memory_mb,
            gpu_available: w.gpu_available,
            battery_pct: w.battery_pct,
            active_workers: w.active_workers,
            max_workers: w.max_workers,
        });
        Ok(())
    }

    /// Heartbeat update for an existing node.
    ///
    /// KG: sprint5B-om-wasm-ui-2026-04-15
    pub fn update_node_resources(&mut self, resources_json: &str) -> Result<(), JsValue> {
        let w: NodeResourcesWire = serde_json::from_str(resources_json)
            .map_err(|e| JsValue::from_str(&format!("parse error: {e}")))?;
        self.inner.update_resources(NodeResources {
            node_id: w.node_id,
            cpu_cores: w.cpu_cores,
            memory_mb: w.memory_mb,
            gpu_available: w.gpu_available,
            battery_pct: w.battery_pct,
            active_workers: w.active_workers,
            max_workers: w.max_workers,
        });
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Job lifecycle
    // -------------------------------------------------------------------------

    /// Submit a compute job and return the job_id string.
    ///
    /// `kind_json` discriminated by `kind` field:
    /// - `{ "kind": "cpu", "estimated_ms": 1000 }`
    /// - `{ "kind": "gpu", "model_size_mb": 256 }`
    /// - `{ "kind": "data", "input_size_kb": 512 }`
    /// - `{ "kind": "mapreduce", "chunk_count": 4, "reduce_fn": "sum" }`
    ///
    /// KG: sprint5B-om-wasm-ui-2026-04-15
    pub fn submit_job(&mut self, kind_json: &str, now_ms: f64) -> Result<String, JsValue> {
        // OCP: OmTaskKindWire serde-tag dispatch → 새 종류 추가 시 이 본문 수정 불필요.
        // 새 compute 종류: OmTaskKindWire에 arm 추가 + From impl에 arm 추가만.
        // KG: SOLID-OCP-fix-om-dispatch-2026-04-16
        let kind: OmTaskKind = serde_json::from_str::<OmTaskKindWire>(kind_json)
            .map(OmTaskKind::from)
            .map_err(|e| JsValue::from_str(&format!("parse error: {e}")))?;
        let job_id = self.inner.submit_job(kind, self.peer_id, now_ms as u64);
        Ok(job_id)
    }

    /// Assign pending tasks to available nodes.
    ///
    /// Returns a JSON array of `TaskAssignment` objects.
    ///
    /// KG: sprint5B-om-wasm-ui-2026-04-15
    pub fn distribute(&mut self, now_ms: f64) -> String {
        let assignments = self.inner.distribute(now_ms as u64);
        serde_json::to_string(&assignments).unwrap_or_else(|_| "[]".to_string())
    }

    /// Submit a task result.
    ///
    /// `result_json` shape: `{ task_id, worker_id, output, compute_ms, output_hash }`
    ///
    /// Returns a typed JSON envelope for accepted, pending, failed, or rejected input.
    ///
    /// KG: sprint5B-om-wasm-ui-2026-04-15
    pub fn submit_result(&mut self, result_json: &str, now_ms: f64) -> Result<String, JsValue> {
        let w: TaskResultWire = serde_json::from_str(result_json)
            .map_err(|e| JsValue::from_str(&format!("parse error: {e}")))?;

        let result = TaskResult {
            task_id: w.task_id,
            worker_id: w.worker_id,
            output: w.output,
            compute_ms: w.compute_ms,
            output_hash: w.output_hash,
        };

        Ok(submit_result_to_json(self.inner.submit_result_from(
            self.peer_id,
            result,
            now_ms as u64,
        )))
    }

    // -------------------------------------------------------------------------
    // Queries
    // -------------------------------------------------------------------------

    /// Get a job's current status as JSON, or `null` if not found.
    ///
    /// KG: sprint5B-om-wasm-ui-2026-04-15
    pub fn job_status_json(&self, job_id: &str) -> String {
        match self.inner.job_status(job_id) {
            Some(job) => serde_json::to_string(job).unwrap_or_else(|_| "null".to_string()),
            None => "null".to_string(),
        }
    }

    /// Network-wide stats snapshot as JSON.
    ///
    /// KG: sprint5B-om-wasm-ui-2026-04-15
    pub fn stats_json(&self) -> String {
        serde_json::to_string(&self.inner.stats()).unwrap_or_else(|_| "{}".to_string())
    }

    /// JSON array of available node resources.
    ///
    /// KG: sprint5B-om-wasm-ui-2026-04-15
    pub fn available_nodes_json(&self) -> String {
        let nodes: Vec<&NodeResources> = self.inner.available_nodes();
        serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".to_string())
    }

    // -------------------------------------------------------------------------
    // Snapshot
    // -------------------------------------------------------------------------

    /// Serialise a stats + peer_id snapshot for persistence / debug.
    ///
    /// KG: sprint5B-om-wasm-ui-2026-04-15
    pub fn serialize_state(&self) -> String {
        let stats = self.inner.stats();
        serde_json::json!({
            "peer_id": self.peer_id,
            "nodes_online": stats.nodes_online,
            "nodes_available": stats.nodes_available,
            "gpu_nodes": stats.gpu_nodes,
            "total_capacity": stats.total_capacity,
            "total_jobs": stats.total_jobs,
            "active_jobs": stats.active_jobs,
            "completed_jobs": stats.completed_jobs,
            "total_compute_ms": stats.total_compute_ms,
            "total_tokens_distributed": stats.total_tokens_distributed,
            "pending_tasks": stats.pending_tasks,
        })
        .to_string()
    }

    // -------------------------------------------------------------------------
    // Utility
    // -------------------------------------------------------------------------

    /// Return this session's peer id.
    ///
    /// KG: sprint5B-om-wasm-ui-2026-04-15
    pub fn peer_id(&self) -> u32 {
        self.peer_id
    }
}
