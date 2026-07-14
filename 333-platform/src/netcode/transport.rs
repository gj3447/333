// KG: sprint7G-webrtc-transport-2026-04-16, SPAN_333_Transport
// BftTransport trait — transport-agnostic BFT message routing.
// Replaces the hardcoded mpsc delivery helpers in bft_multi_node.rs.
// Implementations:
//   MpscTransport  — in-process (tests, native)
//   WebRtcTransport — real DataChannel CH_BFT (WASM, future)
//
// Design: trait is object-safe so PlatformMesh can hold Box<dyn BftTransport>.
// postcard encoding is done at the call site (not in trait impls) so the trait
// stays encoding-agnostic and can be reused for any binary protocol.

use crate::bft::crypto::NodeId;
use crate::bft::types::HotStuffMsg;
use std::collections::HashMap;
use std::sync::mpsc;

/// Transport-agnostic BFT message routing contract.
/// One `BftTransport` instance exists per node; it knows how to deliver
/// messages to other nodes (broadcast or point-to-point to leader).
pub trait BftTransport: Send {
    /// Broadcast to all known peers (used for Proposal, NewView).
    fn broadcast(&mut self, msg: HotStuffMsg);
    /// Send directly to the current leader (used for Vote, ViewChange).
    fn send_to_leader(&mut self, leader_id: NodeId, msg: HotStuffMsg);
    /// Poll inbox — returns all messages delivered to this node since last poll.
    fn poll_incoming(&mut self) -> Vec<HotStuffMsg>;
    /// This node's own ID.
    fn node_id(&self) -> NodeId;
}

// ── MpscTransport ─────────────────────────────────────────────────────────────

/// In-process BFT transport backed by `std::sync::mpsc`.
/// Used in unit/integration tests as a drop-in for the real WebRTC channel.
///
/// Each node holds one `MpscTransport`; they share a `PeerDirectory` that maps
/// node_id → SyncSender. Sending to a node = pushing to its Sender.
pub struct MpscTransport {
    node_id: NodeId,
    tx: mpsc::SyncSender<HotStuffMsg>,
    rx: mpsc::Receiver<HotStuffMsg>,
    /// Shared registry of all other nodes' senders (cloned Arc).
    peers: std::sync::Arc<std::sync::RwLock<HashMap<NodeId, mpsc::SyncSender<HotStuffMsg>>>>,
}

impl MpscTransport {
    fn deliver_to(&self, target: NodeId, msg: HotStuffMsg) {
        if let Ok(peers) = self.peers.read() {
            if let Some(tx) = peers.get(&target) {
                let _ = tx.send(msg); // blocking send for backpressure safety
            }
        }
    }
}

impl BftTransport for MpscTransport {
    fn broadcast(&mut self, msg: HotStuffMsg) {
        let all_ids: Vec<NodeId> = self.peers.read()
            .map(|p| p.keys().copied().collect())
            .unwrap_or_default();
        // Also deliver to self (leader needs its own messages)
        let _ = self.tx.send(msg.clone());
        for id in all_ids {
            if id != self.node_id {
                self.deliver_to(id, msg.clone());
            }
        }
    }

    fn send_to_leader(&mut self, leader_id: NodeId, msg: HotStuffMsg) {
        if leader_id == self.node_id {
            let _ = self.tx.send(msg);
        } else {
            self.deliver_to(leader_id, msg);
        }
    }

    fn poll_incoming(&mut self) -> Vec<HotStuffMsg> {
        let mut msgs = Vec::new();
        while let Ok(m) = self.rx.try_recv() {
            msgs.push(m);
        }
        msgs
    }

    fn node_id(&self) -> NodeId { self.node_id }
}

/// Factory: create N connected `MpscTransport` instances (one per node).
pub fn make_mpsc_transports(node_ids: &[NodeId]) -> Vec<MpscTransport> {
    // Step 1: create all (tx, rx) pairs and the shared directory.
    let peers_map = std::sync::Arc::new(std::sync::RwLock::new(HashMap::new()));

    let mut transports = Vec::with_capacity(node_ids.len());
    for &id in node_ids {
        let (tx, rx) = mpsc::sync_channel(512);
        peers_map.write().unwrap().insert(id, tx.clone());
        transports.push(MpscTransport {
            node_id: id,
            tx,
            rx,
            peers: std::sync::Arc::clone(&peers_map),
        });
    }
    transports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpsc_broadcast_all_receive() {
        let ids = [1u32, 2, 3];
        let mut transports = make_mpsc_transports(&ids);

        // Node 1 broadcasts a NewView message
        let msg = HotStuffMsg::NewView {
            view: 1,
            qc: crate::bft::crypto::QuorumCert::genesis(),
            signature: crate::bft::crypto::Signature {
                signer: 1,
                sig_bytes: vec![0u8; 64],
            },
        };
        transports[0].broadcast(msg.clone());

        // All 3 nodes should receive it (including sender)
        for t in &mut transports {
            let received = t.poll_incoming();
            assert_eq!(received.len(), 1, "node {} should receive broadcast", t.node_id());
        }
    }

    #[test]
    fn mpsc_send_to_leader_point_to_point() {
        let ids = [1u32, 2, 3];
        let mut transports = make_mpsc_transports(&ids);

        let vote = HotStuffMsg::Vote {
            view: 1,
            block_hash: 0xDEAD,
            phase: crate::bft::types::Phase::Prepare,
            signature: crate::bft::crypto::Signature {
                signer: 2,
                sig_bytes: vec![0u8; 64],
            },
        };

        // Node 2 sends vote to leader (node 1)
        transports[1].send_to_leader(1, vote);

        // Only node 1 should receive it
        assert_eq!(transports[0].poll_incoming().len(), 1);
        assert_eq!(transports[1].poll_incoming().len(), 0);
        assert_eq!(transports[2].poll_incoming().len(), 0);
    }
}
