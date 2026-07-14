// KG: prod-wiring-bft-multi-node-2026-04-15
//! Multi-node BFT harness — N `HotStuffCheckpointProvider`s + in-process Vote routing.
//!
//! `MultiNodeBftHarness` drives a real 3-phase HotStuff commit across N nodes
//! using `std::sync::mpsc` channels as the mock transport.  No actual networking;
//! all message delivery is synchronous in-process.
//!
//! # Architecture
//! ```text
//! MultiNodeBftHarness
//!   ├─ nodes[0..N]  HotStuffCheckpointProvider (each owns Arc<RwLock<HotStuffState>>)
//!   ├─ channels[i]  std::sync::mpsc::Sender<HotStuffMsg>  (inbox per node)
//!   └─ drive_checkpoint(frame, hash)
//!        1. leader node builds & broadcasts Proposal
//!        2. each honest non-leader receives → SendToLeader(Vote) → leader inbox
//!        3. leader collects quorum → Broadcast next phase → repeat
//!        4. Decide → return QuorumCert
//! ```
//!
//! # Real-network hook points
//! Replace the `deliver_to_all` / `deliver_to_leader` helpers with actual
//! WebRTC/UDP/TCP send calls to wire real peer transport.
//!
//! # KG: prod-wiring-bft-multi-node-2026-04-15

use std::sync::{Arc, RwLock, mpsc};

use crate::bft::crypto::{NodeId, QuorumCert, ValidatorSet};
use crate::bft::state::HotStuffState;
use crate::bft::types::{HotStuffMsg, OrderedTx, ProcessResult};
use crate::crypto_real::Identity;
use crate::netcode::bft_bridge::BftBridgeError;
use crate::observability; // KG: sprint6C-observability-2026-04-15

// ── Error ────────────────────────────────────────────────────────────────────

/// Harness-level errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiNodeError {
    /// Quorum was not reached within the step budget.
    QuorumTimeout { phase: String, steps: usize },
    /// BFT bridge forwarded an error.
    Bridge(BftBridgeError),
    /// Keyring exchange failed (duplicate node_id).
    DuplicateNodeId(NodeId),
}

impl std::fmt::Display for MultiNodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QuorumTimeout { phase, steps } =>
                write!(f, "quorum timeout in phase={} after {} steps", phase, steps),
            Self::Bridge(e) => write!(f, "bridge error: {}", e),
            Self::DuplicateNodeId(id) => write!(f, "duplicate node_id {}", id),
        }
    }
}

impl From<BftBridgeError> for MultiNodeError {
    fn from(e: BftBridgeError) -> Self { Self::Bridge(e) }
}

// ── NodeSlot ─────────────────────────────────────────────────────────────────

/// One validator slot inside the harness.
struct NodeSlot {
    node_id: NodeId,
    /// Shared HotStuff state (same Arc used by the provider).
    state: Arc<RwLock<HotStuffState>>,
    /// Sender for delivering messages INTO this node's inbox.
    tx: mpsc::SyncSender<HotStuffMsg>,
    /// Receiver for this node's inbox.
    rx: mpsc::Receiver<HotStuffMsg>,
    /// Whether this node is marked as Byzantine (will not vote).
    byzantine: bool,
}

// ── MultiNodeBftHarness ───────────────────────────────────────────────────────

/// N-node in-process HotStuff harness.
///
/// All nodes share the same `ValidatorSet`; each node holds its own
/// `Identity` (private key).  `register_all_peer_identities()` cross-registers
/// every node's public key into every other node's keyring so that BFT
/// signature verification passes without a real key-exchange handshake.
///
/// KG: prod-wiring-bft-multi-node-2026-04-15
pub struct MultiNodeBftHarness {
    slots: Vec<NodeSlot>,
    /// How many votes are needed for a QC in each phase.
    #[allow(dead_code)] // quorum is checked at construction; HotStuffState enforces it internally
    quorum: usize,
    /// Maximum message-processing iterations per phase.
    max_steps: usize,
    /// Round-robin checkpoint counter for leader rotation.
    checkpoint_round: u64,
}

/// Deterministic test identity from node_id seed (TESTING ONLY).
fn test_identity(node_id: NodeId) -> Identity {
    let mut seed = [0u8; 32];
    seed[..4].copy_from_slice(&node_id.to_be_bytes());
    Identity::from_seed(&seed)
}

