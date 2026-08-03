// KG: transport-plan Steps 1–3, 8 (2026-07-14)
//
// Domain-separated, length-prefixed wire codec for authority-path messages.
// Mirrors `signing_message` style (domain tag + u64 LE length prefixes) so
// Transfer / Vote / Certificate / EffectAttestation can leave the process
// without reinventing QC.
//
// Step 8: `AuthorityMsg` framing = 1-byte tag + domain-tagged body. TCP uses
// this payload under a u32-BE length prefix (see `tcp` module / 333-wire style).

use ed25519_dalek::Signature;

use crate::authority::{
    Certificate, CommitteeId, Vote, MAX_AUTHORITY_ID_BYTES, MAX_COMMITTEE_MEMBERS,
};
use crate::effect::EffectAttestation;
use crate::epoch::{EpochCert, EpochProposal, EpochVote, MAX_FRONTIER_ACCOUNTS};
use crate::owner::{
    NetworkId, PolicyId, SignedTransfer, MAX_ACCOUNT_ID_BYTES, MAX_NETWORK_ID_BYTES,
};
use crate::{AccountId, Transfer};

const DOMAIN_TRANSFER: &[u8] = b"transfer333/wire-signed-order/v3\0";
const DOMAIN_VOTE: &[u8] = b"transfer333/wire-vote/v3\0";
const DOMAIN_CERT: &[u8] = b"transfer333/wire-cert/v3\0";
// NEW domain for the M3 effect attestation — never a reuse of a v3 tag for a
// different layout.
const DOMAIN_ATTESTATION: &[u8] = b"transfer333/wire-effect-attestation/v1\0";
// Epoch-change domains (committee-reconfiguration M1) — fresh v1 domains, no
// reuse of any existing tag or layout.
const DOMAIN_EPOCH_PROPOSAL: &[u8] = b"transfer333/wire-epoch-proposal/v1\0";
const DOMAIN_EPOCH_VOTE: &[u8] = b"transfer333/wire-epoch-vote/v1\0";
const DOMAIN_EPOCH_CERT: &[u8] = b"transfer333/wire-epoch-cert/v1\0";
pub const MAX_CERTIFICATE_VOTES: usize = MAX_COMMITTEE_MEMBERS;

/// Wire tag for [`AuthorityMsg`] over TCP / framed streams.
pub const TAG_ORDER: u8 = 0;
pub const TAG_VOTE: u8 = 1;
pub const TAG_CERT: u8 = 2;
pub const TAG_ATTESTATION: u8 = 3;
pub const TAG_EPOCH_PROPOSAL: u8 = 4;
pub const TAG_EPOCH_VOTE: u8 = 5;
pub const TAG_EPOCH_CERT: u8 = 6;

/// Messages that travel the authority mesh (orders, signed votes, certificates,
/// effect attestations, epoch-change proposal/vote/cert). Shared by the
/// in-memory mesh and the TCP backend.
#[derive(Debug, Clone)]
pub enum AuthorityMsg {
    Order(SignedTransfer),
    Vote(Vote),
    Cert(Certificate),
    Attestation(EffectAttestation),
    EpochProposal(EpochProposal),
    EpochVote(EpochVote),
    EpochCert(EpochCert),
}

/// Why a byte stream could not be decoded into an authority message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    Truncated,
    BadDomain,
    BadUtf8,
    BadNetworkId,
    BadSignature,
    LengthOverflow { field: &'static str, length: u64 },
    FieldTooLong {
        field: &'static str,
        length: u64,
        max: usize,
    },
    TooManyVotes { count: u64, max: usize },
    /// Unknown `AuthorityMsg` tag (not Order/Vote/Cert/Attestation).
    UnknownTag(u8),
}

// --- low-level helpers -------------------------------------------------------

fn put_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    buf.extend_from_slice(bytes);
}

fn put_str(buf: &mut Vec<u8>, s: &str) {
    put_bytes(buf, s.as_bytes());
}

/// Unsigned transfer payload without a domain tag. The owner re-vote round
/// follows `from_seq`, matching the signing preimage layout.
fn put_transfer_body(buf: &mut Vec<u8>, t: &Transfer, round: u64) {
    put_str(buf, &t.from);
    buf.extend_from_slice(&t.from_seq.to_le_bytes());
    buf.extend_from_slice(&round.to_le_bytes());
    put_str(buf, &t.to);
    buf.extend_from_slice(&t.amount.to_le_bytes());
}

fn put_signed_transfer_body(buf: &mut Vec<u8>, order: &SignedTransfer) {
    buf.extend_from_slice(order.policy_id().as_bytes());
    put_str(buf, order.network_id.as_str());
    put_transfer_body(buf, &order.transfer, order.round);
    buf.extend_from_slice(&order.owner_signature().to_bytes());
}

fn take<'a>(input: &mut &'a [u8], n: usize) -> Result<&'a [u8], WireError> {
    if input.len() < n {
        return Err(WireError::Truncated);
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Ok(head)
}

fn take_u64(input: &mut &[u8]) -> Result<u64, WireError> {
    let b = take(input, 8)?;
    Ok(u64::from_le_bytes(b.try_into().expect("8 bytes")))
}

fn take_u128(input: &mut &[u8]) -> Result<u128, WireError> {
    let b = take(input, 16)?;
    Ok(u128::from_le_bytes(b.try_into().expect("16 bytes")))
}

fn take_len_bytes<'a>(
    input: &mut &'a [u8],
    field: &'static str,
    max: usize,
) -> Result<&'a [u8], WireError> {
    let length = take_u64(input)?;
    let n = usize::try_from(length)
        .map_err(|_| WireError::LengthOverflow { field, length })?;
    if n > max {
        return Err(WireError::FieldTooLong {
            field,
            length,
            max,
        });
    }
    take(input, n)
}

