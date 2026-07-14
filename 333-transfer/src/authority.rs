// KG: prom16-333-consensusless-frontier (C3, Q1),
//     lesson-transfer333-seqnum-not-byzantine-safe-only-honest-owner-2026-07-13,
//     consensus-prom16-333-no-blockchain-2026-07-12,
//     assess-333-engine-fsm-2026-07-13 (action 9: type-state rail safety),
//     verdict-user-333-coin-local-credit-first-gated-flip-2026-07-14 (real Ed25519 = this file)
//
// FastPay-style authority quorum-certificate layer — the Byzantine double-spend
// defence that the bare sequence-number ledger (see `Ledger::apply`) lacks.
//
// WHY (PROM 16, 2026-07-13, Consensus C3)
// ---------------------------------------
// A per-account sequence number checked over a crash-fault CRDT protects only an
// HONEST owner. A Byzantine / key-compromised owner can author two DIFFERENT
// transfers at the SAME (account, seq) and hand them to disjoint replicas — a
// double-spend (proved by `KNOWN_LIMITATION_equivocation_*` in lib.rs).
//
// The real FastPay defence (Baudet et al. 2020; Cachin-Guerraoui-Rodrigues
// "Signed Echo Broadcast", Alg 3.17) is a Byzantine Consistent Broadcast over an
// INDEPENDENT authority committee. Each authority signs at most ONE transfer per
// (account, seq) slot (lock-on-first-seen) AND only for the account's NEXT
// expected sequence (no seq-skipping). A transfer is final once it collects a
// quorum of such signatures = a certificate.
//
// SAFETY (FastPay Lemma A.1): with quorum(n) = n - f (f = ⌊(n-1)/3⌋) any two
// quorums intersect in n - 2f ≥ f+1 nodes, i.e. in ≥1 HONEST authority. That
// honest authority signed at most one transfer for the slot, so at most one
// transfer can ever assemble a certificate. Double-spend is barred WITHOUT any
// global total order / consensus — single-owner transfer keeps consensus
// number 1 (Guerraoui PODC 2019) while being Byzantine-safe.
//
// TYPE-STATE RAIL (assessment action 9, 2026-07-13): the only way to obtain a
// `Verified` transfer is `Certificate::verify(&committee)`; `Ledger::apply_verified`
// is the ONLY Byzantine-safe apply and it takes a `Verified`. So the unsafe
// double-spend-vulnerable path is not merely discouraged — a caller cannot mint a
// `Verified` without a valid quorum certificate. The verifier always owns the
// `Committee` (never a caller-supplied size), and roster membership is enforced.
//
// AUTHENTICITY (2026-07-14, verdict-user-333-coin-local-credit-first-gated-flip):
// a `Vote` now carries a REAL Ed25519 signature (RFC 8032) over the transfer's
// canonical bytes, produced by the authority's secret `SigningKey`. `Committee`
// binds each `AuthorityId` to its `VerifyingKey`, and `Certificate::is_valid`
// verifies every vote's signature against that key. This closes the last gap the
// prior stand-in `Vote` left open: uniqueness (one transfer per slot per honest
// authority) was already enforced, but a vote was FORGEABLE — anyone knowing a
// committee id could fabricate `Vote { authority, transfer }`. Now a vote is
// unforgeable outside the holder of the authority's secret key, so an attacker
// cannot manufacture the honest-authority votes a certificate needs. What remains
// a follow-up is the gossip/broadcast TRANSPORT (this layer is still in-memory,
// networking-free, unit-testable); the safety + authenticity CORE is complete.

use std::collections::{BTreeMap, HashMap, HashSet};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::{AccountId, Transfer};

/// Identifier of an independent validation authority — DISTINCT from any account
/// owner. Bound to an Ed25519 public key (identity333) by the `Committee`.
pub type AuthorityId = String;

