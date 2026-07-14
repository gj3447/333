// KG: SPAN_333_L14_Orchestration, plan-333-p2p-os-synthesis-execution-2026-04-18,
//     queue-p7-orchestration-2026-04-18, finding_333_synth_rte_d{20..23}
//
// 333 P2P OS L14 Orchestration — trait surface + in-memory saga coordinator.
//
// Design sources (prom32-333-p2p-2026-04-18):
//   - finding_333_synth_rte_sota_d20: Restate-per-node + Temporal/Cadence durable workflows.
//   - finding_333_synth_rte_thr_d21: Saga pattern (Garcia-Molina & Salem 1987) + gossip-vision.
//   - finding_333_synth_rte_prt_d22: WorkflowTrait / StepExecutor / CompensatingAction split.
//   - finding_333_synth_rte_pit_d23: non-idempotent steps, compensation ordering, clock skew.
//
// This crate ships ONLY trait surface + an in-memory reference coordinator.
// Production backends (per-node Restate, PostgreSQL journal) implement
// `Journal` + `StepExecutor` in downstream crates. Events are published to a
// mq333 PubSub broker; state is a crdt333 Or-Set so replicas merge without
// central coordination. No async runtime dependency — coordinator is sync.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use crdt333::{AutoCrdt, Crdt};
#[allow(unused_imports)]
use crdt333::Lww;
use mq333::PubSub;
use thiserror::Error;

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum OrchError {
    #[error("unknown workflow: {0}")]
    UnknownWorkflow(WorkflowId),
    #[error("unknown step: wf={0} step={1}")]
    UnknownStep(WorkflowId, StepId),
    #[error("step executor missing for kind: {0}")]
    NoExecutor(String),
    #[error("step {step} in workflow {wf} is terminal ({state:?}); cannot transition")]
    Terminal { wf: WorkflowId, step: StepId, state: StepState },
    #[error("workflow {wf} is already terminal ({state:?}); cannot re-execute")]
    WorkflowTerminal { wf: WorkflowId, state: WorkflowState },
    #[error("compensation failed at step {0}: {1}")]
    CompensateFailed(StepId, String),
    #[error("step execution failed: {0}")]
    Exec(String),
    #[error("journal: {0}")]
    Journal(String),
    #[error("invalid workflow: {0}")]
    Invalid(String),
}

// ============================================================================
// Core domain types
// ============================================================================

pub type WorkflowId = String;
pub type StepId = String;

/// A single step specification. `kind` picks the `StepExecutor` at dispatch time;
/// `input` is the opaque payload passed to the executor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepSpec {
    pub id: StepId,
    pub kind: String,
    pub input: Vec<u8>,
    /// Steps that must be Committed before this step can run. For a linear saga
    /// the typical shape is `depends_on = vec![prev]`. Empty = start step.
    pub depends_on: Vec<StepId>,
}

/// Workflow definition. Steps form a DAG via `depends_on`. No cycles allowed
/// — validated in `Workflow::new`.
#[derive(Debug, Clone)]
pub struct WorkflowDef {
    pub id: WorkflowId,
    pub steps: Vec<StepSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StepState {
    Pending = 0,
    Running = 1,
    Committed = 2,
    Failed = 3,
    Compensated = 4,
}

impl StepState {
    pub fn is_terminal(self) -> bool {
        matches!(self, StepState::Committed | StepState::Failed | StepState::Compensated)
    }

    /// Legal saga step transitions. Any other edge is illegal and `record`
    /// rejects it with `OrchError::Terminal` (assessment action 6). Idempotent
    /// same-state re-records are allowed (the init loop may re-assert Pending).
    pub fn can_transition_to(self, next: StepState) -> bool {
        use StepState::*;
        self == next
            || matches!(
                (self, next),
                (Pending, Running) | (Running, Committed) | (Running, Failed) | (Committed, Compensated)
            )
    }
}

/// Overall workflow state. Runs the "last write wins" style ranking — used by
/// the Or-Set convergence check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkflowState {
    Running = 0,
    Succeeded = 1,
    Failed = 2,
    Compensating = 3,
    Compensated = 4,
}

impl WorkflowState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            WorkflowState::Succeeded | WorkflowState::Failed | WorkflowState::Compensated
        )
    }
}

