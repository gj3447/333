//! # EffectCert client-finality predicate (applied-at-quorum, Sui-Lutris effect-cert).
//!
//! ## Status (M3, 2026-07-23): the production wiring LANDED — honest scoping kept.
//!
//! The canonical [`EffectAttestation`] / [`EffectCert`] / [`is_final_effectcert`]
//! types now live in **`transfer333::effect`** (re-exported here; the local
//! duplicates from the predicate-only milestone are retired), and
//! **attest-iff-applied is wired into the real `transfer333::authority::
//! confirm()`**: an authority serves an attestation for `(account, seq)` iff its
//! ledger apply genuinely committed (debit + credit + sequence advance), with
//! every rejection path producing none. The withholding-safety and adversary's-
//! dilemma properties are DERIVED through that real confirm path in
//! `transfer333::effect::tests` (`withholding_authority_blocks_effectcert_finality`,
//! `attestation_emitted_only_on_applied_confirm`) — no hand-supplied attestation
//! sets (the 3e03d76 vacuity bug, fixed there and here).
//!
//! **Still honestly scoped:** this establishes the predicate + the production
//! wiring. It does NOT yet prove SYSTEM client-safety closes the withholding
//! fork: that needs the multi-decider withholding harness (concurrent clients /
//! deciders racing a within-f equivocator) and the griefing/liveness argument
//! (a withholding equivocator converts premature finalization into a safe
//! honest STALL = finality-DoS on the target slot, resolved only by the
//! agreement/recovery leg — M4, a separate milestone). KG:
//! `ac-333coin-effectcert-brick-is-predicate-not-wired-system-proof-2026-07-21`.
//!
//! Original design context (kept): the recovery client-safety is inseparable
//! from the client finality predicate: under the shipped `cert-exists` rule (a
//! bare verifying `transfer333::Certificate` = final, FastPay Lemma A.1), a
//! within-`f` Byzantine equivocator can UNICAST its completing vote to a client
//! so a certificate is assembled AT THE CLIENT but applied at NO honest node —
//! the client finalizes while every honest node stalls. KG:
//! `ac-333coin-harness-nofork-is-agreement-object-not-client-finality-2026-07-21`,
//! `lesson-333-recovery-client-safety-inseparable-from-effectcert-2026-07-21`.
//!
//! **The brick:** migrate client finality from *cert-assembled* to *applied-at-
//! quorum*. A bare transfer `Certificate` (a **tx-cert**: a quorum of VOTES) is
//! only PROVISIONAL. Finality requires an **EffectCert**: a quorum of
//! authorities each attesting it APPLIED the order at the slot. This is the
//! Sui-Lutris tx-cert vs effect-cert distinction. An authority attests the
//! effect ONLY if it confirmed the cert (i.e. it actually holds the votes and
//! applied). In the withholding attack the honest authorities never receive the
//! equivocator's completing vote, so they never assemble/apply the cert, so no
//! EffectCert forms — and the client, requiring an EffectCert, correctly
//! WITHHOLDS finality. The premature finalization is converted into a safe
//! stall (liveness, resolved by the agreement leg + a view synchronizer,
//! separate milestone).
//!
//! Scope: the finality predicate + its withholding-safety, over `transfer333`'s
//! real `Certificate`/`Committee`/`quorum` types. Not in scope: the multi-
//! decider withholding harness and liveness (M4).
//! KG: `prom16-333-optionA-total-order-leg`.

use transfer333::Certificate;

// Canonical EffectCert types: defined in `transfer333::effect` (the crate that
// owns `Authority::confirm`), re-exported so this module's public API keeps
// compiling unchanged.
pub use transfer333::{
    effect_message, is_final_effectcert, EffectAttestation, EffectCert,
};