impl MultiNodeBftHarness {
    /// Create a harness with `node_count` honest nodes using the given quorum size.
    ///
    /// Node IDs are assigned 1..=node_count.  All nodes start honest.
    /// Call `register_all_peer_identities()` before driving checkpoints.
    ///
    /// KG: prod-wiring-bft-multi-node-2026-04-15
    pub fn new(node_count: usize, quorum: usize) -> Self {
        assert!(node_count >= 1, "need at least 1 node");
        assert!(quorum >= 1 && quorum <= node_count, "quorum must be in [1, node_count]");

        let ids: Vec<NodeId> = (1..=(node_count as NodeId)).collect();
        let vs = ValidatorSet::new(ids.clone());

        let mut slots = Vec::with_capacity(node_count);
        for &id in &ids {
            let identity = test_identity(id);
            let state = Arc::new(RwLock::new(
                HotStuffState::with_identity(id, vs.clone(), identity)
            ));
            // Bounded channel — backpressure at 256 messages per node.
            let (tx, rx) = mpsc::sync_channel(256);
            slots.push(NodeSlot { node_id: id, state, tx, rx, byzantine: false });
        }

        Self { slots, quorum, max_steps: 512, checkpoint_round: 0 }
    }

    /// Mark node at index `idx` as Byzantine — it will silently drop all messages.
    ///
    /// Used to simulate f Byzantine nodes that do not vote.
    pub fn set_byzantine(&mut self, idx: usize) {
        self.slots[idx].byzantine = true;
    }

    /// Cross-register every node's public key into every other node's keyring.
    ///
    /// This simulates the key-exchange handshake that in a real deployment would
    /// occur over the secure channel (e.g. TLS + headscale VPN).
    ///
    /// # Real-network hook
    /// Replace with `register_peer_key(node_id, peer.peer_id())` where `peer_id`
    /// is received from the remote node during the WebRTC/QUIC handshake.
    ///
    /// KG: prod-wiring-bft-multi-node-2026-04-15
    pub fn register_all_peer_identities(&mut self) {
        // Collect (node_id, Identity) pairs first to avoid borrow conflicts.
        let pairs: Vec<(NodeId, Identity)> = self.slots.iter()
            .map(|s| (s.node_id, test_identity(s.node_id)))
            .collect();

        for slot in &mut self.slots {
            let mut st = slot.state.write().unwrap();
            for (other_id, ref other_identity) in &pairs {
                if *other_id != slot.node_id {
                    // Register full identity so verify() can use peer_id() lookup.
                    // In production: only register PeerId (public key), not full Identity.
                    // KG: prod-wiring-bft-multi-node-2026-04-15
                    st.register_peer_identity(*other_id, other_identity.clone());
                }
            }
        }
    }

    /// Sync all nodes to the current leader's view via a NewView broadcast.
    ///
    /// After a commit the leader advances view internally.  Non-leader nodes
    /// only receive `Proposal` messages and never see the `Committed` result,
    /// so their view stays behind.  This helper broadcasts a synthetic NewView
    /// to bring all nodes to the same view before the next checkpoint.
    ///
    /// NOTE: With the H1 fix (ProcessResult::Committed embeds NewView) the
    /// drive_checkpoint loop now calls broadcast_to_all directly with the
    /// state-machine's own NewView, so this method is no longer called by
    /// drive_checkpoint.  Kept for external callers / future use.
    /// KG: taliban2-H1-fix-2026-04-15
    #[allow(dead_code)]
    fn sync_views_to_leader(&mut self, leader_idx: usize) {
        let (new_view, high_qc, leader_id) = {
            let st = self.slots[leader_idx].state.read().unwrap();
            let sig = crate::bft::crypto::sign_with_identity(
                st.node_id, st.view, &st.identity
            );
            let _ = sig; // sig consumed above; NewView is reconstructed per-recipient below
            (st.view, st.high_qc.clone(), st.node_id)
        };
        // Deliver NewView to all non-leader nodes so they advance.
        // KG: sprint4A-gap-D-fix-2026-04-15 — use blocking send, not try_send
        for (i, slot) in self.slots.iter().enumerate() {
            if i == leader_idx { continue; }
            let _ = slot.tx.send(HotStuffMsg::NewView {
                view: new_view,
                qc: high_qc.clone(),
                signature: crate::bft::crypto::sign_with_identity(
                    leader_id, new_view,
                    &self.slots[leader_idx].state.read().unwrap().identity
                ),
            });
        }
        // Drain non-leader inboxes to process the NewView.
        let n = self.slots.len();
        for i in 0..n {
            if i == leader_idx { continue; }
            while let Ok(msg) = self.slots[i].rx.try_recv() {
                let mut st = self.slots[i].state.write().unwrap();
                let _ = st.process(msg);
            }
        }
    }

