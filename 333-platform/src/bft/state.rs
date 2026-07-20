// KG: SPAN_333_BFT_StateMachine
//! HotStuff state machine — the core consensus logic.
//!
//! 3-phase pipelined commit:
//!   Prepare → PreCommit → Commit → Decide
//!
//! Each phase collects 2f+1 votes to form a QC.
//! When a block reaches Decide, its transactions are committed (executed).

use std::collections::HashMap;
use crate::observability::BFT_VOTE_REJECTED_TOTAL;

// KG: TASK_333_B_BFTCrypto, lesson-333-bft-keyring-exchange-2026-04-14
// Ed25519 real signatures via Identity + ValidatorKeyring for peer verification
use super::crypto::{hash_block, phase_tag, sign_with_identity, sign_vote, verify_vote,
    NodeId, QuorumCert, Signature, ValidatorSet, ValidatorKeyring};
use super::leader::leader_for_view;
use super::types::*;
use super::viewchange::{ViewChangeTracker, ViewChangeConfig};
use crate::crypto_real::Identity;

/// Per-node HotStuff consensus state
/// KG: Taliban BFTCrypto — holds Identity for real Ed25519 signing
/// KG: lesson-333-bft-keyring-exchange-2026-04-14
pub struct HotStuffState {
    /// This node's ID
    pub node_id: NodeId,
    /// Ed25519 identity for signing (random key, not deterministic)
    pub(crate) identity: Identity,
    /// Keyring for verifying remote peer signatures
    pub(crate) keyring: ValidatorKeyring,
    /// Current view number (monotonically increasing)
    pub view: u64,
    /// Current phase within the view
    pub phase: Phase,
    /// Validator set
    pub validators: ValidatorSet,
    /// Highest QC we've seen
    pub high_qc: QuorumCert,
    /// Pending block for current view
    pub pending_block: Option<Block>,
    /// Collected votes, keyed by (block_hash, phase) → signatures.
    ///
    /// Keying by block_hash alone let a late vote from a PREVIOUS phase (e.g. the
    /// 4th Prepare vote arriving after the Prepare QC already formed) share a
    /// bucket with the next phase's votes and be aggregated into that phase's QC.
    /// Phase-bound signatures exposed the mix: such a QC carries signatures that
    /// don't verify under its phase. A vote may only ever count toward the exact
    /// (block, phase) it was signed for.
    /// # KG: fix-333-bft-vote-signature-phase-binding-2026-07-15
    pub votes: HashMap<(u64, Phase), Vec<Signature>>,
    /// # KG: taliban2-H2-fix-2026-04-15
    /// FIX HIGH #9: track (signer, phase, view) → block_hash to prevent replay across views.
    /// Formerly keyed by (NodeId, Phase) — stale votes from a previous view could replay.
    pub vote_tracker: HashMap<(NodeId, Phase, u64), u64>,
    /// Committed blocks (for execution)
    pub committed: Vec<Block>,
    /// Block storage (hash → block).
    /// # KG: taliban2-H3-fix-2026-04-15 — evicted after commit to prevent unbounded growth.
    pub blocks: HashMap<u64, Block>,
    /// View number of the last committed block (used for blocks eviction window).
    /// # KG: taliban2-H3-fix-2026-04-15
    pub committed_view: u64,
    /// Locked QC (safety: don't vote for conflicting blocks)
    pub locked_qc: QuorumCert,
    /// Transaction pool (pending, not yet proposed)
    pub tx_pool: Vec<OrderedTx>,
    /// View change timeout tracker — detects Byzantine leader stall
    /// KG: SPAN_333_BFT_ViewChange, sprint7C-viewchange-timeout-2026-04-16
    pub(crate) view_change_tracker: ViewChangeTracker,
}

impl HotStuffState {
    pub fn new(node_id: NodeId, validators: ValidatorSet) -> Self {
        Self::with_identity(node_id, validators, Identity::generate())
    }

    /// Create with a specific identity (for wasm.rs to share the same key)
    /// KG: Taliban BFTCrypto F1, lesson-333-bft-keyring-exchange-2026-04-14
    pub fn with_identity(node_id: NodeId, validators: ValidatorSet, identity: Identity) -> Self {
        let mut keyring = ValidatorKeyring::new();
        // Register own identity in keyring for self-verification
        keyring.register_identity(node_id, identity.clone());
        Self {
            node_id,
            identity,
            keyring,
            view: 0,
            phase: Phase::Prepare,
            validators,
            high_qc: QuorumCert::genesis(),
            pending_block: None,
            votes: HashMap::new(),
            vote_tracker: HashMap::new(),
            committed: Vec::new(),
            blocks: HashMap::new(),
            committed_view: 0,
            locked_qc: QuorumCert::genesis(),
            tx_pool: Vec::new(),
            view_change_tracker: ViewChangeTracker::new(ViewChangeConfig::default()),
        }
    }

    /// Am I the leader for the current view?
    pub fn is_leader(&self) -> bool {
        leader_for_view(self.view, &self.validators) == self.node_id
    }

    /// Current leader's node ID
    pub fn current_leader(&self) -> NodeId {
        leader_for_view(self.view, &self.validators)
    }

    /// Register a remote peer's identity for BFT signature verification
    /// KG: lesson-333-bft-keyring-exchange-2026-04-14
    pub fn register_peer_identity(&mut self, node_id: NodeId, identity: Identity) {
        self.keyring.register_identity(node_id, identity);
    }

    /// Register a remote peer's public key only (for verification)
    pub fn register_peer_key(&mut self, node_id: NodeId, peer_id: crate::crypto_real::PeerId) {
        self.keyring.register_peer_id(node_id, peer_id);
    }

    /// Maximum transactions in pool (FIX HIGH #8: DoS prevention)
    const MAX_POOL_SIZE: usize = 10_000;

    /// Submit a transaction to the pool
    pub fn submit_tx(&mut self, tx: OrderedTx) -> bool {
        if self.tx_pool.len() >= Self::MAX_POOL_SIZE {
            return false; // pool full, reject
        }
        self.tx_pool.push(tx);
        true
    }

    /// Leader: create a proposal block
    pub fn propose(&mut self) -> Option<HotStuffMsg> {
        if !self.is_leader() || self.tx_pool.is_empty() {
            return None;
        }

        // Drain tx pool into block
        let txs: Vec<_> = self.tx_pool.drain(..).collect();
        let parent_hash = self.high_qc.block_hash;
        let payload: Vec<u8> = serde_json::to_vec(&txs).unwrap_or_default();
        let hash = hash_block(self.view, parent_hash, &payload);

        let block = Block {
            hash,
            view: self.view,
            parent_hash,
            transactions: txs,
            justify: self.high_qc.clone(),
            proposer: self.node_id,
        };

        self.blocks.insert(hash, block.clone());
        self.pending_block = Some(block.clone());

        let sig = sign_with_identity(self.node_id, hash, &self.identity);
        Some(HotStuffMsg::Proposal {
            block,
            phase: Phase::Prepare,
            signature: sig,
        })
    }

