// KG: seed-post-rts-e2e-4peer-sim-2026-04-15
//! E2E 4-peer RTS simulation — Phase 0-6 + Post-RTS A/B/E integration.
//!
//! Three scenarios:
//!   1. green_path      — 100 frames, identical inputs, all hashes match, BFT checkpoint at frame 100
//!   2. desync_detect   — peer 3 injects different input at frame 50; FrameDivergenceDetector fires
//!   3. peer_eject      — peer 2 goes AFK frame 60-70; PeerEjectController ejects; null-input replay
//!
//! Uses only in-process state — no real WebRTC, no real network.
//! MockBftCheckpointProvider drives quorum internally (n=4 quorum=3 simulated).
//!
//! # KG: seed-post-rts-e2e-4peer-sim-2026-04-15

use triple_three::apps::rts_session::{RtsInput, RtsSession};
use triple_three::apps::rts_state_tiers::UnitTactical;
use triple_three::determinism::Fixed32;
use triple_three::netcode::{
    BftCheckpointProvider, CheckpointHandle, FrameDivergenceDetector,
};
use triple_three::netcode::peer_eject::PeerEjectController;
use triple_three::bft::crypto::QuorumCert;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

// ── MockBftCheckpointProvider ────────────────────────────────────────────────
// Simulates n=4 quorum=3 BFT by tracking proposal votes internally.
// Returns Ok(qc) when at least 3 unique "votes" have been cast.
// In-process: each call from any peer counts as one vote.
// KG: seed-post-rts-e2e-4peer-sim-2026-04-15

struct MockBftCheckpointProvider {
    /// proposal frame → vote count
    votes: BTreeMap<u64, u32>,
    quorum: u32,
}

impl MockBftCheckpointProvider {
    fn new(quorum: u32) -> Self {
        Self { votes: BTreeMap::new(), quorum }
    }
}

impl BftCheckpointProvider for MockBftCheckpointProvider {
    fn propose_checkpoint(&mut self, handle: &CheckpointHandle) -> Result<QuorumCert, String> {
        let count = self.votes.entry(handle.frame_n).or_insert(0);
        *count += 1;
        if *count >= self.quorum {
            // Quorum reached — return synthetic QC with block_hash derived from frame
            Ok(QuorumCert {
                block_hash: handle.frame_n,
                view: *count as u64,
                // Synthetic committed-checkpoint QC — a real one is a Commit QC.
                phase: triple_three::bft::types::Phase::Commit,
                signatures: vec![], // genesis-style: block_hash != 0 → accepted by stub verifier
            })
        } else {
            Err(format!(
                "quorum not yet reached: {}/{} votes for frame {}",
                count, self.quorum, handle.frame_n
            ))
        }
    }

    fn verify_checkpoint_qc(&self, handle: &CheckpointHandle, qc: &QuorumCert) -> bool {
        qc.block_hash == handle.frame_n
    }
}

// ── Helper: build 4 identical RtsSessions ───────────────────────────────────
// KG: seed-post-rts-e2e-4peer-sim-2026-04-15