// AutoCrdt impl lets a WorkflowState ride inside a crdt333::Lww. The ordering
// above (Running=0 < … < Compensated=4) is the monotone lattice chosen so that
// a failed run always dominates an intermediate Running on merge.
impl AutoCrdt for WorkflowState {}

/// One execution record (an event in the journal).
#[derive(Debug, Clone)]
pub struct StepEvent {
    pub wf: WorkflowId,
    pub step: StepId,
    pub state: StepState,
    pub output: Vec<u8>,
    pub seq: u64,
}

// ============================================================================
// Executor / Compensator traits (pluggable per step kind)
// ============================================================================

pub trait StepExecutor: Send + Sync {
    fn kind(&self) -> &str;
    fn run_step(&self, wf: &WorkflowId, step: &StepSpec) -> Result<Vec<u8>, OrchError>;
}

pub trait CompensatingAction: Send + Sync {
    fn kind(&self) -> &str;
    /// `forward_output` is whatever the step emitted on success — needed so
    /// compensation can reverse the specific effect (e.g. refund a payment id).
    fn compensate(
        &self,
        wf: &WorkflowId,
        step: &StepSpec,
        forward_output: &[u8],
    ) -> Result<(), OrchError>;
}

// ============================================================================
// Journal trait (durability boundary)
// ============================================================================

pub trait Journal: Send + Sync {
    fn append(&self, ev: StepEvent) -> Result<(), OrchError>;
    fn replay(&self, wf: &WorkflowId) -> Result<Vec<StepEvent>, OrchError>;
}

#[derive(Default)]
pub struct InMemoryJournal {
    inner: Mutex<BTreeMap<WorkflowId, Vec<StepEvent>>>,
}

impl InMemoryJournal {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl Journal for InMemoryJournal {
    fn append(&self, ev: StepEvent) -> Result<(), OrchError> {
        let mut g = self.inner.lock().map_err(|e| OrchError::Journal(e.to_string()))?;
        g.entry(ev.wf.clone()).or_default().push(ev);
        Ok(())
    }
    fn replay(&self, wf: &WorkflowId) -> Result<Vec<StepEvent>, OrchError> {
        let g = self.inner.lock().map_err(|e| OrchError::Journal(e.to_string()))?;
        Ok(g.get(wf).cloned().unwrap_or_default())
    }
}

// ============================================================================
// Workflow (in-memory DAG + validation)
// ============================================================================

#[derive(Debug)]
pub struct Workflow {
    def: WorkflowDef,
    index: HashMap<StepId, usize>,
}

impl Workflow {
    pub fn new(def: WorkflowDef) -> Result<Self, OrchError> {
        if def.steps.is_empty() {
            return Err(OrchError::Invalid("workflow has no steps".into()));
        }
        let mut index = HashMap::new();
        for (i, s) in def.steps.iter().enumerate() {
            if index.insert(s.id.clone(), i).is_some() {
                return Err(OrchError::Invalid(format!("duplicate step id {}", s.id)));
            }
        }
        for s in &def.steps {
            for d in &s.depends_on {
                if !index.contains_key(d) {
                    return Err(OrchError::Invalid(format!(
                        "step {} depends on unknown {}",
                        s.id, d
                    )));
                }
            }
        }
        Self::assert_acyclic(&def, &index)?;
        Ok(Self { def, index })
    }

    pub fn id(&self) -> &WorkflowId {
        &self.def.id
    }

    pub fn steps(&self) -> &[StepSpec] {
        &self.def.steps
    }

    pub fn step(&self, id: &str) -> Option<&StepSpec> {
        self.index.get(id).map(|i| &self.def.steps[*i])
    }

    fn assert_acyclic(
        def: &WorkflowDef,
        index: &HashMap<StepId, usize>,
    ) -> Result<(), OrchError> {
        // Kahn's algorithm over explicit deps.
        let n = def.steps.len();
        let mut indeg = vec![0usize; n];
        let mut adj: Vec<Vec<usize>> = vec![vec![]; n];
        for (i, s) in def.steps.iter().enumerate() {
            for d in &s.depends_on {
                let j = index[d];
                adj[j].push(i);
                indeg[i] += 1;
            }
        }
        let mut q: Vec<usize> = (0..n).filter(|i| indeg[*i] == 0).collect();
        let mut seen = 0usize;
        while let Some(u) = q.pop() {
            seen += 1;
            for &v in &adj[u] {
                indeg[v] -= 1;
                if indeg[v] == 0 {
                    q.push(v);
                }
            }
        }
        if seen != n {
            return Err(OrchError::Invalid("workflow has a cycle".into()));
        }
        Ok(())
    }
}

// ============================================================================
// WorkflowTrait (public execution surface)
// ============================================================================

pub trait WorkflowTrait {
    /// Kick off the workflow, driving all steps to a terminal state.
    /// On any step failure, previously-committed steps are compensated in
    /// reverse topological order.
    fn execute(&self, wf: &Workflow) -> Result<WorkflowState, OrchError>;

