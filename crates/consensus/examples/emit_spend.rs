//! Emit an owned-object equivocation trace as ooptdd JSONL — the bridge a Python ooptdd gate reads
//! to assert exactly-once finalization (no double-spend).
//!   cargo run -p p333-consensus --example emit_spend > verify/spend.jsonl
//!   python verify/ooptdd_verify.py verify/spend.jsonl verify/owned_safety.yaml

use p333_consensus::Ledger;
use p333_ltdd::{to_ooptdd_jsonl, MemoryStore, Store};

fn main() {
    let cid = "spend-demo";
    let mut s = MemoryStore::default();
    let mut l = Ledger::default();
    l.register("coin-A");
    l.spend(&mut s, cid, "coin-A", 0, "alice->bob"); // finalizes
    l.spend(&mut s, cid, "coin-A", 0, "alice->carol"); // equivocation -> rejected
    println!("{}", to_ooptdd_jsonl(&s.query(cid)));
}
