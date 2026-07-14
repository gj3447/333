// KG: SPAN_333_L11_Consensus, plan-333-p2p-os-synthesis-execution-2026-04-18
// 333 P2P OS L11 Consensus — settlement layer trait + BFT-lite reference impl.
//
// Design sources (prom32-333-p2p-2026-04-18):
//   - finding_333_synth_con_sota_d4: Malachite (Tendermint Rust) SOTA, ~780ms finality.
//   - finding_333_synth_con_thr_d5: FLP + partial sync (Dwork-Lynch-Stockmeyer) + f<n/3.
//   - finding_333_synth_con_prt_d6: ConsensusProtocol / SettlementOp / BlockFinality traits.
//   - finding_333_synth_con_pit_d7: equivocation detection, view-change storms, slashing.
//
// This crate ships ONLY the trait surface + an in-memory BFT-lite implementation
// (synchronous rounds, 2-phase prevote+precommit, f<n/3 quorum). Production
// backends (Malachite, HotStuff-2, Tendermint) implement `ConsensusProtocol`
// in a downstream crate. GGRS rollback (game layer) stays separate — it is NOT
// a settlement consensus and is outside this crate's scope.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use identity333::{Keypair, NodeId, Signature};
use thiserror::Error;

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum ConsensusError {
    #[error("not in validator set: {0:?}")]
    NotValidator(NodeId),
    #[error("invalid signature")]
    InvalidSignature,
    #[error("unknown block at height {0}")]
    UnknownBlock(u64),
    #[error("double-sign detected for validator {validator:?} at height {height}")]
    DoubleSign { validator: NodeId, height: u64 },
    #[error("quorum not reached: collected={collected} required={required}")]
    NoQuorum { collected: usize, required: usize },
    #[error("wrong proposer at height {height}: got {got:?}, expected {expected:?}")]
    WrongProposer { height: u64, got: NodeId, expected: NodeId },
    #[error("validator set must be non-empty")]
    EmptyValidatorSet,
}

// ============================================================================
// Core domain types
// ============================================================================

/// Operations that consensus orders. Intentionally small and closed —
/// applications encode richer payloads in the `payload` byte buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlementOp {
    Transfer { from: NodeId, to: NodeId, amount: u64, payload: Vec<u8> },
    AuctionBid { bidder: NodeId, item: String, amount: u64 },
    RankedAction { actor: NodeId, kind: String, rank: u32, payload: Vec<u8> },
}

/// Progressive finality: a block starts Tentative when proposed, moves to
/// Confirmed on 2f+1 prevotes, and Committed on 2f+1 precommits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BlockFinality {
    Tentative = 0,
    Confirmed = 1,
    Committed = 2,
}

/// Canonical block bytes for signing = height || parent_hash || ops_hash.
#[derive(Debug, Clone)]
pub struct Block {
    pub height: u64,
    pub proposer: NodeId,
    pub parent_hash: [u8; 32],
    pub ops: Vec<SettlementOp>,
    pub finality: BlockFinality,
}

impl Block {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(&self.height.to_be_bytes());
        out.extend_from_slice(&self.parent_hash);
        out.extend_from_slice(&ops_hash(&self.ops));
        out
    }

    pub fn hash(&self) -> [u8; 32] {
        // Deterministic placeholder hash: XOR-fold canonical bytes into 32 bytes.
        // Production backend swaps for SHA-256 / BLAKE3. Kept dependency-free here.
        let bytes = self.canonical_bytes();
        let mut out = [0u8; 32];
        for (i, b) in bytes.iter().enumerate() {
            out[i % 32] ^= *b;
        }
        out
    }
}

