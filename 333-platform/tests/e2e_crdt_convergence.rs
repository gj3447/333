// KG: finding_333_e2e_crdt_d4, plan-333-e2e-coverage-expansion-2026-04-16
//! E2E CRDT Convergence Tests
//!
//! Tests CRDT convergence properties for LwwMap and OrSet in realistic
//! multi-replica network scenarios: partition, heal, concurrent writes,
//! delete/re-add cycles, and large-scale state exchange.
//!
//! All tests are self-contained, no external resources required.

use triple_three::lww_map::LwwMap;

// ---------------------------------------------------------------------------
// 1. three_replica_convergence
// ---------------------------------------------------------------------------
/// Three replicas each write distinct keys concurrently, then exchange all
/// deltas. All three maps must converge to identical 9-key state.
#[test]
fn three_replica_convergence() {
    let mut r1: LwwMap<String, String> = LwwMap::new(1);
    let mut r2: LwwMap<String, String> = LwwMap::new(2);
    let mut r3: LwwMap<String, String> = LwwMap::new(3);

    // Each replica writes 3 unique keys independently (no network yet)
    let d1a = r1.set("r1_a".into(), "v1a".into());
    let d1b = r1.set("r1_b".into(), "v1b".into());
    let d1c = r1.set("r1_c".into(), "v1c".into());

    let d2a = r2.set("r2_a".into(), "v2a".into());
    let d2b = r2.set("r2_b".into(), "v2b".into());
    let d2c = r2.set("r2_c".into(), "v2c".into());

    let d3a = r3.set("r3_a".into(), "v3a".into());
    let d3b = r3.set("r3_b".into(), "v3b".into());
    let d3c = r3.set("r3_c".into(), "v3c".into());

    // Flood all deltas to all replicas
    for delta in [&d2a, &d2b, &d2c, &d3a, &d3b, &d3c] {
        r1.merge_delta(delta);
    }
    for delta in [&d1a, &d1b, &d1c, &d3a, &d3b, &d3c] {
        r2.merge_delta(delta);
    }
    for delta in [&d1a, &d1b, &d1c, &d2a, &d2b, &d2c] {
        r3.merge_delta(delta);
    }

    // All three must have 9 active keys
    assert_eq!(r1.len(), 9, "r1 should have 9 keys");
    assert_eq!(r2.len(), 9, "r2 should have 9 keys");
    assert_eq!(r3.len(), 9, "r3 should have 9 keys");

    // Spot-check cross-replica values
    for key in ["r1_a", "r1_b", "r1_c", "r2_a", "r2_b", "r2_c", "r3_a", "r3_b", "r3_c"] {
        let v1 = r1.get(&key.to_string());
        let v2 = r2.get(&key.to_string());
        let v3 = r3.get(&key.to_string());
        assert_eq!(v1, v2, "r1/r2 diverged on key={key}");
        assert_eq!(v2, v3, "r2/r3 diverged on key={key}");
        assert!(v1.is_some(), "key={key} missing");
    }
}

// ---------------------------------------------------------------------------
// 2. concurrent_same_key_lww
// ---------------------------------------------------------------------------
/// Three replicas all set the same key "x" concurrently. Deltas are exchanged
/// in arbitrary order. All must converge to the same value (highest Lamport wins,
/// tie-broken by higher node_id).
///
/// Because node 3 writes *after* receiving node 1 and 2's deltas, its counter
/// will be strictly higher, so "from_node3" must win.
#[test]
fn concurrent_same_key_lww() {
    let mut r1: LwwMap<String, String> = LwwMap::new(1);
    let mut r2: LwwMap<String, String> = LwwMap::new(2);
    let mut r3: LwwMap<String, String> = LwwMap::new(3);

    // r1 and r2 write "x" independently (counter 1 each, no causal link)
    let d1 = r1.set("x".into(), "from_node1".into());
    let d2 = r2.set("x".into(), "from_node2".into());

    // r3 first receives both deltas to advance its clock, then writes "x"
    // — its Lamport counter is guaranteed strictly > r1 and r2
    r3.merge_delta(&d1);
    r3.merge_delta(&d2);
    let d3 = r3.set("x".into(), "from_node3".into());

    // Exchange remaining deltas in a scrambled order
    // r1: receives d2, then d3
    r1.merge_delta(&d2);
    r1.merge_delta(&d3);

    // r2: receives d3, then d1 (reversed)
    r2.merge_delta(&d3);
    r2.merge_delta(&d1);

    // r3 already merged d1/d2 above; d3 is self-written (no re-merge needed)

    let v1 = r1.get(&"x".to_string());
    let v2 = r2.get(&"x".to_string());
    let v3 = r3.get(&"x".to_string());

    assert_eq!(v1, v2, "r1 and r2 diverged");
    assert_eq!(v2, v3, "r2 and r3 diverged");
    assert_eq!(
        v1,
        Some(&"from_node3".to_string()),
        "causally-last writer (node3) must win"
    );
}

