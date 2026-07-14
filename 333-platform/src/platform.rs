// KG: CONTRACT_333_Runtime — PlatformState trait + ExecutionRequest DTO
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-platform-ExecutionRequest/ExecutionResponse/PlatformCore
// KG: lesson-333-solid-audit-2026-04-13 — PlatformCore 미사용, wasm.rs가 concrete 타입 직접 의존
// KG: sprint7F-kernel-platform-integration-2026-04-16 — ServiceRegistry + WorkerQueue 통합
// KG: sprint7J-capability-enforce-2026-04-16 — execute()/enqueue_work_with_cap capability gating
// Taliban Architecture Audit: #1 PlatformState trait, #2 BFT↔Executor contract, #3 Storage wiring
// Decouples wasm.rs from concrete types. Enables testing without WASM.
//
// NOTE (sprint7J-2026-04-16): 5/8 fields remain concrete types (HotStuffState, LwwMap, Hlc,
// Executor, TokenLedger). Storage is already `Box<dyn KVStore>`. Trait abstraction for the
// remaining 5 is DEFERRED — 333 has single implementation per type, and each type has its own
// unit test module. Trait extraction is warranted when alternate implementations emerge
// (e.g., WASM-optimized HotStuff, or persistent LwwMap).

use crate::bft::crypto::{NodeId, ValidatorSet};
use crate::bft::executor::{Executor, TxResult};
use crate::bft::state::HotStuffState;
use crate::bft::types::{OrderedTx, ProcessResult, HotStuffMsg};
use crate::hlc::Hlc;
use crate::lww_map::{LwwMap, LwwDelta};
use crate::storage::{KVStore, MemoryStore};
use crate::tokenomics::{TokenLedger, TokenTx, TokenResult};
use crate::kernel::{
    ServiceRegistry, WorkerQueue, JobPriority, ServiceState,
    Capability, CapabilitySet,
};
use crate::kernel::builtin::{CrdtService, ConsensusService, TokenService};

/// ExecutionRequest — formal BFT↔Executor contract (Taliban #2)
#[derive(Debug, Clone)]
pub enum ExecutionRequest {
    /// CRDT operation (no consensus needed)
    CrdtUpdate { key: String, value: Option<String> },
    /// Ordered operation (requires BFT consensus)
    Ordered(OrderedTx),
    /// Token operation (requires BFT + TokenLedger)
    Token(TokenTx),
}

/// ExecutionResponse
#[derive(Debug, Clone)]
pub enum ExecutionResponse {
    CrdtDelta(String),      // JSON delta for network sync
    Committed(Vec<TxResult>),
    TokenResult(TokenResult),
    Queued,                  // submitted to BFT, not yet committed
    Error(String),
}

/// Core platform state — decoupled from WASM (Taliban #1)
/// KG: sprint7F-kernel-platform-integration-2026-04-16 — kernel services wired
pub struct PlatformCore {
    pub node_id: NodeId,
    pub hlc: Hlc,
    pub world: LwwMap<String, String>,
    pub consensus: HotStuffState,
    pub executor: Executor,
    pub token_ledger: TokenLedger,
    pub storage: Box<dyn KVStore>,
    /// Kernel service registry — lifecycle + capability management
    pub kernel: ServiceRegistry,
    /// Deferred work queue — flushed via tick_kernel()
    pub work_queue: WorkerQueue,
}

impl PlatformCore {
    /// Boot the three built-in kernel services: crdt / consensus / token
    fn boot_kernel() -> ServiceRegistry {
        let mut reg = ServiceRegistry::new();

        let mut crdt_caps = CapabilitySet::new();
        crdt_caps.grant(Capability::WorldRead);
        crdt_caps.grant(Capability::WorldWrite);
        reg.register(CrdtService::new(), crdt_caps)
            .expect("crdt service registration");

        let mut con_caps = CapabilitySet::new();
        con_caps.grant(Capability::ConsensusSubmit);
        reg.register(ConsensusService::new(), con_caps)
            .expect("consensus service registration");

        let mut tok_caps = CapabilitySet::new();
        tok_caps.grant(Capability::TokenRead);
        tok_caps.grant(Capability::TokenTransfer);
        reg.register(TokenService::new(), tok_caps)
            .expect("token service registration");

        let errs = reg.start_all();
        debug_assert!(errs.is_empty(), "kernel boot errors: {:?}", errs);
        reg
    }

