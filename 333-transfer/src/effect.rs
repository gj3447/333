// KG: ac-333coin-effectcert-brick-is-predicate-not-wired-system-proof-2026-07-21,
//     lesson-333-recovery-client-safety-inseparable-from-effectcert-2026-07-21
//
// EffectCert client finality (Sui-Lutris tx-cert vs effect-cert), M3 production
// wiring: the canonical types live HERE, in transfer333, because only this crate
// owns `Authority::confirm` — the one place "applied" is a fact rather than a
// claim.
//
// FINALITY MODEL
// --------------
// A bare quorum `Certificate` (a quorum of VOTES) is **PROVISIONAL**. It proves
// only that a quorum of authorities promised the slot to this order — FastPay
// Lemma A.1 certificate uniqueness still bars two conflicting certificates, but
// it does NOT prove any authority applied the order. A within-f Byzantine
// equivocator can unicast its completing vote to a client so the certificate is
// assembled AT THE CLIENT but applied at NO honest node (the withholding fork).
//
// Client finality therefore requires an **EffectCert**: a quorum of DISTINCT
// authorities each attesting "I APPLIED `order_id` at `(account, seq)`" under
// this exact committee. The attestation is produced ONLY by
// `Authority::confirm` on a genuinely committed ledger apply (debit + credit +
// sequence advance) — attest-iff-applied. Every rejection path (stale seq,
// insufficient balance, wrong committee, bad owner proof) produces NO
// attestation. In the withholding attack the honest authorities never receive
// the completing vote, so they never confirm, so no EffectCert forms and the
// client correctly stalls (liveness recovery is the agreement leg — M4, out of
// scope here).
//
// `slotseal333::finality` re-exports these canonical types; its earlier local
// duplicates (predicate-only model, pre-wiring) are retired.

use std::collections::BTreeSet;

use ed25519_dalek::Signature;

use crate::authority::{AuthorityId, Committee, CommitteeId};
use crate::AccountId;

/// Domain separation for an effect (apply) attestation, distinct from the
/// authority VOTE domain (`transfer333/authority-vote/v5`) so an effect
/// attestation can never be replayed as a vote or vice versa.
pub const EFFECT_ATTESTATION_DOMAIN: &[u8] = b"transfer333/effect-attestation/v1\0";

/// Canonical message an authority signs to attest it APPLIED `order_id` at
/// `(account, seq)` under a specific committee. Same layout discipline as
/// `authority_signing_message`: domain tag, then length-prefixed variable
/// fields so no two field splices can collide.
pub fn effect_message(
    committee_id: &CommitteeId,
    account: &str,
    seq: u64,
    order_id: &[u8; 32],
) -> Vec<u8> {
    let mut m = Vec::with_capacity(EFFECT_ATTESTATION_DOMAIN.len() + 32 + 8 + account.len() + 8 + 32);
    m.extend_from_slice(EFFECT_ATTESTATION_DOMAIN);
    m.extend_from_slice(committee_id.as_bytes());
    m.extend_from_slice(&(account.len() as u64).to_le_bytes());
    m.extend_from_slice(account.as_bytes());
    m.extend_from_slice(&seq.to_le_bytes());
    m.extend_from_slice(order_id);
    m
}

/// One authority's signed attestation that it APPLIED `order_id` at the slot.
///
/// Produced only by `Authority::confirm` (served via `Authority::attestation_for`)
/// after the ledger apply committed; it is otherwise unforgeable outside the
/// authority's key holder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectAttestation {
    /// The attesting authority id (a string alias, as in `authority::Vote`).
    pub authority: AuthorityId,
    /// The committee the attestation is bound to.
    pub committee_id: CommitteeId,
    /// Slot account.
    pub account: AccountId,
    /// Slot sequence.
    pub seq: u64,
    /// The applied order's digest (`SignedTransfer::order_id`).
    pub order_id: [u8; 32],
    /// Ed25519 signature over [`effect_message`].
    pub signature: Signature,
}

impl EffectAttestation {
    /// Verify this attestation binds to `committee` and carries a valid
    /// signature by the named authority's committee key.
    pub fn is_valid(&self, committee: &Committee) -> bool {
        if self.committee_id != committee.id() {
            return false;
        }
        let key = match committee.key_of(&self.authority) {
            Some(k) => k,
            None => return false,
        };
        let msg = effect_message(&self.committee_id, &self.account, self.seq, &self.order_id);
        key.verify_strict(&msg, &self.signature).is_ok()
    }
}