// ---------------------------------------------------------------------------
// 3. delete_then_readd
// ---------------------------------------------------------------------------
/// Replica A sets "k"="v1", syncs to B.
/// A deletes "k", syncs to B.
/// A re-sets "k"="v2", syncs to B.
/// B must end up with "k"="v2".
#[test]
fn delete_then_readd() {
    let mut a: LwwMap<String, String> = LwwMap::new(1);
    let mut b: LwwMap<String, String> = LwwMap::new(2);

    // Phase 1: set and sync
    let d_set = a.set("k".into(), "v1".into());
    b.merge_delta(&d_set);
    assert_eq!(b.get(&"k".to_string()), Some(&"v1".to_string()));

    // Phase 2: delete and sync
    let d_del = a.delete("k".into());
    b.merge_delta(&d_del);
    assert_eq!(b.get(&"k".to_string()), None, "B should see deletion");

    // Phase 3: re-add and sync
    let d_readd = a.set("k".into(), "v2".into());
    b.merge_delta(&d_readd);
    assert_eq!(
        b.get(&"k".to_string()),
        Some(&"v2".to_string()),
        "B should see re-added value"
    );
    assert_eq!(b.len(), 1);
}

// ---------------------------------------------------------------------------
// 4. partition_heal_convergence
// ---------------------------------------------------------------------------
/// Network splits: {A} vs {B, C}. Each partition evolves independently.
/// On heal, all accumulated deltas are exchanged. All 3 converge.
#[test]
fn partition_heal_convergence() {
    let mut a: LwwMap<String, String> = LwwMap::new(1);
    let mut b: LwwMap<String, String> = LwwMap::new(2);
    let mut c: LwwMap<String, String> = LwwMap::new(3);

    // --- Pre-partition state shared by all ---
    let d_shared = a.set("shared".into(), "original".into());
    b.merge_delta(&d_shared);
    c.merge_delta(&d_shared);

    // --- Partition: A isolated, B+C in contact ---

    // A side: overwrites "shared", adds unique keys
    let da1 = a.set("shared".into(), "from_a".into());
    let da2 = a.set("a_only_1".into(), "alpha".into());
    let da3 = a.set("a_only_2".into(), "beta".into());

    // B+C side: B writes, C receives B's delta immediately
    let db1 = b.set("shared".into(), "from_b".into());
    let db2 = b.set("bc_shared".into(), "bc_val".into());
    c.merge_delta(&db1);
    c.merge_delta(&db2);
    let dc1 = c.set("c_only".into(), "gamma".into());
    b.merge_delta(&dc1);

    // --- Heal: A receives everything from B+C partition ---
    a.merge_delta(&db1);
    a.merge_delta(&db2);
    a.merge_delta(&dc1);

    // B+C receive everything from A partition
    b.merge_delta(&da1);
    b.merge_delta(&da2);
    b.merge_delta(&da3);
    c.merge_delta(&da1);
    c.merge_delta(&da2);
    c.merge_delta(&da3);

    // All three should have 5 keys: shared, a_only_1, a_only_2, bc_shared, c_only
    assert_eq!(a.len(), 5, "A should have 5 keys after heal");
    assert_eq!(b.len(), 5, "B should have 5 keys after heal");
    assert_eq!(c.len(), 5, "C should have 5 keys after heal");

    // All must agree on every key
    for key in ["shared", "a_only_1", "a_only_2", "bc_shared", "c_only"] {
        let va = a.get(&key.to_string());
        let vb = b.get(&key.to_string());
        let vc = c.get(&key.to_string());
        assert_eq!(va, vb, "A/B diverged on key={key}");
        assert_eq!(vb, vc, "B/C diverged on key={key}");
        assert!(va.is_some(), "key={key} missing after heal");
    }

    // "shared" must have the same winner on all replicas.
    // A wrote after merging nothing; B wrote independently; B's write had a
    // lower counter than A's subsequent write or vice-versa — the point is
    // that all three agree, regardless of which value won.
    let winner = a.get(&"shared".to_string()).unwrap();
    assert!(
        *winner == "from_a" || *winner == "from_b",
        "winner must be one of the concurrent values, got: {winner}"
    );
}

