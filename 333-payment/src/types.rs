use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

pub type Digest = [u8; 32];
pub type Amount = u128;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PaymentError {
    #[error("committee must not be empty")]
    EmptyCommittee,
    #[error("committee id does not match its domain, epoch and sorted roster")]
    InvalidCommitteeId,
    #[error("committee epoch must advance exactly by one: current={current} next={next}")]
    InvalidCommitteeSuccessor { current: u64, next: u64 },
    #[error("protocol context mismatch")]
    ContextMismatch,
    #[error("owner public key is not bound to the debited account")]
    AccountKeyMismatch,
    #[error("owner signature is invalid")]
    InvalidOwnerSignature,
    #[error("amount must be positive")]
    ZeroAmount,
    #[error("unknown sender")]
    UnknownSender,
    #[error("sequence mismatch: expected={expected} got={got}")]
    StaleSequence { expected: u64, got: u64 },
    #[error("insufficient balance: have={have} need={need}")]
    Insufficient { have: Amount, need: Amount },
    #[error("order expired at control height {valid_until}; current height is {current}")]
    Expired { valid_until: u64, current: u64 },
    #[error("authority is not a member of the active committee")]
    NotCommitteeMember,
    #[error("authority already signed a conflicting value for this safety slot")]
    Equivocation,
    #[error("authority has retired after committee rotation")]
    AuthorityRetired,
    #[error("transfer certificate is invalid")]
    InvalidTransferCertificate,
    #[error("control certificate is invalid")]
    InvalidControlCertificate,
    #[error("control height mismatch: expected={expected} got={got}")]
    ControlHeight { expected: u64, got: u64 },
    #[error("control parent state hash mismatch")]
    ControlParentMismatch,
    #[error("control result state hash mismatch")]
    ControlResultMismatch,
    #[error("control operation list must not be empty")]
    EmptyControlBlock,
    #[error("committee rotation must be the final operation in a control block")]
    RotationMustBeLast,
    #[error("committee rotation is blocked while this authority has pending transfer locks")]
    PendingTransfersAtRotation,
    #[error("job already exists")]
    JobExists,
    #[error("job not found")]
    JobNotFound,
    #[error("job is in the wrong state")]
    InvalidJobState,
    #[error("bid must be positive and no greater than the funded budget")]
    InvalidBid,
    #[error("bid not found")]
    BidNotFound,
    #[error("only the buyer or accepted provider may open this dispute")]
    DisputeActor,
    #[error("escrow funding proof is invalid")]
    InvalidFundingProof,
    #[error("escrow funding certificate was already consumed")]
    FundingAlreadyConsumed,
    #[error("reward epoch was already distributed")]
    RewardEpochAlreadyDistributed,
    #[error("reward allocation is invalid")]
    InvalidRewardAllocation,
    #[error("reward reserve is insufficient")]
    RewardReserveInsufficient,
    #[error("payout voucher not found")]
    VoucherNotFound,
    #[error("payout voucher was already redeemed")]
    VoucherAlreadyRedeemed,
    #[error("arithmetic overflow")]
    Overflow,
    #[error("durable state is locked by another writer")]
    StoreBusy,
    #[error("durable state is corrupt or truncated")]
    CorruptStore,
    #[error("durable state error: {0}")]
    Storage(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AccountId(pub Digest);

impl AccountId {
    /// A protocol-owned account has no corresponding owner key and therefore
    /// cannot originate a FastPay transfer.
    pub fn system(domain: &Domain, label: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(b"payment333/system-account/v1\0");
        h.update(domain.canonical_bytes());
        h.update((label.len() as u64).to_le_bytes());
        h.update(label);
        Self(h.finalize().into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AuthorityId(pub Digest);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Domain {
    pub protocol_version: u16,
    pub network_id: Digest,
    pub asset_id: Digest,
    pub genesis_hash: Digest,
}

impl Domain {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(98);
        out.extend_from_slice(&self.protocol_version.to_le_bytes());
        out.extend_from_slice(&self.network_id);
        out.extend_from_slice(&self.asset_id);
        out.extend_from_slice(&self.genesis_hash);
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaymentContext {
    pub domain: Domain,
    pub committee_epoch: u64,
    pub committee_id: Digest,
}

impl PaymentContext {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = self.domain.canonical_bytes();
        out.extend_from_slice(&self.committee_epoch.to_le_bytes());
        out.extend_from_slice(&self.committee_id);
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitteeSpec {
    domain: Domain,
    epoch: u64,
    id: Digest,
    members: BTreeMap<AuthorityId, [u8; 32]>,
}

impl CommitteeSpec {
    pub fn new<I>(domain: Domain, epoch: u64, keys: I) -> Result<Self, PaymentError>
    where
        I: IntoIterator<Item = [u8; 32]>,
    {
        let members: BTreeMap<AuthorityId, [u8; 32]> = keys
            .into_iter()
            .map(|key| (authority_id_from_bytes(&key), key))
            .collect();
        if members.is_empty() {
            return Err(PaymentError::EmptyCommittee);
        }
        let id = committee_id(&domain, epoch, &members);
        Ok(Self {
            domain,
            epoch,
            id,
            members,
        })
    }

    pub fn successor<I>(&self, next_epoch: u64, keys: I) -> Result<Self, PaymentError>
    where
        I: IntoIterator<Item = [u8; 32]>,
    {
        if next_epoch != self.epoch + 1 {
            return Err(PaymentError::InvalidCommitteeSuccessor {
                current: self.epoch,
                next: next_epoch,
            });
        }
        Self::new(self.domain, next_epoch, keys)
    }

    pub fn validate(&self) -> Result<(), PaymentError> {
        if self.members.is_empty() {
            return Err(PaymentError::EmptyCommittee);
        }
        if self.id != committee_id(&self.domain, self.epoch, &self.members) {
            return Err(PaymentError::InvalidCommitteeId);
        }
        Ok(())
    }

    pub fn domain(&self) -> Domain {
        self.domain
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn id(&self) -> Digest {
        self.id
    }

    pub fn context(&self) -> PaymentContext {
        PaymentContext {
            domain: self.domain,
            committee_epoch: self.epoch,
            committee_id: self.id,
        }
    }

    pub fn members(&self) -> &BTreeMap<AuthorityId, [u8; 32]> {
        &self.members
    }

    pub fn key_of(&self, id: &AuthorityId) -> Option<VerifyingKey> {
        let bytes = self.members.get(id)?;
        VerifyingKey::from_bytes(bytes).ok()
    }

    pub fn contains_key(&self, key: &[u8; 32]) -> bool {
        self.members.get(&authority_id_from_bytes(key)) == Some(key)
    }

    pub fn quorum(&self) -> usize {
        quorum(self.members.len())
    }

    pub(crate) fn append_canonical(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.context().canonical_bytes());
        out.extend_from_slice(&(self.members.len() as u64).to_le_bytes());
        for (id, key) in &self.members {
            out.extend_from_slice(&id.0);
            out.extend_from_slice(key);
        }
    }
}

fn committee_id(domain: &Domain, epoch: u64, members: &BTreeMap<AuthorityId, [u8; 32]>) -> Digest {
    let mut h = Sha256::new();
    h.update(b"payment333/committee/v1\0");
    h.update(domain.canonical_bytes());
    h.update(epoch.to_le_bytes());
    h.update((members.len() as u64).to_le_bytes());
    for (id, key) in members {
        h.update(id.0);
        h.update(key);
    }
    h.finalize().into()
}

/// Safe quorum for arbitrary `n` with `f=floor((n-1)/3)`. This is `n-f`, not
/// `2f+1`, because the latter loses honest quorum intersection when n>3f+1.
pub fn quorum(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    n - (n - 1) / 3
}

pub fn account_id_from_key(key: &VerifyingKey) -> AccountId {
    let mut h = Sha256::new();
    h.update(b"payment333/account/v1\0");
    h.update(key.as_bytes());
    AccountId(h.finalize().into())
}

pub fn authority_id_from_key(key: &VerifyingKey) -> AuthorityId {
    authority_id_from_bytes(key.as_bytes())
}

fn authority_id_from_bytes(key: &[u8; 32]) -> AuthorityId {
    let mut h = Sha256::new();
    h.update(b"payment333/authority/v1\0");
    h.update(key);
    AuthorityId(h.finalize().into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    pub from: AccountId,
    pub from_seq: u64,
    pub to: AccountId,
    pub amount: Amount,
    /// The order is invalid once the BFT control plane advances past this
    /// height. Committee rotation therefore has an explicit drain boundary.
    pub valid_until_control_height: u64,
}

impl Transfer {
    pub(crate) fn append_canonical(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.from.0);
        out.extend_from_slice(&self.from_seq.to_le_bytes());
        out.extend_from_slice(&self.to.0);
        out.extend_from_slice(&self.amount.to_le_bytes());
        out.extend_from_slice(&self.valid_until_control_height.to_le_bytes());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureBytes(#[serde(with = "BigArray")] pub [u8; 64]);

impl SignatureBytes {
    pub fn from_signature(signature: &Signature) -> Self {
        Self(signature.to_bytes())
    }

    pub fn as_signature(&self) -> Signature {
        Signature::from_bytes(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedTransferOrder {
    pub context: PaymentContext,
    pub transfer: Transfer,
    pub owner_public_key: [u8; 32],
    pub owner_signature: SignatureBytes,
}

impl SignedTransferOrder {
    pub fn sign(
        context: PaymentContext,
        transfer: Transfer,
        owner: &SigningKey,
    ) -> Result<Self, PaymentError> {
        if account_id_from_key(&owner.verifying_key()) != transfer.from {
            return Err(PaymentError::AccountKeyMismatch);
        }
        let owner_public_key = owner.verifying_key().to_bytes();
        let message = owner_order_message_parts(&context, &transfer, &owner_public_key);
        Ok(Self {
            context,
            transfer,
            owner_public_key,
            owner_signature: SignatureBytes::from_signature(&owner.sign(&message)),
        })
    }

    /// Constructor for data received from an untrusted wire decoder. It does no
    /// validation; every authority and ledger boundary calls [`verify_owner`].
    pub fn from_untrusted_parts(
        context: PaymentContext,
        transfer: Transfer,
        owner_public_key: [u8; 32],
        owner_signature: SignatureBytes,
    ) -> Self {
        Self {
            context,
            transfer,
            owner_public_key,
            owner_signature,
        }
    }

    pub fn verify_owner(&self, expected: &PaymentContext) -> Result<(), PaymentError> {
        if &self.context != expected {
            return Err(PaymentError::ContextMismatch);
        }
        let key = VerifyingKey::from_bytes(&self.owner_public_key)
            .map_err(|_| PaymentError::InvalidOwnerSignature)?;
        if account_id_from_key(&key) != self.transfer.from {
            return Err(PaymentError::AccountKeyMismatch);
        }
        let message = owner_order_message(self);
        key.verify(&message, &self.owner_signature.as_signature())
            .map_err(|_| PaymentError::InvalidOwnerSignature)
    }

    pub fn digest(&self) -> Digest {
        Sha256::digest(owner_order_message(self)).into()
    }
}

pub fn owner_order_message(order: &SignedTransferOrder) -> Vec<u8> {
    owner_order_message_parts(&order.context, &order.transfer, &order.owner_public_key)
}

fn owner_order_message_parts(
    context: &PaymentContext,
    transfer: &Transfer,
    owner_public_key: &[u8; 32],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(b"payment333/owner-transfer/v1\0");
    out.extend_from_slice(&context.canonical_bytes());
    transfer.append_canonical(&mut out);
    out.extend_from_slice(owner_public_key);
    out
}

pub(crate) fn transfer_vote_message(order: &SignedTransferOrder) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(b"payment333/authority-transfer-vote/v1\0");
    out.extend_from_slice(&order.digest());
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferVote {
    pub authority: AuthorityId,
    pub order: SignedTransferOrder,
    pub signature: SignatureBytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferCertificate {
    pub order: SignedTransferOrder,
    pub votes: Vec<TransferVote>,
}

#[derive(Debug, Clone)]
pub struct VerifiedTransfer(pub(crate) SignedTransferOrder);

impl VerifiedTransfer {
    pub fn order(&self) -> &SignedTransferOrder {
        &self.0
    }

    pub fn transfer(&self) -> &Transfer {
        &self.0.transfer
    }
}

impl TransferCertificate {
    pub fn assemble(
        order: SignedTransferOrder,
        mut votes: Vec<TransferVote>,
        committee: &CommitteeSpec,
    ) -> Result<Self, PaymentError> {
        votes.sort_by_key(|v| v.authority);
        let cert = Self { order, votes };
        cert.verify(committee)?;
        Ok(cert)
    }

    pub fn verify(&self, committee: &CommitteeSpec) -> Result<VerifiedTransfer, PaymentError> {
        committee.validate()?;
        self.order
            .verify_owner(&committee.context())
            .map_err(|_| PaymentError::InvalidTransferCertificate)?;
        if self.order.transfer.amount == 0 {
            return Err(PaymentError::InvalidTransferCertificate);
        }
        let message = transfer_vote_message(&self.order);
        let mut distinct = BTreeSet::new();
        for vote in &self.votes {
            if vote.order != self.order || !distinct.insert(vote.authority) {
                return Err(PaymentError::InvalidTransferCertificate);
            }
            let key = committee
                .key_of(&vote.authority)
                .ok_or(PaymentError::InvalidTransferCertificate)?;
            key.verify(&message, &vote.signature.as_signature())
                .map_err(|_| PaymentError::InvalidTransferCertificate)?;
        }
        if distinct.len() < committee.quorum() {
            return Err(PaymentError::InvalidTransferCertificate);
        }
        Ok(VerifiedTransfer(self.order.clone()))
    }

    pub fn digest(&self) -> Digest {
        let mut h = Sha256::new();
        h.update(b"payment333/transfer-certificate/v1\0");
        h.update(self.order.digest());
        let mut votes = self.votes.clone();
        votes.sort_by_key(|v| v.authority);
        for vote in votes {
            h.update(vote.authority.0);
            h.update(vote.signature.0);
        }
        h.finalize().into()
    }
}

pub(crate) fn hash_bytes(domain: &[u8], bytes: &[u8]) -> Digest {
    let mut h = Sha256::new();
    h.update(domain);
    h.update(bytes);
    h.finalize().into()
}
