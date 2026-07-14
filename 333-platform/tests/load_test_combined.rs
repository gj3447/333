// KG: sprint6D-load-test-2026-04-15
//! Combined load test — BFT + RTS + Desync in parallel.
//!
//! Runs three scenarios concurrently using `std::thread`:
//!   1. BFT throughput  — n=4 q=3, 200 checkpoints
//!   2. RTS session     — 4 peers, 500 frames, deterministic inputs
//!   3. Desync detector — 4 peers, 300 frames, peer 2 diverges every 50 frames
//!
//! Total test must complete in < 30s.
//! All core counters must be non-zero after completion.
//! Final observability snapshot is dumped to stdout for CI log.
//!
//! # KG: sprint6D-load-test-2026-04-15

use triple_three::apps::rts_session::{RtsInput, RtsSession};
use triple_three::apps::rts_state_tiers::UnitTactical;
use triple_three::determinism::Fixed32;
use triple_three::netcode::bft_multi_node::MultiNodeBftHarness;
use triple_three::observability::{
    BFT_CHECKPOINT_COMMITTED_TOTAL, BFT_CHECKPOINT_PROPOSED_TOTAL,
    GGRS_SAVE_SLOT_EVICTED_TOTAL, RTS_DESYNC_EVENTS_TOTAL,
    RTS_FRAME_ADVANCE_TOTAL, RTS_STATE_HASH_MISMATCHES_TOTAL,
};
use std::time::Instant;

// ── Scenario parameters ───────────────────────────────────────────────────────

const BFT_CHECKPOINTS: u64 = 200;
const RTS_FRAMES: u64 = 500;
const DESYNC_FRAMES: u64 = 300;
const DESYNC_INTERVAL: u64 = 50;
const UNIT_COUNT: u32 = 8;

/// Maximum allowed wall-clock time for the entire combined test.
const MAX_SECONDS: u64 = 30;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_session_combined(peer_id: u32) -> RtsSession {
    let units: Vec<UnitTactical> = (0..UNIT_COUNT)
        .map(|i| UnitTactical {
            unit_id: i,
            owner: 0,
            x: Fixed32::from_int(i as i32),
            y: Fixed32::from_int(0),
            hp: 100,
            max_hp: 100,
            animation_frame: 0,
            ability_cooldown: 0,
            attack_range: Fixed32::from_int(10),
            attack_damage: 10,
            attack_cooldown_max: 3,
            attack_cooldown_remaining: 0,
        })
        .collect();
    RtsSession::new(peer_id, units, 64, 64, true)
}

fn make_move_input(frame_n: u64, peer_idx: usize) -> RtsInput {
    let seed = frame_n
        .wrapping_mul(1_000_003)
        .wrapping_add(peer_idx as u64 * 7)
        .wrapping_mul(999_983);
    let unit_id = (seed % UNIT_COUNT as u64) as u16;
    let dx = ((seed.wrapping_mul(1_234_567) % 3) as i32) - 1;
    let dy = ((seed.wrapping_mul(7_654_321) % 3) as i32) - 1;
    RtsInput::move_unit(unit_id, dx, dy)
}

fn make_diverging_input(frame_n: u64) -> RtsInput {
    let seed = frame_n.wrapping_mul(8_888_881).wrapping_add(13_131_313);
    let unit_id = ((seed + 1) % UNIT_COUNT as u64) as u16;
    let dx = (((seed.wrapping_mul(2_222_221) + 2) % 3) as i32) - 1;
    let dy = (((seed.wrapping_mul(3_333_337) + 2) % 3) as i32) - 1;
    RtsInput::move_unit(unit_id, dx, dy)
}

// ── Scenario A: BFT throughput ────────────────────────────────────────────────

/// Drive BFT_CHECKPOINTS checkpoints on n=4 q=3 harness.
/// Returns (success_count, elapsed_ms).
fn scenario_bft() -> (u64, f64) {
    let mut harness = MultiNodeBftHarness::new(4, 3);
    harness.register_all_peer_identities();

    let start = Instant::now();
    let mut succeeded = 0u64;

    for i in 0..BFT_CHECKPOINTS {
        let hash = [(i & 0xFF) as u8; 32];
        if harness.drive_checkpoint(i, hash).is_ok() {
            succeeded += 1;
        }
    }

    (succeeded, start.elapsed().as_secs_f64() * 1000.0)
}

