// KG: finding_333_e2e_worker_d6, plan-333-e2e-coverage-expansion-2026-04-16
//! E2E Kernel Lifecycle Tests — service startup, capability, work queue, multi-priority
//!
//! Tests the kernel subsystem (ServiceRegistry, WorkerQueue, Capability, Lifecycle)
//! as integrated through PlatformCore.

use triple_three::platform::{PlatformCore, ExecutionRequest, ExecutionResponse};
use triple_three::kernel::{
    JobPriority, Capability, CapabilitySet,
    ServiceState, WorkerQueue, ServiceRegistry,
};

const VALIDATORS: &[u32] = &[1, 2, 3, 4, 5, 6, 7];

fn make_platform() -> PlatformCore {
    PlatformCore::new(1, VALIDATORS)
}

// ── 1. Kernel boots with all built-in services ─────────────────────────────

#[test]
fn kernel_boots_three_services_running() {
    let p = make_platform();
    assert!(p.kernel_healthy());
    assert!(p.kernel_service_running("platform:crdt"));
    assert!(p.kernel_service_running("platform:consensus"));
    assert!(p.kernel_service_running("platform:token"));
}

#[test]
fn kernel_nonexistent_service_not_running() {
    let p = make_platform();
    assert!(!p.kernel_service_running("phantom:service"));
}

// ── 2. Work queue priority ordering ────────────────────────────────────────

#[test]
fn work_queue_executes_high_before_low() {
    use std::sync::{Arc, Mutex};
    let mut p = make_platform();
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

    let o = Arc::clone(&order);
    p.enqueue_work("low_job", JobPriority::Low, move || {
        o.lock().unwrap().push("low");
        Ok(())
    });
    let o = Arc::clone(&order);
    p.enqueue_work("normal_job", JobPriority::Normal, move || {
        o.lock().unwrap().push("normal");
        Ok(())
    });
    let o = Arc::clone(&order);
    p.enqueue_work("high_job", JobPriority::High, move || {
        o.lock().unwrap().push("high");
        Ok(())
    });
    let o = Arc::clone(&order);
    p.enqueue_work("critical_job", JobPriority::Critical, move || {
        o.lock().unwrap().push("critical");
        Ok(())
    });

    assert_eq!(p.pending_work(), 4);
    let ran = p.tick_kernel(10);
    assert_eq!(ran, 4);
    assert_eq!(p.pending_work(), 0);

    let result = order.lock().unwrap().clone();
    assert_eq!(result, vec!["critical", "high", "normal", "low"]);
}

// ── 3. Work queue partial tick ─────────────────────────────────────────────

