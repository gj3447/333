//! Emit an owned-object equivocation trace as JSONL for direct inspection.
//! Native tests assert exactly-once finalization and double-spend rejection.

use p333_consensus::Ledger;
use p333_ltdd::{to_trace_jsonl, MemoryStore, Store};

fn main() {
    let cid = "spend-demo";
    let mut s = MemoryStore::default();
    let mut l = Ledger::default();
    l.register(&mut s, cid, "coin-A");
    l.spend(&mut s, cid, "coin-A", 0, "alice->bob"); // finalizes
    l.spend(&mut s, cid, "coin-A", 0, "alice->carol"); // equivocation -> rejected
    println!("{}", to_trace_jsonl(&s.query(cid)));
}
