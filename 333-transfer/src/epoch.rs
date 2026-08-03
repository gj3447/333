// KG: committee-reconfiguration design v1 (docs/design/2026-08-03-committee-reconfiguration.md)
// M1 — epoch-change wire types: EpochProposal / EpochVote / EpochCert.
//
// Epoch 0 is the static deployment committee; every later epoch is justified
// by a certificate of the previous one (no new trust root). The two-phase
// fence-then-change protocol lives in the authority (M2); this module holds
// only the value types, the canonical frontier digest, and the vote
// signing/verification preimage — the pieces the wire codec (M1) and the
// epoch FSM (M2) share.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::authority::{AuthorityId, CommitteeId};
use crate::owner::{NetworkId, PolicyId};
use crate::AccountId;

/// Hard decode cap for a frontier's account count. Frontiers are bounded by
/// the owner roster in practice; the cap exists so a corrupt frame cannot
/// drive an unbounded allocation (wire convention, see `take_len_bytes`).
pub const MAX_FRONTIER_ACCOUNTS: usize = 4096;

const FRONTIER_DIGEST_DOMAIN: &[u8] = b"transfer333/epoch-frontier/v1\0";
const EPOCH_VOTE_SIGNING_DOMAIN: &[u8] = b"transfer333/epoch-vote-signing/v1\0";

/// Canonical digest of a per-account next-seq frontier. Accounts are sorted
/// before hashing, so collection order never changes the digest — two
/// authorities that agree on state agree on this value bit-for-bit.
pub fn frontier_digest(frontier: &[(AccountId, u64)]) -> [u8; 32] {
    let mut sorted: Vec<(AccountId, u64)> = frontier.to_vec();
    sorted.sort();
    let mut digest = Sha256::new();
    digest.update(FRONTIER_DIGEST_DOMAIN);
    digest.update((sorted.len() as u64).to_le_bytes());
    for (account, next_seq) in &sorted {
        let bytes = account.as_bytes();
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
        digest.update(next_seq.to_le_bytes());
    }
    digest.finalize().into()
}

/// The exact preimage an [`EpochVote`] signs. Domain-tagged and complete —
/// authority, old committee (trust root), epoch, next committee, and the
/// frontier digest — so a signature cannot be transplanted onto any other
/// change, committee, or generation.
pub fn epoch_vote_signing_message(
    authority: &AuthorityId,
    committee_id: &CommitteeId,
    epoch: u64,
    next_committee_id: &CommitteeId,
    frontier_digest: &[u8; 32],
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(128 + authority.len());
    msg.extend_from_slice(EPOCH_VOTE_SIGNING_DOMAIN);
    msg.extend_from_slice(&(authority.len() as u64).to_le_bytes());
    msg.extend_from_slice(authority.as_bytes());
    msg.extend_from_slice(committee_id.as_bytes());
    msg.extend_from_slice(&epoch.to_le_bytes());
    msg.extend_from_slice(next_committee_id.as_bytes());
    msg.extend_from_slice(frontier_digest);
    msg
}

/// Operator-proposed next committee (design §4). Deliberately unsigned: the
/// proposal is only a suggestion to vote on — all safety lives in the votes
/// and the certificate they form, never in who asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpochProposal {
    pub network_id: NetworkId,
    pub policy_id: PolicyId,
    /// The generation being entered. Valid only as `current_epoch + 1`.
    pub epoch: u64,
    pub next_roster: Vec<(AuthorityId, VerifyingKey)>,
}

/// One authority's signed assent to an epoch change (design §4, tag 5).
///
/// `committee_id` is the OLD committee — the trust root this vote draws its
/// authority from. `next_committee_id` is the digest the `next_roster` in any
/// valid [`EpochCert`] must reproduce, so a certificate is self-authenticating
/// exactly like a transfer `Certificate`.
///
/// The vote carries the full `frontier` it digests: a reconfig collector has
/// no request/response channel to fetch it later, so the certificate can only
/// be assembled if the frontier travels with the vote. `frontier_digest`
/// stays in the signed preimage (cheap comparison) and every consumer checks
/// `frontier_digest(vote.frontier) == vote.frontier_digest` — self-consistency,
/// so the pair cannot disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpochVote {
    pub authority: AuthorityId,
    pub committee_id: CommitteeId,
    pub epoch: u64,
    pub next_committee_id: CommitteeId,
    pub frontier: Vec<(AccountId, u64)>,
    pub frontier_digest: [u8; 32],
    pub signature: Signature,
}

