//! 333 v2 substrate — relay billing, composing two LTDD-verified lanes.
//!
//! A prepaid credit account is modelled as an **owned object** (Lane A / `p333-consensus`): each
//! debit is a consistent-broadcast spend at the account's current version, so a replayed debit
//! message can't double-charge. The debited amount is the **metered cost** (Lane A metering /
//! `p333-metering`: conservation). The two invariants compose into replay-safe, conserved billing —
//! and the LTDD receipt asserts both at once over the emitted trace.

use p333_consensus::Ledger;
use p333_ltdd::{Event, Store};
use p333_metering::cost;

/// A prepaid credit account whose balance is guarded as an owned object (versioned spends).
pub struct Account {
    object: String,
    balance: u64,
    ledger: Ledger,
}

impl Account {
    /// A new account `object` pre-funded with `balance` credits (owned object at version 0).
    pub fn new(object: &str, balance: u64) -> Self {
        let mut ledger = Ledger::default();
        ledger.register(object);
        Self { object: object.to_string(), balance, ledger }
    }

    /// Remaining credits.
    pub fn balance(&self) -> u64 {
        self.balance
    }

    /// The account object's current (unspent) version.
    pub fn version(&self) -> u64 {
        self.ledger.current_version(&self.object).unwrap_or(0)
    }

    /// Meter `bytes`, then debit the cost as a finalized owned-object spend. On success emits
    /// `spend_finalized` (the owned-object debit, Lane A) + `relay_forwarded{cost}` /
    /// `credit_debited{amount}` (the metering, conservation); when the bucket can't pay, `relay_gated`.
    /// Returns whether the bytes were forwarded.
    pub fn meter_and_bill(
        &mut self,
        store: &mut impl Store,
        cid: &str,
        bytes: u64,
        rate_per_kib: u64,
    ) -> bool {
        let c = cost(bytes, rate_per_kib);
        if self.balance < c {
            store.ship(&[Event::new(cid, "relay_gated")
                .with("requested_bytes", bytes)
                .with("reason", "insufficient_credit")]);
            return false;
        }
        let version = self.version();
        // spend the credit-object at the current version (advances it; a replay at this version is
        // henceforth rejected). This is the no-double-debit guarantee, from Lane A.
        let finalized = self.ledger.spend(store, cid, &self.object, version, "debit");
        debug_assert!(finalized, "spend at the current version always finalizes");
        self.balance -= c;
        store.ship(&[
            Event::new(cid, "relay_forwarded").with("bytes", bytes).with("cost", c),
            Event::new(cid, "credit_debited")
                .with("amount", c)
                .with("version", version)
                .with("remaining", self.balance),
        ]);
        finalized
    }

    /// Replay a debit message at `version`. If `version` is stale (already spent), the owned-object
    /// ledger rejects it (`equivocation_rejected`) — the replay does NOT charge the account again.
    pub fn replay_debit_at(&mut self, store: &mut impl Store, cid: &str, version: u64) -> bool {
        self.ledger.spend(store, cid, &self.object, version, "replayed-debit")
    }
}