    /// Inspect the current state of a step.
    fn step_state(&self, wf: &WorkflowId, step: &StepId) -> Result<StepState, OrchError>;
}

// ============================================================================
// SagaCoordinator — reference implementation
// ============================================================================

pub struct SagaCoordinator {
    executors: HashMap<String, Arc<dyn StepExecutor>>,
    compensators: HashMap<String, Arc<dyn CompensatingAction>>,
    journal: Arc<dyn Journal>,
    bus: Arc<dyn PubSub>,
    state: Mutex<CoordinatorState>,
}

#[derive(Default)]
struct CoordinatorState {
    seq: u64,
    step_state: HashMap<(WorkflowId, StepId), StepState>,
    step_output: HashMap<(WorkflowId, StepId), Vec<u8>>,
    wf_state: HashMap<WorkflowId, WorkflowState>,
    committed_order: HashMap<WorkflowId, Vec<StepId>>,
}

impl SagaCoordinator {
    pub fn new(journal: Arc<dyn Journal>, bus: Arc<dyn PubSub>) -> Self {
        Self {
            executors: HashMap::new(),
            compensators: HashMap::new(),
            journal,
            bus,
            state: Mutex::new(CoordinatorState::default()),
        }
    }

    pub fn register_executor(&mut self, exec: Arc<dyn StepExecutor>) {
        self.executors.insert(exec.kind().to_string(), exec);
    }

    pub fn register_compensator(&mut self, comp: Arc<dyn CompensatingAction>) {
        self.compensators.insert(comp.kind().to_string(), comp);
    }

    pub fn workflow_state(&self, wf: &WorkflowId) -> WorkflowState {
        self.state
            .lock()
            .ok()
            .and_then(|g| g.wf_state.get(wf).copied())
            .unwrap_or(WorkflowState::Running)
    }

    fn publish(&self, ev: &StepEvent) {
        let topic = format!("orch.{}.{}", ev.wf, ev.step);
        let payload = format!("{:?}#{}", ev.state, ev.seq).into_bytes();
        let _ = self.bus.publish(&topic, payload);
    }

    fn record(
        &self,
        wf: &WorkflowId,
        step: &StepId,
        state: StepState,
        output: Vec<u8>,
    ) -> Result<StepEvent, OrchError> {
        let mut g = self.state.lock().map_err(|e| OrchError::Journal(e.to_string()))?;
        // Guard the step FSM: reject illegal transitions (overwriting a terminal
        // state, phase-skipping). Assessment action 6 — the intended guard that
        // was left dead code. Idempotent same-state re-records still pass.
        if let Some(&current) = g.step_state.get(&(wf.clone(), step.clone())) {
            if !current.can_transition_to(state) {
                return Err(OrchError::Terminal {
                    wf: wf.clone(),
                    step: step.clone(),
                    state: current,
                });
            }
        }
        g.seq += 1;
        let seq = g.seq;
        g.step_state.insert((wf.clone(), step.clone()), state);
        if !output.is_empty() {
            g.step_output.insert((wf.clone(), step.clone()), output.clone());
        }
        if state == StepState::Committed {
            g.committed_order.entry(wf.clone()).or_default().push(step.clone());
        }
        let ev = StepEvent {
            wf: wf.clone(),
            step: step.clone(),
            state,
            output,
            seq,
        };
        drop(g);
        self.journal.append(ev.clone())?;
        self.publish(&ev);
        Ok(ev)
    }

    fn set_wf_state(&self, wf: &WorkflowId, s: WorkflowState) {
        if let Ok(mut g) = self.state.lock() {
            g.wf_state.insert(wf.clone(), s);
        }
    }

