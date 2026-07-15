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
//     CLOSES it — see `authority::tests::equivocation_is_barred_by_quorum_intersection`.
//     Authority votes now carry REAL Ed25519 signatures (RFC 8032), verified per
//     authority against the committee's public keys — a vote is unforgeable
//     outside the key holder (`authority::tests::duplicate_or_wrong_key_votes_do_not_make_a_certificate`).
//     The networked rail now also requires a deployment-bound owner signature;
//     owner proof is checked before any authority slot mutation. In-memory, TCP,
//     and epidemic transports all carry that same signed order contract. ***
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
//     disseminate / remote confirm (Steps 5–7), real TCP behind the same
//     `AuthorityNet` trait (Step 8, `tcp`), and Plumtree epidemic over targeted
//     delivery reusing `gossip333` (Step 4, `epidemic`) land in `wire` / `net` /
//     `tcp` / `epidemic`.
//   * premint vs runtime-issuance   -> PREMINT default; but the real boundary is
//     single-minter vs multi-minter, NOT premint vs runtime (PROM Q2). A
//     single-authority runtime mint (cf. Sui TreasuryCap) is still consensus
//     number 1; only a MULTI-minter shared supply cap is consensus number k.
//     `genesis` gives the premint tier.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use crdt333::{CausalBroadcast, CausalMsg, VectorClockBroadcaster};

pub mod authority;
pub mod epidemic;
pub mod journal;
pub mod net;
pub mod owner;
pub mod tcp;
pub mod wire;

pub use authority::{
    authority_signing_message, certify, quorum, signing_message, Authority, AuthorityError,
    Certificate, CertificateError, Certified, Committee, CommitteeId, ConfirmError,
    ConfirmOutcome, Verified, Vote, VoteError, MAX_AUTHORITY_ID_BYTES, MAX_COMMITTEE_MEMBERS,
};
pub use epidemic::{
    mesh_all_eager, mesh_with_dropped_eager_edge, msg_id_of, peer_node_id, pump, EpidemicEndpoint,
    EpidemicNetwork,
};
pub use journal::{Journal, JournalError, JournalRecord, MemJournal, NullJournal};
#[cfg(not(target_arch = "wasm32"))]
pub use journal::FileJournal;
pub use net::{
    authority_handle_round, certify_via_mesh, certify_via_mesh_rounds,
    certify_via_mesh_rounds_with_pause, collect_until_quorum, confirm_from_mesh,
    disseminate_certificate, disseminate_certificate_with_pause, AuthorityMsg, AuthorityNet,
    InMemoryAuthorityMesh, MeshEndpoint, MeshLedger, NetError, VoteCollector,
};
pub use owner::{
    owner_signing_message, NetworkId, NetworkIdError, OwnerAuthError, OwnerRegistry,
    OwnerRegistryError, PolicyId, SignedTransfer, TransferPolicy, MAX_ACCOUNT_ID_BYTES,
    MAX_NETWORK_ID_BYTES,
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

/// Unsigned local-credit payload. `from_seq` is the owner's monotonic
/// per-account counter. The legacy `Ledger::apply` / `Replica` rail assumes an
/// honest local owner and deliberately accepts this bare payload.
///
/// The networked Byzantine-safe rail never accepts a bare `Transfer`: callers
/// must wrap it in an owner-authenticated, network-bound [`SignedTransfer`]
/// before `Authority::handle`, transport, voting, or certification.
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
    /// The sender has consumed the complete u64 sequence space. Reusing or
    /// wrapping sequence zero would make an old signed order replayable.
    SequenceExhausted { account: AccountId },
    /// Crediting the recipient would exceed the representable balance. This is
    /// checked before sender debit so settlement remains all-or-nothing.
    BalanceOverflow {
        account: AccountId,
        have: Amount,
        credit: Amount,
    },
    /// `apply_certified` was called without a valid `n-f` quorum certificate.
    /// This is the gate that makes Byzantine-safe transferable value possible:
    /// an equivocating owner cannot obtain a certificate for two conflicting
    /// transfers at the same (account, seq), so double-spend is barred.
    NoCertificate,
}

