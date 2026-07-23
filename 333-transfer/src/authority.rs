// KG: prom16-333-consensusless-frontier (C3, Q1),
//     lesson-transfer333-seqnum-not-byzantine-safe-only-honest-owner-2026-07-13,
//     assess-333-engine-fsm-2026-07-13 (type-state rail safety)
//
// FastPay-style authority quorum-certificate layer. The owner signs an order;
// an independent authority committee then signs at most one state-valid order
// per (account, sequence) slot. A verifier accepts only a quorum certificate
// tied to its exact committee, owner policy, network, and transfer.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::journal::{Journal, JournalError, JournalRecord, NullJournal, SnapshotData};
use crate::owner::{
    put_network_transfer_body, NetworkId, OwnerAuthError, PolicyId, SignedTransfer,
    TransferPolicy,
};
use crate::{AccountId, Ledger, Reject, Transfer};

const AUTHORITY_VOTE_DOMAIN: &[u8] = b"transfer333/authority-vote/v5\0";
const COMMITTEE_ID_DOMAIN: &[u8] = b"transfer333/committee-id/v1\0";

/// Wire and admission bound for one committee. This also bounds certificate
/// validation work and decoded certificate allocation.
pub const MAX_COMMITTEE_MEMBERS: usize = 1024;

/// Authority ids are human-readable deployment aliases, not unbounded input.
pub const MAX_AUTHORITY_ID_BYTES: usize = 256;

/// Identifier of an independent validation authority, distinct from an account
/// owner. A [`Committee`] binds this alias to an Ed25519 public key.
pub type AuthorityId = String;

/// Canonical digest of `(PolicyId, sorted AuthorityId -> authority key roster)`.
///
/// The roster binding is security-critical: a quorum of votes collected for a
/// four-member committee must not be reusable as a certificate for a different
/// three-member committee that happens to contain the same signers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommitteeId([u8; 32]);

impl CommitteeId {
    /// Construct an untrusted decoded id. Trust comes only from comparing this
    /// value with [`Committee::id`], which is computed from verifier-owned data.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for CommitteeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Domain-separated bytes signed by one authority vote.
///
/// Every trust-domain discriminator is covered: authority alias, committee
/// roster, owner policy, deployment/network, the complete transfer, and the
/// owner-declared re-vote round. No vote can be transplanted between aliases,
/// domains, or rounds while retaining a valid signature.
pub fn authority_signing_message(
    authority: &AuthorityId,
    committee_id: &CommitteeId,
    policy_id: &PolicyId,
    network_id: &NetworkId,
    transfer: &Transfer,
    round: u64,
) -> Vec<u8> {
    let mut message = Vec::with_capacity(
        144 + network_id.as_str().len() + transfer.from.len() + transfer.to.len(),
    );
    message.extend_from_slice(AUTHORITY_VOTE_DOMAIN);
    message.extend_from_slice(committee_id.as_bytes());
    message.extend_from_slice(&(authority.len() as u64).to_le_bytes());
    message.extend_from_slice(authority.as_bytes());
    message.extend_from_slice(policy_id.as_bytes());
    put_network_transfer_body(&mut message, network_id, transfer, round);
    message
}

/// Compatibility name for callers that used `signing_message` directly.
pub fn signing_message(
    authority: &AuthorityId,
    committee_id: &CommitteeId,
    policy_id: &PolicyId,
    network_id: &NetworkId,
    transfer: &Transfer,
    round: u64,
) -> Vec<u8> {
    authority_signing_message(authority, committee_id, policy_id, network_id, transfer, round)
}

/// Byzantine quorum size for `n` authorities tolerating f = floor((n-1)/3).
///
/// `n-f` is used rather than a naive `2f+1`: for committee sizes not exactly
/// `3f+1`, `n-f` preserves an honest intersection between any two quorums.
pub fn quorum(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let f = (n - 1) / 3;
    n - f
}

fn committee_id(
    policy_id: PolicyId,
    members: &BTreeMap<AuthorityId, VerifyingKey>,
) -> CommitteeId {
    let mut digest = Sha256::new();
    digest.update(COMMITTEE_ID_DOMAIN);
    digest.update(policy_id.as_bytes());
    digest.update((members.len() as u64).to_le_bytes());
    for (authority, key) in members {
        let id = authority.as_bytes();
        digest.update((id.len() as u64).to_le_bytes());
        digest.update(id);
        digest.update(key.as_bytes());
    }
    CommitteeId(digest.finalize().into())
}

/// Verifier-owned authority roster plus the immutable owner/network policy.
/// Construction fails closed for an empty, oversized, or malformed roster, as
/// well as duplicate aliases or duplicate public keys. One physical signing key
/// therefore cannot inflate quorum power by appearing under multiple aliases.
/// Iteration order never affects [`CommitteeId`] because members are canonicalized
/// in a `BTreeMap` before hashing.
#[derive(Debug, Clone)]
pub struct Committee {
    members: BTreeMap<AuthorityId, VerifyingKey>,
    policy: TransferPolicy,
    id: CommitteeId,
}

impl Committee {
    pub fn new<I, S>(members: I, policy: TransferPolicy) -> Option<Self>
    where
        I: IntoIterator<Item = (S, VerifyingKey)>,
        S: Into<AuthorityId>,
    {
        let mut roster = BTreeMap::new();
        let mut public_keys = HashSet::new();
        for (authority, key) in members {
            let authority = authority.into();
            if authority.is_empty() || authority.len() > MAX_AUTHORITY_ID_BYTES {
                return None;
            }
            if roster.len() == MAX_COMMITTEE_MEMBERS {
                return None;
            }
            if !public_keys.insert(key.to_bytes()) {
                return None;
            }
            if roster.insert(authority, key).is_some() {
                return None;
            }
        }
        if roster.is_empty() {
            return None;
        }
        let id = committee_id(policy.id(), &roster);
        Some(Self {
            members: roster,
            policy,
            id,
        })
    }

    pub fn id(&self) -> CommitteeId {
        self.id
    }

    pub fn size(&self) -> usize {
        self.members.len()
    }

    pub fn quorum(&self) -> usize {
        quorum(self.size())
    }

    pub fn contains(&self, authority: &AuthorityId) -> bool {
        self.members.contains_key(authority)
    }

    pub fn key_of(&self, authority: &AuthorityId) -> Option<&VerifyingKey> {
        self.members.get(authority)
    }