#[test]
fn work_queue_partial_tick_respects_max() {
    use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
    let mut p = make_platform();
    let counter = Arc::new(AtomicU32::new(0));

    for i in 0..10 {
        let c = Arc::clone(&counter);
        p.enqueue_work(format!("job_{}", i), JobPriority::Normal, move || {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
    }

    assert_eq!(p.pending_work(), 10);

    // Only tick 3 jobs
    let ran = p.tick_kernel(3);
    assert_eq!(ran, 3);
    assert_eq!(counter.load(Ordering::SeqCst), 3);
    assert_eq!(p.pending_work(), 7);

    // Tick remaining
    let ran = p.tick_kernel(100);
    assert_eq!(ran, 7);
    assert_eq!(counter.load(Ordering::SeqCst), 10);
    assert_eq!(p.pending_work(), 0);
}

// ── 4. Capability enforcement ──────────────────────────────────────────────

#[test]
fn capability_enforcement_grants_correct_access() {
    let p = make_platform();
    // CRDT service has WorldRead + WorldWrite
    assert!(p.kernel_has_capability("platform:crdt", &Capability::WorldRead));
    assert!(p.kernel_has_capability("platform:crdt", &Capability::WorldWrite));
    // CRDT does NOT have ConsensusSubmit
    assert!(!p.kernel_has_capability("platform:crdt", &Capability::ConsensusSubmit));
    // Consensus has ConsensusSubmit
    assert!(p.kernel_has_capability("platform:consensus", &Capability::ConsensusSubmit));
    // Token has TokenRead + TokenTransfer
    assert!(p.kernel_has_capability("platform:token", &Capability::TokenRead));
    assert!(p.kernel_has_capability("platform:token", &Capability::TokenTransfer));
}

#[test]
fn capability_denied_blocks_work() {
    let mut p = make_platform();
    // Try to enqueue with wrong capability for service
    let result = p.enqueue_work_with_cap(
        "bad_job", JobPriority::Normal,
        &Capability::ConsensusSubmit, "platform:crdt", // crdt doesn't have ConsensusSubmit
        || Ok(()),
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("capability denied"));
}

#[test]
fn capability_granted_allows_work() {
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
    let mut p = make_platform();
    let executed = Arc::new(AtomicBool::new(false));
    let e = Arc::clone(&executed);

    let result = p.enqueue_work_with_cap(
        "good_job", JobPriority::Normal,
        &Capability::WorldWrite, "platform:crdt", // crdt has WorldWrite
        move || { e.store(true, Ordering::SeqCst); Ok(()) },
    );
    assert!(result.is_ok());
    p.tick_kernel(1);
    assert!(executed.load(Ordering::SeqCst));
}

// ── 5. Failed jobs don't block queue ───────────────────────────────────────

#[test]
fn failed_job_doesnt_block_subsequent_jobs() {
    use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
    let mut p = make_platform();
    let counter = Arc::new(AtomicU32::new(0));

    // Job 1: will fail
    p.enqueue_work("failing_job", JobPriority::High, || {
        Err("intentional failure".into())
    });

    // Job 2: should still run
    let c = Arc::clone(&counter);
    p.enqueue_work("good_job", JobPriority::Normal, move || {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    let ran = p.tick_kernel(10);
    assert_eq!(ran, 2); // both attempted
    assert_eq!(counter.load(Ordering::SeqCst), 1); // good job ran
}

// ── 6. Kernel + CRDT interleaved ───────────────────────────────────────────

#[test]
fn kernel_work_interleaved_with_crdt_operations() {
    use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
    let mut p = make_platform();
    let counter = Arc::new(AtomicU32::new(0));

    // Do some CRDT work
    for i in 0..5 {
        p.execute(ExecutionRequest::CrdtUpdate {
            key: format!("block_{}", i),
            value: Some(format!("value_{}", i)),
        });
    }
    assert_eq!(p.world_size(), 5);

    // Enqueue kernel jobs
    for i in 0..5 {
        let c = Arc::clone(&counter);
        p.enqueue_work(format!("job_{}", i), JobPriority::Normal, move || {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
    }

    // More CRDT work
    for i in 5..10 {
        p.execute(ExecutionRequest::CrdtUpdate {
            key: format!("block_{}", i),
            value: Some(format!("value_{}", i)),
        });
    }
    assert_eq!(p.world_size(), 10);

    // Tick kernel
    p.tick_kernel(10);
    assert_eq!(counter.load(Ordering::SeqCst), 5);
    assert_eq!(p.pending_work(), 0);
    // CRDT state preserved
    assert_eq!(p.world_size(), 10);
    assert_eq!(p.get_block("block_3"), Some(&"value_3".to_string()));
}

// ── 7. Stress: many jobs at all priorities ─────────────────────────────────

#[test]
fn stress_100_jobs_mixed_priority() {
    use std::sync::{Arc, Mutex};
    let mut p = make_platform();
    let results = Arc::new(Mutex::new(Vec::<(JobPriority, u32)>::new()));

    let priorities = [
        JobPriority::Critical,
        JobPriority::High,
        JobPriority::Normal,
        JobPriority::Low,
    ];

    for i in 0..100u32 {
        let prio = priorities[(i % 4) as usize];
        let r = Arc::clone(&results);
        p.enqueue_work(format!("stress_{}", i), prio, move || {
            r.lock().unwrap().push((prio, i));
            Ok(())
        });
    }

    assert_eq!(p.pending_work(), 100);
    let ran = p.tick_kernel(200);
    assert_eq!(ran, 100);
    assert_eq!(p.pending_work(), 0);

    let output = results.lock().unwrap();
    assert_eq!(output.len(), 100);

    // Verify priority ordering: all Critical before all High before all Normal before all Low
    let mut last_prio = JobPriority::Critical;
    for &(prio, _) in output.iter() {
        assert!(prio <= last_prio, "priority ordering violated: {:?} after {:?}", prio, last_prio);
        last_prio = prio;
    }
}
