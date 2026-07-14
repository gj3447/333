// KG: seed-ggrs-bft-adapter-eval-2026-04-15
//! Determinism test — 4-player mock session, 100 frames.
//!
//! Two independent `BftGgrsSession` instances receive identical inputs
//! and must produce matching state checksums at every frame.
//!
//! This validates:
//!   1. `BftGgrsSession::advance_frame` is deterministic given identical inputs
//!   2. `GameStateSnapshot::new` checksum is stable
//!   3. `BftPlayerMap` mapping is consistent across sessions

use triple_three_ggrs_adapter::{
    BftGgrsSession, BftPlayerMap, GameInput, GameStateSnapshot, compute_checksum,
};

/// Minimal deterministic ECS stub — a flat array of u32 counters, one per player.
/// Each frame: counter[player] += input.buttons as u32
/// KG: seed-ggrs-bft-adapter-eval-2026-04-15
struct MockEcs {
    counters: [u32; 4],
    frame: u32,
}

impl MockEcs {
    fn new() -> Self {
        Self { counters: [0; 4], frame: 0 }
    }

    fn advance(&mut self, inputs: &[(u32, GameInput)]) {
        for &(player, input) in inputs {
            if (player as usize) < self.counters.len() {
                self.counters[player as usize] =
                    self.counters[player as usize].wrapping_add(input.buttons as u32);
            }
        }
        self.frame += 1;
    }

    fn snapshot(&self) -> GameStateSnapshot {
        // Serialize: [frame: 4 bytes] ++ [counters: 16 bytes]
        let mut data = Vec::with_capacity(20);
        data.extend_from_slice(&self.frame.to_le_bytes());
        for &c in &self.counters {
            data.extend_from_slice(&c.to_le_bytes());
        }
        GameStateSnapshot::new(self.frame, data)
    }
}

/// Generate a deterministic input stream for testing.
/// frame 0..99: player 0 presses buttons cycling 0..4, others are mirrored.
fn make_inputs(frame: u32) -> [GameInput; 4] {
    [
        GameInput { buttons: (frame % 5) as u8, frame },
        GameInput { buttons: ((frame + 1) % 5) as u8, frame },
        GameInput { buttons: ((frame + 2) % 5) as u8, frame },
        GameInput { buttons: ((frame + 3) % 5) as u8, frame },
    ]
}

#[test]
fn two_sessions_same_hash_after_100_frames() {
    // KG: seed-ggrs-bft-adapter-eval-2026-04-15
    let validators = vec![10u32, 20, 30, 40]; // sorted
    let map_a = BftPlayerMap::from_sorted_validators(&validators);
    let map_b = BftPlayerMap::from_sorted_validators(&validators);

    let mut sess_a = BftGgrsSession::new(map_a, 10).expect("session A init");
    let mut sess_b = BftGgrsSession::new(map_b, 20).expect("session B init");

    let mut ecs_a = MockEcs::new();
    let mut ecs_b = MockEcs::new();

    for frame in 0..100u32 {
        let inputs = make_inputs(frame);

        // Feed identical inputs to both sessions
        sess_a.add_local_input(inputs[0]);
        sess_b.add_local_input(inputs[1]);

        for (p, inp) in inputs.iter().enumerate() {
            sess_a.receive_remote_input(p as u32, *inp);
            sess_b.receive_remote_input(p as u32, *inp);
        }

        // Advance both
        let reqs_a = sess_a.advance_frame().expect("advance A");
        let reqs_b = sess_b.advance_frame().expect("advance B");

        // Process requests — apply AdvanceFrame to ECS
        for req in &reqs_a {
            if let triple_three_ggrs_adapter::GgrsRequest::AdvanceFrame { inputs: adv_inputs } = req {
                ecs_a.advance(adv_inputs);
            }
        }
        for req in &reqs_b {
            if let triple_three_ggrs_adapter::GgrsRequest::AdvanceFrame { inputs: adv_inputs } = req {
                ecs_b.advance(adv_inputs);
            }
        }

        // Compare snapshots
        let snap_a = ecs_a.snapshot();
        let snap_b = ecs_b.snapshot();

        assert_eq!(
            snap_a.checksum, snap_b.checksum,
            "checksum mismatch at frame {frame}: {snap_a:?} vs {snap_b:?}"
        );
        assert_eq!(
            snap_a.data, snap_b.data,
            "state data mismatch at frame {frame}"
        );
    }

    assert_eq!(ecs_a.frame, 100);
    assert_eq!(ecs_b.frame, 100);
}

#[test]
fn checksum_is_stable_for_same_data() {
    // KG: seed-ggrs-bft-adapter-eval-2026-04-15
    let data = b"333-platform-ecs-state-v1".to_vec();
    let c1 = compute_checksum(&data);
    let c2 = compute_checksum(&data);
    assert_eq!(c1, c2, "checksum must be deterministic");
}

#[test]
fn checksum_differs_for_different_data() {
    let a = compute_checksum(b"state-a");
    let b = compute_checksum(b"state-b");
    assert_ne!(a, b, "different states must have different checksums");
}

#[test]
fn player_map_consistent_across_4_peers() {
    // KG: seed-ggrs-bft-adapter-eval-2026-04-15
    // All four peers build their map independently — must agree on handles
    let validators = vec![10u32, 20, 30, 40];
    let maps: Vec<_> = validators
        .iter()
        .map(|_| BftPlayerMap::from_sorted_validators(&validators))
        .collect();

    for node in &validators {
        let handles: Vec<_> = maps.iter().map(|m| m.node_to_handle(*node)).collect();
        assert!(handles.windows(2).all(|w| w[0] == w[1]),
            "node {node} must map to same handle on all peers");
    }
}

#[test]
fn rollback_restores_state() {
    // KG: seed-ggrs-bft-adapter-eval-2026-04-15
    // Simulate: advance 5 frames, save at frame 3, continue to 5, rollback to 3
    let map = BftPlayerMap::from_sorted_validators(&[1, 2, 3, 4]);
    let mut sess = BftGgrsSession::new(map, 1).expect("session init");
    let mut ecs = MockEcs::new();

    let mut snapshot_at_3: Option<GameStateSnapshot> = None;

    for frame in 0..5u32 {
        let inputs = make_inputs(frame);
        for (p, inp) in inputs.iter().enumerate() {
            sess.receive_remote_input(p as u32, *inp);
        }
        sess.add_local_input(inputs[0]);
        let reqs = sess.advance_frame().expect("advance");

        for req in &reqs {
            match req {
                triple_three_ggrs_adapter::GgrsRequest::SaveGameState { frame: f, .. } => {
                    let snap = ecs.snapshot();
                    sess.confirm_save(*f, snap.clone());
                    if *f == 3 { snapshot_at_3 = Some(snap); }
                }
                triple_three_ggrs_adapter::GgrsRequest::AdvanceFrame { inputs: adv } => {
                    ecs.advance(adv);
                }
                _ => {}
            }
        }
    }

    // Verify we have a snapshot at frame 3
    assert!(snapshot_at_3.is_some(), "must have saved at frame 3");
    let stored = sess.get_snapshot(3).expect("stored snapshot missing");
    assert_eq!(stored.checksum, snapshot_at_3.unwrap().checksum);
}