    pub fn policy(&self) -> &TransferPolicy {
        &self.policy
    }
}

/// One authority's Ed25519 vote for one exact order in one exact trust domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vote {
    pub authority: AuthorityId,
    pub committee_id: CommitteeId,
    pub policy_id: PolicyId,
    pub network_id: NetworkId,
    pub transfer: Transfer,
    /// The order round this vote signs. A vote is valid only for an order at
    /// the same round, so votes from different rounds can never mix into one
    /// certificate.
    pub round: u64,
    pub signature: Signature,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoteError {
    WrongCommittee {
        expected: CommitteeId,
        got: CommitteeId,
    },
    WrongPolicy {
        expected: PolicyId,
        got: PolicyId,
    },
    WrongNetwork {
        expected: NetworkId,
        got: NetworkId,
    },
    WrongTransfer,
    /// The vote signs a different round than the order it is presented for.
    WrongRound { expected: u64, got: u64 },
    UnknownAuthority {
        authority: AuthorityId,
    },
    InvalidAuthoritySignature {
        authority: AuthorityId,
    },
}

impl Vote {
    pub fn validate_for(
        &self,
        order: &SignedTransfer,
        committee: &Committee,
    ) -> Result<(), VoteError> {
        if self.committee_id != committee.id() {
            return Err(VoteError::WrongCommittee {
                expected: committee.id(),
                got: self.committee_id,
            });
        }
        if order.policy_id() != committee.policy().id() {
            return Err(VoteError::WrongPolicy {
                expected: committee.policy().id(),
                got: order.policy_id(),
            });
        }
        if self.policy_id != order.policy_id() {
            return Err(VoteError::WrongPolicy {
                expected: order.policy_id(),
                got: self.policy_id,
            });
        }
        if self.network_id != order.network_id {
            return Err(VoteError::WrongNetwork {
                expected: order.network_id.clone(),
                got: self.network_id.clone(),
            });
        }
        if self.transfer != order.transfer {
            return Err(VoteError::WrongTransfer);
        }
        if self.round != order.round {
            return Err(VoteError::WrongRound {
                expected: order.round,
                got: self.round,
            });
        }
        let key = committee
            .key_of(&self.authority)
            .ok_or_else(|| VoteError::UnknownAuthority {
                authority: self.authority.clone(),
            })?;
        key.verify_strict(
            &authority_signing_message(
                &self.authority,
                &self.committee_id,
                &self.policy_id,
                &self.network_id,
                &self.transfer,
                self.round,
            ),
            &self.signature,
        )
        .map_err(|_| VoteError::InvalidAuthoritySignature {
            authority: self.authority.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityError {
    /// Owner proof is checked before reading or mutating pending state.
    OwnerAuth(OwnerAuthError),
    /// A different order already owns this pending account/sequence slot.
    Equivocation { account: AccountId, seq: u64 },
    /// Owner policy knows the sender but this authority's canonical ledger does
    /// not. This is a deployment/genesis mismatch, not an owner-auth failure.
    UnknownSender { account: AccountId },
    /// Zero-value orders consume a sequence without moving value and are not
    /// admitted to the transferable rail.
    ZeroAmount,
    /// The order must match the canonical ledger's exact next sequence.
    OutOfOrder {
        account: AccountId,
        expected: u64,
        got: u64,
    },
    /// State validity is checked before a slot lock is installed.
    InsufficientBalance {
        account: AccountId,
        have: u128,
        need: u128,
    },
    /// Incrementing this sender's sequence would wrap and make an old order
    /// replayable. Rejected before the account/sequence slot is locked.
    SequenceExhausted { account: AccountId },
    /// Crediting the recipient would overflow its balance. Rejected before any
    /// vote or pending-state mutation.
    BalanceOverflow {
        account: AccountId,
        have: u128,
        credit: u128,
    },
    /// A durability barrier failed; the authority has fail-stopped.
    JournalFailed { reason: String },
    /// A prior durability failure fail-stopped this authority. It will not vote
    /// again: its in-memory locks are no longer backed by stable storage, so it
    /// cannot prove it has not already signed a conflicting order for a slot.
    Poisoned,
}

/// Vote-slot identity: one lock per (account, sequence, round). A higher round
/// is a fresh slot, which is what lets a cooperating owner recover a contested
/// lower-round slot by re-signing at a bumped round.
fn slot(order: &SignedTransfer) -> (AccountId, u64, u64) {
    (
        order.transfer.from.clone(),
        order.transfer.from_seq,
        order.round,
    )
}

/// What a journal is claimed by: this authority, this committee, this genesis.
///
/// A journal records decisions, not balances, so recovery is only meaningful
/// against the three inputs it was written under. Binding all three closes two
/// distinct config mistakes that recovery cannot otherwise notice:
///
/// * Wrong file — an authority pointed at a peer's journal adopts that peer's
///   locks *and loses its own*, so it re-votes a slot it already voted on. An
///   honest operator reproduces Byzantine equivocation with a path typo.
/// * Wrong genesis — replay re-drives `Ledger::apply` from whatever the caller
///   passed, so a mismatched genesis silently mints balances.
///
/// `balances()` is a `BTreeMap`, so the fold is deterministic. Lengths are
/// prefixed to keep the digest unambiguous: without them `("ab", "c")` and
/// `("a", "bc")` would hash alike.
/// # KG: fix-333-journal-identity-binding-2026-07-15
fn journal_identity(id: &AuthorityId, committee: &CommitteeId, ledger: &Ledger) -> [u8; 32] {
    let mut d = Sha256::new();
    d.update(b"transfer333/journal-identity/v1\0");
    d.update((id.len() as u32).to_be_bytes());
    d.update(id.as_bytes());
    d.update(committee.as_bytes());
    for (account, balance) in ledger.balances() {
        d.update((account.len() as u32).to_be_bytes());
        d.update(account.as_bytes());
        d.update(balance.to_be_bytes());
        d.update(ledger.next_seq(&account).to_be_bytes());
    }
    d.finalize().into()
}

/// Independent authority state.
///
/// The ledger is the single source of truth for balance and next sequence. A
/// separate speculative sequence counter is deliberately absent: confirmation
/// advances only through an atomic [`Ledger::apply`].
///
/// Slot decisions are backed by a [`Journal`]. The default is [`NullJournal`],
/// which keeps the historical non-durable behaviour; [`Authority::with_journal`]
/// and [`Authority::recover`] opt into durability. See `journal.rs` for why an
/// in-memory lock table turns an honest restart into Byzantine equivocation.
#[derive(Debug)]
pub struct Authority {
    id: AuthorityId,
    signing: SigningKey,
    policy: TransferPolicy,
    committee_id: CommitteeId,
    ledger: Ledger,
    locked: HashMap<(AccountId, u64, u64), SignedTransfer>,
    confirmed: HashMap<(AccountId, u64), [u8; 32]>,
    journal: Box<dyn Journal>,
    /// Set once a journal append fails. In-memory state is then no longer backed
    /// by stable storage, so the authority refuses to vote or confirm rather than
    /// silently serving from state it cannot recover. Fail-stop, not fail-open.
    poisoned: bool,
}

impl Authority {
    /// Construct from the deployment's canonical genesis ledger and committee
    /// id. Callers build the public [`Committee`] first, then instantiate each
    /// authority with `committee.id()` and an identical canonical genesis.
    ///
    /// Uses a [`NullJournal`]: locks are **not** durable and a restart forgets
    /// them. Use [`Authority::with_journal`] for any deployment that can restart.
    pub fn new(
        id: impl Into<AuthorityId>,
        signing: SigningKey,
        policy: TransferPolicy,
        committee_id: CommitteeId,
        ledger: Ledger,
    ) -> Self {
        Self::with_journal(id, signing, policy, committee_id, ledger, NullJournal::new())
    }

    /// Construct with an explicit durability port. The journal is assumed empty;
    /// to rebuild from an existing log use [`Authority::recover`].
    pub fn with_journal(
        id: impl Into<AuthorityId>,
        signing: SigningKey,
        policy: TransferPolicy,
        committee_id: CommitteeId,
        ledger: Ledger,
        journal: impl Journal + 'static,
    ) -> Self {
        let id = id.into();
        let mut journal: Box<dyn Journal> = Box::new(journal);
        // Claim the journal, or refuse to use it. This constructor cannot return
        // an error without churning ~19 call sites, so a rejected claim produces a
        // *poisoned* authority instead: constructed, but refusing to vote or
        // confirm. Fail-stop either way — what must never happen is writing our
        // decisions into a log that is not ours. `recover` is the production path
        // and does surface the error; see its `bind` call.
        // # KG: fix-333-journal-identity-binding-2026-07-15
        let poisoned = journal
            .bind(&journal_identity(&id, &committee_id, &ledger))
            .is_err();
        Self {
            id,
            signing,
            policy,
            committee_id,
            ledger,
            locked: HashMap::new(),
            confirmed: HashMap::new(),
            journal,
            poisoned,
        }
    }

    /// Rebuild authority state by folding the journal over the canonical genesis
    /// ledger. `ledger` must be the same genesis the authority originally started
    /// from — the log carries decisions, not balances.
    ///
    /// Replay is a pure fold in append order: `Locked` installs a lock,
    /// `Confirmed` re-drives `Ledger::apply` and clears the lock. `apply` is
    /// deterministic, so a record that applied once applies again.
    pub fn recover(
        id: impl Into<AuthorityId>,
        signing: SigningKey,
        policy: TransferPolicy,
        committee_id: CommitteeId,
        genesis: Ledger,
        journal: impl Journal + 'static,
    ) -> Result<Self, JournalError> {
        let id = id.into();
        let mut journal = journal;
        // Before replay, not after: replay is what adopts the log's locks and
        // re-drives the ledger, so a journal written by a different authority, a
        // re-rostered committee, or against a different genesis must be rejected
        // while it can still be rejected. `with_journal` below binds the same
        // identity again, which is idempotent.
        // # KG: fix-333-journal-identity-binding-2026-07-15
        journal.bind(&journal_identity(&id, &committee_id, &genesis))?;
        let records = journal.replay()?;
        let mut me = Self::with_journal(id, signing, policy, committee_id, genesis, journal);
        for record in &records {
            match record {
                // A snapshot *replaces* state: it already accounts for every
                // record the compaction discarded. Applying it as a delta would
                // double-count. Compaction writes it at the head of a fresh log,
                // so any Locked/Confirmed that follows is genuinely later.
                JournalRecord::Snapshot(sn) => {
                    me.ledger = Ledger::restore(
                        sn.accounts
                            .iter()
                            .map(|(id, bal, seq)| (id.clone(), *bal, *seq)),
                    )
                    .map_err(|e| {
                        JournalError::Corrupt(format!("snapshot ledger invalid: {e:?}"))
                    })?;
                    me.locked = sn
                        .locked
                        .iter()
                        .map(|o| (slot(o), o.clone()))
                        .collect();
                    me.confirmed = sn
                        .confirmed
                        .iter()
                        .map(|(account, seq, order_id)| ((account.clone(), *seq), *order_id))
                        .collect();
                }
                JournalRecord::Locked(order) => {
                    me.locked.insert(slot(order), order.clone());
                }
                JournalRecord::Confirmed(order) => {
                    let account = order.transfer.from.clone();
                    let seq = order.transfer.from_seq;
                    me.ledger.apply(&order.transfer).map_err(|e| {
                        JournalError::Corrupt(format!(
                            "replay: confirmed record for ({account}, {seq}) does not apply: {e:?}"
                        ))
                    })?;
                    me.confirmed.insert((account.clone(), seq), order.order_id());
                    // Confirmation spends the sequence for every round, so all
                    // round locks for the (account, seq) go.
                    me.locked
                        .retain(|(a, s, _), _| !(*a == account && *s == seq));
                }
            }
        }
        Ok(me)
    }

    /// Capture complete state for a journal snapshot.
    ///
    /// `confirmed` stores only an order id per slot, but a snapshot must be able
    /// to rebuild that map, so the full orders are kept in `locked`/`confirmed`
    /// and the ids are re-derived on recovery. That keeps the snapshot
    /// self-describing: it cannot disagree with itself.
    pub fn snapshot_data(&self) -> SnapshotData {
        let accounts = self
            .ledger
            .balances()
            .into_iter()
            .map(|(id, balance)| {
                let next_seq = self.ledger.next_seq(&id);
                (id, balance, next_seq)
            })
            .collect();
        SnapshotData {
            accounts,
            locked: self.locked.values().cloned().collect(),
            confirmed: self
                .confirmed
                .iter()
                .map(|((account, seq), order_id)| (account.clone(), *seq, *order_id))
                .collect(),
        }
    }

    /// Snapshot current state and discard the log it subsumes.
    ///
    /// Safe to call at any quiescent point; the snapshot is taken from live
    /// state, so it is consistent by construction. A failure fail-stops for the
    /// same reason `append` does: the log on disk may no longer describe us.
    pub fn compact_journal(&mut self) -> Result<(), AuthorityError> {
        let snapshot = self.snapshot_data();
        if let Err(e) = self.journal.compact(&snapshot) {
            self.poisoned = true;
            return Err(AuthorityError::JournalFailed {
                reason: e.to_string(),
            });
        }
        Ok(())
    }

    /// True once a durability failure has fail-stopped this authority.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Append and fail-stop on error. Returns the raw [`JournalError`]; callers
    /// wrap it in their own error type.
    fn journal_append(&mut self, record: &JournalRecord) -> Result<(), JournalError> {
        if let Err(e) = self.journal.append(record) {
            self.poisoned = true;
            return Err(e);
        }
        Ok(())
    }

    pub fn id(&self) -> &AuthorityId {
        &self.id
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    fn sign(&self, order: &SignedTransfer) -> Vote {
        let committee_id = self.committee_id;
        let policy_id = order.policy_id();
        let network_id = order.network_id.clone();
        let transfer = order.transfer.clone();
        let round = order.round;
        let signature = self.signing.sign(&authority_signing_message(
            &self.id,
            &committee_id,
            &policy_id,
            &network_id,
            &transfer,
            round,
        ));
        Vote {
            authority: self.id.clone(),
            committee_id,
            policy_id,
            network_id,
            transfer,
            round,
            signature,
        }
    }

    /// Validate and vote using a deterministic guard order:
    ///
    /// 1. not poisoned; 2. owner proof; 3. pending/idempotence/equivocation per
    /// (account, seq, round) — a higher round is a fresh slot even when a lower
    /// round is locked on a different order, which is the cooperating owner's
    /// recovery path from a split vote;
    /// 4. sender exists; 5. amount is positive; 6. complete ledger preflight,
    /// including sequence and balance arithmetic; 7. **make the lock durable**;
    /// 8. install the lock and sign.
    ///
    /// Every rejection before step 7 is mutation-free. In particular, an
    /// overspend cannot poison the slot and block a later valid order.
    ///
    /// Step 7 is the write-ahead point and it is not an optimisation: the vote is
    /// this authority's irrevocable, externally-visible promise never to sign a
    /// different order for the slot. Producing it before the lock is durable
    /// would let a restart retract the promise — exactly the honest-crash-becomes-
    /// equivocation failure (`audit-333-fsm-vs-borg-k8s-2026-07-15`). Journal
    /// first, sign second.
    pub fn handle(&mut self, order: &SignedTransfer) -> Result<Vote, AuthorityError> {
        if self.poisoned {
            return Err(AuthorityError::Poisoned);
        }
        order
            .verify(&self.policy)
            .map_err(AuthorityError::OwnerAuth)?;

        let transfer = &order.transfer;
        let pending_slot = slot(order);
        match self.locked.get(&pending_slot) {
            Some(previous) if previous == order => return Ok(self.sign(order)),
            Some(_) => {
                return Err(AuthorityError::Equivocation {
                    account: pending_slot.0,
                    seq: pending_slot.1,
                });
            }
            None => {}
        }

        if !self.ledger.contains_account(&transfer.from) {
            return Err(AuthorityError::UnknownSender {
                account: transfer.from.clone(),
            });
        }
        if transfer.amount == 0 {
            return Err(AuthorityError::ZeroAmount);
        }
        if let Err(reject) = self.ledger.preflight(transfer) {
            return Err(match reject {
                Reject::UnknownSender => AuthorityError::UnknownSender {
                    account: transfer.from.clone(),
                },
                Reject::StaleSequence { expected, got } => AuthorityError::OutOfOrder {
                    account: transfer.from.clone(),
                    expected,
                    got,
                },
                Reject::Insufficient { have, need } => AuthorityError::InsufficientBalance {
                    account: transfer.from.clone(),
                    have,
                    need,
                },
                Reject::SequenceExhausted { account } => {
                    AuthorityError::SequenceExhausted { account }
                }
                Reject::BalanceOverflow {
                    account,
                    have,
                    credit,
                } => AuthorityError::BalanceOverflow {
                    account,
                    have,
                    credit,
                },
                Reject::NoCertificate => {
                    unreachable!("ledger preflight never checks certificate admission")
                }
            });
        }

        // Write-ahead: durable before the vote exists. On `Err` no lock is
        // installed and no vote is produced, so the slot stays open.
        self.journal_append(&JournalRecord::Locked(order.clone()))
            .map_err(|e| AuthorityError::JournalFailed {
                reason: e.to_string(),
            })?;
        self.locked.insert(pending_slot, order.clone());
        Ok(self.sign(order))
    }

    /// Apply a verified certificate to this authority's canonical ledger.
    ///
    /// The supplied committee is not ambient authority: its computed id must
    /// equal both the authority's configured trust root and the id carried by
    /// `Verified`. Policy and owner proof are rechecked locally. `Ledger::apply`
    /// performs all state checks before its commit, and lock/confirmation state
    /// changes only after a successful apply.
    pub fn confirm(
        &mut self,
        verified: &Verified,
        committee: &Committee,
    ) -> Result<ConfirmOutcome, ConfirmError> {
        // Fail-stop applies here exactly as in `handle`. Without this guard a
        // *transient* durability failure (a full disk the operator then clears)
        // punches a permanent hole in the log: the failed append leaves
        // `Confirmed(seq N)` unwritten while the in-memory ledger has already
        // advanced, and the next certificate journals `Confirmed(seq N+1)` on top
        // of the gap. Recovery then folds N+1 onto a ledger still at N, rejects
        // it, and the authority never starts again — a recoverable disk blip
        // turned into a bricked node.
        if self.poisoned {
            return Err(ConfirmError::Poisoned);
        }
        if verified.committee_id != self.committee_id {
            return Err(ConfirmError::WrongCommittee {
                expected: self.committee_id,
                got: verified.committee_id,
            });
        }
        if committee.id() != self.committee_id {
            return Err(ConfirmError::WrongCommittee {
                expected: self.committee_id,
                got: committee.id(),
            });
        }
        if committee.policy().id() != self.policy.id() {
            return Err(ConfirmError::WrongPolicy {
                expected: self.policy.id(),
                got: committee.policy().id(),
            });
        }
        if verified.order.policy_id() != self.policy.id() {
            return Err(ConfirmError::WrongPolicy {
                expected: self.policy.id(),
                got: verified.order.policy_id(),
            });
        }
        verified
            .order
            .verify(&self.policy)
            .map_err(ConfirmError::OwnerAuth)?;

        // A matching CommitteeId already commits to this exact roster and key,
        // but checking the local member binding catches constructor/configuration
        // mistakes without relying only on the digest comparison.
        if committee.key_of(&self.id) != Some(&self.verifying_key()) {
            return Err(ConfirmError::WrongCommittee {
                expected: self.committee_id,
                got: committee.id(),
            });
        }

        let transfer = verified.transfer();
        // Confirmation is scoped to the (account, seq), not to a round: a
        // certificate formed at any round spends the sequence, and confirmation
        // checks seq/balance — never this authority's current pending lock — so
        // a straggler lower-round certificate still confirms after authorities
        // have voted at higher rounds.
        let confirmed_slot = (transfer.from.clone(), transfer.from_seq);
        let order_id = verified.order.order_id();
        if let Some(previous) = self.confirmed.get(&confirmed_slot) {
            if previous == &order_id {
                return Ok(ConfirmOutcome::AlreadyApplied);
            }
            return Err(ConfirmError::State(Reject::StaleSequence {
                expected: self.ledger.next_seq(&transfer.from),
                got: transfer.from_seq,
            }));
        }

        self.ledger.apply(transfer).map_err(ConfirmError::State)?;
        // Durability here is weaker than in `handle`, deliberately. A vote is an
        // unrecoverable promise, so it must be journalled first. A confirmation is
        // driven by a quorum certificate — public, self-authenticating evidence
        // that anyone may re-present — so a lost confirmation is recoverable by
        // replaying the certificate, not a safety hole. `apply` must therefore run
        // first (it is the only step that can reject), and the record is written
        // once the outcome is known. A failure here still fail-stops: in-memory
        // state has moved ahead of stable storage.
        self.journal_append(&JournalRecord::Confirmed(verified.order.clone()))
            .map_err(|e| ConfirmError::Journal(e.to_string()))?;
        self.confirmed.insert(confirmed_slot.clone(), order_id);
        // The seq is spent for every round, so all round locks for it go.
        self.locked
            .retain(|(account, seq, _), _| !(account == &transfer.from && *seq == transfer.from_seq));
        Ok(ConfirmOutcome::Applied)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmOutcome {
    Applied,
    AlreadyApplied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmError {
    WrongCommittee {
        expected: CommitteeId,
        got: CommitteeId,
    },
    WrongPolicy {
        expected: PolicyId,
        got: PolicyId,
    },
    OwnerAuth(OwnerAuthError),
    State(Reject),
    /// A durability barrier failed after the ledger moved; fail-stop.
    Journal(String),
    /// A prior durability failure fail-stopped this authority.
    Poisoned,
}

/// Quorum certificate for one owner-signed order.
#[derive(Debug, Clone)]
pub struct Certificate {
    pub order: SignedTransfer,
    pub votes: Vec<Vote>,
}

/// Type-state proof minted only by [`Certificate::verify`]. The committee id is
/// retained so later consumers cannot accidentally apply proof from a foreign
/// trust root.
#[derive(Debug, Clone)]
pub struct Verified {
    order: SignedTransfer,
    committee_id: CommitteeId,
}

impl Verified {
    pub fn transfer(&self) -> &Transfer {
        &self.order.transfer
    }

    pub fn order(&self) -> &SignedTransfer {
        &self.order
    }

    pub fn committee_id(&self) -> CommitteeId {
        self.committee_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertificateError {
    OwnerAuth(OwnerAuthError),
    EmptyVotes,
    InvalidVote {
        authority: AuthorityId,
        source: VoteError,
    },
    DuplicateAuthority {
        authority: AuthorityId,
    },
    InsufficientQuorum {
        have: usize,
        need: usize,
    },
}

impl Certificate {
    pub fn assemble(
        order: SignedTransfer,
        votes: Vec<Vote>,
        committee: &Committee,
    ) -> Option<Self> {
        let certificate = Self { order, votes };
        certificate
            .is_valid(committee)
            .then_some(certificate)
    }

    pub fn validate(&self, committee: &Committee) -> Result<(), CertificateError> {
        self.order
            .verify(committee.policy())
            .map_err(CertificateError::OwnerAuth)?;
        if self.votes.is_empty() {
            return Err(CertificateError::EmptyVotes);
        }

        let mut distinct = HashSet::new();
        for vote in &self.votes {
            vote.validate_for(&self.order, committee)
                .map_err(|source| CertificateError::InvalidVote {
                    authority: vote.authority.clone(),
                    source,
                })?;
            if !distinct.insert(vote.authority.clone()) {
                return Err(CertificateError::DuplicateAuthority {
                    authority: vote.authority.clone(),
                });
            }
        }
        if distinct.len() < committee.quorum() {
            return Err(CertificateError::InsufficientQuorum {
                have: distinct.len(),
                need: committee.quorum(),
            });
        }
        Ok(())
    }

    pub fn is_valid(&self, committee: &Committee) -> bool {
        self.validate(committee).is_ok()
    }

    pub fn verify(&self, committee: &Committee) -> Option<Verified> {
        self.is_valid(committee).then(|| Verified {
            order: self.order.clone(),
            committee_id: committee.id(),
        })
    }
}

/// Observable result of a synchronous certification helper round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Certified {
    Ok,
    Failed {
        votes: usize,
        refusals: usize,
        contested: bool,
    },
}

/// Present one order to every authority, assemble a certificate, then confirm
/// it on each authority. Production transports use the same individual methods;
/// this helper keeps deterministic unit and in-memory tests concise.
pub fn certify(
    order: &SignedTransfer,
    authorities: &mut [Authority],
    committee: &Committee,
) -> (Option<Verified>, Certified) {
    let mut votes = Vec::new();
    let mut refusals = 0;
    let mut contested = false;

    for authority in authorities.iter_mut() {
        match authority.handle(order) {
            Ok(vote) => votes.push(vote),
            Err(AuthorityError::Equivocation { .. }) => {
                refusals += 1;
                contested = true;
            }
            // A durability fault is an operational refusal, not contention: the
            // authority produced no vote, and unlike Equivocation it says nothing
            // about a competing order. Counting it as `contested` would mislabel a
            // disk failure as a double-spend attempt.
            Err(
                AuthorityError::OwnerAuth(_)
                | AuthorityError::UnknownSender { .. }
                | AuthorityError::ZeroAmount
                | AuthorityError::OutOfOrder { .. }
                | AuthorityError::InsufficientBalance { .. }
                | AuthorityError::SequenceExhausted { .. }
                | AuthorityError::BalanceOverflow { .. }
                | AuthorityError::JournalFailed { .. }
                | AuthorityError::Poisoned,
            ) => refusals += 1,
        }
    }

    match Certificate::assemble(order.clone(), votes.clone(), committee) {
        Some(certificate) => {
            let verified = certificate
                .verify(committee)
                .expect("an assembled certificate must verify");
            for authority in authorities.iter_mut() {
                let _ = authority.confirm(&verified, committee);
            }
            (Some(verified), Certified::Ok)
        }
        None => (
            None,
            Certified::Failed {
                votes: votes.len(),
                refusals,
                contested,
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owner::{NetworkId, OwnerRegistry};

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn owner_key(account: &str) -> SigningKey {
        match account {
            "alice" => key(200),
            "bob" => key(201),
            "carol" => key(202),
            _ => key(250),
        }
    }

    fn policy() -> TransferPolicy {
        TransferPolicy::new(
            NetworkId::new("authority-testnet").unwrap(),
            OwnerRegistry::new([
                ("alice", owner_key("alice").verifying_key()),
                ("bob", owner_key("bob").verifying_key()),
                ("carol", owner_key("carol").verifying_key()),
            ])
            .unwrap(),
        )
    }

    fn genesis() -> Ledger {
        Ledger::genesis([
            ("alice".into(), 100),
            ("bob".into(), 0),
            ("carol".into(), 0),
        ])
    }

    fn transfer(from: &str, seq: u64, to: &str, amount: u128) -> Transfer {
        Transfer {
            from: from.into(),
            from_seq: seq,
            to: to.into(),
            amount,
        }
    }

    fn order(
        policy: &TransferPolicy,
        from: &str,
        seq: u64,
        to: &str,
        amount: u128,
    ) -> SignedTransfer {
        SignedTransfer::sign(
            policy,
            transfer(from, seq, to, amount),
            &owner_key(from),
        )
    }

    fn order_at_round(
        policy: &TransferPolicy,
        from: &str,
        seq: u64,
        to: &str,
        amount: u128,
        round: u64,
    ) -> SignedTransfer {
        SignedTransfer::sign_at_round(
            policy,
            transfer(from, seq, to, amount),
            round,
            &owner_key(from),
        )
    }

    fn setup(n: u8) -> (Committee, Vec<Authority>, TransferPolicy) {
        let policy = policy();
        let committee = Committee::new(
            (0..n).map(|i| (format!("a{i}"), key(i).verifying_key())),
            policy.clone(),
        )
        .unwrap();
        let committee_id = committee.id();
        let authorities = (0..n)
            .map(|i| {
                Authority::new(
                    format!("a{i}"),
                    key(i),
                    policy.clone(),
                    committee_id,
                    genesis(),
                )
            })
            .collect();
        (committee, authorities, policy)
    }

    #[test]
    fn quorum_has_honest_intersection_for_all_small_committee_sizes() {
        assert_eq!(quorum(0), 1);
        assert_eq!(quorum(4), 3);
        assert_eq!(quorum(5), 4);
        assert_eq!(quorum(7), 5);
        for n in 1..=64 {
            let f = (n - 1) / 3;
            assert!(2 * quorum(n) as isize - n as isize > f as isize);
        }
    }

    // KNOWN LIMITATION — equivocation split-vote permanently locks the slot for
    // an UNCOOPERATIVE owner who never re-votes. This remains true by design
    // (Guerraoui ceiling: consensusless asset transfer cannot force liveness for
    // an owner that refuses to act). It is self-inflicted and LIVENESS-ONLY: the
    // lock strands only the equivocator's own (account, seq) slot; third-party
    // no-double-spend safety is preserved (no certificate is ever produced).
    // Since the owner re-vote rounds change, a COOPERATING owner now has a
    // recovery path — re-signing at a bumped round — pinned by
    // `owner_revote_at_bumped_round_recovers_split_locked_slot`. This oracle
    // pins what remains: with no re-vote, the slot stays terminal. Consensus
    // (Cuttlefish/Stingray FastUnlock routed through the 333-platform HotStuff
    // engine) would be needed for forced recovery WITHOUT owner cooperation,
    // NEVER a CRDT/LWW tie-break (CALM).
    // LakatoTree: LakatosTree_333_Cryptocurrency_20260721 node `baseline-gap`
    //   (pred recovery_paths_for_uncertified_contested_lock=0 for the
    //    UNCOOPERATIVE owner; cooperating-owner round-bump recovery landed).
    #[test]
    fn KNOWN_LIMITATION_two_two_split_vote_permanently_locks_slot_with_no_recovery() {
        // n = 4 committee, so quorum = n - f = 4 - 1 = 3.
        let (committee, mut authorities, policy) = setup(4);
        assert_eq!(committee.quorum(), 3);

        // Two DISTINCT owner-signed orders equivocate the same (alice, seq 0) slot.
        let order_a = order(&policy, "alice", 0, "bob", 10);
        let order_b = order(&policy, "alice", 0, "carol", 10);
        assert_ne!(order_a, order_b);

        // A 2-2 split: authorities {0,1} lock order_a, authorities {2,3} lock order_b.
        // Each lock is first-seen for that authority, so every handle() succeeds.
        assert!(authorities[0].handle(&order_a).is_ok());
        assert!(authorities[1].handle(&order_a).is_ok());
        assert!(authorities[2].handle(&order_b).is_ok());
        assert!(authorities[3].handle(&order_b).is_ok());

        // Neither order can now reach quorum: a locked authority re-votes only for
        // ITS order and returns Equivocation for the other. Max attainable votes
        // for either candidate is 2 < 3.
        let (verified_a, result_a) = certify(&order_a, &mut authorities, &committee);
        assert!(verified_a.is_none(), "order_a must not certify");
        assert_eq!(
            result_a,
            Certified::Failed { votes: 2, refusals: 2, contested: true },
        );
        let (verified_b, result_b) = certify(&order_b, &mut authorities, &committee);
        assert!(verified_b.is_none(), "order_b must not certify");
        assert_eq!(
            result_b,
            Certified::Failed { votes: 2, refusals: 2, contested: true },
        );

        // TERMINAL for this owner: retrying the same round-0 orders never
        // recovers. Only an owner re-vote at a bumped round opens a fresh slot,
        // and this owner never re-votes — so the slot stays contested forever.
        for _ in 0..5 {
            let (va, ra) = certify(&order_a, &mut authorities, &committee);
            let (vb, rb) = certify(&order_b, &mut authorities, &committee);
            assert!(va.is_none() && vb.is_none());
            assert!(matches!(ra, Certified::Failed { contested: true, .. }));
            assert!(matches!(rb, Certified::Failed { contested: true, .. }));
        }

        // SAFETY IS PRESERVED throughout: because no certificate ever assembled,
        // neither recipient was ever paid — the equivocation costs the equivocator
        // liveness on its own slot, never a third-party double-spend.
        assert_eq!(authorities[0].ledger().balance(&"bob".into()), 0);
        assert_eq!(authorities[0].ledger().balance(&"carol".into()), 0);
        assert_eq!(authorities[0].ledger().balance(&"alice".into()), 100);
    }

    // THE HEADLINE RECEIPT for owner re-vote at a bumped round (Linera-rounds
    // pattern, consensusless stage-1 recovery): the same 2-2 split that the
    // KNOWN_LIMITATION oracle proves terminal for an UNCOOPERATIVE owner is
    // recovered when the owner cooperates — re-signing ONE of the two equivocated
    // orders at round 1 opens a fresh per-(account, seq, round) vote slot on every
    // authority, reaches quorum, and confirms. Recovery metric 0.0 -> 1.0 for the
    // cooperating-owner case; the uncooperative case stays locked by design
    // (Guerraoui ceiling), pinned by the oracle above.
    #[test]
    fn owner_revote_at_bumped_round_recovers_split_locked_slot() {
        // n = 4 committee, so quorum = n - f = 3.
        let (committee, mut authorities, policy) = setup(4);
        assert_eq!(committee.quorum(), 3);

        // The exact KNOWN_LIMITATION scenario: a 2-2 equivocation split at round 0.
        let order_a = order(&policy, "alice", 0, "bob", 10);
        let order_b = order(&policy, "alice", 0, "carol", 10);
        authorities[0].handle(&order_a).unwrap();
        authorities[1].handle(&order_a).unwrap();
        authorities[2].handle(&order_b).unwrap();
        authorities[3].handle(&order_b).unwrap();
        let (stuck, result) = certify(&order_a, &mut authorities, &committee);
        assert!(stuck.is_none());
        assert_eq!(
            result,
            Certified::Failed { votes: 2, refusals: 2, contested: true },
        );

        // The owner cooperates: re-signs ONE of the two orders at round 1. Every
        // authority treats (alice, 0, 1) as a fresh vote slot even though it is
        // locked on a different order at (alice, 0, 0), so the re-vote reaches
        // quorum, certifies, and confirms with correct balances.
        let retry = order_at_round(&policy, "alice", 0, "bob", 10, 1);
        assert_ne!(retry, order_a, "the round is part of the order's identity");
        let (verified, result) = certify(&retry, &mut authorities, &committee);
        assert_eq!(result, Certified::Ok);
        assert!(
            verified.is_some(),
            "round-1 re-vote must certify despite the round-0 split lock"
        );
        for authority in &authorities {
            assert_eq!(authority.ledger().balance(&"bob".into()), 10);
            assert_eq!(authority.ledger().balance(&"carol".into()), 0);
            assert_eq!(authority.ledger().balance(&"alice".into()), 90);
            assert_eq!(authority.ledger().next_seq(&"alice".into()), 1);
        }
    }

    #[test]
    fn authority_votes_once_per_seq_round() {
        let (_committee, mut authorities, policy) = setup(4);
        let order_a = order(&policy, "alice", 0, "bob", 10);
        let order_b = order(&policy, "alice", 0, "carol", 10);
        let authority = &mut authorities[0];

        // First-seen at round 0 installs the (alice, 0, 0) lock; the exact same
        // order re-votes idempotently, and a different order in the SAME round is
        // equivocation — lock-on-first-seen within a round, exactly as before.
        let first = authority.handle(&order_a).unwrap();
        assert_eq!(authority.handle(&order_a).unwrap(), first);
        assert!(matches!(
            authority.handle(&order_b),
            Err(AuthorityError::Equivocation { .. })
        ));

        // A HIGHER round is a fresh vote slot: the authority MUST accept it even
        // though it is locked on a different order at round 0 for the same seq.
        let order_b_r1 = order_at_round(&policy, "alice", 0, "carol", 10, 1);
        assert!(authority.handle(&order_b_r1).is_ok());
        // And that fresh slot is then itself locked: another round-1 order for a
        // different transfer is equivocation again.
        let order_a_r1 = order_at_round(&policy, "alice", 0, "bob", 10, 1);
        assert!(matches!(
            authority.handle(&order_a_r1),
            Err(AuthorityError::Equivocation { .. })
        ));
    }

    // Straggler-cert case: a certificate formed at a LOWER round must remain
    // confirmable after the authorities have moved to higher rounds. Confirmation
    // is certificate-driven and checks seq/balance, never the pending lock.
    #[test]
    fn lower_round_certificate_confirms_after_round_bump() {
        let (committee, mut authorities, policy) = setup(4);
        let order_a = order(&policy, "alice", 0, "bob", 10); // round 0
        let votes = vec![
            authorities[0].handle(&order_a).unwrap(),
            authorities[1].handle(&order_a).unwrap(),
            authorities[2].handle(&order_a).unwrap(),
        ];
        let verified = Certificate::assemble(order_a, votes, &committee)
            .unwrap()
            .verify(&committee)
            .unwrap();

        // Before the round-0 certificate is presented anywhere, every authority
        // moves on and votes a DIFFERENT order at round 1.
        let order_b_r1 = order_at_round(&policy, "alice", 0, "carol", 10, 1);
        let round1_votes: Vec<Vote> = authorities
            .iter_mut()
            .map(|authority| authority.handle(&order_b_r1).unwrap())
            .collect();

        // The straggler round-0 certificate still confirms everywhere.
        for authority in &mut authorities {
            assert_eq!(
                authority.confirm(&verified, &committee),
                Ok(ConfirmOutcome::Applied)
            );
            assert_eq!(authority.ledger().balance(&"bob".into()), 10);
            assert_eq!(authority.ledger().balance(&"carol".into()), 0);
            assert_eq!(authority.ledger().next_seq(&"alice".into()), 1);
        }

        // The round-1 votes DO assemble a certificate (quorum-scoped per round),
        // but it can never be confirmed now: the seq is spent. This is why two
        // valid certificates at different rounds for one seq are not a
        // double-spend — the ledger admits exactly one.
        let round1_cert = Certificate::assemble(order_b_r1, round1_votes, &committee)
            .expect("round-1 votes form their own quorum certificate");
        let round1_verified = round1_cert.verify(&committee).unwrap();
        assert_eq!(
            authorities[0].confirm(&round1_verified, &committee),
            Err(ConfirmError::State(Reject::StaleSequence {
                expected: 1,
                got: 0,
            }))
        );
    }

    #[test]
    fn no_cross_round_certificate_assembly() {
        let (committee, mut authorities, policy) = setup(4);
        let order_r0 = order(&policy, "alice", 0, "bob", 10);
        let order_r1 = order_at_round(&policy, "alice", 0, "bob", 10, 1);

        // Same transfer payload at two rounds. Three distinct authorities vote,
        // but one vote is over the round-1 bytes.
        let v0a = authorities[0].handle(&order_r0).unwrap();
        let v0b = authorities[1].handle(&order_r0).unwrap();
        let v1c = authorities[2].handle(&order_r1).unwrap();

        // A vote is bound to the exact (transfer, round) it signed: the round-1
        // vote is not a vote for the round-0 order, so the mixed set cannot form
        // a certificate even with 3 distinct authorities at quorum size.
        assert!(matches!(
            v1c.validate_for(&order_r0, &committee),
            Err(VoteError::WrongRound { expected: 0, got: 1 })
        ));
        assert!(Certificate::assemble(order_r0.clone(), vec![v0a, v0b, v1c], &committee).is_none());

        // Each round's votes assemble only with their own order.
        let v1a = authorities[0].handle(&order_r1).unwrap();
        let v1b = authorities[1].handle(&order_r1).unwrap();
        let v1c = authorities[2].handle(&order_r1).unwrap();
        assert!(Certificate::assemble(order_r1, vec![v1a, v1b, v1c], &committee).is_some());
    }

    #[test]
    fn confirmed_seq_rejects_later_round_orders() {
        let (committee, mut authorities, policy) = setup(4);
        let order = order(&policy, "alice", 0, "bob", 10);
        let (verified, result) = certify(&order, &mut authorities, &committee);
        assert_eq!(result, Certified::Ok);
        assert!(verified.is_some());

        // Once the certificate is confirmed, next_seq has advanced: NO order for
        // the old seq is accepted at ANY round, however high.
        let late = order_at_round(&policy, "alice", 0, "carol", 10, 2);
        for authority in &mut authorities {
            assert_eq!(
                authority.handle(&late),
                Err(AuthorityError::OutOfOrder {
                    account: "alice".into(),
                    expected: 1,
                    got: 0,
                })
            );
        }
    }

    // OQ1 (total-order necessity). The contested-slot outcome is DELIVERY-ORDER
    // dependent, so any recovery decided from a local no-quorum snapshot races a
    // straggler vote and is unsafe without a total order over {vote, seal}. This
    // is the code-grounded REFUTATION of the "owner-signed bump = CN=1, no
    // consensus needed" hypothesis (a seal-latch design was refuted by adversarial
    // verification in the prom16-333-cryptocurrency OQ1 cycle, 2026-07-21): a
    // broadcast-only seal cannot preserve certificate uniqueness, because the SAME
    // 3-authority snapshot can still resolve to a finalized transfer OR to a
    // terminal lock, decided only by what a straggler observes next. Preserving
    // cert-uniqueness therefore needs the consensus (Cuttlefish/Stingray Theorem 2
    // shared-log) leg, not just Theorem 1 quorum arithmetic.
    // LakatoTree: LakatosTree_333_Cryptocurrency_20260721 node oq1-total-order-required.
    #[test]
    fn contested_slot_outcome_is_delivery_order_dependent_so_seal_needs_total_order() {
        // Snapshot common to both universes: a0,a1 lock order_a, a2 locks order_b,
        // a3 has not voted. Votes so far a=2, b=1 — neither at quorum=3. Any
        // recovery that "seals from this snapshot" must be safe against a3's next
        // move.
        let a_to = "bob";
        let b_to = "carol";

        // Universe 1 — a3 next observes order_a: the slot FINALIZES order_a.
        let (committee1, mut u1, policy1) = setup(4);
        let a1o = order(&policy1, "alice", 0, a_to, 10);
        let b1o = order(&policy1, "alice", 0, b_to, 10);
        u1[0].handle(&a1o).unwrap();
        u1[1].handle(&a1o).unwrap();
        u1[2].handle(&b1o).unwrap();
        // certify presents order_a to all four: a0,a1 re-vote, a2 equivocates, a3
        // (open) votes order_a -> 3 distinct votes = quorum -> a real certificate.
        let (verified1, result1) = certify(&a1o, &mut u1, &committee1);
        assert!(verified1.is_some(), "with a3 voting order_a the slot finalizes");
        assert_eq!(result1, Certified::Ok);
        assert_eq!(u1[0].ledger().balance(&a_to.into()), 10);

        // Universe 2 — a3 next observes order_b (locks it): the slot is TERMINAL.
        let (committee2, mut u2, policy2) = setup(4);
        let a2o = order(&policy2, "alice", 0, a_to, 10);
        let b2o = order(&policy2, "alice", 0, b_to, 10);
        u2[0].handle(&a2o).unwrap();
        u2[1].handle(&a2o).unwrap();
        u2[2].handle(&b2o).unwrap();
        u2[3].handle(&b2o).unwrap();
        let (verified2, _r2) = certify(&a2o, &mut u2, &committee2);
        assert!(verified2.is_none(), "with a3 locking order_b the slot is terminal");
        assert_eq!(u2[0].ledger().balance(&a_to.into()), 0);

        // Same snapshot (a:2, b:1) -> finalized-transfer XOR terminal-lock, decided
        // only by a3's future delivery order. A snapshot-computed seal thus races
        // a3; on transfer333's reliable-broadcast rail (independent FIFO mailboxes,
        // no sequencer) two honest authorities can observe {seal, late cert} in
        // opposite orders => two valid certificates at one slot = a cert-uniqueness
        // break. Recovery needs a TOTAL ORDER (OQ1); owner authorization alone
        // (CN=1) is insufficient.
    }

    #[test]
    fn committee_id_is_sorted_policy_and_roster_bound() {
        let policy = policy();
        let first = Committee::new(
            [
                ("a1", key(1).verifying_key()),
                ("a0", key(0).verifying_key()),
            ],
            policy.clone(),
        )
        .unwrap();
        let reversed = Committee::new(
            [
                ("a0", key(0).verifying_key()),
                ("a1", key(1).verifying_key()),
            ],
            policy.clone(),
        )
        .unwrap();
        let changed_roster = Committee::new(
            [
                ("a0", key(0).verifying_key()),
                ("a1", key(9).verifying_key()),
            ],
            policy.clone(),
        )
        .unwrap();
        let changed_policy = TransferPolicy::new(
            NetworkId::new("other-testnet").unwrap(),
            policy.owners().clone(),
        );
        let changed_policy = Committee::new(
            [
                ("a0", key(0).verifying_key()),
                ("a1", key(1).verifying_key()),
            ],
            changed_policy,
        )
        .unwrap();

        assert_eq!(first.id(), reversed.id());
        assert_ne!(first.id(), changed_roster.id());
        assert_ne!(first.id(), changed_policy.id());
    }

    #[test]
    fn malformed_or_oversized_committee_is_unconstructible() {
        let policy = policy();
        assert!(Committee::new(
            Vec::<(String, VerifyingKey)>::new(),
            policy.clone()
        )
        .is_none());
        assert!(Committee::new(
            [
                ("a0", key(0).verifying_key()),
                ("a0", key(1).verifying_key()),
            ],
            policy.clone(),
        )
        .is_none());
        assert!(Committee::new(
            [
                ("a0", key(0).verifying_key()),
                ("a1", key(0).verifying_key()),
            ],
            policy.clone(),
        )
        .is_none());
        assert!(Committee::new(
            [("x".repeat(MAX_AUTHORITY_ID_BYTES + 1), key(0).verifying_key())],
            policy.clone(),
        )
        .is_none());
        let oversized = (0..=MAX_COMMITTEE_MEMBERS).map(|i| {
            let mut seed = [0_u8; 32];
            seed[..8].copy_from_slice(&(i as u64).to_le_bytes());
            (
                format!("a{i}"),
                SigningKey::from_bytes(&seed).verifying_key(),
            )
        });
        assert!(Committee::new(oversized, policy).is_none());
    }

    #[test]
    fn one_signature_cannot_be_cloned_across_authority_aliases() {
        let policy = policy();
        let shared_signing_key = key(7);
        let shared_public_key = shared_signing_key.verifying_key();

        // `Committee::new` now rejects this alias-inflated roster. Construct
        // the historical malformed shape directly to prove the signing
        // preimage independently binds the alias as defence in depth.
        let members: BTreeMap<AuthorityId, VerifyingKey> = ["a0", "a1", "a2", "a3"]
            .into_iter()
            .map(|alias| (alias.to_owned(), shared_public_key.clone()))
            .collect();
        let committee_id = committee_id(policy.id(), &members);
        let committee = Committee {
            members,
            policy: policy.clone(),
            id: committee_id,
        };
        let mut authority = Authority::new(
            "a0",
            shared_signing_key,
            policy.clone(),
            committee_id,
            genesis(),
        );
        let order = order(&policy, "alice", 0, "bob", 10);
        let signed_vote = authority.handle(&order).unwrap();
        let aliased_votes = ["a0", "a1", "a2"]
            .into_iter()
            .map(|alias| {
                let mut vote = signed_vote.clone();
                vote.authority = alias.to_owned();
                vote
            })
            .collect();
        let certificate = Certificate {
            order: order.clone(),
            votes: aliased_votes,
        };

        assert_eq!(committee.quorum(), 3);
        assert!(matches!(
            certificate.validate(&committee),
            Err(CertificateError::InvalidVote {
                authority,
                source: VoteError::InvalidAuthoritySignature { .. },
            }) if authority == "a1"
        ));
    }

    #[test]
    fn honest_certificate_applies_and_replay_is_idempotent() {
        let (committee, mut authorities, policy) = setup(4);
        let order = order(&policy, "alice", 0, "bob", 30);
        let votes = authorities
            .iter_mut()
            .take(3)
            .map(|authority| authority.handle(&order).unwrap())
            .collect();
        let certificate = Certificate::assemble(order, votes, &committee).unwrap();
        let verified = certificate.verify(&committee).unwrap();
        assert_eq!(verified.committee_id(), committee.id());

        for authority in &mut authorities {
            assert_eq!(
                authority.confirm(&verified, &committee),
                Ok(ConfirmOutcome::Applied)
            );
            assert_eq!(authority.ledger().balance(&"alice".into()), 70);
            assert_eq!(authority.ledger().balance(&"bob".into()), 30);
            assert_eq!(
                authority.confirm(&verified, &committee),
                Ok(ConfirmOutcome::AlreadyApplied)
            );
        }
    }

    #[test]
    fn equivocation_is_barred_by_quorum_intersection() {
        let (committee, mut authorities, policy) = setup(4);
        let first = order(&policy, "alice", 0, "bob", 100);
        let conflict = order(&policy, "alice", 0, "carol", 100);
        let first_votes = vec![
            authorities[0].handle(&first).unwrap(),
            authorities[1].handle(&first).unwrap(),
            authorities[2].handle(&first).unwrap(),
        ];
        assert!(matches!(
            authorities[1].handle(&conflict),
            Err(AuthorityError::Equivocation { .. })
        ));
        assert!(matches!(
            authorities[2].handle(&conflict),
            Err(AuthorityError::Equivocation { .. })
        ));
        let conflict_votes = vec![authorities[3].handle(&conflict).unwrap()];

        assert!(Certificate::assemble(first, first_votes, &committee).is_some());
        assert!(Certificate::assemble(conflict, conflict_votes, &committee).is_none());
    }

    #[test]
    fn vote_cannot_be_reassembled_under_a_different_roster() {
        let policy = policy();
        let committee_four = Committee::new(
            (0..4).map(|i| (format!("a{i}"), key(i).verifying_key())),
            policy.clone(),
        )
        .unwrap();
        let committee_three = Committee::new(
            (0..3).map(|i| (format!("a{i}"), key(i).verifying_key())),
            policy.clone(),
        )
        .unwrap();
        let mut authorities: Vec<_> = (0..4)
            .map(|i| {
                Authority::new(
                    format!("a{i}"),
                    key(i),
                    policy.clone(),
                    committee_four.id(),
                    genesis(),
                )
            })
            .collect();
        let order = order(&policy, "alice", 0, "bob", 10);
        let votes = authorities
            .iter_mut()
            .take(3)
            .map(|authority| authority.handle(&order).unwrap())
            .collect();

        assert_ne!(committee_four.id(), committee_three.id());
        assert!(Certificate::assemble(order, votes, &committee_three).is_none());
    }

    #[test]
    fn overspend_rejection_does_not_lock_or_brick_the_account() {
        let (committee, mut authorities, policy) = setup(4);
        let overspend = order(&policy, "alice", 0, "bob", 101);
        for authority in &mut authorities {
            assert_eq!(
                authority.handle(&overspend),
                Err(AuthorityError::InsufficientBalance {
                    account: "alice".into(),
                    have: 100,
                    need: 101,
                })
            );
        }

        let valid = order(&policy, "alice", 0, "bob", 40);
        let (verified, result) = certify(&valid, &mut authorities, &committee);
        assert_eq!(result, Certified::Ok);
        assert!(verified.is_some(), "overspend must not poison the seq-0 slot");
        for authority in &authorities {
            assert_eq!(authority.ledger().balance(&"alice".into()), 60);
            assert_eq!(authority.ledger().balance(&"bob".into()), 40);
            assert_eq!(authority.ledger().next_seq(&"alice".into()), 1);
        }
    }

    #[test]
    fn arithmetic_overflow_orders_get_zero_votes_and_never_lock() {
        let (committee, mut authorities, policy) = setup(4);

        // Simulate corrupt recovered state that canonical genesis construction
        // forbids. Authority admission must still run the ledger's complete
        // arithmetic preflight before producing a vote or installing a lock.
        for authority in &mut authorities {
            authority
                .ledger
                .accounts
                .get_mut("bob")
                .unwrap()
                .balance = u128::MAX;
        }
        let balance_overflow = order(&policy, "alice", 0, "bob", 1);
        assert_eq!(
            authorities[0].handle(&balance_overflow),
            Err(AuthorityError::BalanceOverflow {
                account: "bob".into(),
                have: u128::MAX,
                credit: 1,
            })
        );
        let (verified, result) = certify(&balance_overflow, &mut authorities, &committee);
        assert!(verified.is_none());
        assert_eq!(
            result,
            Certified::Failed {
                votes: 0,
                refusals: 4,
                contested: false,
            }
        );
        assert!(authorities.iter().all(|authority| authority.locked.is_empty()));

        for authority in &mut authorities {
            authority
                .ledger
                .accounts
                .get_mut("bob")
                .unwrap()
                .balance = 0;
            authority
                .ledger
                .accounts
                .get_mut("alice")
                .unwrap()
                .next_seq = u64::MAX;
        }
        let sequence_overflow = order(&policy, "alice", u64::MAX, "bob", 1);
        assert_eq!(
            authorities[0].handle(&sequence_overflow),
            Err(AuthorityError::SequenceExhausted {
                account: "alice".into(),
            })
        );
        let (verified, result) = certify(&sequence_overflow, &mut authorities, &committee);
        assert!(verified.is_none());
        assert_eq!(
            result,
            Certified::Failed {
                votes: 0,
                refusals: 4,
                contested: false,
            }
        );
        assert!(authorities.iter().all(|authority| authority.locked.is_empty()));
    }

    #[test]
    fn zero_and_skipped_sequence_are_mutation_free_rejections() {
        let (committee, mut authorities, policy) = setup(1);
        assert_eq!(
            authorities[0].handle(&order(&policy, "alice", 0, "bob", 0)),
            Err(AuthorityError::ZeroAmount)
        );
        assert_eq!(
            authorities[0].handle(&order(&policy, "alice", 2, "bob", 1)),
            Err(AuthorityError::OutOfOrder {
                account: "alice".into(),
                expected: 0,
                got: 2,
            })
        );
        assert!(authorities[0]
            .handle(&order(&policy, "alice", 0, "bob", 1))
            .is_ok());
        assert_eq!(authorities[0].ledger().next_seq(&"alice".into()), 0);
        assert_eq!(committee.quorum(), 1);
    }

    #[test]
    fn failed_confirm_never_advances_ledger_or_releases_pending_lock() {
        let (committee, mut authorities, policy) = setup(4);
        let first = order(&policy, "alice", 0, "bob", 30);
        let votes = authorities
            .iter_mut()
            .skip(1)
            .take(3)
            .map(|authority| authority.handle(&first).unwrap())
            .collect();
        let verified = Certificate::assemble(first.clone(), votes, &committee)
            .unwrap()
            .verify(&committee)
            .unwrap();

        // The certificate is valid, but this authority's local canonical state
        // cannot fund it. Ledger::apply rejects before commit, and confirm must
        // not maintain a separate sequence counter that advances anyway.
        let mut underfunded = Authority::new(
            "a0",
            key(0),
            policy.clone(),
            committee.id(),
            Ledger::genesis([
                ("alice".into(), 10),
                ("bob".into(), 0),
                ("carol".into(), 0),
            ]),
        );
        let balance_before = underfunded.ledger().balances();
        let seq_before = underfunded.ledger().next_seq(&"alice".into());
        assert_eq!(
            underfunded.confirm(&verified, &committee),
            Err(ConfirmError::State(Reject::Insufficient {
                have: 10,
                need: 30,
            }))
        );
        assert_eq!(underfunded.ledger().balances(), balance_before);
        assert_eq!(underfunded.ledger().next_seq(&"alice".into()), seq_before);

        // Also exercise the lock side of atomicity. Simulate a state-plane drift
        // after this authority locked the certified order: confirm now fails as
        // stale, and the pre-existing lock must remain to reject a conflicting
        // order rather than silently reopening the slot.
        let mut locked = Authority::new(
            "a0",
            key(0),
            policy.clone(),
            committee.id(),
            genesis(),
        );
        locked.handle(&first).unwrap();
        locked
            .ledger
            .apply(&transfer("alice", 0, "carol", 1))
            .unwrap();
        let drifted_balance = locked.ledger().balances();
        assert_eq!(
            locked.confirm(&verified, &committee),
            Err(ConfirmError::State(Reject::StaleSequence {
                expected: 1,
                got: 0,
            }))
        );
        assert_eq!(locked.ledger().balances(), drifted_balance);
        assert_eq!(
            locked.handle(&order(&policy, "alice", 0, "carol", 1)),
            Err(AuthorityError::Equivocation {
                account: "alice".into(),
                seq: 0,
            })
        );
    }

    #[test]
    fn foreign_committee_verified_is_rejected_without_state_change() {
        let (local, mut local_authorities, policy) = setup(4);
        let foreign = Committee::new(
            (0..4).map(|i| (format!("x{i}"), key(100 + i).verifying_key())),
            policy.clone(),
        )
        .unwrap();
        let mut foreign_authorities: Vec<_> = (0..4)
            .map(|i| {
                Authority::new(
                    format!("x{i}"),
                    key(100 + i),
                    policy.clone(),
                    foreign.id(),
                    genesis(),
                )
            })
            .collect();
        let order = order(&policy, "alice", 0, "bob", 10);
        let votes = foreign_authorities
            .iter_mut()
            .take(3)
            .map(|authority| authority.handle(&order).unwrap())
            .collect();
        let foreign_verified = Certificate::assemble(order, votes, &foreign)
            .unwrap()
            .verify(&foreign)
            .unwrap();

        let before = local_authorities[0].ledger().balances();
        assert_eq!(
            local_authorities[0].confirm(&foreign_verified, &local),
            Err(ConfirmError::WrongCommittee {
                expected: local.id(),
                got: foreign.id(),
            })
        );
        assert_eq!(local_authorities[0].ledger().balances(), before);
        assert_eq!(local_authorities[0].ledger().next_seq(&"alice".into()), 0);
    }

    #[test]
    fn duplicate_or_wrong_key_votes_do_not_make_a_certificate() {
        let (committee, mut authorities, policy) = setup(4);
        let order = order(&policy, "alice", 0, "bob", 10);
        let vote = authorities[0].handle(&order).unwrap();
        assert!(Certificate::assemble(
            order.clone(),
            vec![vote.clone(), vote.clone(), vote],
            &committee,
        )
        .is_none());

        let forged = Vote {
            authority: "a0".into(),
            committee_id: committee.id(),
            policy_id: order.policy_id(),
            network_id: order.network_id.clone(),
            transfer: order.transfer.clone(),
            round: order.round,
            signature: key(99).sign(&authority_signing_message(
                &"a0".to_owned(),
                &committee.id(),
                &order.policy_id(),
                &order.network_id,
                &order.transfer,
                order.round,
            )),
        };
        assert!(matches!(
            forged.validate_for(&order, &committee),
            Err(VoteError::InvalidAuthoritySignature { .. })
        ));
    }
}
