//! M1 — single-decree, externally-valid Byzantine agreement for one contested slot.
//!
//! **Safety property demonstrated:** external validity + FastPay certificate
//! uniqueness ⇒ the agreed [`SlotSeal`] is a deterministic function of the
//! *evidence*, hence INVARIANT to the delivery order of the underlying votes and
//! to Byzantine vote noise. This collapses the delivery-order-dependent fork that
//! `transfer333`'s fast path exhibits (the RED oracle
//! `contested_slot_outcome_is_delivery_order_dependent_so_seal_needs_total_order`,
//! gj3447/333 @3436691) into one globally-agreed fact.
//!
//! **Scope (M1):** single view, honest-leader safety + external validity +
//! order-invariance, on an in-memory deterministic permutation harness.
//! **Out of scope:** liveness / view-change / pacemaker (M4); the Byzantine-aware
//! `Void` validity precondition (M2 — so a `Void` proposal is *never* externally
//! valid here and the agreement cannot Void); real async networking.
//!
//! KG: `prom16-333-optionA-total-order-leg`. Guard G3 (this is the single-leg
//! order-invariance step, NOT yet cross-leg cert-uniqueness — that is M3).

use crate::{SealOutcome, SlotSeal};
use std::collections::BTreeMap;
use transfer333::{Certificate, Committee};

/// Evidence that makes a proposed [`SlotSeal`] externally valid — an honest node
/// votes for a proposal only if this evidence holds under its own committee.
#[derive(Clone)]
pub enum SealEvidence {
    /// A real quorum-certificate exhibiting that an order finalized at the slot.
    CertFor(Certificate),
    /// (M2) A Byzantine-aware proof that no certificate can form. Not accepted at
    /// M1: a `Void` is never externally valid here, so the agreement cannot Void.
    NoCertProof,
}

/// A leader's proposal into the single-decree agreement: one [`SlotSeal`] plus
/// its external-validity evidence.
#[derive(Clone)]
pub struct SealProposal {
    /// The value proposed for decision.
    pub seal: SlotSeal,
    /// The evidence an honest node checks before voting for `seal`.
    pub evidence: SealEvidence,
}

impl SealProposal {
    /// External validity — the precondition an honest node checks before voting.
    ///
    /// `Finalize{order_id}` is valid iff a real committee-verifying certificate
    /// certifies *exactly* that order at *exactly* this `(account, seq)` slot.
    /// Because a committee admits at most one certificate per slot (FastPay
    /// Lemma A.1), at most one `Finalize` value can ever be externally valid — the
    /// root of the order-invariance guarantee. `Void` is not valid at M1.
    pub fn is_externally_valid(&self, committee: &Committee) -> bool {
        match (&self.seal.outcome, &self.evidence) {
            (SealOutcome::Finalize { order_id }, SealEvidence::CertFor(cert)) => {
                match cert.verify(committee) {
                    Some(v) => {
                        let t = v.transfer();
                        v.order().order_id() == *order_id
                            && t.from == self.seal.account
                            && t.from_seq == self.seal.seq
                    }
                    None => false,
                }
            }
            _ => false,
        }
    }
}

/// A message in the single-decree agreement.
#[derive(Clone)]
pub enum Msg {
    /// The leader's externally-valid proposal.
    Propose(SealProposal),
    /// A vote from `from` for the seal whose canonical digest is `seal_digest`.
    Vote {
        /// Canonical digest of the seal being voted for (see [`seal_digest`]).
        seal_digest: [u8; 32],
        /// The voting authority's id (a string alias, as in `transfer333`).
        from: String,
    },
}

