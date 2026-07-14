// KG: finding_333_e2e_integration_d11, plan-333-e2e-coverage-expansion-2026-04-16
//! Cross-module E2E integration tests for the 333 Platform.
//!
//! Validates concurrent operation of CRDT + BFT + Kernel + Tokenomics.
//! All tests are self-contained; no external services or network required.

use triple_three::platform::{PlatformCore, ExecutionRequest, ExecutionResponse};
use triple_three::bft::types::OrderedTx;
use triple_three::tokenomics::{TokenTx, TokenResult};
use triple_three::kernel::{JobPriority, Capability};

const VALIDATORS: &[u32] = &[1, 2, 3, 4, 5, 6, 7];

fn make_core(node_id: u32) -> PlatformCore {
    PlatformCore::new(node_id, VALIDATORS)
}

// ── 1. crdt_and_token_concurrent ──────────────────────────────────────────────
/// 100 CRDT updates AND 5 token transfers on the same PlatformCore.
/// Verifies both subsystems remain consistent: world_size==100, balances correct.
#[test]
fn crdt_and_token_concurrent() {
    let mut core = make_core(1);

    // 100 CRDT updates
    for i in 0..100u32 {
        let resp = core.execute(ExecutionRequest::CrdtUpdate {
            key: format!("block:{}", i),
            value: Some(format!("stone_{}", i)),
        });
        assert!(
            matches!(resp, ExecutionResponse::CrdtDelta(_)),
            "CRDT update {} should return CrdtDelta, got {:?}",
            i, resp
        );
    }

    // 5 token transfers: node 1 sends 100 each to nodes 2..=6
    for to in 2u32..=6 {
        let resp = core.execute(ExecutionRequest::Token(TokenTx::Transfer {
            from: 1,
            to,
            amount: 100,
            nonce: (to - 2) as u64,
        }));
        assert!(
            matches!(resp, ExecutionResponse::TokenResult(TokenResult::Success)),
            "token transfer to {} should succeed, got {:?}",
            to, resp
        );
    }

    // CRDT consistency: all 100 blocks present
    assert_eq!(core.world_size(), 100, "world_size must be 100 after 100 updates");
    for i in 0..100u32 {
        assert_eq!(
            core.get_block(&format!("block:{}", i)),
            Some(&format!("stone_{}", i)),
            "block:{} missing from world",
            i
        );
    }

    // Token consistency: node 1 sent 500 total (5 × 100)
    let expected_sender = 10_000 - 500;
    assert_eq!(core.balance(1), expected_sender, "node 1 balance after 5 transfers");
    for to in 2u32..=6 {
        assert_eq!(core.balance(to), 10_000 + 100, "node {} balance after receiving 100", to);
    }
    // Nodes 7 untouched
    assert_eq!(core.balance(7), 10_000, "node 7 balance should be unchanged");
}

