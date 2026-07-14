// KG: CONTRACT_333_MeshRoom, ATOM_Web_MeshRoom
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-p2p-mesh-RoomConfig/PeerInfo/MeshEvent/MeshRoom
// Room management + Mesh topology + Peer lifecycle
// Contract: create/join/leave. heartbeat 5s. timeout 15s. broadcast→all peers.

use super::channel::{ChannelMode, DataChannel};
#[cfg(test)]
use super::channel::InMemoryChannel;
use std::collections::HashMap;

/// Room configuration
#[derive(Debug, Clone)]
pub struct RoomConfig {
    pub name: String,
    pub max_peers: usize,
    pub heartbeat_interval_ms: u64,
    pub peer_timeout_ms: u64,
}

impl Default for RoomConfig {
    fn default() -> Self {
        Self {
            name: "default".into(),
            max_peers: 50,
            heartbeat_interval_ms: 5000,
            peer_timeout_ms: 15000,
        }
    }
}

/// Peer info in a room
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub node_id: u32,
    pub joined_at: u64, // timestamp ms
    pub last_heartbeat: u64,
}

/// Event emitted by the mesh
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshEvent {
    PeerJoined(u32),
    PeerLeft(u32),
    PeerTimedOut(u32),
    RoomFull,
    MessageReceived { from: u32, data: Vec<u8> },
}

/// Mesh Room — manages peers and channels
pub struct MeshRoom {
    pub config: RoomConfig,
    local_id: u32,
    peers: HashMap<u32, PeerInfo>,
    channels: HashMap<u32, Box<dyn DataChannel>>,
    events: Vec<MeshEvent>,
}

impl MeshRoom {
    pub fn new(local_id: u32, config: RoomConfig) -> Self {
        Self {
            config,
            local_id,
            peers: HashMap::new(),
            channels: HashMap::new(),
            events: Vec::new(),
        }
    }

    /// Add a peer with an established channel
    pub fn add_peer(&mut self, peer_id: u32, channel: Box<dyn DataChannel>, now_ms: u64) -> bool {
        if self.peers.len() >= self.config.max_peers {
            self.events.push(MeshEvent::RoomFull);
            return false;
        }
        self.peers.insert(peer_id, PeerInfo {
            node_id: peer_id,
            joined_at: now_ms,
            last_heartbeat: now_ms,
        });
        self.channels.insert(peer_id, channel);
        self.events.push(MeshEvent::PeerJoined(peer_id));
        true
    }

    /// Remove a peer
    pub fn remove_peer(&mut self, peer_id: u32) {
        self.peers.remove(&peer_id);
        if let Some(mut ch) = self.channels.remove(&peer_id) {
            ch.close();
        }
        self.events.push(MeshEvent::PeerLeft(peer_id));
    }

    /// Update heartbeat for a peer
    pub fn heartbeat(&mut self, peer_id: u32, now_ms: u64) {
        if let Some(info) = self.peers.get_mut(&peer_id) {
            info.last_heartbeat = now_ms;
        }
    }

    /// Check for timed-out peers and remove them
    pub fn check_timeouts(&mut self, now_ms: u64) -> Vec<u32> {
        let timeout = self.config.peer_timeout_ms;
        let timed_out: Vec<u32> = self.peers.iter()
            .filter(|(_, info)| now_ms - info.last_heartbeat > timeout)
            .map(|(&id, _)| id)
            .collect();

        for id in &timed_out {
            self.peers.remove(id);
            if let Some(mut ch) = self.channels.remove(id) {
                ch.close();
            }
            self.events.push(MeshEvent::PeerTimedOut(*id));
        }
        timed_out
    }

    /// Broadcast data to all peers
    pub fn broadcast(&self, data: &[u8], mode: ChannelMode) -> usize {
        let mut sent = 0;
        for ch in self.channels.values() {
            if ch.send(data, mode).is_ok() {
                sent += 1;
            }
        }
        sent
    }

    /// Send to specific peer
    pub fn send_to(&self, peer_id: u32, data: &[u8], mode: ChannelMode) -> bool {
        self.channels.get(&peer_id)
            .is_some_and(|ch| ch.send(data, mode).is_ok())
    }

    /// Poll all channels for incoming messages
    pub fn poll(&mut self) -> Vec<MeshEvent> {
        let mut incoming = Vec::new();
        for (&peer_id, ch) in &self.channels {
            while let Some((data, _mode)) = ch.recv() {
                incoming.push(MeshEvent::MessageReceived { from: peer_id, data });
            }
        }
        incoming
    }

    /// Drain accumulated events
    pub fn drain_events(&mut self) -> Vec<MeshEvent> {
        std::mem::take(&mut self.events)
    }

    /// Current peer count
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// List peer IDs
    pub fn peer_ids(&self) -> Vec<u32> {
        self.peers.keys().copied().collect()
    }