/// Canonical digest of a [`SlotSeal`] outcome for vote-matching. Distinct
/// outcomes (Finalize of different orders, or Void) map to distinct digests, so a
/// vote can only ever count toward the value it names.
pub fn seal_digest(seal: &SlotSeal) -> [u8; 32] {
    // A simple, collision-resistant-enough tag over the decided value. (Not a
    // wire hash; M4 replaces the platform's weak FNV with a real crypto hash on
    // the committed value — tracked as the content-binding milestone.)
    let mut out = [0u8; 32];
    let tag: &[u8] = match &seal.outcome {
        SealOutcome::Finalize { order_id } => order_id,
        SealOutcome::Void => b"VOID____________________________",
    };
    let acct = seal.account.as_bytes();
    for (i, b) in tag.iter().enumerate() {
        out[i] ^= b;
    }
    for (i, b) in acct.iter().enumerate() {
        out[i % 32] ^= b;
    }
    out[0] ^= seal.seq as u8;
    out
}

/// One honest agreement node. It votes for at most one externally-valid proposal
/// (one decree, one view), counts distinct-authority votes for that value, and
/// decides when it observes a quorum. Byzantine votes for any other value are
/// structurally ignored (a node only counts votes matching the value it voted
/// for), so agreement holds under vote noise.
pub struct AgreementNode {
    id: String,
    committee: Committee,
    voted_for: Option<[u8; 32]>,
    votes: BTreeMap<[u8; 32], std::collections::BTreeSet<String>>,
    proposal_seal: Option<SlotSeal>,
    decided: Option<SlotSeal>,
}

impl AgreementNode {
    /// A fresh honest node for `id` under `committee`.
    pub fn new(id: impl Into<String>, committee: Committee) -> Self {
        Self {
            id: id.into(),
            committee,
            voted_for: None,
            votes: BTreeMap::new(),
            proposal_seal: None,
            decided: None,
        }
    }

    /// This node's decision, once reached.
    pub fn decided(&self) -> Option<&SlotSeal> {
        self.decided.as_ref()
    }

    /// Process one delivered message. Returns the node's own vote to broadcast, if
    /// this message caused it to vote (so the harness can fan it out).
    pub fn process(&mut self, msg: &Msg) -> Option<Msg> {
        match msg {
            Msg::Propose(p) => {
                // Vote at most once, and only for an externally-valid proposal.
                if self.voted_for.is_none() && p.is_externally_valid(&self.committee) {
                    let d = seal_digest(&p.seal);
                    self.voted_for = Some(d);
                    self.proposal_seal = Some(p.seal.clone());
                    // count our own vote
                    self.record_vote(d, self.id.clone());
                    return Some(Msg::Vote {
                        seal_digest: d,
                        from: self.id.clone(),
                    });
                }
                None
            }
            Msg::Vote { seal_digest: d, from } => {
                // Only count votes for the exact value we ourselves validated and
                // voted for. A Byzantine vote for anything else is ignored.
                if self.voted_for == Some(*d) {
                    self.record_vote(*d, from.clone());
                }
                None
            }
        }
    }

    fn record_vote(&mut self, d: [u8; 32], from: String) {
        let set = self.votes.entry(d).or_default();
        set.insert(from);
        if self.decided.is_none()
            && self.voted_for == Some(d)
            && set.len() >= self.committee.quorum()
        {
            self.decided = self.proposal_seal.clone();
        }
    }
}

