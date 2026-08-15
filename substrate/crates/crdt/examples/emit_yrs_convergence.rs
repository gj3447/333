//! Emit a real yrs convergence trace as JSONL for direct inspection.
//! Native tests exercise the production CRDT as well as the minimal G-Counter.

use p333_crdt::yrs_text_converge;
use p333_ltdd::{to_trace_jsonl, MemoryStore, Store};

fn main() {
    let cid = "ydoc-demo";
    let mut s = MemoryStore::default();
    yrs_text_converge(&mut s, cid, &[("a", "hello "), ("b", "world"), ("c", "!!!")]);
    println!("{}", to_trace_jsonl(&s.query(cid)));
}
