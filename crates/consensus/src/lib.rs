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
    /// Register a fresh object at version 0, emitting `object_registered` —
    /// registration is a judged transition like any other (wal design §1
    /// gap 1: an event-sourced store cannot rebuild what never shipped).
    pub fn register(&mut self, store: &mut impl Store, cid: &str, object: &str) {
        self.versions.insert(object.to_string(), 0);
        store.ship(&[Event::new(cid, "object_registered").with("object", object)]);
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

    /// [`Ledger::spend`], but the decision comes back only as
    /// [`Externalized<bool>`] — proof the finalize/reject event reached
    /// stable storage (DUR-6 sync-then-send: p333 has no consensus behind
    /// it, so signing/answering an un-persisted spend is amnesia
    /// equivocation waiting for a restart). On a sync failure there is no
    /// value to act on: the caller must refuse the ack.
    #[cfg(all(feature = "wal", unix))]
    pub fn spend_durable(
        &mut self,
        store: &mut p333_ltdd::wal_store::WalStore,
        cid: &str,
        object: &str,
        version: u64,
        txn: &str,
    ) -> Result<p333_ltdd::wal_store::Externalized<bool>, p333_ltdd::wal_store::WalStoreError> {
        let finalized = self.spend(store, cid, object, version, txn);
        store.externalize(finalized)
    }
}
