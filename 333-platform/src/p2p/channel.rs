// KG: CONTRACT_333_DataChannel, ATOM_Web_DataChannel
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-p2p-channel-ChannelMode/ConnState/DataChannel/ChannelError/InMemoryChannel
// DataChannel trait + InMemory mock for testing
// Contract: connect→SDP exchange→ICE→channel open. send/recv. reliable+unreliable.
// Browser impl: web-sys RTCPeerConnection + RTCDataChannel
// Test impl: InMemoryChannel (crossbeam channel)

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Channel reliability mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMode {
    /// TCP-like: guaranteed delivery in order (CRDT state, chat, inventory)
    Reliable,
    /// UDP-like: fast, may drop (cursor position, player movement)
    Unreliable,
}

/// Peer connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
    Failed,
}

/// Abstract data channel — trait for platform swap
pub trait DataChannel: Send + Sync {
    /// Send bytes to remote peer
    fn send(&self, data: &[u8], mode: ChannelMode) -> Result<(), ChannelError>;

    /// Receive bytes (non-blocking)
    fn recv(&self) -> Option<(Vec<u8>, ChannelMode)>;

    /// Connection state
    fn state(&self) -> ConnState;

    /// Close the channel
    fn close(&mut self);

    /// Remote peer ID
    fn remote_peer(&self) -> u32;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelError {
    NotConnected,
    SendFailed(String),
    ChannelClosed,
}

impl std::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NotConnected => write!(f, "not connected"),
            Self::SendFailed(e) => write!(f, "send failed: {}", e),
            Self::ChannelClosed => write!(f, "channel closed"),
        }
    }
}

// ====== InMemory mock for testing ======

/// Shared message queue between two peers
type SharedQueue = Arc<Mutex<VecDeque<(Vec<u8>, ChannelMode)>>>;

/// In-memory channel pair (for testing without WebRTC)
pub struct InMemoryChannel {
    #[allow(dead_code)]
    local_id: u32,
    remote_id: u32,
    outbox: SharedQueue,     // we write, peer reads
    inbox: SharedQueue,      // peer writes, we read
    state: ConnState,
}

impl InMemoryChannel {
    /// Create a connected pair of channels (peer_a ↔ peer_b)
    pub fn create_pair(peer_a: u32, peer_b: u32) -> (Self, Self) {
        let q_a_to_b = Arc::new(Mutex::new(VecDeque::new()));
        let q_b_to_a = Arc::new(Mutex::new(VecDeque::new()));

        let ch_a = Self {
            local_id: peer_a,
            remote_id: peer_b,
            outbox: Arc::clone(&q_a_to_b),
            inbox: Arc::clone(&q_b_to_a),
            state: ConnState::Connected,
        };

        let ch_b = Self {
            local_id: peer_b,
            remote_id: peer_a,
            outbox: q_b_to_a,
            inbox: q_a_to_b,
            state: ConnState::Connected,
        };

        (ch_a, ch_b)
    }
}

impl DataChannel for InMemoryChannel {
    fn send(&self, data: &[u8], mode: ChannelMode) -> Result<(), ChannelError> {
        if self.state != ConnState::Connected {
            return Err(ChannelError::NotConnected);
        }
        let mut q = self.outbox.lock().unwrap();
        q.push_back((data.to_vec(), mode));
        Ok(())
    }

    fn recv(&self) -> Option<(Vec<u8>, ChannelMode)> {
        let mut q = self.inbox.lock().unwrap();
        q.pop_front()
    }

    fn state(&self) -> ConnState {
        self.state
    }

    fn close(&mut self) {
        self.state = ConnState::Disconnected;
    }

    fn remote_peer(&self) -> u32 {
        self.remote_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_pair_connected() {
        let (a, b) = InMemoryChannel::create_pair(1, 2);
        assert_eq!(a.state(), ConnState::Connected);
        assert_eq!(b.state(), ConnState::Connected);
        assert_eq!(a.remote_peer(), 2);
        assert_eq!(b.remote_peer(), 1);
    }

    #[test]
    fn send_recv_reliable() {
        let (a, b) = InMemoryChannel::create_pair(1, 2);
        a.send(b"hello", ChannelMode::Reliable).unwrap();
        let (data, mode) = b.recv().unwrap();
        assert_eq!(data, b"hello");
        assert_eq!(mode, ChannelMode::Reliable);
    }

    #[test]
    fn send_recv_unreliable() {
        let (a, b) = InMemoryChannel::create_pair(1, 2);
        a.send(b"pos:10,20", ChannelMode::Unreliable).unwrap();
        let (data, mode) = b.recv().unwrap();
        assert_eq!(data, b"pos:10,20");
        assert_eq!(mode, ChannelMode::Unreliable);
    }

    #[test]
    fn bidirectional() {
        let (a, b) = InMemoryChannel::create_pair(1, 2);
        a.send(b"ping", ChannelMode::Reliable).unwrap();
        b.send(b"pong", ChannelMode::Reliable).unwrap();

        let from_a = b.recv().unwrap();
        let from_b = a.recv().unwrap();
        assert_eq!(from_a.0, b"ping");
        assert_eq!(from_b.0, b"pong");
    }

    #[test]
    fn recv_empty_returns_none() {
        let (a, _b) = InMemoryChannel::create_pair(1, 2);
        assert!(a.recv().is_none());
    }

    #[test]
    fn send_after_close_fails() {
        let (mut a, _b) = InMemoryChannel::create_pair(1, 2);
        a.close();
        assert_eq!(a.state(), ConnState::Disconnected);
        assert_eq!(a.send(b"fail", ChannelMode::Reliable), Err(ChannelError::NotConnected));
    }

    #[test]
    fn fifo_ordering() {
        let (a, b) = InMemoryChannel::create_pair(1, 2);
        a.send(b"1", ChannelMode::Reliable).unwrap();
        a.send(b"2", ChannelMode::Reliable).unwrap();
        a.send(b"3", ChannelMode::Reliable).unwrap();

        assert_eq!(b.recv().unwrap().0, b"1");
        assert_eq!(b.recv().unwrap().0, b"2");
        assert_eq!(b.recv().unwrap().0, b"3");
        assert!(b.recv().is_none());
    }

    #[test]
    fn multiple_messages_queued() {
        let (a, b) = InMemoryChannel::create_pair(1, 2);
        for i in 0..100 {
            a.send(&[i as u8], ChannelMode::Reliable).unwrap();
        }
        for i in 0..100 {
            let (data, _) = b.recv().unwrap();
            assert_eq!(data, vec![i as u8]);
        }
    }
}
