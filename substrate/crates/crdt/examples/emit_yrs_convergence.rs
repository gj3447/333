//! Emit a REAL yrs convergence trace as ooptdd JSONL — proves the Python `ooptdd` gate
//! (verify/crdt_convergence.yaml) judges the production CRDT, not just the minimal G-Counter.
//!
//!   cargo run -p p333-crdt --example emit_yrs_convergence > verify/yrs_convergence.jsonl
//!   python verify/ooptdd_verify.py verify/yrs_convergence.jsonl verify/crdt_convergence.yaml

use p333_crdt::yrs_text_converge;
use p333_ltdd::{to_ooptdd_jsonl, MemoryStore, Store};

fn main() {
    let cid = "ydoc-demo";
    let mut s = MemoryStore::default();
    yrs_text_converge(&mut s, cid, &[("a", "hello "), ("b", "world"), ("c", "!!!")]);
    println!("{}", to_ooptdd_jsonl(&s.query(cid)));
}