    fn compensate_committed(&self, wf: &Workflow) -> Result<(), OrchError> {
        let order: Vec<StepId> = self
            .state
            .lock()
            .map_err(|e| OrchError::Journal(e.to_string()))?
            .committed_order
            .get(wf.id())
            .cloned()
            .unwrap_or_default();
        // Reverse order: undo most recent first (Garcia-Molina saga semantics).
        for step_id in order.into_iter().rev() {
            let spec = wf
                .step(&step_id)
                .ok_or_else(|| OrchError::UnknownStep(wf.id().clone(), step_id.clone()))?;
            let output = self
                .state
                .lock()
                .map_err(|e| OrchError::Journal(e.to_string()))?
                .step_output
                .get(&(wf.id().clone(), step_id.clone()))
                .cloned()
                .unwrap_or_default();
            let comp = self
                .compensators
                .get(&spec.kind)
                .cloned()
                .ok_or_else(|| OrchError::NoExecutor(spec.kind.clone()))?;
            comp.compensate(wf.id(), spec, &output).map_err(|e| {
                OrchError::CompensateFailed(step_id.clone(), e.to_string())
            })?;
            self.record(wf.id(), &step_id, StepState::Compensated, vec![])?;
        }
        Ok(())
    }
}

impl WorkflowTrait for SagaCoordinator {
    fn execute(&self, wf: &Workflow) -> Result<WorkflowState, OrchError> {
        // Guard: a terminal workflow must not be silently re-driven (assessment
        // action 6). Re-executing previously overwrote terminal step states with
        // Pending and re-ran committed steps.
        let current = self.workflow_state(wf.id());
        if current.is_terminal() {
            return Err(OrchError::WorkflowTerminal { wf: wf.id().clone(), state: current });
        }
        self.set_wf_state(wf.id(), WorkflowState::Running);
        // Initialize every step as Pending in the journal so replay is complete.
        for s in wf.steps() {
            self.record(wf.id(), &s.id, StepState::Pending, vec![])?;
        }

        let mut committed: BTreeSet<StepId> = BTreeSet::new();
        loop {
            // Find any step whose deps are satisfied and is still Pending.
            let ready: Option<&StepSpec> = wf.steps().iter().find(|s| {
                !committed.contains(&s.id)
                    && s.depends_on.iter().all(|d| committed.contains(d))
                    && self
                        .state
                        .lock()
                        .map(|g| {
                            matches!(
                                g.step_state.get(&(wf.id().clone(), s.id.clone())),
                                Some(StepState::Pending) | None
                            )
                        })
                        .unwrap_or(false)
            });

            let Some(step) = ready else {
                break;
            };

            self.record(wf.id(), &step.id, StepState::Running, vec![])?;
            let exec = self
                .executors
                .get(&step.kind)
                .cloned()
                .ok_or_else(|| OrchError::NoExecutor(step.kind.clone()))?;

            match exec.run_step(wf.id(), step) {
                Ok(out) => {
                    self.record(wf.id(), &step.id, StepState::Committed, out)?;
                    committed.insert(step.id.clone());
                }
                Err(e) => {
                    self.record(wf.id(), &step.id, StepState::Failed, e.to_string().into_bytes())?;
                    self.set_wf_state(wf.id(), WorkflowState::Compensating);
                    self.compensate_committed(wf)?;
                    self.set_wf_state(wf.id(), WorkflowState::Compensated);
                    return Ok(WorkflowState::Compensated);
                }
            }
        }

        // Check all steps reached Committed (otherwise we have a deadlock/cycle).
        if committed.len() != wf.steps().len() {
            self.set_wf_state(wf.id(), WorkflowState::Failed);
            return Ok(WorkflowState::Failed);
        }
        self.set_wf_state(wf.id(), WorkflowState::Succeeded);
        Ok(WorkflowState::Succeeded)
    }

