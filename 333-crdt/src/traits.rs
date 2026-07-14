// KG: TASK_ATOM_L5_GP_TraitContracts, CONTRACT_ATOM_L5_GP_TraitContracts
// Foundation CRDT traits (Crdt + AutoCrdt) ported from garage::util::crdt::crdt
//
// Mathematical axioms (enforced by tests/invariants.rs):
//   a ⊔ b = b ⊔ a                  (commutativity)
//   (a ⊔ b) ⊔ c = a ⊔ (b ⊔ c)      (associativity)
//   (a ⊔ b) ⊔ b = a ⊔ b            (idempotence)

/// A CRDT type. Merge must be commutative, associative, idempotent, monotonic.
pub trait Crdt {
    /// Merge `other` into `self`. `other` is not modified.
    fn merge(&mut self, other: &Self);
}

/// Types with `Ord` can implement a trivial CRDT via `a ⊔ b = max(a, b)`.
pub trait AutoCrdt: Ord + Clone + std::fmt::Debug {}

// Blanket impl: any AutoCrdt automatically gets Crdt via max semantics.
impl<T> Crdt for T
where
    T: AutoCrdt,
{
    fn merge(&mut self, other: &Self) {
        if other > self {
            *self = other.clone();
        }
    }
}

// Option<T> where T: Eq: conflicting values collapse to None.
// Cannot overlap with AutoCrdt blanket (Option doesn't impl Ord generically).
impl<T: Eq + Clone> Crdt for Option<T> {
    fn merge(&mut self, other: &Self) {
        if self != other {
            *self = None;
        }
    }
}

// Common AutoCrdt instances for primitives.
impl AutoCrdt for u8 {}
impl AutoCrdt for u16 {}
impl AutoCrdt for u32 {}
impl AutoCrdt for u64 {}
impl AutoCrdt for i8 {}
impl AutoCrdt for i16 {}
impl AutoCrdt for i32 {}
impl AutoCrdt for i64 {}
impl AutoCrdt for String {}

#[cfg(test)]
mod smoke {
    use super::*;

    #[test]
    fn u32_auto_crdt_max_wins() {
        let mut a: u32 = 5;
        a.merge(&10);
        assert_eq!(a, 10);
        a.merge(&3);
        assert_eq!(a, 10);
    }

    #[test]
    fn option_conflict_to_none() {
        let mut a: Option<u32> = Some(5);
        a.merge(&Some(10));
        assert_eq!(a, None);

        let mut b: Option<u32> = Some(5);
        b.merge(&Some(5));
        assert_eq!(b, Some(5));
    }
}
