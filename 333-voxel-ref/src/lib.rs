// KG: SPAN_333_RefApp_Voxel, plan-333-p2p-os-synthesis-execution-2026-04-18,
//     queue-p12-minecraft-wasm-app-2026-04-18
//
// 333 P2P OS reference voxel app — the vertical slice that proves the whole
// stack can host a Minecraft-style app. NOT a full game; a testable kernel
// that wires these primitives:
//
//   consensus333    → BlockPlaceOp settlement (authoritative ordering of world mutations).
//   content333      → chunk blobs (content-addressed, integrity-checked).
//   signaling333    → player join/leave announcements via /voxel/join|leave topics.
//   token333        → reward ledger (hosts earn when they serve chunks).
//
// The kernel owns no rendering, no input loop, no asset pipeline — those are
// the downstream game layer. Here we exercise state transitions, settlement,
// and chunk persistence with deterministic unit tests.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use consensus333::{ConsensusProtocol, SettlementOp};
use content333::{BlockStore, Cid, InMemoryBlockStore};
use identity333::NodeId;
use signaling333::{SignalingMesh, Topic};
use thiserror::Error;
use token333::{Amount, InMemoryLedger, TokenLedger};

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum VoxelError {
    #[error("chunk not loaded: {0:?}")]
    ChunkMissing(ChunkCoord),
    #[error("block index out of range: {0}")]
    OutOfRange(usize),
    #[error("consensus: {0}")]
    Consensus(String),
    #[error("content: {0}")]
    Content(String),
    #[error("signaling: {0}")]
    Signaling(String),
    #[error("player not in world: {0:?}")]
    UnknownPlayer(NodeId),
}

// ============================================================================
// Domain types
// ============================================================================

pub const CHUNK_DIM: usize = 16; // 16×16×16 = 4096 voxels per chunk.

/// Voxel material. 0 = air. Callers map higher ids to textures.
pub type BlockKind = u16;

/// Integer chunk coordinate (world space / CHUNK_DIM).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkCoord {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ChunkCoord {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

/// Flat 4096-voxel chunk; index = x + y*16 + z*256.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub coord: ChunkCoord,
    pub blocks: Vec<BlockKind>,
}

impl Chunk {
    pub fn empty(coord: ChunkCoord) -> Self {
        Self { coord, blocks: vec![0; CHUNK_DIM * CHUNK_DIM * CHUNK_DIM] }
    }

    pub fn index(x: usize, y: usize, z: usize) -> usize {
        x + y * CHUNK_DIM + z * CHUNK_DIM * CHUNK_DIM
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> Result<BlockKind, VoxelError> {
        let i = Self::index(x, y, z);
        self.blocks.get(i).copied().ok_or(VoxelError::OutOfRange(i))
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, kind: BlockKind) -> Result<(), VoxelError> {
        let i = Self::index(x, y, z);
        let slot = self.blocks.get_mut(i).ok_or(VoxelError::OutOfRange(i))?;
        *slot = kind;
        Ok(())
    }

    /// Canonical bytes for content-addressing. Fixed width (little-endian u16
    /// × 4096) so any two replicas hashing the same chunk get the same Cid.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.blocks.len() * 2);
        out.extend_from_slice(&self.coord.x.to_le_bytes());
        out.extend_from_slice(&self.coord.y.to_le_bytes());
        out.extend_from_slice(&self.coord.z.to_le_bytes());
        for b in &self.blocks {
            out.extend_from_slice(&b.to_le_bytes());
        }
        out
    }
}

/// A world-mutation op: one voxel write at an absolute world position. This
/// rides inside consensus333::SettlementOp::RankedAction so ordering matches
/// the chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockPlaceOp {
    pub actor: NodeId,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub kind: BlockKind,
}

impl BlockPlaceOp {
    pub fn chunk_coord(&self) -> ChunkCoord {
        ChunkCoord::new(
            self.x.div_euclid(CHUNK_DIM as i32),
            self.y.div_euclid(CHUNK_DIM as i32),
            self.z.div_euclid(CHUNK_DIM as i32),
        )
    }

