// KG: sprint7F-puter-quality-port-2026-04-16, lesson-333-sdk-test-coupling-2026-04-14
//! Kernel Integration Tests — P1-P10 Puter quality patterns working together.
//!
//! Demonstrates:
//!   P1  ServiceRegistry sealed trait    — service is only valid through registry
//!   P2  Manifest hot-load               — v1 manifest → hot-load v2
//!   P3  Capability model                — denied request returns CapabilityError
//!   P4  Context isolation               — scoped CRDT keys don't leak between services
//!   P5  Service lifecycle               — Created→Starting→Running→Stopping→Stopped
//!   P7  Typed event channels            — publisher/subscriber via Channel<T>
//!   P8  Worker queue                    — deferred jobs, priority ordering
//!   P10 SemVer negotiation             — module compat check against platform version

use triple_three::kernel::{
    // P1
    ServiceRegistry, ServiceError,
    // P2/P10
    ManifestRegistry, ModuleManifest, SemVer, PLATFORM_VERSION,
    // P3/P4
    Capability, CapabilitySet, ServiceContext,
    // P5
    ServiceState, LifecycleMachine,
    // P7
    Channel, ChannelBroker,
    // P8
    WorkerQueue, JobPriority, JobState,
};
use triple_three::kernel::lifecycle::LifecycleError;
use triple_three::kernel::testutil::{NoopService, FailOnInitService};

// ── P1: ServiceRegistry ───────────────────────────────────────────────────────

#[test]
fn kernel_p1_service_lifecycle_full() {
    
    let mut reg = ServiceRegistry::new();
    reg.register(NoopService::new("core"), CapabilitySet::full()).unwrap();
    reg.register(NoopService::new("rts"),  CapabilitySet::app_default()).unwrap();
    reg.register(NoopService::new("social"), CapabilitySet::app_default()).unwrap();

    let errs = reg.start_all();
    assert!(errs.is_empty(), "start_all errors: {:?}", errs);

    assert!(reg.is_running("core"));
    assert!(reg.is_running("rts"));
    assert!(reg.is_running("social"));

    let errs = reg.stop_all();
    assert!(errs.is_empty());
    assert!(!reg.is_running("core"));
}

#[test]
fn kernel_p1_duplicate_registration_rejected() {
    
    let mut reg = ServiceRegistry::new();
    reg.register(NoopService::new("dup"), CapabilitySet::full()).unwrap();
    let err = reg.register(NoopService::new("dup"), CapabilitySet::full());
    assert!(err.is_err());
}

#[test]
fn kernel_p1_init_failure_transitions_to_failed() {
    
    let mut reg = ServiceRegistry::new();
    reg.register(FailOnInitService("bad".into()), CapabilitySet::full()).unwrap();
    assert!(reg.start("bad").is_err());
    assert!(matches!(reg.state("bad"), Some(ServiceState::Failed(_))));
}

// ── P2/P10: Manifest + SemVer ────────────────────────────────────────────────

#[test]
fn kernel_p2_p10_manifest_hot_load() {
    let mut reg = ManifestRegistry::new();

    let v1 = ModuleManifest::static_module(
        "com.333.rts", "RTS v1", SemVer::new(1, 0, 0), SemVer::new(1, 0, 0)
    ).with_required_caps(vec![Capability::WorldRead, Capability::WorldWrite]);

    reg.register(v1).unwrap();
    assert_eq!(reg.get("com.333.rts").unwrap().version, SemVer::new(1, 0, 0));

    // Upgrade to v2 via hot-load
    let v2 = ModuleManifest::static_module(
        "com.333.rts", "RTS v2", SemVer::new(1, 1, 0), SemVer::new(1, 0, 0)
    );
    reg.hot_load(v2, &PLATFORM_VERSION).unwrap();
    assert_eq!(reg.get("com.333.rts").unwrap().version, SemVer::new(1, 1, 0));
}

#[test]
fn kernel_p10_semver_compatibility() {
    // Platform 1.0 satisfies modules requiring >=1.0
    let m = ModuleManifest::static_module(
        "com.333.social", "Social", SemVer::new(1, 0, 0), SemVer::new(1, 0, 0)
    );
    assert!(m.is_platform_compatible(&PLATFORM_VERSION));

    // Module requiring 2.0 is rejected on platform 1.x
    let future = ModuleManifest::static_module(
        "com.future", "Future", SemVer::new(1, 0, 0), SemVer::new(2, 0, 0)
    );
    let err = ManifestRegistry::new().hot_load(future, &PLATFORM_VERSION);
    assert!(err.is_err());
}

// ── P3/P4: Capability + Context Isolation ────────────────────────────────────

#[test]
fn kernel_p3_capability_deny() {
    let ctx = ServiceContext::new("rts:core", CapabilitySet::read_only());
    // read_only doesn't have WorldWrite
    let err = ctx.require(&Capability::WorldWrite);
    assert!(err.is_err());
    let msg = err.unwrap_err().to_string();
    assert!(msg.contains("world:write"), "error: {}", msg);
}

