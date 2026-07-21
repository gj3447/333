//! Emit a deterministic-replay trace as ooptdd JSONL — the bridge a Python ooptdd gate reads.
//!   cargo run -p p333-replay --example emit_replay > verify/replay.jsonl
//!   python verify/ooptdd_verify.py verify/replay.jsonl verify/determinism.yaml

use p333_ltdd::{to_ooptdd_jsonl, MemoryStore, Store};
use p333_replay::record_replays;

fn main() {
    let cid = "match-demo";
    let mut s = MemoryStore::default();
    record_replays(&mut s, cid, 0xABCD, &[3, 1, 4, 1, 5, 9, 2, 6], 5, false);
    println!("{}", to_ooptdd_jsonl(&s.query(cid)));
}