/// An EffectCert: a quorum of DISTINCT authorities each attesting they applied
/// `order_id` at `(account, seq)`. This is the FINALITY object — its existence,
/// not a bare tx-cert, is what makes an order final to a client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectCert {
    account: AccountId,
    seq: u64,
    order_id: [u8; 32],
}

impl EffectCert {
    /// The finalized order digest.
    pub fn order_id(&self) -> [u8; 32] {
        self.order_id
    }

    /// The finalized slot.
    pub fn slot(&self) -> (&AccountId, u64) {
        (&self.account, self.seq)
    }

    /// Assemble an EffectCert iff at least `quorum` DISTINCT authorities
    /// validly attest they applied the SAME `order_id` at the SAME slot.
    /// Returns `None` otherwise — the client is not final. Duplicate signers
    /// count once; cross-committee, forged, and mis-bound attestations are
    /// discarded by `is_valid` / the field match.
    pub fn assemble(
        account: &str,
        seq: u64,
        order_id: [u8; 32],
        attestations: &[EffectAttestation],
        committee: &Committee,
    ) -> Option<Self> {
        let mut distinct = BTreeSet::new();
        for a in attestations {
            if a.account == account
                && a.seq == seq
                && a.order_id == order_id
                && a.is_valid(committee)
            {
                distinct.insert(a.authority.clone());
            }
        }
        if distinct.len() >= committee.quorum() {
            Some(Self {
                account: account.to_string(),
                seq,
                order_id,
            })
        } else {
            None
        }
    }
}