    /// Local node ID
    pub fn local_id(&self) -> u32 {
        self.local_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_room(local_id: u32) -> MeshRoom {
        MeshRoom::new(local_id, RoomConfig::default())
    }

    #[test]
    fn add_and_remove_peer() {
        let mut room = make_room(1);
        let (ch_a, _ch_b) = InMemoryChannel::create_pair(1, 2);
        assert!(room.add_peer(2, Box::new(ch_a), 1000));
        assert_eq!(room.peer_count(), 1);

        room.remove_peer(2);
        assert_eq!(room.peer_count(), 0);
    }

    #[test]
    fn broadcast_to_all() {
        let mut room = make_room(1);

        let (ch_12, ch_21) = InMemoryChannel::create_pair(1, 2);
        let (ch_13, ch_31) = InMemoryChannel::create_pair(1, 3);
        room.add_peer(2, Box::new(ch_12), 0);
        room.add_peer(3, Box::new(ch_13), 0);

        let sent = room.broadcast(b"hello all", ChannelMode::Reliable);
        assert_eq!(sent, 2);

        // Both peers receive
        assert_eq!(ch_21.recv().unwrap().0, b"hello all");
        assert_eq!(ch_31.recv().unwrap().0, b"hello all");
    }

    #[test]
    fn send_to_specific_peer() {
        let mut room = make_room(1);
        let (ch_12, ch_21) = InMemoryChannel::create_pair(1, 2);
        let (ch_13, ch_31) = InMemoryChannel::create_pair(1, 3);
        room.add_peer(2, Box::new(ch_12), 0);
        room.add_peer(3, Box::new(ch_13), 0);

        room.send_to(2, b"private", ChannelMode::Reliable);

        assert_eq!(ch_21.recv().unwrap().0, b"private");
        assert!(ch_31.recv().is_none()); // peer 3 got nothing
    }

    #[test]
    fn peer_timeout() {
        let mut room = make_room(1);
        let (ch, _) = InMemoryChannel::create_pair(1, 2);
        room.add_peer(2, Box::new(ch), 0);

        // At t=10000, no heartbeat → not timed out yet (timeout=15000)
        let timed_out = room.check_timeouts(10000);
        assert!(timed_out.is_empty());
        assert_eq!(room.peer_count(), 1);

        // At t=16000 → timed out
        let timed_out = room.check_timeouts(16000);
        assert_eq!(timed_out, vec![2]);
        assert_eq!(room.peer_count(), 0);
    }

    #[test]
    fn heartbeat_resets_timeout() {
        let mut room = make_room(1);
        let (ch, _) = InMemoryChannel::create_pair(1, 2);
        room.add_peer(2, Box::new(ch), 0);

        // Heartbeat at t=10000
        room.heartbeat(2, 10000);

        // At t=20000 → 10s since heartbeat, not timed out (timeout=15s)
        let timed_out = room.check_timeouts(20000);
        assert!(timed_out.is_empty());

        // At t=26000 → 16s since heartbeat → timed out
        let timed_out = room.check_timeouts(26000);
        assert_eq!(timed_out, vec![2]);
    }

    #[test]
    fn room_full() {
        let config = RoomConfig { max_peers: 2, ..Default::default() };
        let mut room = MeshRoom::new(1, config);

        let (ch1, _) = InMemoryChannel::create_pair(1, 2);
        let (ch2, _) = InMemoryChannel::create_pair(1, 3);
        let (ch3, _) = InMemoryChannel::create_pair(1, 4);

        assert!(room.add_peer(2, Box::new(ch1), 0));
        assert!(room.add_peer(3, Box::new(ch2), 0));
        assert!(!room.add_peer(4, Box::new(ch3), 0)); // full!

        let events = room.drain_events();
        assert!(events.contains(&MeshEvent::RoomFull));
    }

    #[test]
    fn poll_incoming_messages() {
        let mut room = make_room(1);
        let (ch_12, ch_21) = InMemoryChannel::create_pair(1, 2);
        room.add_peer(2, Box::new(ch_12), 0);

        // Peer 2 sends to us
        ch_21.send(b"from peer 2", ChannelMode::Reliable).unwrap();

        let msgs = room.poll();
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            MeshEvent::MessageReceived { from, data } => {
                assert_eq!(*from, 2);
                assert_eq!(data, b"from peer 2");
            }
            _ => panic!("expected MessageReceived"),
        }
    }

    #[test]
    fn events_lifecycle() {
        let mut room = make_room(1);
        let (ch, _) = InMemoryChannel::create_pair(1, 2);
        room.add_peer(2, Box::new(ch), 0);
        room.remove_peer(2);

        let events = room.drain_events();
        assert_eq!(events, vec![MeshEvent::PeerJoined(2), MeshEvent::PeerLeft(2)]);

        // Events drained
        assert!(room.drain_events().is_empty());
    }
}
