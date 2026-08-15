//! # slotseal333 — 333 coin equivocation-recovery, single-decree-per-slot leg (M0 scaffold)
//!
//! Recovery of an equivocation-locked `(account, seq)` slot needs a Byzantine
//! agreement leg: the contested-slot outcome is delivery-order dependent, so a
//! seal decided from a local snapshot races a straggler and breaks certificate
//! uniqueness (proved by `transfer333::authority::tests::
//! contested_slot_outcome_is_delivery_order_dependent_so_seal_needs_total_order`,
//! committed gj3447/333 @3436691).
//!
//! Per PROM `prom16-333-optionA-total-order-leg` (2026-07-21) the frontal fix is a
//! **single-decree, on-demand** Byzantine agreement that decides exactly one
//! [`SlotSeal`] per *provably contested* slot — fired ONLY on
//! `Certified::Failed{contested:true}`, adjudicating BOTH the straggler cert and
//! the seal in one decree, so uncontended single-owner transfers stay CN=1
//! (consensus is off the critical finality path for the common case).
//!
//! **M0 = scaffold only.** This crate defines the decree alphabet, the identity
//! bridge, and pins the quorum to `transfer333`'s `n-f` (never a grafted `2f+1`,
//! which is UNSAFE for non-`3f+1` committees — see the quorum test). No agreement
//! logic, no networking, no `certify()`/`confirm()` edits yet (those are M1–M4).
//!
//! Vehicle = **fresh-native**: typed over `transfer333` identities, zero
//! dependency on `333-platform` — so the platform engine's content-binding gap
//! (weak FNV `hash_block`, no receiver-side recompute) and its unsafe `2f+1`
//! quorum are structurally excluded rather than inherited.
//!
//! Degeneration guards honoured here: **G6** (single quorum source of truth =
//! `transfer333::authority::quorum`), and the scaffold for **G5** (a `Void` is a
//! first-class decree outcome, never a bare seal-latch; its Byzantine-aware
//! validity precondition lands in M2).
//!
//! KG: `prom16-333-optionA-total-order-leg`,
//! `design-333-coin-recovery-optionA-reframe-2026-07-21`.
//! The implementation is covered by the crate's direct invariants and regression tests.
//! `optionA-single-decree-ba-reframe` (predictions pending MCP restore).

pub mod agreement;
pub mod finality;

use identity333::NodeId;
use serde::{Deserialize, Serialize};
use transfer333::authority::AuthorityId;
use transfer333::{AccountId, VerifyingKey};

/// The recovery agreement's quorum is **exactly** `transfer333`'s `n-f`.
///
/// Re-exported as the single source of truth (degeneration guard **G6**). The
/// grafted engines' `2f+1` is UNSAFE for committees where `n != 3f+1` (e.g.
/// `n=5`: `2f+1=3`, honest intersection `2*3-5=1` which is NOT `> f=1`), so it
/// must never be used here. See [`tests::seal_quorum_preserves_honest_intersection_at_non_canonical_sizes`].
pub use transfer333::authority::quorum;

/// The one opaque value a single-decree agreement decides per contested slot.
///
/// Keyed to a specific committee so a decree can never be replayed against a
/// different roster. The decree is *ephemeral*: one instance per
/// `(committee_id, account, seq)`, torn down at Decide.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SlotSeal {
    /// The account whose `(account, seq)` slot was contested.
    pub account: AccountId,
    /// The owner sequence number of the contested slot.
    pub seq: u64,
    /// Binds the decree to the exact committee roster that adjudicated it.
    pub committee_id: [u8; 32],
    /// What the committee agreed happens to the slot.
    pub outcome: SealOutcome,
}

/// The agreed fate of a contested slot.
///
/// Exactly one of these is decided per slot, making "which happened first" a
/// single globally-agreed fact instead of a per-node race between universes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SealOutcome {
    /// A competing certificate won: the slot finalizes this order. `order_id` is
    /// the digest of the winning [`transfer333`] order; the client re-finalizes a
    /// provisional cert iff its order digest equals this.
    Finalize {
        /// Digest of the winning order (`SignedTransfer::order_id`).
        order_id: [u8; 32],
    },
    /// No certificate could form: the slot is voided and the seq is poisoned
    /// (the owner resubmits at `seq + 1`).
    ///
    /// **Legal ONLY under the Byzantine-aware evidence-completeness precondition**
    /// `for every candidate order C: honest_votes(C) + honest_unvoted <= f`.
    /// The naive `v_max + outstanding < quorum` rule is UNSAFE (it ignores the
    /// equivocation capacity of already-voted Byzantine authorities). A formable
    /// or formed certificate is NEVER voided. This precondition is *enforced* in
    /// M2 (`void-byzantine` oracle); M0 only declares the outcome variant.
    Void,
}

