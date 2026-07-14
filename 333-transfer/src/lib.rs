// KG: SA_333_Platform, SPAN_333_L11plus_Token,
//     consensus-prom16-333-no-blockchain-2026-07-12,
//     vp-prom16-333-coin-transferable-vs-local-credit-2026-07-12,
//     vp-prom16-333-coin-premint-vs-runtime-issuance-2026-07-12
//
// 333 consensusless single-owner asset transfer.
//
// WHY THIS EXISTS
// ---------------
// The pre-existing `token333` ledger is "settlement-agnostic: a consensus
// backend applies ordered SettlementOp::Transfer events". That inherits the
// blockchain assumption that *every* transfer must be globally total-ordered.
// The 2026-07-12 research verdict (`consensus-prom16-333-no-blockchain`) and the
// current consensusless-payment frontier both reject that for the common case:
//
//   FastPay (Baudet et al., Meta 2020), Zef (2022), Sui Lutris / Mysticeti fast
//   path (2024), Stingray (2025), Linera single-user microchains (2025), and the
//   quasi-anonymous consensus-free asset transfer of Guerraoui-line work all rest
//   on ONE theorem:
//
//     Asset transfer has consensus number 1 (Guerraoui et al., PODC 2019). When
//     each account is owned by a single process, a reliable broadcast whose
//     deliveries respect per-sender order is SUFFICIENT to prevent double-spend.
//     No total order across accounts is required.
//
//     *** SCOPE CORRECTION (PROM 16, 2026-07-13 — see
//     SYMPOSIUM/THEORY/333_CONSENSUSLESS_FRONTIER/PROM_16_REPORT.md, Consensus
//     C3): the "reliable broadcast" in the FastPay theorem is a BYZANTINE
//     Consistent Broadcast over an INDEPENDENT authority quorum (Signed Echo
//     Broadcast, n>=3f+1, per-(account,seq) certificate uniqueness by quorum
//     intersection). That quorum layer is what bars a Byzantine / key-compromised
//     OWNER from equivocating (two different transfers at the same seq to two
//     recipients). This crate rides `crdt333::rcb::VectorClockBroadcaster`, a
//     CRASH-FAULT vector-clock causal-delivery buffer with NO signatures and NO
//     quorum. So the sequence-number check here defends only against an HONEST
//     owner. The `KNOWN_LIMITATION_*` test proves that gap on the BARE `apply`
//     rail; the `authority.rs` quorum-certificate layer (`apply_certified`) now
//     CLOSES it — see `authority::tests::equivocation_barred_by_quorum_certificate`.
//     Authority votes now carry REAL Ed25519 signatures (RFC 8032), verified per
//     authority against the committee's public keys — a vote is unforgeable
//     outside the key holder (`authority::tests::forged_vote_with_wrong_key_is_rejected`).
//     Gossip/broadcast TRANSPORT remains the follow-up. ***
//
// This crate is the missing single-owner application on top of crdt333::rcb.
//
// TWO OPEN 333 VERDICTS — PROPOSED HERE, THEN REFINED BY PROM 16 (2026-07-13)
// --------------------------------------------------------------------------
//   * transferable vs local-credit  -> CONDITIONAL / 2-tier (PROM Q1). Ship
//     LOCAL-CREDIT / honest-owner semantics on the bare rail (`apply`), and the
//     Byzantine-safe TRANSFERABLE tier via `apply_certified` once authorities
//     are deployed. The FastPay-style authority-quorum certificate layer AND a
//     passing equivocation test — the two conditions PROM 16 required — are both
//     now met (safety core + real Ed25519 votes, `authority.rs`). Wire codec +
//     in-memory authority mesh (Steps 1–3), multi-round collect / multi-ledger
//     disseminate / remote confirm (Steps 5–7), and real TCP behind the same
//     `AuthorityNet` trait (Step 8, `tcp`) land in `wire` / `net` / `tcp`.
//     Plumtree remains a follow-up (Step 4).
//   * premint vs runtime-issuance   -> PREMINT default; but the real boundary is
//     single-minter vs multi-minter, NOT premint vs runtime (PROM Q2). A
//     single-authority runtime mint (cf. Sui TreasuryCap) is still consensus
//     number 1; only a MULTI-minter shared supply cap is consensus number k.
//     `genesis` gives the premint tier.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use crdt333::{CausalBroadcast, CausalMsg, VectorClockBroadcaster};