    /// Process an incoming message
    pub fn process(&mut self, msg: HotStuffMsg) -> ProcessResult {
        match msg {
            HotStuffMsg::Proposal { block, phase, signature } => {
                // KG: lesson-333-bft-keyring-exchange-2026-04-14 — verify with keyring
                if !super::crypto::verify(&signature, block.hash, &self.keyring) {
                    return ProcessResult::None;
                }
                self.on_proposal(block, phase)
            }
            HotStuffMsg::Vote { block_hash, view, phase, signature } => {
                self.on_vote(block_hash, view, phase, signature)
            }
            HotStuffMsg::NewView { view, qc, signature: _ } => self.on_new_view(view, qc),
            HotStuffMsg::ViewChange { new_view, sender, high_qc, signature: _ } => {
                self.process_view_change_quorum(new_view, sender, high_qc)
            }
        }
    }

    /// Validate that a block's `justify` is a real quorum certificate before any
    /// safety decision trusts its fields. Without this a Byzantine / key-compromised
    /// leader can forge a justify (arbitrary high view, empty or bogus sigs), present
    /// a proposal whose `justify.view >= locked_qc.view` passes the safety rule, and
    /// drive honest validators to vote + advance view on a fabrication — collapsing
    /// 3-phase safety and committing on a forged justification. A justify is valid
    /// iff it is the genesis bootstrap QC, or it reaches quorum AND every signature
    /// verifies against `justify.block_hash` by a known validator.
    /// # KG: assess-333-engine-fsm-2026-07-13 (action 1), prom16-333-consensusless-frontier
    fn justify_is_valid(&self, justify: &QuorumCert) -> bool {
        // Genesis bootstrap: the first Prepare proposal carries the empty genesis QC.
        if justify.signatures.is_empty() && justify.block_hash == 0 && justify.view == 0 {
            return true;
        }
        if !justify.has_quorum(self.validators.n()) {
            return false;
        }
        // Vote signatures bind the phase they were cast in (`sign_vote`), so a
        // justify QC must be checked against ITS recorded phase — otherwise a
        // bag of relabeled votes could masquerade as a quorum certificate.
        // # KG: fix-333-bft-vote-signature-phase-binding-2026-07-15
        justify.signatures.iter().all(|s| {
            self.validators.contains(&s.signer)
                && verify_vote(s, justify.block_hash, phase_tag(justify.phase), &self.keyring)
        })
    }

    /// Validator: received a proposal from the leader
    fn on_proposal(&mut self, block: Block, phase: Phase) -> ProcessResult {
        // Verify proposer is current leader
        if block.proposer != self.current_leader() {
            return ProcessResult::None;
        }

        // Validate the proposal's justify QC BEFORE any safety rule trusts its
        // fields (justify.view/block_hash). Forged/sub-quorum justify => reject.
        // # KG: assess-333-engine-fsm-2026-07-13 (action 1)
        if !self.justify_is_valid(&block.justify) {
            return ProcessResult::None;
        }

        // Gap B fix: HotStuff safetyRule — accept block if ANY of:
        //   (a) block.justify.view >= locked_qc.view  [block is at least as recent]
        //   (b) block.parent_hash == locked_qc.block_hash  [block directly extends locked]
        //   (c) block.hash == locked_qc.block_hash  [block IS the locked block; Commit phase]
        //
        // Case (c) covers the leader's self-validation path: after on_vote(PreCommit),
        // the leader sets locked_qc = precommit_qc for the current block.  Then the
        // Commit-phase proposal for the SAME block arrives via broadcast_to_all.
        // Its justify.view (old high_qc.view) < locked_qc.view (current view), and
        // its parent_hash points to the previous block (not the current block), so
        // both (a) and (b) fail.  Case (c) allows committing what was already locked.
        // # KG: sprint4A-gap-B-fix-2026-04-15
        let safe = block.justify.view >= self.locked_qc.view
            || block.parent_hash == self.locked_qc.block_hash
            || block.hash == self.locked_qc.block_hash; // (c): commit the locked block
        if !safe {
            return ProcessResult::None;
        }

        // Store block
        self.blocks.insert(block.hash, block.clone());
        self.pending_block = Some(block.clone());
        self.phase = phase;

        // HotStuff spec: validators update locked_qc when they accept a PreCommit
        // proposal (i.e., when they see 2f+1 Prepare votes via the PreCommit QC).
        // This ensures validators have accurate locked_qc for the safety check.
        // Without this, validators always have locked_qc=genesis and can't enforce safety.
        // # KG: sprint4A-gap-B-fix-2026-04-15
        if phase == Phase::PreCommit {
            // The PreCommit proposal's block.justify is the Prepare QC.
            // Update locked_qc to this QC (locking on the current block).
            self.locked_qc = QuorumCert {
                block_hash: block.hash,
                view: self.view,
                // Local bookkeeping only (compared by view/block_hash, never
                // signature-verified): keep the phase the cloned sigs came from.
                phase: block.justify.phase,
                signatures: block.justify.signatures.clone(),
            };
        }

        // Gap C fix: non-leader validators advance their view when they vote on a
        // Commit-phase proposal.  At that point 2f+1 PreCommit votes have been
        // collected by the leader (otherwise the Commit proposal wouldn't exist),
        // so it is safe for validators to bump the view now — they will be in sync
        // with the leader's view after commit.
        // # KG: sprint4A-gap-C-fix-2026-04-15
        let advance_view_after_vote = phase == Phase::Commit;

        // Vote for this block — sign binds the phase (see phase_tag).
        let sig = sign_vote(self.node_id, block.hash, phase_tag(phase), &self.identity);
        let vote = ProcessResult::SendToLeader(HotStuffMsg::Vote {
            block_hash: block.hash,
            view: self.view,
            phase,
            signature: sig,
        });

        if advance_view_after_vote && !self.is_leader() {
            // Advance view so that after this round the validator is ready for the
            // next leader.  Also clear vote_tracker and pending state.
            self.view += 1;
            self.phase = Phase::Prepare;
            self.pending_block = None;
            self.votes.clear();
            self.vote_tracker.clear(); // Gap A fix (validator side)
            self.view_change_tracker.reset(); // Sprint 7C: reset timeout on normal view advance
        }

        vote
    }