/// Domain-separated, length-prefixed canonical bytes an authority signs to vote
/// for `t`. Unambiguous (every variable-length field is length-prefixed) and
/// domain-separated so a 333 authority vote can never be replayed as any other
/// Ed25519 message. Binding the vote to the exact transfer (incl. `from_seq`) is
/// what ties an authority's at-most-one-per-slot signature to a specific spend.
pub fn signing_message(t: &Transfer) -> Vec<u8> {
    let mut m = Vec::with_capacity(64 + t.from.len() + t.to.len());
    m.extend_from_slice(b"transfer333/authority-vote/v1\0");
    let from = t.from.as_bytes();
    m.extend_from_slice(&(from.len() as u64).to_le_bytes());
    m.extend_from_slice(from);
    m.extend_from_slice(&t.from_seq.to_le_bytes());
    let to = t.to.as_bytes();
    m.extend_from_slice(&(to.len() as u64).to_le_bytes());
    m.extend_from_slice(to);
    m.extend_from_slice(&t.amount.to_le_bytes());
    m
}

/// Byzantine quorum size for `n` authorities tolerating f = ⌊(n-1)/3⌋ faults.
///
/// Returned as `n - f`, NOT the naive `2f+1`. The two agree exactly when
/// n = 3f+1 (n=4→3, n=7→5, n=10→7, matching `333-consensus`), but for other n
/// `2f+1` is UNSAFE: safety needs any two quorums to intersect in an honest
/// authority, i.e. 2q - n > f. With q = n - f that is n - 2f ≥ f+1 (since
/// n ≥ 3f+1), holding for ALL n ≥ 1; with q = 2f+1 it fails whenever n > 3f+1
/// (e.g. n=5,f=1: two size-3 quorums can meet only at the one Byzantine node).
/// n=0 returns 1 so an empty committee can never yield a zero-vote quorum
/// (defence in depth; `Committee` already forbids an empty roster).
pub fn quorum(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let f = (n - 1) / 3;
    n - f
}

/// The trusted set of authorities, each bound to its Ed25519 public key. The
/// VERIFIER owns this; certificates are checked against the committee the verifier
/// knows, never against a size (or key) an untrusted caller supplies.
/// Construction rejects an empty roster.
#[derive(Debug, Clone)]
pub struct Committee {
    members: BTreeMap<AuthorityId, VerifyingKey>,
}

impl Committee {
    /// Build from a roster of `(authority id, Ed25519 public key)` pairs. Returns
    /// `None` for an empty roster.
    pub fn new<I, S>(members: I) -> Option<Committee>
    where
        I: IntoIterator<Item = (S, VerifyingKey)>,
        S: Into<AuthorityId>,
    {
        let members: BTreeMap<AuthorityId, VerifyingKey> =
            members.into_iter().map(|(id, k)| (id.into(), k)).collect();
        if members.is_empty() {
            None
        } else {
            Some(Committee { members })
        }
    }

    pub fn size(&self) -> usize {
        self.members.len()
    }

    /// The (n-f) quorum for this committee.
    pub fn quorum(&self) -> usize {
        quorum(self.size())
    }

    pub fn contains(&self, a: &AuthorityId) -> bool {
        self.members.contains_key(a)
    }

    /// The trusted Ed25519 public key for `a`, or `None` if `a` is not a member.
    pub fn key_of(&self, a: &AuthorityId) -> Option<&VerifyingKey> {
        self.members.get(a)
    }
}

/// An authority's SIGNED vote binding it to one transfer at one slot. The
/// `signature` is a real Ed25519 signature over `signing_message(&transfer)` by
/// the authority's secret key; a vote is therefore unforgeable outside the key
/// holder. Verified in `Certificate::is_valid` against the committee's key for
/// `authority`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vote {
    pub authority: AuthorityId,
    pub transfer: Transfer,
    pub signature: Signature,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityError {
    /// Already signed a DIFFERENT transfer at the same (account, seq): the owner
    /// is equivocating and is refused. This refusal, replicated across a quorum,
    /// is the whole double-spend defence.
    Equivocation { account: AccountId, seq: u64 },
    /// The transfer's sequence is not the account's next expected sequence — an
    /// owner may not skip ahead (certify seq 2 before seq 1) nor replay a past
    /// one. Closes the arbitrary-slot-lock ordering gap (assessment action 9).
    OutOfOrder { account: AccountId, expected: u64, got: u64 },
}

/// The slot an authority locks on: (account, sequence).
fn slot(t: &Transfer) -> (AccountId, u64) {
    (t.from.clone(), t.from_seq)
}

