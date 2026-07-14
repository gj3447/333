// KG: TASK_ATOM_L5_GP_Invariants, CONTRACT_ATOM_L5_GP_Invariants
// Property-based verification of the three CRDT axioms:
//   commutativity:  a ⊔ b = b ⊔ a
//   associativity:  (a ⊔ b) ⊔ c = a ⊔ (b ⊔ c)
//   idempotence:    a ⊔ a = a
// For every concrete impl: u32, Bool, Deletable<u32>, Lww<u32>, Map<u32, u32>, LwwMap<u32, u32>.

use crdt333::{Bool, Crdt, Deletable, Lww, LwwMap, Map};
use proptest::prelude::*;

// ---------- generic assertion helpers ----------

fn assert_commutative<C: Crdt + Clone + PartialEq + std::fmt::Debug>(a: C, b: C) {
    let mut ab = a.clone();
    ab.merge(&b);
    let mut ba = b.clone();
    ba.merge(&a);
    assert_eq!(ab, ba, "commutativity violated");
}

fn assert_associative<C: Crdt + Clone + PartialEq + std::fmt::Debug>(a: C, b: C, c: C) {
    let mut left = a.clone();
    left.merge(&b);
    left.merge(&c);

    let mut right_bc = b.clone();
    right_bc.merge(&c);
    let mut right = a.clone();
    right.merge(&right_bc);

    assert_eq!(left, right, "associativity violated");
}

fn assert_idempotent<C: Crdt + Clone + PartialEq + std::fmt::Debug>(a: C) {
    let mut once = a.clone();
    once.merge(&a);
    assert_eq!(once, a, "idempotence violated");
}

// ---------- u32 (AutoCrdt via max) ----------

proptest! {
    #[test]
    fn u32_commutative(a: u32, b: u32) { assert_commutative(a, b); }

    #[test]
    fn u32_associative(a: u32, b: u32, c: u32) { assert_associative(a, b, c); }

    #[test]
    fn u32_idempotent(a: u32) { assert_idempotent(a); }
}

// ---------- Bool ----------

proptest! {
    #[test]
    fn bool_commutative(a: bool, b: bool) {
        assert_commutative(Bool::new(a), Bool::new(b));
    }

    #[test]
    fn bool_associative(a: bool, b: bool, c: bool) {
        assert_associative(Bool::new(a), Bool::new(b), Bool::new(c));
    }

    #[test]
    fn bool_idempotent(a: bool) { assert_idempotent(Bool::new(a)); }
}

// ---------- Deletable<u32> ----------

fn arb_deletable() -> impl Strategy<Value = Deletable<u32>> {
    prop_oneof![
        Just(Deletable::Deleted),
        any::<u32>().prop_map(Deletable::Present),
    ]
}

proptest! {
    #[test]
    fn deletable_commutative(a in arb_deletable(), b in arb_deletable()) {
        assert_commutative(a, b);
    }

    #[test]
    fn deletable_associative(a in arb_deletable(), b in arb_deletable(), c in arb_deletable()) {
        assert_associative(a, b, c);
    }

    #[test]
    fn deletable_idempotent(a in arb_deletable()) { assert_idempotent(a); }
}

// ---------- Lww<u32> ----------

fn arb_lww() -> impl Strategy<Value = Lww<u32>> {
    (0u64..1000, 0u32..1000).prop_map(|(ts, v)| Lww::raw(ts, v))
}

proptest! {
    #[test]
    fn lww_commutative(a in arb_lww(), b in arb_lww()) {
        assert_commutative(a, b);
    }

    #[test]
    fn lww_associative(a in arb_lww(), b in arb_lww(), c in arb_lww()) {
        assert_associative(a, b, c);
    }

    #[test]
    fn lww_idempotent(a in arb_lww()) { assert_idempotent(a); }
}

// ---------- Map<u32, u32> ----------

fn arb_map() -> impl Strategy<Value = Map<u32, u32>> {
    prop::collection::vec((0u32..20, 0u32..100), 0..5).prop_map(|entries| {
        let mut m = Map::new();
        for (k, v) in entries {
            m.put(k, v);
        }
        m
    })
}

proptest! {
    #[test]
    fn map_commutative(a in arb_map(), b in arb_map()) {
        assert_commutative(a, b);
    }

    #[test]
    fn map_associative(a in arb_map(), b in arb_map(), c in arb_map()) {
        assert_associative(a, b, c);
    }

    #[test]
    fn map_idempotent(a in arb_map()) { assert_idempotent(a); }
}

// ---------- LwwMap<u32, u32> ----------

fn arb_lwwmap() -> impl Strategy<Value = LwwMap<u32, u32>> {
    prop::collection::vec((0u32..20, 0u64..500, 0u32..100), 0..5).prop_map(|entries| {
        let mut m = LwwMap::new();
        for (k, ts, v) in entries {
            m.put(k, ts, v);
        }
        m
    })
}

proptest! {
    #[test]
    fn lwwmap_commutative(a in arb_lwwmap(), b in arb_lwwmap()) {
        assert_commutative(a, b);
    }

    #[test]
    fn lwwmap_associative(a in arb_lwwmap(), b in arb_lwwmap(), c in arb_lwwmap()) {
        assert_associative(a, b, c);
    }

    #[test]
    fn lwwmap_idempotent(a in arb_lwwmap()) { assert_idempotent(a); }
}