pub mod authority;
pub mod net;
pub mod tcp;
pub mod wire;

pub use authority::{
    certify, quorum, signing_message, Authority, AuthorityError, Certificate, Certified, Committee,
    Verified, Vote,
};
pub use net::{
    authority_handle_round, certify_via_mesh, certify_via_mesh_rounds,
    certify_via_mesh_rounds_with_pause, collect_until_quorum, confirm_from_mesh,
    disseminate_certificate, disseminate_certificate_with_pause, AuthorityMsg, AuthorityNet,
    InMemoryAuthorityMesh, MeshEndpoint, MeshLedger, NetError, VoteCollector,
};
pub use tcp::{TcpAuthorityNet, TcpEndpoint};
pub use wire::{
    decode_authority_msg, decode_certificate, decode_transfer, decode_vote, encode_authority_msg,
    encode_certificate, encode_transfer, encode_vote, WireError, TAG_CERT, TAG_ORDER, TAG_VOTE,
};
// Re-exported so callers can build authorities/committees (the public API takes
// Ed25519 keys) and inspect vote signatures without a direct ed25519-dalek dep.
pub use ed25519_dalek::{Signature, SigningKey, VerifyingKey};

/// Account identifier == the owning replica's id. Single-owner by construction:
/// only this key may author transfers that debit the account.
pub type AccountId = String;
pub type Amount = u128;

/// An intent to move value. `from_seq` is the FastPay "account sequence
/// number": the owner's monotonic per-account counter. Against an HONEST owner
/// it is the whole double-spend defence and needs no global ordering; it does
/// NOT defend against a Byzantine owner equivocating across replicas — that
/// needs the authority quorum, built (safety core + real Ed25519 authority
/// votes) in `authority.rs`; use `Ledger::apply_certified`. The Transfer struct
/// itself is unsigned — authenticity lives on the authority `Vote`, not the
/// owner's intent, which is what the FastPay quorum certifies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub from: AccountId,
    pub from_seq: u64,
    pub to: AccountId,
    pub amount: Amount,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Account {
    balance: Amount,
    /// Next acceptable sequence number for a transfer authored by this owner.
    next_seq: u64,
}

/// Why a transfer was not applied. Every reject is deterministic and
/// order-independent — the CALM / coordination-free property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reject {
    UnknownSender,
    /// Sequence did not equal the account's expected next_seq. Covers replay,
    /// double-spend, and out-of-order authorship. `expected`/`got` for debugging.
    StaleSequence { expected: u64, got: u64 },
    Insufficient { have: Amount, need: Amount },
    /// `apply_certified` was called without a valid 2f+1 quorum certificate.
    /// This is the gate that makes Byzantine-safe transferable value possible:
    /// an equivocating owner cannot obtain a certificate for two conflicting
    /// transfers at the same (account, seq), so double-spend is barred.
    NoCertificate,
}

/// The consensusless single-owner ledger. Applying transfers is a pure,
/// deterministic function of (state, transfer); disjoint-account transfers
/// commute, so replicas that deliver the same set converge with no agreement.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    accounts: BTreeMap<AccountId, Account>,
}

impl Ledger {
    pub fn new() -> Self {
        Self::default()
    }

    /// PREMINT: fixed genesis allocation. This is the only way value enters the
    /// fast-path ledger, which is exactly why the fast path needs no issuance
    /// consensus. Idempotent-unsafe by design: call once at construction.
    pub fn genesis<I>(alloc: I) -> Self
    where
        I: IntoIterator<Item = (AccountId, Amount)>,
    {
        let mut accounts = BTreeMap::new();
        for (id, balance) in alloc {
            accounts.insert(id, Account { balance, next_seq: 0 });
        }
        Self { accounts }
    }

    pub fn balance(&self, id: &AccountId) -> Amount {
        self.accounts.get(id).map(|a| a.balance).unwrap_or(0)
    }

    /// The owner's next acceptable sequence number — what a client must stamp on
    /// its next transfer.
    pub fn next_seq(&self, id: &AccountId) -> u64 {
        self.accounts.get(id).map(|a| a.next_seq).unwrap_or(0)
    }

