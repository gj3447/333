//! Canonical 333 payment boundary.
//!
//! `transfer333` proved the FastPay-style quorum core, while `token333`
//! provided an independently mutable in-memory ledger.  Keeping those as two
//! writable sources of truth is unsafe for ORRR: escrow, bids, disputes and
//! reward epochs are multi-writer state and require an ordered control plane,
//! while ordinary owner-to-owner payouts should retain the FastPay path.
//!
//! This crate closes that split with one economic ledger and two proof-typed
//! mutation paths:
//!
//! * [`TransferCertificate`] moves owner-controlled balances without global
//!   ordering. The owner and an independent authority quorum sign the same
//!   domain/asset/genesis/committee-epoch-bound order.
//! * [`ControlCertificate`] orders shared marketplace transitions. It may
//!   consume a certified escrow deposit and create one-shot payout vouchers,
//!   but cannot fabricate an owner transfer.
//!
//! Safety-critical authority locks and ledger state can be opened on durable,
//! single-writer, checksum-protected files. A lock is fsynced before a vote is
//! returned, so restart cannot erase an authority's prior signing decision.

#![forbid(unsafe_code)]

mod authority;
mod control;
mod ledger;
mod storage;
mod types;

pub use authority::Authority;
pub use control::{
    control_block_message, ControlBlock, ControlCertificate, ControlOperation, ControlVote,
    JobResolution, ValidatedControlBlock,
};
pub use ledger::{Job, JobState, PaymentLedger, PayoutVoucher, VoucherSource};
pub use types::{
    account_id_from_key, authority_id_from_key, owner_order_message, quorum, AccountId, Amount,
    AuthorityId, CommitteeSpec, Digest, Domain, PaymentContext, PaymentError, SignatureBytes,
    SignedTransferOrder, Transfer, TransferCertificate, TransferVote, VerifiedTransfer,
};