fn take_str(
    input: &mut &[u8],
    field: &'static str,
    max: usize,
) -> Result<String, WireError> {
    // The length prefix is converted and bounded before UTF-8 validation or
    // `to_owned`, so an attacker cannot drive an unbounded String allocation.
    let b = take_len_bytes(input, field, max)?;
    std::str::from_utf8(b)
        .map(|s| s.to_owned())
        .map_err(|_| WireError::BadUtf8)
}

fn take_transfer_body(input: &mut &[u8]) -> Result<(Transfer, u64), WireError> {
    let from = take_str(input, "transfer.from", MAX_ACCOUNT_ID_BYTES)?;
    let from_seq = take_u64(input)?;
    let round = take_u64(input)?;
    let to = take_str(input, "transfer.to", MAX_ACCOUNT_ID_BYTES)?;
    let amount = take_u128(input)?;
    Ok((
        Transfer {
            from,
            from_seq,
            to,
            amount,
        },
        round,
    ))
}

fn take_network_id(input: &mut &[u8]) -> Result<NetworkId, WireError> {
    NetworkId::new(take_str(
        input,
        "network_id",
        MAX_NETWORK_ID_BYTES,
    )?)
    .map_err(|_| WireError::BadNetworkId)
}

fn take_policy_id(input: &mut &[u8]) -> Result<PolicyId, WireError> {
    let bytes: [u8; 32] = take(input, 32)?
        .try_into()
        .expect("take returned exactly 32 policy-id bytes");
    Ok(PolicyId::from_bytes(bytes))
}

fn take_committee_id(input: &mut &[u8]) -> Result<CommitteeId, WireError> {
    let bytes: [u8; 32] = take(input, 32)?
        .try_into()
        .expect("take returned exactly 32 committee-id bytes");
    Ok(CommitteeId::from_bytes(bytes))
}

fn take_signed_transfer_body(input: &mut &[u8]) -> Result<SignedTransfer, WireError> {
    let policy_id = take_policy_id(input)?;
    let network_id = take_network_id(input)?;
    let (transfer, round) = take_transfer_body(input)?;
    let owner_signature = take_signature(input)?;
    Ok(SignedTransfer::from_parts_at_round(
        network_id,
        policy_id,
        transfer,
        round,
        owner_signature,
    ))
}

fn take_signature(input: &mut &[u8]) -> Result<Signature, WireError> {
    let b = take(input, 64)?;
    let arr: [u8; 64] = b.try_into().map_err(|_| WireError::BadSignature)?;
    Ok(Signature::from_bytes(&arr))
}

fn expect_domain(input: &mut &[u8], domain: &[u8]) -> Result<(), WireError> {
    let got = take(input, domain.len())?;
    if got != domain {
        return Err(WireError::BadDomain);
    }
    Ok(())
}

fn put_vote_body(buf: &mut Vec<u8>, v: &Vote) {
    put_str(buf, &v.authority);
    buf.extend_from_slice(v.committee_id.as_bytes());
    buf.extend_from_slice(v.policy_id.as_bytes());
    put_str(buf, v.network_id.as_str());
    put_transfer_body(buf, &v.transfer, v.round);
    buf.extend_from_slice(&v.signature.to_bytes());
}

fn take_vote_body(input: &mut &[u8]) -> Result<Vote, WireError> {
    let authority = take_str(input, "vote.authority", MAX_AUTHORITY_ID_BYTES)?;
    let committee_id = take_committee_id(input)?;
    let policy_id = take_policy_id(input)?;
    let network_id = take_network_id(input)?;
    let (transfer, round) = take_transfer_body(input)?;
    let signature = take_signature(input)?;
    Ok(Vote {
        authority,
        committee_id,
        policy_id,
        network_id,
        transfer,
        round,
        signature,
    })
}

// --- public encode / decode --------------------------------------------------

/// Encode an owner-authenticated order with the fail-closed v3 domain.
pub fn encode_transfer(order: &SignedTransfer) -> Vec<u8> {
    let t = &order.transfer;
    let mut buf = Vec::with_capacity(
        128 + order.network_id.as_str().len() + t.from.len() + t.to.len(),
    );
    buf.extend_from_slice(DOMAIN_TRANSFER);
    put_signed_transfer_body(&mut buf, order);
    buf
}

/// Decode a v3 owner-authenticated order. Unsigned v1 and pre-round v2 domains
/// fail closed.
pub fn decode_transfer(bytes: &[u8]) -> Result<SignedTransfer, WireError> {
    let mut input = bytes;
    expect_domain(&mut input, DOMAIN_TRANSFER)?;
    let order = take_signed_transfer_body(&mut input)?;
    if !input.is_empty() {
        return Err(WireError::Truncated);
    }
    Ok(order)
}

/// Encode a signed `Vote` (authority id + transfer + 64-byte Ed25519 sig).
pub fn encode_vote(v: &Vote) -> Vec<u8> {
    let mut buf = Vec::with_capacity(
        160
            + v.authority.len()
            + v.network_id.as_str().len()
            + v.transfer.from.len()
            + v.transfer.to.len(),
    );
    buf.extend_from_slice(DOMAIN_VOTE);
    put_vote_body(&mut buf, v);
    buf
}

/// Decode a domain-tagged `Vote`.
pub fn decode_vote(bytes: &[u8]) -> Result<Vote, WireError> {
    let mut input = bytes;
    expect_domain(&mut input, DOMAIN_VOTE)?;
    let v = take_vote_body(&mut input)?;
    if !input.is_empty() {
        return Err(WireError::Truncated);
    }
    Ok(v)
}

/// Encode a quorum `Certificate` (owner-signed order + full vote list).
pub fn encode_certificate(c: &Certificate) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128 + c.votes.len() * 128);
    buf.extend_from_slice(DOMAIN_CERT);
    put_signed_transfer_body(&mut buf, &c.order);
    buf.extend_from_slice(&(c.votes.len() as u64).to_le_bytes());
    for v in &c.votes {
        put_vote_body(&mut buf, v);
    }
    buf
}

