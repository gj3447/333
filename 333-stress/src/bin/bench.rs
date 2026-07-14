// KG: SPAN_333_Stress_Bench
// cargo run --release --bin stress333-bench

use stress333::drivers::*;

fn main() {
    println!("=== 333 P2P OS stress bench (release mode) ===\n");

    // Ledger throughput
    let (ops, ms) = bench_ledger_transfers(500_000);
    let rate = ops as f64 / (ms as f64 / 1000.0);
    println!("ledger transfers     : {ops} ops in {ms} ms ({rate:.0} ops/sec)");

    // Signaling publish (signature verify on every call)
    let (envs, ms) = bench_signaling_publish(10_000);
    let rate = envs as f64 / (ms as f64 / 1000.0);
    println!("signaling publish    : {envs} ops in {ms} ms ({rate:.0} ops/sec, each with ed25519 verify)");

    // CRDT merge
    let (us, _final) = bench_crdt_merge(1_000_000);
    println!("crdt merge (1M u64s) : {us} μs");

    // Concurrent transfers (conservation)
    let (start, end) = run_concurrent_transfers(8, 50_000, 16);
    println!(
        "concurrent transfers : 8 threads × 50_000 iters × 16 accounts. supply_before={start} supply_after={end} conservation={}",
        if start == end { "OK" } else { "BROKEN" }
    );

    // Broker fanout
    let (delivered, expected) = run_broker_fanout(64, 1_000);
    println!(
        "broker fanout        : 64 subs × 1000 pubs. delivered={delivered} expected={expected} match={}",
        if delivered == expected { "OK" } else { "MISS" }
    );

    // Concurrent consensus vote
    let finality = run_concurrent_consensus_vote(32);
    println!("concurrent consensus : 32 validators parallel votes → finality={finality:?}");

    // Byzantine
    let (detected, finality) = run_byzantine_votes(16);
    println!("byzantine votes      : 16 validators, f=5 byzantine → double-signs detected={detected}, finality={finality:?}");
}
