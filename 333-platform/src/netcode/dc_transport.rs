// KG: sprint7I-dc-transport-tdd-2026-04-16
//! DataChannelTransport — bridges DataChannel (p2p layer) → BftTransport (consensus layer).
//!
//! Architecture:
//!   MeshRoom owns DataChannel connections to peers.
//!   DataChannelTransport wraps a MeshRoom and implements BftTransport.
//!   HotStuffMsg is serialized via postcard → wrapped in WireMessage (MsgType::Consensus).
//!
//! TDD: tests written first, implementation follows.

use crate::bft::crypto::NodeId;
use crate::bft::types::HotStuffMsg;
use crate::netcode::transport::BftTransport;
use crate::p2p::channel::{ChannelMode, InMemoryChannel};
use crate::p2p::mesh::{MeshRoom, RoomConfig, MeshEvent};
use crate::wire;

/// BftTransport backed by DataChannel mesh.
/// Bridges p2p::MeshRoom ↔ netcode::BftTransport.
pub struct DataChannelTransport {
    node_id: NodeId,
    mesh: MeshRoom,
    /// Self-loopback inbox: messages sent to self (e.g., leader voting for itself).
    self_inbox: Vec<HotStuffMsg>,
}

impl DataChannelTransport {
    /// Create a transport for `node_id` backed by the given MeshRoom.
    pub fn new(node_id: NodeId, mesh: MeshRoom) -> Self {
        Self { node_id, mesh, self_inbox: Vec::new() }
    }

    /// Encode a HotStuffMsg into a wire-framed byte vector.
    pub fn encode_msg(msg: &HotStuffMsg) -> Vec<u8> {
        let payload = postcard::to_allocvec(msg).expect("postcard encode");
        wire::encode(wire::MsgType::Consensus, &payload).expect("wire encode")
    }

    /// Decode a wire-framed byte vector back into a HotStuffMsg.
    pub fn decode_msg(data: &[u8]) -> Option<HotStuffMsg> {
        match wire::decode(data) {
            wire::DecodeResult::Ok(wire_msg) => {
                if wire_msg.header.msg_type != wire::MsgType::Consensus as u8 {
                    return None;
                }
                postcard::from_bytes::<HotStuffMsg>(&wire_msg.payload).ok()
            }
            _ => None,
        }
    }

    /// Access the underlying mesh (e.g., for adding peers).
    pub fn mesh_mut(&mut self) -> &mut MeshRoom {
        &mut self.mesh
    }
}

impl BftTransport for DataChannelTransport {
    fn broadcast(&mut self, msg: HotStuffMsg) {
        let data = Self::encode_msg(&msg);
        self.mesh.broadcast(&data, ChannelMode::Reliable);
    }

    fn send_to_leader(&mut self, leader_id: NodeId, msg: HotStuffMsg) {
        if leader_id == self.node_id {
            // Self-send: leader sends to itself
            self.self_inbox.push(msg);
        } else {
            let data = Self::encode_msg(&msg);
            self.mesh.send_to(leader_id, &data, ChannelMode::Reliable);
        }
    }

    fn poll_incoming(&mut self) -> Vec<HotStuffMsg> {
        // Drain self-inbox first
        let mut msgs: Vec<HotStuffMsg> = self.self_inbox.drain(..).collect();
        // Then drain mesh
        let events = self.mesh.poll();
        msgs.extend(events.into_iter().filter_map(|ev| {
            match ev {
                MeshEvent::MessageReceived { data, .. } => Self::decode_msg(&data),
                _ => None,
            }
        }));
        msgs
    }

    fn node_id(&self) -> NodeId {
        self.node_id
    }
}

