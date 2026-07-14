// KG: sprint7F-kernel-platform-integration-2026-04-16, SPAN_333_Kernel
// Built-in platform services — shell controllers wired into ServiceRegistry.
// These carry lifecycle state + capability grants; actual data lives in PlatformCore.
// Pattern: Microkernel coordinator (service = policy, not data owner).

use super::service::{Service, ServiceError, sealed_pub::SealedMarker};
use super::capability::{ServiceContext, Capability, CapabilitySet};

/// Built-in service: CRDT world state coordinator
/// Owns no data — PlatformCore.world is the data owner.
/// Registry entry exists so kernel can lifecycle-manage + capability-gate CRDT access.
pub struct CrdtService {
    pub writes: u64,
    pub merges: u64,
}

impl CrdtService {
    pub fn new() -> Self { Self { writes: 0, merges: 0 } }
    pub fn record_write(&mut self)  { self.writes += 1; }
    pub fn record_merge(&mut self)  { self.merges += 1; }
}

impl Default for CrdtService { fn default() -> Self { Self::new() } }
impl SealedMarker for CrdtService {}

impl Service for CrdtService {
    fn name(&self) -> &str { "platform:crdt" }
    fn init(&mut self, _ctx: &mut ServiceContext) -> Result<(), ServiceError> { Ok(()) }
    fn run(&mut self, _ctx: &ServiceContext)      -> Result<(), ServiceError> { Ok(()) }
    fn stop(&mut self)                             -> Result<(), ServiceError> { Ok(()) }
    fn describe(&self) -> String {
        format!("CrdtService(writes={}, merges={})", self.writes, self.merges)
    }
}

/// Built-in service: BFT consensus coordinator
pub struct ConsensusService {
    pub proposals: u64,
    pub commits:   u64,
}

impl ConsensusService {
    pub fn new() -> Self { Self { proposals: 0, commits: 0 } }
    pub fn record_proposal(&mut self) { self.proposals += 1; }
    pub fn record_commit(&mut self)   { self.commits += 1; }
}

impl Default for ConsensusService { fn default() -> Self { Self::new() } }
impl SealedMarker for ConsensusService {}

impl Service for ConsensusService {
    fn name(&self) -> &str { "platform:consensus" }
    fn init(&mut self, _ctx: &mut ServiceContext) -> Result<(), ServiceError> { Ok(()) }
    fn run(&mut self, _ctx: &ServiceContext)      -> Result<(), ServiceError> { Ok(()) }
    fn stop(&mut self)                             -> Result<(), ServiceError> { Ok(()) }
    fn describe(&self) -> String {
        format!("ConsensusService(proposals={}, commits={})", self.proposals, self.commits)
    }
}

/// Built-in service: Token ledger coordinator
pub struct TokenService {
    pub transfers: u64,
}

impl TokenService {
    pub fn new() -> Self { Self { transfers: 0 } }
    pub fn record_transfer(&mut self) { self.transfers += 1; }
}

impl Default for TokenService { fn default() -> Self { Self::new() } }
impl SealedMarker for TokenService {}

impl Service for TokenService {
    fn name(&self) -> &str { "platform:token" }
    fn init(&mut self, _ctx: &mut ServiceContext) -> Result<(), ServiceError> { Ok(()) }
    fn run(&mut self, _ctx: &ServiceContext)      -> Result<(), ServiceError> { Ok(()) }
    fn stop(&mut self)                             -> Result<(), ServiceError> { Ok(()) }
    fn describe(&self) -> String {
        format!("TokenService(transfers={})", self.transfers)
    }
}