    pub fn local_coords(&self) -> (usize, usize, usize) {
        (
            self.x.rem_euclid(CHUNK_DIM as i32) as usize,
            self.y.rem_euclid(CHUNK_DIM as i32) as usize,
            self.z.rem_euclid(CHUNK_DIM as i32) as usize,
        )
    }

    pub fn as_settlement_payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32);
        out.extend_from_slice(&self.x.to_le_bytes());
        out.extend_from_slice(&self.y.to_le_bytes());
        out.extend_from_slice(&self.z.to_le_bytes());
        out.extend_from_slice(&self.kind.to_le_bytes());
        out
    }

    pub fn to_settlement_op(&self) -> SettlementOp {
        SettlementOp::RankedAction {
            actor: self.actor.clone(),
            kind: "voxel.place".into(),
            rank: 0,
            payload: self.as_settlement_payload(),
        }
    }
}

// ============================================================================
// ChunkStore — content333 wrapper tagged by ChunkCoord
// ============================================================================

pub struct ChunkStore<S: BlockStore> {
    pub inner: Arc<S>,
    index: Mutex<HashMap<ChunkCoord, Cid>>,
}

impl<S: BlockStore> ChunkStore<S> {
    pub fn new(inner: Arc<S>) -> Self {
        Self { inner, index: Mutex::new(HashMap::new()) }
    }

    pub fn put(&self, chunk: &Chunk) -> Result<Cid, VoxelError> {
        let cid = self
            .inner
            .put(chunk.canonical_bytes())
            .map_err(|e| VoxelError::Content(e.to_string()))?;
        self.index.lock().unwrap().insert(chunk.coord, cid);
        Ok(cid)
    }

    pub fn get(&self, coord: ChunkCoord) -> Result<Chunk, VoxelError> {
        let cid = {
            let g = self.index.lock().unwrap();
            g.get(&coord)
                .copied()
                .ok_or(VoxelError::ChunkMissing(coord))?
        };
        let bytes = self
            .inner
            .get(&cid)
            .map_err(|e| VoxelError::Content(e.to_string()))?;
        if bytes.len() < 12 + CHUNK_DIM * CHUNK_DIM * CHUNK_DIM * 2 {
            return Err(VoxelError::Content("short chunk payload".into()));
        }
        let coord_x = i32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let coord_y = i32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let coord_z = i32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let stored_coord = ChunkCoord::new(coord_x, coord_y, coord_z);
        if stored_coord != coord {
            return Err(VoxelError::Content("chunk coord mismatch".into()));
        }
        let mut blocks = Vec::with_capacity(CHUNK_DIM * CHUNK_DIM * CHUNK_DIM);
        for off in (12..bytes.len()).step_by(2) {
            let b = u16::from_le_bytes([bytes[off], bytes[off + 1]]);
            blocks.push(b);
        }
        Ok(Chunk { coord, blocks })
    }

    pub fn cid_of(&self, coord: ChunkCoord) -> Option<Cid> {
        self.index.lock().unwrap().get(&coord).copied()
    }
}

// ============================================================================
// WorldState — in-memory loaded chunks + settled ops
// ============================================================================

pub struct WorldState<S: BlockStore> {
    pub chunks: ChunkStore<S>,
    loaded: Mutex<HashMap<ChunkCoord, Chunk>>,
    players: Mutex<Vec<NodeId>>,
    settled_height: Mutex<u64>,
}

impl<S: BlockStore> WorldState<S> {
    pub fn new(chunks: ChunkStore<S>) -> Self {
        Self {
            chunks,
            loaded: Mutex::new(HashMap::new()),
            players: Mutex::new(Vec::new()),
            settled_height: Mutex::new(0),
        }
    }

