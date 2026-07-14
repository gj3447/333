// KG: SPAN_333_Stress_Integration
// Integration stress tests — run via `cargo test --release -p stress333`.
// Each asserts an invariant under load; none measure throughput (that's
// the bench binary's job).

use stress333::drivers::*;

#[test]
fn invariant_ledger_supply_conservation_under_contention() {
    let (start, end) = run_concurrent_transfers(8, 10_000, 16);
    assert_eq!(start, end, "total supply must not drift under concurrent transfers");
}

#[test]
fn invariant_broker_fanout_no_loss() {
    let (delivered, expected) = run_broker_fanout(32, 500);
    assert_eq!(delivered, expected, "every sub must receive every publish");
}

#[test]
fn invariant_parallel_consensus_votes_commit() {
    let finality = run_concurrent_consensus_vote(16);
    assert_eq!(
        finality,
        consensus333::BlockFinality::Committed,
        "parallel votes from a full validator set must drive to Committed"
    );
}

#[test]
fn invariant_byzantine_double_signs_detected_and_honest_commits() {
    let (detected, finality) = run_byzantine_votes(16);
    // f = (16-1)/3 = 5 byzantine. Each should be caught on their second vote.
    assert_eq!(detected, 5, "all byzantine double-signs must be detected");
    assert_eq!(
        finality,
        consensus333::BlockFinality::Committed,
        "honest quorum (11) should still commit despite f=5 byzantine"
    );
}