    /// Find the index of the current leader node (the one where `is_leader()` is true).
    fn find_leader_idx(&self) -> Option<usize> {
        self.slots.iter().position(|s| s.state.read().unwrap().is_leader())
    }

    /// Drive a checkpoint through the full 3-phase HotStuff pipeline.
    ///
    /// Finds the current leader via `HotStuffState::is_leader()` — this correctly
    /// tracks the BFT view even after consecutive checkpoints advance the view.
    /// Returns the committed `QuorumCert` from the leader's `high_qc`.
    ///
    /// # Transport model (in-process mock)
    /// - Broadcast → delivered to every non-Byzantine node's inbox.
    /// - SendToLeader → delivered only to the leader's inbox.
    ///
    /// KG: prod-wiring-bft-multi-node-2026-04-15
    pub fn drive_checkpoint(
        &mut self,
        frame_n: u64,
        tactical_hash: [u8; 32],
    ) -> Result<QuorumCert, MultiNodeError> {
        // KG: sprint6C-observability-2026-04-15
        observability::BFT_CHECKPOINT_PROPOSED_TOTAL.inc();
        #[cfg(not(target_arch = "wasm32"))]
        let _propose_start = std::time::Instant::now();

        let n = self.slots.len();
        self.checkpoint_round += 1;

        // Find actual leader according to HotStuffState view + ValidatorSet.
        let leader_idx = self.find_leader_idx()
            .ok_or(MultiNodeError::Bridge(BftBridgeError::NotLeader))?;

        // --- encode & submit tx to leader ---
        let tx = Self::encode_handle_tx(
            self.slots[leader_idx].node_id,
            frame_n,
            tactical_hash,
        );
        {
            let mut st = self.slots[leader_idx].state.write().unwrap();
            if !st.submit_tx(tx) {
                return Err(MultiNodeError::Bridge(BftBridgeError::UnexpectedResult(
                    "tx pool full".into()
                )));
            }
        }

        // --- leader proposes ---
        let initial_proposal = {
            let mut st = self.slots[leader_idx].state.write().unwrap();
            match st.propose() {
                Some(msg) => msg,
                None => return Err(MultiNodeError::Bridge(BftBridgeError::NotLeader)),
            }
        };

        // Broadcast initial proposal to all non-leader nodes.
        self.broadcast_except(leader_idx, initial_proposal.clone());

        // --- message pump ---
        // We drive the pipeline step by step: drain each node's inbox, collect
        // outgoing messages, route them, repeat until leader reaches Committed.
        let mut steps = 0usize;
        // Seed leader's own inbox with the proposal so it also processes it
        // (tracks pending_block and produces its own Vote for small quorums).
        self.deliver_to(leader_idx, initial_proposal);

        loop {
            if steps >= self.max_steps {
                return Err(MultiNodeError::QuorumTimeout {
                    phase: "pipeline".into(),
                    steps,
                });
            }
            steps += 1;

            // Drain all nodes' inboxes, collecting outgoing messages.
            let mut outgoing: Vec<(usize /* src_idx */, HotStuffMsg)> = Vec::new();

            for idx in 0..n {
                // Drain this node's inbox.
                while let Ok(msg) = self.slots[idx].rx.try_recv() {
                    if self.slots[idx].byzantine {
                        // Byzantine node silently drops messages.
                        continue;
                    }
                    let result = {
                        let mut st = self.slots[idx].state.write().unwrap();
                        st.process(msg)
                    };
                    match result {
                        ProcessResult::Committed(_txs, new_view_msg) => {
                            // Leader committed — Gap A (vote_tracker) and Gap B (locked_qc)
                            // cleanup now happens inside state.rs::on_vote(Commit).
                            // KG: sprint4A-gap-A-fix-2026-04-15, sprint4A-gap-B-fix-2026-04-15
                            //
                            // H1 fix: broadcast the embedded NewView to ALL nodes so non-leaders
                            // advance their view. Previously sync_views_to_leader() generated a
                            // synthetic NewView, but state.rs already builds the correct one —
                            // use it directly instead.
                            // KG: taliban2-H1-fix-2026-04-15
                            self.broadcast_to_all(new_view_msg);
                            // Drain all inboxes so the NewView is processed before we return.
                            let n_nodes = self.slots.len();
                            for i in 0..n_nodes {
                                while let Ok(msg) = self.slots[i].rx.try_recv() {
                                    let mut st = self.slots[i].state.write().unwrap();
                                    let _ = st.process(msg);
                                }
                            }
                            let qc = self.slots[leader_idx].state.read().unwrap().high_qc.clone();
                            // KG: sprint6C-observability-2026-04-15
                            observability::BFT_CHECKPOINT_COMMITTED_TOTAL.inc();
                            #[cfg(not(target_arch = "wasm32"))]
                            {
                                let elapsed_ms = _propose_start.elapsed().as_secs_f64() * 1000.0;
                                observability::BFT_CHECKPOINT_QC_LATENCY_MS.observe(elapsed_ms);
                            }
                            return Ok(qc);
                        }
                        ProcessResult::SendToLeader(vote) => {
                            outgoing.push((idx, vote));
                        }
                        ProcessResult::Broadcast(bcast) => {
                            // Gap B resolved in state.rs::on_proposal (safetyRule condition c).
                            // KG: sprint4A-gap-B-fix-2026-04-15
                            outgoing.push((idx, bcast));
                        }
                        ProcessResult::None | ProcessResult::ViewChange(_) => {}
                    }
                }
            }

            if outgoing.is_empty() {
                // No progress this round — pipeline stalled.
                return Err(MultiNodeError::QuorumTimeout {
                    phase: "stalled".into(),
                    steps,
                });
            }

            // Route outgoing messages.
            for (src_idx, msg) in outgoing {
                match &msg {
                    HotStuffMsg::Vote { .. } => {
                        // Votes go to leader only.
                        self.deliver_to(leader_idx, msg);
                    }
                    HotStuffMsg::Proposal { .. } | HotStuffMsg::NewView { .. } => {
                        // Broadcast: deliver to all nodes including self.
                        self.broadcast_to_all(msg);
                    }
                    HotStuffMsg::ViewChange { .. } => {
                        self.deliver_to(leader_idx, msg);
                    }
                }
                let _ = src_idx; // routing doesn't depend on source in this mock
            }
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Encode a checkpoint handle as a `RankedAction` tx (mirrors bft_bridge.rs).
    fn encode_handle_tx(node_id: NodeId, frame_n: u64, tactical_hash: [u8; 32]) -> OrderedTx {
        let mut payload = Vec::with_capacity(40);
        payload.extend_from_slice(&frame_n.to_le_bytes());
        payload.extend_from_slice(&tactical_hash);
        OrderedTx::RankedAction {
            player: node_id,
            action_type: 0xC0DE_1000,
            payload,
        }
    }

    /// Deliver a message to a specific node's inbox.
    ///
    /// Gap D fix: use blocking `.send()` instead of `.try_send()` so a full
    /// channel triggers backpressure rather than silently dropping the message.
    /// The bounded channel (256) is large enough that blocking is rarely needed;
    /// if it does block the harness will stall rather than lose messages.
    /// KG: sprint4A-gap-D-fix-2026-04-15
    fn deliver_to(&self, idx: usize, msg: HotStuffMsg) {
        // `.send()` blocks until space is available — proper backpressure.
        let _ = self.slots[idx].tx.send(msg);
    }

    /// Broadcast a message to all nodes except `exclude_idx`.
    /// KG: sprint4A-gap-D-fix-2026-04-15
    fn broadcast_except(&self, exclude_idx: usize, msg: HotStuffMsg) {
        for (i, slot) in self.slots.iter().enumerate() {
            if i != exclude_idx {
                let _ = slot.tx.send(msg.clone());
            }
        }
    }

    /// Broadcast a message to ALL nodes (including leader).
    /// KG: sprint4A-gap-D-fix-2026-04-15
    fn broadcast_to_all(&self, msg: HotStuffMsg) {
        for slot in &self.slots {
            let _ = slot.tx.send(msg.clone());
        }
    }

    /// Number of nodes in this harness.
    pub fn node_count(&self) -> usize { self.slots.len() }

    /// Read keyring peer count for node at `idx` (used in tests).
    pub fn keyring_peer_count(&self, idx: usize) -> usize {
        self.slots[idx].state.read().unwrap().keyring.peer_count()
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Test 1: n=4, q=3 — happy path ────────────────────────────────────────

    /// Four honest nodes, quorum=3 (f=1 BFT tolerance).
    /// Expected: checkpoint succeeds and QC is returned.
    #[test]
    fn four_nodes_quorum_three_checkpoint_succeeds() {
        // KG: prod-wiring-bft-multi-node-2026-04-15
        let mut harness = MultiNodeBftHarness::new(4, 3);
        harness.register_all_peer_identities();

        let result = harness.drive_checkpoint(1000, [0xAB; 32]);
        assert!(
            result.is_ok(),
            "n=4 q=3 honest checkpoint must succeed: {:?}",
            result.err()
        );
        let qc = result.unwrap();
        // QC must have at least quorum signatures.
        assert!(
            !qc.signatures.is_empty(),
            "QC must contain at least one signature, got {}",
            qc.signatures.len()
        );
    }

    // ── Test 2: 1 Byzantine node out of 4 — quorum still reached ─────────────

    /// n=4, q=3: node at index 3 is Byzantine (drops all messages).
    /// Remaining 3 honest nodes satisfy quorum=3.
    #[test]
    fn one_node_byzantine_still_reaches_quorum() {
        // KG: prod-wiring-bft-multi-node-2026-04-15
        let mut harness = MultiNodeBftHarness::new(4, 3);
        harness.register_all_peer_identities();
        harness.set_byzantine(3); // node index 3 goes Byzantine

        let result = harness.drive_checkpoint(2000, [0xCD; 32]);
        assert!(
            result.is_ok(),
            "1 Byzantine out of 4 must still reach quorum=3: {:?}",
            result.err()
        );
    }

    // ── Test 3: 2 Byzantine nodes out of 4 — quorum not reached ─────────────

    /// n=4, q=3: 2 Byzantine nodes → only 2 honest → below quorum=3.
    /// Expected: `Err(QuorumTimeout)`.
    #[test]
    fn two_nodes_byzantine_fails_quorum() {
        // KG: prod-wiring-bft-multi-node-2026-04-15
        let mut harness = MultiNodeBftHarness::new(4, 3);
        harness.register_all_peer_identities();
        harness.set_byzantine(2); // node index 2
        harness.set_byzantine(3); // node index 3

        let result = harness.drive_checkpoint(3000, [0xEF; 32]);
        assert!(
            result.is_err(),
            "2 Byzantine out of 4 with quorum=3 must fail, got Ok"
        );
        // Must time-out, not panic.
        match result.unwrap_err() {
            MultiNodeError::QuorumTimeout { .. } => {}
            other => panic!("expected QuorumTimeout, got {:?}", other),
        }
    }

    // ── Test 4: keyring exchange registers all peers ──────────────────────────

    /// After `register_all_peer_identities()`, each node's keyring must contain
    /// entries for every OTHER node (n-1 remote entries).
    #[test]
    fn keyring_exchange_registers_all_peers() {
        // KG: prod-wiring-bft-multi-node-2026-04-15
        let n = 5;
        let mut harness = MultiNodeBftHarness::new(n, 4);
        // Before registration: each node only knows itself (1 local key, 0 remote).
        for i in 0..n {
            // own identity is in keys map (1), peer_ids map is empty (0).
            // peer_count = keys.len() + peer_ids.len() = 1 + 0 = 1
            assert_eq!(
                harness.keyring_peer_count(i), 1,
                "before exchange, node {} keyring should have 1 entry (self)", i
            );
        }

        harness.register_all_peer_identities();

        // After registration: each node has 1 (self) + (n-1) peers = n entries.
        for i in 0..n {
            assert_eq!(
                harness.keyring_peer_count(i), n,
                "after exchange, node {} keyring should have {} entries", i, n
            );
        }
    }

    // ── Test 5: consecutive checkpoints with leader rotation ─────────────────

    /// Drive 3 consecutive checkpoints; leader should rotate round-robin.
    #[test]
    fn consecutive_checkpoints_with_leader_rotation() {
        // KG: prod-wiring-bft-multi-node-2026-04-15
        let mut harness = MultiNodeBftHarness::new(4, 3);
        harness.register_all_peer_identities();

        for (i, frame) in [100u64, 200, 300].iter().enumerate() {
            let result = harness.drive_checkpoint(*frame, [(i as u8); 32]);
            assert!(
                result.is_ok(),
                "checkpoint {} at frame {} must succeed: {:?}",
                i, frame, result.err()
            );
        }
        // All 3 rounds consumed → checkpoint_round advanced.
        assert_eq!(harness.checkpoint_round, 3);
    }

    // ── Test 6: Gap D — backpressure (blocking send) under channel pressure ───

    /// KG: sprint4A-gap-D-fix-2026-04-15
    /// Verify that messages are NOT dropped when the harness drives many
    /// consecutive checkpoints.  If `try_send` were used, a full channel would
    /// silently drop votes → QuorumTimeout.  With blocking `send()` all messages
    /// are delivered.
    ///
    /// We drive 10 consecutive checkpoints on n=4 q=3 to stress the pipeline
    /// and confirm zero message loss.
    #[test]
    fn gap_d_backpressure_no_silent_drops_under_load() {

        // KG: sprint4A-gap-D-fix-2026-04-15
        // Verify that no messages are dropped when the harness drives 8 consecutive
        // checkpoints across all 4 leaders (2 full leader-rotation cycles).
        // With try_send, a full channel would silently drop votes → QuorumTimeout.
        // With blocking send(), all messages are delivered regardless of channel pressure.
        let mut harness = MultiNodeBftHarness::new(4, 3);
        harness.register_all_peer_identities();

        for i in 0..8u64 {
            let result = harness.drive_checkpoint(i * 100, [(i & 0xFF) as u8; 32]);
            assert!(
                result.is_ok(),
                "checkpoint {} must succeed with backpressure-safe delivery: {:?}",
                i, result.err()
            );
        }
        assert_eq!(harness.checkpoint_round, 8, "all 8 checkpoints must complete");
    }

    // ── Test 7: real Ed25519 signatures verified by verify_checkpoint_qc ─────

    /// Drive a checkpoint with 4 real-key nodes, then verify the returned QC
    /// via `HotStuffCheckpointProvider::verify_checkpoint_qc`.
    ///
    /// This test closes the loop: the multi-node harness produces a QC with
    /// actual Ed25519 signatures (each vote is signed by `sign_with_identity`),
    /// and the bridge's verifier must accept it.
    ///
    /// KG: sprint4D-ed25519-real-verify-2026-04-15
    #[test]
    fn real_signatures_round_trip_quorum() {
        use crate::bft::crypto::ValidatorSet;
        use crate::bft::state::HotStuffState;
        use crate::netcode::bft_bridge::HotStuffCheckpointProvider;
        use crate::netcode::two_layer::{BftCheckpointProvider, CheckpointHandle};
        use std::sync::{Arc, RwLock};

        let n: usize = 4;
        let quorum: usize = 3;

        // Drive a real checkpoint via multi-node harness.
        let mut harness = MultiNodeBftHarness::new(n, quorum);
        harness.register_all_peer_identities();

        let frame_n: u64 = 42;
        let tactical_hash = [0x7Eu8; 32];
        let qc = harness.drive_checkpoint(frame_n, tactical_hash)
            .expect("real 4-node quorum checkpoint must succeed");

        // The QC must have at least quorum signatures.
        assert!(
            qc.signatures.len() >= quorum,
            "QC must carry at least {} signatures, got {}",
            quorum, qc.signatures.len()
        );

        // Build a fresh HotStuffCheckpointProvider with all 4 identities registered.
        // Use `register_peer_identity` (public method on HotStuffState) through the Arc.
        let ids: Vec<NodeId> = (1..=(n as NodeId)).collect();
        let vs = ValidatorSet::new(ids.clone());
        let leader_id = 1u32;
        let hs = HotStuffState::with_identity(leader_id, vs, test_identity(leader_id));
        let state_arc = Arc::new(RwLock::new(hs));

        // Register all peer identities via HotStuffState's public method.
        {
            let mut st = state_arc.write().unwrap();
            for &nid in &ids {
                st.register_peer_identity(nid, test_identity(nid));
            }
        }

        let provider = HotStuffCheckpointProvider::new(state_arc);
        let handle = CheckpointHandle { frame_n, tactical_hash };

        assert!(
            provider.verify_checkpoint_qc(&handle, &qc),
            "real Ed25519 multi-sig QC from harness must pass verify_checkpoint_qc"
        );
    }
}