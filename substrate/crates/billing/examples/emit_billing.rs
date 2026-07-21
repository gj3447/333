//! Emit a replay-safe billing session as ooptdd JSONL — judged by the COMBINED gate
//! (verify/billing.yaml): conservation invariant + each debit finalized + no replay.
//!   cargo run -p p333-billing --example emit_billing > verify/billing.jsonl
//!   python verify/ooptdd_verify.py verify/billing.jsonl verify/billing.yaml

use p333_billing::Account;
use p333_ltdd::{to_ooptdd_jsonl, MemoryStore, Store};

fn main() {
    let cid = "billing-demo";
    let mut s = MemoryStore::default();
    let mut acct = Account::new(&mut s, cid, "alice-credits", 100);
    acct.meter_and_bill(&mut s, cid, 2048, 1); // cost 2
    acct.meter_and_bill(&mut s, cid, 1024, 1); // cost 1
    acct.meter_and_bill(&mut s, cid, 5000, 1); // cost 5
    println!("{}", to_ooptdd_jsonl(&s.query(cid)));
}
