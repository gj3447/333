//! Emit a relay-metering trace as ooptdd JSONL — the bridge a Python `ooptdd` gate reads to
//! assert the credit **conservation invariant** (`sum(cost@relay_forwarded) ==
//! sum(amount@credit_debited)`) cross-language.
//!
//!   cargo run -p p333-metering --example emit_metering > verify/metering.jsonl
//!   python verify/ooptdd_verify.py verify/metering.jsonl verify/relay_credit.yaml

use p333_ltdd::{to_ooptdd_jsonl, MemoryStore, Store};
use p333_metering::{relay_forward, CreditBucket};

fn main() {
    let cid = "relay-sess-demo";
    let mut store = MemoryStore::default();
    let mut bucket = CreditBucket::new(&mut store, cid, 5); // 5 prepaid credits
    relay_forward(&mut store, cid, &mut bucket, 2048, 1); // cost 2
    relay_forward(&mut store, cid, &mut bucket, 1024, 1); // cost 1
    relay_forward(&mut store, cid, &mut bucket, 2048, 1); // cost 2 -> bucket now 0
    relay_forward(&mut store, cid, &mut bucket, 4096, 1); // cost 4 > 0 -> GATED
    println!("{}", to_ooptdd_jsonl(&store.query(cid)));
}