fn make_4_sessions() -> [RtsSession; 4] {
    let units: Vec<UnitTactical> = (0u32..4)
        .map(|i| UnitTactical {
            unit_id: i,
            owner: i % 4,
            // Spread units at 2000-unit intervals so 100+ frames of move +1.0/frame
            // never triggers collision guard (min gap = 2000 - 100 = 1900 units >> UNIT_RADIUS).
            // KG: sprint3-3C-game-rules-graphics-2026-04-15
            x: Fixed32::from_int(i as i32 * 2000),
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

    std::array::from_fn(|i| {
        RtsSession::new(
            (i + 1) as u32,  // node_id 1..=4
            units.clone(),
            128,             // bridge_capacity
            128,             // desync_retention
            false,           // headless mode (no GGRS stub overhead)
        )
    })
}

// ── Scenario 1: green_path ───────────────────────────────────────────────────
// 100 frames, all 4 peers apply identical inputs.
// Every 10 frames → hash broadcast + compare. All must match.
// After 100 frames → MockBft checkpoint proposal from 3 peers → QC succeeds.
// KG: seed-post-rts-e2e-4peer-sim-2026-04-15

#[test]
fn green_path_100_frames_all_hashes_match_bft_checkpoint() {
    let mut sessions = make_4_sessions();
    let input = [RtsInput::move_unit(0, Fixed32::from_int(1).0, 0)];

    for frame in 0u64..100 {
        let hashes: [[u8; 32]; 4] = std::array::from_fn(|i| {
            sessions[i].advance_frame(&input)
        });

        // Every 10 frames, verify all 4 peers agree.
        if (frame + 1) % 10 == 0 {
            let reference = hashes[0];
            for (i, &h) in hashes.iter().enumerate() {
                assert_eq!(
                    h, reference,
                    "green_path: frame {} — peer {} hash mismatch with peer 1",
                    frame + 1,
                    i + 1,
                );
            }
        }
    }

    // BFT checkpoint at frame 100: 3 peers propose → quorum=3 → QC success
    // (Peer 4 stays silent — tests f=1 tolerance)
    let tactical_hash = sessions[0].tactical.state_hash;
    let handle = CheckpointHandle { frame_n: 100, tactical_hash };

    let mut bft = MockBftCheckpointProvider::new(3);
    let r1 = bft.propose_checkpoint(&handle);
    assert!(r1.is_err(), "1 vote: quorum not yet reached");
    let r2 = bft.propose_checkpoint(&handle);
    assert!(r2.is_err(), "2 votes: quorum not yet reached");
    let r3 = bft.propose_checkpoint(&handle);
    assert!(r3.is_ok(), "3 votes: quorum reached, QC must be returned");

    let qc = r3.unwrap();
    assert_eq!(qc.block_hash, 100, "QC block_hash must encode frame number");
    assert!(bft.verify_checkpoint_qc(&handle, &qc), "QC must verify");
}

// ── Scenario 2: desync_detection ────────────────────────────────────────────
// Peers 1,2,4 apply normal input. Peer 3 applies diverged input at frame 50.
// FrameDivergenceDetector on peers 1,2,4 must fire DesyncEvent.
// After resync (restore from peer 1 snapshot) all peers converge again.
// KG: seed-post-rts-e2e-4peer-sim-2026-04-15

#[test]
fn desync_detection_peer3_diverges_at_frame50() {
    let mut sessions = make_4_sessions();

    // Detectors for peers 1,2,4 (indices 0,1,3)
    let mut detectors: [FrameDivergenceDetector; 4] =
        std::array::from_fn(|_| FrameDivergenceDetector::new(128));

    let normal_input = [RtsInput::move_unit(0, Fixed32::from_int(1).0, 0)];
    // Diverge: peer 3 sends noop → unit does not move → different state → different hash
    let diverge_input = [RtsInput::noop()];

    // Frames 0-49: all peers advance identically
    for frame in 0u64..50 {
        let hashes: [[u8; 32]; 4] = std::array::from_fn(|i| {
            sessions[i].advance_frame(&normal_input)
        });
        for (i, &h) in hashes.iter().enumerate() {
            detectors[i].record_local(frame, h);
        }
    }

    // Frame 50: peer 3 (index 2) diverges — noop → unit stays still → different hash
    let mut hashes_50 = [[0u8; 32]; 4];
    for i in 0..4 {
        let inp: &[RtsInput] = if i == 2 { &diverge_input } else { &normal_input };
        hashes_50[i] = sessions[i].advance_frame(inp);
    }

    // Record frame 50 in detectors
    for (i, &h) in hashes_50.iter().enumerate() {
        detectors[i].record_local(50, h);
    }

    // Each honest peer (0,1,3) observes peer 3's hash → desync
    let mut desync_count = 0usize;
    for obs_idx in [0usize, 1, 3] {
        let peer3_hash = hashes_50[2];
        if let Some(_ev) = detectors[obs_idx].observe_peer(3, 50, peer3_hash) {
            desync_count += 1;
        }
    }

    assert_eq!(desync_count, 3, "all 3 honest peers must detect desync from peer 3");

    // Peer 3 must NOT see desync when comparing against itself — sanity check
    let peer1_hash = hashes_50[0];
    let self_check = detectors[2].observe_peer(1, 50, peer1_hash);
    assert!(
        self_check.is_some(),
        "peer 3 should also detect that it diverged from peer 1"
    );

    // ── Resync: restore peer 3 to peer 1's state snapshot ───────────────────
    // Simulate: peer 3 receives a full snapshot from peer 1 (the honest majority).
    // Snapshot transfer = clone peer 1's tactical state into peer 3's session.
    // This is the GGRS snapshot restore pattern; here we directly copy state fields.
    // After resync, peer 3's tactical state matches peer 1 exactly.
    // KG: seed-post-rts-e2e-4peer-sim-2026-04-15

    // Copy peer 1's tactical state to peer 3
    sessions[2].tactical = sessions[0].tactical.clone();

    // Now advance frames 51-60 on all peers with normal input — must converge
    for _frame in 51u64..=60 {
        let hashes: [[u8; 32]; 4] = std::array::from_fn(|i| {
            sessions[i].advance_frame(&normal_input)
        });
        // All peers should now converge
        let reference = hashes[0];
        for (i, &h) in hashes.iter().enumerate() {
            assert_eq!(
                h, reference,
                "post-resync: peer {} must converge with peer 1 at frame 51-60",
                i + 1
            );
        }
    }
}

// ── Scenario 3: peer_eject ───────────────────────────────────────────────────
// Peer 2 goes AFK during frames 60-70 (no real-time ticks registered).
// PeerEjectController detects AFK, starts BFT vote, vote approved → ejected.
// Remaining 3 peers continue with null-input replay for peer 2.
// Determinism verified: all 3 continuing peers produce identical hashes.
// KG: seed-post-rts-e2e-4peer-sim-2026-04-15

#[test]
fn peer_eject_afk_peer2_null_input_replay_determinism() {
    let mut sessions = make_4_sessions();
    let normal_input = [RtsInput::move_unit(0, Fixed32::from_int(1).0, 0)];

    // PeerEjectController — 50ms timeout for fast test
    let mut eject_ctrl = PeerEjectController::new(Duration::from_millis(50));

    // Peers 1-4 record input up to frame 59
    for frame in 0u64..60 {
        for peer_id in 1u32..=4 {
            eject_ctrl.record_input(peer_id, frame);
        }
        for s in sessions.iter_mut() {
            s.advance_frame(&normal_input);
        }
    }

    // Simulate peer 2 going AFK: no record_input for peer 2 after frame 59.
    // We wait past the 50ms timeout artificially by back-dating last_input_seen.
    // Directly mutate the internal timestamp by re-recording with a past Instant.
    let past = Instant::now() - Duration::from_millis(200);
    eject_ctrl.last_input_seen.insert(2, past);

    // Frames 60-70: only peers 1,3,4 advance with real input; peer 2 is AFK.
    // On each frame, check AFK and apply null-input for peer 2.
    let mut peer2_ejected = false;

    for frame in 60u64..=70 {
        let now = Instant::now();
        let decisions = eject_ctrl.tick(now);

        // Check if peer 2 is in AFK decisions
        let peer2_afk = decisions.iter().any(|d| d.peer == 2);
        if peer2_afk && !eject_ctrl.is_ejected(2) {
            eject_ctrl.start_eject_vote(2);
        }

        // Simulate BFT vote approval (3 peers vote yes)
        if eject_ctrl.pending_vote.as_ref().map(|v| v.target == 2).unwrap_or(false) {
            eject_ctrl.apply_vote_result(2, true);
            peer2_ejected = true;
        }

        // Advance sessions: peer 2 gets null input if ejected, else normal
        let null_inp: &[RtsInput] = &[];
        for (i, s) in sessions.iter_mut().enumerate() {
            let peer_id = (i + 1) as u32;
            let inp: &[RtsInput] = if peer_id == 2 {
                null_inp  // peer 2 is AFK or ejected — always null input
            } else {
                // Record real input for active peers
                eject_ctrl.record_input(peer_id, frame);
                &normal_input
            };
            s.advance_frame(inp);
        }
    }

    // Peer 2 must be ejected
    assert!(peer2_ejected, "peer 2 must have been ejected after AFK timeout");
    assert!(eject_ctrl.is_ejected(2), "eject_ctrl must mark peer 2 as ejected");

    // Determinism: peers 1, 3, 4 must have identical state after null-input replay.
    // Both received identical inputs (all non-null active inputs were the same).
    // Peer 2 received null inputs throughout frames 60-70 on all sessions → still
    // deterministic since all 4 sessions applied the same null sequence for peer 2.

    // Advance 10 more frames — all peers (including ejected peer 2 with null)
    let mut final_hashes = [[0u8; 32]; 4];
    let null_inp: &[RtsInput] = &[];
    for frame in 71u64..=80 {
        for (i, s) in sessions.iter_mut().enumerate() {
            let peer_id = (i + 1) as u32;
            let inp: &[RtsInput] = if eject_ctrl.is_ejected(peer_id) {
                null_inp
            } else {
                &normal_input
            };
            final_hashes[i] = s.advance_frame(inp);
            let _ = frame;
        }
    }

    // Peers 1, 3, 4 applied identical input sequences → must match
    assert_eq!(
        final_hashes[0], final_hashes[2],
        "peer 1 and peer 3 must converge after null-input replay"
    );
    assert_eq!(
        final_hashes[0], final_hashes[3],
        "peer 1 and peer 4 must converge after null-input replay"
    );

    // null_input_for_ejected is deterministic (idempotent)
    let null_a = eject_ctrl.null_input_for_ejected(2, 75);
    let null_b = eject_ctrl.null_input_for_ejected(2, 75);
    assert_eq!(null_a, null_b, "null input replay must be deterministic");
    assert!(null_a.is_null);

    // Peer 2 must not be re-included after eject
    assert!(eject_ctrl.is_ejected(2), "peer 2 must remain ejected");
}

// ── Bonus: FrameDivergenceDetector bft_floor advances after checkpoint ───────
// KG: seed-post-rts-e2e-4peer-sim-2026-04-15

#[test]
fn divergence_detector_bft_floor_advances_on_checkpoint() {
    use triple_three::bft::crypto::QuorumCert;

    let mut det = FrameDivergenceDetector::new(64);
    assert_eq!(det.bft_floor(), 0);

    // Advance floor to frame 100 using genesis QC (block_hash=0 → stub accepts)
    let genesis_qc = QuorumCert::genesis();
    let result = det.accept_bft_checkpoint(100, [0xAB; 32], &genesis_qc);
    assert!(result.is_ok(), "genesis QC must be accepted as checkpoint");
    assert_eq!(det.bft_floor(), 100, "floor must advance to frame 100");
    assert_eq!(det.bft_floor_hash(), [0xAB; 32]);

    // Older checkpoint must be rejected
    let reject = det.accept_bft_checkpoint(50, [0x00; 32], &genesis_qc);
    assert!(reject.is_err(), "older checkpoint must be rejected");
    assert_eq!(det.bft_floor(), 100, "floor must remain at 100 after rejection");
}