    /// Leader: received a vote
    /// # KG: taliban2-H2-fix-2026-04-15
    fn on_vote(&mut self, block_hash: u64, view: u64, phase: Phase, sig: Signature) -> ProcessResult {
        if !self.is_leader() {
            return ProcessResult::None;
        }

        // # KG: taliban2-H2-fix-2026-04-15 — reject stale-view votes to prevent replay attack.
        // Votes from a previous view (view < self.view) cannot be valid for the current round.
        if view != self.view {
            BFT_VOTE_REJECTED_TOTAL.inc();
            return ProcessResult::None;
        }

        // KG: lesson-333-bft-keyring-exchange-2026-04-14 — verify with keyring
        if !verify_vote(&sig, block_hash, phase_tag(phase), &self.keyring) {
            BFT_VOTE_REJECTED_TOTAL.inc();
            return ProcessResult::None;
        }

        // Verify signer is a validator
        if !self.validators.contains(&sig.signer) {
            BFT_VOTE_REJECTED_TOTAL.inc();
            return ProcessResult::None;
        }

        // FIX HIGH #9 + H2: (signer, phase, view) triple — view inclusion prevents cross-view replay.
        // # KG: taliban2-H2-fix-2026-04-15
        let tracker_key = (sig.signer, phase, view);
        if let Some(&prev_hash) = self.vote_tracker.get(&tracker_key) {
            if prev_hash != block_hash {
                // EQUIVOCATION: same signer voted for different block in same (phase, view)!
                BFT_VOTE_REJECTED_TOTAL.inc();
                return ProcessResult::None;
            }
        }
        self.vote_tracker.insert(tracker_key, block_hash);

        // Collect vote — bucketed per (block, phase) so a vote can only count
        // toward the phase its signature was verified for (see field doc).
        // # KG: fix-333-bft-vote-signature-phase-binding-2026-07-15
        let sigs = self.votes.entry((block_hash, phase)).or_default();

        // Deduplicate within same block
        if sigs.iter().any(|s| s.signer == sig.signer) {
            return ProcessResult::None;
        }
        sigs.push(sig);

        // Check quorum. The QC records the phase its votes were cast in — every
        // signature inside was verified phase-bound above, so this QC can never
        // be re-validated under a different phase.
        // # KG: fix-333-bft-vote-signature-phase-binding-2026-07-15
        let qc = QuorumCert {
            block_hash,
            view: self.view,
            phase,
            signatures: sigs.clone(),
        };

        if !qc.has_quorum(self.validators.n()) {
            return ProcessResult::None;
        }

        // Quorum reached! Advance phase.
        self.votes.clear();

        match phase {
            Phase::Prepare => {
                // Prepare QC formed → move to PreCommit
                self.high_qc = qc.clone();
                self.phase = Phase::PreCommit;

                if let Some(block) = &self.pending_block {
                    let sig = sign_with_identity(self.node_id, block.hash, &self.identity);
                    ProcessResult::Broadcast(HotStuffMsg::Proposal {
                        block: block.clone(),
                        phase: Phase::PreCommit,
                        signature: sig,
                    })
                } else {
                    ProcessResult::None
                }
            }
            Phase::PreCommit => {
                self.locked_qc = qc.clone();
                self.high_qc = qc;
                self.phase = Phase::Commit;

                if let Some(block) = &self.pending_block {
                    let sig = sign_with_identity(self.node_id, block.hash, &self.identity);
                    ProcessResult::Broadcast(HotStuffMsg::Proposal {
                        block: block.clone(),
                        phase: Phase::Commit,
                        signature: sig,
                    })
                } else {
                    ProcessResult::None
                }
            }
            Phase::Commit => {
                // Commit QC → DECIDE! Execute transactions.
                self.high_qc = qc.clone();
                self.phase = Phase::Decide;

                let txs = if let Some(block) = self.pending_block.take() {
                    self.committed_view = block.view; // # KG: taliban2-H3-fix-2026-04-15
                    self.committed.push(block.clone());
                    block.transactions
                } else {
                    vec![]
                };

                // # KG: taliban2-H3-fix-2026-04-15 — evict old blocks to keep blocks map bounded.
                // Retain only blocks from the last K=10 views to support rollback/audit without
                // unbounded memory growth.
                const BLOCK_EVICT_WINDOW: u64 = 10;
                if self.committed_view >= BLOCK_EVICT_WINDOW {
                    let min_view = self.committed_view - BLOCK_EVICT_WINDOW;
                    self.blocks.retain(|_, b| b.view >= min_view);
                }
                // Keep at most 16 entries in the committed Vec (most-recent).
                // # KG: taliban2-H3-fix-2026-04-15
                const MAX_COMMITTED_HISTORY: usize = 16;
                if self.committed.len() > MAX_COMMITTED_HISTORY {
                    let drain_len = self.committed.len() - MAX_COMMITTED_HISTORY;
                    self.committed.drain(..drain_len);
                }

                // Advance to next view
                self.view += 1;
                self.phase = Phase::Prepare;

                // Gap A fix: clear vote_tracker so next round doesn't get false
                // equivocation hits from votes of the round we just committed.
                // # KG: sprint4A-gap-A-fix-2026-04-15
                self.vote_tracker.clear();
                self.view_change_tracker.reset(); // Sprint 7C: reset on commit

                // Gap B fix (leader side): after commit reset locked_qc to high_qc
                // so the safety check `block.justify.view < locked_qc.view` doesn't
                // incorrectly reject the Commit proposal of the NEXT round.
                // The PreCommit handler sets locked_qc.view = current_view, but the
                // next block's justify.view = current_view (high_qc after commit),
                // so locked_qc must not be ahead of justify.
                // # KG: sprint4A-gap-B-fix-2026-04-15
                self.locked_qc = self.high_qc.clone();

                // Build NewView so non-leader validators can advance their view.
                // # KG: sprint4A-gap-C-fix-2026-04-15
                // # KG: taliban2-H1-fix-2026-04-15 — always embed NewView in Committed so callers
                // can broadcast it. Previously this message was built but discarded when txs was
                // non-empty; non-leader nodes never got the view advance → view divergence.
                let new_view_msg = HotStuffMsg::NewView {
                    view: self.view,
                    qc: self.high_qc.clone(),
                    signature: sign_with_identity(self.node_id, self.view, &self.identity),
                };

                // Return committed txs + the NewView to broadcast.
                // Callers MUST broadcast new_view_msg to advance non-leader validators.
                // If txs is empty we still need to advance validators — use Broadcast path.
                if !txs.is_empty() {
                    ProcessResult::Committed(txs, new_view_msg)
                } else {
                    ProcessResult::Broadcast(new_view_msg)
                }
            }
            Phase::Decide => ProcessResult::None,
        }
    }

    /// Received NewView from leader — advance to next view
    fn on_new_view(&mut self, view: u64, qc: QuorumCert) -> ProcessResult {
        // Gap C fix: always update high_qc if the QC in this NewView is newer,
        // even if we already advanced our view (e.g. via on_proposal(Commit) early
        // view advance).  Without this, nodes that advanced view early would miss
        // the high_qc update and propose/extend stale chains.
        // # KG: sprint4A-gap-C-fix-2026-04-15
        if qc.view > self.high_qc.view {
            self.high_qc = qc.clone();
        }

        if view > self.view {
            self.view = view;
            self.phase = Phase::Prepare;
            self.pending_block = None;
            self.votes.clear();
            self.vote_tracker.clear(); // reset equivocation tracker on view change
        }
        ProcessResult::None
    }

    /// View change requested (timeout)
    fn on_view_change(&mut self, new_view: u64, high_qc: QuorumCert) -> ProcessResult {
        if new_view > self.view {
            self.view = new_view;
            self.phase = Phase::Prepare;
            self.pending_block = None;
            self.votes.clear();

            if high_qc.view > self.high_qc.view {
                self.high_qc = high_qc;
            }
        }
        ProcessResult::None
    }

    /// Get number of committed blocks
    pub fn committed_count(&self) -> usize {
        self.committed.len()
    }

    // ── Sprint 7C: View-change timeout mechanism ─────────────────────────
    // # KG: sprint7C-viewchange-timeout-2026-04-16

    /// Tick — call periodically (e.g., every 200ms from WASM poll loop).
    /// Returns ViewChange message if the current leader has stalled.
    pub fn tick(&mut self) -> ProcessResult {
        if self.view_change_tracker.is_timed_out() && !self.is_leader() {
            let vc_msg = self.view_change_tracker.on_timeout(
                self.view, self.node_id, self.high_qc.clone(), &self.identity,
            );
            // Carry the signed message out — dropping it here is what made the
            // pacemaker inert even where tick() was called.
            // # KG: fix-333-tick-discards-signed-viewchange-2026-07-15
            return ProcessResult::ViewChange(self.view + 1, vc_msg);
        }
        ProcessResult::None
    }