/// Why an [`EpochVote`] signature check failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpochVoteError {
    BadSignature,
    /// `frontier_digest(frontier) != frontier_digest` — the pair disagrees.
    FrontierMismatch,
}

impl EpochVote {
    /// Sign assent to `(epoch, next_committee_id, frontier)` under
    /// `committee_id`. The frontier is digested canonically (see
    /// [`frontier_digest`]) — call sites must pass their *actual* local
    /// frontier; the signature binds exactly that state.
    pub fn sign(
        authority: AuthorityId,
        committee_id: CommitteeId,
        epoch: u64,
        next_committee_id: CommitteeId,
        frontier: &[(AccountId, u64)],
        signing: &SigningKey,
    ) -> Self {
        let frontier_digest = frontier_digest(frontier);
        let msg = epoch_vote_signing_message(
            &authority,
            &committee_id,
            epoch,
            &next_committee_id,
            &frontier_digest,
        );
        let signature = signing.sign(&msg);
        Self {
            authority,
            committee_id,
            epoch,
            next_committee_id,
            frontier: frontier.to_vec(),
            frontier_digest,
            signature,
        }
    }

    /// Verify the signature against the authority's advertised key, plus the
    /// `frontier`/`frontier_digest` self-consistency (the pair must not
    /// disagree). Committee membership and quorum are NOT checked here — that
    /// is the certificate assembly's job (M2), mirroring `Vote`/`Certificate`.
    pub fn verify_signature(&self, key: &VerifyingKey) -> Result<(), EpochVoteError> {
        if frontier_digest(&self.frontier) != self.frontier_digest {
            return Err(EpochVoteError::FrontierMismatch);
        }
        let msg = epoch_vote_signing_message(
            &self.authority,
            &self.committee_id,
            self.epoch,
            &self.next_committee_id,
            &self.frontier_digest,
        );
        key.verify(&msg, &self.signature)
            .map_err(|_| EpochVoteError::BadSignature)
    }
}

/// Quorum evidence for an epoch change (design §4, tag 6).
///
/// Self-authenticating: `frontier_digest(frontier)` must equal every vote's
/// `frontier_digest`, and `next_roster` must digest to every vote's
/// `next_committee_id`. Quorum validity against the old committee is checked
/// at assembly/install time (M2), not at decode time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpochCert {
    pub epoch: u64,
    pub next_roster: Vec<(AuthorityId, VerifyingKey)>,
    pub frontier: Vec<(AccountId, u64)>,
    pub votes: Vec<EpochVote>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(i: u8) -> SigningKey {
        SigningKey::from_bytes(&[i; 32])
    }

    fn committee_id(seed: u8) -> CommitteeId {
        CommitteeId::from_bytes([seed; 32])
    }

    #[test]
    fn frontier_digest_is_order_independent_and_content_sensitive() {
        let a = vec![
            ("alice".to_string(), 3u64),
            ("bob".to_string(), 1u64),
            ("carol".to_string(), 7u64),
        ];
        let mut b = a.clone();
        b.reverse();
        assert_eq!(frontier_digest(&a), frontier_digest(&b));
        let c = vec![
            ("alice".to_string(), 4u64), // one next-seq differs
            ("bob".to_string(), 1u64),
            ("carol".to_string(), 7u64),
        ];
        assert_ne!(frontier_digest(&a), frontier_digest(&c));
        let d = vec![("alice".to_string(), 3u64), ("bob".to_string(), 1u64)];
        assert_ne!(frontier_digest(&a), frontier_digest(&d)); // membership differs
    }

    #[test]
    fn epoch_vote_sign_verify_roundtrip() {
        let frontier = vec![("alice".to_string(), 1u64)];
        let vote = EpochVote::sign(
            "a0".to_string(),
            committee_id(1),
            1,
            committee_id(2),
            &frontier,
            &key(7),
        );
        assert!(vote.verify_signature(&key(7).verifying_key()).is_ok());
        // Wrong key fails closed.
        assert!(vote.verify_signature(&key(8).verifying_key()).is_err());
        // A tampered vote fails closed (every field is inside the preimage).
        let mut forged = vote.clone();
        forged.epoch = 2;
        assert!(forged.verify_signature(&key(7).verifying_key()).is_err());
        let mut forged2 = vote.clone();
        forged2.frontier_digest = frontier_digest(&[("alice".to_string(), 99u64)]);
        assert!(forged2.verify_signature(&key(7).verifying_key()).is_err());
    }
}
