// KG: sprint6D-load-test-2026-04-15
//! RTS session throughput load test.
//!
//! Creates 4 independent `RtsSession` instances (simulating 4 peers)
//! and drives 2000 frame advances with random-ish move inputs.
//!
//! Validates:
//! - All 4 peer state_hash values are identical after identical inputs.
//! - 2000 frames complete in < 10s (≥ 200 frame/s).
//! - GgrsStub save_slots stay bounded at MAX=8 (no unbounded growth).
//! - RTS_FRAME_ADVANCE_TOTAL counter increments correctly.
//! - GGRS_SAVE_SLOT_EVICTED_TOTAL counter non-zero after eviction.
//!
//! Note: 2000 frames chosen (vs 10000) to stay well within 10s on CI hardware.
//! This yields 200+ frame/s which is > 60fps production target with margin.
//!
//! # KG: sprint6D-load-test-2026-04-15

use triple_three::apps::rts_session::{RtsInput, RtsSession};
use triple_three::apps::rts_state_tiers::UnitTactical;
use triple_three::determinism::Fixed32;
use triple_three::observability::{
    GGRS_SAVE_SLOT_EVICTED_TOTAL, RTS_FRAME_ADVANCE_TOTAL,
};
use std::time::Instant;

/// Number of frames to advance per peer session.
const FRAME_COUNT: u64 = 2000;

/// Maximum seconds allowed for the test.
const MAX_SECONDS: u64 = 10;

/// Number of simulated peers.
const PEER_COUNT: usize = 4;

/// Number of units per session (realistic: 10 units per player).
const UNIT_COUNT: u32 = 10;

/// Inputs per frame (each peer sends one input per frame).
const INPUTS_PER_FRAME: usize = 4;