// KG: finding_333_om_hyper_333_d2, plan-333-solid-dc-reclassify-2026-04-16
// KG: insight-apt-weapons-as-computer-architecture — 재배맨 = scheduler
/// Built-in service: Hypervisor — manages WASM VM instance lifecycle
/// Pattern: wasmCloud HostApi (heartbeat/start/status/stop) adapted for 333.
/// Errors are encoded in VmState, NOT returned as Err (wasmCloud ErrorInResponse pattern).
///
/// Instance lifecycle: Pending → Starting → Running → Stopping → Stopped | Error
/// BFT consensus used for settlement only (token reward/slashing), NOT placement.
/// Placement hints stored in CRDT (LwwMap) for eventual consistency.
pub struct HypervisorService {
    /// Active VM instances: id → state
    pub instances: std::collections::HashMap<String, VmInstance>,
    /// Total instances ever created (monotonic counter)
    pub total_spawned: u64,
    /// Total instances stopped
    pub total_stopped: u64,
}

/// VM instance state — adapted from wasmCloud WorkloadState + HostWorkload
/// KG: finding_cs_jaebaeman_theory_d0 — instance = Actor with persistent PCB
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmState {
    /// Submitted but not yet initialized
    Pending,
    /// Initialization in progress
    Starting,
    /// Fully running, heartbeat active
    Running,
    /// Stop requested, cleanup in progress
    Stopping,
    /// Cleanly stopped
    Stopped,
    /// Failed with error message
    Error(String),
}

impl VmState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, VmState::Stopped | VmState::Error(_))
    }

    /// Legal VM lifecycle transitions, mirroring the kernel's guarded
    /// `LifecycleMachine` (ServiceState). VmState was previously mutated by
    /// direct field assignment with no guard (assessment action 7); this makes
    /// the FSM contract explicit and enforceable. Any non-terminal live state
    /// may transition to `Error`; terminal states (`Stopped`, `Error`) are stuck.
    pub fn can_transition_to(&self, next: &VmState) -> bool {
        use VmState::*;
        if self == next {
            return true; // idempotent re-assert
        }
        if self.is_terminal() {
            return false;
        }
        matches!(
            (self, next),
            (Pending, Starting) | (Starting, Running) | (Running, Stopping) | (Stopping, Stopped)
        ) || matches!(next, Error(_))
    }
}

/// A single VM instance — the "process" in our OS analogy
/// KG: CONTRACT_333_OmComponentUnit
#[derive(Debug, Clone)]
pub struct VmInstance {
    pub id: String,
    pub name: String,
    pub state: VmState,
    pub created_at: u64,   // unix timestamp ms
    pub heartbeat_at: u64, // last heartbeat ms
}

/// Heartbeat timeout — instances without heartbeat for this duration are marked Error
pub const HEARTBEAT_TIMEOUT_MS: u64 = 30_000; // 30 seconds

impl HypervisorService {
    pub fn new() -> Self {
        Self {
            instances: std::collections::HashMap::new(),
            total_spawned: 0,
            total_stopped: 0,
        }
    }

    /// Start a new VM instance with capability check.
    /// Requires ServiceSpawn capability on the calling service context.
    /// Errors encoded in VmState::Error, NOT returned as Err.
    /// KG: NP_wasmcloud_ErrorInResponse, lesson-tpa-wasmcloud-overclaim-2026-04-16
    pub fn vm_start(&mut self, id: String, name: String, now_ms: u64, ctx: &ServiceContext) -> (String, VmState) {
        // Capability enforcement (Taliban finding #1)
        if !ctx.caps.has(&Capability::ServiceSpawn) {
            return (id, VmState::Error("capability denied: ServiceSpawn required".into()));
        }
        if self.instances.contains_key(&id) {
            return (id, VmState::Error("instance ID already exists".into()));
        }
        let instance = VmInstance {
            id: id.clone(),
            name,
            state: VmState::Starting,
            created_at: now_ms,
            heartbeat_at: now_ms,
        };
        self.instances.insert(id.clone(), instance);
        self.total_spawned = self.total_spawned.saturating_add(1);
        // Transition Starting → Running
        if let Some(inst) = self.instances.get_mut(&id) {
            inst.state = VmState::Running;
        }
        (id, VmState::Running)
    }

