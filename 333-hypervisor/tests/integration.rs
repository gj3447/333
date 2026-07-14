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