/// Decode a domain-tagged `Certificate`. Validity is NOT checked here —
/// callers must still run `Certificate::is_valid` / `verify`.
pub fn decode_certificate(bytes: &[u8]) -> Result<Certificate, WireError> {
    let mut input = bytes;
    expect_domain(&mut input, DOMAIN_CERT)?;
    let order = take_signed_transfer_body(&mut input)?;
    let count = take_u64(&mut input)?;
    if count > MAX_CERTIFICATE_VOTES as u64 {
        return Err(WireError::TooManyVotes {
            count,
            max: MAX_CERTIFICATE_VOTES,
        });
    }
    let n = usize::try_from(count).map_err(|_| WireError::TooManyVotes {
        count,
        max: MAX_CERTIFICATE_VOTES,
    })?;
    let mut votes = Vec::with_capacity(n);
    for _ in 0..n {
        votes.push(take_vote_body(&mut input)?);
    }
    if !input.is_empty() {
        return Err(WireError::Truncated);
    }
    Ok(Certificate { order, votes })
}

// --- effect attestation codec (M3) ---------------------------------------------

fn put_attestation_body(buf: &mut Vec<u8>, a: &EffectAttestation) {
    put_str(buf, &a.authority);
    buf.extend_from_slice(a.committee_id.as_bytes());
    put_str(buf, &a.account);
    buf.extend_from_slice(&a.seq.to_le_bytes());
    buf.extend_from_slice(&a.order_id);
    buf.extend_from_slice(&a.signature.to_bytes());
}

fn take_attestation_body(input: &mut &[u8]) -> Result<EffectAttestation, WireError> {
    let authority = take_str(input, "attestation.authority", MAX_AUTHORITY_ID_BYTES)?;
    let committee_id = take_committee_id(input)?;
    let account = take_str(input, "attestation.account", MAX_ACCOUNT_ID_BYTES)?;
    let seq = take_u64(input)?;
    let order_id: [u8; 32] = take(input, 32)?
        .try_into()
        .expect("take returned exactly 32 order-id bytes");
    let signature = take_signature(input)?;
    Ok(EffectAttestation {
        authority,
        committee_id,
        account,
        seq,
        order_id,
        signature,
    })
}

/// Encode a signed `EffectAttestation` (authority + slot binding + 64-byte
/// Ed25519 sig) under the fresh v1 attestation domain.
pub fn encode_attestation(a: &EffectAttestation) -> Vec<u8> {
    let mut buf = Vec::with_capacity(160 + a.authority.len() + a.account.len());
    buf.extend_from_slice(DOMAIN_ATTESTATION);
    put_attestation_body(&mut buf, a);
    buf
}

/// Decode a domain-tagged `EffectAttestation`. Validity is NOT checked here —
/// callers must still run `EffectAttestation::is_valid`.
pub fn decode_attestation(bytes: &[u8]) -> Result<EffectAttestation, WireError> {
    let mut input = bytes;
    expect_domain(&mut input, DOMAIN_ATTESTATION)?;
    let a = take_attestation_body(&mut input)?;
    if !input.is_empty() {
        return Err(WireError::Truncated);
    }
    Ok(a)
}

// --- epoch-change codec (committee-reconfiguration M1) -------------------------

fn put_roster(buf: &mut Vec<u8>, roster: &[(crate::authority::AuthorityId, ed25519_dalek::VerifyingKey)]) {
    buf.extend_from_slice(&(roster.len() as u64).to_le_bytes());
    for (authority, key) in roster {
        put_str(buf, authority);
        buf.extend_from_slice(key.as_bytes());
    }
}

fn take_roster(
    input: &mut &[u8],
) -> Result<Vec<(crate::authority::AuthorityId, ed25519_dalek::VerifyingKey)>, WireError> {
    let count = take_u64(input)?;
    if count > MAX_COMMITTEE_MEMBERS as u64 {
        return Err(WireError::TooManyVotes {
            count,
            max: MAX_COMMITTEE_MEMBERS,
        });
    }
    let mut roster = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let authority = take_str(input, "epoch.authority", MAX_AUTHORITY_ID_BYTES)?;
        let key_bytes: [u8; 32] = take(input, 32)?
            .try_into()
            .expect("take returned exactly 32 verifying-key bytes");
        let key = ed25519_dalek::VerifyingKey::from_bytes(&key_bytes)
            .map_err(|_| WireError::BadSignature)?;
        roster.push((authority, key));
    }
    Ok(roster)
}

fn put_frontier(buf: &mut Vec<u8>, frontier: &[(AccountId, u64)]) {
    buf.extend_from_slice(&(frontier.len() as u64).to_le_bytes());
    for (account, next_seq) in frontier {
        put_str(buf, account);
        buf.extend_from_slice(&next_seq.to_le_bytes());
    }
}

fn take_frontier(input: &mut &[u8]) -> Result<Vec<(AccountId, u64)>, WireError> {
    let count = take_u64(input)?;
    if count > MAX_FRONTIER_ACCOUNTS as u64 {
        return Err(WireError::FieldTooLong {
            field: "epoch.frontier",
            length: count,
            max: MAX_FRONTIER_ACCOUNTS,
        });
    }
    let mut frontier = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let account = take_str(input, "epoch.frontier.account", MAX_ACCOUNT_ID_BYTES)?;
        let next_seq = take_u64(input)?;
        frontier.push((account, next_seq));
    }
    Ok(frontier)
}

fn put_epoch_proposal_body(buf: &mut Vec<u8>, p: &EpochProposal) {
    buf.extend_from_slice(p.policy_id.as_bytes());
    put_str(buf, p.network_id.as_str());
    buf.extend_from_slice(&p.epoch.to_le_bytes());
    put_roster(buf, &p.next_roster);
}