/// An independent authority holding a secret Ed25519 signing key. Signs at most
/// one transfer per (account, seq) slot (lock-on-first-seen) AND only for the
/// account's next expected sequence. The secret key never leaves the authority;
/// only `verifying_key()` (the public half) is published into a `Committee`.
#[derive(Debug, Clone)]
pub struct Authority {
    id: AuthorityId,
    signing: SigningKey,
    locked: HashMap<(AccountId, u64), Transfer>,
    /// Per-account next acceptable sequence. Advanced by `confirm` once a
    /// certificate for the current slot is observed. Owners cannot skip.
    next_expected: HashMap<AccountId, u64>,
}

impl Authority {
    /// Create an authority from its id and its secret Ed25519 signing key.
    pub fn new(id: impl Into<AuthorityId>, signing: SigningKey) -> Self {
        Self {
            id: id.into(),
            signing,
            locked: HashMap::new(),
            next_expected: HashMap::new(),
        }
    }

    pub fn id(&self) -> &AuthorityId {
        &self.id
    }

    /// This authority's PUBLIC key, to be registered in the `Committee`.
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    fn expected(&self, account: &AccountId) -> u64 {
        self.next_expected.get(account).copied().unwrap_or(0)
    }

    /// Sign `t` (Ed25519 over its canonical bytes) as this authority.
    fn sign(&self, t: &Transfer) -> Vote {
        Vote {
            authority: self.id.clone(),
            transfer: t.clone(),
            signature: self.signing.sign(&signing_message(t)),
        }
    }

    /// Handle a transfer order. Signs (returns a `Vote`) iff:
    ///   * the transfer's seq is the account's next expected seq (no skip/replay), and
    ///   * the (account, seq) slot is free, or already holds THIS exact transfer
    ///     (idempotent re-vote).
    /// Refuses with `OutOfOrder` on a skipped/stale seq, or `Equivocation` if a
    /// DIFFERENT transfer already holds the slot. Ed25519 signing is deterministic
    /// (RFC 8032), so an idempotent re-vote yields the identical signature.
    pub fn handle(&mut self, t: &Transfer) -> Result<Vote, AuthorityError> {
        let expected = self.expected(&t.from);
        if t.from_seq != expected {
            return Err(AuthorityError::OutOfOrder {
                account: t.from.clone(),
                expected,
                got: t.from_seq,
            });
        }
        let s = slot(t);
        match self.locked.get(&s) {
            Some(prev) if prev == t => Ok(self.sign(t)),
            Some(_) => Err(AuthorityError::Equivocation { account: s.0, seq: s.1 }),
            None => {
                self.locked.insert(s, t.clone());
                Ok(self.sign(t))
            }
        }
    }

    /// Advance this authority's next-expected sequence for the account once a
    /// certificate is confirmed. Monotonic; idempotent for the same certificate.
    pub fn confirm(&mut self, v: &Verified) {
        let t = &v.0;
        let e = self.next_expected.entry(t.from.clone()).or_insert(0);
        if t.from_seq + 1 > *e {
            *e = t.from_seq + 1;
        }
    }
}

/// A quorum certificate: enough SIGNED votes from DISTINCT committee authorities,
/// all for the SAME transfer, to reach the committee's quorum.
#[derive(Debug, Clone)]
pub struct Certificate {
    pub transfer: Transfer,
    pub votes: Vec<Vote>,
}

/// A transfer proven final by a valid quorum certificate. UNFORGEABLE outside
/// this module: the field is private and the only constructor is
/// `Certificate::verify`. `Ledger::apply_verified` accepts only this, so the
/// unsafe rail is unrepresentable on the Byzantine-safe path (type-state).
#[derive(Debug, Clone)]
pub struct Verified(Transfer);

impl Verified {
    pub fn transfer(&self) -> &Transfer {
        &self.0
    }
}

impl Certificate {
    /// Assemble a certificate for `transfer` from collected `votes`, validated
    /// against the trusted `committee`. `Some` iff `is_valid`.
    pub fn assemble(transfer: Transfer, votes: Vec<Vote>, committee: &Committee) -> Option<Certificate> {
        let cert = Certificate { transfer, votes };
        if cert.is_valid(committee) {
            Some(cert)
        } else {
            None
        }
    }

