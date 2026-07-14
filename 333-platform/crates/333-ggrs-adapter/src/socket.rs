// KG: seed-ggrs-bft-adapter-eval-2026-04-15
//! WebRtcNonBlockingSocket — mock implementation of `ggrs::NonBlockingSocket`.
//!
//! In-process transport using `std::sync::mpsc` channels.
//! Production: replace `MpscTransport` with calls into `p2p/webrtc.rs`
//! DataChannel (CH_POSITION for unreliable inputs, CH_BFT for confirmations).
//!
//! GGRS trait signature (from docs.rs/ggrs 0.10):
//!   fn send_to(&mut self, msg: &Message, addr: &SocketAddr)
//!   fn receive_all_messages(&mut self) -> Vec<(SocketAddr, Message)>
//!
//! Because ggrs dep is commented out, we define a compatible stand-in trait.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::mpsc::{self, Receiver, Sender};

// ── Stand-in trait (mirrors ggrs::NonBlockingSocket) ─────────────────────

/// Byte-level message passed between GGRS peers.
/// KG: seed-ggrs-bft-adapter-eval-2026-04-15
#[derive(Debug, Clone)]
pub struct RawMessage {
    pub payload: Vec<u8>,
}

/// Trait mirroring `ggrs::NonBlockingSocket<SocketAddr>`.
/// Swap `RawMessage` ↔ `ggrs::Message` when enabling ggrs-real feature.
/// KG: seed-ggrs-bft-adapter-eval-2026-04-15
pub trait NonBlockingSocket {
    fn send_to(&mut self, msg: &RawMessage, addr: &SocketAddr);
    fn receive_all_messages(&mut self) -> Vec<(SocketAddr, RawMessage)>;
}

// ── In-process mpsc transport (test / mock) ────────────────────────────────

/// Per-peer mpsc channel pair.
#[allow(dead_code)] // reserved for future production wiring
struct PeerChannel {
    tx: Sender<RawMessage>,
    rx: Receiver<RawMessage>,
    addr: SocketAddr,
}

/// In-process socket backed by `std::sync::mpsc`.
///
/// Create a matched pair with [`connected_pair`] for two-peer tests,
/// or use [`MockSocketMesh`] for N-peer simulations.
/// KG: seed-ggrs-bft-adapter-eval-2026-04-15
pub struct WebRtcNonBlockingSocket {
    /// Own address (used as identity)
    local_addr: SocketAddr,
    /// Outbound senders to remote peers
    senders: HashMap<SocketAddr, Sender<RawMessage>>,
    /// Inbound receiver (all peers share one queue)
    inbox: Receiver<RawMessage>,
    /// Sender half of our own inbox (cloned out to remote sockets)
    inbox_tx: Sender<RawMessage>,
}

impl WebRtcNonBlockingSocket {
    /// Create a new socket with the given local address.
    pub fn new(local_addr: SocketAddr) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            local_addr,
            senders: HashMap::new(),
            inbox: rx,
            inbox_tx: tx,
        }
    }

    /// Expose the sender so a remote socket can wire itself to us.
    pub fn sender(&self) -> (SocketAddr, Sender<RawMessage>) {
        (self.local_addr, self.inbox_tx.clone())
    }

    /// Register a remote peer: store their sender so we can send to them.
    pub fn connect_to_peer(&mut self, addr: SocketAddr, tx: Sender<RawMessage>) {
        self.senders.insert(addr, tx);
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl NonBlockingSocket for WebRtcNonBlockingSocket {
    /// Send a message to a specific remote address.
    /// Silently drops if peer not registered (mirrors real UDP behaviour).
    fn send_to(&mut self, msg: &RawMessage, addr: &SocketAddr) {
        if let Some(tx) = self.senders.get(addr) {
            let _ = tx.send(msg.clone());
        }
    }

    /// Drain all pending inbound messages.
    fn receive_all_messages(&mut self) -> Vec<(SocketAddr, RawMessage)> {
        // We don't track per-sender identity in this mock;
        // return a synthetic "remote" addr. Production impl reads from
        // DataChannel's onmessage event which carries RTCDataChannel label.
        let mut out = Vec::new();
        while let Ok(msg) = self.inbox.try_recv() {
            // Placeholder: addr unknown in simple mpsc mock
            let from = SocketAddr::from_str("0.0.0.0:0").unwrap();
            out.push((from, msg));
        }
        out
    }
}

// ── N-peer mesh factory ────────────────────────────────────────────────────

/// Build a fully-connected mesh of N sockets for simulation tests.
/// Each socket[i] can send to socket[j] for all i≠j.
/// KG: seed-ggrs-bft-adapter-eval-2026-04-15
pub fn build_mesh(n: usize) -> Vec<WebRtcNonBlockingSocket> {
    // Assign local addresses 127.0.0.1:9000..9000+n
    let addrs: Vec<SocketAddr> = (0..n)
        .map(|i| {
            SocketAddr::from_str(&format!("127.0.0.1:{}", 9000 + i)).unwrap()
        })
        .collect();

    let mut sockets: Vec<WebRtcNonBlockingSocket> = addrs
        .iter()
        .map(|&addr| WebRtcNonBlockingSocket::new(addr))
        .collect();

    // Collect (addr, sender) pairs first to avoid borrow conflict
    let pairs: Vec<(SocketAddr, std::sync::mpsc::Sender<RawMessage>)> =
        sockets.iter().map(|s| s.sender()).collect();

    // Wire each socket to all others
    for (i, socket) in sockets.iter_mut().enumerate() {
        for (j, pair) in pairs.iter().enumerate() {
            if i != j {
                let (addr, tx) = pair.clone();
                socket.connect_to_peer(addr, tx);
            }
        }
    }

    sockets
}

// ── WebRTC bridge note ─────────────────────────────────────────────────────
//
// Production wiring (out of scope for this spike):
//
//   impl NonBlockingSocket for WebRtcNonBlockingSocket {
//     fn send_to(&mut self, msg: &RawMessage, addr: &SocketAddr) {
//       // Look up WebRtcPeer by addr → peer.channels.position.send(&msg.payload)
//       // Use CH_POSITION (unreliable) for input messages (latency-critical)
//       // Use CH_BFT (reliable) for sync/confirm messages
//     }
//     fn receive_all_messages(&mut self) -> Vec<(SocketAddr, RawMessage)> {
//       // Drain WebRtcPeer.position_inbox (SyncInbox::drain_all)
//       // Map each (peer_id, bytes) → (SocketAddr, RawMessage)
//     }
//   }
//
// KG: seed-ggrs-bft-adapter-eval-2026-04-15
