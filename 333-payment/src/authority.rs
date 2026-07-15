use std::collections::BTreeMap;
use std::path::Path;

use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};

use crate::control::{control_vote_message, ControlCertificate, ControlOperation, ControlVote};
use crate::storage::Persistence;
use crate::types::{transfer_vote_message, SignatureBytes};
use crate::{
    account_id_from_key, authority_id_from_key, AccountId, AuthorityId, CommitteeSpec, Digest,
    PaymentError, PaymentLedger, SignedTransferOrder, TransferCertificate, TransferVote,
    ValidatedControlBlock,
};

const AUTHORITY_STATE_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuthorityState {
    version: u16,
    committee: CommitteeSpec,
    pending_transfers: BTreeMap<AccountId, SignedTransferOrder>,
    pending_control: Option<(u64, Digest)>,
    control_height: u64,
    control_hash: Digest,
    retired: bool,
}

/// A single safety signer for both payment lanes. Every first signing decision
/// is durably persisted before the signature leaves this object.
#[derive(Debug)]
pub struct Authority {
    id: AuthorityId,
    signing: SigningKey,
    state: AuthorityState,
    persistence: Persistence,
}

impl Authority {
    pub fn new(
        signing: SigningKey,
        committee: CommitteeSpec,
        control_height: u64,
        control_hash: Digest,
    ) -> Result<Self, PaymentError> {
        let state = initial_state(&signing, committee, control_height, control_hash)?;
        Ok(Self {
            id: authority_id_from_key(&signing.verifying_key()),
            signing,
            state,
            persistence: Persistence::memory(),
        })
    }

    pub fn create(
        path: impl AsRef<Path>,
        signing: SigningKey,
        committee: CommitteeSpec,
        control_height: u64,
        control_hash: Digest,
    ) -> Result<Self, PaymentError> {
        let persistence = Persistence::open(path)?;
        if persistence.exists() {
            return Err(PaymentError::Storage(
                "authority state already exists".into(),
            ));
        }
        let state = initial_state(&signing, committee, control_height, control_hash)?;
        persistence.persist(&state)?;
        Ok(Self {
            id: authority_id_from_key(&signing.verifying_key()),
            signing,
            state,
            persistence,
        })
    }

    pub fn open(path: impl AsRef<Path>, signing: SigningKey) -> Result<Self, PaymentError> {
        let persistence = Persistence::open(path)?;
        if !persistence.exists() {
            return Err(PaymentError::Storage(
                "authority state does not exist".into(),
            ));
        }
        let state: AuthorityState = persistence.load()?;
        if state.version != AUTHORITY_STATE_VERSION {
            return Err(PaymentError::CorruptStore);
        }
        state
            .committee
            .validate()
            .map_err(|_| PaymentError::CorruptStore)?;
        let id = authority_id_from_key(&signing.verifying_key());
        if !state.retired && state.committee.key_of(&id).as_ref() != Some(&signing.verifying_key())
        {
            return Err(PaymentError::NotCommitteeMember);
        }
        Ok(Self {
            id,
            signing,
            state,
            persistence,
        })
    }

    pub fn id(&self) -> AuthorityId {
        self.id
    }

