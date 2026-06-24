//! 333 v2 substrate — relay metering -> credit-bucket, **LTDD-verified**.
//!
//! A Super-Peer relays bytes for ephemeral clients; usage is metered into a credit bucket, and
//! when the bucket can't pay the relay GATES (refuses). Per the OSS survey this maps to
//! `coturn metered -> OpenMeter -> a credit gate (OpenMeter + Stripe on-ramp)`. Here the metering
//! + bucket ACCOUNTING is the deterministic domain logic — independent of the libp2p relay
//! transport — so it is fully testable without the network.
//!
//! HARD RULE (LTDD): the verdict is read back from the store, never the return value. A relay
//! that reports "forwarded ok" while the debit silently never lands is FREE-RIDING; the
//! conservation invariant `sum(cost@relay_forwarded) == sum(amount@credit_debited)` catches it.
//! That invariant is a *value-consistency* relation (ooptdd's `invariant` / the queryable-causal
//! rung), strictly stronger than asserting the events merely exist.

use p333_ltdd::{Event, Store};

/// A prepaid credit balance. Debits are all-or-nothing and never go negative.
#[derive(Debug, Clone)]
pub struct CreditBucket {
    balance: u64,
}

impl CreditBucket {
    /// A bucket pre-funded with `balance` credits.
    pub fn new(balance: u64) -> Self {
        Self { balance }
    }

    /// Remaining credits.
    pub fn balance(&self) -> u64 {
        self.balance
    }

    /// Debit `amount` iff the balance covers it (no overdraw). Returns whether it was charged.
    fn try_debit(&mut self, amount: u64) -> bool {
        if self.balance >= amount {
            self.balance -= amount;
            true
        } else {
            false
        }
    }
}

/// Credits charged to forward `bytes`, at `rate_per_kib` credits per (ceil'd) KiB.
pub fn cost(bytes: u64, rate_per_kib: u64) -> u64 {
    bytes.div_ceil(1024) * rate_per_kib
}

/// Meter + bill one relay forward, emitting the LTDD trace to `store`:
/// - on success: `relay_forwarded{bytes,cost}` **and** `credit_debited{amount,remaining}`,
/// - when the bucket can't pay: `relay_gated{requested_bytes,reason}` and the bytes are refused.
///
/// Returns whether the bytes were forwarded. The store is the judge: a caller that trusts this
/// returning `true` without the paired events landing is exactly the free-riding the conservation
/// invariant exists to catch.
pub fn relay_forward(
    store: &mut impl Store,
    cid: &str,
    bucket: &mut CreditBucket,
    bytes: u64,
    rate_per_kib: u64,
) -> bool {
    let c = cost(bytes, rate_per_kib);
    if bucket.try_debit(c) {
        store.ship(&[
            Event::new(cid, "relay_forwarded")
                .with("bytes", bytes)
                .with("cost", c),
            Event::new(cid, "credit_debited")
                .with("amount", c)
                .with("remaining", bucket.balance()),
        ]);
        true
    } else {
        store.ship(&[Event::new(cid, "relay_gated")
            .with("requested_bytes", bytes)
            .with("reason", "insufficient_credit")]);
        false
    }
}