    /// Stop a VM instance with capability check.
    /// Requires ServiceKill capability.
    /// KG: NP_wasmcloud_AsymmetricLifecycle — stop is best-effort
    pub fn vm_stop(&mut self, id: &str, ctx: &ServiceContext) -> VmState {
        if !ctx.caps.has(&Capability::ServiceKill) {
            return VmState::Error("capability denied: ServiceKill required".into());
        }
        match self.instances.get_mut(id) {
            Some(inst) => {
                inst.state = VmState::Stopping;
                // Transition Stopping → remove
                let _removed = self.instances.remove(id);
                self.total_stopped = self.total_stopped.saturating_add(1);
                VmState::Stopped
            }
            None => VmState::Error("instance not found".into()),
        }
    }

    /// Internal stop without capability check — for Service::stop() cleanup only
    fn vm_stop_internal(&mut self, id: &str) -> VmState {
        match self.instances.remove(id) {
            Some(_) => {
                self.total_stopped = self.total_stopped.saturating_add(1);
                VmState::Stopped
            }
            None => VmState::Error("instance not found".into()),
        }
    }

    /// Query VM status. Never returns Err.
    pub fn vm_status(&self, id: &str) -> VmState {
        match self.instances.get(id) {
            Some(inst) => inst.state.clone(),
            None => VmState::Error("instance not found".into()),
        }
    }

    /// Update heartbeat timestamp. Returns VmState for consistency (ErrorInResponse pattern).
    /// KG: CONTRACT_333_OmNodeHeartbeat
    pub fn vm_heartbeat(&mut self, id: &str, now_ms: u64) -> VmState {
        match self.instances.get_mut(id) {
            Some(inst) => {
                inst.heartbeat_at = now_ms;
                inst.state.clone()
            }
            None => VmState::Error("instance not found".into()),
        }
    }

    /// Sweep stale instances — mark instances without recent heartbeat as Error.
    /// Call from kernel tick loop (WorkerQueue) periodically.
    /// KG: CONTRACT_333_OmNodeHeartbeat — watchdog enforcement
    pub fn sweep_stale(&mut self, now_ms: u64, timeout_ms: u64) -> Vec<String> {
        let stale_ids: Vec<String> = self.instances.iter()
            .filter(|(_, inst)| {
                inst.state == VmState::Running
                    && now_ms.saturating_sub(inst.heartbeat_at) > timeout_ms
            })
            .map(|(id, _)| id.clone())
            .collect();

        for id in &stale_ids {
            if let Some(inst) = self.instances.get_mut(id) {
                inst.state = VmState::Error(format!(
                    "heartbeat timeout: last seen {}ms ago",
                    now_ms.saturating_sub(inst.heartbeat_at)
                ));
            }
        }
        stale_ids
    }

    /// Count running instances
    pub fn running_count(&self) -> usize {
        self.instances.values()
            .filter(|i| i.state == VmState::Running)
            .count()
    }

    /// List all instance IDs with state
    pub fn list_instances(&self) -> Vec<(&str, &VmState)> {
        let mut list: Vec<_> = self.instances.iter()
            .map(|(id, inst)| (id.as_str(), &inst.state))
            .collect();
        list.sort_by_key(|(id, _)| *id); // deterministic ordering
        list
    }
}

impl Default for HypervisorService { fn default() -> Self { Self::new() } }
impl SealedMarker for HypervisorService {}

