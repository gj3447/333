// KG: finding_333_e2e_stress_d9, plan-333-e2e-coverage-expansion-2026-04-16
//! E2E Stress Tests — large state, high throughput, edge cases
//!
//! Smoke-level stress (CI-friendly, <10s). Medium/max deferred to nightly.

use triple_three::platform::{PlatformCore, ExecutionRequest, ExecutionResponse};
use triple_three::bft::types::OrderedTx;
use triple_three::tokenomics::{TokenTx, TokenResult};
use triple_three::kernel::JobPriority;

const VALIDATORS: &[u32] = &[1, 2, 3, 4, 5, 6, 7];

fn make_platform() -> PlatformCore {
    PlatformCore::new(1, VALIDATORS)
}

// ── 1. Large CRDT state (1000 blocks) ──────────────────────────────────────

#[test]
fn stress_1000_crdt_blocks() {
    let mut p = make_platform();
    for i in 0..1000 {
        let resp = p.execute(ExecutionRequest::CrdtUpdate {
            key: format!("x:{},y:{}", i % 100, i / 100),
            value: Some(format!("block_{}", i)),
        });
        match resp {
            ExecutionResponse::CrdtDelta(_) => {}
            _ => panic!("expected CrdtDelta at block {}", i),
        }
    }
    assert_eq!(p.world_size(), 1000);
    // Spot check
    assert_eq!(p.get_block("x:42,y:3"), Some(&"block_342".to_string()));
    assert_eq!(p.get_block("x:0,y:0"), Some(&"block_0".to_string()));
    assert_eq!(p.get_block("x:99,y:9"), Some(&"block_999".to_string()));
}

// ── 2. CRDT overwrite stress (same key 1000 times) ─────────────────────────

#[test]
fn stress_1000_overwrites_same_key() {
    let mut p = make_platform();
    for i in 0..1000 {
        p.execute(ExecutionRequest::CrdtUpdate {
            key: "contested_key".into(),
            value: Some(format!("v{}", i)),
        });
    }
    // Last write wins
    assert_eq!(p.get_block("contested_key"), Some(&"v999".to_string()));
    assert_eq!(p.world_size(), 1);
}

// ── 3. Token transfer chain ────────────────────────────────────────────────

#[test]
fn stress_token_transfer_chain() {
    let mut p = make_platform();
    // Chain: 1→2→3→4→5→6→7, each passing 1000 tokens
    // Initial: everyone has 10000
    for i in 0..6 {
        let from = VALIDATORS[i];
        let to = VALIDATORS[i + 1];
        let resp = p.execute(ExecutionRequest::Token(TokenTx::Transfer {
            from, to, amount: 1000, nonce: 0,
        }));
        match resp {
            ExecutionResponse::TokenResult(TokenResult::Success) => {}
            other => panic!("transfer {}→{} failed: {:?}", from, to, other),
        }
    }
    // Node 1: 10000-1000 = 9000
    assert_eq!(p.balance(1), 9000);
    // Middle nodes: received 1000, sent 1000 = 10000
    for &v in &VALIDATORS[1..6] {
        assert_eq!(p.balance(v), 10000, "node {} balance wrong", v);
    }
    // Node 7: 10000+1000 = 11000
    assert_eq!(p.balance(7), 11000);
}

// ── 4. Rapid CRDT + Token interleave ───────────────────────────────────────

#[test]
fn stress_crdt_token_interleave_500() {
    let mut p = make_platform();
    for i in 0..500u64 {
        // CRDT update
        p.execute(ExecutionRequest::CrdtUpdate {
            key: format!("k{}", i),
            value: Some(format!("v{}", i)),
        });
        // Token micro-transfer (1 token each)
        if i < 100 {
            p.execute(ExecutionRequest::Token(TokenTx::Transfer {
                from: 1, to: 2, amount: 1, nonce: i,
            }));
        }
    }
    assert_eq!(p.world_size(), 500);
    assert_eq!(p.balance(1), 10000 - 100);
    assert_eq!(p.balance(2), 10000 + 100);
}

// ── 5. Kernel work queue saturation ────────────────────────────────────────

#[test]
fn stress_kernel_500_jobs() {
    use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
    let mut p = make_platform();
    let counter = Arc::new(AtomicU32::new(0));

    for i in 0..500 {
        let c = Arc::clone(&counter);
        let prio = match i % 4 {
            0 => JobPriority::Critical,
            1 => JobPriority::High,
            2 => JobPriority::Normal,
            _ => JobPriority::Low,
        };
        p.enqueue_work(format!("stress_{}", i), prio, move || {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
    }

    assert_eq!(p.pending_work(), 500);

    // Drain in batches of 50
    let mut total_ran = 0;
    for _ in 0..20 {
        let ran = p.tick_kernel(50);
        total_ran += ran;
        if p.pending_work() == 0 { break; }
    }
    assert_eq!(total_ran, 500);
    assert_eq!(counter.load(Ordering::SeqCst), 500);
}

// ── 6. Storage growth tracking ─────────────────────────────────────────────

#[test]
fn stress_storage_grows_with_operations() {
    let mut p = make_platform();
    let initial_size = p.storage_size();

    // 100 CRDT operations
    for i in 0..100 {
        p.execute(ExecutionRequest::CrdtUpdate {
            key: format!("s{}", i),
            value: Some(format!("d{}", i)),
        });
    }
    let after_crdt = p.storage_size();
    assert!(after_crdt > initial_size, "storage should grow after CRDT ops");

    // 10 committed blocks via on_commit
    for i in 0..10 {
        p.on_commit(&[OrderedTx::Transfer {
            from: 1, to: 2, amount: 1, nonce: i as u64,
        }]);
    }
    let after_commit = p.storage_size();
    assert!(after_commit > after_crdt, "storage should grow after commits");
}

// ── 7. CRDT delta sync at scale (3 nodes × 100 blocks each) ───────────────

#[test]
fn stress_multi_node_sync_300_blocks() {
    let mut nodes: Vec<PlatformCore> = (1..=3u32)
        .map(|id| PlatformCore::new(id, VALIDATORS))
        .collect();

    // Each node creates 100 unique blocks, collecting deltas
    let mut all_deltas: Vec<Vec<String>> = vec![vec![], vec![], vec![]];
    for (idx, node) in nodes.iter_mut().enumerate() {
        for i in 0..100 {
            let resp = node.execute(ExecutionRequest::CrdtUpdate {
                key: format!("node{}_{}", idx + 1, i),
                value: Some(format!("val_{}_{}", idx + 1, i)),
            });
            if let ExecutionResponse::CrdtDelta(json) = resp {
                all_deltas[idx].push(json);
            }
        }
    }

    // Cross-sync: each node receives deltas from the other two
    for target in 0..3 {
        for source in 0..3 {
            if source == target { continue; }
            for delta_json in &all_deltas[source] {
                nodes[target].merge_remote_delta(delta_json);
            }
        }
    }

    // All nodes should have 300 blocks
    for (idx, node) in nodes.iter().enumerate() {
        assert_eq!(node.world_size(), 300, "node {} should have 300 blocks", idx + 1);
    }

    // Spot check cross-node values
    for node in &nodes {
        assert_eq!(node.get_block("node1_50"), Some(&"val_1_50".to_string()));
        assert_eq!(node.get_block("node2_99"), Some(&"val_2_99".to_string()));
        assert_eq!(node.get_block("node3_0"), Some(&"val_3_0".to_string()));
    }
}
