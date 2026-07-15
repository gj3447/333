// KG: SPAN_333_L10_Hypervisor, plan-333-p2p-os-synthesis-execution-2026-04-18
// Integration tests: cross-trait interactions (VmManager + Instance + Snapshot + QuotaEnforcer).

use hypervisor333::*;
use identity333::Keypair;

fn manifest(quota_mem: u64, quota_cpu: u64, quota_fuel: u64) -> Manifest {
    Manifest {
        name: "t".into(),
        wasm_hash: [7u8; 32],
        owner: Keypair::generate().node_id(),
        quota: Quota {
            max_memory_bytes: quota_mem,
            max_cpu_ms: quota_cpu,
            max_fuel: quota_fuel,
        },
    }
}

#[test]
fn full_lifecycle_with_snapshot_and_quota() {
    let h = InMemoryHypervisor::new();
    let id: InstanceId = "alpha".into();
    h.create(id.clone(), manifest(10_000, 5_000, 100_000)).unwrap();
    h.start(&id).unwrap();
    h.record(&id, Usage { memory_bytes: 1_000, cpu_ms: 100, fuel_consumed: 10_000 })
        .unwrap();
    let snap = h.snapshot(&id).unwrap();
    h.record(&id, Usage { memory_bytes: 5_000, cpu_ms: 500, fuel_consumed: 50_000 })
        .unwrap();
    h.restore(&id, &snap).unwrap();
    let u = h.usage(&id).unwrap();
    assert_eq!(u.memory_bytes, 1_000);
    assert_eq!(u.fuel_consumed, 10_000);
    h.stop(&id).unwrap();
    assert_eq!(h.get_state(&id).unwrap(), State::Stopped);
}

#[test]
fn destroy_removes_from_list() {
    let h = InMemoryHypervisor::new();
    h.create("a".into(), manifest(1, 1, 1)).unwrap();
    h.create("b".into(), manifest(1, 1, 1)).unwrap();
    assert_eq!(h.list().len(), 2);
    h.destroy(&"a".into()).unwrap();
    assert_eq!(h.list(), vec!["b".to_string()]);
}

#[test]
fn quota_saturates_not_panics() {
    let h = InMemoryHypervisor::new();
    h.create("x".into(), manifest(100, 100, 100)).unwrap();
    h.record(&"x".into(), Usage { memory_bytes: u64::MAX, cpu_ms: 0, fuel_consumed: 0 })
        .unwrap_err();
}

#[test]
fn unlimited_quota_never_trips() {
    let m = Manifest {
        name: "u".into(),
        wasm_hash: [0u8; 32],
        owner: Keypair::generate().node_id(),
        quota: Quota::unlimited(),
    };
    let h = InMemoryHypervisor::new();
    h.create("u".into(), m).unwrap();
    h.record(&"u".into(), Usage { memory_bytes: u64::MAX / 2, cpu_ms: u64::MAX / 2, fuel_consumed: u64::MAX / 2 })
        .unwrap();
}

// Stopped is absorbing. A snapshot captured while Running is a live handle to a
// Running state, and restore must not smuggle it past the lifecycle guard.
#[test]
fn restore_cannot_resurrect_stopped_instance() {
    let h = InMemoryHypervisor::new();
    let id: InstanceId = "zombie".into();
    h.create(id.clone(), manifest(10_000, 10_000, 10_000)).unwrap();
    h.start(&id).unwrap();
    let snap = h.snapshot(&id).unwrap();
    h.stop(&id).unwrap();

    let err = h.restore(&id, &snap).unwrap_err();
    assert!(
        matches!(
            err,
            HyperError::InvalidTransition { from: State::Stopped, to: State::Running }
        ),
        "expected InvalidTransition(Stopped -> Running), got {:?}",
        err
    );
    assert_eq!(h.get_state(&id).unwrap(), State::Stopped);
}

// A refused restore must not half-apply: usage is rolled back only if the
// lifecycle move is admitted.
#[test]
fn refused_restore_leaves_usage_untouched() {
    let h = InMemoryHypervisor::new();
    let id: InstanceId = "x".into();
    h.create(id.clone(), manifest(10_000, 10_000, 10_000)).unwrap();
    h.start(&id).unwrap();
    h.record(&id, Usage { memory_bytes: 100, cpu_ms: 1, fuel_consumed: 10 }).unwrap();
    let snap = h.snapshot(&id).unwrap();
    h.stop(&id).unwrap();
    h.record(&id, Usage { memory_bytes: 500, cpu_ms: 5, fuel_consumed: 50 }).unwrap();

    h.restore(&id, &snap).unwrap_err();
    let u = h.usage(&id).unwrap();
    assert_eq!(u.memory_bytes, 600, "usage must survive a refused restore");
    assert_eq!(u.cpu_ms, 6);
    assert_eq!(u.fuel_consumed, 60);
}

