//! PROM · receipt — Lane C determinism law, LTDD-verified. Same inputs must replay to the same
//! state; a step that depends on anything else is caught as `replay_diverged`.

use p333_ltdd::{verify_present, MemoryStore, Verdict};
use p333_replay::{record_replays, replay, step};

#[test]
fn step_is_pure() {
    assert_eq!(step(42, 7), step(42, 7));
    assert_ne!(step(42, 7), step(42, 8));
}

#[test]
fn same_inputs_replay_to_the_same_state_across_runs() {
    let mut s = MemoryStore::default();
    let inputs = [3u8, 1, 4, 1, 5, 9, 2, 6];
    let det = record_replays(&mut s, "match-1", 0xABCD, &inputs, 5, false);
    assert!(det, "a pure replay must be deterministic across runs");
    assert_eq!(verify_present(&s, "match-1", "deterministic", 1), Verdict::Present);
    assert_eq!(verify_present(&s, "match-1", "replica_diverged", 1), Verdict::Absent);
    assert_eq!(verify_present(&s, "match-1", "replay_diverged", 1), Verdict::Absent);
    // the 5 runs are the SAME final state
    assert!((0..5).map(|r| replay(0xABCD, &inputs, r, false)).all(|h| h == replay(0xABCD, &inputs, 0, false)));
}

#[test]
fn a_step_that_depends_on_the_run_diverges_and_is_caught() {
    // a non-deterministic step (mixes in the run index — a stand-in for wall-clock / iteration
    // order / uninit). The runs disagree -> the receipt records `replay_diverged`.
    let mut s = MemoryStore::default();
    let inputs = [1u8, 2, 3];
    let det = record_replays(&mut s, "match-2", 0xABCD, &inputs, 4, true);
    assert!(!det, "a run-dependent step is not deterministic");
    assert_eq!(verify_present(&s, "match-2", "replay_diverged", 1), Verdict::Present);
    assert_eq!(verify_present(&s, "match-2", "deterministic", 1), Verdict::Absent);
}
