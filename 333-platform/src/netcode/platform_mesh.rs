// KG: sprint7G-webrtc-transport-2026-04-16, SPAN_333_Transport
// PlatformMesh — N PlatformCore instances connected by BftTransport.
// This is the full-stack analog to MultiNodeBftHarness:
//   MultiNodeBftHarness  → raw HotStuffState (no PlatformCore)
//   PlatformMesh         → full PlatformCore (with kernel, worker queue, etc.)
//
// The mesh uses MpscTransport in tests.
// In production (WASM), the transport is WebRTC DataChannel CH_BFT routed via process_wire().
//
// Architecture:
//   PlatformMesh.tick()
//     for each node:
//       1. drain transport.poll_incoming() → process each via platform.process_consensus()
//       2. route outgoing ProcessResult (Broadcast / SendToLeader / Committed)
//     return committed block count

use crate::bft::crypto::{NodeId, QuorumCert};
use crate::bft::types::{HotStuffMsg, OrderedTx, ProcessResult};
use crate::crypto_real::Identity;
use crate::platform::{PlatformCore, ExecutionRequest, ExecutionResponse};
use super::transport::{BftTransport, MpscTransport, make_mpsc_transports};

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum MeshError {
    QuorumTimeout { steps: usize },
    NotLeader,
    NoProgress,
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeshError::QuorumTimeout { steps } =>
                write!(f, "quorum timeout after {} steps", steps),
            MeshError::NotLeader => write!(f, "no leader found"),
            MeshError::NoProgress => write!(f, "pipeline stalled"),
        }
    }
}

// ── PlatformNode ──────────────────────────────────────────────────────────────

/// KG: sprint7J-solid-dip-fix-2026-04-16 — Box<dyn BftTransport> for DIP compliance
struct PlatformNode {
    platform: PlatformCore,
    transport: Box<dyn BftTransport>,
    byzantine: bool,
}

// ── PlatformMesh ──────────────────────────────────────────────────────────────

/// N connected PlatformCore nodes — full-stack BFT mesh.
/// KG: sprint7G-webrtc-transport-2026-04-16
pub struct PlatformMesh {
    nodes: Vec<PlatformNode>,
    max_steps: usize,
    committed_total: usize,
}

impl PlatformMesh {
    /// Create a mesh with `node_count` honest nodes.
    /// Node IDs: 1..=node_count.
    pub fn new(node_count: usize) -> Self {
        let ids: Vec<NodeId> = (1..=(node_count as NodeId)).collect();
        let mut transports = make_mpsc_transports(&ids);

        let nodes = ids.iter().zip(transports.into_iter()).map(|(&id, transport)| {
            let seed = {
                let mut s = [0u8; 32];
                s[..4].copy_from_slice(&id.to_be_bytes());
                s
            };
            let identity = Identity::from_seed(&seed);
            let platform = PlatformCore::with_identity(id, &ids, identity);
            PlatformNode { platform, transport: Box::new(transport), byzantine: false }
        }).collect();

        let mut mesh = Self { nodes, max_steps: 512, committed_total: 0 };
        mesh.register_all_peer_keys();
        mesh
    }

    /// Mark node at index `idx` as Byzantine (drops all incoming messages).
    pub fn set_byzantine(&mut self, idx: usize) {
        self.nodes[idx].byzantine = true;
    }

    /// Cross-register all peer public keys for Ed25519 signature verification.
    fn register_all_peer_keys(&mut self) {
        // Collect (id, peer_id) pairs
        let pairs: Vec<(NodeId, crate::crypto_real::PeerId)> = self.nodes.iter()
            .map(|n| {
                let seed = {
                    let mut s = [0u8; 32];
                    s[..4].copy_from_slice(&n.platform.node_id.to_be_bytes());
                    s
                };
                let id = n.platform.node_id;
                let peer_id = Identity::from_seed(&seed).peer_id();
                (id, peer_id)
            })
            .collect();

        for node in &mut self.nodes {
            for (other_id, ref peer_id) in &pairs {
                if *other_id != node.platform.node_id {
                    node.platform.consensus.register_peer_key(*other_id, peer_id.clone());
                }
            }
        }
    }