impl SlotSeal {
    /// Construct a `Finalize` decree for `order_id` on the given slot.
    pub fn finalize(
        account: impl Into<AccountId>,
        seq: u64,
        committee_id: [u8; 32],
        order_id: [u8; 32],
    ) -> Self {
        Self {
            account: account.into(),
            seq,
            committee_id,
            outcome: SealOutcome::Finalize { order_id },
        }
    }

    /// Construct a `Void` decree for the given slot. The caller is responsible
    /// for the Byzantine-aware validity precondition (enforced in M2).
    pub fn void(account: impl Into<AccountId>, seq: u64, committee_id: [u8; 32]) -> Self {
        Self {
            account: account.into(),
            seq,
            committee_id,
            outcome: SealOutcome::Void,
        }
    }
}

/// Losslessly bridge a FastPay authority's Ed25519 verifying key to an
/// `identity333::NodeId`.
///
/// The 32 verifying-key bytes ARE the node id — no `u32` index and no adapter
/// table (unlike `333-platform`'s `NodeId = u32` + `ed25519-compact`, the odd one
/// out). `transfer333`, `identity333`, and `consensus333` already agree on
/// `ed25519_dalek` keys and 32-byte identities, so the agreement leg reuses ONE
/// committee/key set.
pub fn authority_node_id(vk: &VerifyingKey) -> NodeId {
    NodeId::from_bytes(vk.to_bytes())
}

/// Type-level assertion that the FastPay authority identity is a `String` alias,
/// documenting the roster the agreement leg must key its votes to. (No-op at
/// runtime; keeps the `AuthorityId` import load-bearing so a future type change
/// surfaces here.)
pub fn authority_id_hint(id: &AuthorityId) -> &str {
    id.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **C8 / G6 falsifier.** The recovery agreement quorum MUST preserve honest
    /// intersection for NON-canonical committee sizes (`n != 3f+1`), where a
    /// `2f+1` quorum is UNSAFE. We reuse `transfer333`'s `n-f` and assert the
    /// intersection invariant `2*quorum(n) - n > f` across a matrix that includes
    /// non-`3f+1` sizes (n=5,6,8,9), AND that we are provably NOT using `2f+1`
    /// there. A single failure = a safety regression the wire would introduce.
    #[test]
    fn seal_quorum_preserves_honest_intersection_at_non_canonical_sizes() {
        let mut saw_non_canonical = false;
        for n in 4..=9usize {
            let f = (n - 1) / 3;
            let q = quorum(n);
            // n-f used (single source of truth = transfer333):
            assert_eq!(q, n - f, "quorum must be n-f at n={n}");
            // honest intersection strictly exceeds f (two quorums share an honest node):
            assert!(
                2 * q as isize - n as isize > f as isize,
                "honest intersection must exceed f at n={n} (q={q}, f={f})"
            );
            // at non-3f+1 sizes the UNSAFE 2f+1 is provably NOT what we use:
            if n != 3 * f + 1 {
                saw_non_canonical = true;
                assert_ne!(
                    q,
                    2 * f + 1,
                    "must not use the unsafe 2f+1 quorum at non-3f+1 n={n}"
                );
                // demonstrate WHY 2f+1 is unsafe here: its intersection collapses to <= f.
                assert!(
                    2 * (2 * f + 1) as isize - n as isize <= f as isize,
                    "sanity: 2f+1 intersection should collapse at non-3f+1 n={n}"
                );
            }
        }
        assert!(saw_non_canonical, "matrix must include non-3f+1 committee sizes");
    }

    #[test]
    fn authority_node_id_is_exactly_the_verifying_key_bytes() {
        let sk = transfer333::SigningKey::from_bytes(&[7u8; 32]);
        let vk = sk.verifying_key();
        assert_eq!(authority_node_id(&vk), NodeId::from_bytes(vk.to_bytes()));
    }

    #[test]
    fn finalize_and_void_are_distinct_decrees_for_the_same_slot() {
        let cid = [9u8; 32];
        let fin = SlotSeal::finalize("alice", 0, cid, [1u8; 32]);
        let void = SlotSeal::void("alice", 0, cid);
        assert_ne!(fin, void);
        assert_eq!(fin.account, void.account);
        assert_eq!(fin.seq, void.seq);
        assert!(matches!(fin.outcome, SealOutcome::Finalize { .. }));
        assert!(matches!(void.outcome, SealOutcome::Void));
    }
}