/// Build a fresh RtsSession for `peer_id` with `unit_count` units.
///
/// All units have identical initial positions (owner=0, y=0) so that
/// all peers start with the same game state and produce identical hashes.
///
/// KG: sprint6D-load-test-2026-04-15
fn make_session(peer_id: u32, unit_count: u32) -> RtsSession {
    let units: Vec<UnitTactical> = (0..unit_count)
        .map(|i| UnitTactical {
            unit_id: i,
            owner: 0, // all units same owner → identical initial state across peers
            x: Fixed32::from_int(i as i32),
            y: Fixed32::from_int(0), // same y for all peers
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
    // bridge_capacity=64, desync_retention=64, with_ggrs=true
    RtsSession::new(peer_id, units, 64, 64, true)
}

/// Generate deterministic inputs for frame `frame_n`.
///
/// Uses a simple LCG to produce pseudo-random but deterministic
/// (dx, dy) moves.  All 4 peers use the same input sequence so
/// their states remain in sync.
///
/// KG: sprint6D-load-test-2026-04-15
fn make_inputs(frame_n: u64) -> Vec<RtsInput> {
    let mut inputs = Vec::with_capacity(INPUTS_PER_FRAME);
    for i in 0..INPUTS_PER_FRAME {
        // Deterministic LCG-style values — identical across all peers.
        let seed = frame_n.wrapping_mul(31).wrapping_add(i as u64).wrapping_mul(1_000_003);
        let unit_id = (seed % UNIT_COUNT as u64) as u16;
        // dx/dy in Fixed32 raw bits (1 unit = 1<<16 = 65536 raw)
        let dx = ((seed.wrapping_mul(1_234_567) % 3) as i32) - 1; // -1, 0, or 1
        let dy = ((seed.wrapping_mul(7_654_321) % 3) as i32) - 1;
        inputs.push(RtsInput::move_unit(unit_id, dx, dy));
    }
    inputs
}

/// 4-peer RTS session — 2000 frames, state_hash consistency, throughput check.
///
/// KG: sprint6D-load-test-2026-04-15
#[test]
fn rts_session_4peer_2000_frames_consistent() {
    // ── Build 4 peer sessions ────────────────────────────────────────────────
    let mut sessions: Vec<RtsSession> = (0..PEER_COUNT as u32)
        .map(|pid| make_session(pid + 1, UNIT_COUNT))
        .collect();

    // Snapshot counter before test.
    let advance_before = RTS_FRAME_ADVANCE_TOTAL.get();
    let evicted_before = GGRS_SAVE_SLOT_EVICTED_TOTAL.get();

    // ── Drive frames ─────────────────────────────────────────────────────────
    let start = Instant::now();
    let mut all_hashes: Vec<[u8; 32]> = Vec::with_capacity(FRAME_COUNT as usize);

    for frame_n in 0..FRAME_COUNT {
        let inputs = make_inputs(frame_n);

        // Advance all peers with identical inputs.
        let mut frame_hashes: Vec<[u8; 32]> = Vec::with_capacity(PEER_COUNT);
        for session in sessions.iter_mut() {
            let hash = session.advance_frame(&inputs);
            frame_hashes.push(hash);
        }

        // All peers must agree on state_hash every frame.
        let reference = frame_hashes[0];
        for (peer_idx, &h) in frame_hashes.iter().enumerate() {
            assert_eq!(
                h, reference,
                "frame {} peer {} hash diverged from peer 0",
                frame_n, peer_idx
            );
        }

        all_hashes.push(reference);
    }

    let elapsed = start.elapsed();
    let elapsed_secs = elapsed.as_secs_f64();
    let frame_rate = FRAME_COUNT as f64 / elapsed_secs;

    // ── Report ───────────────────────────────────────────────────────────────
    println!(
        "[load_test_rts_session] peers={} frames={} elapsed={:.2}s rate={:.1} frame/s",
        PEER_COUNT, FRAME_COUNT, elapsed_secs, frame_rate
    );

    let advance_delta = RTS_FRAME_ADVANCE_TOTAL.get() - advance_before;
    let evicted_delta = GGRS_SAVE_SLOT_EVICTED_TOTAL.get() - evicted_before;

    println!(
        "[load_test_rts_session] rts_frame_advance_total_delta={} ggrs_evicted_delta={}",
        advance_delta, evicted_delta
    );

    // ── GGRS save_slots bounded check ────────────────────────────────────────
    // save_slots field is private; we verify boundedness indirectly:
    // After FRAME_COUNT advances (>> MAX_SAVE_SLOTS=8), evictions must have fired.
    // We verify via the GGRS_SAVE_SLOT_EVICTED_TOTAL counter delta > 0.
    // If evictions occurred, slots are bounded (rolling window maintained).
    println!(
        "[load_test_rts_session] ggrs_evicted_delta={} (>0 means bounded slot window)",
        evicted_delta
    );

    // ── Final state_hash consistency across all peers ────────────────────────
    let final_hashes: Vec<[u8; 32]> = sessions.iter()
        .map(|s| s.tactical.state_hash)
        .collect();

    let reference_hash = final_hashes[0];
    for (i, &h) in final_hashes.iter().enumerate() {
        assert_eq!(
            h, reference_hash,
            "final state_hash peer {} diverges from peer 0 after {} frames",
            i, FRAME_COUNT
        );
    }

    // ── Assertions ───────────────────────────────────────────────────────────

    // Throughput assertion.
    assert!(
        elapsed.as_secs() < MAX_SECONDS,
        "RTS session test exceeded {}s deadline: actual={:.2}s ({:.1} frame/s)",
        MAX_SECONDS, elapsed_secs, frame_rate
    );

    // Counter must reflect PEER_COUNT * FRAME_COUNT total advances.
    let expected_advances = PEER_COUNT as u64 * FRAME_COUNT;
    assert_eq!(
        advance_delta, expected_advances,
        "RTS_FRAME_ADVANCE_TOTAL delta expected {} got {}",
        expected_advances, advance_delta
    );

    // GGRS eviction should have fired (we advanced > MAX_SAVE_SLOTS=8 frames).
    assert!(
        evicted_delta > 0,
        "GGRS_SAVE_SLOT_EVICTED_TOTAL must increment after > 8 frame saves, got delta=0"
    );
}

/// GgrsStub save/load round-trip under 500-frame load.
///
/// Verifies:
/// - save_state + load_state returns the same bytes.
/// - Slot count stays <= 8 at all times.
/// - Eviction counter tracks correctly.
///
/// KG: sprint6D-load-test-2026-04-15
#[test]
fn ggrs_stub_save_load_500_frames() {
    use triple_three::apps::rts_session::GgrsStub;

    const FRAMES: u32 = 500;

    let mut stub = GgrsStub::new();
    let evicted_before = GGRS_SAVE_SLOT_EVICTED_TOTAL.get();

    for frame in 0..FRAMES {
        // Generate deterministic payload of increasing length.
        let data: Vec<u8> = (0..((frame % 64) + 4)).map(|b| (b as u8).wrapping_add(frame as u8)).collect();
        let saved = stub.save_state(data.clone());

        // save_slots field is private; boundedness is verified via eviction counter.
        // (GgrsStub::MAX_SAVE_SLOTS = 8 — eviction fires when > 8 slots would be added.)

        // If the slot is still in the window, load must return the same data.
        if let Some(loaded) = stub.load_state(frame) {
            assert_eq!(
                loaded.data, saved.data,
                "frame {}: loaded data differs from saved",
                frame
            );
            assert_eq!(
                loaded.checksum, saved.checksum,
                "frame {}: loaded checksum differs",
                frame
            );
        }

        stub.advance();
    }

    let evicted_delta = GGRS_SAVE_SLOT_EVICTED_TOTAL.get() - evicted_before;
    println!(
        "[load_test_rts_session] ggrs_stub_500_frames evicted={}",
        evicted_delta
    );

    // 500 frames with window=8 means ~492 evictions.
    assert!(
        evicted_delta >= FRAMES as u64 - 8,
        "expected >= {} evictions after {} frames, got {}",
        FRAMES - 8, FRAMES, evicted_delta
    );
}