fn take_epoch_proposal_body(input: &mut &[u8]) -> Result<EpochProposal, WireError> {
    let policy_id = take_policy_id(input)?;
    let network_id = take_network_id(input)?;
    let epoch = take_u64(input)?;
    let next_roster = take_roster(input)?;
    Ok(EpochProposal {
        network_id,
        policy_id,
        epoch,
        next_roster,
    })
}

fn put_epoch_vote_body(buf: &mut Vec<u8>, v: &EpochVote) {
    put_str(buf, &v.authority);
    buf.extend_from_slice(v.committee_id.as_bytes());
    buf.extend_from_slice(&v.epoch.to_le_bytes());
    buf.extend_from_slice(v.next_committee_id.as_bytes());
    put_frontier(buf, &v.frontier);
    buf.extend_from_slice(&v.frontier_digest);
    buf.extend_from_slice(&v.signature.to_bytes());
}

fn take_epoch_vote_body(input: &mut &[u8]) -> Result<EpochVote, WireError> {
    let authority = take_str(input, "epoch_vote.authority", MAX_AUTHORITY_ID_BYTES)?;
    let committee_id = take_committee_id(input)?;
    let epoch = take_u64(input)?;
    let next_committee_id = take_committee_id(input)?;
    let frontier = take_frontier(input)?;
    let frontier_digest: [u8; 32] = take(input, 32)?
        .try_into()
        .expect("take returned exactly 32 frontier-digest bytes");
    let signature = take_signature(input)?;
    Ok(EpochVote {
        authority,
        committee_id,
        epoch,
        next_committee_id,
        frontier,
        frontier_digest,
        signature,
    })
}

/// Encode an operator `EpochProposal` under the fresh v1 domain.
pub fn encode_epoch_proposal(p: &EpochProposal) -> Vec<u8> {
    let mut buf = Vec::with_capacity(192 + p.next_roster.len() * 48);
    buf.extend_from_slice(DOMAIN_EPOCH_PROPOSAL);
    put_epoch_proposal_body(&mut buf, p);
    buf
}

/// Decode a domain-tagged `EpochProposal`.
pub fn decode_epoch_proposal(bytes: &[u8]) -> Result<EpochProposal, WireError> {
    let mut input = bytes;
    expect_domain(&mut input, DOMAIN_EPOCH_PROPOSAL)?;
    let p = take_epoch_proposal_body(&mut input)?;
    if !input.is_empty() {
        return Err(WireError::Truncated);
    }
    Ok(p)
}

/// Encode a signed `EpochVote`.
pub fn encode_epoch_vote(v: &EpochVote) -> Vec<u8> {
    let mut buf = Vec::with_capacity(200 + v.authority.len());
    buf.extend_from_slice(DOMAIN_EPOCH_VOTE);
    put_epoch_vote_body(&mut buf, v);
    buf
}

/// Decode a domain-tagged `EpochVote`. Signature validity is NOT checked
/// here — callers must still run `EpochVote::verify_signature`.
pub fn decode_epoch_vote(bytes: &[u8]) -> Result<EpochVote, WireError> {
    let mut input = bytes;
    expect_domain(&mut input, DOMAIN_EPOCH_VOTE)?;
    let v = take_epoch_vote_body(&mut input)?;
    if !input.is_empty() {
        return Err(WireError::Truncated);
    }
    Ok(v)
}

/// Encode an `EpochCert` (roster + frontier + full epoch-vote list).
pub fn encode_epoch_cert(c: &EpochCert) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256 + c.votes.len() * 192);
    buf.extend_from_slice(DOMAIN_EPOCH_CERT);
    buf.extend_from_slice(&c.epoch.to_le_bytes());
    put_roster(&mut buf, &c.next_roster);
    put_frontier(&mut buf, &c.frontier);
    buf.extend_from_slice(&(c.votes.len() as u64).to_le_bytes());
    for v in &c.votes {
        put_epoch_vote_body(&mut buf, v);
    }
    buf
}

/// Decode a domain-tagged `EpochCert`. Quorum validity is NOT checked here —
/// that is the install path's job (M2).
pub fn decode_epoch_cert(bytes: &[u8]) -> Result<EpochCert, WireError> {
    let mut input = bytes;
    expect_domain(&mut input, DOMAIN_EPOCH_CERT)?;
    let epoch = take_u64(&mut input)?;
    let next_roster = take_roster(&mut input)?;
    let frontier = take_frontier(&mut input)?;
    let count = take_u64(&mut input)?;
    if count > MAX_COMMITTEE_MEMBERS as u64 {
        return Err(WireError::TooManyVotes {
            count,
            max: MAX_COMMITTEE_MEMBERS,
        });
    }
    let mut votes = Vec::with_capacity(count as usize);
    for _ in 0..count {
        votes.push(take_epoch_vote_body(&mut input)?);
    }
    if !input.is_empty() {
        return Err(WireError::Truncated);
    }
    Ok(EpochCert {
        epoch,
        next_roster,
        frontier,
        votes,
    })
}

// --- AuthorityMsg frame (1-byte tag + domain body) ---------------------------