fn ops_hash(ops: &[SettlementOp]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, op) in ops.iter().enumerate() {
        let tag: u8 = match op {
            SettlementOp::Transfer { .. } => 1,
            SettlementOp::AuctionBid { .. } => 2,
            SettlementOp::RankedAction { .. } => 3,
        };
        out[i % 32] ^= tag.wrapping_add(i as u8);
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VoteKind {
    Prevote,
    Precommit,
}

#[derive(Debug, Clone)]
pub struct Vote {
    pub height: u64,
    pub block_hash: [u8; 32],
    pub voter: NodeId,
    pub kind: VoteKind,
    pub sig: Signature,
}

// ============================================================================
// Validator set (f < n/3)
// ============================================================================

#[derive(Debug, Clone)]
pub struct ValidatorSet {
    validators: Vec<NodeId>,
}

impl ValidatorSet {
    pub fn new(validators: Vec<NodeId>) -> Result<Self, ConsensusError> {
        if validators.is_empty() {
            return Err(ConsensusError::EmptyValidatorSet);
        }
        Ok(Self { validators })
    }

    pub fn contains(&self, id: &NodeId) -> bool {
        self.validators.iter().any(|v| v == id)
    }

    pub fn size(&self) -> usize {
        self.validators.len()
    }

    /// f < n/3 tolerance (finding_333_synth_con_thr_d5, tight Byzantine bound).
    pub fn max_faults(&self) -> usize {
        self.validators.len().saturating_sub(1) / 3
    }

    /// Quorum size 2f+1.
    pub fn quorum(&self) -> usize {
        2 * self.max_faults() + 1
    }

    /// Round-robin leader selection by height.
    pub fn leader(&self, height: u64) -> &NodeId {
        let idx = (height as usize) % self.validators.len();
        &self.validators[idx]
    }
}

// ============================================================================
// ConsensusProtocol trait
// ============================================================================

/// Settlement-layer consensus. Implementations range from BFT-lite (this crate)
/// to full HotStuff/Tendermint/Malachite (downstream). The trait is intentionally
/// tight — one block per height, linear finality progression.
pub trait ConsensusProtocol {
    /// Propose a block at `height`. Only the round-robin leader may propose.
    fn propose(&self, block: Block) -> Result<(), ConsensusError>;

    /// Cast a vote. Implementations detect equivocation (double-sign for the
    /// same (validator, height, kind) with different block_hash).
    fn vote(&self, vote: Vote) -> Result<(), ConsensusError>;

    /// Finalize: advance the block's finality based on collected votes and
    /// return the new level. Idempotent after Committed.
    fn finalize(&self, height: u64) -> Result<BlockFinality, ConsensusError>;

    /// Read-only accessor for an already-proposed block.
    fn block(&self, height: u64) -> Result<Block, ConsensusError>;
}

// ============================================================================
// In-memory BFT-lite reference implementation
// ============================================================================

#[derive(Debug, Default)]
struct HeightState {
    block: Option<Block>,
    // (voter, kind) -> block_hash. Any re-insert with a different hash = double-sign.
    votes: HashMap<(NodeId, VoteKind), [u8; 32]>,
}

#[derive(Debug)]
pub struct InMemoryConsensus {
    validators: ValidatorSet,
    state: Arc<Mutex<BTreeMap<u64, HeightState>>>,
}

impl InMemoryConsensus {
    pub fn new(validators: ValidatorSet) -> Self {
        Self {
            validators,
            state: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn validators(&self) -> &ValidatorSet {
        &self.validators
    }

    /// Helper: sign a vote with the given keypair (mirror of what a real node does).
    /// The signature is bound to canonical bytes = height || block_hash || kind_tag.
    pub fn sign_vote(
        kp: &Keypair,
        height: u64,
        block_hash: [u8; 32],
        kind: VoteKind,
    ) -> Vote {
        let msg = Self::vote_canonical_bytes(height, block_hash, kind);
        Vote {
            height,
            block_hash,
            voter: kp.node_id(),
            kind,
            sig: kp.sign(&msg),
        }
    }

    fn vote_canonical_bytes(height: u64, block_hash: [u8; 32], kind: VoteKind) -> Vec<u8> {
        let mut m = Vec::with_capacity(41);
        m.extend_from_slice(&height.to_be_bytes());
        m.extend_from_slice(&block_hash);
        m.push(match kind {
            VoteKind::Prevote => 1,
            VoteKind::Precommit => 2,
        });
        m
    }

    fn verify_vote(&self, vote: &Vote) -> Result<(), ConsensusError> {
        if !self.validators.contains(&vote.voter) {
            return Err(ConsensusError::NotValidator(vote.voter.clone()));
        }
        let msg = Self::vote_canonical_bytes(vote.height, vote.block_hash, vote.kind);
        Keypair::verify(vote.voter.as_bytes(), &msg, &vote.sig)
            .map_err(|_| ConsensusError::InvalidSignature)?;
        Ok(())
    }

    fn count_votes(&self, height: u64, block_hash: [u8; 32], kind: VoteKind) -> usize {
        let g = self.state.lock().unwrap();
        match g.get(&height) {
            None => 0,
            Some(h) => h
                .votes
                .iter()
                .filter(|((_, k), hash)| *k == kind && **hash == block_hash)
                .count(),
        }
    }
}

impl ConsensusProtocol for InMemoryConsensus {
    fn propose(&self, block: Block) -> Result<(), ConsensusError> {
        if !self.validators.contains(&block.proposer) {
            return Err(ConsensusError::NotValidator(block.proposer.clone()));
        }
        let expected = self.validators.leader(block.height);
        if &block.proposer != expected {
            return Err(ConsensusError::WrongProposer {
                height: block.height,
                got: block.proposer.clone(),
                expected: expected.clone(),
            });
        }
        let mut g = self.state.lock().unwrap();
        let entry = g.entry(block.height).or_default();
        let mut block = block;
        block.finality = BlockFinality::Tentative;
        entry.block = Some(block);
        Ok(())
    }

    fn vote(&self, vote: Vote) -> Result<(), ConsensusError> {
        self.verify_vote(&vote)?;
        let mut g = self.state.lock().unwrap();
        let entry = g.entry(vote.height).or_default();
        let key = (vote.voter.clone(), vote.kind);
        if let Some(existing) = entry.votes.get(&key) {
            if *existing != vote.block_hash {
                return Err(ConsensusError::DoubleSign {
                    validator: vote.voter.clone(),
                    height: vote.height,
                });
            }
            // Idempotent re-vote on the same hash: no-op.
            return Ok(());
        }
        entry.votes.insert(key, vote.block_hash);
        Ok(())
    }

    fn finalize(&self, height: u64) -> Result<BlockFinality, ConsensusError> {
        let (block_hash, current) = {
            let g = self.state.lock().unwrap();
            let h = g.get(&height).ok_or(ConsensusError::UnknownBlock(height))?;
            let block = h.block.as_ref().ok_or(ConsensusError::UnknownBlock(height))?;
            (block.hash(), block.finality)
        };
        let quorum = self.validators.quorum();
        let prevotes = self.count_votes(height, block_hash, VoteKind::Prevote);
        let precommits = self.count_votes(height, block_hash, VoteKind::Precommit);

        let new = if precommits >= quorum {
            BlockFinality::Committed
        } else if prevotes >= quorum {
            BlockFinality::Confirmed
        } else {
            BlockFinality::Tentative
        };

        // Monotonic: never regress.
        let final_level = std::cmp::max(current, new);
        let mut g = self.state.lock().unwrap();
        if let Some(h) = g.get_mut(&height) {
            if let Some(b) = h.block.as_mut() {
                b.finality = final_level;
            }
        }
        Ok(final_level)
    }

    fn block(&self, height: u64) -> Result<Block, ConsensusError> {
        let g = self.state.lock().unwrap();
        g.get(&height)
            .and_then(|h| h.block.clone())
            .ok_or(ConsensusError::UnknownBlock(height))
    }
}

// ============================================================================
// Helpers re-exported for downstream crates
// ============================================================================

pub fn distinct_validators(votes: &[Vote]) -> BTreeSet<NodeId> {
    votes.iter().map(|v| v.voter.clone()).collect()
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn validator_set(n: usize) -> (ValidatorSet, Vec<Keypair>) {
        let kps: Vec<Keypair> = (0..n).map(|_| Keypair::generate()).collect();
        let ids: Vec<NodeId> = kps.iter().map(|k| k.node_id()).collect();
        (ValidatorSet::new(ids).unwrap(), kps)
    }

    fn simple_block(height: u64, proposer: NodeId) -> Block {
        Block {
            height,
            proposer,
            parent_hash: [0u8; 32],
            ops: vec![SettlementOp::Transfer {
                from: Keypair::generate().node_id(),
                to: Keypair::generate().node_id(),
                amount: 42,
                payload: vec![],
            }],
            finality: BlockFinality::Tentative,
        }
    }

    #[test]
    fn quorum_math() {
        let (vs4, _) = validator_set(4);
        assert_eq!(vs4.max_faults(), 1);
        assert_eq!(vs4.quorum(), 3);
        let (vs7, _) = validator_set(7);
        assert_eq!(vs7.max_faults(), 2);
        assert_eq!(vs7.quorum(), 5);
        let (vs10, _) = validator_set(10);
        assert_eq!(vs10.max_faults(), 3);
        assert_eq!(vs10.quorum(), 7);
    }

    #[test]
    fn empty_validator_set_rejected() {
        assert!(matches!(
            ValidatorSet::new(vec![]),
            Err(ConsensusError::EmptyValidatorSet)
        ));
    }

    #[test]
    fn round_robin_leader() {
        let (vs, kps) = validator_set(3);
        let ids: Vec<NodeId> = kps.iter().map(|k| k.node_id()).collect();
        assert_eq!(vs.leader(0), &ids[0]);
        assert_eq!(vs.leader(1), &ids[1]);
        assert_eq!(vs.leader(2), &ids[2]);
        assert_eq!(vs.leader(3), &ids[0]);
    }

    #[test]
    fn propose_wrong_leader_rejected() {
        let (vs, kps) = validator_set(4);
        let c = InMemoryConsensus::new(vs);
        // height=0 leader = kps[0], propose as kps[1]
        let bad = simple_block(0, kps[1].node_id());
        assert!(matches!(
            c.propose(bad),
            Err(ConsensusError::WrongProposer { .. })
        ));
    }

    #[test]
    fn happy_path_4_validators_commits() {
        let (vs, kps) = validator_set(4);
        let c = InMemoryConsensus::new(vs);
        let b = simple_block(0, kps[0].node_id());
        let h = b.hash();
        c.propose(b).unwrap();

        // All 4 prevote
        for kp in &kps {
            c.vote(InMemoryConsensus::sign_vote(kp, 0, h, VoteKind::Prevote))
                .unwrap();
        }
        assert_eq!(c.finalize(0).unwrap(), BlockFinality::Confirmed);

        // All 4 precommit
        for kp in &kps {
            c.vote(InMemoryConsensus::sign_vote(kp, 0, h, VoteKind::Precommit))
                .unwrap();
        }
        assert_eq!(c.finalize(0).unwrap(), BlockFinality::Committed);
    }

    #[test]
    fn commits_with_f_byzantine_silent() {
        // n=4, f=1, quorum=3. 3 honest + 1 silent byzantine = 3 votes.
        let (vs, kps) = validator_set(4);
        let c = InMemoryConsensus::new(vs);
        let b = simple_block(0, kps[0].node_id());
        let h = b.hash();
        c.propose(b).unwrap();
        for kp in &kps[..3] {
            c.vote(InMemoryConsensus::sign_vote(kp, 0, h, VoteKind::Prevote)).unwrap();
            c.vote(InMemoryConsensus::sign_vote(kp, 0, h, VoteKind::Precommit)).unwrap();
        }
        assert_eq!(c.finalize(0).unwrap(), BlockFinality::Committed);
    }

    #[test]
    fn stalls_when_faults_exceed_bound() {
        // n=7, f=2, quorum=5. Only 4 honest vote → stalls at Tentative.
        let (vs, kps) = validator_set(7);
        let c = InMemoryConsensus::new(vs);
        let b = simple_block(0, kps[0].node_id());
        let h = b.hash();
        c.propose(b).unwrap();
        for kp in &kps[..4] {
            c.vote(InMemoryConsensus::sign_vote(kp, 0, h, VoteKind::Prevote)).unwrap();
            c.vote(InMemoryConsensus::sign_vote(kp, 0, h, VoteKind::Precommit)).unwrap();
        }
        assert_eq!(c.finalize(0).unwrap(), BlockFinality::Tentative);
    }

    #[test]
    fn double_sign_rejected() {
        let (vs, kps) = validator_set(4);
        let c = InMemoryConsensus::new(vs);
        let b1 = simple_block(0, kps[0].node_id());
        let h1 = b1.hash();
        c.propose(b1).unwrap();
        c.vote(InMemoryConsensus::sign_vote(&kps[1], 0, h1, VoteKind::Prevote)).unwrap();
        // Second prevote for a DIFFERENT hash at same height = double-sign
        let fake_hash = [9u8; 32];
        let err = c
            .vote(InMemoryConsensus::sign_vote(&kps[1], 0, fake_hash, VoteKind::Prevote))
            .unwrap_err();
        assert!(matches!(err, ConsensusError::DoubleSign { .. }));
    }

    #[test]
    fn non_validator_vote_rejected() {
        let (vs, kps) = validator_set(4);
        let c = InMemoryConsensus::new(vs);
        let b = simple_block(0, kps[0].node_id());
        let h = b.hash();
        c.propose(b).unwrap();
        let outsider = Keypair::generate();
        let err = c
            .vote(InMemoryConsensus::sign_vote(&outsider, 0, h, VoteKind::Prevote))
            .unwrap_err();
        assert!(matches!(err, ConsensusError::NotValidator(_)));
    }

    #[test]
    fn tampered_signature_rejected() {
        let (vs, kps) = validator_set(4);
        let c = InMemoryConsensus::new(vs);
        let b = simple_block(0, kps[0].node_id());
        let h = b.hash();
        c.propose(b).unwrap();
        // Craft a vote whose payload claims hash=h but sig was made over a different hash.
        let other_hash = [3u8; 32];
        let bad_sig = kps[1].sign(&InMemoryConsensus::vote_canonical_bytes(
            0,
            other_hash,
            VoteKind::Prevote,
        ));
        let tampered = Vote {
            height: 0,
            block_hash: h,
            voter: kps[1].node_id(),
            kind: VoteKind::Prevote,
            sig: bad_sig,
        };
        assert!(matches!(c.vote(tampered), Err(ConsensusError::InvalidSignature)));
    }

    #[test]
    fn idempotent_re_vote() {
        let (vs, kps) = validator_set(4);
        let c = InMemoryConsensus::new(vs);
        let b = simple_block(0, kps[0].node_id());
        let h = b.hash();
        c.propose(b).unwrap();
        let v = InMemoryConsensus::sign_vote(&kps[1], 0, h, VoteKind::Prevote);
        c.vote(v.clone()).unwrap();
        c.vote(v.clone()).unwrap(); // duplicate is fine
        c.vote(v).unwrap();
    }

    #[test]
    fn finality_monotonic() {
        let (vs, kps) = validator_set(4);
        let c = InMemoryConsensus::new(vs);
        let b = simple_block(0, kps[0].node_id());
        let h = b.hash();
        c.propose(b).unwrap();
        for kp in &kps {
            c.vote(InMemoryConsensus::sign_vote(kp, 0, h, VoteKind::Prevote)).unwrap();
            c.vote(InMemoryConsensus::sign_vote(kp, 0, h, VoteKind::Precommit)).unwrap();
        }
        assert_eq!(c.finalize(0).unwrap(), BlockFinality::Committed);
        // Finalize again — should stay Committed, never regress.
        assert_eq!(c.finalize(0).unwrap(), BlockFinality::Committed);
    }

    #[test]
    fn unknown_block_returns_error() {
        let (vs, _) = validator_set(4);
        let c = InMemoryConsensus::new(vs);
        assert!(matches!(c.finalize(99), Err(ConsensusError::UnknownBlock(99))));
        assert!(matches!(c.block(99), Err(ConsensusError::UnknownBlock(99))));
    }

    #[test]
    fn settlement_op_variants_roundtrip() {
        let a = Keypair::generate().node_id();
        let b = Keypair::generate().node_id();
        let ops = vec![
            SettlementOp::Transfer { from: a.clone(), to: b.clone(), amount: 1, payload: vec![] },
            SettlementOp::AuctionBid { bidder: a.clone(), item: "x".into(), amount: 2 },
            SettlementOp::RankedAction { actor: b, kind: "move".into(), rank: 3, payload: vec![1,2,3] },
        ];
        let block = Block {
            height: 0,
            proposer: a,
            parent_hash: [0u8; 32],
            ops: ops.clone(),
            finality: BlockFinality::Tentative,
        };
        let h1 = block.hash();
        // Same block → same hash (determinism).
        assert_eq!(h1, block.hash());
        assert_eq!(ops.len(), 3);
    }
}