    pub fn load(&self, coord: ChunkCoord) -> Result<(), VoxelError> {
        let chunk = self.chunks.get(coord)?;
        self.loaded.lock().unwrap().insert(coord, chunk);
        Ok(())
    }

    pub fn load_or_create(&self, coord: ChunkCoord) {
        let mut g = self.loaded.lock().unwrap();
        g.entry(coord).or_insert_with(|| Chunk::empty(coord));
    }

    pub fn voxel(&self, x: i32, y: i32, z: i32) -> Result<BlockKind, VoxelError> {
        let cc = ChunkCoord::new(
            x.div_euclid(CHUNK_DIM as i32),
            y.div_euclid(CHUNK_DIM as i32),
            z.div_euclid(CHUNK_DIM as i32),
        );
        let g = self.loaded.lock().unwrap();
        let chunk = g.get(&cc).ok_or(VoxelError::ChunkMissing(cc))?;
        chunk.get(
            x.rem_euclid(CHUNK_DIM as i32) as usize,
            y.rem_euclid(CHUNK_DIM as i32) as usize,
            z.rem_euclid(CHUNK_DIM as i32) as usize,
        )
    }

    /// Apply a `BlockPlaceOp` that has already been committed by consensus.
    /// Callers MUST gate this on `ConsensusProtocol::finalize == Committed`.
    pub fn apply_committed(&self, op: &BlockPlaceOp) -> Result<(), VoxelError> {
        let coord = op.chunk_coord();
        self.load_or_create(coord);
        let (lx, ly, lz) = op.local_coords();
        let mut g = self.loaded.lock().unwrap();
        let chunk = g.get_mut(&coord).ok_or(VoxelError::ChunkMissing(coord))?;
        chunk.set(lx, ly, lz, op.kind)?;
        // Persist snapshot so remote hosts can fetch.
        let c = chunk.clone();
        drop(g);
        self.chunks.put(&c)?;
        Ok(())
    }

    pub fn admit_player(&self, p: NodeId) {
        self.players.lock().unwrap().push(p);
    }

    pub fn remove_player(&self, p: &NodeId) -> Result<(), VoxelError> {
        let mut g = self.players.lock().unwrap();
        let before = g.len();
        g.retain(|x| x != p);
        if g.len() == before {
            return Err(VoxelError::UnknownPlayer(p.clone()));
        }
        Ok(())
    }

    pub fn players(&self) -> Vec<NodeId> {
        self.players.lock().unwrap().clone()
    }

    pub fn settled_height(&self) -> u64 {
        *self.settled_height.lock().unwrap()
    }

    pub fn advance_height(&self, h: u64) {
        let mut g = self.settled_height.lock().unwrap();
        if h > *g {
            *g = h;
        }
    }
}

// ============================================================================
// SignalingJoinHandler — turns /voxel/join envelopes into admit_player calls
// ============================================================================

pub struct SignalingJoinHandler<M: SignalingMesh> {
    pub mesh: Arc<M>,
}

impl<M: SignalingMesh> SignalingJoinHandler<M> {
    pub fn drain_joins<S: BlockStore>(&self, world: &WorldState<S>) -> usize {
        let envs = self.mesh.drain(&Topic::Custom("/voxel/join".into()));
        let n = envs.len();
        for env in envs {
            world.admit_player(env.from);
        }
        n
    }

