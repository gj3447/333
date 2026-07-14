// KG: TASK_ATOM_L5_GP_ScalarCrdt, CONTRACT_ATOM_L5_GP_ScalarCrdt
// Simple scalar CRDTs: Bool (absorbing true) + Deletable<T> (tombstone)

use serde::{Deserialize, Serialize};

use crate::traits::Crdt;

/// Boolean CRDT where `true` is an absorbing state (merge = OR).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bool(bool);

impl Bool {
    pub fn new(b: bool) -> Self {
        Self(b)
    }
    pub fn set(&mut self) {
        self.0 = true;
    }
    pub fn get(&self) -> bool {
        self.0
    }
}

impl From<bool> for Bool {
    fn from(b: bool) -> Self {
        Self(b)
    }
}

impl Crdt for Bool {
    fn merge(&mut self, other: &Self) {
        self.0 = self.0 || other.0;
    }
}

/// Tombstone CRDT: once deleted, cannot revert. Deleted absorbs Present.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Deletable<T> {
    Present(T),
    Deleted,
}

impl<T: Crdt> Deletable<T> {
    pub fn present(v: T) -> Self {
        Self::Present(v)
    }
    pub fn delete() -> Self {
        Self::Deleted
    }
    pub fn is_deleted(&self) -> bool {
        matches!(self, Self::Deleted)
    }
    pub fn as_option(&self) -> Option<&T> {
        match self {
            Self::Present(v) => Some(v),
            Self::Deleted => None,
        }
    }
    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Present(v) => Some(v),
            Self::Deleted => None,
        }
    }
}

impl<T: Crdt> Crdt for Deletable<T> {
    fn merge(&mut self, other: &Self) {
        match (&mut *self, other) {
            (Self::Deleted, _) => {} // self already deleted, absorbing
            (_, Self::Deleted) => *self = Self::Deleted,
            (Self::Present(v), Self::Present(v2)) => v.merge(v2),
        }
    }
}

#[cfg(test)]
mod smoke {
    use super::*;

    #[test]
    fn bool_or_truth_table() {
        let cases = [
            (false, false, false),
            (false, true, true),
            (true, false, true),
            (true, true, true),
        ];
        for (a, b, expected) in cases {
            let mut x = Bool::new(a);
            x.merge(&Bool::new(b));
            assert_eq!(x.get(), expected, "{a}||{b}={expected}");
        }
    }

    #[test]
    fn deletable_tombstone_absorbs() {
        let mut a: Deletable<u32> = Deletable::present(5);
        a.merge(&Deletable::Deleted);
        assert_eq!(a, Deletable::Deleted);

        let mut b: Deletable<u32> = Deletable::Deleted;
        b.merge(&Deletable::present(10));
        assert_eq!(b, Deletable::Deleted); // Deleted is absorbing
    }

    #[test]
    fn deletable_present_merges_inner() {
        let mut a: Deletable<u32> = Deletable::present(5);
        a.merge(&Deletable::present(10));
        assert_eq!(a, Deletable::present(10)); // inner u32 max
    }
}