// ── Scenario B: RTS session throughput ───────────────────────────────────────

/// Drive RTS_FRAMES on 4 peer sessions with consistent inputs.
/// Returns (frames_completed, all_hashes_consistent, elapsed_ms).
fn scenario_rts() -> (u64, bool, f64) {
    let mut sessions: Vec<RtsSession> = (1..=4u32)
        .map(make_session_combined)
        .collect();

    let start = Instant::now();
    let mut consistent = true;

    for frame_n in 0..RTS_FRAMES {
        let mut hashes = Vec::with_capacity(4);
        for (pidx, session) in sessions.iter_mut().enumerate() {
            let input = make_move_input(frame_n, pidx % 4);
            // Use same input for all peers (deterministic consistency).
            let h = session.advance_frame(&[make_move_input(frame_n, 0)]);
            hashes.push(h);
            let _ = input; // pidx-based variant kept for reference
        }
        let reference = hashes[0];
        if hashes.iter().any(|&h| h != reference) {
            consistent = false;
        }
    }

    (RTS_FRAMES, consistent, start.elapsed().as_secs_f64() * 1000.0)
}

// ── Scenario C: Desync detection ─────────────────────────────────────────────

/// Drive DESYNC_FRAMES on 4 peer sessions; peer 2 diverges every DESYNC_INTERVAL.
/// Returns (detected_count, expected_count, elapsed_ms).
fn scenario_desync() -> (usize, usize, f64) {
    let mut sessions: Vec<RtsSession> = (1..=4u32)
        .map(make_session_combined)
        .collect();

    let start = Instant::now();
    let mut detected = 0usize;
    let mut expected = 0usize;

    for frame_n in 0..DESYNC_FRAMES {
        let is_diverge = frame_n > 0 && frame_n % DESYNC_INTERVAL == 0;

        let mut hashes: Vec<[u8; 32]> = Vec::with_capacity(4);
        for (pidx, session) in sessions.iter_mut().enumerate() {
            let input = if pidx == 2 && is_diverge {
                make_diverging_input(frame_n)
            } else {
                make_move_input(frame_n, 0) // same honest input for all others
            };
            let h = session.advance_frame(&[input]);
            hashes.push(h);
        }

        if is_diverge {
            let consensus = hashes[0];
            let diverging = hashes[2];
            if diverging != consensus {
                expected += 1;
                // Peer 0 observes peer 2 (peer_id=3).
                if sessions[0].observe_peer_digest(3, frame_n, diverging).is_some() {
                    detected += 1;
                }
            }
        }
    }

    (detected, expected, start.elapsed().as_secs_f64() * 1000.0)
}

// ── Combined parallel test ────────────────────────────────────────────────────

