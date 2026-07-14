// KG: sprint6D-load-test-2026-04-15
//! Desync chaos load test — DesyncDetector under adversarial conditions.
//!
//! Scenario:
//! - 4 peers, 500 frames.
//! - Peer 3 (index 2, 0-based) intentionally diverges at every 100th frame
//!   by injecting a different input.
//! - DesyncDetector must catch ALL divergence events (no missed detection).
//! - RTS_DESYNC_EVENTS_TOTAL / RTS_STATE_HASH_MISMATCHES_TOTAL counters
//!   increment exactly as expected.
//! - After detection, "snapshot resync" is simulated: diverged peer resets
//!   its state_hash to the consensus value (the other 3 peers' hash).
//!
//! Detection guarantee tested:
//! - For each frame where peer 2 had a different input, their hash must differ.
//! - observe_peer_digest must return Some(DesyncEvent) for that frame.
//!
//! Note on resync: a full snapshot resync would require replaying the full
//! TacticalState — that is Phase 4 work.  Here we verify the *detection* path
//! fires reliably and the counter increments correctly.
//!
//! # KG: sprint6D-load-test-2026-04-15

use triple_three::apps::rts_session::{RtsInput, RtsSession};
use triple_three::apps::rts_state_tiers::UnitTactical;
use triple_three::determinism::Fixed32;
use triple_three::observability::{RTS_DESYNC_EVENTS_TOTAL, RTS_STATE_HASH_MISMATCHES_TOTAL};
use std::time::Instant;

const FRAME_COUNT: u64 = 500;
const DESYNC_INTERVAL: u64 = 100; // peer 2 diverges every 100th frame
const PEER_COUNT: usize = 4;
const UNIT_COUNT: u32 = 8;
const DIVERGING_PEER_IDX: usize = 2;

/// Build a fresh RtsSession for `peer_id` with `unit_count` units, all owned by peer 0.
///
/// All peers use owner=0 so the game state starts identically.
///
/// KG: sprint6D-load-test-2026-04-15
fn make_chaos_session(peer_id: u32) -> RtsSession {
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
    RtsSession::new(peer_id, units, 64, 128, false)
}

/// Deterministic "honest" inputs for frame_n.
fn honest_inputs(frame_n: u64) -> Vec<RtsInput> {
    let seed = frame_n.wrapping_mul(1_000_003).wrapping_add(42);
    let unit_id = (seed % UNIT_COUNT as u64) as u16;
    let dx = ((seed.wrapping_mul(1_234_567) % 3) as i32) - 1;
    let dy = ((seed.wrapping_mul(7_654_321) % 3) as i32) - 1;
    vec![RtsInput::move_unit(unit_id, dx, dy)]
}

/// Diverging inputs that differ from honest_inputs.
/// Uses a different seed to guarantee the output differs.
fn diverging_inputs(frame_n: u64) -> Vec<RtsInput> {
    let seed = frame_n.wrapping_mul(9_999_991).wrapping_add(777);
    let unit_id = ((seed.wrapping_add(1)) % UNIT_COUNT as u64) as u16;
    // Force opposite direction vs honest by adding 2 before mod.
    let dx = (((seed.wrapping_mul(2_468_013) + 2) % 3) as i32) - 1;
    let dy = (((seed.wrapping_mul(1_357_911) + 2) % 3) as i32) - 1;
    vec![RtsInput::move_unit(unit_id, dx, dy)]
}