    pub fn new(node_id: NodeId, validators: &[NodeId]) -> Self {
        let vs = ValidatorSet::new(validators.to_vec());
        let mut executor = Executor::new();
        let mut token_ledger = TokenLedger::new();

        for &v in validators {
            executor.init_account(v, 10000);
        }
        token_ledger.genesis(validators);

        Self {
            node_id,
            hlc: Hlc::new(node_id),
            world: LwwMap::new(node_id),
            consensus: HotStuffState::new(node_id, vs),
            executor,
            token_ledger,
            storage: Box::new(MemoryStore::new()),
            kernel: Self::boot_kernel(),
            work_queue: WorkerQueue::new(),
        }
    }

    /// DIP: construct with explicit Identity (for WASM random-key scenario)
    /// KG: lesson-333-solid-audit-2026-04-13 — DIP fix
    pub fn with_identity(node_id: NodeId, validators: &[NodeId], identity: crate::crypto_real::Identity) -> Self {
        let vs = ValidatorSet::new(validators.to_vec());
        let mut executor = Executor::new();
        let mut token_ledger = TokenLedger::new();

        for &v in validators {
            executor.init_account(v, 10000);
        }
        token_ledger.genesis(validators);

        Self {
            node_id,
            hlc: Hlc::new(node_id),
            world: LwwMap::new(node_id),
            consensus: HotStuffState::with_identity(node_id, vs, identity),
            executor,
            token_ledger,
            storage: Box::new(MemoryStore::new()),
            kernel: Self::boot_kernel(),
            work_queue: WorkerQueue::new(),
        }
    }

    /// Execute a request through the appropriate path.
    /// Capability enforcement: each operation checks kernel service capabilities.
    /// KG: sprint7J-capability-enforce-2026-04-16
    pub fn execute(&mut self, req: ExecutionRequest) -> ExecutionResponse {
        match req {
            ExecutionRequest::CrdtUpdate { key, value } => {
                // Capability gate: CRDT write requires WorldWrite
                if !self.kernel_has_capability("platform:crdt", &Capability::WorldWrite) {
                    return ExecutionResponse::Error(
                        "capability denied: platform:crdt lacks world:write".into()
                    );
                }
                self.hlc.tick();
                let delta = match value {
                    Some(v) => self.world.set(key, v),
                    None => self.world.delete(key),
                };
                // Persist to storage (Taliban #3)
                let delta_json = serde_json::to_string(&delta).unwrap_or_default();
                let _ = self.storage.put(b"last_delta", delta_json.as_bytes());
                ExecutionResponse::CrdtDelta(delta_json)
            }
            ExecutionRequest::Ordered(tx) => {
                // Capability gate: consensus submit requires ConsensusSubmit
                if !self.kernel_has_capability("platform:consensus", &Capability::ConsensusSubmit) {
                    return ExecutionResponse::Error(
                        "capability denied: platform:consensus lacks consensus:submit".into()
                    );
                }
                self.consensus.submit_tx(tx);
                ExecutionResponse::Queued
            }
            ExecutionRequest::Token(tx) => {
                // Capability gate: token ops require TokenTransfer
                if !self.kernel_has_capability("platform:token", &Capability::TokenTransfer) {
                    return ExecutionResponse::Error(
                        "capability denied: platform:token lacks token:transfer".into()
                    );
                }
                let result = self.token_ledger.execute(&tx);
                ExecutionResponse::TokenResult(result)
            }
        }
    }

    // ── Kernel integration (sprint7F) ─────────────────────────────────────────

    /// Flush deferred work queue — call once per game-loop tick or JS poll.
    /// Returns number of jobs executed.
    /// KG: sprint7F-kernel-platform-integration-2026-04-16
    pub fn tick_kernel(&mut self, max_jobs: usize) -> usize {
        self.work_queue.tick(max_jobs)
    }

    /// Enqueue deferred work with given priority.
    /// Requires the specified capability — returns None if denied.
    /// KG: sprint7J-capability-enforce-2026-04-16
    pub fn enqueue_work_with_cap<F>(
        &mut self,
        name: impl Into<String>,
        priority: JobPriority,
        required_cap: &Capability,
        service_name: &str,
        work: F,
    ) -> Result<crate::kernel::JobId, String>
    where
        F: FnOnce() -> Result<(), String> + Send + 'static,
    {
        if !self.kernel_has_capability(service_name, required_cap) {
            return Err(format!("capability denied: {} lacks {}", service_name, required_cap));
        }
        Ok(self.work_queue.enqueue(name, priority, work))
    }

    /// Enqueue deferred work (unchecked — for platform-internal use only).
    pub fn enqueue_work<F>(&mut self, name: impl Into<String>, priority: JobPriority, work: F) -> crate::kernel::JobId
    where
        F: FnOnce() -> Result<(), String> + Send + 'static,
    {
        self.work_queue.enqueue(name, priority, work)
    }