    /// Submit a CRDT update on node `idx` and route the delta to all peers.
    pub fn crdt_update(&mut self, idx: usize, key: impl Into<String>, value: impl Into<String>) -> String {
        let resp = self.nodes[idx].platform.execute(ExecutionRequest::CrdtUpdate {
            key: key.into(),
            value: Some(value.into()),
        });
        match resp {
            ExecutionResponse::CrdtDelta(json) => {
                // Merge into all other nodes
                for i in 0..self.nodes.len() {
                    if i != idx {
                        self.nodes[i].platform.merge_remote_delta(&json);
                    }
                }
                json
            }
            _ => String::new(),
        }
    }

    /// Drive BFT consensus for one transaction across all nodes.
    /// Returns the QuorumCert once committed.
    pub fn drive_bft_round(&mut self, tx: OrderedTx) -> Result<QuorumCert, MeshError> {
        let n = self.nodes.len();

        // Find leader
        let leader_idx = self.find_leader_idx().ok_or(MeshError::NotLeader)?;

        // Submit tx to leader and get proposal (index-based, no active borrow)
        self.nodes[leader_idx].platform.consensus.submit_tx(tx);
        let proposal = self.nodes[leader_idx].platform.consensus.propose()
            .ok_or(MeshError::NotLeader)?;

        // Broadcast proposal to all nodes (via leader's transport)
        self.nodes[leader_idx].transport.broadcast(proposal);

        // Message pump — index-based to avoid borrow conflicts
        let mut steps = 0;
        loop {
            if steps >= self.max_steps {
                return Err(MeshError::QuorumTimeout { steps });
            }
            steps += 1;

            // Phase 1: drain all inboxes, collect outgoing
            // (leader_id re-looked up before each phase to handle view rotation)
            let current_leader_id = self.nodes.iter()
                .find(|n| n.platform.consensus.is_leader())
                .map(|n| n.platform.node_id)
                .unwrap_or(1);

            let mut outgoing: Vec<(usize, HotStuffMsg, bool /* to_leader */)> = Vec::new();
            let mut committed: Option<(QuorumCert, HotStuffMsg)> = None;

            for idx in 0..n {
                let msgs: Vec<HotStuffMsg> = self.nodes[idx].transport.poll_incoming();
                if self.nodes[idx].byzantine { continue; }

                for msg in msgs {
                    let result = self.nodes[idx].platform.consensus.process(msg);
                    match result {
                        ProcessResult::Committed(txs, new_view_msg) => {
                            let _ = self.nodes[idx].platform.executor.execute_block(&txs);
                            let qc = self.nodes[idx].platform.consensus.high_qc.clone();
                            committed = Some((qc, new_view_msg));
                            break; // stop draining this node's inbox after commit
                        }
                        ProcessResult::SendToLeader(vote) => {
                            outgoing.push((idx, vote, true));
                        }
                        ProcessResult::Broadcast(bcast) => {
                            outgoing.push((idx, bcast, false));
                        }
                        ProcessResult::ViewChange(_) | ProcessResult::None => {}
                    }
                }
                if committed.is_some() { break; }
            }

            // Phase 2: handle commit
            if let Some((qc, new_view_msg)) = committed {
                // Broadcast NewView to all nodes
                for idx in 0..n {
                    self.nodes[idx].transport.broadcast(new_view_msg.clone());
                }
                // Drain all inboxes once (process NewView)
                for idx in 0..n {
                    let msgs: Vec<HotStuffMsg> = self.nodes[idx].transport.poll_incoming();
                    for m in msgs {
                        let _ = self.nodes[idx].platform.consensus.process(m);
                    }
                }
                self.committed_total += 1;
                return Ok(qc);
            }

            // Phase 3: route outgoing messages
            if outgoing.is_empty() {
                return Err(MeshError::NoProgress);
            }

            for (src_idx, msg, to_leader) in outgoing {
                if to_leader {
                    self.nodes[src_idx].transport.send_to_leader(current_leader_id, msg);
                } else {
                    self.nodes[src_idx].transport.broadcast(msg);
                }
            }
        }
    }