/// Create N connected DataChannelTransport instances (for testing).
/// Each node gets a MeshRoom with InMemoryChannels to all other nodes.
pub fn make_dc_transports(node_ids: &[NodeId]) -> Vec<DataChannelTransport> {
    let n = node_ids.len();
    // Create mesh rooms
    let mut rooms: Vec<MeshRoom> = node_ids.iter()
        .map(|&id| MeshRoom::new(id, RoomConfig { max_peers: n, ..Default::default() }))
        .collect();

    // Wire up InMemoryChannel pairs for every (i, j) where i < j
    for i in 0..n {
        for j in (i + 1)..n {
            let (ch_ij, ch_ji) = InMemoryChannel::create_pair(node_ids[i], node_ids[j]);
            rooms[i].add_peer(node_ids[j], Box::new(ch_ij), 0);
            rooms[j].add_peer(node_ids[i], Box::new(ch_ji), 0);
        }
    }

    node_ids.iter().zip(rooms.into_iter())
        .map(|(&id, room)| DataChannelTransport::new(id, room))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bft::crypto::{QuorumCert, Signature};
    use crate::bft::types::Phase;

    fn make_test_msg(view: u64, signer: NodeId) -> HotStuffMsg {
        HotStuffMsg::NewView {
            view,
            qc: QuorumCert::genesis(),
            signature: Signature { signer, sig_bytes: vec![0u8; 64] },
        }
    }

    fn make_vote(view: u64, signer: NodeId) -> HotStuffMsg {
        HotStuffMsg::Vote {
            view,
            block_hash: 0xBEEF,
            phase: Phase::Prepare,
            signature: Signature { signer, sig_bytes: vec![0u8; 64] },
        }
    }

    // ── TDD: encode/decode roundtrip ──────────────────────────────────────────

    #[test]
    fn encode_decode_roundtrip_newview() {
        let msg = make_test_msg(42, 1);
        let encoded = DataChannelTransport::encode_msg(&msg);
        let decoded = DataChannelTransport::decode_msg(&encoded).expect("decode must succeed");
        // Check fields match (HotStuffMsg doesn't impl PartialEq, check structurally)
        match decoded {
            HotStuffMsg::NewView { view, .. } => assert_eq!(view, 42),
            _ => panic!("expected NewView"),
        }
    }

    #[test]
    fn encode_decode_roundtrip_vote() {
        let msg = make_vote(7, 3);
        let encoded = DataChannelTransport::encode_msg(&msg);
        let decoded = DataChannelTransport::decode_msg(&encoded).expect("decode must succeed");
        match decoded {
            HotStuffMsg::Vote { view, block_hash, phase, signature } => {
                assert_eq!(view, 7);
                assert_eq!(block_hash, 0xBEEF);
                assert_eq!(phase, Phase::Prepare);
                assert_eq!(signature.signer, 3);
            }
            _ => panic!("expected Vote"),
        }
    }

    #[test]
    fn decode_garbage_returns_none() {
        assert!(DataChannelTransport::decode_msg(&[0xFF, 0xFF, 0xFF]).is_none());
    }

    #[test]
    fn decode_wrong_msg_type_returns_none() {
        // Encode as Heartbeat instead of Consensus
        let payload = postcard::to_allocvec(&make_test_msg(1, 1)).unwrap();
        let encoded = wire::encode(wire::MsgType::Heartbeat, &payload).unwrap();
        assert!(DataChannelTransport::decode_msg(&encoded).is_none());
    }

    // ── TDD: BftTransport trait compliance ────────────────────────────────────

    #[test]
    fn dc_broadcast_all_receive() {
        let ids = [1u32, 2, 3, 4];
        let mut transports = make_dc_transports(&ids);

        // Node 1 broadcasts
        let msg = make_test_msg(1, 1);
        transports[0].broadcast(msg);

        // Nodes 2, 3, 4 should receive it
        for t in &mut transports[1..] {
            let received = t.poll_incoming();
            assert_eq!(received.len(), 1, "node {} should receive broadcast", t.node_id());
            match &received[0] {
                HotStuffMsg::NewView { view, .. } => assert_eq!(*view, 1),
                _ => panic!("expected NewView"),
            }
        }
    }

    #[test]
    fn dc_send_to_leader_point_to_point() {
        let ids = [1u32, 2, 3];
        let mut transports = make_dc_transports(&ids);

        // Node 2 sends vote to leader (node 1)
        let vote = make_vote(1, 2);
        transports[1].send_to_leader(1, vote);

        // Only node 1 receives
        assert_eq!(transports[0].poll_incoming().len(), 1);
        assert_eq!(transports[2].poll_incoming().len(), 0);
    }

    #[test]
    fn dc_empty_poll_returns_empty() {
        let ids = [1u32, 2];
        let mut transports = make_dc_transports(&ids);
        assert!(transports[0].poll_incoming().is_empty());
        assert!(transports[1].poll_incoming().is_empty());
    }

    #[test]
    fn dc_node_id_correct() {
        let ids = [10u32, 20, 30];
        let transports = make_dc_transports(&ids);
        assert_eq!(transports[0].node_id(), 10);
        assert_eq!(transports[1].node_id(), 20);
        assert_eq!(transports[2].node_id(), 30);
    }

    #[test]
    fn dc_multiple_messages_fifo() {
        let ids = [1u32, 2];
        let mut transports = make_dc_transports(&ids);

        // Send 3 messages
        for v in 1..=3 {
            transports[0].broadcast(make_test_msg(v, 1));
        }

        let received = transports[1].poll_incoming();
        assert_eq!(received.len(), 3);
        for (i, msg) in received.iter().enumerate() {
            match msg {
                HotStuffMsg::NewView { view, .. } => assert_eq!(*view, (i + 1) as u64),
                _ => panic!("expected NewView"),
            }
        }
    }

    #[test]
    fn dc_bidirectional() {
        let ids = [1u32, 2];
        let mut transports = make_dc_transports(&ids);

        transports[0].broadcast(make_test_msg(10, 1));
        transports[1].broadcast(make_test_msg(20, 2));

        let from_1 = transports[1].poll_incoming();
        let from_2 = transports[0].poll_incoming();

        assert_eq!(from_1.len(), 1);
        assert_eq!(from_2.len(), 1);
        match &from_1[0] {
            HotStuffMsg::NewView { view, .. } => assert_eq!(*view, 10),
            _ => panic!("expected NewView"),
        }
        match &from_2[0] {
            HotStuffMsg::NewView { view, .. } => assert_eq!(*view, 20),
            _ => panic!("expected NewView"),
        }
    }

    // ── TDD: Wire protocol integration ────────────────────────────────────────

    #[test]
    fn wire_consensus_type_tag() {
        let msg = make_test_msg(1, 1);
        let encoded = DataChannelTransport::encode_msg(&msg);
        // Byte 1 = MsgType::Consensus = 0x04
        assert_eq!(encoded[1], 0x04);
    }

    #[test]
    fn wire_version_is_current() {
        let msg = make_test_msg(1, 1);
        let encoded = DataChannelTransport::encode_msg(&msg);
        assert_eq!(encoded[0], wire::WIRE_VERSION);
    }
}
