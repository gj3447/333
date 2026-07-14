//! Integration test: Full BFT pipeline
//!
//! Transport → HotStuff → Executor
//! 7 validators, submit tx, 3-phase commit, execute, verify balances.

use triple_three::bft::crypto::{sign_standalone as sign, NodeId, ValidatorSet};
use triple_three::crypto_real::Identity;
use triple_three::bft::executor::Executor;
use triple_three::bft::state::HotStuffState;
use triple_three::bft::transport::{InMemoryNetwork, Transport};
use triple_three::bft::types::*;

const VALIDATORS: [NodeId; 7] = [1, 2, 3, 4, 5, 6, 7];

struct TestNode {
    state: HotStuffState,
    net: InMemoryNetwork,
    executor: Executor,
}

fn test_identity(node_id: NodeId) -> Identity {
    let mut seed = [0u8; 32];
    seed[..4].copy_from_slice(&node_id.to_be_bytes());
    Identity::from_seed(&seed)
}

fn setup() -> Vec<TestNode> {
    let vs = ValidatorSet::new(VALIDATORS.to_vec());
    let nets = InMemoryNetwork::create(&VALIDATORS);

    let mut nodes: Vec<TestNode> = nets.into_iter()
        .zip(VALIDATORS.iter())
        .map(|(net, &id)| {
            let mut executor = Executor::new();
            for &v in &VALIDATORS {
                executor.init_account(v, 10000);
            }
            TestNode {
                state: HotStuffState::with_identity(id, vs.clone(), test_identity(id)),
                net,
                executor,
            }
        })
        .collect();

    // KG: lesson-333-bft-keyring-exchange-2026-04-14 — register all peer keys
    for node in &mut nodes {
        for &vid in &VALIDATORS {
            if vid != node.state.node_id {
                node.state.register_peer_identity(vid, test_identity(vid));
            }
        }
    }
    nodes
}

/// Drive one round of consensus to completion
fn run_consensus_round(nodes: &mut [TestNode]) -> bool {
    // Find leader
    let leader_idx = nodes
        .iter()
        .position(|n| n.state.is_leader())
        .expect("no leader found");

    // Leader proposes
    let proposal = match nodes[leader_idx].state.propose() {
        Some(msg) => msg,
        None => return false,
    };

    // Broadcast proposal
    let block_hash = if let HotStuffMsg::Proposal { ref block, .. } = proposal {
        block.hash
    } else {
        return false;
    };

    // === 3 phases ===
    let phases = [Phase::Prepare, Phase::PreCommit, Phase::Commit];

    let mut current_proposal = proposal;

    for &phase in &phases {
        // Deliver proposal to all validators
        for (i, node) in nodes.iter_mut().enumerate() {
            if i == leader_idx {
                continue;
            }
            node.net.send(
                node.state.node_id,
                current_proposal.clone(),
            );
        }

        // Each validator processes proposal and generates vote
        let mut votes = vec![];
        for (i, node) in nodes.iter_mut().enumerate() {
            if i == leader_idx {
                continue;
            }
            if let Some((_, msg)) = node.net.recv() {
                if let ProcessResult::SendToLeader(vote) = node.state.process(msg) {
                    votes.push(vote);
                }
            }
        }

        // Leader self-vote
        votes.push(HotStuffMsg::Vote {
            block_hash,
            view: nodes[leader_idx].state.view,
            phase,
            signature: sign(nodes[leader_idx].state.node_id, block_hash),
        });

        // Leader collects votes
        let mut next_msg = None;
        for vote in votes {
            match nodes[leader_idx].state.process(vote) {
                ProcessResult::Broadcast(msg) => {
                    next_msg = Some(msg);
                    break;
                }
                ProcessResult::Committed(txs, _new_view) => {
                    // Execute committed transactions on leader
                    nodes[leader_idx].executor.execute_block(&txs);
                    // Execute on all validators too
                    for (i, node) in nodes.iter_mut().enumerate() {
                        if i != leader_idx {
                            node.executor.execute_block(&txs);
                        }
                    }
                    return true;
                }
                _ => {}
            }
        }

        if let Some(msg) = next_msg {
            current_proposal = msg;
        }
    }

    false
}

#[test]
fn full_pipeline_token_transfer() {
    let mut nodes = setup();

    // Submit transfer: node 1 sends 500 to node 2
    nodes[0].state.submit_tx(OrderedTx::Transfer {
        from: 1,
        to: 2,
        amount: 500,
        nonce: 0,
    });

    // Run consensus
    let committed = run_consensus_round(&mut nodes);
    assert!(committed, "block should be committed");

    // Verify balances on ALL nodes (consistency)
    for node in &nodes {
        assert_eq!(node.executor.balance(&1), 9500, "sender balance");
        assert_eq!(node.executor.balance(&2), 10500, "receiver balance");
    }
}

#[test]
fn supply_conservation_across_rounds() {
    let mut nodes = setup();
    let initial_supply = nodes[0].executor.total_supply();

    // Round 1: node 1 → node 3, 1000 tokens
    nodes[0].state.submit_tx(OrderedTx::Transfer {
        from: 1,
        to: 3,
        amount: 1000,
        nonce: 0,
    });
    assert!(run_consensus_round(&mut nodes));

    // Advance all nodes to new view
    for node in &mut nodes {
        node.state.view = 1;
    }

    // Round 2: node 3 → node 5, 500 tokens
    // Leader for view 1 is node 2
    nodes[1].state.submit_tx(OrderedTx::Transfer {
        from: 3,
        to: 5,
        amount: 500,
        nonce: 0,
    });
    assert!(run_consensus_round(&mut nodes));

    // Supply must be conserved
    for node in &nodes {
        assert_eq!(
            node.executor.total_supply(),
            initial_supply,
            "total supply must be conserved after {} rounds",
            2
        );
    }
}

#[test]
fn all_validators_agree_on_state() {
    let mut nodes = setup();

    // Multiple transfers in one block
    nodes[0].state.submit_tx(OrderedTx::Transfer {
        from: 1, to: 2, amount: 100, nonce: 0,
    });
    nodes[0].state.submit_tx(OrderedTx::Transfer {
        from: 3, to: 4, amount: 200, nonce: 0,
    });
    nodes[0].state.submit_tx(OrderedTx::Transfer {
        from: 5, to: 6, amount: 300, nonce: 0,
    });

    assert!(run_consensus_round(&mut nodes));

    // All validators must have identical state
    let reference_balances: Vec<_> = VALIDATORS.iter()
        .map(|id| nodes[0].executor.balance(id))
        .collect();

    for (i, node) in nodes.iter().enumerate() {
        let balances: Vec<_> = VALIDATORS.iter()
            .map(|id| node.executor.balance(id))
            .collect();
        assert_eq!(balances, reference_balances,
            "node {} state diverges from reference", i);
    }
}

#[test]
fn mixed_transaction_types() {
    let mut nodes = setup();

    nodes[0].state.submit_tx(OrderedTx::Transfer {
        from: 1, to: 7, amount: 250, nonce: 0,
    });
    nodes[0].state.submit_tx(OrderedTx::RankedAction {
        player: 1, action_type: 42, payload: vec![1, 2, 3],
    });
    nodes[0].state.submit_tx(OrderedTx::AuctionBid {
        bidder: 2, item_id: 99, amount: 100,
    });

    assert!(run_consensus_round(&mut nodes));

    // Transfer executed
    assert_eq!(nodes[0].executor.balance(&1), 9750);
    assert_eq!(nodes[0].executor.balance(&7), 10250);

    // Auction bid deducted from bidder
    assert_eq!(nodes[0].executor.balance(&2), 9900);
}