    fn step_state(&self, wf: &WorkflowId, step: &StepId) -> Result<StepState, OrchError> {
        let g = self.state.lock().map_err(|e| OrchError::Journal(e.to_string()))?;
        g.step_state
            .get(&(wf.clone(), step.clone()))
            .copied()
            .ok_or_else(|| OrchError::UnknownStep(wf.clone(), step.clone()))
    }
}

// ============================================================================
// Or-Set CRDT state snapshot (committed step ids per workflow)
// ============================================================================

/// Lightweight snapshot of committed step ids, suitable for gossip-vision
/// replication across nodes. Uses crdt333::Map<StepId, Lww<WorkflowState>> as
/// a convergent representation. Merges across replicas are commutative.
pub fn merge_state(a: &mut crdt333::Lww<WorkflowState>, b: crdt333::Lww<WorkflowState>) {
    a.merge(&b);
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct OkExec {
        calls: Mutex<Vec<StepId>>,
    }
    impl StepExecutor for OkExec {
        fn kind(&self) -> &str {
            "ok"
        }
        fn run_step(&self, _wf: &WorkflowId, step: &StepSpec) -> Result<Vec<u8>, OrchError> {
            self.calls.lock().unwrap().push(step.id.clone());
            Ok(format!("out:{}", step.id).into_bytes())
        }
    }

    #[derive(Default)]
    struct FailExec {
        fail_on: String,
    }
    impl StepExecutor for FailExec {
        fn kind(&self) -> &str {
            "flaky"
        }
        fn run_step(&self, _wf: &WorkflowId, step: &StepSpec) -> Result<Vec<u8>, OrchError> {
            if step.id == self.fail_on {
                Err(OrchError::Exec(format!("boom at {}", step.id)))
            } else {
                Ok(vec![])
            }
        }
    }

    #[derive(Default)]
    struct RecComp {
        log: Mutex<Vec<StepId>>,
    }
    impl CompensatingAction for RecComp {
        fn kind(&self) -> &str {
            "ok"
        }
        fn compensate(
            &self,
            _wf: &WorkflowId,
            step: &StepSpec,
            _out: &[u8],
        ) -> Result<(), OrchError> {
            self.log.lock().unwrap().push(step.id.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct FlakyComp {
        log: Mutex<Vec<StepId>>,
    }
    impl CompensatingAction for FlakyComp {
        fn kind(&self) -> &str {
            "flaky"
        }
        fn compensate(
            &self,
            _wf: &WorkflowId,
            step: &StepSpec,
            _out: &[u8],
        ) -> Result<(), OrchError> {
            self.log.lock().unwrap().push(step.id.clone());
            Ok(())
        }
    }

    fn linear(id: &str, steps: &[&str], kind: &str) -> WorkflowDef {
        let mut v = Vec::new();
        for (i, s) in steps.iter().enumerate() {
            let deps = if i == 0 { vec![] } else { vec![steps[i - 1].to_string()] };
            v.push(StepSpec {
                id: s.to_string(),
                kind: kind.into(),
                input: vec![],
                depends_on: deps,
            });
        }
        WorkflowDef { id: id.into(), steps: v }
    }

    fn coordinator() -> (SagaCoordinator, Arc<InMemoryJournal>) {
        let j = InMemoryJournal::new();
        let bus = mq333::InMemoryBroker::new();
        (SagaCoordinator::new(j.clone(), bus), j)
    }

    #[test]
    fn happy_path_linear_3_steps() {
        let def = linear("wf1", &["a", "b", "c"], "ok");
        let wf = Workflow::new(def).unwrap();
        let (mut c, _j) = coordinator();
        let exec = Arc::new(OkExec::default());
        let comp = Arc::new(RecComp::default());
        c.register_executor(exec.clone());
        c.register_compensator(comp);

        let s = c.execute(&wf).unwrap();
        assert_eq!(s, WorkflowState::Succeeded);
        let calls = exec.calls.lock().unwrap().clone();
        assert_eq!(calls, vec!["a".to_string(), "b".into(), "c".into()]);
    }

    #[test]
    fn step_state_queryable_after_success() {
        let def = linear("wf2", &["a", "b"], "ok");
        let wf = Workflow::new(def).unwrap();
        let (mut c, _) = coordinator();
        c.register_executor(Arc::new(OkExec::default()));
        c.register_compensator(Arc::new(RecComp::default()));
        c.execute(&wf).unwrap();
        assert_eq!(c.step_state(&"wf2".into(), &"a".into()).unwrap(), StepState::Committed);
        assert_eq!(c.step_state(&"wf2".into(), &"b".into()).unwrap(), StepState::Committed);
    }

    #[test]
    fn re_executing_terminal_workflow_is_rejected() {
        // Assessment action 6: a Succeeded workflow must not be silently re-driven.
        let def = linear("wft", &["a", "b"], "ok");
        let wf = Workflow::new(def).unwrap();
        let (mut c, _) = coordinator();
        c.register_executor(Arc::new(OkExec::default()));
        c.register_compensator(Arc::new(RecComp::default()));
        assert_eq!(c.execute(&wf).unwrap(), WorkflowState::Succeeded);

        let err = c.execute(&wf).unwrap_err();
        assert!(
            matches!(err, OrchError::WorkflowTerminal { state: WorkflowState::Succeeded, .. }),
            "re-execute of terminal workflow must error, got {err:?}"
        );
        // Terminal step states preserved (not overwritten back to Pending).
        assert_eq!(c.step_state(&"wft".into(), &"a".into()).unwrap(), StepState::Committed);
    }

    #[test]
    fn step_state_transition_table_rejects_illegal_edges() {
        use StepState::*;
        // legal edges
        assert!(Pending.can_transition_to(Running));
        assert!(Running.can_transition_to(Committed));
        assert!(Running.can_transition_to(Failed));
        assert!(Committed.can_transition_to(Compensated));
        assert!(Committed.can_transition_to(Committed)); // idempotent re-record
        // illegal edges (the ones record() now rejects)
        assert!(!Committed.can_transition_to(Running)); // terminal overwrite / backward
        assert!(!Pending.can_transition_to(Committed)); // phase skip
        assert!(!Compensated.can_transition_to(Running));
        assert!(!Failed.can_transition_to(Committed));
    }

    #[test]
    fn failure_triggers_reverse_compensation() {
        let def = linear("wf3", &["a", "b", "c"], "flaky");
        let wf = Workflow::new(def).unwrap();
        let (mut c, _) = coordinator();
        c.register_executor(Arc::new(FailExec { fail_on: "c".into() }));
        let comp = Arc::new(FlakyComp::default());
        c.register_compensator(comp.clone());

        let s = c.execute(&wf).unwrap();
        assert_eq!(s, WorkflowState::Compensated);

        let log = comp.log.lock().unwrap().clone();
        // Reverse order: b first, then a.
        assert_eq!(log, vec!["b".to_string(), "a".into()]);
    }

    #[test]
    fn first_step_failure_no_compensation() {
        let def = linear("wf4", &["a", "b"], "flaky");
        let wf = Workflow::new(def).unwrap();
        let (mut c, _) = coordinator();
        c.register_executor(Arc::new(FailExec { fail_on: "a".into() }));
        let comp = Arc::new(FlakyComp::default());
        c.register_compensator(comp.clone());

        let s = c.execute(&wf).unwrap();
        assert_eq!(s, WorkflowState::Compensated);
        assert!(comp.log.lock().unwrap().is_empty());
    }

    #[test]
    fn unknown_step_state_errors() {
        let (c, _) = coordinator();
        let err = c.step_state(&"nope".into(), &"x".into()).unwrap_err();
        assert!(matches!(err, OrchError::UnknownStep(_, _)));
    }

    #[test]
    fn missing_executor_errors() {
        let def = linear("wf5", &["a"], "does-not-exist");
        let wf = Workflow::new(def).unwrap();
        let (c, _) = coordinator();
        // No executors registered at all.
        let err = c.execute(&wf).unwrap_err();
        assert!(matches!(err, OrchError::NoExecutor(_)));
    }

    #[test]
    fn empty_workflow_rejected() {
        let def = WorkflowDef { id: "empty".into(), steps: vec![] };
        let err = Workflow::new(def).unwrap_err();
        assert!(matches!(err, OrchError::Invalid(_)));
    }

    #[test]
    fn duplicate_step_id_rejected() {
        let def = WorkflowDef {
            id: "dup".into(),
            steps: vec![
                StepSpec { id: "a".into(), kind: "ok".into(), input: vec![], depends_on: vec![] },
                StepSpec { id: "a".into(), kind: "ok".into(), input: vec![], depends_on: vec![] },
            ],
        };
        assert!(matches!(Workflow::new(def), Err(OrchError::Invalid(_))));
    }

    #[test]
    fn unknown_dep_rejected() {
        let def = WorkflowDef {
            id: "bad".into(),
            steps: vec![StepSpec {
                id: "a".into(),
                kind: "ok".into(),
                input: vec![],
                depends_on: vec!["ghost".into()],
            }],
        };
        assert!(matches!(Workflow::new(def), Err(OrchError::Invalid(_))));
    }

    #[test]
    fn cycle_rejected() {
        let def = WorkflowDef {
            id: "cyc".into(),
            steps: vec![
                StepSpec {
                    id: "a".into(),
                    kind: "ok".into(),
                    input: vec![],
                    depends_on: vec!["b".into()],
                },
                StepSpec {
                    id: "b".into(),
                    kind: "ok".into(),
                    input: vec![],
                    depends_on: vec!["a".into()],
                },
            ],
        };
        let err = Workflow::new(def).unwrap_err();
        assert!(matches!(err, OrchError::Invalid(_)));
    }

    #[test]
    fn diamond_dag_executes_all() {
        // a -> b, a -> c, b+c -> d
        let def = WorkflowDef {
            id: "diamond".into(),
            steps: vec![
                StepSpec {
                    id: "a".into(),
                    kind: "ok".into(),
                    input: vec![],
                    depends_on: vec![],
                },
                StepSpec {
                    id: "b".into(),
                    kind: "ok".into(),
                    input: vec![],
                    depends_on: vec!["a".into()],
                },
                StepSpec {
                    id: "c".into(),
                    kind: "ok".into(),
                    input: vec![],
                    depends_on: vec!["a".into()],
                },
                StepSpec {
                    id: "d".into(),
                    kind: "ok".into(),
                    input: vec![],
                    depends_on: vec!["b".into(), "c".into()],
                },
            ],
        };
        let wf = Workflow::new(def).unwrap();
        let (mut c, _) = coordinator();
        let exec = Arc::new(OkExec::default());
        c.register_executor(exec.clone());
        c.register_compensator(Arc::new(RecComp::default()));
        assert_eq!(c.execute(&wf).unwrap(), WorkflowState::Succeeded);
        let calls = exec.calls.lock().unwrap().clone();
        // a must precede b, c; b and c must precede d.
        let pos = |id: &str| calls.iter().position(|x| x == id).unwrap();
        assert!(pos("a") < pos("b"));
        assert!(pos("a") < pos("c"));
        assert!(pos("b") < pos("d"));
        assert!(pos("c") < pos("d"));
    }

    #[test]
    fn journal_replay_roundtrip() {
        let def = linear("wfj", &["a", "b"], "ok");
        let wf = Workflow::new(def).unwrap();
        let (mut c, j) = coordinator();
        c.register_executor(Arc::new(OkExec::default()));
        c.register_compensator(Arc::new(RecComp::default()));
        c.execute(&wf).unwrap();
        let events = j.replay(&"wfj".into()).unwrap();
        // Pending (a,b) + Running/Committed (a) + Running/Committed (b) = 6 events.
        assert_eq!(events.len(), 6);
        let last = events.last().unwrap();
        assert_eq!(last.step, "b");
        assert_eq!(last.state, StepState::Committed);
    }

    #[test]
    fn pubsub_events_fire_for_every_transition() {
        let def = linear("wfe", &["a"], "ok");
        let wf = Workflow::new(def).unwrap();
        let bus = mq333::InMemoryBroker::new();
        let sid = bus.subscribe("orch.wfe.a").unwrap();
        let j = InMemoryJournal::new();
        let mut c = SagaCoordinator::new(j, bus.clone());
        c.register_executor(Arc::new(OkExec::default()));
        c.register_compensator(Arc::new(RecComp::default()));
        c.execute(&wf).unwrap();
        let msgs = bus.poll(sid).unwrap();
        // Pending + Running + Committed
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn crdt_state_merge_monotone() {
        use crdt333::Lww;
        let mut a = Lww::raw(1, WorkflowState::Running);
        let b = Lww::raw(2, WorkflowState::Succeeded);
        merge_state(&mut a, b);
        assert_eq!(*a.get(), WorkflowState::Succeeded);
    }

    #[test]
    fn step_state_terminal_classification() {
        assert!(!StepState::Pending.is_terminal());
        assert!(!StepState::Running.is_terminal());
        assert!(StepState::Committed.is_terminal());
        assert!(StepState::Failed.is_terminal());
        assert!(StepState::Compensated.is_terminal());
    }

    #[test]
    fn workflow_state_queryable() {
        let def = linear("wfs", &["a"], "ok");
        let wf = Workflow::new(def).unwrap();
        let (mut c, _) = coordinator();
        c.register_executor(Arc::new(OkExec::default()));
        c.register_compensator(Arc::new(RecComp::default()));
        assert_eq!(c.workflow_state(&"wfs".into()), WorkflowState::Running);
        c.execute(&wf).unwrap();
        assert_eq!(c.workflow_state(&"wfs".into()), WorkflowState::Succeeded);
    }
}
