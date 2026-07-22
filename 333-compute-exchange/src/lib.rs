//! Consent-first pure domain kernel for the 333 Compute Exchange alpha.
//!
//! The crate intentionally contains no network, storage, sandbox, payment, or
//! clock adapter. Environmental work leaves the reducer as typed [`Effect`]
//! values. The application shell must durably persist accepted events and an
//! outbox before it can make stronger recovery claims.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt::Write as _;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub String);

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl $name {
            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }
    };
}

string_id!(NodeId);
string_id!(PrincipalId);
string_id!(GrantId);
string_id!(LeaseId);
string_id!(AssignmentId);
string_id!(AttemptId);
string_id!(SettlementId);
string_id!(CapabilityId);
string_id!(IdempotencyKey);
string_id!(EffectId);

pub const BROWSER_ALPHA_V1_POLICY_HASH: &str = "333-browser-alpha-v1";

/// Policy is injected into the kernel. It is not contributor consent: a grant
/// must be both valid under this upper bound and explicitly accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnginePolicy {
    pub policy_hash: String,
    pub maximum: ResourceCaps,
    pub browser_foreground_only: bool,
    pub maximum_outstanding_assignments: u8,
}

impl EnginePolicy {
    pub fn browser_alpha_v1() -> Self {
        Self {
            policy_hash: BROWSER_ALPHA_V1_POLICY_HASH.to_owned(),
            maximum: ResourceCaps {
                workers: 4,
                duty_cycle_percent: 50,
                memory_mib: 512,
                network_bytes: 0,
                gpu: false,
            },
            browser_foreground_only: true,
            maximum_outstanding_assignments: 1,
        }
    }
}

