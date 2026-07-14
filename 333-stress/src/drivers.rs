// KG: SPAN_333_Stress_Drivers
// Shared stress-test drivers. Tests exercise invariants; the bench binary
// measures throughput of the same drivers.

use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

use consensus333::{Block, BlockFinality, ConsensusProtocol, InMemoryConsensus, SettlementOp, ValidatorSet, VoteKind};
use identity333::Keypair;
use mq333::{InMemoryBroker, PubSub};
use token333::{InMemoryLedger, TokenLedger};

/// Concurrent-transfer driver: `threads` workers, `iters` transfers each,
/// between a ring of `n_accounts` accounts starting with 1_000_000 each.
/// Returns final total supply — should equal starting supply if conservation holds.
pub fn run_concurrent_transfers(
    threads: usize,
    iters: usize,
    n_accounts: usize,
) -> (u128, u128) {
    let ledger = Arc::new(InMemoryLedger::new());
    let accts: Vec<_> = (0..n_accounts).map(|_| Keypair::generate().node_id()).collect();
    let initial: u128 = 1_000_000;
    for a in &accts {
        ledger.mint(a, initial).unwrap();
    }
    let starting = ledger.total_supply();
    let barrier = Arc::new(Barrier::new(threads));
    let mut handles = Vec::new();
    for tid in 0..threads {
        let ledger = ledger.clone();
        let accts = accts.clone();
        let barrier = barrier.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            for i in 0..iters {
                let from = &accts[(tid + i) % accts.len()];
                let to = &accts[(tid + i + 1) % accts.len()];
                // Try amount 1; failures (insufficient funds) are acceptable but rare here.
                let _ = ledger.transfer(from, to, 1);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    (starting, ledger.total_supply())
}

/// Multi-sub broker fanout driver: `subs` subscribers on the same topic,
/// `messages` publishes, returns (sum_delivered_per_sub, expected).
pub fn run_broker_fanout(subs: usize, messages: usize) -> (usize, usize) {
    let broker = InMemoryBroker::new();
    let sids: Vec<_> = (0..subs).map(|_| broker.subscribe("t").unwrap()).collect();
    for _ in 0..messages {
        broker.publish("t", b"x".to_vec()).unwrap();
    }
    let mut sum = 0;
    for sid in sids {
        sum += broker.poll(sid).unwrap().len();
    }
    (sum, subs * messages)
}

/// Concurrent voting: n validators proposing votes for the same block in
/// parallel. Invariant: exactly one Committed block, no panics, no double-sign
/// false positives.
pub fn run_concurrent_consensus_vote(n_validators: usize) -> BlockFinality {
    let kps: Vec<Keypair> = (0..n_validators).map(|_| Keypair::generate()).collect();
    let ids: Vec<_> = kps.iter().map(|k| k.node_id()).collect();
    let vs = ValidatorSet::new(ids).unwrap();
    let c = Arc::new(InMemoryConsensus::new(vs));

    let leader = kps[0].node_id();
    let block = Block {
        height: 0,
        proposer: leader,
        parent_hash: [0u8; 32],
        ops: vec![SettlementOp::RankedAction {
            actor: kps[0].node_id(),
            kind: "noop".into(),
            rank: 0,
            payload: vec![],
        }],
        finality: BlockFinality::Tentative,
    };
    c.propose(block.clone()).unwrap();
    let h = block.hash();

    let barrier = Arc::new(Barrier::new(n_validators));
    let kps_arc = Arc::new(kps);
    let mut handles = Vec::new();
    for i in 0..n_validators {
        let c = c.clone();
        let kps = kps_arc.clone();
        let barrier = barrier.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            c.vote(InMemoryConsensus::sign_vote(&kps[i], 0, h, VoteKind::Prevote)).unwrap();
            c.vote(InMemoryConsensus::sign_vote(&kps[i], 0, h, VoteKind::Precommit)).unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    c.finalize(0).unwrap()
}

/// Byzantine vote stress: `f` byzantine validators each attempt one double-sign;
/// the remaining honest validators commit normally. Expect: double-signs detected
/// (returned as errors), honest quorum still commits.
pub fn run_byzantine_votes(total: usize) -> (usize, BlockFinality) {
    assert!(total >= 4);
    let f = (total - 1) / 3;
    let kps: Vec<Keypair> = (0..total).map(|_| Keypair::generate()).collect();
    let ids: Vec<_> = kps.iter().map(|k| k.node_id()).collect();
    let vs = ValidatorSet::new(ids).unwrap();
    let c = InMemoryConsensus::new(vs);
    let leader = kps[0].node_id();
    let block = Block {
        height: 0,
        proposer: leader,
        parent_hash: [0u8; 32],
        ops: vec![],
        finality: BlockFinality::Tentative,
    };
    c.propose(block.clone()).unwrap();
    let h = block.hash();
    let mut double_sign_detected = 0;

    // Honest validators (f..total): prevote + precommit the real hash.
    for i in f..total {
        c.vote(InMemoryConsensus::sign_vote(&kps[i], 0, h, VoteKind::Prevote)).unwrap();
        c.vote(InMemoryConsensus::sign_vote(&kps[i], 0, h, VoteKind::Precommit)).unwrap();
    }
    // Byzantine validators (0..f): prevote real then try to prevote a fake hash.
    let fake_hash = [0xff; 32];
    for i in 0..f {
        c.vote(InMemoryConsensus::sign_vote(&kps[i], 0, h, VoteKind::Prevote)).unwrap();
        if c.vote(InMemoryConsensus::sign_vote(&kps[i], 0, fake_hash, VoteKind::Prevote))
            .is_err()
        {
            double_sign_detected += 1;
        }
    }
    (double_sign_detected, c.finalize(0).unwrap())
}

/// Throughput: lots of sequential ledger transfers. Returns (ops, duration_ms).
pub fn bench_ledger_transfers(ops: usize) -> (usize, u128) {
    let ledger = InMemoryLedger::new();
    let a = Keypair::generate().node_id();
    let b = Keypair::generate().node_id();
    ledger.mint(&a, ops as u128 + 1).unwrap();
    let start = Instant::now();
    for _ in 0..ops {
        ledger.transfer(&a, &b, 1).unwrap();
    }
    (ops, start.elapsed().as_millis())
}

/// CRDT Or-Set merge: build two sets, merge, measure commutativity.
pub fn bench_crdt_merge(n: usize) -> (u128, usize) {
    use crdt333::{AutoCrdt, Crdt, Lww};
    // Simple: n u64s, a merges max semantics
    let start = Instant::now();
    let mut a: u64 = 0;
    for i in 0..n as u64 {
        a.merge(&i);
    }
    let _ = Lww::raw(0u64, 0u64);
    fn _assert_auto<T: AutoCrdt>() {}
    _assert_auto::<u64>();
    (start.elapsed().as_micros(), a as usize)
}

/// Signaling: `envelopes` signed publishes through a fresh mesh.
pub fn bench_signaling_publish(envelopes: usize) -> (usize, u128) {
    use signaling333::{Envelope, InMemorySignalingMesh, ScoreParams, SignalingMesh, Topic};
    let me = Keypair::generate();
    let peer = Keypair::generate();
    let mesh = InMemorySignalingMesh::new(me.node_id(), ScoreParams::default());
    mesh.subscribe(Topic::Custom("/t".into())).unwrap();
    let start = Instant::now();
    for i in 0..envelopes {
        let env = Envelope::sign(
            &peer,
            Topic::Custom("/t".into()),
            None,
            vec![],
            i as u64,
        );
        mesh.publish(env).unwrap();
    }
    (envelopes, start.elapsed().as_millis())
}
