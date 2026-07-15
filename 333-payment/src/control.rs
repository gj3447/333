use std::collections::BTreeSet;

use ed25519_dalek::Verifier;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::types::{hash_bytes, SignatureBytes};
use crate::{
    AccountId, Amount, AuthorityId, CommitteeSpec, Digest, PaymentContext, PaymentError,
    TransferCertificate,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobResolution {
    PayProvider,
    RefundBuyer,
}

/// Multi-writer marketplace transitions. These operations never directly
/// debit an owner-controlled account. `OpenJob` consumes a final FastPay
/// deposit into the protocol escrow vault; resolution and rewards create
/// one-shot vouchers funded by protocol-owned reserve accounts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlOperation {
    OpenJob {
        job_id: Digest,
        buyer: AccountId,
        budget: Amount,
        funding: TransferCertificate,
    },
    PlaceBid {
        job_id: Digest,
        provider: AccountId,
        price: Amount,
    },
    AcceptBid {
        job_id: Digest,
        provider: AccountId,
    },
    OpenDispute {
        job_id: Digest,
        actor: AccountId,
    },
    ResolveJob {
        job_id: Digest,
        resolution: JobResolution,
    },
    DistributeReward {
        reward_epoch: u64,
        allocations: Vec<(AccountId, Amount)>,
    },
    RotateCommittee {
        next: CommitteeSpec,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlBlock {
    pub context: PaymentContext,
    pub height: u64,
    pub parent_control_hash: Digest,
    pub result_control_hash: Digest,
    pub operations: Vec<ControlOperation>,
}

impl ControlBlock {
    pub fn digest(&self) -> Digest {
        hash_bytes(
            b"payment333/control-block/v1\0",
            &control_block_message(self),
        )
    }
}

/// The exact bytes control authorities sign. Variable data is length-prefixed;
/// nested funding certificates are committed by their canonical digest.
pub fn control_block_message(block: &ControlBlock) -> Vec<u8> {
    let mut out = Vec::with_capacity(512);
    out.extend_from_slice(b"payment333/control-block-message/v1\0");
    out.extend_from_slice(&block.context.canonical_bytes());
    out.extend_from_slice(&block.height.to_le_bytes());
    out.extend_from_slice(&block.parent_control_hash);
    out.extend_from_slice(&block.result_control_hash);
    out.extend_from_slice(&(block.operations.len() as u64).to_le_bytes());
    for operation in &block.operations {
        append_operation(operation, &mut out);
    }
    out
}

fn append_operation(operation: &ControlOperation, out: &mut Vec<u8>) {
    match operation {
        ControlOperation::OpenJob {
            job_id,
            buyer,
            budget,
            funding,
        } => {
            out.push(1);
            out.extend_from_slice(job_id);
            out.extend_from_slice(&buyer.0);
            out.extend_from_slice(&budget.to_le_bytes());
            out.extend_from_slice(&funding.digest());
        }
        ControlOperation::PlaceBid {
            job_id,
            provider,
            price,
        } => {
            out.push(2);
            out.extend_from_slice(job_id);
            out.extend_from_slice(&provider.0);
            out.extend_from_slice(&price.to_le_bytes());
        }
        ControlOperation::AcceptBid { job_id, provider } => {
            out.push(3);
            out.extend_from_slice(job_id);
            out.extend_from_slice(&provider.0);
        }
        ControlOperation::OpenDispute { job_id, actor } => {
            out.push(4);
            out.extend_from_slice(job_id);
            out.extend_from_slice(&actor.0);
        }
        ControlOperation::ResolveJob { job_id, resolution } => {
            out.push(5);
            out.extend_from_slice(job_id);
            out.push(match resolution {
                JobResolution::PayProvider => 1,
                JobResolution::RefundBuyer => 2,
            });
        }
        ControlOperation::DistributeReward {
            reward_epoch,
            allocations,
        } => {
            out.push(6);
            out.extend_from_slice(&reward_epoch.to_le_bytes());
            out.extend_from_slice(&(allocations.len() as u64).to_le_bytes());
            for (recipient, amount) in allocations {
                out.extend_from_slice(&recipient.0);
                out.extend_from_slice(&amount.to_le_bytes());
            }
        }
        ControlOperation::RotateCommittee { next } => {
            out.push(7);
            next.append_canonical(out);
        }
    }
}

pub(crate) fn control_vote_message(block: &ControlBlock) -> Vec<u8> {
    let mut out = Vec::with_capacity(72);
    out.extend_from_slice(b"payment333/control-precommit/v1\0");
    out.extend_from_slice(&block.digest());
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlVote {
    pub authority: AuthorityId,
    pub block_digest: Digest,
    pub signature: SignatureBytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlCertificate {
    pub block: ControlBlock,
    pub votes: Vec<ControlVote>,
}

impl ControlCertificate {
    pub fn assemble(
        block: ControlBlock,
        mut votes: Vec<ControlVote>,
        committee: &CommitteeSpec,
    ) -> Result<Self, PaymentError> {
        votes.sort_by_key(|vote| vote.authority);
        let cert = Self { block, votes };
        cert.verify(committee)?;
        Ok(cert)
    }

    pub fn verify(&self, committee: &CommitteeSpec) -> Result<(), PaymentError> {
        committee.validate()?;
        if self.block.context != committee.context() || self.block.operations.is_empty() {
            return Err(PaymentError::InvalidControlCertificate);
        }
        let digest = self.block.digest();
        let message = control_vote_message(&self.block);
        let mut distinct = BTreeSet::new();
        for vote in &self.votes {
            if vote.block_digest != digest || !distinct.insert(vote.authority) {
                return Err(PaymentError::InvalidControlCertificate);
            }
            let key = committee
                .key_of(&vote.authority)
                .ok_or(PaymentError::InvalidControlCertificate)?;
            key.verify(&message, &vote.signature.as_signature())
                .map_err(|_| PaymentError::InvalidControlCertificate)?;
        }
        if distinct.len() < committee.quorum() {
            return Err(PaymentError::InvalidControlCertificate);
        }
        Ok(())
    }

    pub fn digest(&self) -> Digest {
        let mut h = Sha256::new();
        h.update(b"payment333/control-certificate/v1\0");
        h.update(self.block.digest());
        let mut votes = self.votes.clone();
        votes.sort_by_key(|vote| vote.authority);
        for vote in votes {
            h.update(vote.authority.0);
            h.update(vote.signature.0);
        }
        h.finalize().into()
    }
}

/// Type-state proof that a validator's local [`crate::PaymentLedger`] executed
/// this block from its current control root and obtained the advertised result.
/// Only `PaymentLedger::validate_control_block` can construct it.
#[derive(Debug, Clone)]
pub struct ValidatedControlBlock(pub(crate) ControlBlock);

impl ValidatedControlBlock {
    pub fn block(&self) -> &ControlBlock {
        &self.0
    }
}
