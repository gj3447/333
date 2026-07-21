//! # EffectCert client-finality — closing the withholding fork (the load-bearing joint).
//!
//! The 5-lens re-verification showed the recovery's client-safety is INSEPARABLE
//! from the client finality predicate: under the shipped `cert-exists` rule (a bare
//! verifying `transfer333::Certificate` = final, FastPay Lemma A.1), a within-`f`
//! Byzantine equivocator can UNICAST its completing vote to a client so a certificate
//! is assembled AT THE CLIENT but applied at NO honest node — the client finalizes
//! while every honest node stalls. KG:
//! `ac-333coin-harness-nofork-is-agreement-object-not-client-finality-2026-07-21`,
//! `lesson-333-recovery-client-safety-inseparable-from-effectcert-2026-07-21`.
//!
//! **The brick:** migrate client finality from *cert-assembled* to *applied-at-
//! quorum*. A bare transfer `Certificate` (a **tx-cert**: a quorum of VOTES) is only
//! PROVISIONAL. Finality requires an **EffectCert**: a quorum of authorities each
//! attesting it APPLIED the order at the slot. This is the Sui-Lutris tx-cert vs
//! effect-cert distinction. An authority attests the effect ONLY if it confirmed the
//! cert (i.e. it actually holds the votes and applied). In the withholding attack the
//! honest authorities never receive the equivocator's completing vote, so they never
//! assemble/apply the cert, so no EffectCert forms — and the client, requiring an
//! EffectCert, correctly WITHHOLDS finality. The premature finalization is converted
//! into a safe stall (liveness, resolved by the agreement leg + a view synchronizer,
//! separate milestone).
//!
//! Scope: the finality predicate + its withholding-safety, over `transfer333`'s real
//! `Certificate`/`Committee`/`quorum` types. Not in scope: wiring the effect-
//! attestation into `transfer333::authority::confirm()` production path, and liveness.
//! KG: `prom16-333-optionA-total-order-leg`.

use std::collections::BTreeSet;
use transfer333::{Certificate, Committee, Signature, VerifyingKey};

/// Domain separation for an effect (apply) attestation, distinct from the authority
/// VOTE domain so an effect attestation can never be replayed as a vote or vice versa.
const EFFECT_DOMAIN: &[u8] = b"333/slot-effect-applied/v1\0";

/// Canonical message an authority signs to attest it APPLIED `order_id` at
/// `(account, seq)` under a specific committee.
pub fn effect_message(
    committee_id: &[u8; 32],
    account: &str,
    seq: u64,
    order_id: &[u8; 32],
) -> Vec<u8> {
    let mut m = Vec::with_capacity(EFFECT_DOMAIN.len() + 32 + 8 + account.len() + 8 + 32);
    m.extend_from_slice(EFFECT_DOMAIN);
    m.extend_from_slice(committee_id);
    m.extend_from_slice(&(account.len() as u64).to_le_bytes());
    m.extend_from_slice(account.as_bytes());
    m.extend_from_slice(&seq.to_le_bytes());
    m.extend_from_slice(order_id);
    m
}

/// One authority's signed attestation that it applied `order_id` at the slot.
#[derive(Clone, Debug)]
pub struct EffectAttestation {
    /// The attesting authority id (a string alias, as in `transfer333`).
    pub authority: String,
    /// The committee the attestation is bound to.
    pub committee_id: [u8; 32],
    /// Slot account.
    pub account: String,
    /// Slot sequence.
    pub seq: u64,
    /// The applied order's digest (`SignedTransfer::order_id`).
    pub order_id: [u8; 32],
    /// Ed25519 signature over [`effect_message`].
    pub signature: Signature,
}

impl EffectAttestation {
    /// Verify this attestation binds to `committee` and carries a valid signature by
    /// the named authority's committee key.
    pub fn is_valid(&self, committee: &Committee) -> bool {
        if self.committee_id != *committee.id().as_bytes() {
            return false;
        }
        let key: &VerifyingKey = match committee.key_of(&self.authority) {
            Some(k) => k,
            None => return false,
        };
        let msg = effect_message(&self.committee_id, &self.account, self.seq, &self.order_id);
        key.verify_strict(&msg, &self.signature).is_ok()
    }
}

/// An EffectCert: a quorum of distinct authorities each attesting they applied
/// `order_id` at `(account, seq)`. This is the FINALITY object — its existence, not a
/// bare tx-cert, is what makes an order final to a client.
#[derive(Clone, Debug)]
pub struct EffectCert {
    account: String,
    seq: u64,
    order_id: [u8; 32],
}

impl EffectCert {
    /// The finalized order digest.
    pub fn order_id(&self) -> [u8; 32] {
        self.order_id
    }

    /// Assemble an EffectCert iff at least `quorum` DISTINCT authorities validly
    /// attest they applied the SAME `order_id` at the SAME slot. Returns `None`
    /// otherwise — the client is not final.
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
            Some(Self { account: account.to_string(), seq, order_id })
        } else {
            None
        }
    }
}

/// **The migrated client finality predicate.** An order is final to a client iff a
/// valid [`EffectCert`] (quorum of apply-attestations) can be assembled from the
/// client's collected `attestations`. A bare transfer `Certificate` is NOT final.
pub fn is_final_effectcert(
    account: &str,
    seq: u64,
    order_id: [u8; 32],
    attestations: &[EffectAttestation],
    committee: &Committee,
) -> bool {
    EffectCert::assemble(account, seq, order_id, attestations, committee).is_some()
}