impl Default for EnginePolicy {
    fn default() -> Self {
        Self::browser_alpha_v1()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceCaps {
    pub workers: u16,
    pub duty_cycle_percent: u8,
    pub memory_mib: u32,
    pub network_bytes: u64,
    pub gpu: bool,
}

impl ResourceCaps {
    fn validate_nonzero(&self) -> Result<(), Rejection> {
        if self.workers == 0 {
            return Err(Rejection::InvalidInput {
                field: "workers",
                problem: InputProblem::MustBePositive,
            });
        }
        if self.duty_cycle_percent == 0 {
            return Err(Rejection::InvalidInput {
                field: "duty_cycle_percent",
                problem: InputProblem::MustBePositive,
            });
        }
        if self.memory_mib == 0 {
            return Err(Rejection::InvalidInput {
                field: "memory_mib",
                problem: InputProblem::MustBePositive,
            });
        }
        Ok(())
    }

    fn require_within(&self, upper: &Self) -> Result<(), Rejection> {
        if self.workers > upper.workers {
            return Err(Rejection::ScopeExceeded(ScopeViolation::Workers {
                requested: self.workers as u64,
                allowed: upper.workers as u64,
            }));
        }
        if self.duty_cycle_percent > upper.duty_cycle_percent {
            return Err(Rejection::ScopeExceeded(ScopeViolation::DutyCycle {
                requested: self.duty_cycle_percent as u64,
                allowed: upper.duty_cycle_percent as u64,
            }));
        }
        if self.memory_mib > upper.memory_mib {
            return Err(Rejection::ScopeExceeded(ScopeViolation::MemoryMib {
                requested: self.memory_mib as u64,
                allowed: upper.memory_mib as u64,
            }));
        }
        if self.network_bytes > upper.network_bytes {
            return Err(Rejection::ScopeExceeded(ScopeViolation::NetworkBytes {
                requested: self.network_bytes,
                allowed: upper.network_bytes,
            }));
        }
        if self.gpu && !upper.gpu {
            return Err(Rejection::ScopeExceeded(ScopeViolation::GpuForbidden));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    BrowserForeground,
    BrowserBackground,
    NativeNode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceGrant {
    pub id: GrantId,
    pub principal: PrincipalId,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub browser_foreground_only: bool,
    pub max_workers: u16,
    pub duty_cycle_percent: u8,
    pub memory_mib: u32,
    pub network_bytes: u64,
    pub gpu_allowed: bool,
    pub policy_hash: String,
}

impl ResourceGrant {
    pub fn caps(&self) -> ResourceCaps {
        ResourceCaps {
            workers: self.max_workers,
            duty_cycle_percent: self.duty_cycle_percent,
            memory_mib: self.memory_mib,
            network_bytes: self.network_bytes,
            gpu: self.gpu_allowed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLease {
    pub id: LeaseId,
    pub grant_id: GrantId,
    pub generation: u64,
    pub mode: ExecutionMode,
    pub resources: ResourceCaps,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub id: AssignmentId,
    pub lease_id: LeaseId,
    pub generation: u64,
    pub requester: PrincipalId,
    /// Digest of the exact workload envelope. The kernel does not fetch code.
    pub workload_digest: String,
    pub mode: ExecutionMode,
    pub resources: ResourceCaps,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub id: AttemptId,
    pub assignment_id: AssignmentId,
    pub producer: PrincipalId,
    pub generation: u64,
    /// Frozen at admission so result metering cannot expand from an assignment
    /// subset back up to the broader lease allowance.
    pub resources: ResourceCaps,
    pub workload_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedResult {
    pub producer: PrincipalId,
    pub result_digest: String,
    pub usage: ResourceCaps,
    pub compute_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifierCapability {
    pub id: CapabilityId,
    pub subject: PrincipalId,
    pub node_id: NodeId,
    pub attempt_id: AttemptId,
    pub not_before_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationVerdict {
    Accept,
    Reject { code: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationRecord {
    pub attempt_id: AttemptId,
    pub verifier: PrincipalId,
    pub capability_id: CapabilityId,
    pub result_digest: String,
    pub verified_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementPlan {
    pub id: SettlementId,
    pub attempt_id: AttemptId,
    pub beneficiary: PrincipalId,
    pub service_credits: u64,
    pub verified_result_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReceipt {
    pub settlement_id: SettlementId,
    pub service_credits: u64,
    /// Opaque receipt returned by the accounting adapter.
    pub external_reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentState {
    Unseen,
    Active { grant_id: GrantId },
    Revoked { grant_id: GrantId, at_ms: u64 },
    Expired { grant_id: GrantId, at_ms: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseState {
    Closed,
    Active(ResourceLease),
    Stopped {
        lease_id: LeaseId,
        reason: StopReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptState {
    None,
    Offered(Assignment),
    Running(Attempt),
    ResultSubmitted {
        attempt: Attempt,
        result: SubmittedResult,
    },
    Verified {
        attempt: Attempt,
        result: SubmittedResult,
        verification: VerificationRecord,
    },
    Failed {
        assignment_id: AssignmentId,
        attempt_id: Option<AttemptId>,
        reason: AttemptFailure,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlementState {
    None,
    Prepared(SettlementPlan),
    Posted {
        plan: SettlementPlan,
        receipt: SettlementReceipt,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    ConsentRevoked,
    ConsentExpired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptFailure {
    ConsentRevoked,
    ConsentExpired,
    VerificationRejected { code: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aggregate {
    pub node_id: NodeId,
    pub generation: u64,
    pub observed_at_ms: u64,
    pub consent: ConsentState,
    pub grant: Option<ResourceGrant>,
    pub lease: LeaseState,
    pub attempt: AttemptState,
    pub settlement: SettlementState,
}

impl Aggregate {
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            generation: 0,
            observed_at_ms: 0,
            consent: ConsentState::Unseen,
            grant: None,
            lease: LeaseState::Closed,
            attempt: AttemptState::None,
            settlement: SettlementState::None,
        }
    }

    pub fn has_outstanding_assignment(&self) -> bool {
        match &self.attempt {
            AttemptState::None | AttemptState::Failed { .. } => false,
            AttemptState::Verified { .. } => {
                !matches!(self.settlement, SettlementState::Posted { .. })
            }
            AttemptState::Offered(_)
            | AttemptState::Running(_)
            | AttemptState::ResultSubmitted { .. } => true,
        }
    }

    fn active_grant(&self) -> Result<&ResourceGrant, Rejection> {
        match &self.consent {
            ConsentState::Active { .. } => {
                let grant = self.grant.as_ref().ok_or(Rejection::ConsentRequired)?;
                if self.observed_at_ms >= grant.expires_at_ms {
                    return Err(Rejection::Expired {
                        expires_at_ms: grant.expires_at_ms,
                        observed_at_ms: self.observed_at_ms,
                    });
                }
                Ok(grant)
            }
            ConsentState::Unseen => Err(Rejection::ConsentRequired),
            ConsentState::Revoked { .. } => Err(Rejection::ConsentTerminal {
                state: TerminalConsent::Revoked,
            }),
            ConsentState::Expired { .. } => Err(Rejection::ConsentTerminal {
                state: TerminalConsent::Expired,
            }),
        }
    }

    fn active_lease(&self) -> Result<&ResourceLease, Rejection> {
        match &self.lease {
            LeaseState::Active(lease) => Ok(lease),
            LeaseState::Closed | LeaseState::Stopped { .. } => Err(Rejection::LeaseRequired),
        }
    }

    fn active_attempt_id(&self) -> Option<AttemptId> {
        match &self.attempt {
            AttemptState::Running(attempt)
            | AttemptState::ResultSubmitted { attempt, .. }
            | AttemptState::Verified { attempt, .. } => Some(attempt.id.clone()),
            AttemptState::None | AttemptState::Offered(_) | AttemptState::Failed { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    GrantConsent(ResourceGrant),
    ObserveTime {
        observed_at_ms: u64,
    },
    OpenLease(ResourceLease),
    OfferAssignment(Assignment),
    StartAttempt {
        attempt_id: AttemptId,
        assignment_id: AssignmentId,
    },
    SubmitResult {
        attempt_id: AttemptId,
        result: SubmittedResult,
    },
    VerifyResult {
        attempt_id: AttemptId,
        verifier: PrincipalId,
        capability: VerifierCapability,
        verdict: VerificationVerdict,
    },
    PrepareSettlement {
        settlement_id: SettlementId,
        attempt_id: AttemptId,
        service_credits: u64,
    },
    RecordSettlementReceipt(SettlementReceipt),
    RevokeConsent {
        grant_id: GrantId,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEnvelope {
    pub idempotency_key: IdempotencyKey,
    pub generation: u64,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    ConsentGranted {
        generation: u64,
        grant: ResourceGrant,
    },
    TimeObserved {
        observed_at_ms: u64,
    },
    LeaseOpened(ResourceLease),
    AssignmentOffered(Assignment),
    AttemptStarted(Attempt),
    ResultSubmitted {
        attempt_id: AttemptId,
        result: SubmittedResult,
    },
    ResultVerified(VerificationRecord),
    ResultRejected {
        attempt_id: AttemptId,
        code: String,
    },
    SettlementPrepared(SettlementPlan),
    SettlementReceiptRecorded(SettlementReceipt),
    ConsentRevoked {
        grant_id: GrantId,
        at_ms: u64,
        reason: String,
    },
    ConsentExpired {
        grant_id: GrantId,
        at_ms: u64,
    },
    LeaseStopped {
        lease_id: LeaseId,
        reason: StopReason,
    },
    AttemptFailed {
        assignment_id: AssignmentId,
        attempt_id: Option<AttemptId>,
        reason: AttemptFailure,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxStopReason {
    ResultSubmitted,
    ConsentRevoked,
    ConsentExpired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    StartSandbox {
        effect_id: EffectId,
        node_id: NodeId,
        lease_id: LeaseId,
        assignment_id: AssignmentId,
        attempt_id: AttemptId,
        mode: ExecutionMode,
        resources: ResourceCaps,
        workload_digest: String,
    },
    StopSandbox {
        effect_id: EffectId,
        node_id: NodeId,
        lease_id: Option<LeaseId>,
        attempt_id: Option<AttemptId>,
        reason: SandboxStopReason,
    },
    PostServiceCredit {
        effect_id: EffectId,
        settlement: SettlementPlan,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputProblem {
    MustBePositive,
    MustBeNonEmpty,
    InvalidTimeRange,
    DoesNotMatchRecordedValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalConsent {
    Revoked,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeViolation {
    Workers { requested: u64, allowed: u64 },
    DutyCycle { requested: u64, allowed: u64 },
    MemoryMib { requested: u64, allowed: u64 },
    NetworkBytes { requested: u64, allowed: u64 },
    GpuForbidden,
    BackgroundExecutionForbidden,
    PolicyHashMismatch { expected: String, actual: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizationFailure {
    ProducerIsVerifier,
    CapabilitySubjectMismatch,
    CapabilityNodeMismatch,
    CapabilityAttemptMismatch,
    CapabilityNotYetValid,
    CapabilityExpired,
    ResultProducerMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    ConsentRequired,
    ConsentTerminal {
        state: TerminalConsent,
    },
    LeaseRequired,
    ScopeExceeded(ScopeViolation),
    InvalidTransition {
        state: &'static str,
        command: &'static str,
    },
    InvalidInput {
        field: &'static str,
        problem: InputProblem,
    },
    Unauthorized(AuthorizationFailure),
    Expired {
        expires_at_ms: u64,
        observed_at_ms: u64,
    },
    StaleGeneration {
        expected: u64,
        provided: u64,
    },
    IdempotencyConflict {
        key: IdempotencyKey,
        original_fingerprint: String,
        attempted_fingerprint: String,
    },
    ResultUnverified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedDecision {
    pub events: Vec<Event>,
    /// Empty on an idempotent replay. The original event receipt is retained,
    /// while external effects are not re-dispatched by this in-memory kernel.
    pub effects: Vec<Effect>,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedDecision {
    pub reason: Rejection,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Accepted(AcceptedDecision),
    Rejected(RejectedDecision),
}

impl CommandOutcome {
    pub fn rejection(&self) -> Option<&Rejection> {
        match self {
            Self::Rejected(rejected) => Some(&rejected.reason),
            Self::Accepted(_) => None,
        }
    }

    pub fn effects(&self) -> &[Effect] {
        match self {
            Self::Accepted(accepted) => &accepted.effects,
            Self::Rejected(_) => &[],
        }
    }

    pub fn events(&self) -> &[Event] {
        match self {
            Self::Accepted(accepted) => &accepted.events,
            Self::Rejected(_) => &[],
        }
    }

    pub fn replayed(&self) -> bool {
        match self {
            Self::Accepted(accepted) => accepted.replayed,
            Self::Rejected(rejected) => rejected.replayed,
        }
    }

    fn replay_receipt(&self) -> Self {
        match self {
            Self::Accepted(accepted) => Self::Accepted(AcceptedDecision {
                events: accepted.events.clone(),
                effects: Vec::new(),
                replayed: true,
            }),
            Self::Rejected(rejected) => Self::Rejected(RejectedDecision {
                reason: rejected.reason.clone(),
                replayed: true,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Intent {
    generation: u64,
    command: Command,
}

#[derive(Debug, Clone)]
struct InboxRecord {
    fingerprint: String,
    intent: Intent,
    outcome: CommandOutcome,
}

/// Single-writer in-memory aggregate plus idempotency inbox.
///
/// `handle` is deterministic for a given state, policy, and command envelope.
/// This type does not itself provide thread safety or durability.
#[derive(Debug, Clone)]
pub struct ComputeExchange {
    policy: EnginePolicy,
    aggregate: Aggregate,
    inbox: BTreeMap<IdempotencyKey, InboxRecord>,
}

impl ComputeExchange {
    pub fn new(node_id: NodeId, policy: EnginePolicy) -> Self {
        Self {
            policy,
            aggregate: Aggregate::new(node_id),
            inbox: BTreeMap::new(),
        }
    }

    pub fn browser_alpha_v1(node_id: NodeId) -> Self {
        Self::new(node_id, EnginePolicy::browser_alpha_v1())
    }

    pub fn policy(&self) -> &EnginePolicy {
        &self.policy
    }

    pub fn aggregate(&self) -> &Aggregate {
        &self.aggregate
    }

    pub fn inbox_len(&self) -> usize {
        self.inbox.len()
    }

    pub fn handle(&mut self, envelope: CommandEnvelope) -> CommandOutcome {
        if envelope.idempotency_key.is_empty() {
            return rejected(Rejection::InvalidInput {
                field: "idempotency_key",
                problem: InputProblem::MustBeNonEmpty,
            });
        }
        let intent = Intent {
            generation: envelope.generation,
            command: envelope.command.clone(),
        };
        let fingerprint = fingerprint(&intent);

        if let Some(record) = self.inbox.get(&envelope.idempotency_key) {
            if record.intent == intent {
                return record.outcome.replay_receipt();
            }
            return rejected(Rejection::IdempotencyConflict {
                key: envelope.idempotency_key,
                original_fingerprint: record.fingerprint.clone(),
                attempted_fingerprint: fingerprint,
            });
        }

        let outcome = match self.check_generation(&envelope) {
            Ok(()) => decide(
                &self.aggregate,
                &self.policy,
                &envelope.idempotency_key,
                &envelope.command,
                envelope.generation,
            ),
            Err(reason) => rejected(reason),
        };

        if let CommandOutcome::Accepted(accepted) = &outcome {
            for event in &accepted.events {
                evolve(&mut self.aggregate, event);
            }
        }

        self.inbox.insert(
            envelope.idempotency_key,
            InboxRecord {
                fingerprint,
                intent,
                outcome: outcome.clone(),
            },
        );
        outcome
    }

    fn check_generation(&self, envelope: &CommandEnvelope) -> Result<(), Rejection> {
        let expected = if matches!(envelope.command, Command::GrantConsent(_)) {
            self.aggregate.generation.saturating_add(1)
        } else {
            self.aggregate.generation
        };
        if envelope.generation != expected {
            return Err(Rejection::StaleGeneration {
                expected,
                provided: envelope.generation,
            });
        }
        Ok(())
    }
}

/// Pure decision kernel. Clock, storage, sandboxing, verification signatures,
/// and accounting are recorded inputs or emitted effects.
pub fn decide(
    state: &Aggregate,
    policy: &EnginePolicy,
    key: &IdempotencyKey,
    command: &Command,
    generation: u64,
) -> CommandOutcome {
    let decision = match command {
        Command::GrantConsent(grant) => decide_grant(state, policy, grant, generation),
        Command::ObserveTime { observed_at_ms } => decide_observe_time(state, key, *observed_at_ms),
        Command::OpenLease(lease) => decide_open_lease(state, policy, lease),
        Command::OfferAssignment(assignment) => decide_offer(state, policy, assignment),
        Command::StartAttempt {
            attempt_id,
            assignment_id,
        } => decide_start(state, key, attempt_id, assignment_id),
        Command::SubmitResult { attempt_id, result } => {
            decide_submit_result(state, key, attempt_id, result)
        }
        Command::VerifyResult {
            attempt_id,
            verifier,
            capability,
            verdict,
        } => decide_verify(state, attempt_id, verifier, capability, verdict),
        Command::PrepareSettlement {
            settlement_id,
            attempt_id,
            service_credits,
        } => decide_prepare_settlement(state, key, settlement_id, attempt_id, *service_credits),
        Command::RecordSettlementReceipt(receipt) => decide_record_receipt(state, receipt),
        Command::RevokeConsent { grant_id, reason } => decide_revoke(state, key, grant_id, reason),
    };

    match decision {
        Ok((events, effects)) => accepted(events, effects),
        Err(reason) => rejected(reason),
    }
}

fn decide_grant(
    state: &Aggregate,
    policy: &EnginePolicy,
    grant: &ResourceGrant,
    generation: u64,
) -> Result<(Vec<Event>, Vec<Effect>), Rejection> {
    match state.consent {
        ConsentState::Active { .. } => return Err(invalid(state, "GrantConsent")),
        ConsentState::Unseen | ConsentState::Revoked { .. } | ConsentState::Expired { .. } => {}
    }

    if state.has_outstanding_assignment()
        || matches!(state.settlement, SettlementState::Prepared(_))
    {
        return Err(invalid(state, "GrantConsent"));
    }
    validate_grant(policy, grant)?;
    if grant.issued_at_ms < state.observed_at_ms {
        return Err(Rejection::InvalidInput {
            field: "issued_at_ms",
            problem: InputProblem::InvalidTimeRange,
        });
    }
    Ok((
        vec![Event::ConsentGranted {
            generation,
            grant: grant.clone(),
        }],
        vec![],
    ))
}

fn decide_observe_time(
    state: &Aggregate,
    key: &IdempotencyKey,
    observed_at_ms: u64,
) -> Result<(Vec<Event>, Vec<Effect>), Rejection> {
    if observed_at_ms < state.observed_at_ms {
        return Err(Rejection::InvalidInput {
            field: "observed_at_ms",
            problem: InputProblem::InvalidTimeRange,
        });
    }

    let mut events = vec![Event::TimeObserved { observed_at_ms }];
    let mut effects = vec![];
    if let (ConsentState::Active { grant_id }, Some(grant)) = (&state.consent, &state.grant) {
        if observed_at_ms >= grant.expires_at_ms {
            events.push(Event::ConsentExpired {
                grant_id: grant_id.clone(),
                at_ms: observed_at_ms,
            });
            append_stop_events(state, &mut events, StopReason::ConsentExpired);
            effects.push(stop_sandbox_effect(
                state,
                key,
                SandboxStopReason::ConsentExpired,
            ));
        }
    }
    Ok((events, effects))
}

fn decide_open_lease(
    state: &Aggregate,
    policy: &EnginePolicy,
    lease: &ResourceLease,
) -> Result<(Vec<Event>, Vec<Effect>), Rejection> {
    let grant = state.active_grant()?;
    if !matches!(state.lease, LeaseState::Closed | LeaseState::Stopped { .. }) {
        return Err(invalid(state, "OpenLease"));
    }
    if lease.grant_id != grant.id || lease.generation != state.generation {
        return Err(Rejection::InvalidInput {
            field: "lease.grant_id/generation",
            problem: InputProblem::DoesNotMatchRecordedValue,
        });
    }
    if lease.id.is_empty() {
        return Err(Rejection::InvalidInput {
            field: "lease.id",
            problem: InputProblem::MustBeNonEmpty,
        });
    }
    validate_mode(policy, lease.mode)?;
    lease.resources.validate_nonzero()?;
    lease.resources.require_within(&grant.caps())?;
    Ok((vec![Event::LeaseOpened(lease.clone())], vec![]))
}

fn decide_offer(
    state: &Aggregate,
    policy: &EnginePolicy,
    assignment: &Assignment,
) -> Result<(Vec<Event>, Vec<Effect>), Rejection> {
    state.active_grant()?;
    let lease = state.active_lease()?;
    if state.has_outstanding_assignment() || policy.maximum_outstanding_assignments == 0 {
        return Err(invalid(state, "OfferAssignment"));
    }
    if assignment.lease_id != lease.id || assignment.generation != state.generation {
        return Err(Rejection::InvalidInput {
            field: "assignment.lease_id/generation",
            problem: InputProblem::DoesNotMatchRecordedValue,
        });
    }
    if assignment.id.is_empty()
        || assignment.requester.is_empty()
        || assignment.workload_digest.is_empty()
    {
        return Err(Rejection::InvalidInput {
            field: "assignment.id/requester/workload_digest",
            problem: InputProblem::MustBeNonEmpty,
        });
    }
    validate_mode(policy, assignment.mode)?;
    assignment.resources.validate_nonzero()?;
    assignment.resources.require_within(&lease.resources)?;
    Ok((vec![Event::AssignmentOffered(assignment.clone())], vec![]))
}

fn decide_start(
    state: &Aggregate,
    key: &IdempotencyKey,
    attempt_id: &AttemptId,
    assignment_id: &AssignmentId,
) -> Result<(Vec<Event>, Vec<Effect>), Rejection> {
    let grant = state.active_grant()?;
    let lease = state.active_lease()?;
    let assignment = match &state.attempt {
        AttemptState::Offered(assignment) => assignment,
        _ => return Err(invalid(state, "StartAttempt")),
    };
    if attempt_id.is_empty() || assignment.id != *assignment_id || assignment.lease_id != lease.id {
        return Err(Rejection::InvalidInput {
            field: "attempt_id/assignment_id",
            problem: InputProblem::DoesNotMatchRecordedValue,
        });
    }
    let attempt = Attempt {
        id: attempt_id.clone(),
        assignment_id: assignment_id.clone(),
        producer: grant.principal.clone(),
        generation: state.generation,
        resources: assignment.resources,
        workload_digest: assignment.workload_digest.clone(),
    };
    let effect = Effect::StartSandbox {
        effect_id: effect_id(key, "start-sandbox"),
        node_id: state.node_id.clone(),
        lease_id: lease.id.clone(),
        assignment_id: assignment.id.clone(),
        attempt_id: attempt_id.clone(),
        mode: assignment.mode,
        resources: assignment.resources,
        workload_digest: assignment.workload_digest.clone(),
    };
    Ok((vec![Event::AttemptStarted(attempt)], vec![effect]))
}

fn decide_submit_result(
    state: &Aggregate,
    key: &IdempotencyKey,
    attempt_id: &AttemptId,
    result: &SubmittedResult,
) -> Result<(Vec<Event>, Vec<Effect>), Rejection> {
    let grant = state.active_grant()?;
    let lease = state.active_lease()?;
    let attempt = match &state.attempt {
        AttemptState::Running(attempt) if attempt.id == *attempt_id => attempt,
        _ => return Err(invalid(state, "SubmitResult")),
    };
    if result.producer != grant.principal || result.producer != attempt.producer {
        return Err(Rejection::Unauthorized(
            AuthorizationFailure::ResultProducerMismatch,
        ));
    }
    if result.result_digest.is_empty() {
        return Err(Rejection::InvalidInput {
            field: "result_digest",
            problem: InputProblem::MustBeNonEmpty,
        });
    }
    result.usage.validate_nonzero()?;
    result.usage.require_within(&attempt.resources)?;
    Ok((
        vec![Event::ResultSubmitted {
            attempt_id: attempt_id.clone(),
            result: result.clone(),
        }],
        vec![Effect::StopSandbox {
            effect_id: effect_id(key, "stop-sandbox-result-submitted"),
            node_id: state.node_id.clone(),
            lease_id: Some(lease.id.clone()),
            attempt_id: Some(attempt_id.clone()),
            reason: SandboxStopReason::ResultSubmitted,
        }],
    ))
}

fn decide_verify(
    state: &Aggregate,
    attempt_id: &AttemptId,
    verifier: &PrincipalId,
    capability: &VerifierCapability,
    verdict: &VerificationVerdict,
) -> Result<(Vec<Event>, Vec<Effect>), Rejection> {
    let (attempt, result) = match &state.attempt {
        AttemptState::ResultSubmitted { attempt, result } if attempt.id == *attempt_id => {
            (attempt, result)
        }
        _ => return Err(invalid(state, "VerifyResult")),
    };
    if verifier.is_empty() || capability.id.is_empty() {
        return Err(Rejection::InvalidInput {
            field: "verifier/capability.id",
            problem: InputProblem::MustBeNonEmpty,
        });
    }
    if capability.not_before_ms >= capability.expires_at_ms {
        return Err(Rejection::InvalidInput {
            field: "capability.not_before_ms/expires_at_ms",
            problem: InputProblem::InvalidTimeRange,
        });
    }
    if *verifier == result.producer {
        return Err(Rejection::Unauthorized(
            AuthorizationFailure::ProducerIsVerifier,
        ));
    }
    if capability.subject != *verifier {
        return Err(Rejection::Unauthorized(
            AuthorizationFailure::CapabilitySubjectMismatch,
        ));
    }
    if capability.node_id != state.node_id {
        return Err(Rejection::Unauthorized(
            AuthorizationFailure::CapabilityNodeMismatch,
        ));
    }
    if capability.attempt_id != attempt.id {
        return Err(Rejection::Unauthorized(
            AuthorizationFailure::CapabilityAttemptMismatch,
        ));
    }
    if state.observed_at_ms < capability.not_before_ms {
        return Err(Rejection::Unauthorized(
            AuthorizationFailure::CapabilityNotYetValid,
        ));
    }
    if state.observed_at_ms >= capability.expires_at_ms {
        return Err(Rejection::Unauthorized(
            AuthorizationFailure::CapabilityExpired,
        ));
    }

    match verdict {
        VerificationVerdict::Accept => Ok((
            vec![Event::ResultVerified(VerificationRecord {
                attempt_id: attempt.id.clone(),
                verifier: verifier.clone(),
                capability_id: capability.id.clone(),
                result_digest: result.result_digest.clone(),
                verified_at_ms: state.observed_at_ms,
            })],
            vec![],
        )),
        VerificationVerdict::Reject { code } => {
            if code.is_empty() {
                return Err(Rejection::InvalidInput {
                    field: "verification_rejection_code",
                    problem: InputProblem::MustBeNonEmpty,
                });
            }
            Ok((
                vec![Event::ResultRejected {
                    attempt_id: attempt.id.clone(),
                    code: code.clone(),
                }],
                vec![],
            ))
        }
    }
}

fn decide_prepare_settlement(
    state: &Aggregate,
    key: &IdempotencyKey,
    settlement_id: &SettlementId,
    attempt_id: &AttemptId,
    service_credits: u64,
) -> Result<(Vec<Event>, Vec<Effect>), Rejection> {
    let (attempt, result) = match &state.attempt {
        AttemptState::Verified {
            attempt, result, ..
        } if attempt.id == *attempt_id => (attempt, result),
        AttemptState::ResultSubmitted { .. }
        | AttemptState::Running(_)
        | AttemptState::Offered(_) => return Err(Rejection::ResultUnverified),
        _ => return Err(invalid(state, "PrepareSettlement")),
    };
    if !matches!(state.settlement, SettlementState::None) {
        return Err(invalid(state, "PrepareSettlement"));
    }
    if settlement_id.is_empty() || service_credits == 0 {
        return Err(Rejection::InvalidInput {
            field: "settlement_id/service_credits",
            problem: InputProblem::MustBePositive,
        });
    }
    let beneficiary = state
        .grant
        .as_ref()
        .map(|grant| grant.principal.clone())
        .ok_or(Rejection::ConsentRequired)?;
    let plan = SettlementPlan {
        id: settlement_id.clone(),
        attempt_id: attempt.id.clone(),
        beneficiary,
        service_credits,
        verified_result_digest: result.result_digest.clone(),
    };
    Ok((
        vec![Event::SettlementPrepared(plan.clone())],
        vec![Effect::PostServiceCredit {
            effect_id: effect_id(key, "post-service-credit"),
            settlement: plan,
        }],
    ))
}

fn decide_record_receipt(
    state: &Aggregate,
    receipt: &SettlementReceipt,
) -> Result<(Vec<Event>, Vec<Effect>), Rejection> {
    let plan = match &state.settlement {
        SettlementState::Prepared(plan) => plan,
        _ => return Err(invalid(state, "RecordSettlementReceipt")),
    };
    if receipt.settlement_id != plan.id
        || receipt.service_credits != plan.service_credits
        || receipt.external_reference.is_empty()
    {
        return Err(Rejection::InvalidInput {
            field: "settlement_receipt",
            problem: InputProblem::DoesNotMatchRecordedValue,
        });
    }
    Ok((
        vec![Event::SettlementReceiptRecorded(receipt.clone())],
        vec![],
    ))
}

fn decide_revoke(
    state: &Aggregate,
    key: &IdempotencyKey,
    grant_id: &GrantId,
    reason: &str,
) -> Result<(Vec<Event>, Vec<Effect>), Rejection> {
    let grant = state.active_grant()?;
    if *grant_id != grant.id {
        return Err(Rejection::InvalidInput {
            field: "grant_id",
            problem: InputProblem::DoesNotMatchRecordedValue,
        });
    }
    if reason.is_empty() {
        return Err(Rejection::InvalidInput {
            field: "reason",
            problem: InputProblem::MustBeNonEmpty,
        });
    }
    let mut events = vec![Event::ConsentRevoked {
        grant_id: grant_id.clone(),
        at_ms: state.observed_at_ms,
        reason: reason.to_owned(),
    }];
    append_stop_events(state, &mut events, StopReason::ConsentRevoked);
    Ok((
        events,
        vec![stop_sandbox_effect(
            state,
            key,
            SandboxStopReason::ConsentRevoked,
        )],
    ))
}

fn validate_grant(policy: &EnginePolicy, grant: &ResourceGrant) -> Result<(), Rejection> {
    if grant.id.is_empty() || grant.principal.is_empty() || grant.policy_hash.is_empty() {
        return Err(Rejection::InvalidInput {
            field: "grant.id/principal/policy_hash",
            problem: InputProblem::MustBeNonEmpty,
        });
    }
    if grant.issued_at_ms >= grant.expires_at_ms {
        return Err(Rejection::InvalidInput {
            field: "grant.issued_at_ms/expires_at_ms",
            problem: InputProblem::InvalidTimeRange,
        });
    }
    if grant.policy_hash != policy.policy_hash {
        return Err(Rejection::ScopeExceeded(
            ScopeViolation::PolicyHashMismatch {
                expected: policy.policy_hash.clone(),
                actual: grant.policy_hash.clone(),
            },
        ));
    }
    if policy.browser_foreground_only && !grant.browser_foreground_only {
        return Err(Rejection::ScopeExceeded(
            ScopeViolation::BackgroundExecutionForbidden,
        ));
    }
    grant.caps().validate_nonzero()?;
    grant.caps().require_within(&policy.maximum)
}

fn validate_mode(policy: &EnginePolicy, mode: ExecutionMode) -> Result<(), Rejection> {
    if policy.browser_foreground_only && mode != ExecutionMode::BrowserForeground {
        return Err(Rejection::ScopeExceeded(
            ScopeViolation::BackgroundExecutionForbidden,
        ));
    }
    Ok(())
}

fn append_stop_events(state: &Aggregate, events: &mut Vec<Event>, reason: StopReason) {
    if let LeaseState::Active(lease) = &state.lease {
        events.push(Event::LeaseStopped {
            lease_id: lease.id.clone(),
            reason,
        });
    }
    match &state.attempt {
        AttemptState::Offered(assignment) => events.push(Event::AttemptFailed {
            assignment_id: assignment.id.clone(),
            attempt_id: None,
            reason: match reason {
                StopReason::ConsentRevoked => AttemptFailure::ConsentRevoked,
                StopReason::ConsentExpired => AttemptFailure::ConsentExpired,
            },
        }),
        AttemptState::Running(attempt) => events.push(Event::AttemptFailed {
            assignment_id: attempt.assignment_id.clone(),
            attempt_id: Some(attempt.id.clone()),
            reason: match reason {
                StopReason::ConsentRevoked => AttemptFailure::ConsentRevoked,
                StopReason::ConsentExpired => AttemptFailure::ConsentExpired,
            },
        }),
        AttemptState::None
        | AttemptState::ResultSubmitted { .. }
        | AttemptState::Verified { .. }
        | AttemptState::Failed { .. } => {}
    }
}

fn stop_sandbox_effect(
    state: &Aggregate,
    key: &IdempotencyKey,
    reason: SandboxStopReason,
) -> Effect {
    let lease_id = match &state.lease {
        LeaseState::Active(lease) => Some(lease.id.clone()),
        LeaseState::Closed | LeaseState::Stopped { .. } => None,
    };
    Effect::StopSandbox {
        effect_id: effect_id(key, "stop-sandbox"),
        node_id: state.node_id.clone(),
        lease_id,
        attempt_id: state.active_attempt_id(),
        reason,
    }
}

fn effect_id(key: &IdempotencyKey, suffix: &str) -> EffectId {
    EffectId(format!("{}:{suffix}", key.0))
}

fn accepted(events: Vec<Event>, effects: Vec<Effect>) -> CommandOutcome {
    CommandOutcome::Accepted(AcceptedDecision {
        events,
        effects,
        replayed: false,
    })
}

fn rejected(reason: Rejection) -> CommandOutcome {
    CommandOutcome::Rejected(RejectedDecision {
        reason,
        replayed: false,
    })
}

fn invalid(state: &Aggregate, command: &'static str) -> Rejection {
    Rejection::InvalidTransition {
        state: state_name(state),
        command,
    }
}

fn state_name(state: &Aggregate) -> &'static str {
    match (
        &state.consent,
        &state.lease,
        &state.attempt,
        &state.settlement,
    ) {
        (ConsentState::Unseen, _, _, _) => "consent:unseen",
        (ConsentState::Revoked { .. }, _, _, _) => "consent:revoked",
        (ConsentState::Expired { .. }, _, _, _) => "consent:expired",
        (_, LeaseState::Closed, _, _) => "lease:closed",
        (_, LeaseState::Stopped { .. }, _, _) => "lease:stopped",
        (_, _, AttemptState::None, _) => "attempt:none",
        (_, _, AttemptState::Offered(_), _) => "attempt:offered",
        (_, _, AttemptState::Running(_), _) => "attempt:running",
        (_, _, AttemptState::ResultSubmitted { .. }, _) => "attempt:result-submitted",
        (_, _, AttemptState::Verified { .. }, SettlementState::None) => "attempt:verified",
        (_, _, AttemptState::Verified { .. }, SettlementState::Prepared(_)) => {
            "settlement:prepared"
        }
        (_, _, AttemptState::Verified { .. }, SettlementState::Posted { .. }) => {
            "settlement:posted"
        }
        (_, _, AttemptState::Failed { .. }, _) => "attempt:failed",
    }
}

/// Apply one accepted fact to the aggregate. This reducer performs no I/O and
/// is intentionally public so event-replay adapters can conformance-test it.
pub fn evolve(state: &mut Aggregate, event: &Event) {
    match event {
        Event::ConsentGranted { generation, grant } => {
            state.generation = *generation;
            state.observed_at_ms = grant.issued_at_ms;
            state.consent = ConsentState::Active {
                grant_id: grant.id.clone(),
            };
            state.grant = Some(grant.clone());
            state.lease = LeaseState::Closed;
            state.attempt = AttemptState::None;
            state.settlement = SettlementState::None;
        }
        Event::TimeObserved { observed_at_ms } => state.observed_at_ms = *observed_at_ms,
        Event::LeaseOpened(lease) => state.lease = LeaseState::Active(lease.clone()),
        Event::AssignmentOffered(assignment) => {
            state.attempt = AttemptState::Offered(assignment.clone());
            state.settlement = SettlementState::None;
        }
        Event::AttemptStarted(attempt) => state.attempt = AttemptState::Running(attempt.clone()),
        Event::ResultSubmitted { attempt_id, result } => {
            if let AttemptState::Running(attempt) = &state.attempt {
                debug_assert_eq!(&attempt.id, attempt_id);
                state.attempt = AttemptState::ResultSubmitted {
                    attempt: attempt.clone(),
                    result: result.clone(),
                };
            }
        }
        Event::ResultVerified(verification) => {
            if let AttemptState::ResultSubmitted { attempt, result } = &state.attempt {
                debug_assert_eq!(attempt.id, verification.attempt_id);
                state.attempt = AttemptState::Verified {
                    attempt: attempt.clone(),
                    result: result.clone(),
                    verification: verification.clone(),
                };
            }
        }
        Event::ResultRejected { attempt_id, code } => {
            if let AttemptState::ResultSubmitted { attempt, .. } = &state.attempt {
                debug_assert_eq!(&attempt.id, attempt_id);
                state.attempt = AttemptState::Failed {
                    assignment_id: attempt.assignment_id.clone(),
                    attempt_id: Some(attempt.id.clone()),
                    reason: AttemptFailure::VerificationRejected { code: code.clone() },
                };
            }
        }
        Event::SettlementPrepared(plan) => {
            state.settlement = SettlementState::Prepared(plan.clone())
        }
        Event::SettlementReceiptRecorded(receipt) => {
            if let SettlementState::Prepared(plan) = &state.settlement {
                state.settlement = SettlementState::Posted {
                    plan: plan.clone(),
                    receipt: receipt.clone(),
                };
            }
        }
        Event::ConsentRevoked {
            grant_id, at_ms, ..
        } => {
            state.consent = ConsentState::Revoked {
                grant_id: grant_id.clone(),
                at_ms: *at_ms,
            }
        }
        Event::ConsentExpired { grant_id, at_ms } => {
            state.consent = ConsentState::Expired {
                grant_id: grant_id.clone(),
                at_ms: *at_ms,
            }
        }
        Event::LeaseStopped { lease_id, reason } => {
            state.lease = LeaseState::Stopped {
                lease_id: lease_id.clone(),
                reason: *reason,
            }
        }
        Event::AttemptFailed {
            assignment_id,
            attempt_id,
            reason,
        } => {
            state.attempt = AttemptState::Failed {
                assignment_id: assignment_id.clone(),
                attempt_id: attempt_id.clone(),
                reason: reason.clone(),
            }
        }
    }
}

fn fingerprint(intent: &Intent) -> String {
    // A versioned, deterministic in-process fingerprint. The full intent is
    // retained and compared too, so an FNV collision cannot turn different
    // commands into a successful replay. This is not a cryptographic wire hash.
    let material = format!("v1\0{}\0{:?}", intent.generation, intent.command);
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in material.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mut out = String::from("v1-fnv64-");
    write!(&mut out, "{hash:016x}").expect("writing to String cannot fail");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps() -> ResourceCaps {
        ResourceCaps {
            workers: 2,
            duty_cycle_percent: 25,
            memory_mib: 128,
            network_bytes: 0,
            gpu: false,
        }
    }

    fn grant() -> ResourceGrant {
        ResourceGrant {
            id: "grant-1".into(),
            principal: "contributor".into(),
            issued_at_ms: 100,
            expires_at_ms: 1_000,
            browser_foreground_only: true,
            max_workers: 2,
            duty_cycle_percent: 25,
            memory_mib: 128,
            network_bytes: 0,
            gpu_allowed: false,
            policy_hash: BROWSER_ALPHA_V1_POLICY_HASH.to_owned(),
        }
    }

    fn lease() -> ResourceLease {
        ResourceLease {
            id: "lease-1".into(),
            grant_id: "grant-1".into(),
            generation: 1,
            mode: ExecutionMode::BrowserForeground,
            resources: caps(),
        }
    }

    fn assignment() -> Assignment {
        Assignment {
            id: "assignment-1".into(),
            lease_id: "lease-1".into(),
            generation: 1,
            requester: "metahumotonic-saas".into(),
            workload_digest: "sha256:workload".to_owned(),
            mode: ExecutionMode::BrowserForeground,
            resources: caps(),
        }
    }

    fn result() -> SubmittedResult {
        SubmittedResult {
            producer: "contributor".into(),
            result_digest: "sha256:result".to_owned(),
            usage: caps(),
            compute_ms: 200,
        }
    }

    fn capability(subject: &str) -> VerifierCapability {
        VerifierCapability {
            id: "cap-1".into(),
            subject: subject.into(),
            node_id: "node-1".into(),
            attempt_id: "attempt-1".into(),
            not_before_ms: 100,
            expires_at_ms: 900,
        }
    }

    fn env(key: &str, generation: u64, command: Command) -> CommandEnvelope {
        CommandEnvelope {
            idempotency_key: key.into(),
            generation,
            command,
        }
    }

    fn new_engine() -> ComputeExchange {
        ComputeExchange::browser_alpha_v1("node-1".into())
    }

    fn accept(engine: &mut ComputeExchange, key: &str, command: Command) -> CommandOutcome {
        let generation = if matches!(command, Command::GrantConsent(_)) {
            engine.aggregate().generation + 1
        } else {
            engine.aggregate().generation
        };
        let outcome = engine.handle(env(key, generation, command));
        assert!(
            matches!(outcome, CommandOutcome::Accepted(_)),
            "{outcome:?}"
        );
        outcome
    }

    fn grant_lease_offer(engine: &mut ComputeExchange) {
        accept(engine, "grant", Command::GrantConsent(grant()));
        accept(engine, "lease", Command::OpenLease(lease()));
        accept(engine, "offer", Command::OfferAssignment(assignment()));
    }

    fn running(engine: &mut ComputeExchange) {
        grant_lease_offer(engine);
        accept(
            engine,
            "start",
            Command::StartAttempt {
                attempt_id: "attempt-1".into(),
                assignment_id: "assignment-1".into(),
            },
        );
    }

    fn submitted(engine: &mut ComputeExchange) {
        running(engine);
        accept(
            engine,
            "submit",
            Command::SubmitResult {
                attempt_id: "attempt-1".into(),
                result: result(),
            },
        );
    }

    fn verified(engine: &mut ComputeExchange) {
        submitted(engine);
        accept(
            engine,
            "verify",
            Command::VerifyResult {
                attempt_id: "attempt-1".into(),
                verifier: "independent-verifier".into(),
                capability: capability("independent-verifier"),
                verdict: VerificationVerdict::Accept,
            },
        );
    }

    fn assert_rejection(outcome: CommandOutcome, expected: impl FnOnce(&Rejection) -> bool) {
        match outcome {
            CommandOutcome::Rejected(rejected) => {
                assert!(
                    expected(&rejected.reason),
                    "unexpected: {:?}",
                    rejected.reason
                )
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn page_visit_or_time_observation_cannot_start_sandbox() {
        let mut engine = new_engine();
        assert!(matches!(engine.aggregate().consent, ConsentState::Unseen));
        let outcome = engine.handle(env(
            "time-before-consent",
            0,
            Command::ObserveTime { observed_at_ms: 50 },
        ));
        assert!(outcome.effects().is_empty());
        assert!(!outcome
            .effects()
            .iter()
            .any(|effect| matches!(effect, Effect::StartSandbox { .. })));
        assert!(matches!(engine.aggregate().consent, ConsentState::Unseen));
    }

    #[test]
    fn opening_lease_without_resource_grant_is_rejected() {
        let mut engine = new_engine();
        let outcome = engine.handle(env("lease", 0, Command::OpenLease(lease())));
        assert_rejection(outcome, |reason| {
            matches!(reason, Rejection::ConsentRequired)
        });
    }

    #[test]
    fn empty_idempotency_key_is_rejected_without_inbox_claim() {
        let mut engine = new_engine();
        let outcome = engine.handle(env("", 1, Command::GrantConsent(grant())));
        assert_rejection(outcome, |reason| {
            matches!(
                reason,
                Rejection::InvalidInput {
                    field: "idempotency_key",
                    problem: InputProblem::MustBeNonEmpty
                }
            )
        });
        assert_eq!(engine.inbox_len(), 0);
    }

    #[test]
    fn lease_and_assignment_ids_must_be_non_empty() {
        let mut engine = new_engine();
        accept(&mut engine, "grant", Command::GrantConsent(grant()));
        let mut empty_lease = lease();
        empty_lease.id = "".into();
        let outcome = engine.handle(env("empty-lease", 1, Command::OpenLease(empty_lease)));
        assert_rejection(outcome, |reason| {
            matches!(
                reason,
                Rejection::InvalidInput {
                    field: "lease.id",
                    problem: InputProblem::MustBeNonEmpty
                }
            )
        });

        accept(&mut engine, "lease", Command::OpenLease(lease()));
        let mut empty_assignment = assignment();
        empty_assignment.id = "".into();
        let outcome = engine.handle(env(
            "empty-assignment",
            1,
            Command::OfferAssignment(empty_assignment),
        ));
        assert_rejection(outcome, |reason| {
            matches!(
                reason,
                Rejection::InvalidInput {
                    field: "assignment.id/requester/workload_digest",
                    problem: InputProblem::MustBeNonEmpty
                }
            )
        });
    }

    #[test]
    fn grant_above_v1_cap_is_rejected() {
        let mut engine = new_engine();
        let mut over = grant();
        over.duty_cycle_percent = 51;
        let outcome = engine.handle(env("grant-over", 1, Command::GrantConsent(over)));
        assert_rejection(outcome, |reason| {
            matches!(
                reason,
                Rejection::ScopeExceeded(ScopeViolation::DutyCycle {
                    requested: 51,
                    allowed: 50
                })
            )
        });
        assert!(matches!(engine.aggregate().consent, ConsentState::Unseen));
    }

    #[test]
    fn browser_background_native_network_and_gpu_are_rejected() {
        let mut cases = Vec::new();
        let mut background = lease();
        background.mode = ExecutionMode::BrowserBackground;
        cases.push(("background", background));
        let mut native = lease();
        native.mode = ExecutionMode::NativeNode;
        cases.push(("native", native));
        let mut network = lease();
        network.resources.network_bytes = 1;
        cases.push(("network", network));
        let mut gpu = lease();
        gpu.resources.gpu = true;
        cases.push(("gpu", gpu));

        for (name, candidate) in cases {
            let mut engine = new_engine();
            accept(
                &mut engine,
                &format!("grant-{name}"),
                Command::GrantConsent(grant()),
            );
            let outcome = engine.handle(env(
                &format!("lease-{name}"),
                1,
                Command::OpenLease(candidate),
            ));
            assert_rejection(outcome, |reason| {
                matches!(reason, Rejection::ScopeExceeded(_))
            });
        }
    }

    #[test]
    fn start_without_lease_is_rejected() {
        let mut engine = new_engine();
        accept(&mut engine, "grant", Command::GrantConsent(grant()));
        let outcome = engine.handle(env(
            "start",
            1,
            Command::StartAttempt {
                attempt_id: "attempt-1".into(),
                assignment_id: "assignment-1".into(),
            },
        ));
        assert_rejection(outcome, |reason| matches!(reason, Rejection::LeaseRequired));
    }

    #[test]
    fn start_effect_requires_consent_lease_and_offered_attempt() {
        let mut engine = new_engine();
        grant_lease_offer(&mut engine);
        let outcome = accept(
            &mut engine,
            "start",
            Command::StartAttempt {
                attempt_id: "attempt-1".into(),
                assignment_id: "assignment-1".into(),
            },
        );
        assert_eq!(outcome.effects().len(), 1);
        assert!(matches!(outcome.effects()[0], Effect::StartSandbox { .. }));
        assert!(matches!(
            engine.aggregate().attempt,
            AttemptState::Running(_)
        ));
    }

    #[test]
    fn result_usage_above_assignment_cap_is_rejected() {
        let mut engine = new_engine();
        running(&mut engine);
        let mut over = result();
        over.usage.memory_mib = 129;
        let outcome = engine.handle(env(
            "submit-over",
            1,
            Command::SubmitResult {
                attempt_id: "attempt-1".into(),
                result: over,
            },
        ));
        assert_rejection(outcome, |reason| {
            matches!(
                reason,
                Rejection::ScopeExceeded(ScopeViolation::MemoryMib { .. })
            )
        });
        assert!(matches!(
            engine.aggregate().attempt,
            AttemptState::Running(_)
        ));
    }

    #[test]
    fn assignment_subset_remains_the_attempt_usage_ceiling() {
        let mut engine = new_engine();
        accept(&mut engine, "grant", Command::GrantConsent(grant()));
        accept(&mut engine, "lease", Command::OpenLease(lease()));
        let mut narrow = assignment();
        narrow.resources.memory_mib = 64;
        accept(
            &mut engine,
            "narrow-offer",
            Command::OfferAssignment(narrow),
        );
        accept(
            &mut engine,
            "narrow-start",
            Command::StartAttempt {
                attempt_id: "attempt-1".into(),
                assignment_id: "assignment-1".into(),
            },
        );
        let mut over_assignment_but_under_lease = result();
        over_assignment_but_under_lease.usage.memory_mib = 65;
        let outcome = engine.handle(env(
            "narrow-submit",
            1,
            Command::SubmitResult {
                attempt_id: "attempt-1".into(),
                result: over_assignment_but_under_lease,
            },
        ));
        assert_rejection(outcome, |reason| {
            matches!(
                reason,
                Rejection::ScopeExceeded(ScopeViolation::MemoryMib {
                    requested: 65,
                    allowed: 64
                })
            )
        });
    }

    #[test]
    fn submit_result_stops_sandbox_but_does_not_settle() {
        let mut engine = new_engine();
        running(&mut engine);
        let outcome = accept(
            &mut engine,
            "submit",
            Command::SubmitResult {
                attempt_id: "attempt-1".into(),
                result: result(),
            },
        );
        assert!(matches!(
            outcome.effects(),
            [Effect::StopSandbox {
                reason: SandboxStopReason::ResultSubmitted,
                ..
            }]
        ));
        assert!(!outcome
            .effects()
            .iter()
            .any(|effect| matches!(effect, Effect::PostServiceCredit { .. })));
    }

    #[test]
    fn result_producer_cannot_verify_own_result() {
        let mut engine = new_engine();
        submitted(&mut engine);
        let outcome = engine.handle(env(
            "verify-self",
            1,
            Command::VerifyResult {
                attempt_id: "attempt-1".into(),
                verifier: "contributor".into(),
                capability: capability("contributor"),
                verdict: VerificationVerdict::Accept,
            },
        ));
        assert_rejection(outcome, |reason| {
            matches!(
                reason,
                Rejection::Unauthorized(AuthorizationFailure::ProducerIsVerifier)
            )
        });
    }

    #[test]
    fn verifier_capability_scope_and_expiry_are_enforced() {
        let mut cases = Vec::new();
        let mut wrong_subject = capability("someone-else");
        wrong_subject.expires_at_ms = 900;
        cases.push(("subject", wrong_subject));
        let mut wrong_node = capability("independent-verifier");
        wrong_node.node_id = "node-2".into();
        cases.push(("node", wrong_node));
        let mut wrong_attempt = capability("independent-verifier");
        wrong_attempt.attempt_id = "attempt-2".into();
        cases.push(("attempt", wrong_attempt));
        let mut expired = capability("independent-verifier");
        expired.not_before_ms = 0;
        expired.expires_at_ms = 100;
        cases.push(("expired", expired));

        for (name, candidate) in cases {
            let mut engine = new_engine();
            submitted(&mut engine);
            let outcome = engine.handle(env(
                &format!("verify-{name}"),
                1,
                Command::VerifyResult {
                    attempt_id: "attempt-1".into(),
                    verifier: "independent-verifier".into(),
                    capability: candidate,
                    verdict: VerificationVerdict::Accept,
                },
            ));
            assert_rejection(outcome, |reason| {
                matches!(reason, Rejection::Unauthorized(_))
            });
        }
    }

    #[test]
    fn verifier_and_capability_identity_and_time_range_are_required() {
        enum Mutation {
            EmptyVerifier,
            EmptyCapability,
            ReversedTime,
        }
        for (name, mutation) in [
            ("empty-verifier", Mutation::EmptyVerifier),
            ("empty-capability", Mutation::EmptyCapability),
            ("reversed-time", Mutation::ReversedTime),
        ] {
            let mut engine = new_engine();
            submitted(&mut engine);
            let mut verifier: PrincipalId = "independent-verifier".into();
            let mut candidate = capability("independent-verifier");
            match mutation {
                Mutation::EmptyVerifier => {
                    verifier = "".into();
                    candidate.subject = "".into();
                }
                Mutation::EmptyCapability => candidate.id = "".into(),
                Mutation::ReversedTime => {
                    candidate.not_before_ms = 900;
                    candidate.expires_at_ms = 900;
                }
            }
            let outcome = engine.handle(env(
                name,
                1,
                Command::VerifyResult {
                    attempt_id: "attempt-1".into(),
                    verifier,
                    capability: candidate,
                    verdict: VerificationVerdict::Accept,
                },
            ));
            assert_rejection(outcome, |reason| {
                matches!(reason, Rejection::InvalidInput { .. })
            });
        }
    }

    #[test]
    fn settlement_before_verification_is_rejected() {
        let mut engine = new_engine();
        submitted(&mut engine);
        let outcome = engine.handle(env(
            "settle-too-soon",
            1,
            Command::PrepareSettlement {
                settlement_id: "settlement-1".into(),
                attempt_id: "attempt-1".into(),
                service_credits: 10,
            },
        ));
        assert_rejection(outcome, |reason| {
            matches!(reason, Rejection::ResultUnverified)
        });
    }

    #[test]
    fn verified_result_can_prepare_and_record_service_credit() {
        let mut engine = new_engine();
        verified(&mut engine);
        let prepare = accept(
            &mut engine,
            "prepare",
            Command::PrepareSettlement {
                settlement_id: "settlement-1".into(),
                attempt_id: "attempt-1".into(),
                service_credits: 10,
            },
        );
        assert!(matches!(
            prepare.effects(),
            [Effect::PostServiceCredit { .. }]
        ));
        accept(
            &mut engine,
            "receipt",
            Command::RecordSettlementReceipt(SettlementReceipt {
                settlement_id: "settlement-1".into(),
                service_credits: 10,
                external_reference: "ledger-receipt-1".to_owned(),
            }),
        );
        assert!(matches!(
            engine.aggregate().settlement,
            SettlementState::Posted { .. }
        ));
    }

    #[test]
    fn revocation_emits_stop_and_blocks_new_work() {
        let mut engine = new_engine();
        running(&mut engine);
        let outcome = accept(
            &mut engine,
            "revoke",
            Command::RevokeConsent {
                grant_id: "grant-1".into(),
                reason: "user clicked stop".to_owned(),
            },
        );
        assert!(matches!(
            outcome.effects(),
            [Effect::StopSandbox {
                reason: SandboxStopReason::ConsentRevoked,
                ..
            }]
        ));
        assert!(matches!(
            engine.aggregate().consent,
            ConsentState::Revoked { .. }
        ));
        assert!(matches!(
            engine.aggregate().lease,
            LeaseState::Stopped { .. }
        ));
        assert!(matches!(
            engine.aggregate().attempt,
            AttemptState::Failed { .. }
        ));

        let outcome = engine.handle(env(
            "offer-after-revoke",
            1,
            Command::OfferAssignment(assignment()),
        ));
        assert_rejection(outcome, |reason| {
            matches!(
                reason,
                Rejection::ConsentTerminal {
                    state: TerminalConsent::Revoked
                }
            )
        });
    }

    #[test]
    fn observed_expiry_emits_stop_and_blocks_new_work() {
        let mut engine = new_engine();
        running(&mut engine);
        let outcome = accept(
            &mut engine,
            "expire",
            Command::ObserveTime {
                observed_at_ms: 1_000,
            },
        );
        assert!(matches!(
            outcome.effects(),
            [Effect::StopSandbox {
                reason: SandboxStopReason::ConsentExpired,
                ..
            }]
        ));
        assert!(matches!(
            engine.aggregate().consent,
            ConsentState::Expired { .. }
        ));
        let outcome = engine.handle(env("lease-after-expiry", 1, Command::OpenLease(lease())));
        assert_rejection(outcome, |reason| {
            matches!(
                reason,
                Rejection::ConsentTerminal {
                    state: TerminalConsent::Expired
                }
            )
        });
    }

    #[test]
    fn expiry_is_not_inferred_without_observed_time() {
        let mut engine = new_engine();
        grant_lease_offer(&mut engine);
        assert_eq!(engine.aggregate().observed_at_ms, 100);
        assert!(matches!(
            engine.aggregate().consent,
            ConsentState::Active { .. }
        ));
    }

    #[test]
    fn stale_generation_is_rejected_before_transition() {
        let mut engine = new_engine();
        accept(&mut engine, "grant", Command::GrantConsent(grant()));
        let outcome = engine.handle(env("stale-lease", 0, Command::OpenLease(lease())));
        assert_rejection(outcome, |reason| {
            matches!(
                reason,
                Rejection::StaleGeneration {
                    expected: 1,
                    provided: 0
                }
            )
        });
    }

    #[test]
    fn same_key_same_intent_replays_without_effect_redelivery() {
        let mut engine = new_engine();
        grant_lease_offer(&mut engine);
        let envelope = env(
            "start-once",
            1,
            Command::StartAttempt {
                attempt_id: "attempt-1".into(),
                assignment_id: "assignment-1".into(),
            },
        );
        let first = engine.handle(envelope.clone());
        assert_eq!(first.effects().len(), 1);
        let replay = engine.handle(envelope);
        assert!(replay.replayed());
        assert!(replay.effects().is_empty());
        assert_eq!(first.events(), replay.events());
    }

    #[test]
    fn same_key_different_intent_is_conflict() {
        let mut engine = new_engine();
        let first = env("same", 1, Command::GrantConsent(grant()));
        assert!(matches!(engine.handle(first), CommandOutcome::Accepted(_)));
        let outcome = engine.handle(env(
            "same",
            1,
            Command::ObserveTime {
                observed_at_ms: 200,
            },
        ));
        assert_rejection(outcome, |reason| {
            matches!(reason, Rejection::IdempotencyConflict { .. })
        });
    }

    #[test]
    fn duplicate_settlement_command_does_not_post_twice() {
        let mut engine = new_engine();
        verified(&mut engine);
        let prepare = env(
            "prepare-once",
            1,
            Command::PrepareSettlement {
                settlement_id: "settlement-1".into(),
                attempt_id: "attempt-1".into(),
                service_credits: 10,
            },
        );
        let first = engine.handle(prepare.clone());
        assert!(matches!(
            first.effects(),
            [Effect::PostServiceCredit { .. }]
        ));
        let replay = engine.handle(prepare);
        assert!(replay.replayed());
        assert!(replay.effects().is_empty());
        assert!(matches!(
            engine.aggregate().settlement,
            SettlementState::Prepared(_)
        ));

        let second_key = engine.handle(env(
            "prepare-different-key",
            1,
            Command::PrepareSettlement {
                settlement_id: "settlement-1".into(),
                attempt_id: "attempt-1".into(),
                service_credits: 10,
            },
        ));
        assert_rejection(second_key, |reason| {
            matches!(reason, Rejection::InvalidTransition { .. })
        });
    }

    #[test]
    fn rejected_decision_is_also_idempotent() {
        let mut engine = new_engine();
        let envelope = env("no-consent", 0, Command::OpenLease(lease()));
        let first = engine.handle(envelope.clone());
        let replay = engine.handle(envelope);
        assert_eq!(first.rejection(), replay.rejection());
        assert!(replay.replayed());
        assert_eq!(engine.inbox_len(), 1);
    }

    #[test]
    fn only_one_outstanding_assignment_is_admitted() {
        let mut engine = new_engine();
        grant_lease_offer(&mut engine);
        let mut second = assignment();
        second.id = "assignment-2".into();
        let outcome = engine.handle(env("second-offer", 1, Command::OfferAssignment(second)));
        assert_rejection(outcome, |reason| {
            matches!(reason, Rejection::InvalidTransition { .. })
        });
    }

    #[test]
    fn negative_transition_table_is_fail_closed() {
        struct Case {
            name: &'static str,
            setup: fn(&mut ComputeExchange),
            command: Command,
            expected: fn(&Rejection) -> bool,
        }

        let cases = [
            Case {
                name: "submit-before-start",
                setup: grant_lease_offer,
                command: Command::SubmitResult {
                    attempt_id: "attempt-1".into(),
                    result: result(),
                },
                expected: |reason| matches!(reason, Rejection::InvalidTransition { .. }),
            },
            Case {
                name: "receipt-before-prepare",
                setup: verified,
                command: Command::RecordSettlementReceipt(SettlementReceipt {
                    settlement_id: "settlement-1".into(),
                    service_credits: 10,
                    external_reference: "receipt".to_owned(),
                }),
                expected: |reason| matches!(reason, Rejection::InvalidTransition { .. }),
            },
            Case {
                name: "verify-before-result",
                setup: running,
                command: Command::VerifyResult {
                    attempt_id: "attempt-1".into(),
                    verifier: "independent-verifier".into(),
                    capability: capability("independent-verifier"),
                    verdict: VerificationVerdict::Accept,
                },
                expected: |reason| matches!(reason, Rejection::InvalidTransition { .. }),
            },
        ];

        for case in cases {
            let mut engine = new_engine();
            (case.setup)(&mut engine);
            let generation = engine.aggregate().generation;
            let outcome = engine.handle(env(case.name, generation, case.command));
            assert_rejection(outcome, case.expected);
        }
    }

    #[test]
    fn submitted_result_can_be_verified_and_settled_after_revocation() {
        let mut engine = new_engine();
        submitted(&mut engine);
        accept(
            &mut engine,
            "revoke-after-submit",
            Command::RevokeConsent {
                grant_id: "grant-1".into(),
                reason: "stop future work".to_owned(),
            },
        );
        accept(
            &mut engine,
            "verify-after-revoke",
            Command::VerifyResult {
                attempt_id: "attempt-1".into(),
                verifier: "independent-verifier".into(),
                capability: capability("independent-verifier"),
                verdict: VerificationVerdict::Accept,
            },
        );
        let outcome = accept(
            &mut engine,
            "prepare-after-revoke",
            Command::PrepareSettlement {
                settlement_id: "settlement-1".into(),
                attempt_id: "attempt-1".into(),
                service_credits: 10,
            },
        );
        assert!(matches!(
            outcome.effects(),
            [Effect::PostServiceCredit { .. }]
        ));
    }

    #[test]
    fn terminal_grant_can_be_renewed_only_with_new_generation() {
        let mut engine = new_engine();
        accept(&mut engine, "grant", Command::GrantConsent(grant()));
        accept(
            &mut engine,
            "revoke",
            Command::RevokeConsent {
                grant_id: "grant-1".into(),
                reason: "rotate".to_owned(),
            },
        );
        let mut renewed = grant();
        renewed.id = "grant-2".into();
        renewed.issued_at_ms = 200;
        renewed.expires_at_ms = 2_000;
        let outcome = engine.handle(env("renew", 2, Command::GrantConsent(renewed)));
        assert!(matches!(outcome, CommandOutcome::Accepted(_)));
        assert_eq!(engine.aggregate().generation, 2);
        assert!(matches!(
            engine.aggregate().consent,
            ConsentState::Active { .. }
        ));
    }
}