    /// Check if a kernel service is running.
    pub fn kernel_service_running(&self, name: &str) -> bool {
        self.kernel.is_running(name)
    }

    /// Kernel service lifecycle state.
    pub fn kernel_service_state(&self, name: &str) -> Option<&ServiceState> {
        self.kernel.state(name)
    }

    /// All three built-in kernel services running?
    pub fn kernel_healthy(&self) -> bool {
        self.kernel.is_running("platform:crdt")
            && self.kernel.is_running("platform:consensus")
            && self.kernel.is_running("platform:token")
    }

    /// Check if a kernel service holds a specific capability.
    /// KG: sprint7J-capability-enforce-2026-04-16
    pub fn kernel_has_capability(&self, service_name: &str, cap: &Capability) -> bool {
        self.kernel.has_capability(service_name, cap)
    }

    /// Pending deferred jobs count.
    pub fn pending_work(&self) -> usize {
        self.work_queue.pending_count()
    }

    /// Process a consensus message (from P2P network)
    pub fn process_consensus(&mut self, msg: HotStuffMsg) -> ProcessResult {
        self.consensus.process(msg)
    }

    /// Execute committed transactions (after BFT decides)
    pub fn on_commit(&mut self, txs: &[OrderedTx]) -> Vec<TxResult> {
        let results = self.executor.execute_block(txs);
        // Persist committed block to storage (Taliban #3)
        let block_json = serde_json::to_string(txs).unwrap_or_default();
        let key = format!("block:{}", self.consensus.committed_count());
        let _ = self.storage.put(key.as_bytes(), block_json.as_bytes());
        results
    }

    /// Merge remote CRDT delta
    pub fn merge_remote_delta(&mut self, delta_json: &str) {
        if let Ok(delta) = serde_json::from_str::<LwwDelta<String, String>>(delta_json) {
            self.world.merge_delta(&delta);
        }
    }

    /// Get block from world
    pub fn get_block(&self, key: &str) -> Option<&String> {
        self.world.get(&key.to_string())
    }

    /// Token balance
    pub fn balance(&self, node_id: NodeId) -> u64 {
        self.token_ledger.balance(node_id)
    }

    /// World size
    pub fn world_size(&self) -> usize {
        self.world.len()
    }

    /// Storage size
    pub fn storage_size(&self) -> usize {
        self.storage.size()
    }

