//! Emit a deterministic-replay trace as JSONL for direct inspection.
//! Native tests assert replay determinism directly.

use p333_ltdd::{to_trace_jsonl, MemoryStore, Store};
use p333_replay::record_replays;

fn main() {
    let cid = "match-demo";
    let mut s = MemoryStore::default();
    record_replays(&mut s, cid, 0xABCD, &[3, 1, 4, 1, 5, 9, 2, 6], 5, false);
    println!("{}", to_trace_jsonl(&s.query(cid)));
}
