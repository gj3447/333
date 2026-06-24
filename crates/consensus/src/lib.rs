//! 333 v2 substrate — Lane A: owned-object consistency (Sui-Lutris / FastPay style; production
//! adopts a Sui-Lutris/FastPay engine, with Linera as a readable reference).
//!
//! OWNED (single-writer) objects don't need global consensus — a consistent broadcast finalizes
//! the owner's spend, and the only thing that can go wrong is EQUIVOCATION: the owner signing two
//! conflicting spends of the same object version (a double-spend). The safety law: each object
//! version finalizes AT MOST ONCE; a conflicting/stale spend is rejected. LTDD framing: read the
//! finalizations back from the store and assert exactly-once (`spend_finalized == 1` per version) —
//! a broken broadcast that double-finalizes turns the count check RED.

use p333_ltdd::{Event, Store};
use std::collections::HashMap;

/// A minimal owned-object ledger: each object id maps to its current (unspent) version. A spend
/// matching the current version finalizes and advances it; any other version is an equivocation.
#[derive(Default)]
pub struct Ledger {
    versions: HashMap<String, u64>,
}

impl Ledger {
    /// Register a fresh object at version 0.
    pub fn register(&mut self, object: &str) {
        self.versions.insert(object.to_string(), 0);
    }

    /// The object's current (unspent) version, if registered.
    pub fn current_version(&self, object: &str) -> Option<u64> {
        self.versions.get(object).copied()
    }

    /// Attempt to spend `object` at `version`. Finalizes (advancing the version, emitting
    /// `spend_finalized`) iff `version` is the current one; otherwise it is a conflicting/replayed
    /// spend and is rejected (`equivocation_rejected`). Returns whether it finalized.
    pub fn spend(
        &mut self,
        store: &mut impl Store,
        cid: &str,
        object: &str,
        version: u64,
        txn: &str,
    ) -> bool {
        match self.versions.get(object).copied() {
            Some(cur) if cur == version => {
                self.versions.insert(object.to_string(), cur + 1);
                store.ship(&[Event::new(cid, "spend_finalized")
                    .with("object", object)
                    .with("version", version)
                    .with("txn", txn)]);
                true
            }
            _ => {
                store.ship(&[Event::new(cid, "equivocation_rejected")
                    .with("object", object)
                    .with("version", version)
                    .with("txn", txn)]);
                false
            }
        }
    }
}