#[test]
fn kernel_p4_namespace_isolation() {
    let ctx_a = ServiceContext::new("app_a", CapabilitySet::app_default());
    let ctx_b = ServiceContext::new("app_b", CapabilitySet::app_default());

    // Same logical key → different scoped keys
    assert_eq!(ctx_a.scoped_key("units"), "app_a:units");
    assert_eq!(ctx_b.scoped_key("units"), "app_b:units");
    assert_ne!(ctx_a.scoped_key("units"), ctx_b.scoped_key("units"));
}

#[test]
fn kernel_p4_capability_intersection() {
    let full = CapabilitySet::full();
    let ro   = CapabilitySet::read_only();
    let intersection = full.intersect(&ro);

    // Intersection contains only what read_only has
    assert!(intersection.has(&Capability::WorldRead));
    assert!(!intersection.has(&Capability::WorldWrite));
    assert!(!intersection.has(&Capability::TokenTransfer));
}

// ── P5: Lifecycle State Machine ───────────────────────────────────────────────

#[test]
fn kernel_p5_lifecycle_transitions() {
    let mut m = LifecycleMachine::new();
    assert_eq!(*m.state(), ServiceState::Created);

    m.transition(ServiceState::Starting).unwrap();
    m.transition(ServiceState::Running).unwrap();
    assert!(m.is_running());

    m.transition(ServiceState::Stopping).unwrap();
    m.transition(ServiceState::Stopped).unwrap();
    assert_eq!(*m.state(), ServiceState::Stopped);
}

#[test]
fn kernel_p5_illegal_transition_rejected() {
    let mut m = LifecycleMachine::new();
    // Cannot jump Created → Running (must go through Starting)
    let err = m.transition(ServiceState::Running);
    assert!(matches!(err, Err(LifecycleError::InvalidTransition { .. })));
}

#[test]
fn kernel_p5_fail_from_any_non_terminal_state() {
    // fail from Starting
    let mut m = LifecycleMachine::new();
    m.transition(ServiceState::Starting).unwrap();
    m.transition(ServiceState::Failed("test".into())).unwrap();
    assert!(matches!(m.state(), &ServiceState::Failed(_)));
    // failed is terminal
    assert!(m.transition(ServiceState::Running).is_err());
}

// ── P7: Typed Event Channels ─────────────────────────────────────────────────

use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct RtsEvent { kind: String, unit_id: u32 }

#[test]
fn kernel_p7_channel_publish_subscribe() {
    use std::sync::{Arc, Mutex};

    let mut ch: Channel<RtsEvent> = Channel::new("rts:events");
    let received = Arc::new(Mutex::new(Vec::new()));
    let r = Arc::clone(&received);

    ch.subscribe(move |e: RtsEvent| { r.lock().unwrap().push(e); });

    ch.publish(RtsEvent { kind: "spawn".into(), unit_id: 1 });
    ch.publish(RtsEvent { kind: "move".into(),  unit_id: 2 });

    let msgs = received.lock().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].kind, "spawn");
    assert_eq!(msgs[1].unit_id, 2);
}

#[test]
fn kernel_p7_late_subscriber_replay() {
    let mut ch: Channel<RtsEvent> = Channel::new("rts:history");
    for i in 0..20u32 {
        ch.publish(RtsEvent { kind: "tick".into(), unit_id: i });
    }

    // Late subscriber replays last 5
    let mut replayed = Vec::new();
    ch.replay(5, |e: RtsEvent| replayed.push(e.unit_id));
    assert_eq!(replayed.len(), 5);
    assert_eq!(replayed[0], 15); // events 15-19
    assert_eq!(replayed[4], 19);
}

#[test]
fn kernel_p7_channel_isolation() {
    use std::sync::{Arc, Mutex};

    let mut rts_ch: Channel<RtsEvent>  = Channel::new("rts:events");
    let mut social_ch: Channel<String> = Channel::new("social:posts");

    let rts_received    = Arc::new(Mutex::new(0u32));
    let social_received = Arc::new(Mutex::new(0u32));
    let r1 = Arc::clone(&rts_received);
    let r2 = Arc::clone(&social_received);

    rts_ch.subscribe(move |_| { *r1.lock().unwrap() += 1; });
    social_ch.subscribe(move |_: String| { *r2.lock().unwrap() += 1; });

    rts_ch.publish(RtsEvent { kind: "a".into(), unit_id: 0 });
    social_ch.publish("hello".to_string());

    // Each channel only received its own events
    assert_eq!(*rts_received.lock().unwrap(), 1);
    assert_eq!(*social_received.lock().unwrap(), 1);
}

// ── P8: Worker Queue ──────────────────────────────────────────────────────────