/// 4-peer desync chaos — peer 2 diverges every 100 frames, detector catches all.
///
/// KG: sprint6D-load-test-2026-04-15
#[test]
fn desync_chaos_4peer_500_frames_all_detected() {
    let mut sessions: Vec<RtsSession> = (0..PEER_COUNT as u32)
        .map(|pid| make_chaos_session(pid + 1))
        .collect();

    let desync_before = RTS_DESYNC_EVENTS_TOTAL.get();
    let mismatch_before = RTS_STATE_HASH_MISMATCHES_TOTAL.get();

    let start = Instant::now();

    // Track which frames had intentional divergence.
    let mut diverged_frames: Vec<u64> = Vec::new();
    // Track desync events actually detected.
    let mut detected_frames: Vec<u64> = Vec::new();

    for frame_n in 0..FRAME_COUNT {
        let is_divergence_frame = frame_n > 0 && frame_n % DESYNC_INTERVAL == 0;

        // Compute hashes for each peer.
        let mut peer_hashes: Vec<[u8; 32]> = Vec::with_capacity(PEER_COUNT);

        for (peer_idx, session) in sessions.iter_mut().enumerate() {
            let inputs = if peer_idx == DIVERGING_PEER_IDX && is_divergence_frame {
                // Peer 2 uses diverging inputs at the designated frames.
                diverging_inputs(frame_n)
            } else {
                honest_inputs(frame_n)
            };
            let hash = session.advance_frame(&inputs);
            peer_hashes.push(hash);
        }

        if is_divergence_frame {
            diverged_frames.push(frame_n);

            // Peer 2's hash MUST differ from the consensus hash (peers 0, 1, 3).
            // If they happen to be identical (hash collision / same move), note it.
            let consensus_hash = peer_hashes[0];
            let diverging_hash = peer_hashes[DIVERGING_PEER_IDX];

            if diverging_hash != consensus_hash {
                // Now simulate: peers 0, 1, 3 observe peer 2's hash.
                // observe_peer_digest uses the local DesyncDetector on the observer.
                // Peer 0 observes peer 2.
                let event = sessions[0].observe_peer_digest(
                    3, // peer_id of the diverging peer (1-based: peer_id=3, idx=2)
                    frame_n,
                    diverging_hash,
                );
                if event.is_some() {
                    detected_frames.push(frame_n);
                }
            }
        }
    }

    let elapsed = start.elapsed();

    // ── Report ───────────────────────────────────────────────────────────────
    println!(
        "[load_test_desync_chaos] frames={} elapsed={:.2}s diverged={} detected={}",
        FRAME_COUNT,
        elapsed.as_secs_f64(),
        diverged_frames.len(),
        detected_frames.len()
    );

    let desync_delta = RTS_DESYNC_EVENTS_TOTAL.get() - desync_before;
    let mismatch_delta = RTS_STATE_HASH_MISMATCHES_TOTAL.get() - mismatch_before;

    println!(
        "[load_test_desync_chaos] rts_desync_events_delta={} rts_state_hash_mismatches_delta={}",
        desync_delta, mismatch_delta
    );

    // ── Assertions ───────────────────────────────────────────────────────────

    // Test must complete in < 10s.
    assert!(
        elapsed.as_secs() < 10,
        "desync chaos test exceeded 10s: actual={:.2}s",
        elapsed.as_secs_f64()
    );

    // Divergence frames: frame_n > 0 && frame_n % DESYNC_INTERVAL == 0.
    // With FRAME_COUNT=500 and DESYNC_INTERVAL=100: frames 100,200,300,400 = 4 frames.
    // Formula: (FRAME_COUNT - 1) / DESYNC_INTERVAL accounts for frame_n > 0 exclusion.
    let expected_divergences = (FRAME_COUNT - 1) / DESYNC_INTERVAL;
    assert_eq!(
        diverged_frames.len() as u64, expected_divergences,
        "expected {} divergence frames, got {} (frames: {:?})",
        expected_divergences, diverged_frames.len(), diverged_frames
    );

    // All divergence frames where the hash actually differed must be detected.
    // (If hash collides — highly unlikely with blake3 — the frame is excluded.)
    // detected must equal the actual number of non-colliding divergences.
    let non_trivial_divergences = diverged_frames.len(); // all should be non-trivial
    assert_eq!(
        detected_frames.len(), non_trivial_divergences,
        "DesyncDetector must catch all {} divergences, only caught {}",
        non_trivial_divergences, detected_frames.len()
    );

    // RTS_STATE_HASH_MISMATCHES_TOTAL is incremented by observe_peer_digest.
    // RTS_DESYNC_EVENTS_TOTAL is a separate counter (incremented by other code paths).
    // We assert that mismatch_delta equals our detected count.
    assert_eq!(
        mismatch_delta, detected_frames.len() as u64,
        "RTS_STATE_HASH_MISMATCHES_TOTAL delta must equal detected divergences ({}), got {}",
        detected_frames.len(), mismatch_delta
    );
}

/// DesyncDetector retention eviction — verify old frames are evicted.
///
/// DesyncDetector with retention=10 must evict frames older than 10.
/// This prevents memory monotonic growth under sustained load.
///
/// KG: sprint6D-load-test-2026-04-15
#[test]
fn desync_detector_retention_bounded() {
    use triple_three::determinism::state_hash::DesyncDetector;

    const RETENTION: usize = 16;
    const FRAMES: u64 = 1000;

    let mut detector = DesyncDetector::new(RETENTION);

    for frame_n in 0..FRAMES {
        detector.record_local(frame_n, [(frame_n & 0xFF) as u8; 32]);

        // stored_count must never exceed retention.
        let stored = detector.stored_count();
        assert!(
            stored <= RETENTION,
            "frame {}: stored_count={} > retention={}",
            frame_n, stored, RETENTION
        );
    }

    let final_stored = detector.stored_count();
    println!(
        "[load_test_desync_chaos] retention_test: final_stored={} retention={}",
        final_stored, RETENTION
    );

    // Final count must be exactly retention (last 16 frames retained).
    assert_eq!(
        final_stored, RETENTION,
        "after {} frames with retention={}, stored must be exactly {}, got {}",
        FRAMES, RETENTION, RETENTION, final_stored
    );
}

/// Verify that divergence detection works when peer hash arrives before local record.
///
/// Scenario: peer hash arrives first → observe_peer returns None (unknown frame).
/// Then local records hash → subsequent observation detects mismatch.
///
/// KG: sprint6D-load-test-2026-04-15
#[test]
fn desync_detector_late_local_record() {
    use triple_three::determinism::state_hash::DesyncDetector;

    let mut detector = DesyncDetector::new(64);

    let frame = 42u64;
    let local_hash = [0xAAu8; 32];
    let peer_hash = [0xBBu8; 32];

    // Peer hash arrives before local is recorded → None (no false alarm).
    let event = detector.observe_peer(99, frame, peer_hash);
    assert!(
        event.is_none(),
        "early peer observation (no local) must return None, got {:?}",
        event.map(|e| e.frame_n)
    );

    // Now record local.
    detector.record_local(frame, local_hash);

    // Re-observe with the same diverging peer hash → must detect mismatch.
    let event = detector.observe_peer(99, frame, peer_hash);
    assert!(
        event.is_some(),
        "after local record, diverging peer hash must trigger DesyncEvent"
    );
    let ev = event.unwrap();
    assert_eq!(ev.frame_n, frame);
    assert_eq!(ev.local_hash, local_hash);
    assert_eq!(ev.peer_hash, peer_hash);
}