/// **The OLD, unsafe predicate** (kept only to demonstrate the fork it permits): a
/// bare verifying transfer `Certificate` treated as final. This is exactly what the
/// withholding attack exploits.
pub fn is_final_cert_exists(cert: &Certificate, committee: &Committee) -> bool {
    cert.verify(committee).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;
    use transfer333::{
        authority_signing_message, Authority, Certificate, Ledger, NetworkId, OwnerRegistry,
        SignedTransfer, SigningKey, Transfer, TransferPolicy, Vote,
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
        );
        Vote {
            authority: id,
            committee_id: committee.id(),
            policy_id: order.policy_id(),
            network_id: order.network_id.clone(),
            transfer: order.transfer.clone(),
            signature: key(idx).sign(&msg),
        }
    }

    /// Forge an authority's effect attestation (an authority applies + attests). Used
    /// to model who could attest — in the withholding attack only the equivocator,
    /// which holds the cert, could attest, and one attestation is below quorum.
    fn attest(idx: u8, committee: &Committee, account: &str, seq: u64, order_id: [u8; 32]) -> EffectAttestation {
        let id = format!("a{idx}");
        let cid = *committee.id().as_bytes();
        let msg = effect_message(&cid, account, seq, &order_id);
        EffectAttestation {
            authority: id,
            committee_id: cid,
            account: account.to_string(),
            seq,
            order_id,
            signature: key(idx).sign(&msg),
        }
    }

    // ⭐ THE BRICK — EffectCert converts the withholding client-fork into a safe stall.
    // Byzantine a0 shows order_a to honest but UNICASTS order_b only to the client, who
    // assembles cert_b = {a1,a2,a0}. Under cert-exists the client finalizes order_b
    // (UNSAFE — no honest node applied it). Under EffectCert the client needs a quorum
    // of apply-attestations; only a0 (which holds the cert) can attest -> 1 < quorum 3
    // -> NOT final. The premature finalization is withheld: no client-visible fork.
    #[test]
    fn effectcert_withholds_finality_that_cert_exists_would_prematurely_grant() {
        let (committee, mut auth, p) = setup(4);
        assert_eq!(committee.quorum(), 3);
        let order_a = order(&p, "alice", 0, "bob", 10);
        let order_b = order(&p, "alice", 0, "carol", 10);
        let _va1 = auth[1].handle(&order_a).unwrap(); // honest a1 -> a (its lock)
        let vb2 = auth[2].handle(&order_b).unwrap(); // honest a2 -> b
        // a1 also legitimately votes b? No: a1 locked a. The honest b-voters are a2, a3.
        let vb3 = auth[3].handle(&order_b).unwrap(); // honest a3 -> b
        let vb0 = forge_vote(0, &committee, &order_b); // equivocator a0's WITHHELD b-vote
        // cert_b is assembled AT THE CLIENT from the withheld equivocation vote:
        let cert_b = Certificate::assemble(order_b.clone(), vec![vb0, vb2, vb3], &committee)
            .expect("client assembles cert_b from a0(withheld)+a2+a3");

        // OLD predicate: cert-exists says FINAL (the unsafe over-finalization).
        assert!(
            is_final_cert_exists(&cert_b, &committee),
            "cert-exists prematurely finalizes the client's assembled-but-unapplied cert"
        );

        // NEW predicate: only authorities that APPLIED order_b can attest. Honest a2,a3
        // voted b but never received a0's completing vote, so they could not assemble
        // /apply cert_b -> they do NOT attest. Only the equivocator a0 (which holds the
        // cert) attests -> 1 attestation.
        let oid_b = order_b.order_id();
        let attestations = vec![attest(0, &committee, "alice", 0, oid_b)];
        assert!(
            !is_final_effectcert("alice", 0, oid_b, &attestations, &committee),
            "EffectCert withholds finality: 1 attestation < quorum 3 -> not final"
        );
        assert!(EffectCert::assemble("alice", 0, oid_b, &attestations, &committee).is_none());
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

    // EffectCert requires n-f (not 2f+1) at a NON-canonical committee. n=7,f=2: quorum
    // = n-f = 5. 3 attestations (= 2f+1, the UNSAFE bound n=4 hides) must NOT finalize;
    // 5 must. Guards the quorum hard core (C8) on the finality object too.
    #[test]
    fn effectcert_needs_n_minus_f_not_2f_plus_1_at_n7() {
        let (committee, _auth, p) = setup(7);
        assert_eq!(committee.quorum(), 5); // n-f = 7-2
        let o = order(&p, "alice", 0, "bob", 10);
        let oid = o.order_id();
        let three: Vec<_> = (0..3).map(|i| attest(i, &committee, "alice", 0, oid)).collect();
        assert!(
            !is_final_effectcert("alice", 0, oid, &three, &committee),
            "3 attestations (2f+1) must NOT finalize at n=7; only n-f=5 does"
        );
        let five: Vec<_> = (0..5).map(|i| attest(i, &committee, "alice", 0, oid)).collect();
        assert!(is_final_effectcert("alice", 0, oid, &five, &committee));
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