/// **The OLD, unsafe predicate** (kept only to demonstrate the fork it
/// permits): a bare verifying transfer `Certificate` treated as final. This is
/// exactly what the withholding attack exploits; clients must use
/// [`is_final_effectcert`].
pub fn is_final_cert_exists(cert: &Certificate, committee: &transfer333::Committee) -> bool {
    cert.verify(committee).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;
    use transfer333::{
        authority_signing_message, Authority, Certificate, Committee, Ledger, NetworkId,
        OwnerRegistry, SignedTransfer, SigningKey, Transfer, TransferPolicy, Vote,
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
            NetworkId::new("slotseal-fin-testnet").unwrap(),
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
                    Ledger::genesis([("alice".into(), 100), ("bob".into(), 0), ("carol".into(), 0)]),
                )
            })
            .collect();
        (committee, auth, p)
    }
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

    /// Forge an authority's effect attestation (an authority applies + attests). Used
    /// to model who could attest — in the withholding attack only the equivocator,
    /// which holds the cert, could attest, and one attestation is below quorum.
    fn attest(idx: u8, committee: &Committee, account: &str, seq: u64, order_id: [u8; 32]) -> EffectAttestation {
        let id = format!("a{idx}");
        let msg = effect_message(&committee.id(), account, seq, &order_id);
        EffectAttestation {
            authority: id,
            committee_id: committee.id(),
            account: account.to_string(),
            seq,
            order_id,
            signature: key(idx).sign(&msg),
        }
    }

    // An authority attests order X only if it could actually CONFIRM/apply X — i.e. it
    // holds a valid quorum certificate for X from its OWN evidence. This MODELS the
    // attest-iff-applied binding at the predicate level so the withholding set is
    // DERIVED from what each node holds, not hand-picked; the production wire itself
    // (confirm() -> attestation_for) landed in M3 and is driven end-to-end in
    // `transfer333::effect::tests`.
    fn attest_if_can_confirm(
        idx: u8,
        own_votes: &[Vote],
        order: &SignedTransfer,
        committee: &Committee,
    ) -> Option<EffectAttestation> {
        Certificate::assemble(order.clone(), own_votes.to_vec(), committee)?;
        Some(attest(
            idx,
            committee,
            &order.transfer.from,
            order.transfer.from_seq,
            order.order_id(),
        ))
    }

    // EffectCert vs cert-exists under withholding — the attestation set is DERIVED
    // (attest-iff-can-confirm), NOT hand-picked, and the predicate FLIPS with the
    // adversary's choice (the adversary's dilemma), so this is not the earlier
    // tautology. a0 equivocates and WITHHOLDS its order_b vote to itself/a client.
    #[test]
    fn effectcert_withholds_derived_finality_but_delivery_grants_it() {
        let (committee, mut auth, p) = setup(4);
        assert_eq!(committee.quorum(), 3);
        let order_a = order(&p, "alice", 0, "bob", 10);
        let order_b = order(&p, "alice", 0, "carol", 10);
        let _va1 = auth[1].handle(&order_a).unwrap(); // a1 locked order_a
        let vb2 = auth[2].handle(&order_b).unwrap(); // honest a2 -> b
        let vb3 = auth[3].handle(&order_b).unwrap(); // honest a3 -> b
        let vb0 = forge_vote(0, &committee, &order_b); // equivocator a0's b-vote
        let oid_b = order_b.order_id();

        // WITHHOLDING: honest a2,a3 hold only {a2_b, a3_b} = 2 < quorum -> cannot confirm
        // cert_b -> DERIVED: no attestation. Only a0 (holding the cert) can attest.
        let a2 = attest_if_can_confirm(2, &[vb2.clone(), vb3.clone()], &order_b, &committee);
        let a3 = attest_if_can_confirm(3, &[vb3.clone(), vb2.clone()], &order_b, &committee);
        let a0 = attest_if_can_confirm(0, &[vb0.clone(), vb2.clone(), vb3.clone()], &order_b, &committee);
        assert!(a2.is_none() && a3.is_none(), "honest cannot attest what they could not confirm");
        assert!(a0.is_some(), "the equivocator, holding the cert, can attest");
        let derived: Vec<_> = [a2, a3, a0].into_iter().flatten().collect();
        assert_eq!(derived.len(), 1, "withholding DERIVES 1 < quorum attestations");
        assert!(
            !is_final_effectcert("alice", 0, oid_b, &derived, &committee),
            "withholding -> not final (safe stall)"
        );
        // cert-exists WOULD have prematurely finalized the client's assembled cert:
        let cert_b =
            Certificate::assemble(order_b.clone(), vec![vb0.clone(), vb2.clone(), vb3.clone()], &committee)
                .unwrap();
        assert!(is_final_cert_exists(&cert_b, &committee));

        // ADVERSARY'S DILEMMA (this is why it is not a tautology): if a0 DELIVERS its
        // vote instead of withholding, honest a2,a3 CAN confirm cert_b -> DERIVED: they
        // attest -> quorum -> final. The SAME predicate flips with the adversary choice.
        let d2 = attest_if_can_confirm(2, &[vb2.clone(), vb3.clone(), vb0.clone()], &order_b, &committee);
        let d3 = attest_if_can_confirm(3, &[vb3.clone(), vb2.clone(), vb0.clone()], &order_b, &committee);
        let d0 = attest_if_can_confirm(0, &[vb0.clone(), vb2.clone(), vb3.clone()], &order_b, &committee);
        let delivered: Vec<_> = [d2, d3, d0].into_iter().flatten().collect();
        assert_eq!(delivered.len(), 3, "delivery DERIVES a quorum of attestations");
        assert!(is_final_effectcert("alice", 0, oid_b, &delivered, &committee), "delivery -> final");
    }

    // Common case preserved: a genuinely applied order (a quorum of honest authorities
    // confirm it) DOES form an EffectCert -> final. EffectCert does not break the
    // uncontended path.
    #[test]
    fn effectcert_is_final_when_a_quorum_actually_applied() {
        let (committee, _auth, p) = setup(4);
        let o = order(&p, "alice", 0, "bob", 10);
        let oid = o.order_id();
        // a0,a1,a2 each applied and attest (3 = quorum).
        let attestations = vec![
            attest(0, &committee, "alice", 0, oid),
            attest(1, &committee, "alice", 0, oid),
            attest(2, &committee, "alice", 0, oid),
        ];
        assert!(
            is_final_effectcert("alice", 0, oid, &attestations, &committee),
            "a quorum of apply-attestations makes the order final"
        );
    }

    // EffectCert requires n-f (not 2f+1) at a genuinely NON-canonical committee. n=5,
    // f=1: n-f=4, 2f+1=3 DIFFER (unlike n=7=3f+1 where they coincide — the prior test's
    // bug). 3 (=2f+1) must NOT finalize; 4 (=n-f) must. Also: EffectCert UNIQUENESS —
    // even with a double-attesting Byzantine a0, two conflicting orders cannot BOTH
    // reach n-f (two quorums of 4 in n=5 share >=3, >=2 honest, who each applied only
    // one order), so no two conflicting EffectCerts -> no finality fork.
    #[test]
    fn effectcert_needs_n_minus_f_not_2f_plus_1_and_is_unique_at_n5() {
        let (committee, _auth, p) = setup(5);
        assert_eq!(committee.quorum(), 4); // n-f = 5-1 (2f+1 = 3 would be UNSAFE)
        let oa = order(&p, "alice", 0, "bob", 10);
        let ob = order(&p, "alice", 0, "carol", 10);
        let oid_a = oa.order_id();
        let oid_b = ob.order_id();

        let three: Vec<_> = (0..3).map(|i| attest(i, &committee, "alice", 0, oid_a)).collect();
        assert!(
            !is_final_effectcert("alice", 0, oid_a, &three, &committee),
            "3 (=2f+1) must NOT finalize at n=5; only n-f=4 does"
        );
        let four: Vec<_> = (0..4).map(|i| attest(i, &committee, "alice", 0, oid_a)).collect();
        assert!(is_final_effectcert("alice", 0, oid_a, &four, &committee));

        // Uniqueness under the adversary's best split: a0 double-attests both orders,
        // honest split 2/2. Neither reaches n-f=4 -> no two conflicting EffectCerts.
        let ec_a = vec![
            attest(0, &committee, "alice", 0, oid_a),
            attest(1, &committee, "alice", 0, oid_a),
            attest(2, &committee, "alice", 0, oid_a),
        ]; // {a0,a1,a2} = 3 < 4
        let ec_b = vec![
            attest(0, &committee, "alice", 0, oid_b),
            attest(3, &committee, "alice", 0, oid_b),
            attest(4, &committee, "alice", 0, oid_b),
        ]; // {a0,a3,a4} = 3 < 4
        assert!(!is_final_effectcert("alice", 0, oid_a, &ec_a, &committee));
        assert!(!is_final_effectcert("alice", 0, oid_b, &ec_b, &committee));
    }

    // Cross-committee / forged attestation is rejected: an attestation whose signature
    // was made for a DIFFERENT committee does not validate, so it cannot pad a quorum.
    #[test]
    fn effectcert_rejects_cross_committee_or_unsigned_attestation() {
        let (committee, _a, p) = setup(4);
        let (other, _b, _p2) = setup(5); // different roster/id
        let o = order(&p, "alice", 0, "bob", 10);
        let oid = o.order_id();
        // an attestation signed under `other`'s committee id, presented to `committee`:
        let foreign = attest(0, &other, "alice", 0, oid);
        assert!(!foreign.is_valid(&committee), "cross-committee attestation is invalid");
        // even three foreign attestations cannot finalize under `committee`:
        let foreigns: Vec<_> = (0..3).map(|i| attest(i, &other, "alice", 0, oid)).collect();
        assert!(!is_final_effectcert("alice", 0, oid, &foreigns, &committee));
    }
}