// ── 2. kernel_work_during_crdt ────────────────────────────────────────────────
/// Enqueue 10 kernel jobs, do 5 CRDT updates, then tick kernel.
/// Verifies CRDT state is correct AND all 10 jobs were executed.
#[test]
fn kernel_work_during_crdt() {
    use std::sync::{Arc, atomic::{AtomicU32, Ordering}};

    let mut core = make_core(1);
    let counter = Arc::new(AtomicU32::new(0));

    // Enqueue 10 kernel jobs before any CRDT work
    for i in 0..10u32 {
        let c = Arc::clone(&counter);
        core.enqueue_work(format!("job_{}", i), JobPriority::Normal, move || {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
    }
    assert_eq!(core.pending_work(), 10, "10 jobs must be pending before tick");

    // 5 CRDT updates while jobs are enqueued but not yet executed
    for i in 0..5u32 {
        let resp = core.execute(ExecutionRequest::CrdtUpdate {
            key: format!("key:{}", i),
            value: Some(format!("val:{}", i)),
        });
        assert!(
            matches!(resp, ExecutionResponse::CrdtDelta(_)),
            "CRDT update {} should succeed",
            i
        );
    }

    // Tick kernel — flush all 10 jobs
    let executed = core.tick_kernel(20);
    assert_eq!(executed, 10, "tick_kernel must execute all 10 jobs");
    assert_eq!(core.pending_work(), 0, "no jobs should remain after tick");
    assert_eq!(
        counter.load(Ordering::SeqCst), 10,
        "counter must equal 10 after all jobs run"
    );

    // CRDT state unaffected by kernel work
    assert_eq!(core.world_size(), 5, "world_size must be 5 after 5 CRDT updates");
    for i in 0..5u32 {
        assert_eq!(
            core.get_block(&format!("key:{}", i)),
            Some(&format!("val:{}", i)),
            "key:{} missing",
            i
        );
    }
}

// ── 3. token_transfer_depletes_balance ───────────────────────────────────────
/// Transfer all 10 000 tokens from node 1 to node 2.
/// A subsequent transfer must fail with InsufficientBalance.
#[test]
fn token_transfer_depletes_balance() {
    let mut core = make_core(1);

    // Transfer entire balance
    let resp = core.execute(ExecutionRequest::Token(TokenTx::Transfer {
        from: 1,
        to: 2,
        amount: 10_000,
        nonce: 0,
    }));
    assert!(
        matches!(resp, ExecutionResponse::TokenResult(TokenResult::Success)),
        "full balance transfer should succeed, got {:?}",
        resp
    );
    assert_eq!(core.balance(1), 0, "node 1 should have 0 balance after full transfer");
    assert_eq!(core.balance(2), 20_000, "node 2 should have 20 000 after receiving 10 000");

    // Attempt to transfer 1 token from now-empty node 1
    let resp2 = core.execute(ExecutionRequest::Token(TokenTx::Transfer {
        from: 1,
        to: 3,
        amount: 1,
        nonce: 1,
    }));
    match resp2 {
        ExecutionResponse::TokenResult(TokenResult::InsufficientBalance) => { /* expected */ }
        other => panic!(
            "expected InsufficientBalance after depleting balance, got {:?}",
            other
        ),
    }
    // Balances must be unchanged after the failed transfer
    assert_eq!(core.balance(1), 0, "node 1 balance must remain 0 after failed transfer");
    assert_eq!(core.balance(3), 10_000, "node 3 balance must be unaffected");
}

// ── 4. storage_persists_across_operations ────────────────────────────────────
/// Verify storage grows after a CRDT update and again after a token transfer.
#[test]
fn storage_persists_across_operations() {
    let mut core = make_core(1);

    let size_initial = core.storage_size();

    // After a CRDT update the delta is written to "last_delta"
    core.execute(ExecutionRequest::CrdtUpdate {
        key: "terrain:0,0".into(),
        value: Some("grass".into()),
    });
    let size_after_crdt = core.storage_size();
    assert!(
        size_after_crdt > size_initial,
        "storage must grow after CRDT update (was {}, now {})",
        size_initial, size_after_crdt
    );

    // After a committed block the block JSON is written to "block:N"
    let txs = vec![OrderedTx::Transfer {
        from: 1, to: 2, amount: 50, nonce: 0,
    }];
    core.on_commit(&txs);
    let size_after_commit = core.storage_size();
    assert!(
        size_after_commit > size_after_crdt,
        "storage must grow after on_commit (was {}, now {})",
        size_after_crdt, size_after_commit
    );
}

// ── 5. capability_enforcement_cross_module ────────────────────────────────────
/// Enqueue work with a wrong capability → denied.
/// Enqueue work with correct capability → accepted and executed.
#[test]
fn capability_enforcement_cross_module() {
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

    let mut core = make_core(1);

    // Wrong capability: platform:crdt does NOT hold ConsensusSubmit
    let denied = core.enqueue_work_with_cap(
        "bad_job",
        JobPriority::Normal,
        &Capability::ConsensusSubmit,
        "platform:crdt",
        || Ok(()),
    );
    assert!(denied.is_err(), "job with wrong capability must be denied");
    assert!(
        denied.unwrap_err().contains("capability denied"),
        "error message must mention 'capability denied'"
    );
    assert_eq!(core.pending_work(), 0, "denied job must not enter the queue");

    // Correct capability: platform:crdt holds WorldWrite
    let ran = Arc::new(AtomicBool::new(false));
    let ran_clone = Arc::clone(&ran);
    let accepted = core.enqueue_work_with_cap(
        "good_job",
        JobPriority::Normal,
        &Capability::WorldWrite,
        "platform:crdt",
        move || {
            ran_clone.store(true, Ordering::SeqCst);
            Ok(())
        },
    );
    assert!(accepted.is_ok(), "job with correct capability must be accepted");
    assert_eq!(core.pending_work(), 1, "accepted job must be in the queue");

    let executed = core.tick_kernel(10);
    assert_eq!(executed, 1, "tick_kernel must execute the accepted job");
    assert!(ran.load(Ordering::SeqCst), "job closure must have run");
}

// ── 6. multi_node_crdt_sync ───────────────────────────────────────────────────
/// Three PlatformCores. Node 1 writes 10 blocks and emits deltas.
/// Each delta is merged into nodes 2 and 3. All three must agree on world state.
#[test]
fn multi_node_crdt_sync() {
    let mut node1 = make_core(1);
    let mut node2 = make_core(2);
    let mut node3 = make_core(3);

    // Node 1 writes 10 blocks, collects all deltas
    let mut deltas: Vec<String> = Vec::new();
    for i in 0..10u32 {
        let resp = node1.execute(ExecutionRequest::CrdtUpdate {
            key: format!("chunk:{}", i),
            value: Some(format!("biome_{}", i)),
        });
        match resp {
            ExecutionResponse::CrdtDelta(json) => deltas.push(json),
            other => panic!("expected CrdtDelta for write {}, got {:?}", i, other),
        }
    }
    assert_eq!(deltas.len(), 10, "must have collected 10 deltas");

    // Merge all deltas into nodes 2 and 3
    for delta in &deltas {
        node2.merge_remote_delta(delta);
        node3.merge_remote_delta(delta);
    }

    // All three nodes must agree on world size
    assert_eq!(node1.world_size(), 10, "node 1 world_size must be 10");
    assert_eq!(node2.world_size(), 10, "node 2 world_size must be 10 after merge");
    assert_eq!(node3.world_size(), 10, "node 3 world_size must be 10 after merge");

    // All blocks must have identical values across nodes
    for i in 0..10u32 {
        let key = format!("chunk:{}", i);
        let expected = Some(&format!("biome_{}", i));
        assert_eq!(
            node1.get_block(&key), expected,
            "node 1 block {} mismatch", i
        );
        assert_eq!(
            node2.get_block(&key), expected,
            "node 2 block {} mismatch after sync", i
        );
        assert_eq!(
            node3.get_block(&key), expected,
            "node 3 block {} mismatch after sync", i
        );
    }

    // Nodes 2 and 3 must have unchanged token balances (CRDT sync doesn't touch tokens)
    for &nid in &[2u32, 3] {
        assert_eq!(
            node2.balance(nid), 10_000,
            "node {} balance on node2 must be untouched by CRDT sync", nid
        );
        assert_eq!(
            node3.balance(nid), 10_000,
            "node {} balance on node3 must be untouched by CRDT sync", nid
        );
    }
}
