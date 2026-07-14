// KG: CONTRACT_333_Signaling — 시그널링 클라이언트
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-p2p-signaling-SignalMsg/SignalingClient/SignalEvent
// WebSocket으로 시그널링 서버에 연결, SDP/ICE 릴레이
// 서버: signaling/server.mjs (90줄 Node.js)

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Signaling message types (matches server.mjs protocol)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SignalMsg {
    /// Join a room
    #[serde(rename = "join")]
    Join { room: String, #[serde(rename = "peerId")] peer_id: String },

    /// Server response: current peers in room
    #[serde(rename = "peers")]
    Peers { peers: Vec<String>, you: String },

    /// New peer joined
    #[serde(rename = "peer-joined")]
    PeerJoined { #[serde(rename = "peerId")] peer_id: String },

    /// Peer left
    #[serde(rename = "peer-left")]
    PeerLeft { #[serde(rename = "peerId")] peer_id: String },

    /// SDP Offer
    #[serde(rename = "offer")]
    Offer { to: String, sdp: String, #[serde(skip_serializing_if = "Option::is_none")] from: Option<String> },

    /// SDP Answer
    #[serde(rename = "answer")]
    Answer { to: String, sdp: String, #[serde(skip_serializing_if = "Option::is_none")] from: Option<String> },

    /// ICE Candidate
    #[serde(rename = "ice")]
    Ice { to: String, candidate: String, #[serde(skip_serializing_if = "Option::is_none")] from: Option<String> },

    /// Broadcast (CRDT delta relay)
    #[serde(rename = "broadcast")]
    Broadcast { data: String, #[serde(skip_serializing_if = "Option::is_none")] from: Option<String> },
}

/// Signaling client state machine (transport-agnostic for testing)
#[derive(Debug)]
pub struct SignalingClient {
    pub peer_id: String,
    pub room: String,
    pub known_peers: Vec<String>,
    pub outbox: VecDeque<String>,    // JSON messages to send
    pub events: VecDeque<SignalEvent>,
}

/// Events emitted by the signaling client
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalEvent {
    Joined { peers: Vec<String> },
    PeerJoined(String),
    PeerLeft(String),
    OfferReceived { from: String, sdp: String },
    AnswerReceived { from: String, sdp: String },
    IceReceived { from: String, candidate: String },
    BroadcastReceived { from: String, data: String },
}

impl SignalingClient {
    pub fn new(peer_id: &str, room: &str) -> Self {
        let mut client = Self {
            peer_id: peer_id.into(),
            room: room.into(),
            known_peers: Vec::new(),
            outbox: VecDeque::new(),
            events: VecDeque::new(),
        };
        // Auto-send join message
        let join = SignalMsg::Join {
            room: room.into(),
            peer_id: peer_id.into(),
        };
        if let Ok(json) = serde_json::to_string(&join) {
            client.outbox.push_back(json);
        }
        client
    }

    /// Process incoming JSON message from WebSocket
    pub fn on_message(&mut self, json: &str) {
        let msg: SignalMsg = match serde_json::from_str(json) {
            Ok(m) => m,
            Err(_) => return,
        };

        match msg {
            SignalMsg::Peers { peers, you: _ } => {
                self.known_peers = peers.clone();
                self.events.push_back(SignalEvent::Joined { peers });
            }
            SignalMsg::PeerJoined { peer_id } => {
                if !self.known_peers.contains(&peer_id) {
                    self.known_peers.push(peer_id.clone());
                }
                self.events.push_back(SignalEvent::PeerJoined(peer_id));
            }
            SignalMsg::PeerLeft { peer_id } => {
                self.known_peers.retain(|p| p != &peer_id);
                self.events.push_back(SignalEvent::PeerLeft(peer_id));
            }
            SignalMsg::Offer { from: Some(from), sdp, .. } => {
                self.events.push_back(SignalEvent::OfferReceived { from, sdp });
            }
            SignalMsg::Offer { .. } => {}
            SignalMsg::Answer { from: Some(from), sdp, .. } => {
                self.events.push_back(SignalEvent::AnswerReceived { from, sdp });
            }
            SignalMsg::Answer { .. } => {}
            SignalMsg::Ice { from: Some(from), candidate, .. } => {
                self.events.push_back(SignalEvent::IceReceived { from, candidate });
            }
            SignalMsg::Ice { .. } => {}
            SignalMsg::Broadcast { from: Some(from), data } => {
                self.events.push_back(SignalEvent::BroadcastReceived { from, data });
            }
            SignalMsg::Broadcast { .. } => {}
            _ => {}
        }
    }

    /// Send SDP offer to a specific peer
    pub fn send_offer(&mut self, to: &str, sdp: &str) {
        let msg = SignalMsg::Offer { to: to.into(), sdp: sdp.into(), from: None };
        if let Ok(json) = serde_json::to_string(&msg) {
            self.outbox.push_back(json);
        }
    }

    /// Send SDP answer
    pub fn send_answer(&mut self, to: &str, sdp: &str) {
        let msg = SignalMsg::Answer { to: to.into(), sdp: sdp.into(), from: None };
        if let Ok(json) = serde_json::to_string(&msg) {
            self.outbox.push_back(json);
        }
    }

    /// Send ICE candidate
    pub fn send_ice(&mut self, to: &str, candidate: &str) {
        let msg = SignalMsg::Ice { to: to.into(), candidate: candidate.into(), from: None };
        if let Ok(json) = serde_json::to_string(&msg) {
            self.outbox.push_back(json);
        }
    }

    /// Broadcast data to room
    pub fn broadcast(&mut self, data: &str) {
        let msg = SignalMsg::Broadcast { data: data.into(), from: None };
        if let Ok(json) = serde_json::to_string(&msg) {
            self.outbox.push_back(json);
        }
    }

    /// Take next outgoing message
    pub fn next_outgoing(&mut self) -> Option<String> {
        self.outbox.pop_front()
    }

    /// Take next event
    pub fn next_event(&mut self) -> Option<SignalEvent> {
        self.events.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_sends_message() {
        let mut client = SignalingClient::new("peer-1", "room-test");
        let msg = client.next_outgoing().unwrap();
        assert!(msg.contains("\"type\":\"join\""));
        assert!(msg.contains("room-test"));
        assert!(msg.contains("peer-1"));
    }

    #[test]
    fn peers_response() {
        let mut client = SignalingClient::new("peer-1", "room");
        client.on_message(r#"{"type":"peers","peers":["peer-2","peer-3"],"you":"peer-1"}"#);
        assert_eq!(client.known_peers, vec!["peer-2", "peer-3"]);
        let evt = client.next_event().unwrap();
        assert_eq!(evt, SignalEvent::Joined { peers: vec!["peer-2".into(), "peer-3".into()] });
    }

    #[test]
    fn peer_join_leave() {
        let mut client = SignalingClient::new("peer-1", "room");
        client.on_message(r#"{"type":"peer-joined","peerId":"peer-5"}"#);
        assert!(client.known_peers.contains(&"peer-5".to_string()));
        assert_eq!(client.next_event(), Some(SignalEvent::PeerJoined("peer-5".into())));

        client.on_message(r#"{"type":"peer-left","peerId":"peer-5"}"#);
        assert!(!client.known_peers.contains(&"peer-5".to_string()));
        assert_eq!(client.next_event(), Some(SignalEvent::PeerLeft("peer-5".into())));
    }

    #[test]
    fn offer_answer_ice_relay() {
        let mut client = SignalingClient::new("peer-1", "room");

        client.on_message(r#"{"type":"offer","to":"peer-1","sdp":"v=0\r\n...","from":"peer-2"}"#);
        let evt = client.next_event().unwrap();
        assert_eq!(evt, SignalEvent::OfferReceived { from: "peer-2".into(), sdp: "v=0\r\n...".into() });

        client.on_message(r#"{"type":"answer","to":"peer-1","sdp":"v=0\r\nanswer","from":"peer-2"}"#);
        let evt = client.next_event().unwrap();
        assert_eq!(evt, SignalEvent::AnswerReceived { from: "peer-2".into(), sdp: "v=0\r\nanswer".into() });

        client.on_message(r#"{"type":"ice","to":"peer-1","candidate":"candidate:1 1 udp ...","from":"peer-2"}"#);
        let evt = client.next_event().unwrap();
        assert_eq!(evt, SignalEvent::IceReceived { from: "peer-2".into(), candidate: "candidate:1 1 udp ...".into() });
    }

    #[test]
    fn send_offer() {
        let mut client = SignalingClient::new("peer-1", "room");
        let _ = client.next_outgoing(); // consume join msg
        client.send_offer("peer-2", "v=0\r\nsdp-data");
        let msg = client.next_outgoing().unwrap();
        assert!(msg.contains("\"type\":\"offer\""));
        assert!(msg.contains("peer-2"));
        assert!(msg.contains("v=0"));
    }

    #[test]
    fn broadcast() {
        let mut client = SignalingClient::new("peer-1", "room");
        let _ = client.next_outgoing();
        client.broadcast("{\"delta\":{\"key\":\"0,0,0\",\"val\":\"stone\"}}");
        let msg = client.next_outgoing().unwrap();
        assert!(msg.contains("\"type\":\"broadcast\""));
        assert!(msg.contains("delta"));
    }

    #[test]
    fn invalid_json_ignored() {
        let mut client = SignalingClient::new("peer-1", "room");
        client.on_message("not json at all");
        client.on_message("{broken");
        assert!(client.next_event().is_none());
    }

    #[test]
    fn broadcast_received() {
        let mut client = SignalingClient::new("peer-1", "room");
        client.on_message(r#"{"type":"broadcast","data":"hello world","from":"peer-3"}"#);
        assert_eq!(
            client.next_event(),
            Some(SignalEvent::BroadcastReceived { from: "peer-3".into(), data: "hello world".into() })
        );
    }
}
