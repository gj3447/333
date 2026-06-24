//! Emit a transport-driven relay session as ooptdd JSONL — judged by the SAME conservation gate
//! (verify/relay_credit.yaml) as the hand-driven metering example.
//!   cargo run -p p333-metering --example emit_session > verify/session.jsonl
//!   python verify/ooptdd_verify.py verify/session.jsonl verify/relay_credit.yaml

use p333_ltdd::{to_ooptdd_jsonl, MemoryStore, Store};
use p333_metering::{run_session, CreditBucket, Loopback};

fn main() {
    let cid = "relay-session-demo";
    let mut s = MemoryStore::default();
    let mut transport = Loopback::new(vec![2048, 1024, 2048]); // 2 + 1 + 2 = 5
    let mut bucket = CreditBucket::new(5);
    run_session(&mut s, cid, &mut transport, &mut bucket, 1);
    println!("{}", to_ooptdd_jsonl(&s.query(cid)));
}