    fn find_leader_idx(&self) -> Option<usize> {
        self.nodes.iter().position(|n| n.platform.consensus.is_leader())
    }

    /// Total committed BFT rounds across the mesh lifetime.
    pub fn committed_total(&self) -> usize { self.committed_total }

    /// World state at node `idx`.
    pub fn world_size(&self, idx: usize) -> usize {
        self.nodes[idx].platform.world_size()
    }

    /// Get block at key from node `idx`.
    pub fn get_block(&self, idx: usize, key: &str) -> Option<&String> {
        self.nodes[idx].platform.get_block(key)
    }

    /// Kernel healthy on node `idx`?
    pub fn kernel_healthy(&self, idx: usize) -> bool {
        self.nodes[idx].platform.kernel_healthy()
    }

    /// Number of nodes.
    pub fn node_count(&self) -> usize { self.nodes.len() }

    /// Balance for `node_id` at node `idx`.
    pub fn balance(&self, idx: usize, node_id: NodeId) -> u64 {
        self.nodes[idx].platform.balance(node_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_crdt_convergence_4_nodes() {
        let mut mesh = PlatformMesh::new(4);

        // Each node writes a unique key
        for i in 0..4 {
            mesh.crdt_update(i, format!("key_{}", i), format!("val_{}", i));
        }

        // All nodes should have all 4 keys
        for i in 0..4 {
            assert_eq!(mesh.world_size(i), 4, "node {} must have all 4 keys", i);
            for j in 0..4 {
                assert_eq!(
                    mesh.get_block(i, &format!("key_{}", j)),
                    Some(&format!("val_{}", j)),
                    "node {} missing key_{}", i, j
                );
            }
        }
    }

    #[test]
    fn mesh_bft_token_transfer() {
        let mut mesh = PlatformMesh::new(4);

        let tx = OrderedTx::Transfer { from: 1, to: 2, amount: 500, nonce: 0 };
        let qc = mesh.drive_bft_round(tx).expect("BFT round must succeed");
        assert!(!qc.signatures.is_empty());
        assert_eq!(mesh.committed_total(), 1);
    }

    #[test]
    fn mesh_kernel_healthy_on_all_nodes() {
        let mesh = PlatformMesh::new(4);
        for i in 0..4 {
            assert!(mesh.kernel_healthy(i), "node {} kernel not healthy", i);
        }
    }

    #[test]
    fn mesh_bft_consecutive_rounds() {
        let mut mesh = PlatformMesh::new(4);

        for nonce in 0..3u64 {
            let tx = OrderedTx::Transfer { from: 1, to: 2, amount: 100, nonce };
            mesh.drive_bft_round(tx).expect(&format!("round {} failed", nonce));
        }
        assert_eq!(mesh.committed_total(), 3);
    }

    #[test]
    fn mesh_crdt_and_bft_combined() {
        let mut mesh = PlatformMesh::new(4);

        // Mixed: CRDT + BFT in one session
        mesh.crdt_update(0, "world_state", "v1");
        let qc = mesh.drive_bft_round(
            OrderedTx::Transfer { from: 1, to: 3, amount: 200, nonce: 0 }
        ).unwrap();

        assert!(!qc.signatures.is_empty());
        for i in 0..4 {
            assert_eq!(mesh.world_size(i), 1); // CRDT converged
        }
    }

    #[test]
    fn mesh_bft_with_one_byzantine_node() {
        // n=4, f=1: 3 honest nodes satisfy quorum=3
        let mut mesh = PlatformMesh::new(4);
        mesh.set_byzantine(3); // node index 3 (id=4) drops all messages

        let tx = OrderedTx::Transfer { from: 1, to: 2, amount: 100, nonce: 0 };
        let result = mesh.drive_bft_round(tx);
        assert!(result.is_ok(), "1 Byzantine out of 4 must still reach quorum: {:?}", result.err());
    }
}
