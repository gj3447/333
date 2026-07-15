use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use bincode::Options;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::control::{ControlBlock, ControlCertificate, ControlOperation, JobResolution};
use crate::storage::Persistence;
use crate::{
    AccountId, Amount, CommitteeSpec, Digest, Domain, PaymentContext, PaymentError,
    TransferCertificate, ValidatedControlBlock,
};

const LEDGER_STATE_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Account {
    balance: Amount,
    next_seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Open,
    Leased,
    Disputed,
    Settled,
    Refunded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: Digest,
    pub buyer: AccountId,
    pub budget: Amount,
    pub funding_certificate: Digest,
    pub state: JobState,
    pub bids: BTreeMap<AccountId, Amount>,
    pub accepted: Option<(AccountId, Amount)>,
    pub payout_vouchers: Vec<Digest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoucherSource {
    EscrowVault,
    RewardReserve,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayoutVoucher {
    pub id: Digest,
    pub source: VoucherSource,
    pub recipient: AccountId,
    pub amount: Amount,
    pub created_control_height: u64,
    /// Escrow vouchers are redeemable only on replicas that have durably
    /// applied this exact FastPay deposit. Reward vouchers have no funding cert.
    pub funding_certificate: Option<Digest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LedgerState {
    version: u16,
    domain: Domain,
    accounts: BTreeMap<AccountId, Account>,
    applied_transfer_certificates: BTreeSet<Digest>,
    committees: BTreeMap<u64, CommitteeSpec>,
    current_epoch: u64,
    control_height: u64,
    jobs: BTreeMap<Digest, Job>,
    consumed_funding: BTreeSet<Digest>,
    vouchers: BTreeMap<Digest, PayoutVoucher>,
    redeemed_vouchers: BTreeSet<Digest>,
    reward_epochs: BTreeSet<u64>,
    reward_remaining: Amount,
    initial_supply: Amount,
}

#[derive(Serialize)]
struct ControlCommitment<'a> {
    version: u16,
    domain: Domain,
    committees: &'a BTreeMap<u64, CommitteeSpec>,
    current_epoch: u64,
    control_height: u64,
    jobs: &'a BTreeMap<Digest, Job>,
    consumed_funding: &'a BTreeSet<Digest>,
    vouchers: &'a BTreeMap<Digest, PayoutVoucher>,
    reward_epochs: &'a BTreeSet<u64>,
    reward_remaining: Amount,
}

/// One economic ledger with proof-typed fast and control mutation paths.
#[derive(Debug)]
pub struct PaymentLedger {
    state: LedgerState,
    persistence: Persistence,
}

impl PaymentLedger {
    pub fn new<I>(
        genesis_committee: CommitteeSpec,
        allocations: I,
        reward_reserve: Amount,
    ) -> Result<Self, PaymentError>
    where
        I: IntoIterator<Item = (AccountId, Amount)>,
    {
        let state = Self::genesis_state(genesis_committee, allocations, reward_reserve)?;
        Ok(Self {
            state,
            persistence: Persistence::memory(),
        })
    }

    pub fn create<I>(
        path: impl AsRef<Path>,
        genesis_committee: CommitteeSpec,
        allocations: I,
        reward_reserve: Amount,
    ) -> Result<Self, PaymentError>
    where
        I: IntoIterator<Item = (AccountId, Amount)>,
    {
        let persistence = Persistence::open(path)?;
        if persistence.exists() {
            return Err(PaymentError::Storage("ledger state already exists".into()));
        }
        let state = Self::genesis_state(genesis_committee, allocations, reward_reserve)?;
        persistence.persist(&state)?;
        Ok(Self { state, persistence })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, PaymentError> {
        let persistence = Persistence::open(path)?;
        if !persistence.exists() {
            return Err(PaymentError::Storage("ledger state does not exist".into()));
        }
        let state: LedgerState = persistence.load()?;
        validate_loaded_state(&state)?;
        Ok(Self { state, persistence })
    }

    fn genesis_state<I>(
        genesis_committee: CommitteeSpec,
        allocations: I,
        reward_reserve: Amount,
    ) -> Result<LedgerState, PaymentError>
    where
        I: IntoIterator<Item = (AccountId, Amount)>,
    {
        genesis_committee.validate()?;
        if genesis_committee.epoch() != 0 {
            return Err(PaymentError::InvalidCommitteeSuccessor {
                current: 0,
                next: genesis_committee.epoch(),
            });
        }
        let domain = genesis_committee.domain();
        let escrow = AccountId::system(&domain, b"escrow-vault");
        let rewards = AccountId::system(&domain, b"reward-reserve");
        let mut accounts = BTreeMap::new();
        let mut initial_supply = reward_reserve;
        for (id, amount) in allocations {
            if id == escrow || id == rewards {
                return Err(PaymentError::AccountKeyMismatch);
            }
            let entry = accounts.entry(id).or_insert(Account {
                balance: 0,
                next_seq: 0,
            });
            entry.balance = entry
                .balance
                .checked_add(amount)
                .ok_or(PaymentError::Overflow)?;
            initial_supply = initial_supply
                .checked_add(amount)
                .ok_or(PaymentError::Overflow)?;
        }
        accounts.insert(
            escrow,
            Account {
                balance: 0,
                next_seq: 0,
            },
        );
        accounts.insert(
            rewards,
            Account {
                balance: reward_reserve,
                next_seq: 0,
            },
        );
        let mut committees = BTreeMap::new();
        committees.insert(0, genesis_committee);
        Ok(LedgerState {
            version: LEDGER_STATE_VERSION,
            domain,
            accounts,
            applied_transfer_certificates: BTreeSet::new(),
            committees,
            current_epoch: 0,
            control_height: 0,
            jobs: BTreeMap::new(),
            consumed_funding: BTreeSet::new(),
            vouchers: BTreeMap::new(),
            redeemed_vouchers: BTreeSet::new(),
            reward_epochs: BTreeSet::new(),
            reward_remaining: reward_reserve,
            initial_supply,
        })
    }

    pub fn domain(&self) -> Domain {
        self.state.domain
    }

    pub fn current_committee(&self) -> &CommitteeSpec {
        self.state
            .committees
            .get(&self.state.current_epoch)
            .expect("validated ledger always has current committee")
    }

    pub fn current_context(&self) -> PaymentContext {
        self.current_committee().context()
    }

    pub fn committee_at(&self, epoch: u64) -> Option<&CommitteeSpec> {
        self.state.committees.get(&epoch)
    }

    pub fn control_height(&self) -> u64 {
        self.state.control_height
    }

    pub fn control_hash(&self) -> Digest {
        control_hash(&self.state)
    }

    pub fn escrow_account(&self) -> AccountId {
        AccountId::system(&self.state.domain, b"escrow-vault")
    }

    pub fn reward_reserve_account(&self) -> AccountId {
        AccountId::system(&self.state.domain, b"reward-reserve")
    }

    pub fn has_account(&self, id: &AccountId) -> bool {
        self.state.accounts.contains_key(id)
    }

    pub fn balance(&self, id: &AccountId) -> Amount {
        self.state
            .accounts
            .get(id)
            .map(|account| account.balance)
            .unwrap_or(0)
    }

    pub fn next_seq(&self, id: &AccountId) -> u64 {
        self.state
            .accounts
            .get(id)
            .map(|account| account.next_seq)
            .unwrap_or(0)
    }

    pub fn total_supply(&self) -> Amount {
        checked_total_supply(&self.state).expect("validated ledger supply cannot overflow")
    }

    pub fn initial_supply(&self) -> Amount {
        self.state.initial_supply
    }

    pub fn has_applied_transfer_certificate(&self, digest: &Digest) -> bool {
        self.state.applied_transfer_certificates.contains(digest)
    }

    pub fn job(&self, id: &Digest) -> Option<&Job> {
        self.state.jobs.get(id)
    }

    pub fn voucher(&self, id: &Digest) -> Option<&PayoutVoucher> {
        self.state.vouchers.get(id)
    }

    pub fn voucher_ids(&self) -> Vec<Digest> {
        self.state.vouchers.keys().copied().collect()
    }

    pub fn voucher_redeemed(&self, id: &Digest) -> bool {
        self.state.redeemed_vouchers.contains(id)
    }

    /// Apply only a current-epoch, owner-authenticated quorum certificate.
    /// Old committee certificates fail closed immediately after rotation.
    pub fn apply_fast(&mut self, certificate: &TransferCertificate) -> Result<(), PaymentError> {
        let committee = self.current_committee();
        let verified = certificate.verify(committee)?;
        let transfer = verified.transfer();
        if self.state.control_height > transfer.valid_until_control_height {
            return Err(PaymentError::Expired {
                valid_until: transfer.valid_until_control_height,
                current: self.state.control_height,
            });
        }
        let from = self
            .state
            .accounts
            .get(&transfer.from)
            .ok_or(PaymentError::UnknownSender)?;
        if transfer.from_seq != from.next_seq {
            return Err(PaymentError::StaleSequence {
                expected: from.next_seq,
                got: transfer.from_seq,
            });
        }
        if transfer.amount == 0 {
            return Err(PaymentError::ZeroAmount);
        }
        if from.balance < transfer.amount {
            return Err(PaymentError::Insufficient {
                have: from.balance,
                need: transfer.amount,
            });
        }
        let mut next = self.state.clone();
        let from = next.state_account_mut(&transfer.from)?;
        from.balance -= transfer.amount;
        from.next_seq += 1;
        let to = next.accounts.entry(transfer.to).or_insert(Account {
            balance: 0,
            next_seq: 0,
        });
        to.balance = to
            .balance
            .checked_add(transfer.amount)
            .ok_or(PaymentError::Overflow)?;
        if transfer.to == self.escrow_account() {
            next.applied_transfer_certificates
                .insert(certificate.digest());
        }
        self.commit(next)
    }

    pub fn build_control_block(
        &self,
        operations: Vec<ControlOperation>,
    ) -> Result<ControlBlock, PaymentError> {
        if operations.is_empty() {
            return Err(PaymentError::EmptyControlBlock);
        }
        let mut block = ControlBlock {
            context: self.current_context(),
            height: self.state.control_height,
            parent_control_hash: self.control_hash(),
            result_control_hash: [0; 32],
            operations,
        };
        let mut next = self.state.clone();
        apply_operations(&mut next, &block)?;
        next.control_height += 1;
        block.result_control_hash = control_hash(&next);
        Ok(block)
    }

    pub fn validate_control_block(
        &self,
        block: &ControlBlock,
    ) -> Result<ValidatedControlBlock, PaymentError> {
        self.validate_block_and_next(block)?;
        Ok(ValidatedControlBlock(block.clone()))
    }

    pub fn apply_control(&mut self, certificate: &ControlCertificate) -> Result<(), PaymentError> {
        certificate.verify(self.current_committee())?;
        let next = self.validate_block_and_next(&certificate.block)?;
        self.commit(next)
    }

    fn validate_block_and_next(&self, block: &ControlBlock) -> Result<LedgerState, PaymentError> {
        if block.context != self.current_context() {
            return Err(PaymentError::ContextMismatch);
        }
        if block.height != self.state.control_height {
            return Err(PaymentError::ControlHeight {
                expected: self.state.control_height,
                got: block.height,
            });
        }
        if block.parent_control_hash != self.control_hash() {
            return Err(PaymentError::ControlParentMismatch);
        }
        if block.operations.is_empty() {
            return Err(PaymentError::EmptyControlBlock);
        }
        let mut next = self.state.clone();
        apply_operations(&mut next, block)?;
        next.control_height += 1;
        if control_hash(&next) != block.result_control_hash {
            return Err(PaymentError::ControlResultMismatch);
        }
        Ok(next)
    }

    /// Redeem a BFT-created voucher exactly once. This is the only bridge from
    /// shared control state back into transferable balances.
    pub fn redeem_voucher(&mut self, id: &Digest) -> Result<(), PaymentError> {
        let voucher = self
            .state
            .vouchers
            .get(id)
            .cloned()
            .ok_or(PaymentError::VoucherNotFound)?;
        if self.state.redeemed_vouchers.contains(id) {
            return Err(PaymentError::VoucherAlreadyRedeemed);
        }
        if let Some(funding) = voucher.funding_certificate {
            if !self.state.applied_transfer_certificates.contains(&funding) {
                return Err(PaymentError::InvalidFundingProof);
            }
        }
        let source = match voucher.source {
            VoucherSource::EscrowVault => self.escrow_account(),
            VoucherSource::RewardReserve => self.reward_reserve_account(),
        };
        let available = self.balance(&source);
        if available < voucher.amount {
            return Err(PaymentError::Insufficient {
                have: available,
                need: voucher.amount,
            });
        }
        let mut next = self.state.clone();
        next.accounts
            .get_mut(&source)
            .expect("system account exists")
            .balance -= voucher.amount;
        let recipient = next.accounts.entry(voucher.recipient).or_insert(Account {
            balance: 0,
            next_seq: 0,
        });
        recipient.balance = recipient
            .balance
            .checked_add(voucher.amount)
            .ok_or(PaymentError::Overflow)?;
        next.redeemed_vouchers.insert(*id);
        self.commit(next)
    }

    fn commit(&mut self, next: LedgerState) -> Result<(), PaymentError> {
        if checked_total_supply(&next) != Some(next.initial_supply) {
            return Err(PaymentError::CorruptStore);
        }
        self.persistence.persist(&next)?;
        self.state = next;
        Ok(())
    }
}

impl LedgerState {
    fn state_account_mut(&mut self, id: &AccountId) -> Result<&mut Account, PaymentError> {
        self.accounts.get_mut(id).ok_or(PaymentError::UnknownSender)
    }
}

fn apply_operations(state: &mut LedgerState, block: &ControlBlock) -> Result<(), PaymentError> {
    let rotation_pos = block
        .operations
        .iter()
        .position(|op| matches!(op, ControlOperation::RotateCommittee { .. }));
    if let Some(pos) = rotation_pos {
        if pos + 1 != block.operations.len() {
            return Err(PaymentError::RotationMustBeLast);
        }
    }

    for (index, operation) in block.operations.iter().enumerate() {
        match operation {
            ControlOperation::OpenJob {
                job_id,
                buyer,
                budget,
                funding,
            } => {
                if state.jobs.contains_key(job_id) {
                    return Err(PaymentError::JobExists);
                }
                let committee = state
                    .committees
                    .get(&funding.order.context.committee_epoch)
                    .ok_or(PaymentError::InvalidFundingProof)?;
                let verified = funding
                    .verify(committee)
                    .map_err(|_| PaymentError::InvalidFundingProof)?;
                let transfer = verified.transfer();
                if transfer.from != *buyer
                    || transfer.to != AccountId::system(&state.domain, b"escrow-vault")
                    || transfer.amount != *budget
                    || *budget == 0
                {
                    return Err(PaymentError::InvalidFundingProof);
                }
                let funding_digest = funding.digest();
                if !state
                    .applied_transfer_certificates
                    .contains(&funding_digest)
                {
                    return Err(PaymentError::InvalidFundingProof);
                }
                if !state.consumed_funding.insert(funding_digest) {
                    return Err(PaymentError::FundingAlreadyConsumed);
                }
                state.jobs.insert(
                    *job_id,
                    Job {
                        id: *job_id,
                        buyer: *buyer,
                        budget: *budget,
                        funding_certificate: funding_digest,
                        state: JobState::Open,
                        bids: BTreeMap::new(),
                        accepted: None,
                        payout_vouchers: Vec::new(),
                    },
                );
            }
            ControlOperation::PlaceBid {
                job_id,
                provider,
                price,
            } => {
                let job = state
                    .jobs
                    .get_mut(job_id)
                    .ok_or(PaymentError::JobNotFound)?;
                if job.state != JobState::Open {
                    return Err(PaymentError::InvalidJobState);
                }
                if *price == 0 || *price > job.budget {
                    return Err(PaymentError::InvalidBid);
                }
                job.bids.insert(*provider, *price);
            }
            ControlOperation::AcceptBid { job_id, provider } => {
                let job = state
                    .jobs
                    .get_mut(job_id)
                    .ok_or(PaymentError::JobNotFound)?;
                if job.state != JobState::Open {
                    return Err(PaymentError::InvalidJobState);
                }
                let price = *job.bids.get(provider).ok_or(PaymentError::BidNotFound)?;
                job.accepted = Some((*provider, price));
                job.state = JobState::Leased;
            }
            ControlOperation::OpenDispute { job_id, actor } => {
                let job = state
                    .jobs
                    .get_mut(job_id)
                    .ok_or(PaymentError::JobNotFound)?;
                if job.state != JobState::Leased {
                    return Err(PaymentError::InvalidJobState);
                }
                let provider = job
                    .accepted
                    .map(|accepted| accepted.0)
                    .ok_or(PaymentError::BidNotFound)?;
                if *actor != job.buyer && *actor != provider {
                    return Err(PaymentError::DisputeActor);
                }
                job.state = JobState::Disputed;
            }
            ControlOperation::ResolveJob { job_id, resolution } => {
                let snapshot = state
                    .jobs
                    .get(job_id)
                    .cloned()
                    .ok_or(PaymentError::JobNotFound)?;
                if snapshot.state != JobState::Leased && snapshot.state != JobState::Disputed {
                    return Err(PaymentError::InvalidJobState);
                }
                let mut ids = Vec::new();
                match resolution {
                    JobResolution::PayProvider => {
                        let (provider, price) =
                            snapshot.accepted.ok_or(PaymentError::BidNotFound)?;
                        ids.push(create_voucher(
                            state,
                            block.height,
                            index,
                            0,
                            job_id,
                            VoucherSource::EscrowVault,
                            provider,
                            price,
                            Some(snapshot.funding_certificate),
                        )?);
                        let refund = snapshot.budget - price;
                        if refund > 0 {
                            ids.push(create_voucher(
                                state,
                                block.height,
                                index,
                                1,
                                job_id,
                                VoucherSource::EscrowVault,
                                snapshot.buyer,
                                refund,
                                Some(snapshot.funding_certificate),
                            )?);
                        }
                        let job = state.jobs.get_mut(job_id).expect("snapshotted job exists");
                        job.state = JobState::Settled;
                        job.payout_vouchers = ids;
                    }
                    JobResolution::RefundBuyer => {
                        ids.push(create_voucher(
                            state,
                            block.height,
                            index,
                            0,
                            job_id,
                            VoucherSource::EscrowVault,
                            snapshot.buyer,
                            snapshot.budget,
                            Some(snapshot.funding_certificate),
                        )?);
                        let job = state.jobs.get_mut(job_id).expect("snapshotted job exists");
                        job.state = JobState::Refunded;
                        job.payout_vouchers = ids;
                    }
                }
            }
            ControlOperation::DistributeReward {
                reward_epoch,
                allocations,
            } => {
                if allocations.is_empty() {
                    return Err(PaymentError::InvalidRewardAllocation);
                }
                if !state.reward_epochs.insert(*reward_epoch) {
                    return Err(PaymentError::RewardEpochAlreadyDistributed);
                }
                let mut total = 0u128;
                for (_, amount) in allocations {
                    if *amount == 0 {
                        return Err(PaymentError::InvalidRewardAllocation);
                    }
                    total = total.checked_add(*amount).ok_or(PaymentError::Overflow)?;
                }
                if total > state.reward_remaining {
                    return Err(PaymentError::RewardReserveInsufficient);
                }
                state.reward_remaining -= total;
                let cause = Sha256::digest(reward_epoch.to_le_bytes()).into();
                for (subindex, (recipient, amount)) in allocations.iter().enumerate() {
                    create_voucher(
                        state,
                        block.height,
                        index,
                        subindex,
                        &cause,
                        VoucherSource::RewardReserve,
                        *recipient,
                        *amount,
                        None,
                    )?;
                }
            }
            ControlOperation::RotateCommittee { next } => {
                next.validate()?;
                let current = state
                    .committees
                    .get(&state.current_epoch)
                    .ok_or(PaymentError::InvalidCommitteeId)?;
                if next.domain() != state.domain || next.epoch() != current.epoch() + 1 {
                    return Err(PaymentError::InvalidCommitteeSuccessor {
                        current: current.epoch(),
                        next: next.epoch(),
                    });
                }
                state.current_epoch = next.epoch();
                state.committees.insert(next.epoch(), next.clone());
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn create_voucher(
    state: &mut LedgerState,
    height: u64,
    operation_index: usize,
    subindex: usize,
    cause: &Digest,
    source: VoucherSource,
    recipient: AccountId,
    amount: Amount,
    funding_certificate: Option<Digest>,
) -> Result<Digest, PaymentError> {
    if amount == 0 {
        return Err(PaymentError::InvalidRewardAllocation);
    }
    let mut h = Sha256::new();
    h.update(b"payment333/payout-voucher/v1\0");
    h.update(state.domain.canonical_bytes());
    h.update(height.to_le_bytes());
    h.update((operation_index as u64).to_le_bytes());
    h.update((subindex as u64).to_le_bytes());
    h.update(cause);
    h.update([match source {
        VoucherSource::EscrowVault => 1,
        VoucherSource::RewardReserve => 2,
    }]);
    h.update(recipient.0);
    h.update(amount.to_le_bytes());
    match funding_certificate {
        Some(digest) => {
            h.update([1]);
            h.update(digest);
        }
        None => h.update([0]),
    }
    let id: Digest = h.finalize().into();
    if state.vouchers.contains_key(&id) {
        return Err(PaymentError::Equivocation);
    }
    state.vouchers.insert(
        id,
        PayoutVoucher {
            id,
            source,
            recipient,
            amount,
            created_control_height: height,
            funding_certificate,
        },
    );
    Ok(id)
}

fn control_hash(state: &LedgerState) -> Digest {
    let commitment = ControlCommitment {
        version: state.version,
        domain: state.domain,
        committees: &state.committees,
        current_epoch: state.current_epoch,
        control_height: state.control_height,
        jobs: &state.jobs,
        consumed_funding: &state.consumed_funding,
        vouchers: &state.vouchers,
        reward_epochs: &state.reward_epochs,
        reward_remaining: state.reward_remaining,
    };
    let bytes = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .serialize(&commitment)
        .expect("control commitment contains only serializable bounded values");
    let mut h = Sha256::new();
    h.update(b"payment333/control-state/v1\0");
    h.update(bytes);
    h.finalize().into()
}

fn validate_loaded_state(state: &LedgerState) -> Result<(), PaymentError> {
    if state.version != LEDGER_STATE_VERSION
        || checked_total_supply(state) != Some(state.initial_supply)
    {
        return Err(PaymentError::CorruptStore);
    }
    for (epoch, committee) in &state.committees {
        committee
            .validate()
            .map_err(|_| PaymentError::CorruptStore)?;
        if committee.domain() != state.domain || committee.epoch() != *epoch {
            return Err(PaymentError::CorruptStore);
        }
    }
    state
        .committees
        .get(&state.current_epoch)
        .ok_or(PaymentError::CorruptStore)?;
    Ok(())
}

fn checked_total_supply(state: &LedgerState) -> Option<Amount> {
    state
        .accounts
        .values()
        .try_fold(0u128, |total, account| total.checked_add(account.balance))
}