#[test]
fn kernel_p8_priority_ordering() {
    use std::sync::{Arc, Mutex};

    let mut wq = WorkerQueue::new();
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

    let o = Arc::clone(&order);
    wq.enqueue("low",      JobPriority::Low,      move || { o.lock().unwrap().push("low");      Ok(()) });
    let o = Arc::clone(&order);
    wq.enqueue("critical", JobPriority::Critical, move || { o.lock().unwrap().push("critical"); Ok(()) });
    let o = Arc::clone(&order);
    wq.enqueue("normal",   JobPriority::Normal,   move || { o.lock().unwrap().push("normal");   Ok(()) });

    wq.tick(3);
    let result = order.lock().unwrap().clone();
    assert_eq!(result, vec!["critical", "normal", "low"]);
}

#[test]
fn kernel_p8_deferred_work() {
    use std::sync::{Arc, Mutex};

    let mut wq = WorkerQueue::new();
    let results = Arc::new(Mutex::new(Vec::new()));

    // Simulate game-loop deferred work
    for i in 0..5u32 {
        let r = Arc::clone(&results);
        wq.enqueue(format!("frame_{}", i), JobPriority::Normal, move || {
            r.lock().unwrap().push(i);
            Ok(())
        });
    }

    // Process 2 per tick (like a game loop budget)
    wq.tick(2); assert_eq!(results.lock().unwrap().len(), 2);
    wq.tick(2); assert_eq!(results.lock().unwrap().len(), 4);
    wq.tick(2); assert_eq!(results.lock().unwrap().len(), 5);
    assert_eq!(wq.pending_count(), 0);
}

#[test]
fn kernel_p8_cancel_and_stats() {
    let mut wq = WorkerQueue::new();
    let id_ok  = wq.enqueue("ok",     JobPriority::Normal, || Ok(()));
    let id_bad = wq.enqueue("bad",    JobPriority::Normal, || Err("boom".into()));
    let id_can = wq.enqueue("cancel", JobPriority::Normal, || Ok(()));

    wq.cancel(id_can);
    wq.tick(10);

    let s = wq.stats();
    assert_eq!(s.submitted, 3);
    assert_eq!(s.completed, 1);
    assert_eq!(s.failed, 1);

    assert!(matches!(wq.outcome(id_ok),  Some(JobState::Completed)));
    assert!(matches!(wq.outcome(id_bad), Some(JobState::Failed(_))));
    assert!(matches!(wq.outcome(id_can), Some(JobState::Cancelled)));
}

// ── Full Integration: Kernel Orchestration ────────────────────────────────────

/// Demonstrates all 7 patterns working together in a mini platform bootstrap.
#[test]
fn kernel_full_bootstrap() {
    
    use std::sync::{Arc, Mutex};

    // 1. P2/P10: Register module manifests with version check
    let mut manifests = ManifestRegistry::new();
    manifests.register(ModuleManifest::static_module(
        "com.333.kernel", "Platform Kernel",
        SemVer::new(1, 0, 0), SemVer::new(1, 0, 0),
    )).unwrap();
    manifests.register(ModuleManifest::static_module(
        "com.333.rts", "RTS Game",
        SemVer::new(1, 0, 0), SemVer::new(1, 0, 0),
    ).with_required_caps(vec![Capability::WorldRead, Capability::WorldWrite])).unwrap();
    assert_eq!(manifests.len(), 2);

    // 2. P1: Register services in registry
    let mut registry = ServiceRegistry::new();
    registry.register(NoopService::new("kernel"), CapabilitySet::full()).unwrap();
    registry.register(NoopService::new("rts:core"), CapabilitySet::app_default()).unwrap();

    // 3. P5: All services boot correctly
    let errs = registry.start_all();
    assert!(errs.is_empty());

    // 4. P3: Verify capability enforcement
    let ctx = ServiceContext::new("rts:core", CapabilitySet::app_default());
    assert!(ctx.require(&Capability::WorldWrite).is_ok());
    assert!(ctx.require(&Capability::ConsensusSubmit).is_err());

    // 5. P4: Scoped keys don't collide
    let ctx2 = ServiceContext::new("social", CapabilitySet::app_default());
    assert_ne!(ctx.scoped_key("data"), ctx2.scoped_key("data"));

    // 6. P7: Events flow on typed channels
    let mut rts_channel: Channel<RtsEvent> = Channel::new("rts:events");
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let l = Arc::clone(&log);
    rts_channel.subscribe(move |e: RtsEvent| { l.lock().unwrap().push(e.kind); });
    rts_channel.publish(RtsEvent { kind: "unit_spawned".into(), unit_id: 1 });
    assert_eq!(log.lock().unwrap().len(), 1);

    // 7. P8: Deferred work drains correctly
    let mut wq = WorkerQueue::new();
    let ran = Arc::new(Mutex::new(false));
    let r = Arc::clone(&ran);
    wq.enqueue("shutdown_hook", JobPriority::High, move || { *r.lock().unwrap() = true; Ok(()) });
    registry.stop_all();
    wq.tick(1);
    assert!(*ran.lock().unwrap());
}