    /// Registered peer count (delegates to consensus keyring)
    /// KG: finding_333_solid_333_d2 — ValidatorKeyring 중복 제거, PlatformCore로 위임
    pub fn peer_count(&self) -> usize {
        self.consensus.keyring.peer_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_platform() -> PlatformCore {
        PlatformCore::new(1, &[1, 2, 3, 4, 5, 6, 7])
    }

    #[test]
    fn crdt_update_and_read() {
        let mut p = make_platform();
        let resp = p.execute(ExecutionRequest::CrdtUpdate {
            key: "0,0".into(), value: Some("stone".into()),
        });
        match resp {
            ExecutionResponse::CrdtDelta(json) => assert!(!json.is_empty()),
            _ => panic!("expected CrdtDelta"),
        }
        assert_eq!(p.get_block("0,0"), Some(&"stone".to_string()));
        assert_eq!(p.world_size(), 1);
    }

    #[test]
    fn crdt_delete() {
        let mut p = make_platform();
        p.execute(ExecutionRequest::CrdtUpdate {
            key: "x".into(), value: Some("v".into()),
        });
        p.execute(ExecutionRequest::CrdtUpdate {
            key: "x".into(), value: None,
        });
        assert_eq!(p.get_block("x"), None);
    }

    #[test]
    fn ordered_tx_queued() {
        let mut p = make_platform();
        let resp = p.execute(ExecutionRequest::Ordered(OrderedTx::Transfer {
            from: 1, to: 2, amount: 100, nonce: 0,
        }));
        match resp {
            ExecutionResponse::Queued => {}
            _ => panic!("expected Queued"),
        }
    }

    #[test]
    fn token_transfer() {
        let mut p = make_platform();
        let resp = p.execute(ExecutionRequest::Token(TokenTx::Transfer {
            from: 1, to: 2, amount: 500, nonce: 0,
        }));
        match resp {
            ExecutionResponse::TokenResult(TokenResult::Success) => {}
            other => panic!("expected Success, got {:?}", other),
        }
        assert_eq!(p.balance(1), 10000 - 500);
        assert_eq!(p.balance(2), 10000 + 500);
    }

    #[test]
    fn storage_persists_delta() {
        let mut p = make_platform();
        p.execute(ExecutionRequest::CrdtUpdate {
            key: "block".into(), value: Some("dirt".into()),
        });
        // Storage should have the last delta
        assert!(p.storage.get(b"last_delta").is_some());
        assert!(p.storage_size() > 0);
    }

    #[test]
    fn on_commit_persists_block() {
        let mut p = make_platform();
        let txs = vec![OrderedTx::Transfer {
            from: 1, to: 2, amount: 100, nonce: 0,
        }];
        let results = p.on_commit(&txs);
        assert_eq!(results.len(), 1);
        // Block persisted in storage
        assert!(p.storage.get(b"block:0").is_some());
    }

    #[test]
    fn merge_remote_delta() {
        let mut p1 = make_platform();
        let mut p2 = PlatformCore::new(2, &[1, 2, 3, 4, 5, 6, 7]);

        let resp = p1.execute(ExecutionRequest::CrdtUpdate {
            key: "a".into(), value: Some("1".into()),
        });
        if let ExecutionResponse::CrdtDelta(json) = resp {
            p2.merge_remote_delta(&json);
        }
        assert_eq!(p2.get_block("a"), Some(&"1".to_string()));
    }

    // ── Kernel integration tests ──────────────────────────────────────────────

    #[test]
    fn kernel_boots_with_platform() {
        let p = make_platform();
        assert!(p.kernel_healthy(), "all 3 built-in services must be Running on boot");
        assert!(p.kernel_service_running("platform:crdt"));
        assert!(p.kernel_service_running("platform:consensus"));
        assert!(p.kernel_service_running("platform:token"));
    }

    #[test]
    fn tick_kernel_drains_work() {
        use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
        let mut p = make_platform();
        let counter = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&counter);
        p.enqueue_work("test_job", JobPriority::Normal, move || {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert_eq!(p.pending_work(), 1);
        let ran = p.tick_kernel(10);
        assert_eq!(ran, 1);
        assert_eq!(p.pending_work(), 0);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn kernel_work_queue_priority() {
        use std::sync::{Arc, Mutex};
        let mut p = make_platform();
        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

        let o = Arc::clone(&order);
        p.enqueue_work("low",  JobPriority::Low,  move || { o.lock().unwrap().push("low");  Ok(()) });
        let o = Arc::clone(&order);
        p.enqueue_work("high", JobPriority::High, move || { o.lock().unwrap().push("high"); Ok(()) });

        p.tick_kernel(2);
        let result = order.lock().unwrap().clone();
        assert_eq!(result, vec!["high", "low"]);
    }

    #[test]
    fn kernel_state_after_construction() {
        use crate::kernel::ServiceState;
        let p = make_platform();
        assert_eq!(*p.kernel_service_state("platform:crdt").unwrap(), ServiceState::Running);
    }

    // ── GAP 1: Capability enforcement tests ──────────────────────────────────
    // KG: sprint7J-capability-enforce-2026-04-16

    #[test]
    fn execute_crdt_has_capability() {
        let p = make_platform();
        // platform:crdt should have WorldWrite
        assert!(p.kernel_has_capability("platform:crdt", &Capability::WorldWrite));
    }

    #[test]
    fn execute_consensus_has_capability() {
        let p = make_platform();
        assert!(p.kernel_has_capability("platform:consensus", &Capability::ConsensusSubmit));
    }

    #[test]
    fn execute_token_has_capability() {
        let p = make_platform();
        assert!(p.kernel_has_capability("platform:token", &Capability::TokenTransfer));
    }

    #[test]
    fn execute_nonexistent_service_no_capability() {
        let p = make_platform();
        assert!(!p.kernel_has_capability("phantom", &Capability::WorldRead));
    }

    #[test]
    fn execute_crdt_still_works_with_capability() {
        // Regression: ensure existing tests still pass with enforcement
        let mut p = make_platform();
        let resp = p.execute(ExecutionRequest::CrdtUpdate {
            key: "cap_test".into(), value: Some("v".into()),
        });
        match resp {
            ExecutionResponse::CrdtDelta(_) => {}
            _ => panic!("expected CrdtDelta"),
        }
    }

    // ── GAP 2: enqueue_work_with_cap ─────────────────────────────────────────

    #[test]
    fn enqueue_with_valid_cap_succeeds() {
        let mut p = make_platform();
        let result = p.enqueue_work_with_cap(
            "crdt_job", JobPriority::Normal,
            &Capability::WorldWrite, "platform:crdt",
            || Ok(()),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn enqueue_with_missing_cap_denied() {
        let mut p = make_platform();
        // platform:crdt does NOT have ConsensusSubmit
        let result = p.enqueue_work_with_cap(
            "bad_job", JobPriority::Normal,
            &Capability::ConsensusSubmit, "platform:crdt",
            || Ok(()),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("capability denied"));
    }
}