    /// Valid against `committee` iff: at least one vote; every vote is for this
    /// transfer, from a committee member (roster check rejects unknown / fabricated
    /// authorities), AND carries a valid Ed25519 signature under that member's
    /// public key (authenticity: a vote cannot be forged without the authority's
    /// secret key); and DISTINCT signers reach `committee.quorum()`.
    pub fn is_valid(&self, committee: &Committee) -> bool {
        if self.votes.is_empty() {
            return false;
        }
        let message = signing_message(&self.transfer);
        let mut distinct: HashSet<&AuthorityId> = HashSet::new();
        for v in &self.votes {
            if v.transfer != self.transfer {
                return false;
            }
            let key = match committee.key_of(&v.authority) {
                Some(k) => k,
                None => return false, // not on the roster
            };
            if key.verify(&message, &v.signature).is_err() {
                return false; // forged / wrong-key signature
            }
            distinct.insert(&v.authority);
        }
        distinct.len() >= committee.quorum()
    }

    /// The ONLY way to mint a `Verified`. Returns `Some(Verified)` iff this
    /// certificate is valid against `committee`.
    pub fn verify(&self, committee: &Committee) -> Option<Verified> {
        if self.is_valid(committee) {
            Some(Verified(self.transfer.clone()))
        } else {
            None
        }
    }
}

/// Outcome of one certification round against a committee of authorities: either
/// a `Verified` transfer, or the reason it could not certify. Makes a
/// contested / stuck slot OBSERVABLE (assessment action 9) instead of a silent
/// failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Certified {
    /// Reached quorum and verified.
    Ok,
    /// Some authorities refused (equivocation / out-of-order); quorum unreachable.
    /// If `contested` is true, opposing locks make a certificate impossible for
    /// this slot — a permanent liveness fault needing account recovery, not a
    /// transient sub-quorum.
    Failed { votes: usize, refusals: usize, contested: bool },
}

