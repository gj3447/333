// KG: SPAN_333_BFT_Transport
//! Network Transport abstraction for HotStuff BFT.
//!
//! Trait-based design: swap between InMemory (testing), WebRTC (browser),
//! or TCP (native super peer) without changing consensus logic.

use super::crypto::NodeId;
use super::types::HotStuffMsg;
use std::collections::VecDeque;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Transport trait — send/receive HotStuffMsg between validators
pub trait Transport {
    /// Send a message to a specific validator
    fn send(&mut self, to: NodeId, msg: HotStuffMsg);

    /// Broadcast a message to all validators
    fn broadcast(&mut self, msg: HotStuffMsg);

    /// Receive next message (non-blocking). Returns (sender, message).
    fn recv(&mut self) -> Option<(NodeId, HotStuffMsg)>;
}

/// Shared in-process message queues: maps each node to its inbox.
type SharedQueues = Arc<Mutex<HashMap<NodeId, VecDeque<(NodeId, HotStuffMsg)>>>>;

/// In-memory transport for testing — simulates a perfect network
#[derive(Clone)]
pub struct InMemoryNetwork {
    /// Shared message queues: node_id → inbox
    queues: SharedQueues,
    /// All known nodes
    nodes: Vec<NodeId>,
    /// This node's ID
    me: NodeId,
}

impl InMemoryNetwork {
    /// Create a network with N nodes. Returns one handle per node.
    pub fn create(nodes: &[NodeId]) -> Vec<Self> {
        let queues = Arc::new(Mutex::new(
            nodes.iter().map(|&id| (id, VecDeque::new())).collect(),
        ));
        nodes
            .iter()
            .map(|&id| Self {
                queues: Arc::clone(&queues),
                nodes: nodes.to_vec(),
                me: id,
            })
            .collect()
    }

    /// Number of pending messages for this node
    pub fn pending_count(&self) -> usize {
        let q = self.queues.lock().unwrap();
        q.get(&self.me).map_or(0, |inbox| inbox.len())
    }

    /// Total messages across all nodes (for debugging)
    pub fn total_pending(&self) -> usize {
        let q = self.queues.lock().unwrap();
        q.values().map(|inbox| inbox.len()).sum()
    }
}

impl Transport for InMemoryNetwork {
    fn send(&mut self, to: NodeId, msg: HotStuffMsg) {
        let mut q = self.queues.lock().unwrap();
        if let Some(inbox) = q.get_mut(&to) {
            inbox.push_back((self.me, msg));
        }
    }

    fn broadcast(&mut self, msg: HotStuffMsg) {
        let mut q = self.queues.lock().unwrap();
        for &node in &self.nodes {
            if node != self.me {
                if let Some(inbox) = q.get_mut(&node) {
                    inbox.push_back((self.me, msg.clone()));
                }
            }
        }
    }

    fn recv(&mut self) -> Option<(NodeId, HotStuffMsg)> {
        let mut q = self.queues.lock().unwrap();
        q.get_mut(&self.me)?.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bft::crypto::sign_standalone as sign;

    #[test]
    fn send_and_recv() {
        let nodes = vec![1, 2, 3];
        let mut nets = InMemoryNetwork::create(&nodes);

        // Node 1 sends to Node 2
        nets[0].send(2, HotStuffMsg::NewView {
            view: 1,
            qc: crate::bft::crypto::QuorumCert::genesis(),
            signature: sign(1, 1),
        });

        // Node 2 receives
        let msg = nets[1].recv();
        assert!(msg.is_some());
        let (sender, _) = msg.unwrap();
        assert_eq!(sender, 1);

        // Node 3 has nothing
        assert!(nets[2].recv().is_none());
    }

    #[test]
    fn broadcast_reaches_all_except_self() {
        let nodes = vec![1, 2, 3, 4, 5];
        let mut nets = InMemoryNetwork::create(&nodes);

        // Node 1 broadcasts
        nets[0].broadcast(HotStuffMsg::NewView {
            view: 5,
            qc: crate::bft::crypto::QuorumCert::genesis(),
            signature: sign(1, 5),
        });

        // All others receive
        assert!(nets[1].recv().is_some());
        assert!(nets[2].recv().is_some());
        assert!(nets[3].recv().is_some());
        assert!(nets[4].recv().is_some());

        // Sender doesn't receive own broadcast
        assert!(nets[0].recv().is_none());
    }

    #[test]
    fn message_ordering_fifo() {
        let nodes = vec![1, 2];
        let mut nets = InMemoryNetwork::create(&nodes);

        // Send 3 messages in order
        for i in 0..3 {
            nets[0].send(2, HotStuffMsg::Vote {
                block_hash: i,
                view: i,
                phase: crate::bft::types::Phase::Prepare,
                signature: sign(1, i),
            });
        }

        // Receive in same order (FIFO)
        for i in 0..3 {
            let (_, msg) = nets[1].recv().unwrap();
            if let HotStuffMsg::Vote { block_hash, .. } = msg {
                assert_eq!(block_hash, i);
            }
        }
    }

    #[test]
    fn pending_count() {
        let nodes = vec![1, 2];
        let mut nets = InMemoryNetwork::create(&nodes);

        assert_eq!(nets[1].pending_count(), 0);

        nets[0].send(2, HotStuffMsg::NewView {
            view: 0, qc: crate::bft::crypto::QuorumCert::genesis(), signature: sign(1, 0),
        });
        nets[0].send(2, HotStuffMsg::NewView {
            view: 1, qc: crate::bft::crypto::QuorumCert::genesis(), signature: sign(1, 1),
        });

        assert_eq!(nets[1].pending_count(), 2);
        nets[1].recv();
        assert_eq!(nets[1].pending_count(), 1);
    }
}
