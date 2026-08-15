//! Emit a sample Super-Peer announcement trace as JSONL on stdout.
//! The output is a transparent diagnostic format; native tests own the verdict.

use p333_ltdd::{to_trace_jsonl, Event};

fn main() {
    let cid = "superpeer-demo";
    let trace = vec![
        Event::new(cid, "announce_requested").with("did", "12D3KooWdemo"),
        Event::new(cid, "packet_signed"),
        Event::new(cid, "dht_put_ok").with("addr", "/ip4/10.0.0.1/udp/4001/quic-v1"),
        Event::new(cid, "resolve_ok").with("by", "independent-client"),
    ];
    println!("{}", to_trace_jsonl(&trace));
}
