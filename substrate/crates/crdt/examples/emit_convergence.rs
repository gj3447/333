//! Emit a CRDT convergence trace as JSONL for direct inspection.
//! Native tests assert convergence and detect replica divergence.

use p333_crdt::{gossip, record_states, GCounter};
use p333_ltdd::{to_trace_jsonl, MemoryStore, Store};

fn main() {
    let cid = std::env::var("P333_CID")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "doc-demo".to_owned());
    let mut s = MemoryStore::default();
    let mut r = [
        ("a", GCounter::default()),
        ("b", GCounter::default()),
        ("c", GCounter::default()),
    ];
    r[0].1.increment("a", 2);
    r[1].1.increment("b", 3);
    r[2].1.increment("c", 4);
    gossip(&mut r);
    record_states(&mut s, &cid, &r);
    println!("{}", to_trace_jsonl(&s.query(&cid)));
}
