// KG: SPAN_333_L10_Hypervisor, plan-333-p2p-os-synthesis-execution-2026-04-18
// 333 P2P OS L10 Hypervisor — trait layer for managing WASM instances as VMs.
//
// Design sources (prom32-333-p2p-2026-04-18):
//   - finding_333_synth_hyp_sota_d0: Hyperlight/wasmCloud/WASI P2 — 3-layer hypervisor.
//   - finding_333_synth_hyp_prt_d2: 4-trait hierarchy (VmManager/Instance/Snapshot/QuotaEnforcer).
//   - finding_333_synth_hyp_pit_d3: isolation gap, memory quota bypass, pooling allocator leak.
//
// This crate defines ONLY the trait surface + an in-memory implementation for
// deterministic testing. Real WASM backends (Wasmtime, Hyperlight) implement
// `Backend` in a downstream crate; the trait is deliberately narrow so a
// wasmtime::Store / hyperlight::Sandbox can be dropped in without changes here.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use identity333::NodeId;
use thiserror::Error;

// ============================================================================
// Error type
// ============================================================================

#[derive(Debug, Error)]
pub enum HyperError {
    #[error("unknown instance id")]
    UnknownInstance,
    #[error("instance already exists: {0}")]
    AlreadyExists(InstanceId),
    #[error("quota exceeded: {resource} used={used} limit={limit}")]
    QuotaExceeded {
        resource: &'static str,
        used: u64,
        limit: u64,
    },
    #[error("invalid state transition: {from:?} -> {to:?}")]
    InvalidTransition { from: State, to: State },
    #[error("snapshot not found: {0}")]
    SnapshotNotFound(SnapshotId),
    #[error("backend error: {0}")]
    Backend(String),
}

// ============================================================================
// Core types
// ============================================================================

pub type InstanceId = String;
pub type SnapshotId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Created,
    Running,
    Paused,
    Stopped,
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub name: String,
    pub wasm_hash: [u8; 32],
    pub owner: NodeId,
    pub quota: Quota,
}

#[derive(Debug, Clone, Copy)]
pub struct Quota {
    pub max_memory_bytes: u64,
    pub max_cpu_ms: u64,
    pub max_fuel: u64,
}