/// Encode an [`AuthorityMsg`] as `tag || domain-tagged-body`.
/// TCP layers a u32 big-endian length prefix around this blob (see `tcp`).
pub fn encode_authority_msg(msg: &AuthorityMsg) -> Vec<u8> {
    match msg {
        AuthorityMsg::Order(t) => {
            let body = encode_transfer(t);
            let mut out = Vec::with_capacity(1 + body.len());
            out.push(TAG_ORDER);
            out.extend_from_slice(&body);
            out
        }
        AuthorityMsg::Vote(v) => {
            let body = encode_vote(v);
            let mut out = Vec::with_capacity(1 + body.len());
            out.push(TAG_VOTE);
            out.extend_from_slice(&body);
            out
        }
        AuthorityMsg::Cert(c) => {
            let body = encode_certificate(c);
            let mut out = Vec::with_capacity(1 + body.len());
            out.push(TAG_CERT);
            out.extend_from_slice(&body);
            out
        }
        AuthorityMsg::Attestation(a) => {
            let body = encode_attestation(a);
            let mut out = Vec::with_capacity(1 + body.len());
            out.push(TAG_ATTESTATION);
            out.extend_from_slice(&body);
            out
        }
        AuthorityMsg::EpochProposal(p) => {
            let body = encode_epoch_proposal(p);
            let mut out = Vec::with_capacity(1 + body.len());
            out.push(TAG_EPOCH_PROPOSAL);
            out.extend_from_slice(&body);
            out
        }
        AuthorityMsg::EpochVote(v) => {
            let body = encode_epoch_vote(v);
            let mut out = Vec::with_capacity(1 + body.len());
            out.push(TAG_EPOCH_VOTE);
            out.extend_from_slice(&body);
            out
        }
        AuthorityMsg::EpochCert(c) => {
            let body = encode_epoch_cert(c);
            let mut out = Vec::with_capacity(1 + body.len());
            out.push(TAG_EPOCH_CERT);
            out.extend_from_slice(&body);
            out
        }
    }
}

