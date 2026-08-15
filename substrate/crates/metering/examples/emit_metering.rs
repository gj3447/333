//! Emit a relay-metering trace as JSONL for direct inspection.
//! Native tests assert credit conservation and refusal on insufficient balance.

use p333_ltdd::{to_trace_jsonl, MemoryStore, Store};
use p333_metering::{relay_forward, CreditBucket};

fn main() {
    let cid = "relay-sess-demo";
    let mut store = MemoryStore::default();
    let mut bucket = CreditBucket::new(&mut store, cid, 5); // 5 prepaid credits
    relay_forward(&mut store, cid, &mut bucket, 2048, 1); // cost 2
    relay_forward(&mut store, cid, &mut bucket, 1024, 1); // cost 1
    relay_forward(&mut store, cid, &mut bucket, 2048, 1); // cost 2 -> bucket now 0
    relay_forward(&mut store, cid, &mut bucket, 4096, 1); // cost 4 > 0 -> GATED
    println!("{}", to_trace_jsonl(&store.query(cid)));
}
