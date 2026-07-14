// KG: SPAN_333_Infra — Super Peer mode
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-p2p-super-SuperPeerConfig/ConnectedPeer/SuperPeer/SuperPeerStats
// Always-on infrastructure node: DHT anchor, TURN relay, task cache
// Super Peers are the backbone of the 333 network

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Super Peer capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuperPeerConfig {
    pub peer_id: String,
    pub public_addr: String,           // e.g. "wss://signal.333.app"
    pub max_connections: usize,
    pub relay_enabled: bool,           // TURN relay for NAT-blocked peers
    pub dht_anchor: bool,              // participates in DHT routing
    pub task_cache_enabled: bool,      // caches compute tasks
    pub bandwidth_limit_mbps: Option<u32>,
}

/// Connected peer info
#[derive(Debug, Clone)]
pub struct ConnectedPeer {
    pub peer_id: String,
    pub connected_at: u64,
    pub bytes_relayed: u64,
    pub is_super_peer: bool,
}

/// Super Peer runtime state
pub struct SuperPeer {
    pub config: SuperPeerConfig,
    connections: HashMap<String, ConnectedPeer>,
    relay_pairs: Vec<(String, String)>,  // pairs being relayed
    dht_cache: HashMap<Vec<u8>, Vec<u8>>,
    stats: SuperPeerStats,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SuperPeerStats {
    pub total_connections: u64,
    pub active_connections: usize,
    pub bytes_relayed: u64,
    pub dht_entries: usize,
    pub relay_pairs: usize,
    pub uptime_ms: u64,
}

impl SuperPeer {
    pub fn new(config: SuperPeerConfig) -> Self {
        Self {
            config,
            connections: HashMap::new(),
            relay_pairs: Vec::new(),
            dht_cache: HashMap::new(),
            stats: SuperPeerStats::default(),
        }
    }

    /// Accept a new peer connection
    pub fn accept_connection(&mut self, peer_id: &str, now_ms: u64) -> bool {
        if self.connections.len() >= self.config.max_connections {
            return false;
        }
        self.connections.insert(peer_id.to_string(), ConnectedPeer {
            peer_id: peer_id.to_string(),
            connected_at: now_ms,
            bytes_relayed: 0,
            is_super_peer: false,
        });
        self.stats.total_connections += 1;
        self.stats.active_connections = self.connections.len();
        true
    }

    /// Disconnect a peer
    pub fn disconnect(&mut self, peer_id: &str) {
        self.connections.remove(peer_id);
        self.relay_pairs.retain(|(a, b)| a != peer_id && b != peer_id);
        self.stats.active_connections = self.connections.len();
        self.stats.relay_pairs = self.relay_pairs.len();
    }

    /// Set up relay between two NAT-blocked peers
    pub fn create_relay(&mut self, peer_a: &str, peer_b: &str) -> bool {
        if !self.config.relay_enabled { return false; }
        if !self.connections.contains_key(peer_a) || !self.connections.contains_key(peer_b) {
            return false;
        }
        self.relay_pairs.push((peer_a.to_string(), peer_b.to_string()));
        self.stats.relay_pairs = self.relay_pairs.len();
        true
    }

    /// Relay data between two peers
    pub fn relay_data(&mut self, from: &str, data_len: u64) -> Vec<String> {
        let mut targets = Vec::new();
        for (a, b) in &self.relay_pairs {
            if a == from { targets.push(b.clone()); }
            if b == from { targets.push(a.clone()); }
        }
        if !targets.is_empty() {
            self.stats.bytes_relayed += data_len * targets.len() as u64;
            if let Some(conn) = self.connections.get_mut(from) {
                conn.bytes_relayed += data_len;
            }
        }
        targets
    }

    /// Store DHT entry (anchor mode)
    pub fn dht_store(&mut self, key: &[u8], value: &[u8]) -> bool {
        if !self.config.dht_anchor { return false; }
        self.dht_cache.insert(key.to_vec(), value.to_vec());
        self.stats.dht_entries = self.dht_cache.len();
        true
    }

    /// Lookup DHT entry
    pub fn dht_lookup(&self, key: &[u8]) -> Option<&Vec<u8>> {
        self.dht_cache.get(key)
    }

    /// Get connected peer IDs
    pub fn connected_peers(&self) -> Vec<&str> {
        self.connections.keys().map(|s| s.as_str()).collect()
    }

    /// Get stats
    pub fn stats(&self) -> &SuperPeerStats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SuperPeerConfig {
        SuperPeerConfig {
            peer_id: "super-1".into(),
            public_addr: "wss://signal.333.app".into(),
            max_connections: 100,
            relay_enabled: true,
            dht_anchor: true,
            task_cache_enabled: true,
            bandwidth_limit_mbps: None,
        }
    }

    #[test]
    fn accept_and_disconnect() {
        let mut sp = SuperPeer::new(test_config());
        assert!(sp.accept_connection("peer-a", 0));
        assert!(sp.accept_connection("peer-b", 0));
        assert_eq!(sp.stats().active_connections, 2);

        sp.disconnect("peer-a");
        assert_eq!(sp.stats().active_connections, 1);
    }

    #[test]
    fn max_connections_enforced() {
        let mut cfg = test_config();
        cfg.max_connections = 2;
        let mut sp = SuperPeer::new(cfg);
        assert!(sp.accept_connection("p1", 0));
        assert!(sp.accept_connection("p2", 0));
        assert!(!sp.accept_connection("p3", 0)); // rejected
    }

    #[test]
    fn relay_between_peers() {
        let mut sp = SuperPeer::new(test_config());
        sp.accept_connection("alice", 0);
        sp.accept_connection("bob", 0);
        assert!(sp.create_relay("alice", "bob"));

        let targets = sp.relay_data("alice", 1024);
        assert_eq!(targets, vec!["bob"]);
        assert_eq!(sp.stats().bytes_relayed, 1024);
    }

    #[test]
    fn dht_anchor_store_lookup() {
        let mut sp = SuperPeer::new(test_config());
        assert!(sp.dht_store(b"block:0,0", b"stone"));
        assert_eq!(sp.dht_lookup(b"block:0,0"), Some(&b"stone".to_vec()));
        assert_eq!(sp.stats().dht_entries, 1);
    }

    #[test]
    fn relay_disabled_rejects() {
        let mut cfg = test_config();
        cfg.relay_enabled = false;
        let mut sp = SuperPeer::new(cfg);
        sp.accept_connection("alice", 0);
        sp.accept_connection("bob", 0);
        assert!(!sp.create_relay("alice", "bob"));
    }

    #[test]
    fn disconnect_cleans_relay() {
        let mut sp = SuperPeer::new(test_config());
        sp.accept_connection("alice", 0);
        sp.accept_connection("bob", 0);
        sp.create_relay("alice", "bob");
        assert_eq!(sp.stats().relay_pairs, 1);

        sp.disconnect("alice");
        assert_eq!(sp.stats().relay_pairs, 0);
    }
}
