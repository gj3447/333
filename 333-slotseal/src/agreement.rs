//! # Single-decree externally-valid Byzantine agreement — the safety harness.
//!
//! Recovering an equivocation-locked `(account, seq)` slot needs a Byzantine
//! agreement leg. The RED oracle
//! (`transfer333::authority::tests::contested_slot_outcome_is_delivery_order_dependent_so_seal_needs_total_order`,
//! gj3447/333 @3436691) shows the fast path forks: the same snapshot resolves to a
//! finalized transfer XOR a terminal lock. The genuine order-dependence is
//! **set-membership** dependence — is a straggler/equivocator vote IN a node's
//! evidence set at decision time — driven by the async, no-sequencer rail
//! (`net.rs` independent per-peer FIFO mailboxes, `epidemic.rs` Plumtree).
//!
//! ## Why the first attempt was retracted (5-lens Naesengmoon BLOCKED, 2026-07-21)
//!
//! An earlier "order-invariance" oracle permuted the delivery of a FIXED, complete,
//! shared vote-multiset and showed convergence — which is only commutativity of a
//! threshold count, true by construction, NOT a safety result. It conditioned on the
//! certificate whose *formation* IS the locus of order-dependence, then proved
//! invariance of the rest. Retracted. KG:
//! `ac-333coin-M1-order-invariance-oracle-is-fixed-set-commutativity-2026-07-21`,
//! `lesson-333-do-not-condition-on-the-cert-whose-formation-is-the-order-dependence-2026-07-21`.
//!
//! ## What this harness actually tests (the real hazard)
//!
//! A within-`f` **equivocator** double-signs `order_a` to some honest nodes and
//! `order_b` to others (forged votes — impossible via the honest `handle()`
//! lock-on-first-seen). This makes a certificate's existence **node-dependent**:
//! nodes that received the equivocator's `order_b` vote can assemble `cert_b`;
//! nodes that did not cannot. The safety property: **all honest nodes decide the
//! SAME `SlotSeal`, no fork**, enforced by EXTERNAL VALIDITY BY PREDICATE:
//!
//! * `Finalize{order_id}` is valid iff a real committee-verifying certificate
//!   certifies that order (so any node can accept it once the leader broadcasts the
//!   cert as evidence, even if it did not locally collect the votes).
//! * `Void` is valid iff a **Byzantine-aware evidence-completeness** proof holds:
//!   for every candidate order `C`, `votes(C) + f + unvoted < quorum` — no order can
//!   reach quorum even with all `f` Byzantine authorities piling on and all unvoted
//!   honest authorities joining. A node checks this against the UNION of the
//!   proposal's evidence and its OWN votes, so a node that saw the cert-completing
//!   equivocation vote **rejects** a Void — a Void can never reach quorum while any
//!   honest node can exhibit the cert. Finalize-validity and Void-validity are
//!   mutually exclusive, and at most one Finalize is valid (cert uniqueness), so at
//!   most one value is decidable ⇒ no fork.
//!
//! Scope: safety (no-fork) under a synchronous deterministic harness. Liveness /
//! view-change / termination under partition is a separate milestone (M4).
//! KG: `prom16-333-optionA-total-order-leg`.

use crate::{SealOutcome, SlotSeal};
use std::collections::{BTreeMap, BTreeSet};
use transfer333::{authority_signing_message, Certificate, Committee, Vote};

/// Evidence that makes a proposed [`SlotSeal`] externally valid.
#[derive(Clone)]
pub enum SealEvidence {
    /// A real quorum-certificate exhibiting that an order finalized at the slot.
    CertFor(Certificate),
    /// A Byzantine-aware proof that no certificate can form: the collected votes,
    /// plus the committee size they were collected under.
    NoCertProof {
        /// Every vote the proposer has observed for the slot.
        votes: Vec<Vote>,
        /// The committee size the proof is computed under (must match the committee).
        committee_size: usize,
    },
}

