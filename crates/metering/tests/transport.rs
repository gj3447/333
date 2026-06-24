//! PROM · receipt — the metering→credit gate wired behind a Transport (here an in-process
//! Loopback; production swaps in the libp2p Circuit-Relay-v2 stream). The end-to-end byte→credit
//! flow still conserves, and still gates when the bucket runs out.

use p333_ltdd::{verify_present, MemoryStore, Store, Verdict};
use p333_metering::{run_session, CreditBucket, Loopback};

fn sum_u64(store: &impl Store, cid: &str, event: &str, field: &str) -> u64 {
    store
        .query(cid)
        .iter()
        .filter(|e| e.event == event)
        .map(|e| e.attrs.get(field).and_then(|v| v.as_u64()).unwrap_or(0))
        .sum()
}

#[test]
fn a_loopback_session_meters_within_budget_and_conserves_end_to_end() {
    let mut s = MemoryStore::default();
    let mut transport = Loopback::new(vec![2048, 1024, 5000]); // costs 2, 1, 5 = 8
    let mut bucket = CreditBucket::new(100);
    let forwarded = run_session(&mut s, "sess-1", &mut transport, &mut bucket, 1);

    assert_eq!(forwarded, 3, "all chunks forwarded within budget");
    assert_eq!(bucket.balance(), 100 - 8);
    assert_eq!(
        sum_u64(&s, "sess-1", "relay_forwarded", "cost"),
        sum_u64(&s, "sess-1", "credit_debited", "amount"),
        "conservation holds end-to-end through the transport",
    );
}

#[test]
fn a_session_that_outruns_its_budget_gates_partway() {
    let mut s = MemoryStore::default();
    let mut transport = Loopback::new(vec![2048, 4096, 2048]); // costs 2, 4, 2
    let mut bucket = CreditBucket::new(3); // covers the first chunk (2), gates on the 4
    let forwarded = run_session(&mut s, "sess-2", &mut transport, &mut bucket, 1);

    assert_eq!(forwarded, 1, "stops at the gate");
    assert_eq!(verify_present(&s, "sess-2", "relay_gated", 1), Verdict::Present);
    assert_eq!(bucket.balance(), 1, "never overdrawn");
}
