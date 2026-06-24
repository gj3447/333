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

/// A byte source the relay meters. The PRODUCTION impl is a libp2p Circuit-Relay-v2 stream; this
/// in-process [`Loopback`] lets the metered byte→credit flow be receipted WITHOUT the network (the
/// libp2p relay needs a public reservation address — out of scope in the sandbox, see crates/relay).
pub trait Transport {
    /// The next chunk of bytes to forward, or `None` when the stream is exhausted.
    fn next_chunk(&mut self) -> Option<u64>;
}

/// An in-process transport replaying a fixed list of byte-chunk sizes.
pub struct Loopback {
    chunks: std::vec::IntoIter<u64>,
}

impl Loopback {
    pub fn new(chunks: Vec<u64>) -> Self {
        Self { chunks: chunks.into_iter() }
    }
}

impl Transport for Loopback {
    fn next_chunk(&mut self) -> Option<u64> {
        self.chunks.next()
    }
}

/// Wire a transport end-to-end to the credit gate: pull byte chunks and meter+bill each until the
/// stream ends or the bucket gates. Returns how many chunks were forwarded before any gate. The
/// same `relay_forward` accounting runs whether the bytes come from `Loopback` or a real libp2p relay.
pub fn run_session(
    store: &mut impl Store,
    cid: &str,
    transport: &mut impl Transport,
    bucket: &mut CreditBucket,
    rate_per_kib: u64,
) -> usize {
    let mut forwarded = 0;
    while let Some(bytes) = transport.next_chunk() {
        if relay_forward(store, cid, bucket, bytes, rate_per_kib) {
            forwarded += 1;
        } else {
            break; // gated: the bucket can't pay -> stop the session
        }
    }
    forwarded
}