    pub fn verifying_key(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    pub fn committee(&self) -> &CommitteeSpec {
        &self.state.committee
    }

    pub fn is_retired(&self) -> bool {
        self.state.retired
    }

    /// Validate owner authentication, domain/epoch, expiry, sequence and local
    /// durable balance before locking the account slot and voting.
    pub fn handle_transfer(
        &mut self,
        order: &SignedTransferOrder,
        ledger: &PaymentLedger,
    ) -> Result<TransferVote, PaymentError> {
        self.ensure_active()?;
        if ledger.current_context() != self.state.committee.context() {
            return Err(PaymentError::ContextMismatch);
        }
        order.verify_owner(&self.state.committee.context())?;
        let transfer = &order.transfer;
        if transfer.amount == 0 {
            return Err(PaymentError::ZeroAmount);
        }
        if ledger.control_height() > transfer.valid_until_control_height {
            return Err(PaymentError::Expired {
                valid_until: transfer.valid_until_control_height,
                current: ledger.control_height(),
            });
        }
        if !ledger.has_account(&transfer.from) {
            return Err(PaymentError::UnknownSender);
        }
        let expected = ledger.next_seq(&transfer.from);
        if transfer.from_seq != expected {
            return Err(PaymentError::StaleSequence {
                expected,
                got: transfer.from_seq,
            });
        }
        let have = ledger.balance(&transfer.from);
        if have < transfer.amount {
            return Err(PaymentError::Insufficient {
                have,
                need: transfer.amount,
            });
        }
        match self.state.pending_transfers.get(&transfer.from) {
            Some(existing) if existing == order => return Ok(self.sign_transfer(order)),
            Some(_) => return Err(PaymentError::Equivocation),
            None => {}
        }
        let mut next = self.state.clone();
        next.pending_transfers.insert(transfer.from, order.clone());
        self.persist_then_install(next)?;
        Ok(self.sign_transfer(order))
    }

    /// Clear a transfer lock only after the local durable ledger has applied a
    /// valid certificate and advanced the account sequence.
    pub fn confirm_transfer(
        &mut self,
        certificate: &TransferCertificate,
        ledger: &PaymentLedger,
    ) -> Result<(), PaymentError> {
        certificate.verify(&self.state.committee)?;
        let transfer = &certificate.order.transfer;
        if ledger.next_seq(&transfer.from) <= transfer.from_seq {
            return Err(PaymentError::StaleSequence {
                expected: transfer.from_seq + 1,
                got: ledger.next_seq(&transfer.from),
            });
        }
        let mut next = self.state.clone();
        next.pending_transfers.remove(&transfer.from);
        self.persist_then_install(next)
    }

    /// Crash recovery for the narrow window where the ledger was fsynced but
    /// this authority had not yet cleared its pending order.
    pub fn recover_transfers_from_ledger(
        &mut self,
        ledger: &PaymentLedger,
    ) -> Result<usize, PaymentError> {
        let before = self.state.pending_transfers.len();
        let mut next = self.state.clone();
        next.pending_transfers
            .retain(|account, order| ledger.next_seq(account) <= order.transfer.from_seq);
        let removed = before - next.pending_transfers.len();
        if removed > 0 {
            self.persist_then_install(next)?;
        }
        Ok(removed)
    }

    /// Vote only for a block already executed by this validator's local ledger.
    /// A durable per-height lock makes conflicting quorum certificates
    /// impossible under the usual f<n/3 assumption, even across restart.
    pub fn vote_control(
        &mut self,
        validated: &ValidatedControlBlock,
    ) -> Result<ControlVote, PaymentError> {
        self.ensure_active()?;
        let block = validated.block();
        if block.context != self.state.committee.context() {
            return Err(PaymentError::ContextMismatch);
        }
        if block.height != self.state.control_height {
            return Err(PaymentError::ControlHeight {
                expected: self.state.control_height,
                got: block.height,
            });
        }
        if block.parent_control_hash != self.state.control_hash {
            return Err(PaymentError::ControlParentMismatch);
        }
        if block
            .operations
            .iter()
            .any(|operation| matches!(operation, ControlOperation::RotateCommittee { .. }))
            && !self.state.pending_transfers.is_empty()
        {
            return Err(PaymentError::PendingTransfersAtRotation);
        }
        let digest = block.digest();
        match self.state.pending_control {
            Some((height, existing)) if height == block.height && existing == digest => {
                return Ok(self.sign_control(block));
            }
            Some((height, _)) if height == block.height => return Err(PaymentError::Equivocation),
            Some(_) => return Err(PaymentError::Equivocation),
            None => {}
        }
        let mut next = self.state.clone();
        next.pending_control = Some((block.height, digest));
        self.persist_then_install(next)?;
        Ok(self.sign_control(block))
    }

    /// Advance the control safety slot only after the local ledger has durably
    /// committed the certified block. Committee rotation retires removed keys.
    pub fn confirm_control(
        &mut self,
        certificate: &ControlCertificate,
        ledger: &PaymentLedger,
    ) -> Result<(), PaymentError> {
        certificate.verify(&self.state.committee)?;
        let block = &certificate.block;
        if ledger.control_height() != block.height + 1 {
            return Err(PaymentError::ControlHeight {
                expected: block.height + 1,
                got: ledger.control_height(),
            });
        }
        if ledger.control_hash() != block.result_control_hash {
            return Err(PaymentError::ControlResultMismatch);
        }
        let mut next = self.state.clone();
        next.pending_control = None;
        next.control_height = block.height + 1;
        next.control_hash = block.result_control_hash;
        if ledger.current_context() != next.committee.context() {
            next.committee = ledger.current_committee().clone();
            next.retired =
                next.committee.key_of(&self.id).as_ref() != Some(&self.signing.verifying_key());
        }
        self.persist_then_install(next)
    }

    fn ensure_active(&self) -> Result<(), PaymentError> {
        if self.state.retired {
            Err(PaymentError::AuthorityRetired)
        } else if self.state.committee.key_of(&self.id).as_ref()
            != Some(&self.signing.verifying_key())
        {
            Err(PaymentError::NotCommitteeMember)
        } else {
            Ok(())
        }
    }

    fn sign_transfer(&self, order: &SignedTransferOrder) -> TransferVote {
        let message = transfer_vote_message(order);
        TransferVote {
            authority: self.id,
            order: order.clone(),
            signature: SignatureBytes::from_signature(&self.signing.sign(&message)),
        }
    }

    fn sign_control(&self, block: &crate::ControlBlock) -> ControlVote {
        let message = control_vote_message(block);
        ControlVote {
            authority: self.id,
            block_digest: block.digest(),
            signature: SignatureBytes::from_signature(&self.signing.sign(&message)),
        }
    }

    fn persist_then_install(&mut self, next: AuthorityState) -> Result<(), PaymentError> {
        self.persistence.persist(&next)?;
        self.state = next;
        Ok(())
    }
}

fn initial_state(
    signing: &SigningKey,
    committee: CommitteeSpec,
    control_height: u64,
    control_hash: Digest,
) -> Result<AuthorityState, PaymentError> {
    committee.validate()?;
    if !committee.contains_key(signing.verifying_key().as_bytes()) {
        return Err(PaymentError::NotCommitteeMember);
    }
    let _owner_binding_sanity = account_id_from_key(&signing.verifying_key());
    Ok(AuthorityState {
        version: AUTHORITY_STATE_VERSION,
        committee,
        pending_transfers: BTreeMap::new(),
        pending_control: None,
        control_height,
        control_hash,
        retired: false,
    })
}