/// Decode an [`AuthorityMsg`] frame (`tag || domain-tagged-body`).
pub fn decode_authority_msg(bytes: &[u8]) -> Result<AuthorityMsg, WireError> {
    if bytes.is_empty() {
        return Err(WireError::Truncated);
    }
    let tag = bytes[0];
    let body = &bytes[1..];
    match tag {
        TAG_ORDER => Ok(AuthorityMsg::Order(decode_transfer(body)?)),
        TAG_VOTE => Ok(AuthorityMsg::Vote(decode_vote(body)?)),
        TAG_CERT => Ok(AuthorityMsg::Cert(decode_certificate(body)?)),
        TAG_ATTESTATION => Ok(AuthorityMsg::Attestation(decode_attestation(body)?)),
        TAG_EPOCH_PROPOSAL => Ok(AuthorityMsg::EpochProposal(decode_epoch_proposal(body)?)),
        TAG_EPOCH_VOTE => Ok(AuthorityMsg::EpochVote(decode_epoch_vote(body)?)),
        TAG_EPOCH_CERT => Ok(AuthorityMsg::EpochCert(decode_epoch_cert(body)?)),
        other => Err(WireError::UnknownTag(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{Authority, Certificate, Committee};
    use crate::{Ledger, OwnerAuthError, OwnerRegistry, TransferPolicy};
    use ed25519_dalek::SigningKey;

    fn tx(from: &str, seq: u64, to: &str, amount: u128) -> Transfer {
        Transfer {
            from: from.into(),
            from_seq: seq,
            to: to.into(),
            amount,
        }
    }

    fn key(i: u8) -> SigningKey {
        SigningKey::from_bytes(&[i; 32])
    }

    fn policy() -> TransferPolicy {
        TransferPolicy::new(
            NetworkId::new("wire-testnet").unwrap(),
            OwnerRegistry::new([
                ("alice", key(42).verifying_key()),
                ("bob", key(43).verifying_key()),
                ("carol", key(44).verifying_key()),
                ("a", key(45).verifying_key()),
                ("b", key(46).verifying_key()),
            ])
            .unwrap(),
        )
    }

    fn signed_tx(
        policy: &TransferPolicy,
        from: &str,
        owner_seed: u8,
        seq: u64,
        to: &str,
        amount: u128,
    ) -> SignedTransfer {
        SignedTransfer::sign(policy, tx(from, seq, to, amount), &key(owner_seed))
    }

    fn setup(n: u8) -> (Committee, Vec<Authority>, TransferPolicy) {
        let policy = policy();
        let committee = Committee::new(
            (0..n).map(|i| (format!("a{i}"), key(i).verifying_key())),
            policy.clone(),
        )
        .unwrap();
        let genesis = Ledger::genesis([
            ("alice".into(), 100),
            ("bob".into(), 0),
            ("carol".into(), 0),
            ("a".into(), 0),
            ("b".into(), 0),
        ]);
        let auth: Vec<Authority> = (0..n)
            .map(|i| {
                Authority::new(
                    format!("a{i}"),
                    key(i),
                    policy.clone(),
                    committee.id(),
                    genesis.clone(),
                )
            })
            .collect();
        (committee, auth, policy)
    }

    #[test]
    fn transfer_round_trip() {
        let policy = policy();
        let order = signed_tx(&policy, "alice", 42, 7, "bob", 42);
        let bytes = encode_transfer(&order);
        let back = decode_transfer(&bytes).unwrap();
        assert_eq!(back, order);
        assert_eq!(back.policy_id(), policy.id());
        assert_eq!(back.verify(&policy), Ok(()));
    }

    #[test]
    fn vote_round_trip() {
        let (_committee, mut authorities, policy) = setup(1);
        let order = signed_tx(&policy, "alice", 42, 0, "bob", 10);
        let v = authorities[0].handle(&order).unwrap();
        let bytes = encode_vote(&v);
        let back = decode_vote(&bytes).unwrap();
        assert_eq!(back, v);
        assert_eq!(back.committee_id, v.committee_id);
        assert_eq!(back.policy_id, policy.id());
    }

    #[test]
    fn tampered_order_policy_id_round_trips_but_fails_policy_validation() {
        let policy = policy();
        let order = signed_tx(&policy, "alice", 42, 0, "bob", 10);
        let mut bytes = encode_transfer(&order);
        bytes[DOMAIN_TRANSFER.len()] ^= 0x01;

        let tampered = decode_transfer(&bytes).expect("policy id remains structurally valid");
        assert_ne!(tampered.policy_id(), policy.id());
        assert!(matches!(
            tampered.verify(&policy),
            Err(OwnerAuthError::WrongPolicy { .. })
        ));
    }

    #[test]
    fn tampered_vote_policy_or_committee_id_cannot_validate_for_order() {
        let (committee, mut authorities, policy) = setup(4);
        let order = signed_tx(&policy, "alice", 42, 0, "bob", 10);
        let vote = authorities[0].handle(&order).unwrap();
        let id_offset = DOMAIN_VOTE.len() + 8 + vote.authority.len();

        let mut wrong_committee = encode_vote(&vote);
        wrong_committee[id_offset] ^= 0x01;
        let wrong_committee = decode_vote(&wrong_committee).unwrap();
        assert_ne!(wrong_committee.committee_id, vote.committee_id);
        assert!(wrong_committee.validate_for(&order, &committee).is_err());

        let mut wrong_policy = encode_vote(&vote);
        wrong_policy[id_offset + 32] ^= 0x01;
        let wrong_policy = decode_vote(&wrong_policy).unwrap();
        assert_ne!(wrong_policy.policy_id, vote.policy_id);
        assert!(wrong_policy.validate_for(&order, &committee).is_err());
    }

    #[test]
    fn certificate_round_trip() {
        let (c, mut auth, policy) = setup(4);
        let order = signed_tx(&policy, "alice", 42, 0, "bob", 30);
        let votes: Vec<Vote> = auth
            .iter_mut()
            .take(3)
            .map(|a| a.handle(&order).unwrap())
            .collect();
        let cert = Certificate::assemble(order, votes, &c).expect("quorum");
        let bytes = encode_certificate(&cert);
        let back = decode_certificate(&bytes).unwrap();
        assert_eq!(back.order, cert.order);
        assert_eq!(back.votes, cert.votes);
        assert!(back.is_valid(&c));
    }

    #[test]
    fn wrong_domain_rejected() {
        let policy = policy();
        let order = signed_tx(&policy, "alice", 42, 0, "bob", 1);
        let mut bytes = encode_transfer(&order);
        // Corrupt the domain tag.
        bytes[0] ^= 0xff;
        assert_eq!(decode_transfer(&bytes), Err(WireError::BadDomain));
    }

    #[test]
    fn truncated_stream_rejected() {
        let policy = policy();
        let order = signed_tx(&policy, "alice", 42, 0, "bob", 1);
        let bytes = encode_transfer(&order);
        assert!(decode_transfer(&bytes[..bytes.len().saturating_sub(1)]).is_err());
    }

    #[test]
    fn unsigned_v1_order_fails_closed() {
        let mut legacy = Vec::new();
        legacy.extend_from_slice(b"transfer333/wire-transfer/v1\0");
        put_transfer_body(&mut legacy, &tx("alice", 0, "bob", 1), 0);
        assert_eq!(decode_transfer(&legacy), Err(WireError::BadDomain));

        let mut framed = vec![TAG_ORDER];
        framed.extend_from_slice(&legacy);
        assert!(matches!(
            decode_authority_msg(&framed),
            Err(WireError::BadDomain)
        ));
    }

    #[test]
    fn oversized_network_length_is_rejected_before_string_allocation() {
        let policy = policy();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(DOMAIN_TRANSFER);
        bytes.extend_from_slice(policy.id().as_bytes());
        bytes.extend_from_slice(&((MAX_NETWORK_ID_BYTES as u64) + 1).to_le_bytes());

        assert_eq!(
            decode_transfer(&bytes),
            Err(WireError::FieldTooLong {
                field: "network_id",
                length: (MAX_NETWORK_ID_BYTES as u64) + 1,
                max: MAX_NETWORK_ID_BYTES,
            })
        );
    }

    #[test]
    fn oversized_account_length_is_rejected_before_string_allocation() {
        let policy = policy();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(DOMAIN_TRANSFER);
        bytes.extend_from_slice(policy.id().as_bytes());
        put_str(&mut bytes, policy.network_id().as_str());
        bytes.extend_from_slice(&((MAX_ACCOUNT_ID_BYTES as u64) + 1).to_le_bytes());

        assert_eq!(
            decode_transfer(&bytes),
            Err(WireError::FieldTooLong {
                field: "transfer.from",
                length: (MAX_ACCOUNT_ID_BYTES as u64) + 1,
                max: MAX_ACCOUNT_ID_BYTES,
            })
        );
    }

    #[test]
    fn oversized_authority_length_is_rejected_before_string_allocation() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(DOMAIN_VOTE);
        bytes.extend_from_slice(&((MAX_AUTHORITY_ID_BYTES as u64) + 1).to_le_bytes());

        assert_eq!(
            decode_vote(&bytes),
            Err(WireError::FieldTooLong {
                field: "vote.authority",
                length: (MAX_AUTHORITY_ID_BYTES as u64) + 1,
                max: MAX_AUTHORITY_ID_BYTES,
            })
        );
    }

    #[test]
    fn absurd_certificate_vote_count_is_rejected_before_allocation() {
        assert_eq!(MAX_CERTIFICATE_VOTES, MAX_COMMITTEE_MEMBERS);
        let policy = policy();
        let order = signed_tx(&policy, "alice", 42, 0, "bob", 1);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(DOMAIN_CERT);
        put_signed_transfer_body(&mut bytes, &order);
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());

        assert!(matches!(
            decode_certificate(&bytes),
            Err(WireError::TooManyVotes {
                count: u64::MAX,
                max: MAX_CERTIFICATE_VOTES,
            })
        ));
    }

    #[test]
    fn tampered_vote_bytes_fail_certificate_is_valid() {
        // A flipped signature byte must not silently produce a valid cert.
        let (committee, mut auth, policy) = setup(4);
        let order = signed_tx(&policy, "alice", 42, 0, "bob", 10);
        let votes: Vec<Vote> = auth
            .iter_mut()
            .take(3)
            .map(|a| a.handle(&order).unwrap())
            .collect();
        let cert = Certificate::assemble(order, votes, &committee).unwrap();
        assert!(cert.is_valid(&committee));

        let mut bytes = encode_certificate(&cert);
        // Flip a byte in the trailing signature region (last 64 bytes of stream).
        let idx = bytes.len() - 1;
        bytes[idx] ^= 0x01;

        match decode_certificate(&bytes) {
            Ok(tampered) => {
                assert!(
                    !tampered.is_valid(&committee),
                    "tampered cert must fail is_valid, not decode as valid"
                );
            }
            Err(_) => {
                // Decode-time failure is also acceptable — never a valid cert.
            }
        }
    }

    #[test]
    fn tampered_standalone_vote_fails_when_assembled() {
        let (committee, mut auth, policy) = setup(4);
        let order = signed_tx(&policy, "alice", 42, 0, "bob", 10);
        let good: Vec<Vote> = auth
            .iter_mut()
            .take(2)
            .map(|a| a.handle(&order).unwrap())
            .collect();
        let mut v2 = auth[2].handle(&order).unwrap();
        let mut wire = encode_vote(&v2);
        // Corrupt signature bytes at the end of the vote stream.
        let last = wire.len() - 1;
        wire[last] ^= 0x5a;
        match decode_vote(&wire) {
            Ok(bad) => {
                v2 = bad;
                let mut all = good;
                all.push(v2);
                assert!(
                    Certificate::assemble(order, all, &committee).is_none(),
                    "tampered vote must not form a valid certificate"
                );
            }
            Err(_) => {
                // Fine: codec refused the stream.
            }
        }
    }

    #[test]
    fn wire_layout_round_trip_preserves_a_valid_vote_preimage() {
        let (committee, mut authorities, policy) = setup(4);
        let order = signed_tx(&policy, "alice", 42, 0, "bob", 3);
        let v = authorities[0].handle(&order).unwrap();
        let back = decode_vote(&encode_vote(&v)).unwrap();
        assert_eq!(back, v);
        assert!(back.validate_for(&order, &committee).is_ok());
    }

    #[test]
    fn attestation_round_trip_and_frame() {
        let (committee, mut auth, policy) = setup(4);
        let order = signed_tx(&policy, "alice", 42, 0, "bob", 30);
        let votes: Vec<Vote> = auth
            .iter_mut()
            .take(3)
            .map(|a| a.handle(&order).unwrap())
            .collect();
        let verified = Certificate::assemble(order, votes, &committee)
            .unwrap()
            .verify(&committee)
            .unwrap();
        auth[0].confirm(&verified, &committee).unwrap();
        let attestation = auth[0]
            .attestation_for(&"alice".to_string(), 0)
            .expect("applied confirm attests");

        let back = decode_attestation(&encode_attestation(&attestation)).unwrap();
        assert_eq!(back, attestation);
        assert!(back.is_valid(&committee));

        let framed = decode_authority_msg(&encode_authority_msg(&AuthorityMsg::Attestation(
            attestation.clone(),
        )))
        .unwrap();
        match framed {
            AuthorityMsg::Attestation(got) => {
                assert_eq!(got, attestation);
                assert!(got.is_valid(&committee));
            }
            _ => panic!("expected Attestation"),
        }

        // A corrupted attestation frame must never decode to a valid one.
        let mut bytes = encode_authority_msg(&AuthorityMsg::Attestation(attestation));
        let idx = bytes.len() - 1;
        bytes[idx] ^= 0x01;
        match decode_authority_msg(&bytes) {
            Ok(AuthorityMsg::Attestation(tampered)) => {
                assert!(!tampered.is_valid(&committee));
            }
            Ok(_) => panic!("expected Attestation after tag-preserving tamper"),
            Err(_) => {}
        }
    }

    #[test]
    fn authority_msg_frame_round_trip_order_vote_cert() {
        let (committee, mut auth, policy) = setup(4);
        let order = signed_tx(&policy, "alice", 42, 0, "bob", 30);

        let order_msg = AuthorityMsg::Order(order.clone());
        let back = decode_authority_msg(&encode_authority_msg(&order_msg)).unwrap();
        match back {
            AuthorityMsg::Order(got) => assert_eq!(got, order),
            _ => panic!("expected Order"),
        }

        let vote = auth[0].handle(&order).unwrap();
        let vote_msg = AuthorityMsg::Vote(vote.clone());
        let back = decode_authority_msg(&encode_authority_msg(&vote_msg)).unwrap();
        match back {
            AuthorityMsg::Vote(got) => assert_eq!(got, vote),
            _ => panic!("expected Vote"),
        }

        let votes: Vec<Vote> = auth
            .iter_mut()
            .take(3)
            .map(|a| a.handle(&order).unwrap())
            .collect();
        let cert = Certificate::assemble(order, votes, &committee).unwrap();
        let cert_msg = AuthorityMsg::Cert(cert.clone());
        let back = decode_authority_msg(&encode_authority_msg(&cert_msg)).unwrap();
        match back {
            AuthorityMsg::Cert(got) => {
                assert_eq!(got.order, cert.order);
                assert_eq!(got.votes, cert.votes);
                assert!(got.is_valid(&committee));
            }
            _ => panic!("expected Cert"),
        }
    }

    #[test]
    fn authority_msg_frame_tamper_fails_certificate_is_valid() {
        // Same guarantee as in-memory: a flipped cert byte must not yield a valid QC.
        let (committee, mut auth, policy) = setup(4);
        let order = signed_tx(&policy, "alice", 42, 0, "bob", 10);
        let votes: Vec<Vote> = auth
            .iter_mut()
            .take(3)
            .map(|a| a.handle(&order).unwrap())
            .collect();
        let cert = Certificate::assemble(order, votes, &committee).unwrap();
        assert!(cert.is_valid(&committee));

        let mut bytes = encode_authority_msg(&AuthorityMsg::Cert(cert));
        let idx = bytes.len() - 1;
        bytes[idx] ^= 0x01;

        match decode_authority_msg(&bytes) {
            Ok(AuthorityMsg::Cert(tampered)) => {
                assert!(
                    !tampered.is_valid(&committee),
                    "tampered AuthorityMsg::Cert must fail is_valid"
                );
            }
            Ok(_) => panic!("expected Cert after tag-preserving tamper"),
            Err(_) => {
                // Decode-time failure is also acceptable.
            }
        }
    }

    #[test]
    fn authority_msg_unknown_tag_rejected() {
        let policy = policy();
        let order = signed_tx(&policy, "a", 45, 0, "b", 1);
        let mut bytes = encode_authority_msg(&AuthorityMsg::Order(order));
        bytes[0] = 0xFF;
        match decode_authority_msg(&bytes) {
            Err(WireError::UnknownTag(0xFF)) => {}
            other => panic!("expected UnknownTag(0xFF), got {other:?}"),
        }
    }

    // --- epoch-change codec (M1) ------------------------------------------------

    fn epoch_setup() -> (EpochProposal, EpochVote, EpochCert) {
        let roster: Vec<(String, _)> = (0..4u8)
            .map(|i| (format!("b{i}"), key(100 + i).verifying_key()))
            .collect();
        let next_committee = crate::authority::Committee::with_epoch(roster.clone(), policy(), 1)
            .expect("valid next roster");
        let frontier = vec![
            ("alice".to_string(), 1u64),
            ("bob".to_string(), 1u64),
            ("carol".to_string(), 0u64),
        ];
        let proposal = EpochProposal {
            network_id: NetworkId::new("wire-testnet").unwrap(),
            policy_id: policy().id(),
            epoch: 1,
            next_roster: roster,
        };
        let old_committee_id = crate::authority::CommitteeId::from_bytes([7u8; 32]);
        let vote = EpochVote::sign(
            "a0".to_string(),
            old_committee_id,
            1,
            next_committee.id(),
            &frontier,
            &key(7),
        );
        let votes = (0..3u8)
            .map(|i| {
                EpochVote::sign(
                    format!("a{i}"),
                    old_committee_id,
                    1,
                    next_committee.id(),
                    &frontier,
                    &key(i),
                )
            })
            .collect();
        let cert = EpochCert {
            epoch: 1,
            next_roster: proposal.next_roster.clone(),
            frontier,
            votes,
        };
        (proposal, vote, cert)
    }

    #[test]
    fn epoch_proposal_roundtrip() {
        let (proposal, _, _) = epoch_setup();
        let bytes = encode_epoch_proposal(&proposal);
        assert_eq!(decode_epoch_proposal(&bytes).unwrap(), proposal);
    }

    #[test]
    fn epoch_vote_roundtrip_and_signature_survives() {
        let (_, vote, _) = epoch_setup();
        let bytes = encode_epoch_vote(&vote);
        let back = decode_epoch_vote(&bytes).unwrap();
        assert_eq!(back, vote);
        assert!(back.verify_signature(&key(7).verifying_key()).is_ok());
    }

    #[test]
    fn epoch_cert_roundtrip() {
        let (_, _, cert) = epoch_setup();
        let bytes = encode_epoch_cert(&cert);
        assert_eq!(decode_epoch_cert(&bytes).unwrap(), cert);
    }

    #[test]
    fn epoch_frames_roundtrip_via_authority_msg() {
        let (proposal, vote, cert) = epoch_setup();
        for msg in [
            AuthorityMsg::EpochProposal(proposal),
            AuthorityMsg::EpochVote(vote),
            AuthorityMsg::EpochCert(cert),
        ] {
            let bytes = encode_authority_msg(&msg);
            let back = decode_authority_msg(&bytes).unwrap();
            match (&msg, &back) {
                (AuthorityMsg::EpochProposal(a), AuthorityMsg::EpochProposal(b)) => {
                    assert_eq!(a, b)
                }
                (AuthorityMsg::EpochVote(a), AuthorityMsg::EpochVote(b)) => assert_eq!(a, b),
                (AuthorityMsg::EpochCert(a), AuthorityMsg::EpochCert(b)) => assert_eq!(a, b),
                _ => panic!("epoch frame decoded to the wrong variant"),
            }
        }
    }

    #[test]
    fn epoch_domains_fail_closed_against_foreign_bodies() {
        let (_, vote, _) = epoch_setup();
        // An epoch-vote body is not a transfer-vote body and must not decode as one.
        assert!(decode_vote(&encode_epoch_vote(&vote)).is_err());
        // And a transfer-vote must not decode as an epoch-vote.
        let policy = policy();
        let order = signed_tx(&policy, "alice", 42, 0, "bob", 1);
        let (committee, mut auth, _) = setup(4);
        let _ = committee;
        let v = auth[0].handle(&order).unwrap();
        assert!(decode_epoch_vote(&encode_vote(&v)).is_err());
    }

    #[test]
    fn epoch_cert_rejects_oversized_vote_count() {
        let (proposal, _, cert) = epoch_setup();
        // Hand-build a frame whose declared vote count exceeds the cap: the
        // decoder must fail closed before allocating for it.
        let mut forged = Vec::new();
        forged.extend_from_slice(DOMAIN_EPOCH_CERT);
        forged.extend_from_slice(&cert.epoch.to_le_bytes());
        put_roster(&mut forged, &proposal.next_roster);
        put_frontier(&mut forged, &cert.frontier);
        forged.extend_from_slice(&((crate::authority::MAX_COMMITTEE_MEMBERS as u64) + 1).to_le_bytes());
        match decode_epoch_cert(&forged) {
            Err(WireError::TooManyVotes { .. }) => {}
            other => panic!("expected TooManyVotes, got {other:?}"),
        }
    }
}