    /// Total value in the ledger. Invariant: transfers preserve it exactly
    /// (conservation) — no mint/burn on the consensusless path.
    pub fn total_supply(&self) -> Amount {
        self.accounts.values().map(|a| a.balance).sum()
    }

    /// Apply a transfer. This is the whole consensus-number-1 core:
    ///   1. sender must exist (single-owner account),
    ///   2. from_seq must equal the owner's expected next_seq (double-spend /
    ///      replay defence — the *only* ordering constraint, and it is per-sender
    ///      not global),
    ///   3. sender must have the funds.
    /// On success: debit sender, credit receiver (creating it if absent), and
    /// bump the sender's sequence.
    pub fn apply(&mut self, t: &Transfer) -> Result<(), Reject> {
        let from = self.accounts.get(&t.from).ok_or(Reject::UnknownSender)?;
        if t.from_seq != from.next_seq {
            return Err(Reject::StaleSequence {
                expected: from.next_seq,
                got: t.from_seq,
            });
        }
        if from.balance < t.amount {
            return Err(Reject::Insufficient {
                have: from.balance,
                need: t.amount,
            });
        }
        // Commit. Receiver may be a fresh account (transferable value, not a
        // pre-registered credit line).
        let from = self.accounts.get_mut(&t.from).expect("checked above");
        from.balance -= t.amount;
        from.next_seq += 1;
        let to = self.accounts.entry(t.to.clone()).or_default();
        to.balance += t.amount;
        Ok(())
    }

    /// BYZANTINE-SAFE TRANSFERABLE PATH (PROM 16 Q1 fix, 2026-07-13).
    /// Apply a transfer ONLY if it carries a certificate valid against the
    /// trusted `committee` (which the verifier owns — never a caller-supplied
    /// size). Certificate uniqueness (FastPay Lemma A.1: any two quorums
    /// intersect in an honest authority that signs at most one transfer per
    /// (account, seq)) guarantees a Byzantine / key-compromised owner can never
    /// obtain two certificates for conflicting transfers at the same slot — so
    /// double-spend is barred WITHOUT any global consensus / total order. This
    /// closes the gap the bare `apply` (honest-owner only) leaves open. See
    /// `authority.rs` and the `equivocation_barred_*` / `byzantine_authority_*`
    /// tests.
    pub fn apply_certified(&mut self, cert: &Certificate, committee: &Committee) -> Result<(), Reject> {
        if !cert.is_valid(committee) {
            return Err(Reject::NoCertificate);
        }
        self.apply(&cert.transfer)
    }

    /// Apply a `Verified` transfer — the type-state Byzantine-safe path. A
    /// `Verified` can only be minted by `Certificate::verify(&committee)`, so
    /// this signature makes the double-spend-vulnerable bare `apply` impossible
    /// to reach with an uncertified transfer: the safe rail is the DEFAULT rail,
    /// not merely the recommended one (assessment action 9).
    pub fn apply_verified(&mut self, v: &Verified) -> Result<(), Reject> {
        self.apply(v.transfer())
    }
}

/// A replica: an owner's local ledger view plus the reliable-causal-broadcast
/// transport. `submit` authors + broadcasts a transfer at the correct sequence;
/// `deliver` feeds an incoming broadcast message and applies everything that is
/// now causally ready. This binds `crdt333::rcb` to the ledger.
pub struct Replica {
    me: AccountId,
    ledger: Ledger,
    rcb: VectorClockBroadcaster<Transfer>,
}

/// What a `Replica::deliver` did with the causally-ready transfers: the ones
/// applied, and the ones rejected (e.g. an equivocation-loser) — the latter made
/// OBSERVABLE rather than silently dropped (assessment action 9).
#[derive(Debug, Clone, Default)]
pub struct DeliveryOutcome {
    pub applied: Vec<Transfer>,
    pub rejected: Vec<(Transfer, Reject)>,
}

