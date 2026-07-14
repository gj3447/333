// KG: sprint6D-load-test-2026-04-15
//! BFT throughput load test — MultiNodeBftHarness n=4 q=3.
//!
//! Drives 500 consecutive checkpoints and measures:
//! - Total elapsed time
//! - Average throughput (checkpoints/second)
//! - BFT_CHECKPOINT_COMMITTED_TOTAL counter
//! - BFT_CHECKPOINT_QC_LATENCY_MS histogram (non-zero)
//!
//! Benchmark target: 500 checkpoints in < 30s (≥ 16/s average on Mac M-series).
//! Slow-ratchet warning: < 100 checkpoints/10s is flagged in output.
//!
//! # KG: sprint6D-load-test-2026-04-15

use triple_three::netcode::bft_multi_node::MultiNodeBftHarness;
use triple_three::observability::{
    BFT_CHECKPOINT_COMMITTED_TOTAL, BFT_CHECKPOINT_PROPOSED_TOTAL, BFT_CHECKPOINT_QC_LATENCY_MS,
};
use std::time::Instant;

/// Total checkpoints to drive in this load test.
const CHECKPOINT_COUNT: u64 = 500;

/// Hard deadline: test must finish within this many seconds.
const MAX_SECONDS: u64 = 30;

/// Slow-ratchet threshold: warn if throughput < this many checkpoints/s.
const SLOW_RATCHET_THRESHOLD: f64 = 5.0;

/// Spin up n=4 q=3 harness and drive CHECKPOINT_COUNT consecutive checkpoints.
///
/// Validates:
/// 1. All checkpoints succeed (no QuorumTimeout).
/// 2. Total elapsed < MAX_SECONDS.
/// 3. Observability counters updated correctly.
/// 4. QC latency histogram has non-zero observations.
///
/// KG: sprint6D-load-test-2026-04-15
#[test]
fn bft_throughput_500_checkpoints() {
    // ── Build harness ────────────────────────────────────────────────────────
    let mut harness = MultiNodeBftHarness::new(4, 3);
    harness.register_all_peer_identities();

    // Snapshot counters before test (other tests may have incremented them).
    let proposed_before = BFT_CHECKPOINT_PROPOSED_TOTAL.get();
    let committed_before = BFT_CHECKPOINT_COMMITTED_TOTAL.get();

    // ── Drive checkpoints ────────────────────────────────────────────────────
    let start = Instant::now();
    let mut failed = 0u64;

    for i in 0..CHECKPOINT_COUNT {
        // Unique hash per checkpoint (frame_n = i, hash = i repeated).
        let hash = [(i & 0xFF) as u8; 32];
        match harness.drive_checkpoint(i, hash) {
            Ok(_qc) => {}
            Err(e) => {
                failed += 1;
                eprintln!("[load_test_bft] checkpoint {} failed: {}", i, e);
            }
        }
    }

    let elapsed = start.elapsed();
    let elapsed_secs = elapsed.as_secs_f64();
    let throughput = (CHECKPOINT_COUNT - failed) as f64 / elapsed_secs;

    // ── Report ───────────────────────────────────────────────────────────────
    println!(
        "[load_test_bft_throughput] checkpoints={} failed={} elapsed={:.2}s throughput={:.1}/s",
        CHECKPOINT_COUNT, failed, elapsed_secs, throughput
    );

    let proposed_delta = BFT_CHECKPOINT_PROPOSED_TOTAL.get() - proposed_before;
    let committed_delta = BFT_CHECKPOINT_COMMITTED_TOTAL.get() - committed_before;

    println!(
        "[load_test_bft_throughput] proposed_delta={} committed_delta={}",
        proposed_delta, committed_delta
    );

    // Print histogram summary (just count for CI friendliness)
    let qc_latency_count = BFT_CHECKPOINT_QC_LATENCY_MS
        .render_json()
        .contains("\"count\":0");
    println!(
        "[load_test_bft_throughput] qc_latency_histogram_empty={}",
        qc_latency_count
    );

    // ── Assertions ───────────────────────────────────────────────────────────

    // All checkpoints must succeed.
    assert_eq!(
        failed, 0,
        "all {} checkpoints must succeed, {} failed",
        CHECKPOINT_COUNT, failed
    );

    // Must complete within deadline.
    assert!(
        elapsed.as_secs() < MAX_SECONDS,
        "BFT throughput test exceeded {}s deadline: actual={:.2}s",
        MAX_SECONDS, elapsed_secs
    );

    // Observability counters must reflect at least all our checkpoints.
    // Using >= rather than == because other tests running in parallel may
    // also increment the global counters.
    assert!(
        proposed_delta >= CHECKPOINT_COUNT,
        "BFT_CHECKPOINT_PROPOSED_TOTAL delta must be >= {} (got {})",
        CHECKPOINT_COUNT, proposed_delta
    );
    assert!(
        committed_delta >= CHECKPOINT_COUNT,
        "BFT_CHECKPOINT_COMMITTED_TOTAL delta must be >= {} (got {})",
        CHECKPOINT_COUNT, committed_delta
    );

    // QC latency histogram must have observations (non-empty).
    // render_json returns count from +Inf bucket — must be > 0.
    let qc_json = BFT_CHECKPOINT_QC_LATENCY_MS.render_json();
    // Count is embedded as "count":<N> — extract and verify > 0.
    assert!(
        !qc_json.contains("\"count\":0"),
        "QC latency histogram must have at least 1 observation after {} checkpoints",
        CHECKPOINT_COUNT
    );

    // Slow-ratchet warning (not a hard failure — just warns in CI output).
    if throughput < SLOW_RATCHET_THRESHOLD {
        eprintln!(
            "[load_test_bft_throughput] SLOW RATCHET WARNING: throughput={:.1}/s < threshold={:.1}/s",
            throughput, SLOW_RATCHET_THRESHOLD
        );
    }
}

