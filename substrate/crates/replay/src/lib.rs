//! 333 v2 substrate — Lane C: deterministic replay (rollback netcode; production adopts **ggrs**).
//!
//! The safety law of rollback/lockstep is DETERMINISM: replaying the same input sequence must
//! produce the same state. A step that reads anything outside its inputs (wall-clock, map iteration
//! order, uninitialised memory) silently breaks it — and a desync is the worst class of netcode bug.
//! LTDD framing: replay the same inputs N times and assert (read back from the store) that every run
//! agrees; a run that diverged is surfaced as `replay_diverged` (the `absent`/forbid check turns RED).

use p333_ltdd::{Event, Store};

/// A deterministic state transition: `new = mix(state, input)`. Pure — depends only on its inputs.
pub fn step(state: u64, input: u8) -> u64 {
    let s = state ^ (input as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    s.rotate_left(7).wrapping_add(0x1234_5678)
}

/// Replay `inputs` from `seed` to a final state. `nondeterministic` folds in the run index — a
/// stand-in for any out-of-inputs dependency — so a receipt can prove the breakage is caught.
pub fn replay(seed: u64, inputs: &[u8], run: u64, nondeterministic: bool) -> u64 {
    let mut s = seed;
    if nondeterministic {
        s ^= run; // the bug: the result depends on WHICH run it is, not just the inputs
    }
    for &i in inputs {
        s = step(s, i);
    }
    s
}

/// Replay the same inputs `runs` times and record whether every run agrees. Emits
/// `replay_result{run,hash}` per run; `deterministic` iff all agree, else `replay_diverged`.
/// Returns determinism. The store — not this bool — is the receipt's judge.
pub fn record_replays(
    store: &mut impl Store,
    cid: &str,
    seed: u64,
    inputs: &[u8],
    runs: u64,
    nondeterministic: bool,
) -> bool {
    let hashes: Vec<u64> = (0..runs).map(|r| replay(seed, inputs, r, nondeterministic)).collect();
    let canonical = hashes.first().copied().unwrap_or(0);
    let mut deterministic = true;
    for (r, &h) in hashes.iter().enumerate() {
        store.ship(&[Event::new(cid, "replay_result").with("run", r as u64).with("hash", h)]);
        if h != canonical {
            deterministic = false;
            store.ship(&[Event::new(cid, "replay_diverged")
                .with("run", r as u64)
                .with("hash", h)
                .with("expected", canonical)]);
        }
    }
    if deterministic {
        store.ship(&[Event::new(cid, "deterministic").with("runs", runs)]);
    }
    deterministic
}