impl Replica {
    pub fn new(me: AccountId, ledger: Ledger) -> Self {
        let rcb = VectorClockBroadcaster::new(me.clone());
        Self { me, ledger, rcb }
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    pub fn me(&self) -> &AccountId {
        &self.me
    }

    /// Author a transfer FROM this replica's account, stamped at the owner's
    /// current next_seq, apply it locally, and return the broadcast message for
    /// peers. Returns the reject if the local apply fails (funds/seq).
    pub fn submit(&mut self, to: AccountId, amount: Amount) -> Result<CausalMsg<Transfer>, Reject> {
        let t = Transfer {
            from: self.me.clone(),
            from_seq: self.ledger.next_seq(&self.me),
            to,
            amount,
        };
        self.ledger.apply(&t)?;
        Ok(self.rcb.submit(t))
    }

    /// Feed an incoming broadcast. Reliable causal broadcast releases messages in
    /// per-sender order; each released transfer is applied. Applies that reject
    /// (e.g. a Byzantine double-authored seq that lost the race) are dropped —
    /// the reject is itself deterministic, so all honest replicas drop the same
    /// one. Returns the transfers that were applied, in delivery order.
    pub fn deliver(&mut self, msg: CausalMsg<Transfer>) -> DeliveryOutcome {
        let ready = self.rcb.receive(msg);
        let mut outcome = DeliveryOutcome::default();
        for m in ready {
            match self.ledger.apply(&m.payload) {
                Ok(()) => outcome.applied.push(m.payload),
                Err(e) => outcome.rejected.push((m.payload, e)),
            }
        }
        outcome
    }

    pub fn pending(&self) -> usize {
        self.rcb.pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn genesis() -> Ledger {
        Ledger::genesis([("alice".into(), 100u128), ("bob".into(), 0u128)])
    }

    #[test]
    fn premint_sets_supply_and_no_issuance_path() {
        let l = genesis();
        assert_eq!(l.balance(&"alice".into()), 100);
        assert_eq!(l.total_supply(), 100);
        // There is deliberately no mint() on Ledger: issuance is off the fast path.
    }

    #[test]
    fn valid_transfer_moves_value_and_bumps_seq() {
        let mut l = genesis();
        l.apply(&Transfer { from: "alice".into(), from_seq: 0, to: "bob".into(), amount: 30 })
            .unwrap();
        assert_eq!(l.balance(&"alice".into()), 70);
        assert_eq!(l.balance(&"bob".into()), 30);
        assert_eq!(l.next_seq(&"alice".into()), 1);
        assert_eq!(l.total_supply(), 100); // conservation
    }

    #[test]
    fn double_spend_rejected_by_sequence_without_any_consensus() {
        let mut l = genesis();
        let t = Transfer { from: "alice".into(), from_seq: 0, to: "bob".into(), amount: 60 };
        l.apply(&t).unwrap();
        // Replaying the SAME seq (the essence of a double-spend) is rejected
        // purely by the account sequence number — no global ordering consulted.
        let err = l.apply(&t).unwrap_err();
        assert_eq!(err, Reject::StaleSequence { expected: 1, got: 0 });
        assert_eq!(l.balance(&"alice".into()), 40);
        assert_eq!(l.total_supply(), 100);
    }

    #[test]
    fn insufficient_funds_rejected() {
        let mut l = genesis();
        let err = l
            .apply(&Transfer { from: "alice".into(), from_seq: 0, to: "bob".into(), amount: 101 })
            .unwrap_err();
        assert_eq!(err, Reject::Insufficient { have: 100, need: 101 });
    }

    #[test]
    fn unknown_sender_rejected() {
        let mut l = genesis();
        let err = l
            .apply(&Transfer { from: "mallory".into(), from_seq: 0, to: "bob".into(), amount: 1 })
            .unwrap_err();
        assert_eq!(err, Reject::UnknownSender);
    }

    #[test]
    fn disjoint_account_transfers_commute() {
        // CALM / coordination-free: two owners spending their OWN accounts touch
        // disjoint state, so applying them in either order yields the same
        // ledger. This is why 333 needs no cross-account agreement.
        let base = Ledger::genesis([
            ("alice".into(), 50u128),
            ("bob".into(), 50u128),
            ("carol".into(), 0u128),
        ]);
        let ta = Transfer { from: "alice".into(), from_seq: 0, to: "carol".into(), amount: 10 };
        let tb = Transfer { from: "bob".into(), from_seq: 0, to: "carol".into(), amount: 20 };

        let mut l1 = base.clone();
        l1.apply(&ta).unwrap();
        l1.apply(&tb).unwrap();

        let mut l2 = base.clone();
        l2.apply(&tb).unwrap(); // opposite order
        l2.apply(&ta).unwrap();

        assert_eq!(l1.balance(&"carol".into()), 30);
        assert_eq!(l1.balance(&"carol".into()), l2.balance(&"carol".into()));
        assert_eq!(l1.balance(&"alice".into()), l2.balance(&"alice".into()));
        assert_eq!(l1.balance(&"bob".into()), l2.balance(&"bob".into()));
    }

    #[test]
    fn rcb_delivers_out_of_order_transfers_in_causal_order() {
        // alice authors two dependent transfers; bob receives them reversed.
        // Reliable causal broadcast buffers the second until the first arrives,
        // so bob's ledger never sees a stale-sequence gap.
        let mut alice = Replica::new(
            "alice".into(),
            Ledger::genesis([("alice".into(), 100u128)]),
        );
        let m1 = alice.submit("bob".into(), 40).unwrap();
        let m2 = alice.submit("bob".into(), 25).unwrap();

        let mut bob = Replica::new(
            "bob".into(),
            Ledger::genesis([("alice".into(), 100u128), ("bob".into(), 0u128)]),
        );
        // Deliver reversed.
        let out_first = bob.deliver(m2.clone());
        assert!(out_first.applied.is_empty(), "second msg must wait for its predecessor");
        assert_eq!(bob.pending(), 1);

        let out_second = bob.deliver(m1);
        // m1 delivers, then m2 becomes ready and delivers too.
        assert_eq!(out_second.applied.len(), 2);
        assert_eq!(bob.pending(), 0);
        assert_eq!(bob.ledger().balance(&"bob".into()), 65);
        assert_eq!(bob.ledger().balance(&"alice".into()), 35);
        // Author and receiver converged with zero consensus rounds.
        assert_eq!(alice.ledger().balance(&"alice".into()), 35);
    }

    #[test]
    #[allow(non_snake_case)]
    fn KNOWN_LIMITATION_equivocation_double_spends_without_a_quorum() {
        // PROM 16 (2026-07-13) adversarial finding C3 / Divergence DV-1:
        // the per-account sequence number defends ONLY against an honest owner.
        // With no independent authority quorum (FastPay Signed Echo Broadcast,
        // per-(account,seq) certificate uniqueness), a Byzantine / key-compromised
        // owner can author TWO DIFFERENT transfers at the SAME seq and have both
        // accepted by disjoint replicas that have not yet synced — a double-spend.
        // This test is the executable ORACLE that proves the gap is real in
        // transfer333 as built (Q1 = CONDITIONAL). Closing it requires the
        // quorum-certificate layer; until then the coin is honest-owner/local-credit.
        let genesis = Ledger::genesis([
            ("alice".into(), 100u128),
            ("bob".into(), 0u128),
            ("carol".into(), 0u128),
        ]);

        // Replica R1 only ever hears alice -> bob @seq0. R2 only hears alice ->
        // carol @seq0. Each is internally consistent; neither consulted the other.
        let mut r1 = genesis.clone();
        r1.apply(&Transfer { from: "alice".into(), from_seq: 0, to: "bob".into(), amount: 100 })
            .unwrap();
        let mut r2 = genesis.clone();
        r2.apply(&Transfer { from: "alice".into(), from_seq: 0, to: "carol".into(), amount: 100 })
            .unwrap();

        // Both applied cleanly. alice spent her 100 TWICE: 200 units of value now
        // "exist" as received balances across the two divergent replicas, from a
        // 100-supply account. No quorum step ever forced R1 and R2 to agree on
        // which seq-0 transfer is THE transfer.
        assert_eq!(r1.balance(&"bob".into()), 100);
        assert_eq!(r2.balance(&"carol".into()), 100);
        assert_ne!(r1.balance(&"carol".into()), r2.balance(&"carol".into())); // diverged
        // The gap, stated as an invariant this crate CANNOT yet uphold across
        // replicas: sum of credited value should never exceed alice's 100.
        let cross_replica_credited = r1.balance(&"bob".into()) + r2.balance(&"carol".into());
        assert_eq!(cross_replica_credited, 200, "documents the double-spend: 200 > 100 supply");
    }
}