// ---------------------------------------------------------------------------
// 5. large_state_convergence
// ---------------------------------------------------------------------------
/// 3 replicas each create 1000 unique keys. After full-state merge, all
/// three have 3000 active keys with identical values.
#[test]
fn large_state_convergence() {
    let mut r1: LwwMap<String, String> = LwwMap::new(1);
    let mut r2: LwwMap<String, String> = LwwMap::new(2);
    let mut r3: LwwMap<String, String> = LwwMap::new(3);

    // Write 1000 unique keys per replica (disjoint keyspaces)
    let mut deltas1 = Vec::with_capacity(1000);
    let mut deltas2 = Vec::with_capacity(1000);
    let mut deltas3 = Vec::with_capacity(1000);

    for i in 0..1000u32 {
        deltas1.push(r1.set(format!("r1_{i}"), format!("val_r1_{i}")));
        deltas2.push(r2.set(format!("r2_{i}"), format!("val_r2_{i}")));
        deltas3.push(r3.set(format!("r3_{i}"), format!("val_r3_{i}")));
    }

    // Exchange: each replica receives the other two's deltas
    for d in &deltas2 { r1.merge_delta(d); }
    for d in &deltas3 { r1.merge_delta(d); }

    for d in &deltas1 { r2.merge_delta(d); }
    for d in &deltas3 { r2.merge_delta(d); }

    for d in &deltas1 { r3.merge_delta(d); }
    for d in &deltas2 { r3.merge_delta(d); }

    // All should have 3000 active keys
    assert_eq!(r1.len(), 3000, "r1 should have 3000 keys");
    assert_eq!(r2.len(), 3000, "r2 should have 3000 keys");
    assert_eq!(r3.len(), 3000, "r3 should have 3000 keys");

    // Spot-check values at boundaries
    for (prefix, node_r) in [("r1", &r1), ("r2", &r2), ("r3", &r3)] {
        let probe_key = format!("{prefix}_500");
        let expected = format!("val_{prefix}_500");
        // All three replicas must agree
        let v1 = r1.get(&probe_key);
        let v2 = r2.get(&probe_key);
        let v3 = r3.get(&probe_key);
        assert_eq!(v1, Some(&expected), "r1 missing {probe_key}");
        assert_eq!(v2, Some(&expected), "r2 missing {probe_key}");
        assert_eq!(v3, Some(&expected), "r3 missing {probe_key}");
        let _ = node_r; // suppress unused warning
    }
}

// ---------------------------------------------------------------------------
// 6. causal_ordering_hlc
// ---------------------------------------------------------------------------
/// Replica A sets "x"="1". Replica B receives the delta (establishing causal
/// ordering via Lamport recv), then sets "x"="2" — B's write is causally after
/// A's because merge_delta advances B's clock past A's counter.
/// Both must converge to "2".
#[test]
fn causal_ordering_hlc() {
    let mut a: LwwMap<String, String> = LwwMap::new(1);
    let mut b: LwwMap<String, String> = LwwMap::new(2);

    // A writes "x"="1"
    let da = a.set("x".into(), "1".into());

    // B receives A's delta — B's Lamport clock is now advanced past A's counter
    b.merge_delta(&da);
    assert_eq!(b.get(&"x".to_string()), Some(&"1".to_string()));

    // B writes "x"="2" (causally after A's write, because B's counter > A's)
    let db = b.set("x".into(), "2".into());

    // A receives B's update
    a.merge_delta(&db);

    // Both replicas must agree that "2" wins (B wrote causally later)
    let va = a.get(&"x".to_string());
    let vb = b.get(&"x".to_string());

    assert_eq!(va, vb, "A and B diverged");
    assert_eq!(
        va,
        Some(&"2".to_string()),
        "causally-later write (B='2') must win over earlier write (A='1')"
    );
}