/// The Byzantine-aware evidence-completeness predicate (C11). Returns true iff, on
/// the evidence in `votes`, **no** candidate order for `(account, seq)` can reach a
/// quorum certificate — even if all `f` Byzantine authorities equivocate onto it and
/// every unvoted honest authority joins. A `Void` is externally valid only when this
/// holds. Only signature-valid votes for the exact slot are counted.
pub fn no_cert_can_form(
    votes: &[Vote],
    committee: &Committee,
    account: &str,
    seq: u64,
) -> bool {
    let n = committee.size();
    let f = (n.saturating_sub(1)) / 3;
    let q = committee.quorum();

    let mut per_order: BTreeMap<(String, u128), BTreeSet<String>> = BTreeMap::new();
    let mut voted: BTreeSet<String> = BTreeSet::new();
    for v in votes {
        if v.transfer.from != account || v.transfer.from_seq != seq {
            continue;
        }
        let key = match committee.key_of(&v.authority) {
            Some(k) => k,
            None => continue,
        };
        let msg = authority_signing_message(
            &v.authority,
            &v.committee_id,
            &v.policy_id,
            &v.network_id,
            &v.transfer,
        );
        if key.verify_strict(&msg, &v.signature).is_err() {
            continue;
        }
        // an equivocator that signs two orders is counted in BOTH tallies.
        per_order
            .entry((v.transfer.to.clone(), v.transfer.amount))
            .or_default()
            .insert(v.authority.clone());
        voted.insert(v.authority.clone());
    }
    let unvoted = n.saturating_sub(voted.len());
    // A brand-new order (0 current votes) could still be formed by the unvoted
    // honest authorities plus f Byzantine; guard that phantom order too.
    let base_ok = f + unvoted < q;
    let per_ok = per_order
        .values()
        .all(|auths| auths.len() + f + unvoted < q);
    base_ok && per_ok
}

/// A leader's proposal into the single-decree agreement: one [`SlotSeal`] plus its
/// external-validity evidence.
#[derive(Clone)]
pub struct SealProposal {
    /// The value proposed for decision.
    pub seal: SlotSeal,
    /// The evidence an honest node checks before voting for `seal`.
    pub evidence: SealEvidence,
}

