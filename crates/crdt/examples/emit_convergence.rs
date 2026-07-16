//! Emit a CRDT convergence trace as ooptdd JSONL — the bridge a Python `ooptdd` gate reads to
//! assert convergence cross-language (`converged` present, `replica_diverged` ABSENT).
//!
//!   cargo run -p p333-crdt --example emit_convergence > verify/convergence.jsonl
//!   python verify/ooptdd_verify.py verify/convergence.jsonl verify/crdt_convergence.yaml

use p333_crdt::{gossip, record_states, GCounter};
use p333_ltdd::{to_ooptdd_jsonl, MemoryStore, Store};

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
    println!("{}", to_ooptdd_jsonl(&s.query(&cid)));
}