impl Quota {
    pub const fn unlimited() -> Self {
        Self {
            max_memory_bytes: u64::MAX,
            max_cpu_ms: u64::MAX,
            max_fuel: u64::MAX,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    pub memory_bytes: u64,
    pub cpu_ms: u64,
    pub fuel_consumed: u64,
}

// ============================================================================
// Traits
// ============================================================================

/// L10-top: Manage the set of instances on a single node.
pub trait VmManager {
    fn create(&self, id: InstanceId, manifest: Manifest) -> Result<(), HyperError>;
    fn destroy(&self, id: &InstanceId) -> Result<(), HyperError>;
    fn list(&self) -> Vec<InstanceId>;
    fn get_state(&self, id: &InstanceId) -> Result<State, HyperError>;
}

/// L10-mid: Control an individual instance's lifecycle.
///
/// State machine:
///   Created -> Running   (start)
///   Running -> Paused    (pause)
///   Paused  -> Running   (resume)
///   any     -> Stopped   (stop)
///
/// `pause` on Created or `resume` on Running returns InvalidTransition.
pub trait Instance {
    fn start(&self, id: &InstanceId) -> Result<(), HyperError>;
    fn pause(&self, id: &InstanceId) -> Result<(), HyperError>;
    fn resume(&self, id: &InstanceId) -> Result<(), HyperError>;
    fn stop(&self, id: &InstanceId) -> Result<(), HyperError>;
}

/// L10-right: CoW snapshot capture/restore (finding_333_synth_hyp_sota_d0).
///
/// `restore` is a lifecycle mutation and answers to the `Instance` state
/// machine: it may not move the instance along an edge the machine forbids.
/// Stopped is absorbing, so restoring a Running-era snapshot onto a Stopped
/// instance returns InvalidTransition rather than resurrecting it. Restoring
/// onto the same state is permitted and rewinds usage only.
pub trait Snapshot {
    fn snapshot(&self, id: &InstanceId) -> Result<SnapshotId, HyperError>;
    fn restore(&self, id: &InstanceId, snap: &SnapshotId) -> Result<(), HyperError>;
    fn drop_snapshot(&self, snap: &SnapshotId) -> Result<(), HyperError>;
}

/// L10-left: Host-side quota enforcement (finding_333_synth_hyp_pit_d3 —
/// memory quota bypass prevention).
pub trait QuotaEnforcer {
    fn usage(&self, id: &InstanceId) -> Result<Usage, HyperError>;
    fn check(&self, id: &InstanceId, delta: Usage) -> Result<(), HyperError>;
    fn record(&self, id: &InstanceId, delta: Usage) -> Result<(), HyperError>;
}

// ============================================================================
// In-memory reference implementation
// ============================================================================

#[derive(Debug)]
struct Slot {
    manifest: Manifest,
    state: State,
    usage: Usage,
    snapshots: BTreeMap<SnapshotId, (State, Usage)>,
}

#[derive(Debug, Default)]
pub struct InMemoryHypervisor {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    slots: HashMap<InstanceId, Slot>,
    snap_counter: u64,
}

impl InMemoryHypervisor {
    pub fn new() -> Self {
        Self::default()
    }
}

fn valid_transition(from: State, to: State) -> bool {
    use State::*;
    matches!(
        (from, to),
        (Created, Running)
            | (Running, Paused)
            | (Paused, Running)
            | (Created, Stopped)
            | (Running, Stopped)
            | (Paused, Stopped)
    )
}

/// Restore replays a snapshot's captured lifecycle state onto a live slot, so it
/// is a state mutation and must answer to the same guard as `transition` — a
/// snapshot taken while Running must not be able to resurrect a Stopped
/// instance. Identity is admitted because restoring usage without moving the
/// lifecycle is not a transition at all; it keeps Stopped absorbing rather than
/// making restore-onto-terminal a special case.
fn valid_restore(from: State, to: State) -> bool {
    from == to || valid_transition(from, to)
}

impl VmManager for InMemoryHypervisor {
    fn create(&self, id: InstanceId, manifest: Manifest) -> Result<(), HyperError> {
        let mut g = self.inner.lock().unwrap();
        if g.slots.contains_key(&id) {
            return Err(HyperError::AlreadyExists(id));
        }
        g.slots.insert(
            id,
            Slot {
                manifest,
                state: State::Created,
                usage: Usage::default(),
                snapshots: BTreeMap::new(),
            },
        );
        Ok(())
    }

    fn destroy(&self, id: &InstanceId) -> Result<(), HyperError> {
        let mut g = self.inner.lock().unwrap();
        g.slots.remove(id).ok_or(HyperError::UnknownInstance)?;
        Ok(())
    }

    fn list(&self) -> Vec<InstanceId> {
        self.inner.lock().unwrap().slots.keys().cloned().collect()
    }

    fn get_state(&self, id: &InstanceId) -> Result<State, HyperError> {
        let g = self.inner.lock().unwrap();
        g.slots.get(id).map(|s| s.state).ok_or(HyperError::UnknownInstance)
    }
}

impl Instance for InMemoryHypervisor {
    fn start(&self, id: &InstanceId) -> Result<(), HyperError> {
        transition(self, id, State::Running)
    }
    fn pause(&self, id: &InstanceId) -> Result<(), HyperError> {
        transition(self, id, State::Paused)
    }
    fn resume(&self, id: &InstanceId) -> Result<(), HyperError> {
        transition(self, id, State::Running)
    }
    fn stop(&self, id: &InstanceId) -> Result<(), HyperError> {
        transition(self, id, State::Stopped)
    }
}

fn transition(
    h: &InMemoryHypervisor,
    id: &InstanceId,
    to: State,
) -> Result<(), HyperError> {
    let mut g = h.inner.lock().unwrap();
    let slot = g.slots.get_mut(id).ok_or(HyperError::UnknownInstance)?;
    if !valid_transition(slot.state, to) {
        return Err(HyperError::InvalidTransition { from: slot.state, to });
    }
    slot.state = to;
    Ok(())
}

impl Snapshot for InMemoryHypervisor {
    fn snapshot(&self, id: &InstanceId) -> Result<SnapshotId, HyperError> {
        let mut g = self.inner.lock().unwrap();
        g.snap_counter += 1;
        let snap_id = format!("snap-{}-{}", id, g.snap_counter);
        let slot = g.slots.get_mut(id).ok_or(HyperError::UnknownInstance)?;
        slot.snapshots.insert(snap_id.clone(), (slot.state, slot.usage));
        Ok(snap_id)
    }

    fn restore(&self, id: &InstanceId, snap: &SnapshotId) -> Result<(), HyperError> {
        let mut g = self.inner.lock().unwrap();
        let slot = g.slots.get_mut(id).ok_or(HyperError::UnknownInstance)?;
        let (state, usage) = slot
            .snapshots
            .get(snap)
            .copied()
            .ok_or_else(|| HyperError::SnapshotNotFound(snap.clone()))?;
        // Reject before touching either field: a refused restore must leave the
        // slot exactly as it was, not half-applied with rolled-back usage.
        // Reject before touching either field: a refused restore must leave the
        // slot exactly as it was, not half-applied with rolled-back usage.
        if !valid_restore(slot.state, state) {
            return Err(HyperError::InvalidTransition { from: slot.state, to: state });
        }
        slot.state = state;
        slot.usage = usage;
        Ok(())
    }

    fn drop_snapshot(&self, snap: &SnapshotId) -> Result<(), HyperError> {
        let mut g = self.inner.lock().unwrap();
        for slot in g.slots.values_mut() {
            if slot.snapshots.remove(snap).is_some() {
                return Ok(());
            }
        }
        Err(HyperError::SnapshotNotFound(snap.clone()))
    }
}

/// Quota arithmetic against a slot the caller already holds the lock for.
/// Split out of `check` so `record` can admit and apply a delta under one
/// guard: re-entering the public `check` would drop the lock in between and let
/// a racing recorder spend the headroom this one just measured.
fn check_slot(slot: &Slot, delta: Usage) -> Result<(), HyperError> {
    let q = &slot.manifest.quota;
    let next_mem = slot.usage.memory_bytes.saturating_add(delta.memory_bytes);
    let next_cpu = slot.usage.cpu_ms.saturating_add(delta.cpu_ms);
    let next_fuel = slot.usage.fuel_consumed.saturating_add(delta.fuel_consumed);
    if next_mem > q.max_memory_bytes {
        return Err(HyperError::QuotaExceeded {
            resource: "memory",
            used: next_mem,
            limit: q.max_memory_bytes,
        });
    }
    if next_cpu > q.max_cpu_ms {
        return Err(HyperError::QuotaExceeded {
            resource: "cpu_ms",
            used: next_cpu,
            limit: q.max_cpu_ms,
        });
    }
    if next_fuel > q.max_fuel {
        return Err(HyperError::QuotaExceeded {
            resource: "fuel",
            used: next_fuel,
            limit: q.max_fuel,
        });
    }
    Ok(())
}

impl QuotaEnforcer for InMemoryHypervisor {
    fn usage(&self, id: &InstanceId) -> Result<Usage, HyperError> {
        let g = self.inner.lock().unwrap();
        g.slots.get(id).map(|s| s.usage).ok_or(HyperError::UnknownInstance)
    }

    /// Advisory only: the verdict is stale the moment the lock drops. Callers
    /// that intend to spend the headroom must use `record`, which decides and
    /// applies atomically.
    fn check(&self, id: &InstanceId, delta: Usage) -> Result<(), HyperError> {
        let g = self.inner.lock().unwrap();
        let slot = g.slots.get(id).ok_or(HyperError::UnknownInstance)?;
        check_slot(slot, delta)
    }

    fn record(&self, id: &InstanceId, delta: Usage) -> Result<(), HyperError> {
        let mut g = self.inner.lock().unwrap();
        let slot = g.slots.get_mut(id).ok_or(HyperError::UnknownInstance)?;
        check_slot(slot, delta)?;
        slot.usage.memory_bytes = slot.usage.memory_bytes.saturating_add(delta.memory_bytes);
        slot.usage.cpu_ms = slot.usage.cpu_ms.saturating_add(delta.cpu_ms);
        slot.usage.fuel_consumed = slot.usage.fuel_consumed.saturating_add(delta.fuel_consumed);
        Ok(())
    }
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use identity333::Keypair;

    fn mk_manifest(name: &str) -> Manifest {
        let kp = Keypair::generate();
        Manifest {
            name: name.to_string(),
            wasm_hash: [0u8; 32],
            owner: kp.node_id(),
            quota: Quota {
                max_memory_bytes: 4096,
                max_cpu_ms: 1000,
                max_fuel: 1_000_000,
            },
        }
    }

    #[test]
    fn create_and_list() {
        let h = InMemoryHypervisor::new();
        h.create("a".into(), mk_manifest("a")).unwrap();
        h.create("b".into(), mk_manifest("b")).unwrap();
        let mut list = h.list();
        list.sort();
        assert_eq!(list, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn duplicate_create_rejected() {
        let h = InMemoryHypervisor::new();
        h.create("a".into(), mk_manifest("a")).unwrap();
        let err = h.create("a".into(), mk_manifest("a")).unwrap_err();
        assert!(matches!(err, HyperError::AlreadyExists(_)));
    }

    #[test]
    fn lifecycle_happy_path() {
        let h = InMemoryHypervisor::new();
        h.create("x".into(), mk_manifest("x")).unwrap();
        assert_eq!(h.get_state(&"x".into()).unwrap(), State::Created);
        h.start(&"x".into()).unwrap();
        assert_eq!(h.get_state(&"x".into()).unwrap(), State::Running);
        h.pause(&"x".into()).unwrap();
        h.resume(&"x".into()).unwrap();
        h.stop(&"x".into()).unwrap();
        assert_eq!(h.get_state(&"x".into()).unwrap(), State::Stopped);
    }

    #[test]
    fn invalid_transition_rejected() {
        let h = InMemoryHypervisor::new();
        h.create("x".into(), mk_manifest("x")).unwrap();
        // Created cannot pause directly
        let err = h.pause(&"x".into()).unwrap_err();
        assert!(matches!(err, HyperError::InvalidTransition { .. }));
    }

    #[test]
    fn snapshot_restore_roundtrip() {
        let h = InMemoryHypervisor::new();
        h.create("x".into(), mk_manifest("x")).unwrap();
        h.start(&"x".into()).unwrap();
        h.record(&"x".into(), Usage { memory_bytes: 100, cpu_ms: 50, fuel_consumed: 1000 })
            .unwrap();
        let snap = h.snapshot(&"x".into()).unwrap();
        h.pause(&"x".into()).unwrap();
        h.record(&"x".into(), Usage { memory_bytes: 200, cpu_ms: 0, fuel_consumed: 0 })
            .unwrap();
        h.restore(&"x".into(), &snap).unwrap();
        assert_eq!(h.get_state(&"x".into()).unwrap(), State::Running);
        let u = h.usage(&"x".into()).unwrap();
        assert_eq!(u.memory_bytes, 100);
        assert_eq!(u.fuel_consumed, 1000);
    }

    #[test]
    fn quota_memory_exceeded() {
        let h = InMemoryHypervisor::new();
        h.create("x".into(), mk_manifest("x")).unwrap();
        h.record(&"x".into(), Usage { memory_bytes: 4000, cpu_ms: 0, fuel_consumed: 0 })
            .unwrap();
        let err = h
            .record(&"x".into(), Usage { memory_bytes: 200, cpu_ms: 0, fuel_consumed: 0 })
            .unwrap_err();
        match err {
            HyperError::QuotaExceeded { resource, .. } => assert_eq!(resource, "memory"),
            _ => panic!("expected QuotaExceeded(memory)"),
        }
    }

    #[test]
    fn quota_fuel_exceeded() {
        let h = InMemoryHypervisor::new();
        h.create("x".into(), mk_manifest("x")).unwrap();
        let err = h
            .record(&"x".into(), Usage { memory_bytes: 0, cpu_ms: 0, fuel_consumed: 2_000_000 })
            .unwrap_err();
        assert!(matches!(err, HyperError::QuotaExceeded { resource: "fuel", .. }));
    }

    #[test]
    fn unknown_instance_errors() {
        let h = InMemoryHypervisor::new();
        assert!(matches!(h.get_state(&"nope".into()), Err(HyperError::UnknownInstance)));
        assert!(matches!(h.start(&"nope".into()), Err(HyperError::UnknownInstance)));
        assert!(matches!(h.snapshot(&"nope".into()), Err(HyperError::UnknownInstance)));
    }

    #[test]
    fn drop_snapshot_frees_id() {
        let h = InMemoryHypervisor::new();
        h.create("x".into(), mk_manifest("x")).unwrap();
        let snap = h.snapshot(&"x".into()).unwrap();
        h.drop_snapshot(&snap).unwrap();
        let err = h.restore(&"x".into(), &snap).unwrap_err();
        assert!(matches!(err, HyperError::SnapshotNotFound(_)));
    }
}