#[test]
fn invariant_concurrent_world_writes_persist_last_wins() {
    // Hammer a single voxel position from many threads; last committed op wins
    // but the world must NOT enter a torn state.
    use std::sync::Arc;
    use std::thread;
    use std::sync::Barrier;
    use voxel_ref333::*;

    let store: Arc<content333::InMemoryBlockStore> =
        Arc::new(content333::InMemoryBlockStore::new());
    let chunks = ChunkStore::new(store);
    let world = Arc::new(WorldState::new(chunks));

    let threads = 16;
    let barrier = Arc::new(Barrier::new(threads));
    let mut handles = Vec::new();
    for tid in 0..threads {
        let world = world.clone();
        let barrier = barrier.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            let op = BlockPlaceOp {
                actor: identity333::Keypair::generate().node_id(),
                x: 0,
                y: 0,
                z: 0,
                kind: (tid + 1) as u16,
            };
            world.apply_committed(&op).unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let v = world.voxel(0, 0, 0).unwrap();
    assert!(v >= 1 && v <= threads as u16, "voxel value in valid range, got {}", v);
}

#[test]
fn invariant_large_provenance_chain_semver_monotone() {
    use identity333::Keypair;
    use release333::*;

    let signer = Ed25519Signer::new(Keypair::generate());
    let chain = ProvenanceChain::new();
    chain.trust(signer.node_id());

    let n = 500;
    let mut parent: Option<String> = None;
    for i in 0..n {
        let rid = format!("rel-{}", i);
        let version = Version::new(1, 0, i as u32);
        let m = signer
            .sign_manifest(&rid, version, 1, parent.as_deref(), vec![])
            .unwrap();
        chain.append(m).unwrap();
        parent = Some(rid);
    }
    assert_eq!(chain.len(), n);
    let line = chain.ancestry(&format!("rel-{}", n - 1));
    assert_eq!(line.len(), n, "ancestry walks the whole chain");
    // Versions must be strictly increasing along the ancestry.
    for w in line.windows(2) {
        assert!(w[0].version < w[1].version);
    }
}

#[test]
fn invariant_tampered_manifest_rejected_at_scale() {
    use identity333::Keypair;
    use release333::*;

    let signer = Ed25519Signer::new(Keypair::generate());
    let chain = ProvenanceChain::new();
    chain.trust(signer.node_id());

    // Fill with 100 legit releases.
    let mut parent: Option<String> = None;
    for i in 0..100 {
        let rid = format!("legit-{}", i);
        let m = signer
            .sign_manifest(&rid, Version::new(1, 0, i as u32), 1, parent.as_deref(), vec![])
            .unwrap();
        chain.append(m).unwrap();
        parent = Some(rid);
    }
    // Now attempt 100 tampered appends — ALL must be rejected.
    let mut rejected = 0;
    for i in 0..100 {
        let mut m = signer
            .sign_manifest(&format!("evil-{}", i), Version::new(2, 0, i as u32), 1, parent.as_deref(), vec![])
            .unwrap();
        m.version = Version::new(9, 9, 9); // tamper after signing
        if chain.append(m).is_err() {
            rejected += 1;
        }
    }
    assert_eq!(rejected, 100, "every tampered manifest must be rejected");
}

#[test]
fn invariant_opfs_writer_contention_serializes() {
    use std::sync::Arc;
    use std::thread;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use storage_advanced333::*;

    let opfs = Arc::new(InMemoryOpfs::new(1024 * 1024));
    let acquired = Arc::new(AtomicUsize::new(0));
    let denied = Arc::new(AtomicUsize::new(0));

    let threads = 16;
    let mut handles = Vec::new();
    for _ in 0..threads {
        let opfs = opfs.clone();
        let acquired = acquired.clone();
        let denied = denied.clone();
        handles.push(thread::spawn(move || {
            match opfs.acquire_writer("/contended") {
                Ok(g) => {
                    acquired.fetch_add(1, Ordering::Relaxed);
                    let _ = g.write(b"payload");
                    // Hold the lock briefly to expose contention.
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(_) => {
                    denied.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let a = acquired.load(Ordering::Relaxed);
    let d = denied.load(Ordering::Relaxed);
    assert_eq!(a + d, threads, "every thread resolves");
    // With sleep-while-held, most threads should be denied the first attempt.
    // We just assert at least one was denied to prove mutual exclusion.
    assert!(d >= 1, "OPFS should deny at least one concurrent writer, got {} denied", d);
}

#[test]
fn invariant_parallel_saga_workflows_independent() {
    use std::sync::Arc;
    use std::thread;
    use orchestration333::*;

    // Shared coordinator, independent workflows → no cross-contamination.
    struct OkExec;
    impl StepExecutor for OkExec {
        fn kind(&self) -> &str { "ok" }
        fn run_step(&self, _: &WorkflowId, _: &StepSpec) -> Result<Vec<u8>, OrchError> {
            Ok(vec![])
        }
    }
    struct OkComp;
    impl CompensatingAction for OkComp {
        fn kind(&self) -> &str { "ok" }
        fn compensate(&self, _: &WorkflowId, _: &StepSpec, _: &[u8]) -> Result<(), OrchError> { Ok(()) }
    }

    let j = InMemoryJournal::new();
    let bus = mq333::InMemoryBroker::new();
    let mut coord = SagaCoordinator::new(j, bus);
    coord.register_executor(Arc::new(OkExec));
    coord.register_compensator(Arc::new(OkComp));
    let coord = Arc::new(coord);

    let threads = 8;
    let mut handles = Vec::new();
    for tid in 0..threads {
        let coord = coord.clone();
        handles.push(thread::spawn(move || {
            let def = WorkflowDef {
                id: format!("wf-{}", tid),
                steps: vec![StepSpec {
                    id: "a".into(),
                    kind: "ok".into(),
                    input: vec![],
                    depends_on: vec![],
                }],
            };
            let wf = Workflow::new(def).unwrap();
            let result = coord.execute(&wf).unwrap();
            assert_eq!(result, WorkflowState::Succeeded);
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn scale_broker_1m_messages() {
    // Scale pass: 1 sub, 1M messages. Validates no unbounded allocation panics.
    let (delivered, expected) = run_broker_fanout(1, 1_000_000);
    assert_eq!(delivered, expected);
}

#[test]
fn scale_voxel_world_10k_chunks() {
    use voxel_ref333::*;
    let store: std::sync::Arc<content333::InMemoryBlockStore> =
        std::sync::Arc::new(content333::InMemoryBlockStore::new());
    let chunks = ChunkStore::new(store);
    let world = WorldState::new(chunks);
    // Place 10k blocks across 10k chunks (one per chunk).
    for i in 0..10_000 {
        let op = BlockPlaceOp {
            actor: identity333::Keypair::generate().node_id(),
            x: i * 32, // force distinct chunks (CHUNK_DIM=16)
            y: 0,
            z: 0,
            kind: 1,
        };
        world.apply_committed(&op).unwrap();
    }
    // Sanity: each placement created a chunk, and random lookup is non-zero.
    assert_eq!(world.voxel(5000 * 32, 0, 0).unwrap(), 1);
}