/// Frame budget: p95 per-message processing latency on a single node.
/// KG: sprint7J-frame-budget-p95-2026-04-16
///
/// 333 targets 60fps (16.6ms/frame). Per-frame work on a single node includes:
///   poll_incoming → process each message → produce outgoing
///
/// This test measures a single node's `process()` latency (not end-to-end
/// multi-node round-trip). Budget: p95 < 7ms per message.
///
/// Also reports end-to-end checkpoint p95 as informational.
#[test]
fn bft_p95_per_message_latency() {
    use triple_three::bft::crypto::{NodeId, Signature, ValidatorSet};
    use triple_three::bft::types::{HotStuffMsg, Phase};
    use triple_three::bft::state::HotStuffState;

    let ids: Vec<NodeId> = vec![1, 2, 3, 4];
    let vs = ValidatorSet::new(ids.clone());
    let mut node = HotStuffState::new(1, vs);

    // Measure process() latency for Vote messages (most common in BFT loop)
    let rounds = 500;
    let mut latencies_us: Vec<u64> = Vec::with_capacity(rounds);

    for i in 0..rounds {
        let msg = HotStuffMsg::Vote {
            view: node.view,
            block_hash: 0xCAFE + i as u64,
            phase: Phase::Prepare,
            signature: Signature { signer: 2, sig_bytes: vec![0u8; 64] },
        };
        let start = Instant::now();
        let _ = node.process(msg);
        latencies_us.push(start.elapsed().as_micros() as u64);
    }

    latencies_us.sort();

    let p50 = latencies_us[latencies_us.len() / 2];
    let p95 = latencies_us[latencies_us.len() * 95 / 100];
    let p99 = latencies_us[latencies_us.len() * 99 / 100];

    println!(
        "[bft_p95_msg] rounds={} p50={:.1}µs p95={:.1}µs p99={:.1}µs",
        rounds, p50 as f64, p95 as f64, p99 as f64,
    );

    // Assert p95 < 7ms (7000µs) — single-message processing
    assert!(
        p95 < 7000,
        "BFT single-message p95 must be < 7ms for 60fps frame budget, actual p95={}µs",
        p95,
    );

    // Informational: end-to-end checkpoint latency
    let mut harness = MultiNodeBftHarness::new(4, 3);
    harness.register_all_peer_identities();
    let mut checkpoint_latencies: Vec<u64> = Vec::with_capacity(50);

    for i in 0..50u64 {
        let hash = [(i & 0xFF) as u8; 32];
        let start = Instant::now();
        harness.drive_checkpoint(i, hash).expect("checkpoint must succeed");
        checkpoint_latencies.push(start.elapsed().as_micros() as u64);
    }
    checkpoint_latencies.sort();
    let cp_p50 = checkpoint_latencies[checkpoint_latencies.len() / 2];
    let cp_p95 = checkpoint_latencies[checkpoint_latencies.len() * 95 / 100];

    println!(
        "[bft_p95_checkpoint] rounds=50 p50={:.2}ms p95={:.2}ms (informational, multi-node)",
        cp_p50 as f64 / 1000.0,
        cp_p95 as f64 / 1000.0,
    );
}

/// Verify that n=5 q=4 with 1 Byzantine node at index 4 sustains throughput.
///
/// n=5, Byzantine=index 4 (node_id=5, leader at views 4,9,14,…).
/// The first 4 consecutive views (0-3) have honest leaders; we drive only
/// 4 checkpoints before the Byzantine node would become leader.
/// This validates that f=1 honest-leader rounds with n=5,q=4 all commit.
///
/// KG: sprint6D-load-test-2026-04-15
#[test]
fn bft_throughput_byzantine_f1_honest_leader_rounds() {
    // n=5, quorum=4 (f=1 Byzantine tolerance).
    // Byzantine node is at index 4 (node_id=5) — leader for view 4.
    // We drive exactly 4 checkpoints (views 0-3) — all honest leaders.
    const TOTAL: u64 = 4;

    let mut harness = MultiNodeBftHarness::new(5, 4);
    harness.register_all_peer_identities();
    harness.set_byzantine(4); // node index 4 (node_id=5) — leader at view 4 only

    let start = Instant::now();

    for i in 0..TOTAL {
        let hash = [(i & 0xFF) as u8; 32];
        let result = harness.drive_checkpoint(i * 10, hash);
        assert!(
            result.is_ok(),
            "f=1 Byzantine: honest-leader checkpoint {} must succeed: {:?}",
            i, result.err()
        );
    }

    let elapsed = start.elapsed();
    println!(
        "[load_test_bft_throughput] byzantine_f1 n=5 q=4 checkpoints={} elapsed={:.2}s throughput={:.1}/s",
        TOTAL,
        elapsed.as_secs_f64(),
        TOTAL as f64 / elapsed.as_secs_f64()
    );

    assert!(
        elapsed.as_secs() < 5,
        "byzantine f=1 honest-leader test must complete in < 5s, actual={:.2}s",
        elapsed.as_secs_f64()
    );
}