/// Run all three scenarios in parallel threads and verify combined metrics.
///
/// KG: sprint6D-load-test-2026-04-15
#[test]
fn combined_bft_rts_desync_parallel() {
    // Snapshot counters before.
    let proposed_before = BFT_CHECKPOINT_PROPOSED_TOTAL.get();
    let committed_before = BFT_CHECKPOINT_COMMITTED_TOTAL.get();
    let advance_before = RTS_FRAME_ADVANCE_TOTAL.get();
    let evicted_before = GGRS_SAVE_SLOT_EVICTED_TOTAL.get();
    let desync_before = RTS_DESYNC_EVENTS_TOTAL.get();
    let mismatch_before = RTS_STATE_HASH_MISMATCHES_TOTAL.get();

    let wall_start = Instant::now();

    // Spawn three threads.
    let bft_handle = std::thread::spawn(scenario_bft);
    let rts_handle = std::thread::spawn(scenario_rts);
    let desync_handle = std::thread::spawn(scenario_desync);

    // Join all threads.
    let (bft_success, bft_ms) = bft_handle.join().expect("BFT scenario panicked");
    let (rts_frames, rts_consistent, rts_ms) = rts_handle.join().expect("RTS scenario panicked");
    let (desync_detected, desync_expected, desync_ms) =
        desync_handle.join().expect("Desync scenario panicked");

    let wall_elapsed = wall_start.elapsed();

    // ── Collect counter deltas ────────────────────────────────────────────────
    let proposed_delta = BFT_CHECKPOINT_PROPOSED_TOTAL.get() - proposed_before;
    let committed_delta = BFT_CHECKPOINT_COMMITTED_TOTAL.get() - committed_before;
    let advance_delta = RTS_FRAME_ADVANCE_TOTAL.get() - advance_before;
    let evicted_delta = GGRS_SAVE_SLOT_EVICTED_TOTAL.get() - evicted_before;
    let desync_delta = RTS_DESYNC_EVENTS_TOTAL.get() - desync_before;
    let mismatch_delta = RTS_STATE_HASH_MISMATCHES_TOTAL.get() - mismatch_before;

    // ── Metrics dump (prometheus format) ─────────────────────────────────────
    println!("\n===== LOAD TEST METRICS SNAPSHOT =====");
    println!(
        "bft_checkpoint_proposed_total_delta {}",
        proposed_delta
    );
    println!(
        "bft_checkpoint_committed_total_delta {}",
        committed_delta
    );
    println!("rts_frame_advance_total_delta {}", advance_delta);
    println!("ggrs_save_slot_evicted_total_delta {}", evicted_delta);
    println!("rts_desync_events_total_delta {}", desync_delta);
    println!("rts_state_hash_mismatches_total_delta {}", mismatch_delta);
    println!();
    println!(
        "scenario_bft:    success={}/{} elapsed={:.0}ms ({:.1}/s)",
        bft_success, BFT_CHECKPOINTS, bft_ms, bft_success as f64 / (bft_ms / 1000.0)
    );
    println!(
        "scenario_rts:    frames={} consistent={} elapsed={:.0}ms ({:.0} fps)",
        rts_frames, rts_consistent, rts_ms, rts_frames as f64 / (rts_ms / 1000.0) * 4.0
    );
    println!(
        "scenario_desync: detected={}/{} elapsed={:.0}ms",
        desync_detected, desync_expected, desync_ms
    );
    println!(
        "wall_elapsed: {:.2}s",
        wall_elapsed.as_secs_f64()
    );
    println!("======================================\n");

    // ── Assertions ───────────────────────────────────────────────────────────

    // Total wall time.
    assert!(
        wall_elapsed.as_secs() < MAX_SECONDS,
        "combined test exceeded {}s deadline: actual={:.2}s",
        MAX_SECONDS, wall_elapsed.as_secs_f64()
    );

    // BFT: all checkpoints must succeed.
    assert_eq!(
        bft_success, BFT_CHECKPOINTS,
        "BFT: expected {} successes, got {}",
        BFT_CHECKPOINTS, bft_success
    );

    // RTS: state must be consistent.
    assert!(
        rts_consistent,
        "RTS: peer state_hashes diverged — determinism broken!"
    );

    // Desync: all expected divergences must be detected.
    assert_eq!(
        desync_detected, desync_expected,
        "Desync: detected {}/{} divergences",
        desync_detected, desync_expected
    );

    // BFT counters non-zero.
    assert!(
        proposed_delta >= BFT_CHECKPOINTS,
        "BFT_CHECKPOINT_PROPOSED_TOTAL delta {} < {}",
        proposed_delta, BFT_CHECKPOINTS
    );
    assert!(
        committed_delta >= BFT_CHECKPOINTS,
        "BFT_CHECKPOINT_COMMITTED_TOTAL delta {} < {}",
        committed_delta, BFT_CHECKPOINTS
    );

    // RTS frame advance counter: at least RTS_FRAMES * 4 (from RTS scenario)
    // plus DESYNC_FRAMES * 4 (from desync scenario).
    let min_advance = RTS_FRAMES * 4 + DESYNC_FRAMES * 4;
    assert!(
        advance_delta >= min_advance,
        "RTS_FRAME_ADVANCE_TOTAL delta {} < expected min {}",
        advance_delta, min_advance
    );

    // GGRS eviction must have fired (> 8 frames saved per session).
    assert!(
        evicted_delta > 0,
        "GGRS_SAVE_SLOT_EVICTED_TOTAL must be non-zero after > 8 frames"
    );

    // Desync mismatch counter must be non-zero if any divergences expected.
    if desync_expected > 0 {
        assert!(
            mismatch_delta > 0,
            "RTS_STATE_HASH_MISMATCHES_TOTAL must be non-zero when divergences detected"
        );
    }
}
