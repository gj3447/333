// KG: SPAN_333_L11_Consensus, plan-333-p2p-os-synthesis-execution-2026-04-18
// Integration: multi-height commits, leader rotation, concurrent voters, progressive finality.

use consensus333::*;
use identity333::Keypair;

fn setup(n: usize) -> (InMemoryConsensus, Vec<Keypair>) {
    let kps: Vec<Keypair> = (0..n).map(|_| Keypair::generate()).collect();
    let ids = kps.iter().map(|k| k.node_id()).collect();
    let vs = ValidatorSet::new(ids).unwrap();
    (InMemoryConsensus::new(vs), kps)
}

fn block_at(height: u64, proposer: identity333::NodeId, parent_hash: [u8; 32]) -> Block {
    Block {
        height,
        proposer,
        parent_hash,
        ops: vec![SettlementOp::AuctionBid {
            bidder: Keypair::generate().node_id(),
            item: format!("item-{height}"),
            amount: height * 10,
        }],
        finality: BlockFinality::Tentative,
    }
}

#[test]
fn multi_height_chain_commits() {
    let (c, kps) = setup(4);
    let mut parent = [0u8; 32];
    for h in 0..5 {
        let leader = c.validators().leader(h).clone();
        let b = block_at(h, leader, parent);
        let hash = b.hash();
        c.propose(b).unwrap();
        for kp in &kps {
            c.vote(InMemoryConsensus::sign_vote(kp, h, hash, VoteKind::Prevote)).unwrap();
            c.vote(InMemoryConsensus::sign_vote(kp, h, hash, VoteKind::Precommit)).unwrap();
        }
        assert_eq!(c.finalize(h).unwrap(), BlockFinality::Committed);
        parent = hash;
    }
}

#[test]
fn progressive_finality_prevote_then_precommit() {
    let (c, kps) = setup(4);
    let b = block_at(0, kps[0].node_id(), [0u8; 32]);
    let h = b.hash();
    c.propose(b).unwrap();

    // Step 1: only 2 prevotes (below quorum 3)
    c.vote(InMemoryConsensus::sign_vote(&kps[0], 0, h, VoteKind::Prevote)).unwrap();
    c.vote(InMemoryConsensus::sign_vote(&kps[1], 0, h, VoteKind::Prevote)).unwrap();
    assert_eq!(c.finalize(0).unwrap(), BlockFinality::Tentative);

    // Step 2: third prevote → Confirmed
    c.vote(InMemoryConsensus::sign_vote(&kps[2], 0, h, VoteKind::Prevote)).unwrap();
    assert_eq!(c.finalize(0).unwrap(), BlockFinality::Confirmed);

    // Step 3: 3 precommits → Committed
    for kp in &kps[..3] {
        c.vote(InMemoryConsensus::sign_vote(kp, 0, h, VoteKind::Precommit)).unwrap();
    }
    assert_eq!(c.finalize(0).unwrap(), BlockFinality::Committed);
}

#[test]
fn concurrent_voters() {
    use std::sync::Arc;
    use std::thread;

    let (c, kps) = setup(7);
    let c = Arc::new(c);
    let leader = c.validators().leader(0).clone();
    let b = block_at(0, leader, [0u8; 32]);
    let h = b.hash();
    c.propose(b).unwrap();

    let mut handles = vec![];
    for kp in kps.clone() {
        let c2 = c.clone();
        handles.push(thread::spawn(move || {
            c2.vote(InMemoryConsensus::sign_vote(&kp, 0, h, VoteKind::Prevote)).unwrap();
            c2.vote(InMemoryConsensus::sign_vote(&kp, 0, h, VoteKind::Precommit)).unwrap();
        }));
    }
    for j in handles {
        j.join().unwrap();
    }
    assert_eq!(c.finalize(0).unwrap(), BlockFinality::Committed);
}

#[test]
fn leader_rotates_across_heights() {
    let (c, _kps) = setup(3);
    let l0 = c.validators().leader(0).clone();
    let l1 = c.validators().leader(1).clone();
    let l2 = c.validators().leader(2).clone();
    let l3 = c.validators().leader(3).clone();
    assert_ne!(l0, l1);
    assert_ne!(l1, l2);
    assert_eq!(l0, l3);
}

#[test]
fn two_thirds_byzantine_prevent_commit() {
    // n=4, f=1; if 2 byzantine don't vote, only 2 votes < quorum 3.
    let (c, kps) = setup(4);
    let b = block_at(0, kps[0].node_id(), [0u8; 32]);
    let h = b.hash();
    c.propose(b).unwrap();
    for kp in &kps[..2] {
        c.vote(InMemoryConsensus::sign_vote(kp, 0, h, VoteKind::Prevote)).unwrap();
        c.vote(InMemoryConsensus::sign_vote(kp, 0, h, VoteKind::Precommit)).unwrap();
    }
    assert_eq!(c.finalize(0).unwrap(), BlockFinality::Tentative);
}