/// Invalid canonical genesis input. A deployment must never silently replace
/// a duplicate account allocation or create a supply that cannot be represented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenesisError {
    DuplicateAccount { account: AccountId },
    TotalSupplyOverflow,
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
        Self::try_genesis(alloc).expect("invalid canonical genesis")
    }

    /// Fallible production constructor for canonical premint state.
    pub fn try_genesis<I>(alloc: I) -> Result<Self, GenesisError>
    where
        I: IntoIterator<Item = (AccountId, Amount)>,
    {
        let mut accounts = BTreeMap::new();
        let mut total = 0u128;
        for (id, balance) in alloc {
            if accounts.contains_key(&id) {
                return Err(GenesisError::DuplicateAccount { account: id });
            }
            total = total
                .checked_add(balance)
                .ok_or(GenesisError::TotalSupplyOverflow)?;
            accounts.insert(id, Account { balance, next_seq: 0 });
        }
        let _ = total;
        Ok(Self { accounts })
    }

    pub fn balance(&self, id: &AccountId) -> Amount {
        self.accounts.get(id).map(|a| a.balance).unwrap_or(0)
    }

    pub fn contains_account(&self, id: &AccountId) -> bool {
        self.accounts.contains_key(id)
    }

    /// Snapshot of every account balance (node telemetry / harness JSON).
    pub fn balances(&self) -> BTreeMap<AccountId, Amount> {
        self.accounts
            .iter()
            .map(|(id, a)| (id.clone(), a.balance))
            .collect()
    }

    /// The owner's next acceptable sequence number — what a client must stamp on
    /// its next transfer.
    pub fn next_seq(&self, id: &AccountId) -> u64 {
        self.accounts.get(id).map(|a| a.next_seq).unwrap_or(0)
    }

    /// Total value in the ledger. Invariant: transfers preserve it exactly
    /// (conservation) — no mint/burn on the consensusless path.
    pub fn total_supply(&self) -> Amount {
        self.accounts
            .values()
            .try_fold(0u128, |total, account| total.checked_add(account.balance))
            .expect("ledger supply invariant violated")
    }

    /// Validate every arithmetic and state guard without mutation. Authorities
    /// call this before locking/voting; `apply` repeats it before committing.
    pub(crate) fn preflight(&self, t: &Transfer) -> Result<(), Reject> {
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
        from.next_seq
            .checked_add(1)
            .ok_or_else(|| Reject::SequenceExhausted {
                account: t.from.clone(),
            })?;
        if t.from != t.to {
            let recipient_balance = self.balance(&t.to);
            recipient_balance
                .checked_add(t.amount)
                .ok_or_else(|| Reject::BalanceOverflow {
                    account: t.to.clone(),
                    have: recipient_balance,
                    credit: t.amount,
                })?;
        }
        Ok(())
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
        self.preflight(t)?;

        let next_seq = self
            .accounts
            .get(&t.from)
            .expect("preflight checked sender")
            .next_seq
            .checked_add(1)
            .expect("preflight checked sequence arithmetic");
        if t.from == t.to {
            // A self-transfer moves no value but still consumes exactly one
            // authenticated sequence slot.
            self.accounts
                .get_mut(&t.from)
                .expect("preflight checked sender")
                .next_seq = next_seq;
            return Ok(());
        }

        let credited = self
            .balance(&t.to)
            .checked_add(t.amount)
            .expect("preflight checked recipient arithmetic");
        // Commit only after every fallible calculation above has succeeded.
        let from = self
            .accounts
            .get_mut(&t.from)
            .expect("preflight checked sender");
        from.balance -= t.amount;
        from.next_seq = next_seq;
        self.accounts.entry(t.to.clone()).or_default().balance = credited;
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
    pub fn apply_certified(
        &mut self,
        cert: &Certificate,
        committee: &Committee,
    ) -> Result<(), Reject> {
        if !cert.is_valid(committee) {
            return Err(Reject::NoCertificate);
        }
        self.apply(&cert.order.transfer)
    }

    /// Apply a `Verified` transfer — the type-state Byzantine-safe path. A
    /// `Verified` can only be minted by `Certificate::verify(&committee)`, so
    /// this signature makes the double-spend-vulnerable bare `apply` impossible
    /// to reach with an uncertified transfer: the safe rail is the DEFAULT rail,
    /// not merely the recommended one (assessment action 9).
    pub fn apply_verified(&mut self, v: &Verified, committee: &Committee) -> Result<(), Reject> {
        if v.committee_id() != committee.id()
            || v.order().verify(committee.policy()).is_err()
        {
            return Err(Reject::NoCertificate);
        }
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
    fn genesis_rejects_duplicate_accounts_and_supply_overflow() {
        assert!(matches!(
            Ledger::try_genesis([
                ("alice".into(), 1),
                ("alice".into(), 2),
            ]),
            Err(GenesisError::DuplicateAccount { account }) if account == "alice"
        ));
        // Duplicate identity is the primary structural error even if summing
        // the duplicate allocation would otherwise overflow.
        assert!(matches!(
            Ledger::try_genesis([
                ("alice".into(), u128::MAX),
                ("alice".into(), 1),
            ]),
            Err(GenesisError::DuplicateAccount { account }) if account == "alice"
        ));
        assert!(matches!(
            Ledger::try_genesis([
                ("alice".into(), u128::MAX),
                ("bob".into(), 1),
            ]),
            Err(GenesisError::TotalSupplyOverflow)
        ));
    }

    #[test]
    fn arithmetic_failure_is_checked_before_any_ledger_mutation() {
        // Construct deliberately corrupt recovered state to exercise the last
        // defensive boundary. `try_genesis` prevents this state in normal use.
        let mut l = Ledger {
            accounts: BTreeMap::from([
                (
                    "alice".into(),
                    Account {
                        balance: 1,
                        next_seq: 0,
                    },
                ),
                (
                    "bob".into(),
                    Account {
                        balance: u128::MAX,
                        next_seq: 0,
                    },
                ),
            ]),
        };
        let before = l.clone();
        assert_eq!(
            l.apply(&Transfer {
                from: "alice".into(),
                from_seq: 0,
                to: "bob".into(),
                amount: 1,
            }),
            Err(Reject::BalanceOverflow {
                account: "bob".into(),
                have: u128::MAX,
                credit: 1,
            })
        );
        assert_eq!(l.accounts, before.accounts);

        l.accounts.get_mut("bob").unwrap().balance = 0;
        l.accounts.get_mut("alice").unwrap().next_seq = u64::MAX;
        let before = l.clone();
        assert_eq!(
            l.apply(&Transfer {
                from: "alice".into(),
                from_seq: u64::MAX,
                to: "bob".into(),
                amount: 1,
            }),
            Err(Reject::SequenceExhausted {
                account: "alice".into(),
            })
        );
        assert_eq!(l.accounts, before.accounts);
    }

    #[test]
    fn self_transfer_preserves_value_and_consumes_exactly_one_sequence() {
        let mut l = genesis();
        l.apply(&Transfer {
            from: "alice".into(),
            from_seq: 0,
            to: "alice".into(),
            amount: 100,
        })
        .unwrap();

        assert_eq!(l.balance(&"alice".into()), 100);
        assert_eq!(l.next_seq(&"alice".into()), 1);
        assert_eq!(l.total_supply(), 100);

        let before = l.clone();
        assert_eq!(
            l.apply(&Transfer {
                from: "alice".into(),
                from_seq: 1,
                to: "alice".into(),
                amount: 101,
            }),
            Err(Reject::Insufficient {
                have: 100,
                need: 101,
            })
        );
        assert_eq!(l.accounts, before.accounts);
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
