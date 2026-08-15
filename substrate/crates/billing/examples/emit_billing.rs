//! Emit a replay-safe billing session as JSONL for direct inspection.
//! The crate tests assert conservation, finalization, and replay safety directly.

use p333_billing::Account;
use p333_ltdd::{to_trace_jsonl, MemoryStore, Store};

fn main() {
    let cid = "billing-demo";
    let mut s = MemoryStore::default();
    let mut acct = Account::new(&mut s, cid, "alice-credits", 100);
    acct.meter_and_bill(&mut s, cid, 2048, 1); // cost 2
    acct.meter_and_bill(&mut s, cid, 1024, 1); // cost 1
    acct.meter_and_bill(&mut s, cid, 5000, 1); // cost 5
    println!("{}", to_trace_jsonl(&s.query(cid)));
}