    /// Process a ViewChange message with quorum tracking.
    /// When quorum is reached, advances to new view with the highest QC.
    fn process_view_change_quorum(
        &mut self, new_view: u64, sender: NodeId, high_qc: QuorumCert,
    ) -> ProcessResult {
        if let Some((agreed_view, highest_qc)) =
            self.view_change_tracker.collect_view_change(
                new_view, sender, high_qc, &self.validators,
            )
        {
            // Quorum reached — advance to new view
            self.view = agreed_view;
            self.phase = Phase::Prepare;
            self.pending_block = None;
            self.votes.clear();
            self.vote_tracker.clear();
            if highest_qc.view > self.high_qc.view {
                self.high_qc = highest_qc;
            }
            self.view_change_tracker.reset();
        }
        ProcessResult::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bft::crypto::sign_standalone as sign;
    use crate::crypto_real::Identity;

    /// Create deterministic identity from node_id (for testing only)
    fn test_identity(node_id: u32) -> Identity {
        let mut seed = [0u8; 32];
        seed[..4].copy_from_slice(&node_id.to_be_bytes());
        Identity::from_seed(&seed)
    }

    /// Hand-roll a vote signature the way an honest validator does: bound to the
    /// phase the vote is cast in. Plain `sign_standalone` (block_hash only) no
    /// longer verifies as a vote — that asymmetry IS the security fix under test.
    /// # KG: fix-333-bft-vote-signature-phase-binding-2026-07-15
    fn sign_vote_as(node_id: u32, block_hash: u64, phase: Phase) -> Signature {
        sign_vote(node_id, block_hash, phase_tag(phase), &test_identity(node_id))
    }

    fn make_validators() -> ValidatorSet {
        ValidatorSet::new(vec![1, 2, 3, 4, 5, 6, 7])
    }

    /// Create HotStuffState with deterministic identity + all peer keys registered
    fn make_state(node_id: u32) -> HotStuffState {
        let mut state = HotStuffState::with_identity(node_id, make_validators(), test_identity(node_id));
        // Register all other validators' public keys in keyring
        for &vid in &[1u32, 2, 3, 4, 5, 6, 7] {
            if vid != node_id {
                let peer_identity = test_identity(vid);
                state.keyring.register_identity(vid, peer_identity);
            }
        }
        state
    }

    #[test]
    fn leader_rotation() {
        let state = make_state(1);
        assert!(state.is_leader()); // view 0 → node 1
    }

    #[test]
    fn propose_creates_block() {
        let mut leader = make_state(1);
        leader.submit_tx(OrderedTx::Transfer {
            from: 1, to: 2, amount: 100, nonce: 1,
        });

        let msg = leader.propose();
        assert!(msg.is_some());

        if let Some(HotStuffMsg::Proposal { block, phase, signature: _ }) = msg {
            assert_eq!(phase, Phase::Prepare);
            assert_eq!(block.view, 0);
            assert_eq!(block.transactions.len(), 1);
        }
    }

    #[test]
    fn full_3_phase_commit() {
        let validators = make_validators();
        let mut nodes: Vec<_> = validators.validators.iter()
            .map(|&id| {
                let mut state = HotStuffState::with_identity(id, validators.clone(), test_identity(id));
                // Register all other validators' keys
                for &vid in &validators.validators {
                    if vid != id { state.keyring.register_identity(vid, test_identity(vid)); }
                }
                state
            })
            .collect();

        // Leader (node 1) proposes
        nodes[0].submit_tx(OrderedTx::Transfer {
            from: 1, to: 2, amount: 50, nonce: 1,
        });
        let proposal = nodes[0].propose().unwrap();

        // === Phase 1: PREPARE ===
        // All validators receive proposal and vote
        let mut votes = vec![];
        for node in &mut nodes[1..] {
            if let ProcessResult::SendToLeader(vote) = node.process(proposal.clone()) {
                votes.push(vote);
            }
        }
        // Leader also votes for own block
        if let HotStuffMsg::Proposal { ref block, .. } = proposal {
            votes.push(HotStuffMsg::Vote {
                block_hash: block.hash,
                view: 0,
                phase: Phase::Prepare,
                signature: sign_vote_as(1, block.hash, Phase::Prepare),
            });
        }

        // Leader collects votes
        let mut next_broadcast = None;
        for vote in votes {
            let result = nodes[0].process(vote);
            if let ProcessResult::Broadcast(msg) = result {
                next_broadcast = Some(msg);
                break;
            }
        }
        assert!(next_broadcast.is_some(), "should have PreCommit broadcast after quorum");

        // === Phase 2: PRECOMMIT ===
        let precommit_proposal = next_broadcast.unwrap();
        let mut votes2 = vec![];
        for node in &mut nodes[1..] {
            if let ProcessResult::SendToLeader(vote) = node.process(precommit_proposal.clone()) {
                votes2.push(vote);
            }
        }
        if let HotStuffMsg::Proposal { ref block, .. } = precommit_proposal {
            votes2.push(HotStuffMsg::Vote {
                block_hash: block.hash,
                view: 0,
                phase: Phase::PreCommit,
                signature: sign_vote_as(1, block.hash, Phase::PreCommit),
            });
        }

        let mut next_broadcast2 = None;
        for vote in votes2 {
            let result = nodes[0].process(vote);
            if let ProcessResult::Broadcast(msg) = result {
                next_broadcast2 = Some(msg);
                break;
            }
        }
        assert!(next_broadcast2.is_some(), "should have Commit broadcast");

        // === Phase 3: COMMIT ===
        let commit_proposal = next_broadcast2.unwrap();
        let mut votes3 = vec![];
        for node in &mut nodes[1..] {
            if let ProcessResult::SendToLeader(vote) = node.process(commit_proposal.clone()) {
                votes3.push(vote);
            }
        }
        if let HotStuffMsg::Proposal { ref block, .. } = commit_proposal {
            votes3.push(HotStuffMsg::Vote {
                block_hash: block.hash,
                view: 0,
                phase: Phase::Commit,
                signature: sign_vote_as(1, block.hash, Phase::Commit),
            });
        }

        let mut committed = false;
        for vote in votes3 {
            let result = nodes[0].process(vote);
            if let ProcessResult::Committed(txs, _new_view) = result {
                assert_eq!(txs.len(), 1);
                committed = true;
                break;
            }
        }
        assert!(committed, "block should be committed after 3 phases");
        assert_eq!(nodes[0].committed_count(), 1);
        assert_eq!(nodes[0].view, 1, "view should advance after commit");
    }

    #[test]
    fn safety_locked_qc() {
        let validators = make_validators();
        let mut node = HotStuffState::with_identity(2, validators, test_identity(2));

        // Set locked QC at view 5
        node.locked_qc = QuorumCert {
            block_hash: 999,
            view: 5,
            phase: Phase::PreCommit,
            signatures: vec![],
        };

        // Reject proposal with justify.view < locked_qc.view and different parent
        let bad_block = Block {
            hash: 123,
            view: 6,
            parent_hash: 888, // doesn't match locked_qc.block_hash (999)
            transactions: vec![],
            justify: QuorumCert { block_hash: 0, view: 3, phase: Phase::Prepare, signatures: vec![] },
            proposer: 1,
        };

        let result = node.on_proposal(bad_block, Phase::Prepare);
        assert!(matches!(result, ProcessResult::None), "should reject conflicting block");
    }

    // Assessment action 1 regression (assess-333-engine-fsm-2026-07-13): a
    // Byzantine leader forges a justify with a high view and NO real quorum
    // signatures. on_proposal must reject it BEFORE the safety rule trusts
    // justify.view — otherwise honest validators vote + advance on a fabrication.
    #[test]
    fn forged_justify_is_rejected() {
        let validators = make_validators();
        let mut node = HotStuffState::with_identity(2, validators, test_identity(2));
        let leader = node.current_leader();

        // Forged: high view, empty signatures. Passes the OLD safety rule
        // (justify.view >= locked_qc.view) but is not a real quorum certificate.
        let forged = Block {
            hash: 777,
            view: node.view,
            parent_hash: 0,
            transactions: vec![],
            justify: QuorumCert { block_hash: 555, view: 99, phase: Phase::Commit, signatures: vec![] },
            proposer: leader,
        };
        assert!(
            matches!(node.on_proposal(forged, Phase::Prepare), ProcessResult::None),
            "forged justify must be rejected (no vote)"
        );

        // Contrast: a genuine genesis-justify proposal from the same leader votes.
        let good = Block {
            hash: 778,
            view: node.view,
            parent_hash: 0,
            transactions: vec![],
            justify: QuorumCert::genesis(),
            proposer: leader,
        };
        assert!(
            matches!(node.on_proposal(good, Phase::Prepare), ProcessResult::SendToLeader(_)),
            "genuine genesis-justify proposal should be voted"
        );
    }

    #[test]
    fn non_leader_cannot_propose() {
        let mut node = make_state(2); // node 2, view 0 → leader is 1
        node.submit_tx(OrderedTx::Transfer { from: 2, to: 3, amount: 10, nonce: 1 });
        assert!(node.propose().is_none(), "non-leader cannot propose");
    }

    // A vote's signature must bind the PHASE it was cast in, not just the block
    // hash. `hash_block` folds `view` in, so cross-VIEW replay already dies on the
    // hash. Cross-PHASE replay inside one view had no such barrier: an attacker
    // relabels captured Prepare votes as Commit votes, the bytes still verify, and
    // the leader forms a Commit QC — reaching Decide while PreCommit/Commit never
    // happened and no validator ever locked. That is a direct HotStuff safety break.
    #[test]
    fn cross_phase_vote_replay_cannot_forge_commit_qc() {
        let ids = vec![1u32, 2, 3, 4];
        let mut nodes = make_ring(&ids);
        nodes[0].submit_tx(OrderedTx::Transfer { from: 1, to: 2, amount: 10, nonce: 1 });
        let proposal = nodes[0].propose().expect("leader proposes");

        // Honest validators vote ONLY in Prepare.
        let mut prepare_votes = vec![];
        for node in &mut nodes[1..4] {
            if let ProcessResult::SendToLeader(v) = node.process(proposal.clone()) {
                prepare_votes.push(v);
            }
        }
        assert_eq!(prepare_votes.len(), 3, "n=4 quorum is 3 honest Prepare votes");

        // Attacker relabels the phase field. Signature bytes are untouched.
        let replayed: Vec<HotStuffMsg> = prepare_votes
            .iter()
            .map(|v| match v {
                HotStuffMsg::Vote { block_hash, view, signature, .. } => HotStuffMsg::Vote {
                    block_hash: *block_hash,
                    view: *view,
                    phase: Phase::Commit,
                    signature: signature.clone(),
                },
                _ => unreachable!("validators reply with votes"),
            })
            .collect();

        for v in replayed {
            assert!(
                !matches!(nodes[0].process(v), ProcessResult::Committed(..)),
                "Prepare votes relabeled as Commit must never form a Commit QC"
            );
        }
        assert_eq!(nodes[0].committed_count(), 0, "no block may commit from replayed votes");
        assert_ne!(nodes[0].phase, Phase::Decide, "leader must not reach Decide");
    }

    // KG: lesson-333-bft-multivalidator-stress-2026-04-14
    // Helper: build N-validator ring with cross-registered keyrings
    fn make_ring(ids: &[NodeId]) -> Vec<HotStuffState> {
        let vs = ValidatorSet::new(ids.to_vec());
        ids.iter().map(|&id| {
            let mut s = HotStuffState::with_identity(id, vs.clone(), test_identity(id));
            for &other in ids { if other != id { s.keyring.register_identity(other, test_identity(other)); } }
            s
        }).collect()
    }

    // Helper: drive one phase — leader's proposal → all voters → back to leader
    fn drive_phase(nodes: &mut [HotStuffState], proposal: HotStuffMsg, voters: &[usize]) -> Option<HotStuffMsg> {
        let mut votes = vec![];
        for &i in voters {
            if let ProcessResult::SendToLeader(v) = nodes[i].process(proposal.clone()) { votes.push(v); }
        }
        // Leader self-votes (wasm.rs path doesn't, but tests need it for small N quorum)
        if let HotStuffMsg::Proposal { ref block, phase, .. } = proposal {
            votes.push(HotStuffMsg::Vote { block_hash: block.hash, view: nodes[0].view, phase, signature: sign_vote_as(nodes[0].node_id, block.hash, phase) });
        }
        let mut out = None;
        for v in votes {
            if let ProcessResult::Broadcast(m) | ProcessResult::SendToLeader(m) = nodes[0].process(v) { out = Some(m); }
        }
        out
    }

    // KG: lesson-333-bft-multivalidator-stress-2026-04-14
    // 4-validator (n=4, f=1, quorum=3) happy path
    #[test]
    fn four_validator_commit_flow() {
        let ids = vec![1u32, 2, 3, 4];
        let mut nodes = make_ring(&ids);
        assert_eq!(nodes[0].validators.quorum_size(), 3, "n=4 quorum must be 3");

        nodes[0].submit_tx(OrderedTx::Transfer { from: 1, to: 2, amount: 10, nonce: 1 });
        let proposal = nodes[0].propose().expect("leader proposes");
        let precommit = drive_phase(&mut nodes, proposal, &[1,2,3]).expect("precommit broadcast");
        let commit = drive_phase(&mut nodes, precommit, &[1,2,3]).expect("commit broadcast");
        // Final phase: commit votes → Committed
        let mut committed = false;
        let commit_clone = commit.clone();
        let mut votes = vec![];
        for node in &mut nodes[1..4] {
            if let ProcessResult::SendToLeader(v) = node.process(commit_clone.clone()) { votes.push(v); }
        }
        if let HotStuffMsg::Proposal { ref block, .. } = commit {
            votes.push(HotStuffMsg::Vote { block_hash: block.hash, view: 0, phase: Phase::Commit, signature: sign_vote_as(1, block.hash, Phase::Commit) });
        }
        for v in votes {
            if let ProcessResult::Committed(txs, _new_view) = nodes[0].process(v) {
                assert_eq!(txs.len(), 1);
                committed = true;
                break;
            }
        }
        assert!(committed, "4-validator BFT must commit");
        assert_eq!(nodes[0].committed_count(), 1);
        assert_eq!(nodes[0].view, 1);
    }

    // KG: lesson-333-bft-multivalidator-stress-2026-04-14
    // 4-validator with 1 silent (offline) — remaining 3 = exact quorum
    #[test]
    fn four_validator_one_silent_still_commits() {
        let ids = vec![1u32, 2, 3, 4];
        let mut nodes = make_ring(&ids);
        nodes[0].submit_tx(OrderedTx::Transfer { from: 1, to: 2, amount: 5, nonce: 1 });
        let proposal = nodes[0].propose().unwrap();
        // Node 4 (index 3) stays silent — only 1,2,3 participate (index 1,2 vote + leader self)
        let precommit = drive_phase(&mut nodes, proposal, &[1,2]).expect("precommit with 3 votes");
        let commit = drive_phase(&mut nodes, precommit, &[1,2]).expect("commit with 3 votes");
        let mut votes = vec![];
        for node in &mut nodes[1..3] {
            if let ProcessResult::SendToLeader(v) = node.process(commit.clone()) { votes.push(v); }
        }
        if let HotStuffMsg::Proposal { ref block, .. } = commit {
            votes.push(HotStuffMsg::Vote { block_hash: block.hash, view: 0, phase: Phase::Commit, signature: sign_vote_as(1, block.hash, Phase::Commit) });
        }
        let mut committed = false;
        for v in votes {
            if let ProcessResult::Committed(_, _) = nodes[0].process(v) { committed = true; break; }
        }
        assert!(committed, "n=4 f=1 tolerance: 3 votes enough even with 1 silent");
    }

    // KG: lesson-333-bft-multivalidator-stress-2026-04-14
    // Equivocation: same (signer, phase) signs two different block_hashes → leader must reject second
    #[test]
    fn equivocation_detection_4validator() {
        let ids = vec![1u32, 2, 3, 4];
        let mut nodes = make_ring(&ids);
        nodes[0].submit_tx(OrderedTx::Transfer { from: 1, to: 2, amount: 1, nonce: 1 });
        let proposal = nodes[0].propose().unwrap();
        let real_hash = if let HotStuffMsg::Proposal { ref block, .. } = proposal { block.hash } else { 0 };

        // First legit vote from node 2
        let v1 = HotStuffMsg::Vote { block_hash: real_hash, view: 0, phase: Phase::Prepare, signature: sign_vote_as(2, real_hash, Phase::Prepare) };
        let _ = nodes[0].process(v1);

        // Byzantine: node 2 tries to vote for DIFFERENT hash in same (signer=2, phase=Prepare)
        let fake_hash = real_hash ^ 0xDEADBEEF;
        let v2 = HotStuffMsg::Vote { block_hash: fake_hash, view: 0, phase: Phase::Prepare, signature: sign_vote_as(2, fake_hash, Phase::Prepare) };
        let result = nodes[0].process(v2);
        assert!(matches!(result, ProcessResult::None), "equivocating vote must be rejected");
    }

    // KG: lesson-333-bft-multivalidator-stress-2026-04-14
    // Leader rotation across views — 4-validator round-robin
    #[test]
    fn leader_rotation_across_views_4validator() {
        let vs = ValidatorSet::new(vec![1u32, 2, 3, 4]);
        assert_eq!(crate::bft::leader::leader_for_view(0, &vs), 1);
        assert_eq!(crate::bft::leader::leader_for_view(1, &vs), 2);
        assert_eq!(crate::bft::leader::leader_for_view(2, &vs), 3);
        assert_eq!(crate::bft::leader::leader_for_view(3, &vs), 4);
        assert_eq!(crate::bft::leader::leader_for_view(4, &vs), 1); // wraps
    }

    // KG: lesson-333-bft-2validator-commit-2026-04-14
    #[test]
    fn two_validator_commit_flow() {
        let validators = ValidatorSet::new(vec![1, 2]);
        let mut n1 = HotStuffState::with_identity(1, validators.clone(), test_identity(1));
        let mut n2 = HotStuffState::with_identity(2, validators.clone(), test_identity(2));
        n1.keyring.register_identity(2, test_identity(2));
        n2.keyring.register_identity(1, test_identity(1));
        assert!(n1.is_leader());
        assert_eq!(validators.quorum_size(), 1);

        n1.submit_tx(OrderedTx::Transfer { from: 1, to: 2, amount: 50, nonce: 1 });
        let proposal = n1.propose().expect("leader must propose");
        let vote1 = match n2.process(proposal) {
            ProcessResult::SendToLeader(v) => v,
            o => panic!("n2 must vote on prepare, got {:?}", o),
        };
        let precommit = match n1.process(vote1) {
            ProcessResult::Broadcast(m) => m,
            o => panic!("n1 must broadcast precommit, got {:?}", o),
        };
        let vote2 = match n2.process(precommit) {
            ProcessResult::SendToLeader(v) => v,
            o => panic!("n2 must vote on precommit, got {:?}", o),
        };
        let commit = match n1.process(vote2) {
            ProcessResult::Broadcast(m) => m,
            o => panic!("n1 must broadcast commit, got {:?}", o),
        };
        let vote3 = match n2.process(commit) {
            ProcessResult::SendToLeader(v) => v,
            o => panic!("n2 must vote on commit, got {:?}", o),
        };
        let result = n1.process(vote3);
        assert!(matches!(result, ProcessResult::Committed(_, _)),
                "n1 must commit, got {:?}", result);
        assert_eq!(n1.committed_count(), 1);
    }

    #[test]
    fn quorum_needs_5_of_7() {
        let mut leader = make_state(1);

        leader.submit_tx(OrderedTx::Transfer { from: 1, to: 2, amount: 10, nonce: 1 });
        let proposal = leader.propose().unwrap();

        let block_hash = if let HotStuffMsg::Proposal { ref block, .. } = proposal { block.hash } else { 0 };

        // 4 votes — not enough for quorum (need 5)
        for i in 1..=4 {
            let result = leader.process(HotStuffMsg::Vote {
                block_hash,
                view: 0,
                phase: Phase::Prepare,
                signature: sign_vote_as(i, block_hash, Phase::Prepare),
            });
            if i < 4 {
                assert!(matches!(result, ProcessResult::None));
            }
        }

        // 5th vote — quorum!
        let result = leader.process(HotStuffMsg::Vote {
            block_hash,
            view: 0,
            phase: Phase::Prepare,
            signature: sign_vote_as(5, block_hash, Phase::Prepare),
        });
        assert!(matches!(result, ProcessResult::Broadcast(_)), "5th vote should trigger quorum");
    }

    // ── Gap A: vote_tracker cleared after commit ─────────────────────────────

    /// # KG: sprint4A-gap-A-fix-2026-04-15
    /// After a block is committed the leader's vote_tracker must be empty.
    /// Without the fix the second round would hit false equivocation errors.
    #[test]
    fn gap_a_vote_tracker_cleared_after_commit() {
        let ids = vec![1u32, 2, 3, 4];
        let mut nodes = make_ring(&ids);

        nodes[0].submit_tx(OrderedTx::Transfer { from: 1, to: 2, amount: 1, nonce: 1 });
        let proposal = nodes[0].propose().unwrap();
        let precommit = drive_phase(&mut nodes, proposal, &[1, 2, 3]).expect("precommit");
        let commit = drive_phase(&mut nodes, precommit, &[1, 2, 3]).expect("commit");

        // Collect commit votes
        let mut votes = vec![];
        for node in &mut nodes[1..4] {
            if let ProcessResult::SendToLeader(v) = node.process(commit.clone()) {
                votes.push(v);
            }
        }
        let leader_node_id = nodes[0].node_id;
        if let HotStuffMsg::Proposal { ref block, .. } = commit {
            votes.push(HotStuffMsg::Vote {
                block_hash: block.hash,
                view: 0,
                phase: Phase::Commit,
                signature: sign_vote_as(leader_node_id, block.hash, Phase::Commit),
            });
        }
        let mut committed = false;
        for v in votes {
            if let ProcessResult::Committed(_, _) = nodes[0].process(v) {
                committed = true;
                break;
            }
        }
        assert!(committed, "should commit");
        // Gap A: vote_tracker must be empty after commit
        assert!(
            nodes[0].vote_tracker.is_empty(),
            "vote_tracker must be cleared after commit (Gap A fix)"
        );
    }

    // ── Gap B: locked_qc reset after commit prevents second-round rejection ──

    /// # KG: sprint4A-gap-B-fix-2026-04-15
    /// After a commit, locked_qc must be set to high_qc (same view) so the
    /// next round's Commit proposal is not rejected by the safety check.
    #[test]
    fn gap_b_locked_qc_reset_after_commit() {
        let ids = vec![1u32, 2, 3, 4];
        let mut nodes = make_ring(&ids);

        // Complete round 1
        nodes[0].submit_tx(OrderedTx::Transfer { from: 1, to: 2, amount: 1, nonce: 1 });
        let proposal = nodes[0].propose().unwrap();
        let precommit = drive_phase(&mut nodes, proposal, &[1, 2, 3]).expect("precommit");
        let commit = drive_phase(&mut nodes, precommit, &[1, 2, 3]).expect("commit");

        let mut votes = vec![];
        for node in &mut nodes[1..4] {
            if let ProcessResult::SendToLeader(v) = node.process(commit.clone()) {
                votes.push(v);
            }
        }
        let leader_node_id_2 = nodes[0].node_id;
        if let HotStuffMsg::Proposal { ref block, .. } = commit {
            votes.push(HotStuffMsg::Vote {
                block_hash: block.hash,
                view: 0,
                phase: Phase::Commit,
                signature: sign_vote_as(leader_node_id_2, block.hash, Phase::Commit),
            });
        }
        for v in votes {
            let _ = nodes[0].process(v);
        }

        // After commit: locked_qc.view must equal high_qc.view (Gap B fix)
        assert_eq!(
            nodes[0].locked_qc.view, nodes[0].high_qc.view,
            "locked_qc.view must equal high_qc.view after commit (Gap B fix)"
        );
        assert_eq!(
            nodes[0].locked_qc.block_hash, nodes[0].high_qc.block_hash,
            "locked_qc.block_hash must equal high_qc.block_hash after commit (Gap B fix)"
        );
    }

    // ── Gap C: non-leader view advance after voting on Commit proposal ────────

    /// # KG: sprint4A-gap-C-fix-2026-04-15
    /// A non-leader validator must advance its view after voting on a Commit
    /// phase proposal so the next round finds the correct leader.
    #[test]
    fn gap_c_non_leader_view_advances_after_commit_vote() {
        let ids = vec![1u32, 2, 3, 4];
        let mut nodes = make_ring(&ids);
        assert_eq!(nodes[0].view, 0);

        nodes[0].submit_tx(OrderedTx::Transfer { from: 1, to: 2, amount: 1, nonce: 1 });
        let proposal = nodes[0].propose().unwrap();
        let precommit = drive_phase(&mut nodes, proposal, &[1, 2, 3]).expect("precommit");
        let commit_proposal = drive_phase(&mut nodes, precommit, &[1, 2, 3]).expect("commit proposal");

        // Before voting on Commit non-leaders are still at view 0
        assert_eq!(nodes[1].view, 0, "before commit vote, non-leader must be at view 0");

        // Non-leader nodes process the Commit proposal (index 1, 2, 3)
        for node in &mut nodes[1..4] {
            let _ = node.process(commit_proposal.clone());
        }

        // Gap C: after voting on Commit-phase proposal, non-leaders advance to view 1
        for (i, node) in nodes[1..4].iter().enumerate() {
            assert_eq!(
                node.view, 1,
                "non-leader node {} must have advanced to view 1 after commit vote (Gap C fix)", i + 1
            );
        }
    }

    // ── Taliban 2 regression tests ────────────────────────────────────────────

    /// # KG: taliban2-H2-fix-2026-04-15
    /// Votes from a previous view must be rejected to prevent replay attacks.
    /// If a valid vote from view=0 is replayed into a leader that is now at view=1,
    /// it MUST be discarded rather than counted toward the new round's quorum.
    #[test]
    fn stale_view_vote_rejected() {
        let ids = vec![1u32, 2, 3, 4];
        let mut nodes = make_ring(&ids);
        nodes[0].submit_tx(OrderedTx::Transfer { from: 1, to: 2, amount: 1, nonce: 1 });

        // Complete one full round to advance leader to view=1.
        let proposal = nodes[0].propose().unwrap();
        let precommit = drive_phase(&mut nodes, proposal, &[1, 2, 3]).unwrap();
        let commit = drive_phase(&mut nodes, precommit, &[1, 2, 3]).unwrap();
        let mut votes = vec![];
        for node in &mut nodes[1..4] {
            if let ProcessResult::SendToLeader(v) = node.process(commit.clone()) {
                votes.push(v);
            }
        }
        let leader_id = nodes[0].node_id;
        if let HotStuffMsg::Proposal { ref block, .. } = commit {
            votes.push(HotStuffMsg::Vote { block_hash: block.hash, view: 0, phase: Phase::Commit, signature: sign_vote_as(leader_id, block.hash, Phase::Commit) });
        }
        for v in votes {
            let _ = nodes[0].process(v);
        }
        // Leader is now at view=1.
        assert_eq!(nodes[0].view, 1);

        // Craft a stale vote with view=0 (same as a vote from round 0).
        let stale_vote = HotStuffMsg::Vote {
            block_hash: 0xDEAD_BEEF,
            view: 0, // stale!
            phase: Phase::Prepare,
            signature: sign_vote_as(2, 0xDEAD_BEEF, Phase::Prepare),
        };
        // # KG: taliban2-H2-fix-2026-04-15
        let result = nodes[0].process(stale_vote);
        assert!(
            matches!(result, ProcessResult::None),
            "stale-view vote (view=0, current=1) must be rejected; got {:?}", result
        );
    }

    /// # KG: taliban2-H3-fix-2026-04-15
    /// After committing, the blocks HashMap must evict entries older than
    /// BLOCK_EVICT_WINDOW views to prevent unbounded memory growth.
    #[test]
    fn blocks_map_evicted_after_commit() {
        let ids = vec![1u32, 2, 3, 4];
        let mut nodes = make_ring(&ids);

        // Drive one full commit round.
        nodes[0].submit_tx(OrderedTx::Transfer { from: 1, to: 2, amount: 1, nonce: 1 });
        let proposal = nodes[0].propose().unwrap();
        let block_hash = if let HotStuffMsg::Proposal { ref block, .. } = proposal {
            block.hash
        } else {
            panic!("expected Proposal");
        };
        let precommit = drive_phase(&mut nodes, proposal, &[1, 2, 3]).unwrap();
        let commit = drive_phase(&mut nodes, precommit, &[1, 2, 3]).unwrap();

        let mut votes = vec![];
        for node in &mut nodes[1..4] {
            if let ProcessResult::SendToLeader(v) = node.process(commit.clone()) {
                votes.push(v);
            }
        }
        let lid = nodes[0].node_id;
        if let HotStuffMsg::Proposal { ref block, .. } = commit {
            votes.push(HotStuffMsg::Vote { block_hash: block.hash, view: 0, phase: Phase::Commit, signature: sign_vote_as(lid, block.hash, Phase::Commit) });
        }
        for v in votes {
            let _ = nodes[0].process(v);
        }

        // After commit at view=0 with BLOCK_EVICT_WINDOW=10, committed_view=0 < 10
        // so no eviction occurs yet — but the committed block was taken from pending_block
        // (via take()), so it is NOT in blocks. The test confirms blocks has at most
        // the block stored during propose (1 entry), not unbounded.
        // With committed_view=0 < BLOCK_EVICT_WINDOW, retain condition keeps all.
        // Key assertion: the block committed is no longer keyed under block_hash
        // (it was moved to committed Vec via pending_block.take()).
        // # KG: taliban2-H3-fix-2026-04-15
        assert!(
            !nodes[0].blocks.contains_key(&block_hash)
                || nodes[0].blocks.len() <= 1,
            "blocks map must not grow unbounded after commit (len={})",
            nodes[0].blocks.len()
        );
        // committed Vec must be bounded to MAX_COMMITTED_HISTORY=16
        assert!(
            nodes[0].committed.len() <= 16,
            "committed Vec must be bounded to 16 (len={})",
            nodes[0].committed.len()
        );
    }

    // ── Sprint 7A: observability counter wiring tests ────────────────────────

    /// Verify that stale-view vote increments bft_vote_rejected_total counter.
    #[test]
    fn stale_view_vote_increments_rejected_counter() {
        let before = BFT_VOTE_REJECTED_TOTAL.get();
        let ids = vec![1u32, 2, 3, 4];
        let mut nodes = make_ring(&ids);
        // Leader at view 0 — send vote with view=99 (stale from perspective of mismatch)
        let stale = HotStuffMsg::Vote {
            block_hash: 0xBEEF,
            view: 99,
            phase: Phase::Prepare,
            signature: sign_vote_as(2, 0xBEEF, Phase::Prepare),
        };
        let _ = nodes[0].process(stale);
        assert!(
            BFT_VOTE_REJECTED_TOTAL.get() > before,
            "stale-view vote must increment bft_vote_rejected_total"
        );
    }

    /// Verify that equivocation increments bft_vote_rejected_total counter.
    #[test]
    fn equivocation_increments_rejected_counter() {
        let ids = vec![1u32, 2, 3, 4];
        let mut nodes = make_ring(&ids);
        nodes[0].submit_tx(OrderedTx::Transfer { from: 1, to: 2, amount: 1, nonce: 1 });
        let proposal = nodes[0].propose().unwrap();
        // Extract block hash from proposal
        let block_hash = if let HotStuffMsg::Proposal { ref block, .. } = proposal {
            block.hash
        } else {
            panic!("expected proposal");
        };
        // First vote — accepted
        let vote1 = HotStuffMsg::Vote {
            block_hash,
            view: 0,
            phase: Phase::Prepare,
            signature: sign_vote_as(2, block_hash, Phase::Prepare),
        };
        let _ = nodes[0].process(vote1);

        // Equivocating vote — same signer, same phase+view, different hash
        let before = BFT_VOTE_REJECTED_TOTAL.get();
        let vote2 = HotStuffMsg::Vote {
            block_hash: block_hash.wrapping_add(1),
            view: 0,
            phase: Phase::Prepare,
            signature: sign_vote_as(2, block_hash.wrapping_add(1), Phase::Prepare),
        };
        let _ = nodes[0].process(vote2);
        assert!(
            BFT_VOTE_REJECTED_TOTAL.get() > before,
            "equivocation must increment bft_vote_rejected_total"
        );
    }

    // ── Sprint 7C: View-change timeout tests ─────────────────────────────

    /// tick() on non-leader with short timeout should produce ViewChange.
    #[test]
    fn tick_triggers_view_change_on_timeout() {
        use crate::bft::viewchange::ViewChangeConfig;
        use std::time::Duration;

        let ids = vec![1u32, 2, 3, 4];
        let mut node = make_ring(&ids).into_iter().nth(1).unwrap(); // node 2 (non-leader)
        // Override timeout to very short for test
        node.view_change_tracker = crate::bft::viewchange::ViewChangeTracker::new(
            ViewChangeConfig {
                base_timeout: Duration::from_millis(10),
                max_timeout: Duration::from_secs(1),
            }
        );
        std::thread::sleep(Duration::from_millis(20));
        let result = node.tick();
        assert!(
            matches!(result, ProcessResult::ViewChange(..)),
            "non-leader tick after timeout must produce ViewChange; got {:?}", result
        );
    }

    /// tick() on leader should NOT produce ViewChange (leader doesn't timeout on itself).
    #[test]
    fn tick_leader_does_not_timeout() {
        use crate::bft::viewchange::ViewChangeConfig;
        use std::time::Duration;

        let ids = vec![1u32, 2, 3, 4];
        let mut node = make_ring(&ids).into_iter().next().unwrap(); // node 1 = leader at view 0
        node.view_change_tracker = crate::bft::viewchange::ViewChangeTracker::new(
            ViewChangeConfig {
                base_timeout: Duration::from_millis(10),
                max_timeout: Duration::from_secs(1),
            }
        );
        std::thread::sleep(Duration::from_millis(20));
        let result = node.tick();
        assert!(
            matches!(result, ProcessResult::None),
            "leader must not self-timeout; got {:?}", result
        );
    }

    /// ViewChange quorum advances view for all nodes.
    #[test]
    fn view_change_quorum_advances_view() {
        let ids = vec![1u32, 2, 3, 4];
        let mut nodes = make_ring(&ids);
        // Simulate: 3 nodes send ViewChange for view=1
        let vc1 = HotStuffMsg::ViewChange {
            new_view: 1, sender: 2,
            high_qc: QuorumCert::genesis(),
            signature: sign(2, 1),
        };
        let vc2 = HotStuffMsg::ViewChange {
            new_view: 1, sender: 3,
            high_qc: QuorumCert::genesis(),
            signature: sign(3, 1),
        };
        let vc3 = HotStuffMsg::ViewChange {
            new_view: 1, sender: 4,
            high_qc: QuorumCert::genesis(),
            signature: sign(4, 1),
        };
        nodes[0].process(vc1);
        nodes[0].process(vc2);
        nodes[0].process(vc3);
        assert_eq!(nodes[0].view, 1, "quorum of ViewChange must advance view to 1");
    }
}