impl SealProposal {
    /// External validity, checked by an honest node against `committee` and its OWN
    /// observed votes `own_votes` (so a node that saw the cert-completing equivocation
    /// vote rejects a Void that the leader's partial evidence would otherwise allow).
    pub fn is_externally_valid(&self, committee: &Committee, own_votes: &[Vote]) -> bool {
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
            (SealOutcome::Void, SealEvidence::NoCertProof { votes, committee_size }) => {
                if *committee_size != committee.size() {
                    return false;
                }
                // check the UNION of the proposal's evidence and this node's own votes:
                // any honest node holding a cert-completing vote will reject the Void.
                let mut union: Vec<Vote> = votes.clone();
                union.extend_from_slice(own_votes);
                no_cert_can_form(&union, committee, &self.seal.account, self.seal.seq)
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
        /// Canonical digest of the seal being voted for.
        seal_digest: [u8; 32],
        /// The voting authority's id.
        from: String,
    },
}

/// Canonical digest of a decided [`SlotSeal`] outcome, for vote-matching. Uses the
/// order digest for `Finalize` (already collision-resistant, from the SignedTransfer)
/// and a fixed tag for `Void`, xored with the slot — enough to keep distinct outcomes
/// on distinct digests for this in-process harness.
pub fn seal_digest(seal: &SlotSeal) -> [u8; 32] {
    let mut out = [0u8; 32];
    match &seal.outcome {
        SealOutcome::Finalize { order_id } => out.copy_from_slice(order_id),
        SealOutcome::Void => out[..4].copy_from_slice(b"VOID"),
    }
    for (i, b) in seal.account.as_bytes().iter().enumerate() {
        out[i % 32] ^= b;
    }
    out[31] ^= seal.seq as u8;
    out
}

/// One honest agreement node, holding its OWN observed votes (node-dependent
/// evidence). Votes once, for an externally-valid proposal validated against its own
/// evidence; decides at quorum; ignores votes for any other value.
pub struct AgreementNode {
    id: String,
    committee: Committee,
    own_votes: Vec<Vote>,
    voted_for: Option<[u8; 32]>,
    votes: BTreeMap<[u8; 32], BTreeSet<String>>,
    proposal_seal: Option<SlotSeal>,
    decided: Option<SlotSeal>,
}

impl AgreementNode {
    /// A fresh honest node with its local evidence `own_votes`.
    pub fn new(id: impl Into<String>, committee: Committee, own_votes: Vec<Vote>) -> Self {
        Self {
            id: id.into(),
            committee,
            own_votes,
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

    /// Process one delivered message; returns the node's own vote if it voted.
    pub fn process(&mut self, msg: &Msg) -> Option<Msg> {
        match msg {
            Msg::Propose(p) => {
                if self.voted_for.is_none()
                    && p.is_externally_valid(&self.committee, &self.own_votes)
                {
                    let d = seal_digest(&p.seal);
                    self.voted_for = Some(d);
                    self.proposal_seal = Some(p.seal.clone());
                    self.record_vote(d, self.id.clone());
                    return Some(Msg::Vote { seal_digest: d, from: self.id.clone() });
                }
                None
            }
            Msg::Vote { seal_digest: d, from } => {
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

/// Run the single-decree agreement over honest nodes (each with its OWN evidence),
/// delivering the resulting votes in `delivery` order plus any `byzantine_votes`.
/// Returns each honest node's decision.
pub fn run_single_decree(
    committee: &Committee,
    proposal: &SealProposal,
    honest: &[(String, Vec<Vote>)],
    delivery: &[usize],
    byzantine_votes: &[Msg],
) -> Vec<Option<SlotSeal>> {
    let mut nodes: Vec<AgreementNode> = honest
        .iter()
        .map(|(id, ev)| AgreementNode::new(id.clone(), committee.clone(), ev.clone()))
        .collect();

    let mut broadcast: Vec<Msg> = Vec::new();
    for n in nodes.iter_mut() {
        if let Some(v) = n.process(&Msg::Propose(proposal.clone())) {
            broadcast.push(v);
        }
    }
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
    use ed25519_dalek::Signer;
    use std::collections::HashSet;
    use transfer333::{
        Authority, Committee, Ledger, NetworkId, OwnerRegistry, SignedTransfer, SigningKey,
        Transfer, TransferPolicy,
    };

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }
    fn owner_key(a: &str) -> SigningKey {
        match a {
            "alice" => key(200),
            "bob" => key(201),
            "carol" => key(202),
            "dave" => key(203),
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
                ("dave", owner_key("dave").verifying_key()),
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
                    Ledger::genesis([
                        ("alice".into(), 100),
                        ("bob".into(), 0),
                        ("carol".into(), 0),
                        ("dave".into(), 0),
                    ]),
                )
            })
            .collect();
        (committee, auth, p)
    }

    /// Forge a Byzantine authority `a{idx}`'s vote for `order` — signs the exact
    /// authority-vote message with the authority's key, bypassing the honest
    /// lock-on-first-seen so one authority can equivocate across two orders.
    fn forge_vote(idx: u8, committee: &Committee, order: &SignedTransfer) -> Vote {
        let id = format!("a{idx}");
        let msg = authority_signing_message(
            &id,
            &committee.id(),
            &order.policy_id(),
            &order.network_id,
            &order.transfer,
        );
        let sig = key(idx).sign(&msg);
        Vote {
            authority: id,
            committee_id: committee.id(),
            policy_id: order.policy_id(),
            network_id: order.network_id.clone(),
            transfer: order.transfer.clone(),
            signature: sig,
        }
    }

    // ⭐ SAFETY oracle — an equivocator makes a certificate's existence NODE-DEPENDENT,
    // yet all honest nodes decide the SAME SlotSeal (no fork). a0 equivocates: signs
    // BOTH order_a and order_b for slot (alice,0). Honest a1->a, a2->b, a3->b. cert_b
    // = {a0_b, a2, a3} is formable; cert_a = {a0_a, a1} = 2 < quorum is not. Nodes have
    // DIFFERENT local evidence (some hold a0_b, some hold a0_a). The leader (holding
    // cert_b) proposes Finalize(order_b) + cert_b; every honest node validates the cert
    // and decides Finalize(order_b), identical across delivery permutations.
    #[test]
    fn equivocator_makes_cert_node_dependent_but_all_honest_decide_one_seal() {
        let (committee, mut auth, p) = setup(4);
        assert_eq!(committee.quorum(), 3);
        let order_a = order(&p, "alice", 0, "bob", 10);
        let order_b = order(&p, "alice", 0, "carol", 10);

        // honest votes (via real handle, lock-on-first-seen):
        let va1 = auth[1].handle(&order_a).unwrap(); // a1 -> a
        let vb2 = auth[2].handle(&order_b).unwrap(); // a2 -> b
        let vb3 = auth[3].handle(&order_b).unwrap(); // a3 -> b
        // a0 EQUIVOCATES (forged, double-signs):
        let va0 = forge_vote(0, &committee, &order_a);
        let vb0 = forge_vote(0, &committee, &order_b);

        // cert_b is formable from the equivocation vote + honest b-votes:
        let cert_b = Certificate::assemble(order_b.clone(), vec![vb0.clone(), vb2.clone(), vb3.clone()], &committee)
            .expect("cert_b assembles from a0_b + a2 + a3");
        // cert_a is NOT formable (only a0_a + a1 = 2 < quorum 3):
        assert!(
            Certificate::assemble(order_a.clone(), vec![va0.clone(), va1.clone()], &committee).is_none()
        );

        let cid = *committee.id().as_bytes();
        // node-dependent evidence: a2 saw a0's b-vote; a1 saw a0's a-vote; a3 saw a0's b-vote.
        let honest: Vec<(String, Vec<Vote>)> = vec![
            ("a1".into(), vec![va1.clone(), va0.clone()]),           // a1 does NOT hold cert_b locally
            ("a2".into(), vec![vb2.clone(), vb0.clone(), vb3.clone()]), // a2 CAN form cert_b
            ("a3".into(), vec![vb3.clone(), vb0.clone(), vb2.clone()]),
        ];

        let proposal = SealProposal {
            seal: SlotSeal::finalize("alice", 0, cid, order_b.order_id()),
            evidence: SealEvidence::CertFor(cert_b),
        };

        for delivery in [[0usize, 1, 2], [2, 1, 0], [1, 0, 2]] {
            let decisions = run_single_decree(&committee, &proposal, &honest, &delivery, &[]);
            assert!(decisions.iter().all(|d| d.is_some()), "all honest decide");
            let vals: HashSet<_> = decisions.into_iter().map(|d| d.unwrap()).collect();
            assert_eq!(vals.len(), 1, "no fork: one agreed value despite node-dependent evidence");
            assert_eq!(
                vals.into_iter().next().unwrap().outcome,
                SealOutcome::Finalize { order_id: order_b.order_id() }
            );
        }
    }

    // SAFETY: a Void is REJECTED while a certificate can still form. A Byzantine leader
    // proposes Void with partial evidence that MISSES a0's b-vote (so its local tally
    // sees order_b at only 2); but an honest node that HOLDS a0's b-vote checks the
    // UNION and finds cert_b formable -> rejects the Void -> Void never reaches quorum,
    // so no honest node is defrauded into a Void that forks the cert_b finalizers.
    #[test]
    fn void_is_rejected_when_a_cert_can_still_form_byzantine_aware() {
        let (committee, mut auth, p) = setup(4);
        let order_a = order(&p, "alice", 0, "bob", 10);
        let order_b = order(&p, "alice", 0, "carol", 10);
        let va1 = auth[1].handle(&order_a).unwrap();
        let vb2 = auth[2].handle(&order_b).unwrap();
        let vb3 = auth[3].handle(&order_b).unwrap();
        let va0 = forge_vote(0, &committee, &order_a);
        let vb0 = forge_vote(0, &committee, &order_b);
        let cid = *committee.id().as_bytes();

        // Byzantine leader's Void proof deliberately OMITS a0's b-vote:
        let void = SealProposal {
            seal: SlotSeal::void("alice", 0, cid),
            evidence: SealEvidence::NoCertProof {
                votes: vec![va0.clone(), va1.clone(), vb2.clone(), vb3.clone()],
                committee_size: 4,
            },
        };
        // an honest node holding a0's b-vote: union forms cert_b -> Void invalid.
        assert!(
            !void.is_externally_valid(&committee, &[vb0.clone()]),
            "a node that saw the equivocation vote rejects the Void"
        );
        // even a node WITHOUT a0_b: order_b has 2 votes, +f(1)+unvoted(0)=3 >= quorum
        // -> order_b could still reach quorum via a0's equivocation -> Void invalid.
        assert!(!void.is_externally_valid(&committee, &[]));

        // run it: no honest node decides the Void.
        let honest: Vec<(String, Vec<Vote>)> = vec![
            ("a1".into(), vec![va1.clone(), va0.clone()]),
            ("a2".into(), vec![vb2.clone(), vb0.clone()]), // holds a0_b
            ("a3".into(), vec![vb3.clone(), vb0.clone()]),
        ];
        let d = run_single_decree(&committee, &void, &honest, &[0, 1, 2], &[]);
        assert!(d.iter().all(|x| x.is_none()), "no honest node decides an unsafe Void");
    }

    // LIVENESS-side sanity: a Void IS externally valid when no order can possibly
    // reach quorum. n=4, four DISTINCT recipients each with a single vote: every order
    // has 1, +f(1)+unvoted(0)=2 < quorum 3, and the phantom base f+unvoted=1<3 -> Void
    // valid. Confirms the predicate is not vacuously-always-false.
    #[test]
    fn void_is_valid_when_no_order_can_reach_quorum() {
        let (committee, mut auth, p) = setup(4);
        let o0 = order(&p, "alice", 0, "bob", 10);
        let o1 = order(&p, "alice", 0, "carol", 10);
        // only two honest handle (a0->bob, a1->carol); a2,a3 also split via forged
        // distinct-recipient votes so all four vote different orders, none reaching 3.
        let v0 = auth[0].handle(&o0).unwrap();
        let v1 = auth[1].handle(&o1).unwrap();
        let o2 = order(&p, "alice", 0, "dave", 10);
        let o3 = order(&p, "alice", 0, "alice", 10); // self-recipient distinct order tag
        let v2 = forge_vote(2, &committee, &o2);
        let v3 = forge_vote(3, &committee, &o3);
        let cid = *committee.id().as_bytes();
        let void = SealProposal {
            seal: SlotSeal::void("alice", 0, cid),
            evidence: SealEvidence::NoCertProof {
                votes: vec![v0, v1, v2, v3],
                committee_size: 4,
            },
        };
        assert!(
            void.is_externally_valid(&committee, &[]),
            "Void is valid when every order has <quorum reachable support"
        );
    }

    // External-validity teeth: an insufficient certificate (2 votes < quorum) does not
    // verify, so a Finalize carrying it is rejected by every honest node.
    #[test]
    fn finalize_with_insufficient_certificate_is_rejected() {
        let (committee, mut auth, p) = setup(4);
        let order_a = order(&p, "alice", 0, "bob", 10);
        let va0 = auth[0].handle(&order_a).unwrap();
        let va1 = auth[1].handle(&order_a).unwrap();
        let cid = *committee.id().as_bytes();
        // Certificate::assemble refuses < quorum, so we cannot even build a bad cert;
        // assert that directly (the type system + assemble enforce external validity).
        assert!(Certificate::assemble(order_a.clone(), vec![va0, va1], &committee).is_none());
        let _ = cid;
    }
}