/// Run the single-decree agreement over `honest_ids` under one honest leader
/// proposal, delivering the resulting votes to every honest node in the order
/// given by `delivery` (a permutation of `honest_ids` indices), and injecting any
/// `byzantine_votes` at the end. Returns each honest node's decision.
///
/// This is the deterministic permutation harness the order-invariance oracle
/// drives: the decided *value* must be identical for every `delivery` permutation
/// and unaffected by Byzantine votes.
pub fn run_single_decree(
    committee: &Committee,
    proposal: &SealProposal,
    honest_ids: &[String],
    delivery: &[usize],
    byzantine_votes: &[Msg],
) -> Vec<Option<SlotSeal>> {
    let mut nodes: Vec<AgreementNode> = honest_ids
        .iter()
        .map(|id| AgreementNode::new(id.clone(), committee.clone()))
        .collect();

    // 1. every honest node receives the leader proposal and votes.
    let mut broadcast: Vec<Msg> = Vec::new();
    for n in nodes.iter_mut() {
        if let Some(v) = n.process(&Msg::Propose(proposal.clone())) {
            broadcast.push(v);
        }
    }

    // 2. deliver the honest votes to every node in the permuted order, plus any
    //    Byzantine vote noise. Order must not change the decided value.
    for &i in delivery {
        if i < broadcast.len() {
            let v = broadcast[i].clone();
            for n in nodes.iter_mut() {
                n.process(&v);
            }
        }
    }
    for bv in byzantine_votes {
        for n in nodes.iter_mut() {
            n.process(bv);
        }
    }

    nodes.iter().map(|n| n.decided().cloned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use transfer333::{
        Authority, Committee, NetworkId, OwnerRegistry, SignedTransfer, SigningKey, Transfer,
        TransferPolicy, VerifyingKey,
    };

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }
    fn owner_key(a: &str) -> SigningKey {
        match a {
            "alice" => key(200),
            "bob" => key(201),
            "carol" => key(202),
            _ => key(250),
        }
    }
    fn policy() -> TransferPolicy {
        TransferPolicy::new(
            NetworkId::new("slotseal-testnet").unwrap(),
            OwnerRegistry::new([
                ("alice", owner_key("alice").verifying_key()),
                ("bob", owner_key("bob").verifying_key()),
                ("carol", owner_key("carol").verifying_key()),
            ])
            .unwrap(),
        )
    }
    fn order(p: &TransferPolicy, from: &str, seq: u64, to: &str, amt: u128) -> SignedTransfer {
        SignedTransfer::sign(
            p,
            Transfer { from: from.into(), from_seq: seq, to: to.into(), amount: amt },
            &owner_key(from),
        )
    }
    fn setup(n: u8) -> (Committee, Vec<Authority>, TransferPolicy) {
        let p = policy();
        let committee =
            Committee::new((0..n).map(|i| (format!("a{i}"), key(i).verifying_key())), p.clone())
                .unwrap();
        let cid = committee.id();
        let auth = (0..n)
            .map(|i| {
                Authority::new(
                    format!("a{i}"),
                    key(i),
                    p.clone(),
                    cid,
                    transfer333::Ledger::genesis([
                        ("alice".into(), 100),
                        ("bob".into(), 0),
                        ("carol".into(), 0),
                    ]),
                )
            })
            .collect();
        (committee, auth, p)
    }

    // ⭐ M1 order-invariance oracle. The RED baseline
    // (contested_slot_outcome_is_delivery_order_dependent) shows the SAME 2-1-1
    // snapshot resolves to a finalized transfer XOR a terminal lock depending on
    // the straggler's delivery. Here: route the decision through the single-decree
    // agreement and assert the decided SlotSeal is IDENTICAL across EVERY delivery
    // permutation of the underlying votes AND under Byzantine vote noise —
    // permutation-agreement = 1.0 (vs the order-dependent baseline).
    // Closes LakatoTree prediction `order-invariance` (contested_slot_outcome
    // permutation-agreement 0.5 -> 1.0). Guard G2 honoured: permutations + a
    // Byzantine vote are exercised, not a single happy path.
    #[test]
    fn ba_decides_one_order_invariant_seal_across_all_vote_permutations() {
        let (committee, mut auth, p) = setup(4);
        assert_eq!(committee.quorum(), 3);
        let order_a = order(&p, "alice", 0, "bob", 10);
        let order_b = order(&p, "alice", 0, "carol", 10);

        // Contested 2-1-1: a0,a1,a3 can vote order_a; a2 locks order_b. cert_a is
        // formable (3 votes) — the leader assembles it as the agreement evidence.
        let va0 = auth[0].handle(&order_a).unwrap();
        let va1 = auth[1].handle(&order_a).unwrap();
        let _b2 = auth[2].handle(&order_b).unwrap();
        let va3 = auth[3].handle(&order_a).unwrap();
        let votes_a = [va0, va1, va3];

        let cid = *committee.id().as_bytes();
        let honest: Vec<String> = vec!["a0".into(), "a1".into(), "a3".into()]; // 3 honest = quorum

        // a Byzantine authority votes for a bogus (Void) digest — must be ignored.
        let byz = [Msg::Vote {
            seal_digest: seal_digest(&SlotSeal::void("alice", 0, cid)),
            from: "a2".into(),
        }];

        let perms: [[usize; 3]; 6] =
            [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
        let mut distinct = HashSet::new();
        for perm in perms {
            // the leader assembles cert_a from the votes in THIS delivery order;
            // Certificate::assemble is set-based, so the cert (and its order_id)
            // is the same regardless — that is the point.
            let ordered: Vec<_> = perm.iter().map(|&i| votes_a[i].clone()).collect();
            let cert = Certificate::assemble(order_a.clone(), ordered, &committee)
                .expect("cert_a must assemble from 3 votes");
            let proposal = SealProposal {
                seal: SlotSeal::finalize("alice", 0, cid, order_a.order_id()),
                evidence: SealEvidence::CertFor(cert),
            };
            assert!(proposal.is_externally_valid(&committee));

            let decisions = run_single_decree(&committee, &proposal, &honest, &perm, &byz);
            // every honest node decided, and all decided the SAME value.
            assert!(decisions.iter().all(|d| d.is_some()), "all honest must decide");
            let vals: HashSet<_> = decisions.into_iter().map(|d| d.unwrap()).collect();
            assert_eq!(vals.len(), 1, "no fork: all honest agree on one value");
            let decided = vals.into_iter().next().unwrap();
            assert_eq!(decided.outcome, SealOutcome::Finalize { order_id: order_a.order_id() });
            distinct.insert(decided);
        }
        // ORDER-INVARIANCE: one identical decision across all 6 permutations = 1.0.
        assert_eq!(distinct.len(), 1, "decided value invariant to delivery order");
    }

    // External validity teeth: a Byzantine leader cannot get honest nodes to
    // decide an uncertified seal. Proposing Finalize(order_b) without a real
    // cert_b, or a Void, is not externally valid -> honest nodes never vote ->
    // no decision. This is the M1 half of what M2 hardens for the Void case.
    #[test]
    fn ba_rejects_byzantine_leader_proposing_an_uncertified_or_void_seal() {
        let (committee, mut auth, p) = setup(4);
        let order_a = order(&p, "alice", 0, "bob", 10);
        let order_b = order(&p, "alice", 0, "carol", 10);
        let va0 = auth[0].handle(&order_a).unwrap();
        let va1 = auth[1].handle(&order_a).unwrap();
        let va3 = auth[3].handle(&order_a).unwrap();
        let cert_a = Certificate::assemble(order_a.clone(), vec![va0, va1, va3], &committee).unwrap();
        let cid = *committee.id().as_bytes();
        let honest: Vec<String> = vec!["a0".into(), "a1".into(), "a3".into()];

        // (1) Finalize(order_b) carrying cert_a as bogus evidence: order_id mismatch.
        let bad_b = SealProposal {
            seal: SlotSeal::finalize("alice", 0, cid, order_b.order_id()),
            evidence: SealEvidence::CertFor(cert_a.clone()),
        };
        assert!(!bad_b.is_externally_valid(&committee), "cert_a does not certify order_b");
        let d1 = run_single_decree(&committee, &bad_b, &honest, &[0, 1, 2], &[]);
        assert!(d1.iter().all(|d| d.is_none()), "no honest node decides an invalid Finalize");

        // (2) Void is never externally valid at M1 (M2 hardens it).
        let void_p = SealProposal {
            seal: SlotSeal::void("alice", 0, cid),
            evidence: SealEvidence::NoCertProof,
        };
        assert!(!void_p.is_externally_valid(&committee));
        let d2 = run_single_decree(&committee, &void_p, &honest, &[0, 1, 2], &[]);
        assert!(d2.iter().all(|d| d.is_none()), "no honest node decides a Void at M1");
    }
}
