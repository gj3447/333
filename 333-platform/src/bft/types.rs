// KG: SPAN_333_BFT_Types
//! Core types for HotStuff BFT consensus.

use super::crypto::{NodeId, QuorumCert, Signature};
use serde::{Deserialize, Serialize};

/// Transaction that requires total ordering (not suitable for CRDT)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderedTx {
    /// Token transfer: from → to, amount
    Transfer {
        from: NodeId,
        to: NodeId,
        amount: u64,
        nonce: u64,
    },
    /// Auction bid
    AuctionBid {
        bidder: NodeId,
        item_id: u64,
        amount: u64,
    },
    /// Game action that requires global ordering (e.g., "who mined first?")
    RankedAction {
        player: NodeId,
        action_type: u32,
        payload: Vec<u8>,
    },
}

/// HotStuff phases — 3-phase commit pipeline
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Phase {
    Prepare,
    PreCommit,
    Commit,
    Decide,
}

/// A block proposed by the leader
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub hash: u64,
    pub view: u64,
    pub parent_hash: u64,
    pub transactions: Vec<OrderedTx>,
    /// QC from the previous phase (proves parent was accepted)
    pub justify: QuorumCert,
    pub proposer: NodeId,
}

/// Message types in HotStuff protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HotStuffMsg {
    /// Leader → All: "Here's a new block proposal"
    Proposal {
        block: Block,
        phase: Phase,
        /// FIX Issue 7.1: proposer signature prevents MITM injection
        signature: Signature,
    },
    /// Validator → Leader: "I vote for this block"
    Vote {
        block_hash: u64,
        view: u64,
        phase: Phase,
        signature: Signature,
    },
    /// Leader → All: "Quorum reached, advance phase"
    NewView {
        view: u64,
        qc: QuorumCert,
        /// FIX Issue 7.1: leader signature on view advancement
        signature: Signature,
    },
    /// Timeout: no progress in current view, request view change
    ViewChange {
        new_view: u64,
        sender: NodeId,
        high_qc: QuorumCert,
        /// FIX Issue 7.1: sender signature on view change request
        signature: Signature,
    },
}

/// Result of processing a message
/// # KG: taliban2-H1-fix-2026-04-15
#[derive(Debug)]
pub enum ProcessResult {
    /// No action needed
    None,
    /// Send this message to the leader
    SendToLeader(HotStuffMsg),
    /// Broadcast this message to all validators
    Broadcast(HotStuffMsg),
    /// Block has been committed — execute these transactions.
    /// Also carry the NewView message that MUST be broadcast to advance non-leader validators.
    /// Callers (bft_bridge, bft_multi_node) MUST broadcast the inner NewView message.
    /// # KG: taliban2-H1-fix-2026-04-15
    Committed(Vec<OrderedTx>, HotStuffMsg),
    /// Need to change view (leader timeout)
    ViewChange(u64),
}