/// Run one synchronous certification round: present `t` to every authority,
/// collect votes, and try to assemble+verify a certificate against `committee`.
/// Returns `(maybe_verified, Certified)`. `contested` is set when refusals came
/// from Equivocation (an opposing transfer holds the slot) so no certificate can
/// ever assemble for this slot — the FastPay account-recovery trigger.
pub fn certify(
    t: &Transfer,
    authorities: &mut [Authority],
    committee: &Committee,
) -> (Option<Verified>, Certified) {
    let mut votes = Vec::new();
    let mut refusals = 0usize;
    let mut equivocation_seen = false;
    for a in authorities.iter_mut() {
        match a.handle(t) {
            Ok(v) => votes.push(v),
            Err(AuthorityError::Equivocation { .. }) => {
                refusals += 1;
                equivocation_seen = true;
            }
            Err(AuthorityError::OutOfOrder { .. }) => refusals += 1,
        }
    }
    match Certificate::assemble(t.clone(), votes.clone(), committee) {
        Some(cert) => {
            let verified = cert.verify(committee).expect("assembled cert verifies");
            // Advance every authority's next-expected sequence on confirmation.
            for a in authorities.iter_mut() {
                a.confirm(&verified);
            }
            (Some(verified), Certified::Ok)
        }
        None => (
            None,
            Certified::Failed { votes: votes.len(), refusals, contested: equivocation_seen },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(from: &str, seq: u64, to: &str, amount: u128) -> Transfer {
        Transfer { from: from.into(), from_seq: seq, to: to.into(), amount }
    }

    /// Deterministic per-index secret key for tests (seed = [i; 32]). Distinct
    /// per authority; the seed is secret-in-test, NOT derived from the public id.
    fn key(i: u8) -> SigningKey {
        SigningKey::from_bytes(&[i; 32])
    }

    /// n authorities `a0..a{n-1}` with seeded keys, plus a committee binding each
    /// id to its public key.
    fn setup(n: u8) -> (Committee, Vec<Authority>) {
        let auth: Vec<Authority> = (0..n).map(|i| Authority::new(format!("a{i}"), key(i))).collect();
        let committee = Committee::new(auth.iter().map(|a| (a.id().clone(), a.verifying_key()))).unwrap();
        (committee, auth)
    }

    #[test]
    fn quorum_matches_333_consensus_at_3f_plus_1_and_stays_safe_elsewhere() {
        assert_eq!(quorum(4), 3);
        assert_eq!(quorum(7), 5);
        assert_eq!(quorum(10), 7);
        assert_eq!(quorum(1), 1);
        assert_eq!(quorum(0), 1);
        assert_eq!(quorum(5), 4); // f=1; unsafe 2f+1=3
        assert_eq!(quorum(6), 5);
        for n in 1..=30 {
            let f = (n - 1) / 3;
            assert!(2 * quorum(n) as isize - n as isize > f as isize, "n={n}");
        }
    }

    #[test]
    fn empty_committee_is_unconstructible() {
        let empty: Vec<(String, VerifyingKey)> = Vec::new();
        assert!(Committee::new(empty).is_none());
    }

    #[test]
    fn honest_transfer_assembles_and_verifies() {
        let t = tx("alice", 0, "bob", 30);
        let (c, mut auth) = setup(4);
        let votes: Vec<Vote> = auth.iter_mut().take(3).map(|a| a.handle(&t).unwrap()).collect();
        let cert = Certificate::assemble(t.clone(), votes, &c).expect("3/4 = quorum, all signed");
        assert!(cert.verify(&c).is_some());
    }

    #[test]
    fn equivocation_barred_by_quorum_certificate() {
        // A Byzantine owner tries to double-spend seq-0: T1->bob AND T2->carol.
        let t1 = tx("alice", 0, "bob", 100);
        let t2 = tx("alice", 0, "carol", 100);
        let (c, mut a) = setup(4);
        let v_t1: Vec<Vote> = vec![
            a[0].handle(&t1).unwrap(),
            a[1].handle(&t1).unwrap(),
            a[2].handle(&t1).unwrap(),
        ];
        assert_eq!(a[1].handle(&t2), Err(AuthorityError::Equivocation { account: "alice".into(), seq: 0 }));
        assert_eq!(a[2].handle(&t2), Err(AuthorityError::Equivocation { account: "alice".into(), seq: 0 }));
        let v_t2: Vec<Vote> = vec![a[3].handle(&t2).unwrap()];
        assert!(Certificate::assemble(t1, v_t1, &c).is_some());
        assert!(Certificate::assemble(t2, v_t2, &c).is_none(), "conflicting transfer can NEVER certify");
    }

    #[test]
    fn byzantine_authority_double_signing_still_cannot_double_certify() {
        // n=5, quorum=4. Byzantine a4 double-signs BOTH transfers (it holds its
        // own key, so both signatures are valid); honest a0..a3 split 2/2.
        let t1 = tx("alice", 0, "bob", 100);
        let t2 = tx("alice", 0, "carol", 100);
        let (c, mut a) = setup(5);
        let a4_key = key(4); // a4's real secret key (it is a committee member)
        let a4_vote = |t: &Transfer| Vote {
            authority: "a4".into(),
            transfer: t.clone(),
            signature: a4_key.sign(&signing_message(t)),
        };
        let mut v_t1 = vec![a[0].handle(&t1).unwrap(), a[1].handle(&t1).unwrap()];
        let mut v_t2 = vec![a[2].handle(&t2).unwrap(), a[3].handle(&t2).unwrap()];
        v_t1.push(a4_vote(&t1));
        v_t2.push(a4_vote(&t2));
        // Each side has only 3 distinct signers < quorum 4.
        assert!(Certificate::assemble(t1, v_t1, &c).is_none());
        assert!(Certificate::assemble(t2, v_t2, &c).is_none());
    }

    #[test]
    fn certificate_requires_distinct_authorities_not_repeated_votes() {
        let t = tx("alice", 0, "bob", 10);
        let (c, mut a) = setup(4);
        let v = a[0].handle(&t).unwrap();
        let padded = vec![v.clone(), v.clone(), v.clone()];
        assert!(Certificate::assemble(t, padded, &c).is_none());
    }

    #[test]
    fn votes_from_non_committee_authorities_are_rejected() {
        // Outsiders sign with their own (valid) keys but are not on the roster.
        let t = tx("alice", 0, "bob", 10);
        let (c, _) = setup(4);
        let outsiders: Vec<Vote> = (0..3u8)
            .map(|i| {
                let rogue = key(100 + i);
                Vote { authority: format!("x{i}"), transfer: t.clone(), signature: rogue.sign(&signing_message(&t)) }
            })
            .collect();
        assert!(Certificate::assemble(t, outsiders, &c).is_none());
    }

    #[test]
    fn forged_vote_with_wrong_key_is_rejected() {
        // AUTHENTICITY: an attacker forges votes claiming REAL committee ids
        // a0/a1/a2 (roster + distinct-quorum would otherwise pass) but signs them
        // with a rogue key it controls. Per-authority Ed25519 verification fails,
        // so no certificate assembles — the gap the stand-in `Vote` left open.
        let t = tx("alice", 0, "bob", 10);
        let (c, _) = setup(4);
        let rogue = key(200);
        let forged: Vec<Vote> = ["a0", "a1", "a2"]
            .iter()
            .map(|id| Vote { authority: (*id).into(), transfer: t.clone(), signature: rogue.sign(&signing_message(&t)) })
            .collect();
        assert!(
            Certificate::assemble(t, forged, &c).is_none(),
            "forged (wrong-key) signatures must fail per-authority Ed25519 verification"
        );
    }

    #[test]
    fn vote_signature_does_not_transfer_to_a_different_transfer() {
        // A valid signature for T1 cannot be replayed onto T2: the message binds
        // the full transfer (recipient/amount/seq), so swapping the transfer while
        // keeping the signature fails verification.
        let t1 = tx("alice", 0, "bob", 10);
        let t2 = tx("alice", 0, "carol", 10);
        let (c, mut a) = setup(4);
        let good = a[0].handle(&t1).unwrap();
        let spliced = Vote { authority: good.authority.clone(), transfer: t2.clone(), signature: good.signature };
        // is_valid checks v.transfer == cert.transfer first, but even a cert built
        // FOR t2 with this spliced vote fails the signature check.
        assert!(Certificate::assemble(t2, vec![spliced], &c).is_none());
    }

    #[test]
    fn authority_refuses_out_of_order_sequence() {
        // seq must be the account's next expected (0 first). Skipping is refused.
        let mut a0 = Authority::new("a0", key(0));
        let skip = tx("alice", 2, "bob", 10);
        assert_eq!(
            a0.handle(&skip),
            Err(AuthorityError::OutOfOrder { account: "alice".into(), expected: 0, got: 2 })
        );
        // seq 0 is accepted.
        assert!(a0.handle(&tx("alice", 0, "bob", 10)).is_ok());
    }

    #[test]
    fn certify_round_and_confirm_advance_sequence() {
        let (c, mut auth) = setup(4);
        let (v0, r0) = certify(&tx("alice", 0, "bob", 10), &mut auth, &c);
        assert_eq!(r0, Certified::Ok);
        assert!(v0.is_some());
        // seq 2 now (skipping seq 1) -> all authorities OutOfOrder -> Failed, not contested.
        let (v_skip, r_skip) = certify(&tx("alice", 2, "bob", 10), &mut auth, &c);
        assert!(v_skip.is_none());
        assert!(matches!(r_skip, Certified::Failed { contested: false, .. }));
        // seq 1 certifies.
        let (v1, r1) = certify(&tx("alice", 1, "bob", 10), &mut auth, &c);
        assert_eq!(r1, Certified::Ok);
        assert!(v1.is_some());
    }

    #[test]
    fn certify_flags_contested_slot_on_equivocation() {
        let (c, mut auth) = setup(4);
        let (v1, _) = certify(&tx("alice", 0, "bob", 100), &mut auth, &c);
        assert!(v1.is_some()); // T1 certified (all 4 lock it, then confirm advances)
        // After confirm, seq 0 is now past -> T2 at seq 0 is OutOfOrder, not contested.
        let (v2, r2) = certify(&tx("alice", 0, "carol", 100), &mut auth, &c);
        assert!(v2.is_none());
        assert!(matches!(r2, Certified::Failed { .. }));
    }

    #[test]
    fn contested_when_locks_split_before_any_certificate() {
        let (c, mut auth) = setup(4);
        let t1 = tx("alice", 0, "bob", 100);
        let t2 = tx("alice", 0, "carol", 100);
        auth[0].handle(&t1).unwrap();
        auth[1].handle(&t1).unwrap();
        auth[2].handle(&t2).unwrap();
        auth[3].handle(&t2).unwrap();
        // Certifying T1 over the full committee: a0,a1 re-vote T1 (idempotent),
        // a2,a3 refuse (Equivocation) -> 2 votes < quorum 3 -> Failed+contested.
        let (v, r) = certify(&t1, &mut auth, &c);
        assert!(v.is_none());
        assert_eq!(r, Certified::Failed { votes: 2, refusals: 2, contested: true });
    }
}