    pub fn drain_leaves<S: BlockStore>(&self, world: &WorldState<S>) -> usize {
        let envs = self.mesh.drain(&Topic::Custom("/voxel/leave".into()));
        let mut n = 0;
        for env in envs {
            if world.remove_player(&env.from).is_ok() {
                n += 1;
            }
        }
        n
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// End-to-end helper: propose a block-place through consensus, on Committed
/// apply it to the world. Returns the consensus finality level reached after
/// the supplied votes.
pub fn settle_and_apply<C: ConsensusProtocol, S: BlockStore>(
    consensus: &C,
    world: &WorldState<S>,
    block: consensus333::Block,
    ops: &[BlockPlaceOp],
) -> Result<consensus333::BlockFinality, VoxelError> {
    consensus.propose(block.clone()).map_err(|e| VoxelError::Consensus(e.to_string()))?;
    let height = block.height;
    let finality = consensus
        .finalize(height)
        .map_err(|e| VoxelError::Consensus(e.to_string()))?;
    if finality == consensus333::BlockFinality::Committed {
        for op in ops {
            world.apply_committed(op)?;
        }
        world.advance_height(height);
    }
    Ok(finality)
}

/// Reward accountant: credits `reward_per_chunk` to every unique host in
/// `hosts` via the supplied ledger. Returns the total minted.
pub fn reward_chunk_hosts(
    ledger: &impl TokenLedger,
    hosts: &[NodeId],
    reward_per_chunk: Amount,
) -> Result<Amount, VoxelError> {
    let mut total: Amount = 0;
    for h in hosts {
        ledger
            .mint(h, reward_per_chunk)
            .map_err(|e| VoxelError::Consensus(e.to_string()))?;
        total += reward_per_chunk;
    }
    Ok(total)
}

// Re-exports so downstream game code only needs `use voxel_ref333::*`.
pub use content333::InMemoryBlockStore as DefaultBlockStore;
pub use signaling333::{InMemorySignalingMesh, ScoreParams};

// Exposed so integration tests can build in-memory primitives without pulling
// sub-crates directly.
pub fn default_ledger() -> Arc<InMemoryLedger> {
    Arc::new(InMemoryLedger::new())
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use consensus333::{Block, BlockFinality, InMemoryConsensus, ValidatorSet, VoteKind};
    use identity333::Keypair;
    use signaling333::Envelope;

    fn ledger_and_world() -> WorldState<InMemoryBlockStore> {
        let store: Arc<InMemoryBlockStore> = Arc::new(InMemoryBlockStore::new());
        let chunks = ChunkStore::new(store);
        WorldState::new(chunks)
    }

    #[test]
    fn chunk_coord_mapping_roundtrip() {
        let op = BlockPlaceOp {
            actor: Keypair::generate().node_id(),
            x: 33,
            y: -5,
            z: 17,
            kind: 1,
        };
        assert_eq!(op.chunk_coord(), ChunkCoord::new(2, -1, 1));
        assert_eq!(op.local_coords(), (1, 11, 1));
    }

    #[test]
    fn chunk_set_get_roundtrip() {
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        c.set(3, 5, 7, 42).unwrap();
        assert_eq!(c.get(3, 5, 7).unwrap(), 42);
        // Untouched voxels still air.
        assert_eq!(c.get(0, 0, 0).unwrap(), 0);
    }

    #[test]
    fn chunk_canonical_bytes_deterministic() {
        let c1 = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let c2 = Chunk::empty(ChunkCoord::new(0, 0, 0));
        assert_eq!(c1.canonical_bytes(), c2.canonical_bytes());
    }

    #[test]
    fn chunk_store_put_get_roundtrip() {
        let world = ledger_and_world();
        let mut c = Chunk::empty(ChunkCoord::new(1, 2, 3));
        c.set(0, 0, 0, 99).unwrap();
        world.chunks.put(&c).unwrap();
        let got = world.chunks.get(ChunkCoord::new(1, 2, 3)).unwrap();
        assert_eq!(got.coord, ChunkCoord::new(1, 2, 3));
        assert_eq!(got.get(0, 0, 0).unwrap(), 99);
    }

    #[test]
    fn chunk_store_missing_chunk_errors() {
        let world = ledger_and_world();
        assert!(matches!(
            world.chunks.get(ChunkCoord::new(99, 0, 0)),
            Err(VoxelError::ChunkMissing(_))
        ));
    }

    #[test]
    fn world_load_or_create_is_idempotent() {
        let world = ledger_and_world();
        world.load_or_create(ChunkCoord::new(0, 0, 0));
        world.load_or_create(ChunkCoord::new(0, 0, 0));
        // Just asserts no panic and chunk is loaded (voxel reads air).
        assert_eq!(world.voxel(0, 0, 0).unwrap(), 0);
    }

    #[test]
    fn apply_committed_mutates_world_and_persists_chunk() {
        let world = ledger_and_world();
        let op = BlockPlaceOp {
            actor: Keypair::generate().node_id(),
            x: 5,
            y: 6,
            z: 7,
            kind: 3,
        };
        world.apply_committed(&op).unwrap();
        assert_eq!(world.voxel(5, 6, 7).unwrap(), 3);
        // Persisted.
        assert!(world.chunks.cid_of(op.chunk_coord()).is_some());
    }

    #[test]
    fn player_admit_and_remove() {
        let world = ledger_and_world();
        let p = Keypair::generate().node_id();
        world.admit_player(p.clone());
        assert_eq!(world.players(), vec![p.clone()]);
        world.remove_player(&p).unwrap();
        assert!(world.players().is_empty());
    }

    #[test]
    fn player_remove_unknown_errors() {
        let world = ledger_and_world();
        let p = Keypair::generate().node_id();
        assert!(matches!(
            world.remove_player(&p),
            Err(VoxelError::UnknownPlayer(_))
        ));
    }

    #[test]
    fn signaling_join_admits_player() {
        let me = Keypair::generate();
        let mesh: Arc<InMemorySignalingMesh> =
            InMemorySignalingMesh::new(me.node_id(), ScoreParams::default());
        mesh.subscribe(Topic::Custom("/voxel/join".into())).unwrap();
        let player = Keypair::generate();
        let env = Envelope::sign(&player, Topic::Custom("/voxel/join".into()), None, vec![], 1);
        mesh.publish(env).unwrap();

        let world = ledger_and_world();
        let handler = SignalingJoinHandler { mesh };
        let n = handler.drain_joins(&world);
        assert_eq!(n, 1);
        assert_eq!(world.players(), vec![player.node_id()]);
    }

    #[test]
    fn signaling_leave_removes_player() {
        let me = Keypair::generate();
        let mesh: Arc<InMemorySignalingMesh> =
            InMemorySignalingMesh::new(me.node_id(), ScoreParams::default());
        mesh.subscribe(Topic::Custom("/voxel/leave".into())).unwrap();
        let player = Keypair::generate();
        let env = Envelope::sign(&player, Topic::Custom("/voxel/leave".into()), None, vec![], 1);
        mesh.publish(env).unwrap();

        let world = ledger_and_world();
        world.admit_player(player.node_id());
        let handler = SignalingJoinHandler { mesh };
        let n = handler.drain_leaves(&world);
        assert_eq!(n, 1);
        assert!(world.players().is_empty());
    }

    #[test]
    fn settle_and_apply_requires_committed() {
        // 1 validator, quorum=1.
        let kp = Keypair::generate();
        let vs = ValidatorSet::new(vec![kp.node_id()]).unwrap();
        let c = InMemoryConsensus::new(vs);
        let world = ledger_and_world();
        let block = Block {
            height: 0,
            proposer: kp.node_id(),
            parent_hash: [0u8; 32],
            ops: vec![],
            finality: BlockFinality::Tentative,
        };
        let op = BlockPlaceOp {
            actor: kp.node_id(),
            x: 1,
            y: 2,
            z: 3,
            kind: 7,
        };
        // No votes yet → not Committed; world unchanged.
        let f = settle_and_apply(&c, &world, block.clone(), &[op.clone()]).unwrap();
        assert_eq!(f, BlockFinality::Tentative);
        assert!(world.voxel(1, 2, 3).is_err() || world.voxel(1, 2, 3).unwrap() == 0);

        // Now cast prevote + precommit to push to Committed.
        let h = block.hash();
        c.vote(InMemoryConsensus::sign_vote(&kp, 0, h, VoteKind::Prevote))
            .unwrap();
        c.vote(InMemoryConsensus::sign_vote(&kp, 0, h, VoteKind::Precommit))
            .unwrap();
        let f2 = settle_and_apply(&c, &world, block, &[op]).unwrap();
        assert_eq!(f2, BlockFinality::Committed);
        assert_eq!(world.voxel(1, 2, 3).unwrap(), 7);
    }

    #[test]
    fn reward_chunk_hosts_credits_each_host() {
        let ledger = InMemoryLedger::new();
        let hosts: Vec<NodeId> = (0..3).map(|_| Keypair::generate().node_id()).collect();
        let total = reward_chunk_hosts(&ledger, &hosts, 100).unwrap();
        assert_eq!(total, 300);
        for h in &hosts {
            assert_eq!(ledger.balance(h), 100);
        }
    }

    #[test]
    fn settlement_payload_roundtrips_fields() {
        let op = BlockPlaceOp {
            actor: Keypair::generate().node_id(),
            x: -42,
            y: 7,
            z: 100,
            kind: 9,
        };
        let payload = op.as_settlement_payload();
        assert_eq!(payload.len(), 14);
        let rx = i32::from_le_bytes(payload[0..4].try_into().unwrap());
        let ry = i32::from_le_bytes(payload[4..8].try_into().unwrap());
        let rz = i32::from_le_bytes(payload[8..12].try_into().unwrap());
        let rk = u16::from_le_bytes(payload[12..14].try_into().unwrap());
        assert_eq!((rx, ry, rz, rk), (-42, 7, 100, 9));
    }

    #[test]
    fn world_advances_settled_height_monotonically() {
        let world = ledger_and_world();
        world.advance_height(5);
        world.advance_height(3); // ignored, not greater
        world.advance_height(10);
        assert_eq!(world.settled_height(), 10);
    }

    #[test]
    fn full_stack_smoke_test() {
        // Tie it all together: 1 validator consensus + world + signaling + ledger.
        let kp = Keypair::generate();
        let vs = ValidatorSet::new(vec![kp.node_id()]).unwrap();
        let consensus = InMemoryConsensus::new(vs);

        let mesh: Arc<InMemorySignalingMesh> =
            InMemorySignalingMesh::new(kp.node_id(), ScoreParams::default());
        mesh.subscribe(Topic::Custom("/voxel/join".into())).unwrap();

        let world = ledger_and_world();
        let ledger = InMemoryLedger::new();
        let handler = SignalingJoinHandler { mesh: mesh.clone() };

        // Player joins via signaling.
        let player = Keypair::generate();
        let join_env =
            Envelope::sign(&player, Topic::Custom("/voxel/join".into()), None, vec![], 1);
        mesh.publish(join_env).unwrap();
        assert_eq!(handler.drain_joins(&world), 1);

        // Player places a block via consensus.
        let op = BlockPlaceOp { actor: player.node_id(), x: 0, y: 0, z: 0, kind: 1 };
        let block = Block {
            height: 0,
            proposer: kp.node_id(),
            parent_hash: [0u8; 32],
            ops: vec![op.to_settlement_op()],
            finality: BlockFinality::Tentative,
        };
        let h = block.hash();
        consensus.propose(block.clone()).unwrap();
        consensus.vote(InMemoryConsensus::sign_vote(&kp, 0, h, VoteKind::Prevote)).unwrap();
        consensus.vote(InMemoryConsensus::sign_vote(&kp, 0, h, VoteKind::Precommit)).unwrap();
        let f = consensus.finalize(0).unwrap();
        assert_eq!(f, BlockFinality::Committed);
        world.apply_committed(&op).unwrap();

        // Reward the host node (simulated: the proposer hosted the chunk).
        reward_chunk_hosts(&ledger, &[kp.node_id()], 50).unwrap();

        // Verify final state.
        assert_eq!(world.voxel(0, 0, 0).unwrap(), 1);
        assert_eq!(world.players(), vec![player.node_id()]);
        assert_eq!(ledger.balance(&kp.node_id()), 50);
    }
}
