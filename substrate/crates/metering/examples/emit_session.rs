//! Emit a transport-driven relay session as JSONL for direct inspection.
//! The same conservation invariant is asserted in native tests.

use p333_ltdd::{to_trace_jsonl, MemoryStore, Store};
use p333_metering::{run_session, CreditBucket, Loopback};

fn main() {
    let cid = "relay-session-demo";
    let mut s = MemoryStore::default();
    let mut transport = Loopback::new(vec![2048, 1024, 2048]); // 2 + 1 + 2 = 5
    let mut bucket = CreditBucket::new(&mut s, cid, 5);
    run_session(&mut s, cid, &mut transport, &mut bucket, 1);
    println!("{}", to_trace_jsonl(&s.query(cid)));
}
