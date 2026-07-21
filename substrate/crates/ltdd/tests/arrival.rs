//! PROM · receipt — Rust-native LTDD: the verdict is read back from the STORE, never the
//! return value. Same as the discovery receipt's "real DHT put+get, not a serialize-then-read"
//! discipline, generalized: a function returning "ok" is a CLAIM; the store is the judge.

use p333_ltdd::{verify_present, Event, MemoryStore, Store, Verdict};

/// Stands in for a substrate flow (e.g. announcing a Super-Peer): emits structured events as it
/// works, then self-reports "ok" — a claim a return-value test would believe.
fn announce_superpeer(store: &mut impl Store, cid: &str) -> &'static str {
    store.ship(&[Event::new(cid, "announce_requested").with("did", "12D3KooWdemo")]);
    store.ship(&[Event::new(cid, "packet_signed")]);
    store.ship(&[Event::new(cid, "dht_put_ok").with("addr", "/ip4/10.0.0.1/udp/4001/quic-v1")]);
    "ok"
}

#[test]
fn healthy_store_positively_confirms_arrival() {
    let mut s = MemoryStore::default();
    assert_eq!(announce_superpeer(&mut s, "cyc-1"), "ok");
    // the real assertion: the events ARRIVED, read back from the store.
    assert_eq!(verify_present(&s, "cyc-1", "dht_put_ok", 1), Verdict::Present);
    assert_eq!(verify_present(&s, "cyc-1", "packet_signed", 1), Verdict::Present);
}

#[test]
fn silent_loss_is_caught_even_though_the_self_report_says_ok() {
    // accepts every ship() and silently discards it — the 401 nobody noticed.
    let mut s = MemoryStore::dropping();
    assert_eq!(announce_superpeer(&mut s, "cyc-2"), "ok"); // <- the LIE a return-value test believes
    assert_eq!(verify_present(&s, "cyc-2", "dht_put_ok", 1), Verdict::Absent); // <- the truth
}

#[test]
fn unreachable_store_is_inconclusive_never_a_hard_fail() {
    // querying the store failed entirely: '?' (inconclusive), unrelated to the substrate —
    // it must NEVER be a hard failure, or an infra blip becomes a flaky receipt.
    let s = MemoryStore::unreachable();
    assert_eq!(verify_present(&s, "cyc-3", "dht_put_ok", 1), Verdict::Inconclusive);
}

#[test]
fn green_to_absent_flip_is_real() {
    // the SAME emit flow: a real store confirms arrival (Present); a dropping store flips it to
    // Absent. The verdict tracks the store, not the unchanged "ok" self-report.
    let mut ok = MemoryStore::default();
    announce_superpeer(&mut ok, "cyc-4");
    let mut lost = MemoryStore::dropping();
    announce_superpeer(&mut lost, "cyc-4");
    assert_eq!(verify_present(&ok, "cyc-4", "dht_put_ok", 1), Verdict::Present);
    assert_eq!(verify_present(&lost, "cyc-4", "dht_put_ok", 1), Verdict::Absent);
}

#[test]
fn jsonl_is_the_ooptdd_envelope_for_a_cross_language_python_verifier() {
    // the bridge: emit the ooptdd wire shape so a Python ooptdd gate (generator != verifier,
    // across a language boundary) can judge a trace this Rust substrate emitted.
    let evs = vec![Event::new("cyc-5", "dht_put_ok").with("addr", "/ip4/1.2.3.4")];
    let line = p333_ltdd::to_ooptdd_jsonl(&evs);
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["cid"], "cyc-5");
    assert_eq!(v["cycle_id"], "cyc-5"); // ooptdd resolves the cid from cycle_id/cid
    assert_eq!(v["event"], "dht_put_ok");
    assert_eq!(v["addr"], "/ip4/1.2.3.4"); // attrs flattened — whole-row passthrough for gate `where:`
}
