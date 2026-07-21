//! Emit a sample Super-Peer-announce trace as ooptdd JSONL on stdout — the bridge a Python
//! `ooptdd` gate reads to judge a trace this Rust substrate emitted (generator ≠ verifier,
//! across a language boundary).
//!
//!   cargo run -p p333-ltdd --example emit_trace > verify/trace.jsonl
//!   python verify/ooptdd_verify.py verify/trace.jsonl verify/superpeer.yaml

use p333_ltdd::{to_ooptdd_jsonl, Event};

fn main() {
    let cid = "superpeer-demo";
    let trace = vec![
        Event::new(cid, "announce_requested").with("did", "12D3KooWdemo"),
        Event::new(cid, "packet_signed"),
        Event::new(cid, "dht_put_ok").with("addr", "/ip4/10.0.0.1/udp/4001/quic-v1"),
        Event::new(cid, "resolve_ok").with("by", "independent-client"),
    ];
    println!("{}", to_ooptdd_jsonl(&trace));
}