/// **The migrated client finality predicate.** An order is final to a client
/// iff a valid [`EffectCert`] (a quorum of distinct apply-attestations) can be
/// assembled from the client's collected `attestations`. A bare transfer
/// `Certificate` is PROVISIONAL, never final.
pub fn is_final_effectcert(
    account: &str,
    seq: u64,
    order_id: [u8; 32],
    attestations: &[EffectAttestation],
    committee: &Committee,
) -> bool {
    EffectCert::assemble(account, seq, order_id, attestations, committee).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{
        authority_signing_message, Authority, Certificate, ConfirmError, ConfirmOutcome, Verified,
        Vote,
    };
    use crate::owner::{NetworkId, OwnerRegistry};
    use crate::{Ledger, Reject, SignedTransfer, Transfer, TransferPolicy};
    use ed25519_dalek::{Signer, SigningKey};

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
            NetworkId::new("effect-testnet").unwrap(),
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

    fn signed_order(
        policy: &TransferPolicy,
        from: &str,
        seq: u64,
        to: &str,
        amount: u128,
    ) -> SignedTransfer {
        SignedTransfer::sign(
            policy,
            Transfer {
                from: from.into(),
                from_seq: seq,
                to: to.into(),
                amount,
            },
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

    /// A Byzantine authority's vote for an order it never locked (the
    /// equivocator's capability — it signs whatever it likes with its own key).
    fn forge_vote(idx: u8, committee: &Committee, order: &SignedTransfer) -> Vote {
        let id = format!("a{idx}");
        let msg = authority_signing_message(
            &id,
            &committee.id(),
            &order.policy_id(),
            &order.network_id,
            &order.transfer,
            order.round,
        );
        Vote {
            authority: id,
            committee_id: committee.id(),
            policy_id: order.policy_id(),
            network_id: order.network_id.clone(),
            transfer: order.transfer.clone(),
            round: order.round,
            signature: key(idx).sign(&msg),
        }
    }

    /// Drive the real path: present a certificate to `Authority::confirm` and
    /// return whatever attestation the authority then serves for the slot.
    /// A rejected confirm yields no attestation by construction.
    fn confirm_and_attest(
        authority: &mut Authority,
        verified: &Verified,
        committee: &Committee,
        account: &str,
        seq: u64,
    ) -> Option<EffectAttestation> {
        match authority.confirm(verified, committee) {
            Ok(_) => authority.attestation_for(&account.to_string(), seq),
            Err(_) => None,
        }
    }

    // ATTEST-IFF-APPLIED: a genuinely applied confirm yields a verifiable
    // attestation bound to (committee, account, seq, order_id); EVERY rejection
    // path (stale seq, insufficient balance, foreign/wrong cert) yields none.
    #[test]
    fn attestation_emitted_only_on_applied_confirm() {
        let (committee, mut authorities, policy) = setup(4);
        let order = signed_order(&policy, "alice", 0, "bob", 30);
        let oid = order.order_id();
        let votes: Vec<Vote> = authorities
            .iter_mut()
            .take(3)
            .map(|a| a.handle(&order).unwrap())
            .collect();
        let verified = Certificate::assemble(order.clone(), votes, &committee)
            .unwrap()
            .verify(&committee)
            .unwrap();

        // Applied confirm -> attestation exists and verifies.
        assert_eq!(
            authorities[0].confirm(&verified, &committee),
            Ok(ConfirmOutcome::Applied)
        );
        let att = authorities[0]
            .attestation_for(&"alice".to_string(), 0)
            .expect("an applied confirm must emit an attestation");
        assert_eq!(att.authority, "a0");
        assert_eq!(att.committee_id, committee.id());
        assert_eq!(att.account, "alice");
        assert_eq!(att.seq, 0);
        assert_eq!(att.order_id, oid);
        assert!(att.is_valid(&committee));
        // Idempotent re-confirm still serves it (the effect WAS applied).
        assert_eq!(
            authorities[0].confirm(&verified, &committee),
            Ok(ConfirmOutcome::AlreadyApplied)
        );
        assert!(authorities[0].attestation_for(&"alice".to_string(), 0).is_some());

        // REJECTION 1 — stale seq: once a1 has also applied the original
        // order, a conflicting certificate for the same slot can never
        // confirm, so no attestation binds the conflict anywhere.
        assert_eq!(
            authorities[1].confirm(&verified, &committee),
            Ok(ConfirmOutcome::Applied)
        );
        let conflict = signed_order(&policy, "alice", 0, "carol", 30);
        let conflict_oid = conflict.order_id();
        // a3 never locked the original; a0/a2 are modeled Byzantine here so a
        // structurally valid conflicting certificate exists at all.
        let conflict_votes = vec![
            authorities[3].handle(&conflict).unwrap(),
            forge_vote(0, &committee, &conflict),
            forge_vote(2, &committee, &conflict),
        ];
        let conflict_verified = Certificate::assemble(conflict, conflict_votes, &committee)
            .unwrap()
            .verify(&committee)
            .unwrap();
        assert!(matches!(
            authorities[1].confirm(&conflict_verified, &committee),
            Err(ConfirmError::State(Reject::StaleSequence { .. }))
        ));
        let att = authorities[1]
            .attestation_for(&"alice".to_string(), 0)
            .expect("a1 applied the original order, so it attests THAT one");
        assert_ne!(att.order_id, conflict_oid, "no attestation may bind the rejected order");

        // REJECTION 2 — insufficient balance: a valid certificate an
        // underfunded authority cannot apply produces no attestation.
        let mut underfunded = Authority::new(
            "a0",
            key(0),
            policy.clone(),
            committee.id(),
            Ledger::genesis([("alice".into(), 10), ("bob".into(), 0), ("carol".into(), 0)]),
        );
        assert_eq!(
            underfunded.confirm(&verified, &committee),
            Err(ConfirmError::State(Reject::Insufficient { have: 10, need: 30 }))
        );
        assert!(
            underfunded.attestation_for(&"alice".to_string(), 0).is_none(),
            "a failed apply must emit no attestation"
        );

        // REJECTION 3 — wrong/foreign committee cert: rejected before any
        // state mutation, no attestation.
        let foreign = Committee::new(
            (0..4).map(|i| (format!("x{i}"), key(100 + i).verifying_key())),
            policy.clone(),
        )
        .unwrap();
        let mut foreign_auths: Vec<Authority> = (0..4)
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
        let fvotes: Vec<Vote> = foreign_auths
            .iter_mut()
            .take(3)
            .map(|a| a.handle(&order).unwrap())
            .collect();
        let foreign_verified = Certificate::assemble(order.clone(), fvotes, &foreign)
            .unwrap()
            .verify(&foreign)
            .unwrap();
        assert!(matches!(
            authorities[2].confirm(&foreign_verified, &committee),
            Err(ConfirmError::WrongCommittee { .. })
        ));
        assert!(
            authorities[2].attestation_for(&"alice".to_string(), 0).is_none(),
            "a rejected cert must emit no attestation"
        );

        // And an authority that NEVER saw any cert attests nothing.
        assert!(authorities[3].attestation_for(&"alice".to_string(), 0).is_none());
    }

    // THE ADVERSARY'S DILEMMA, derived through the real confirm() path (n=4,
    // f=1, quorum=3): a0 equivocates and UNICASTS its completing b-vote to the
    // client only. The client assembles a bare Certificate — but the honest
    // authorities never receive the cert, never confirm, and therefore serve
    // NO attestation. The client collects 1 < quorum attestations and must
    // WITHHOLD finality even though cert-exists would have finalized. If a0
    // instead DELIVERS the cert, honest confirms derive a quorum of
    // attestations and the SAME predicate flips to final. No attestation set
    // is hand-supplied: every attestation comes out of `Authority::confirm`.
    #[test]
    fn withholding_authority_blocks_effectcert_finality() {
        let (committee, mut authorities, policy) = setup(4);
        assert_eq!(committee.quorum(), 3);
        let order_a = signed_order(&policy, "alice", 0, "bob", 10);
        let order_b = signed_order(&policy, "alice", 0, "carol", 10);
        let oid_b = order_b.order_id();

        // a0,a1 lock order_a; honest a2,a3 lock order_b. a0 then equivocates:
        // its b-vote is forged (it is Byzantine; it can sign anything).
        authorities[0].handle(&order_a).unwrap();
        authorities[1].handle(&order_a).unwrap();
        let vb2 = authorities[2].handle(&order_b).unwrap();
        let vb3 = authorities[3].handle(&order_b).unwrap();
        let vb0 = forge_vote(0, &committee, &order_b);

        // The client holds a bare certificate for order_b (a0 unicast the
        // completing vote to the client ONLY). cert-exists is satisfied...
        let cert_b = Certificate::assemble(order_b.clone(), vec![vb0, vb2, vb3], &committee)
            .expect("the client's assembled certificate is structurally valid");
        assert!(cert_b.is_valid(&committee), "cert-exists WOULD finalize here");
        let verified_b = cert_b.verify(&committee).unwrap();

        // WITHHOLDING: the equivocator confirms its own unicast, but the
        // honest a1,a2,a3 never receive the cert. Derived attestations = only
        // what real confirms produced.
        let a0 = confirm_and_attest(&mut authorities[0], &verified_b, &committee, "alice", 0);
        let a1 = authorities[1].attestation_for(&"alice".to_string(), 0);
        let a2 = authorities[2].attestation_for(&"alice".to_string(), 0);
        let a3 = authorities[3].attestation_for(&"alice".to_string(), 0);
        let collected: Vec<_> = [a0, a1, a2, a3].into_iter().flatten().collect();
        assert_eq!(
            collected.len(),
            1,
            "withholding DERIVES exactly one attestation: only a0 applied"
        );
        assert!(
            !is_final_effectcert("alice", 0, oid_b, &collected, &committee),
            "quorum-1 attestations -> NOT final, despite the existing bare cert"
        );
        assert!(EffectCert::assemble("alice", 0, oid_b, &collected, &committee).is_none());

        // ADVERSARY'S DILEMMA: deliver the cert to the honest authorities and
        // the same predicate flips — real confirms derive a quorum.
        let d1 = confirm_and_attest(&mut authorities[1], &verified_b, &committee, "alice", 0);
        let d2 = confirm_and_attest(&mut authorities[2], &verified_b, &committee, "alice", 0);
        let d3 = confirm_and_attest(&mut authorities[3], &verified_b, &committee, "alice", 0);
        let mut delivered: Vec<_> = [d1, d2, d3].into_iter().flatten().collect();
        delivered.extend(collected);
        assert!(delivered.len() >= committee.quorum());
        assert!(
            is_final_effectcert("alice", 0, oid_b, &delivered, &committee),
            "delivery -> quorum of apply-attestations -> final"
        );
        for authority in &authorities {
            assert_eq!(authority.ledger().balance(&"carol".into()), 10);
            assert_eq!(authority.ledger().balance(&"bob".into()), 0);
        }
    }

    // Common case: a quorum of authorities genuinely confirm the same order
    // through `certify`; their real attestations assemble a final EffectCert.
    #[test]
    fn delivered_attestations_assemble_final_effectcert() {
        let (committee, mut authorities, policy) = setup(4);
        let order = signed_order(&policy, "alice", 0, "bob", 30);
        let oid = order.order_id();
        let (verified, result) = crate::certify(&order, &mut authorities, &committee);
        assert_eq!(result, crate::Certified::Ok);
        assert!(verified.is_some());

        let attestations: Vec<EffectAttestation> = authorities
            .iter()
            .filter_map(|a| a.attestation_for(&"alice".to_string(), 0))
            .collect();
        assert_eq!(attestations.len(), 4, "every authority applied and attests");
        assert!(attestations.iter().all(|a| a.is_valid(&committee)));

        let effect_cert = EffectCert::assemble("alice", 0, oid, &attestations, &committee)
            .expect("a quorum of real apply-attestations assembles an EffectCert");
        assert_eq!(effect_cert.order_id(), oid);
        assert_eq!(effect_cert.slot(), (&"alice".to_string(), 0));
        assert!(is_final_effectcert("alice", 0, oid, &attestations, &committee));

        // A bare quorum-minus-one is NOT final.
        assert!(!is_final_effectcert(
            "alice",
            0,
            oid,
            &attestations[..committee.quorum() - 1],
            &committee
        ));
    }

    // Assembly rejects anything that is not a quorum of DISTINCT, correctly
    // bound, committee-verified attestations: cross-committee signatures,
    // wrong (order_id / seq / account) bindings, unknown or impersonated
    // signers, and duplicate signers.
    #[test]
    fn effectcert_rejects_cross_committee_and_forged_attestations() {
        let (committee, mut authorities, policy) = setup(4);
        // A second committee over a DIFFERENT roster size (same key seeds) so
        // its id differs; its attestations must be worthless here.
        let other = Committee::new(
            (0..5).map(|i| (format!("a{i}"), key(i).verifying_key())),
            policy.clone(),
        )
        .unwrap();
        let mut other_auths: Vec<Authority> = (0..5)
            .map(|i| {
                Authority::new(format!("a{i}"), key(i), policy.clone(), other.id(), genesis())
            })
            .collect();
        assert_ne!(committee.id(), other.id());

        let order = signed_order(&policy, "alice", 0, "bob", 10);
        let oid = order.order_id();
        let (_, result) = crate::certify(&order, &mut authorities, &committee);
        assert_eq!(result, crate::Certified::Ok);
        let real: Vec<EffectAttestation> = authorities
            .iter()
            .filter_map(|a| a.attestation_for(&"alice".to_string(), 0))
            .collect();

        // Cross-committee: the same slot applied under `other` produces
        // attestations that are invalid here and cannot pad a quorum.
        let (_, r2) = crate::certify(&order, &mut other_auths, &other);
        assert_eq!(r2, crate::Certified::Ok);
        let foreign: Vec<EffectAttestation> = other_auths
            .iter()
            .filter_map(|a| a.attestation_for(&"alice".to_string(), 0))
            .collect();
        assert!(!foreign.is_empty());
        assert!(foreign.iter().all(|a| !a.is_valid(&committee)));
        assert!(!is_final_effectcert("alice", 0, oid, &foreign, &committee));
        // Mixed: 2 real + 2 foreign is still below a quorum of VALID ones.
        let mut mixed = real[..2].to_vec();
        mixed.extend(foreign.iter().take(2).cloned());
        assert!(!is_final_effectcert("alice", 0, oid, &mixed, &committee));

        // Forged signer: a non-committee key's attestation is invalid; an
        // attestation signed by a1's key but claiming to be a0 is invalid.
        let msg = effect_message(&committee.id(), "alice", 0, &oid);
        let outsider = EffectAttestation {
            authority: "mallory".into(),
            committee_id: committee.id(),
            account: "alice".into(),
            seq: 0,
            order_id: oid,
            signature: key(99).sign(&msg),
        };
        assert!(!outsider.is_valid(&committee));
        let impersonated = EffectAttestation {
            authority: "a0".into(),
            signature: key(1).sign(&msg),
            ..outsider.clone()
        };
        assert!(!impersonated.is_valid(&committee));

        // Wrong bindings: a real attestation does not count for a different
        // order_id, seq, or account.
        assert!(!is_final_effectcert("alice", 0, [0xEE; 32], &real, &committee));
        assert!(!is_final_effectcert("alice", 1, oid, &real, &committee));
        assert!(!is_final_effectcert("bob", 0, oid, &real, &committee));

        // Duplicate signer: the same attestation three times is one signer.
        let dupes = vec![real[0].clone(), real[0].clone(), real[0].clone()];
        assert!(EffectCert::assemble("alice", 0, oid, &dupes, &committee).is_none());
    }
}
