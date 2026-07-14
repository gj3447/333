// KG: SPAN_333_Wire_CrossSocketTest
// Cross-socket integration test — two Nodes bound to separate TCP ports on
// localhost, converging on BlockFinality::Committed by exchanging real frames
// through the kernel socket layer.

use std::sync::Arc;
use std::time::Duration;

use consensus333::{Block, BlockFinality, ConsensusProtocol, SettlementOp, ValidatorSet, VoteKind, InMemoryConsensus};
use identity333::Keypair;
use wire333::{Frame, Node};

fn wait_for<F: Fn() -> bool>(max_ms: u64, f: F) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(max_ms) {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn two_validators_reach_committed_over_real_tcp() {
    // Two validators with the same validator set.
    let ka = Keypair::generate();
    let kb = Keypair::generate();
    let vs = ValidatorSet::new(vec![ka.node_id(), kb.node_id()]).unwrap();

    // Bind each on an ephemeral port (port 0 → kernel picks).
    let node_a = Node::bind(ka.clone(), vs.clone(), "127.0.0.1:0").unwrap();
    let node_b = Node::bind(kb.clone(), vs.clone(), "127.0.0.1:0").unwrap();

    let addr_a = node_a.bound_addr().to_string();
    let addr_b = node_b.bound_addr().to_string();
    println!("node A bound on {addr_a}, node B bound on {addr_b}");

    // Each side dials the other so both can push frames.
    node_a.dial(&addr_b).unwrap();
    node_b.dial(&addr_a).unwrap();

    // Give listeners a beat to register the inbound connections.
    std::thread::sleep(Duration::from_millis(50));

    // Leader at height 0 is validators[0] = ka.
    // Build proposal locally and broadcast to peer.
    let block = Block {
        height: 0,
        proposer: ka.node_id(),
        parent_hash: [0u8; 32],
        ops: vec![SettlementOp::RankedAction {
            actor: ka.node_id(),
            kind: "voxel.place".into(),
            rank: 1,
            payload: b"hello-world-across-sockets".to_vec(),
        }],
        finality: BlockFinality::Tentative,
    };
    let block_hash = block.hash();

    // A applies its own proposal locally AND broadcasts it.
    node_a.apply(&Frame::Proposal(block.clone())).unwrap();
    node_a.broadcast(&Frame::Proposal(block.clone())).unwrap();

    // B should see the proposal on the wire.
    let drained_b = node_b.drain_and_apply(1, Duration::from_millis(2000));
    assert_eq!(drained_b, 1, "node B must receive the proposal over TCP");

    // Each side prevotes locally + broadcasts.
    let a_prevote = InMemoryConsensus::sign_vote(&ka, 0, block_hash, VoteKind::Prevote);
    let b_prevote = InMemoryConsensus::sign_vote(&kb, 0, block_hash, VoteKind::Prevote);
    node_a.apply(&Frame::Vote(a_prevote.clone())).unwrap();
    node_b.apply(&Frame::Vote(b_prevote.clone())).unwrap();
    node_a.broadcast(&Frame::Vote(a_prevote)).unwrap();
    node_b.broadcast(&Frame::Vote(b_prevote)).unwrap();

    // Drain one incoming prevote on each side.
    assert_eq!(node_a.drain_and_apply(1, Duration::from_millis(2000)), 1);
    assert_eq!(node_b.drain_and_apply(1, Duration::from_millis(2000)), 1);

    // Precommits same dance.
    let a_precommit = InMemoryConsensus::sign_vote(&ka, 0, block_hash, VoteKind::Precommit);
    let b_precommit = InMemoryConsensus::sign_vote(&kb, 0, block_hash, VoteKind::Precommit);
    node_a.apply(&Frame::Vote(a_precommit.clone())).unwrap();
    node_b.apply(&Frame::Vote(b_precommit.clone())).unwrap();
    node_a.broadcast(&Frame::Vote(a_precommit)).unwrap();
    node_b.broadcast(&Frame::Vote(b_precommit)).unwrap();

    assert_eq!(node_a.drain_and_apply(1, Duration::from_millis(2000)), 1);
    assert_eq!(node_b.drain_and_apply(1, Duration::from_millis(2000)), 1);

    // Both sides finalize — both must see Committed. n=2, f=0, quorum=1,
    // so each side alone would commit, but we want both sides to independently
    // arrive at the same finality after the wire round-trips.
    let fa = node_a.consensus.finalize(0).unwrap();
    let fb = node_b.consensus.finalize(0).unwrap();
    assert_eq!(fa, BlockFinality::Committed, "node A did not reach Committed");
    assert_eq!(fb, BlockFinality::Committed, "node B did not reach Committed");

    // And both nodes must have the same block hash on their local ledger.
    let ba = node_a.consensus.block(0).unwrap();
    let bb = node_b.consensus.block(0).unwrap();
    assert_eq!(ba.hash(), bb.hash(), "blocks on each side must hash identically");
    // Payload survived the wire intact.
    if let SettlementOp::RankedAction { payload, .. } = &bb.ops[0] {
        assert_eq!(payload, b"hello-world-across-sockets");
    } else {
        panic!("expected RankedAction op");
    }
}

#[test]
fn four_validator_wire_quorum() {
    // n=4, f=1, quorum=3. Run on 4 real ports, everyone broadcasts to everyone,
    // require every node ends Committed.
    let kps: Vec<Keypair> = (0..4).map(|_| Keypair::generate()).collect();
    let ids: Vec<_> = kps.iter().map(|k| k.node_id()).collect();
    let vs = ValidatorSet::new(ids).unwrap();

    let nodes: Vec<Arc<Node>> = kps
        .iter()
        .map(|kp| Node::bind(kp.clone(), vs.clone(), "127.0.0.1:0").unwrap())
        .collect();

    // Fully connect: each node dials every other node.
    for i in 0..4 {
        for j in 0..4 {
            if i == j {
                continue;
            }
            nodes[i].dial(nodes[j].bound_addr()).unwrap();
        }
    }
    std::thread::sleep(Duration::from_millis(80));

    // Leader = kps[0], proposes height 0.
    let block = Block {
        height: 0,
        proposer: kps[0].node_id(),
        parent_hash: [0u8; 32],
        ops: vec![],
        finality: BlockFinality::Tentative,
    };
    let h = block.hash();
    nodes[0].apply(&Frame::Proposal(block.clone())).unwrap();
    nodes[0].broadcast(&Frame::Proposal(block.clone())).unwrap();

    // Other 3 nodes drain the proposal.
    for i in 1..4 {
        assert_eq!(
            nodes[i].drain_and_apply(1, Duration::from_millis(2000)),
            1,
            "node {i} missed proposal"
        );
    }

    // Every validator prevotes + broadcasts.
    for i in 0..4 {
        let v = InMemoryConsensus::sign_vote(&kps[i], 0, h, VoteKind::Prevote);
        nodes[i].apply(&Frame::Vote(v.clone())).unwrap();
        nodes[i].broadcast(&Frame::Vote(v)).unwrap();
    }
    // Each node should receive 3 prevotes (from each peer).
    for i in 0..4 {
        let got = nodes[i].drain_and_apply(3, Duration::from_millis(3000));
        assert_eq!(got, 3, "node {i} drained {got} prevotes, expected 3");
    }

    // Every validator precommits + broadcasts.
    for i in 0..4 {
        let v = InMemoryConsensus::sign_vote(&kps[i], 0, h, VoteKind::Precommit);
        nodes[i].apply(&Frame::Vote(v.clone())).unwrap();
        nodes[i].broadcast(&Frame::Vote(v)).unwrap();
    }
    for i in 0..4 {
        let got = nodes[i].drain_and_apply(3, Duration::from_millis(3000));
        assert_eq!(got, 3, "node {i} drained {got} precommits, expected 3");
    }

    // Every node must now see Committed.
    let finalities: Vec<_> = nodes
        .iter()
        .map(|n| n.consensus.finalize(0).unwrap())
        .collect();
    assert!(
        wait_for(2000, || finalities
            .iter()
            .all(|f| *f == BlockFinality::Committed)),
        "not all nodes Committed: {:?}",
        finalities
    );
    for (i, f) in finalities.iter().enumerate() {
        assert_eq!(*f, BlockFinality::Committed, "node {i} stuck at {f:?}");
    }

    // Sanity: every node sees the same block bytes.
    let canonical = nodes[0].consensus.block(0).unwrap().hash();
    for (i, n) in nodes.iter().enumerate() {
        assert_eq!(
            n.consensus.block(0).unwrap().hash(),
            canonical,
            "node {i} has a different block hash"
        );
    }
}

#[test]
fn wire_refuses_tampered_vote_signature() {
    // If an attacker flips a byte in a vote frame, the receiving consensus
    // must reject it on signature verification. Nobody should commit.
    let ka = Keypair::generate();
    let kb = Keypair::generate();
    let vs = ValidatorSet::new(vec![ka.node_id(), kb.node_id()]).unwrap();
    let node_b = Node::bind(kb.clone(), vs.clone(), "127.0.0.1:0").unwrap();

    // Build a legitimate signed vote from A, then tamper the block_hash field
    // after signing so the sig no longer matches.
    let h = [0x11u8; 32];
    let mut v = InMemoryConsensus::sign_vote(&ka, 0, h, VoteKind::Prevote);
    v.block_hash = [0xeeu8; 32]; // tamper

    // Applying to B's consensus must fail at signature verification.
    let err = node_b.consensus.vote(v);
    assert!(err.is_err(), "tampered vote must be rejected");
}