impl Service for HypervisorService {
    fn name(&self) -> &str { "platform:hypervisor" }
    fn init(&mut self, _ctx: &mut ServiceContext) -> Result<(), ServiceError> { Ok(()) }
    fn run(&mut self, _ctx: &ServiceContext) -> Result<(), ServiceError> { Ok(()) }
    fn stop(&mut self) -> Result<(), ServiceError> {
        // Best-effort: stop all running instances (no cap check — service-level cleanup)
        let ids: Vec<String> = self.instances.keys().cloned().collect();
        for id in ids {
            self.vm_stop_internal(&id);
        }
        Ok(())
    }
    fn describe(&self) -> String {
        format!("HypervisorService(running={}, spawned={}, stopped={})",
            self.running_count(), self.total_spawned, self.total_stopped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{ServiceRegistry, CapabilitySet, Capability};

    fn make_hyp_ctx() -> ServiceContext {
        let mut caps = CapabilitySet::new();
        caps.grant(Capability::ServiceSpawn);
        caps.grant(Capability::ServiceKill);
        ServiceContext::new("platform:hypervisor".to_string(), caps)
    }

    fn make_no_caps_ctx() -> ServiceContext {
        ServiceContext::new("unauthorized".to_string(), CapabilitySet::new())
    }

    #[test]
    fn hypervisor_service_lifecycle() {
        let mut hyp = HypervisorService::new();
        let ctx = make_hyp_ctx();

        // Start VM
        let (id, state) = hyp.vm_start("vm-1".into(), "test-vm".into(), 1000, &ctx);
        assert_eq!(id, "vm-1");
        assert_eq!(state, VmState::Running);
        assert_eq!(hyp.running_count(), 1);
        assert_eq!(hyp.total_spawned, 1);

        // Duplicate ID
        let (_, state) = hyp.vm_start("vm-1".into(), "dup".into(), 2000, &ctx);
        assert_eq!(state, VmState::Error("instance ID already exists".into()));
        assert_eq!(hyp.running_count(), 1);

        // Status
        assert_eq!(hyp.vm_status("vm-1"), VmState::Running);
        assert_eq!(hyp.vm_status("nonexistent"), VmState::Error("instance not found".into()));

        // Heartbeat
        assert_eq!(hyp.vm_heartbeat("vm-1", 3000), VmState::Running);
        assert_eq!(hyp.vm_heartbeat("nonexistent", 3000), VmState::Error("instance not found".into()));

        // Stop
        assert_eq!(hyp.vm_stop("vm-1", &ctx), VmState::Stopped);
        assert_eq!(hyp.running_count(), 0);
        assert_eq!(hyp.total_stopped, 1);

        // Stop nonexistent
        assert_eq!(hyp.vm_stop("vm-1", &ctx), VmState::Error("instance not found".into()));
    }

    #[test]
    fn hypervisor_capability_enforcement() {
        let mut hyp = HypervisorService::new();
        let no_caps = make_no_caps_ctx();
        let good_ctx = make_hyp_ctx();

        // Start without ServiceSpawn → denied
        let (_, state) = hyp.vm_start("vm-1".into(), "test".into(), 0, &no_caps);
        assert_eq!(state, VmState::Error("capability denied: ServiceSpawn required".into()));
        assert_eq!(hyp.running_count(), 0);

        // Start with proper caps → ok
        let (_, state) = hyp.vm_start("vm-1".into(), "test".into(), 0, &good_ctx);
        assert_eq!(state, VmState::Running);

        // Stop without ServiceKill → denied
        assert_eq!(hyp.vm_stop("vm-1", &no_caps), VmState::Error("capability denied: ServiceKill required".into()));
        assert_eq!(hyp.running_count(), 1); // still running

        // Stop with proper caps → ok
        assert_eq!(hyp.vm_stop("vm-1", &good_ctx), VmState::Stopped);
    }

    #[test]
    fn hypervisor_heartbeat_watchdog() {
        let mut hyp = HypervisorService::new();
        let ctx = make_hyp_ctx();

        hyp.vm_start("vm-1".into(), "a".into(), 1000, &ctx);
        hyp.vm_start("vm-2".into(), "b".into(), 1000, &ctx);

        // vm-1 sends heartbeat at 20000, vm-2 doesn't
        hyp.vm_heartbeat("vm-1", 20000);

        // Sweep at 35000 with 30s timeout → vm-2 stale (last seen at 1000, 34s ago)
        let stale = hyp.sweep_stale(35000, HEARTBEAT_TIMEOUT_MS);
        assert_eq!(stale, vec!["vm-2".to_string()]);
        assert_eq!(hyp.vm_status("vm-2"), VmState::Error("heartbeat timeout: last seen 34000ms ago".into()));
        assert_eq!(hyp.vm_status("vm-1"), VmState::Running); // still ok
        assert_eq!(hyp.running_count(), 1);
    }

    #[test]
    fn hypervisor_multiple_instances() {
        let mut hyp = HypervisorService::new();
        let ctx = make_hyp_ctx();
        for i in 0..10 {
            hyp.vm_start(format!("vm-{}", i), format!("worker-{}", i), i * 100, &ctx);
        }
        assert_eq!(hyp.running_count(), 10);
        assert_eq!(hyp.total_spawned, 10);
        assert_eq!(hyp.list_instances().len(), 10);

        // Stop half
        for i in 0..5 {
            hyp.vm_stop(&format!("vm-{}", i), &ctx);
        }
        assert_eq!(hyp.running_count(), 5);
        assert_eq!(hyp.total_stopped, 5);
    }

    #[test]
    fn hypervisor_service_stop_cleans_all() {
        let mut hyp = HypervisorService::new();
        let ctx = make_hyp_ctx();
        hyp.vm_start("a".into(), "a".into(), 0, &ctx);
        hyp.vm_start("b".into(), "b".into(), 0, &ctx);
        hyp.vm_start("c".into(), "c".into(), 0, &ctx);
        assert_eq!(hyp.running_count(), 3);

        hyp.stop().unwrap();
        assert_eq!(hyp.running_count(), 0);
        assert_eq!(hyp.total_stopped, 3);
    }

    #[test]
    fn hypervisor_list_deterministic_order() {
        let mut hyp = HypervisorService::new();
        let ctx = make_hyp_ctx();
        hyp.vm_start("charlie".into(), "c".into(), 0, &ctx);
        hyp.vm_start("alpha".into(), "a".into(), 0, &ctx);
        hyp.vm_start("bravo".into(), "b".into(), 0, &ctx);
        let list = hyp.list_instances();
        let ids: Vec<&str> = list.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec!["alpha", "bravo", "charlie"]); // sorted
    }

    #[test]
    fn hypervisor_registered_in_kernel() {
        let mut reg = ServiceRegistry::new();
        let mut hyp_caps = CapabilitySet::new();
        hyp_caps.grant(Capability::ServiceSpawn);
        hyp_caps.grant(Capability::ServiceKill);
        reg.register(HypervisorService::new(), hyp_caps).unwrap();
        let errs = reg.start_all();
        assert!(errs.is_empty());
        assert!(reg.is_running("platform:hypervisor"));
    }

    #[test]
    fn builtin_services_boot() {
        let mut reg = ServiceRegistry::new();
        let mut crdt_caps = CapabilitySet::new();
        crdt_caps.grant(Capability::WorldRead);
        crdt_caps.grant(Capability::WorldWrite);
        reg.register(CrdtService::new(), crdt_caps).unwrap();

        let mut con_caps = CapabilitySet::new();
        con_caps.grant(Capability::ConsensusSubmit);
        reg.register(ConsensusService::new(), con_caps).unwrap();

        let mut tok_caps = CapabilitySet::new();
        tok_caps.grant(Capability::TokenRead);
        tok_caps.grant(Capability::TokenTransfer);
        reg.register(TokenService::new(), tok_caps).unwrap();

        let errs = reg.start_all();
        assert!(errs.is_empty());
        assert!(reg.is_running("platform:crdt"));
        assert!(reg.is_running("platform:consensus"));
        assert!(reg.is_running("platform:token"));
    }

    #[test]
    fn vmstate_transition_table_guards_illegal_edges() {
        use VmState::*;
        // legal lifecycle edges
        assert!(Pending.can_transition_to(&Starting));
        assert!(Starting.can_transition_to(&Running));
        assert!(Running.can_transition_to(&Stopping));
        assert!(Stopping.can_transition_to(&Stopped));
        assert!(Running.can_transition_to(&Error("boom".into()))); // live state may fail
        assert!(Running.can_transition_to(&Running)); // idempotent
        // illegal edges (previously possible via direct field assignment)
        assert!(!Stopped.can_transition_to(&Running)); // terminal
        assert!(!Error("x".into()).can_transition_to(&Running)); // terminal
        assert!(!Pending.can_transition_to(&Running)); // skips Starting
        assert!(!Running.can_transition_to(&Pending)); // backward
    }
}