// Restoring onto the same lifecycle state is not a transition — it rewinds
// usage and must stay legal, including on a terminal instance.
#[test]
fn restore_onto_same_state_is_permitted() {
    let h = InMemoryHypervisor::new();
    let id: InstanceId = "x".into();
    h.create(id.clone(), manifest(10_000, 10_000, 10_000)).unwrap();
    h.start(&id).unwrap();
    h.record(&id, Usage { memory_bytes: 100, cpu_ms: 0, fuel_consumed: 0 }).unwrap();
    let running_snap = h.snapshot(&id).unwrap();
    h.record(&id, Usage { memory_bytes: 900, cpu_ms: 0, fuel_consumed: 0 }).unwrap();
    h.restore(&id, &running_snap).unwrap();
    assert_eq!(h.get_state(&id).unwrap(), State::Running);
    assert_eq!(h.usage(&id).unwrap().memory_bytes, 100);

    h.stop(&id).unwrap();
    let stopped_snap = h.snapshot(&id).unwrap();
    h.restore(&id, &stopped_snap).unwrap();
    assert_eq!(h.get_state(&id).unwrap(), State::Stopped);
}

// Restore is guarded, not frozen: legal edges still go through.
#[test]
fn restore_along_legal_edge_still_works() {
    let h = InMemoryHypervisor::new();
    let id: InstanceId = "x".into();
    h.create(id.clone(), manifest(10_000, 10_000, 10_000)).unwrap();
    h.start(&id).unwrap();
    let snap = h.snapshot(&id).unwrap();
    h.pause(&id).unwrap();
    h.restore(&id, &snap).unwrap(); // Paused -> Running is a legal edge
    assert_eq!(h.get_state(&id).unwrap(), State::Running);
}

// record() admits and applies under one lock. Eight threads racing for headroom
// that only fits five of them must not overshoot the limit.
#[test]
fn concurrent_record_never_exceeds_quota() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    const THREADS: usize = 8;
    const DELTA: u64 = 200;
    const LIMIT: u64 = 1_000;

    // Repeated because a TOCTOU window is a race, not a certainty: the
    // pre-fix code needed a handful of trials to lose it.
    for trial in 0..2_000 {
        let h = Arc::new(InMemoryHypervisor::new());
        let id: InstanceId = "x".into();
        h.create(id.clone(), manifest(LIMIT, u64::MAX, u64::MAX)).unwrap();

        let barrier = Arc::new(Barrier::new(THREADS));
        let mut handles = vec![];
        for _ in 0..THREADS {
            let h2 = h.clone();
            let b2 = barrier.clone();
            let id2 = id.clone();
            handles.push(thread::spawn(move || {
                b2.wait();
                h2.record(&id2, Usage { memory_bytes: DELTA, cpu_ms: 0, fuel_consumed: 0 })
                    .is_ok()
            }));
        }
        let admitted = handles
            .into_iter()
            .fold(0u64, |acc, j| acc + u64::from(j.join().unwrap()));

        let used = h.usage(&id).unwrap().memory_bytes;
        assert!(
            used <= LIMIT,
            "trial {}: usage {} exceeded quota {}",
            trial,
            used,
            LIMIT
        );
        // Every admitted record is accounted for; no lost updates either.
        assert_eq!(used, admitted * DELTA, "trial {}: usage/admission mismatch", trial);
        assert_eq!(admitted, LIMIT / DELTA, "trial {}: headroom under-used", trial);
    }
}

#[test]
fn concurrent_create_destroy_via_clone() {
    use std::sync::Arc;
    use std::thread;
    let h = Arc::new(InMemoryHypervisor::new());
    let mut handles = vec![];
    for i in 0..16 {
        let h2 = h.clone();
        handles.push(thread::spawn(move || {
            let id: InstanceId = format!("n{}", i);
            h2.create(id.clone(), manifest(1_000, 1_000, 10_000)).unwrap();
            h2.start(&id).unwrap();
            h2.record(&id, Usage { memory_bytes: 100, cpu_ms: 10, fuel_consumed: 100 })
                .unwrap();
            h2.stop(&id).unwrap();
        }));
    }
    for j in handles {
        j.join().unwrap();
    }
    assert_eq!(h.list().len(), 16);
}
