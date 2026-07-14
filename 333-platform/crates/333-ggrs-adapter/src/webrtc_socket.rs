// KG: taliban-fix-H2-webrtc-socket-2026-04-15
//! `WebRtcNonBlockingSocket` — real 333 WebRTC DataChannel wiring for GGRS.
//!
//! **Taliban H2 fix**: the original mock in `socket.rs` loses sender identity
//! (`0.0.0.0:0`) because a single untagged mpsc queue cannot attribute which
//! peer sent a message.  This module fixes that by storing inbound messages as
//! `(peer_id, bytes)` tuples and synthesising a deterministic IPv6 `SocketAddr`
//! from each `peer_id`, so GGRS `receive_all_messages()` returns attributable
//! `(SocketAddr, RawMessage)` pairs.
//!
//! ## peer_id → SocketAddr mapping rule
//! ```text
//! fd00::{peer_id as u32 → 4 octets, zero-padded to 128 bits}
//! port = 0
//! ```
//! Example: peer_id=7 → `fd00::7:0`
//!
//! The `fd00::/8` prefix is a ULA (RFC 4193) — never routable on the public
//! internet, so there is no clash with real addresses.
//!
//! ## Channel selection
//! All GGRS input messages use `CH_POSITION` (unreliable, unordered, 50 ms
//! lifetime) — latency-critical lockstep inputs tolerate drops because GGRS
//! handles rollback internally.
//!
//! ## Fallback on send failure
//! `send_to()` silently drops unknown peers (mirrors UDP behaviour).
//! Callers detect stale inputs via GGRS's missing-input timeout, not errors.
//!
//! # KG: taliban-fix-H2-webrtc-socket-2026-04-15

use crate::socket::{NonBlockingSocket, RawMessage};
use std::collections::HashMap;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::{Arc, Mutex};

// ── peer_id ↔ SocketAddr helpers ─────────────────────────────────────────────

/// Convert a `peer_id` (u32) to a deterministic ULA IPv6 `SocketAddr`.
///
/// Mapping: `fd00::0000:0000:xxxx:xxxx` where `xxxx:xxxx` is peer_id as big-endian u32.
/// KG: taliban-fix-H2-webrtc-socket-2026-04-15
pub fn peer_id_to_addr(peer_id: u32) -> SocketAddr {
    let hi = (peer_id >> 16) as u16;
    let lo = (peer_id & 0xFFFF) as u16;
    let ip = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, hi, lo);
    SocketAddr::V6(SocketAddrV6::new(ip, 0, 0, 0))
}

/// Convert a synthesised `SocketAddr` back to `peer_id`.
/// Returns `None` if the address was not synthesised by this module.
/// KG: taliban-fix-H2-webrtc-socket-2026-04-15
pub fn addr_to_peer_id(addr: &SocketAddr) -> Option<u32> {
    if let SocketAddr::V6(v6) = addr {
        let seg = v6.ip().segments();
        if seg[0] == 0xfd00 && seg[1] == 0 && seg[2] == 0 && seg[3] == 0
            && seg[4] == 0 && seg[5] == 0
        {
            let peer_id = ((seg[6] as u32) << 16) | (seg[7] as u32);
            return Some(peer_id);
        }
    }
    None
}

// ── Shared inbox (peer_id-tagged) ─────────────────────────────────────────────

/// Inner storage type for TaggedInbox: peer_id-tagged byte payloads.
type TaggedInboxInner = Arc<Mutex<Vec<(u32, Vec<u8>)>>>;

/// Thread-safe inbox used by the WebRTC onmessage callback.
/// Each entry carries the originating `peer_id` so identity is preserved.
/// KG: taliban-fix-H2-webrtc-socket-2026-04-15
#[derive(Clone)]
pub struct TaggedInbox {
    inner: TaggedInboxInner,
}

impl TaggedInbox {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(Vec::new())) }
    }

    /// Enqueue a tagged message (called by the WebRTC onmessage handler).
    pub fn enqueue(&self, peer_id: u32, data: Vec<u8>) {
        self.inner.lock().unwrap().push((peer_id, data));
    }

    /// Drain all pending messages; returns `(peer_id, bytes)` pairs.
    pub fn drain_all(&self) -> Vec<(u32, Vec<u8>)> {
        std::mem::take(&mut self.inner.lock().unwrap())
    }
}

impl Default for TaggedInbox {
    fn default() -> Self {
        Self::new()
    }
}

// ── Outbound channel (peer_id-keyed) ─────────────────────────────────────────

/// One outbound slot per remote peer — backed by a simple function/closure.
/// In production this calls `WebRtcPeer::send_on(CH_POSITION, &bytes)`.
/// In tests a shared `TaggedInbox` stands in.
pub type SendFn = Box<dyn FnMut(&[u8]) -> Result<(), String> + Send + 'static>;

// ── WebRtcNonBlockingSocket ───────────────────────────────────────────────────

/// GGRS `NonBlockingSocket` wired to 333 WebRTC DataChannel (CH_POSITION).
///
/// * `send_to(msg, addr)` → resolves `addr` → `peer_id` → calls the registered
///   send-closure, which forwards bytes to `WebRtcPeer::send_on(CH_POSITION, …)`.
/// * `receive_all_messages()` → drains the shared `TaggedInbox`, converts each
///   `peer_id` to a synthesised `SocketAddr`, returns attributable pairs.
///
/// KG: taliban-fix-H2-webrtc-socket-2026-04-15
pub struct WebRtcNonBlockingSocket {
    /// Own synthesised address (derived from local peer_id)
    local_addr: SocketAddr,
    /// peer_id → outbound send closure (CH_POSITION)
    senders: HashMap<u32, SendFn>,
    /// Shared inbox filled by WebRTC onmessage callbacks
    inbox: TaggedInbox,
}

impl WebRtcNonBlockingSocket {
    /// Create socket for `local_peer_id`, sharing the provided `TaggedInbox`.
    ///
    /// The inbox must be the same `Arc`-clone given to the WebRTC onmessage
    /// handler so that arriving bytes end up here automatically.
    /// KG: taliban-fix-H2-webrtc-socket-2026-04-15
    pub fn new(local_peer_id: u32, inbox: TaggedInbox) -> Self {
        Self {
            local_addr: peer_id_to_addr(local_peer_id),
            senders: HashMap::new(),
            inbox,
        }
    }

    /// Register a remote peer with its outbound send closure.
    ///
    /// In production: `socket.register_peer(peer.remote_id(), Box::new(move |b| peer.send_on(CH_POSITION, b).map_err(|e| format!("{e:?}"))));`
    pub fn register_peer(&mut self, peer_id: u32, send_fn: SendFn) {
        self.senders.insert(peer_id, send_fn);
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl NonBlockingSocket for WebRtcNonBlockingSocket {
    /// Forward bytes to a remote peer via CH_POSITION.
    /// Silently drops if peer not registered (mirrors real UDP semantics).
    /// KG: taliban-fix-H2-webrtc-socket-2026-04-15
    fn send_to(&mut self, msg: &RawMessage, addr: &SocketAddr) {
        if let Some(peer_id) = addr_to_peer_id(addr) {
            if let Some(send_fn) = self.senders.get_mut(&peer_id) {
                let _ = send_fn(&msg.payload);
            }
        }
    }

    /// Drain the tagged inbox, preserving per-peer identity.
    ///
    /// Taliban H2 fix: every returned `SocketAddr` is synthesised from the
    /// originating `peer_id`, so GGRS can attribute inputs per-player.
    /// KG: taliban-fix-H2-webrtc-socket-2026-04-15
    fn receive_all_messages(&mut self) -> Vec<(SocketAddr, RawMessage)> {
        self.inbox
            .drain_all()
            .into_iter()
            .map(|(peer_id, bytes)| {
                let addr = peer_id_to_addr(peer_id);
                (addr, RawMessage { payload: bytes })
            })
            .collect()
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::socket::NonBlockingSocket;

    // ── Helper: build a back-to-back pair of WebRtcNonBlockingSockets ────────

    /// Wires socket_a (peer 1) ↔ socket_b (peer 2) through two shared inboxes.
    /// Simulates a direct WebRTC DataChannel without actual network I/O.
    fn make_pair() -> (WebRtcNonBlockingSocket, WebRtcNonBlockingSocket) {
        let inbox_a = TaggedInbox::new();
        let inbox_b = TaggedInbox::new();

        let inbox_a_clone = inbox_a.clone();
        let inbox_b_clone = inbox_b.clone();

        // socket A delivers to inbox_b; socket B delivers to inbox_a
        let mut sock_a = WebRtcNonBlockingSocket::new(1, inbox_a);
        let mut sock_b = WebRtcNonBlockingSocket::new(2, inbox_b);

        // A's send-closure drops bytes into B's inbox (tagged as peer 1)
        sock_a.register_peer(2, Box::new(move |data: &[u8]| {
            inbox_b_clone.enqueue(1, data.to_vec());
            Ok(())
        }));

        // B's send-closure drops bytes into A's inbox (tagged as peer 2)
        sock_b.register_peer(1, Box::new(move |data: &[u8]| {
            inbox_a_clone.enqueue(2, data.to_vec());
            Ok(())
        }));

        (sock_a, sock_b)
    }

    // ── Test 1: send → receive round-trip with identity preserved ────────────

    /// KG: taliban-fix-H2-webrtc-socket-2026-04-15
    #[test]
    fn roundtrip_preserves_peer_identity() {
        let (mut sock_a, mut sock_b) = make_pair();

        let payload = b"game-input-frame-42".to_vec();
        let msg = RawMessage { payload: payload.clone() };

        // A sends to B's synthesised address
        let addr_b = peer_id_to_addr(2);
        sock_a.send_to(&msg, &addr_b);

        // B drains its inbox
        let received = sock_b.receive_all_messages();
        assert_eq!(received.len(), 1, "B must receive exactly one message");

        let (from_addr, recv_msg) = &received[0];
        assert_eq!(recv_msg.payload, payload, "payload must be intact");

        // Critical H2 fix: sender identity must be peer 1, NOT 0.0.0.0:0
        let from_peer_id = addr_to_peer_id(from_addr)
            .expect("address must be a synthesised ULA addr, not 0.0.0.0:0");
        assert_eq!(from_peer_id, 1, "message must be attributed to peer 1");
    }

    // ── Test 2: multiple peers' messages preserve distinct identities ─────────

    /// Three-peer mesh: peer-1 and peer-3 both send to peer-2.
    /// Peer-2's `receive_all_messages()` must return both with correct origins.
    /// KG: taliban-fix-H2-webrtc-socket-2026-04-15
    #[test]
    fn multi_peer_identity_preservation() {
        let inbox_2 = TaggedInbox::new();

        let inbox_2_for_1 = inbox_2.clone();
        let inbox_2_for_3 = inbox_2.clone();

        let mut sock_1 = WebRtcNonBlockingSocket::new(1, TaggedInbox::new());
        let mut sock_3 = WebRtcNonBlockingSocket::new(3, TaggedInbox::new());
        let mut sock_2 = WebRtcNonBlockingSocket::new(2, inbox_2);

        sock_1.register_peer(2, Box::new(move |d| { inbox_2_for_1.enqueue(1, d.to_vec()); Ok(()) }));
        sock_3.register_peer(2, Box::new(move |d| { inbox_2_for_3.enqueue(3, d.to_vec()); Ok(()) }));
        // sock_2 doesn't need outbound for this test

        let addr_2 = peer_id_to_addr(2);
        sock_1.send_to(&RawMessage { payload: b"from-1".to_vec() }, &addr_2);
        sock_3.send_to(&RawMessage { payload: b"from-3".to_vec() }, &addr_2);

        let msgs = sock_2.receive_all_messages();
        assert_eq!(msgs.len(), 2, "peer-2 must see 2 messages");

        let mut origins: Vec<u32> = msgs.iter()
            .map(|(a, _)| addr_to_peer_id(a).unwrap())
            .collect();
        origins.sort();
        assert_eq!(origins, vec![1, 3], "origins must be peer-1 and peer-3");

        let payloads: Vec<&[u8]> = msgs.iter().map(|(_, m)| m.payload.as_slice()).collect();
        assert!(payloads.contains(&b"from-1".as_ref()));
        assert!(payloads.contains(&b"from-3".as_ref()));
    }

    // ── Test 3: peer disconnection / unregistered peer drops silently ─────────

    /// Sending to an unknown peer must not panic; message is silently dropped.
    /// KG: taliban-fix-H2-webrtc-socket-2026-04-15
    #[test]
    fn send_to_unknown_peer_drops_silently() {
        let inbox = TaggedInbox::new();
        let mut sock = WebRtcNonBlockingSocket::new(1, inbox);

        // No peers registered — send to peer 99 (unregistered)
        let addr_99 = peer_id_to_addr(99);
        sock.send_to(&RawMessage { payload: b"lost".to_vec() }, &addr_99);

        // No panic, inbox remains empty
        let received = sock.receive_all_messages();
        assert!(received.is_empty(), "unregistered peer send must be a no-op");
    }

    // ── Test 4: addr ↔ peer_id round-trip ─────────────────────────────────────

    /// KG: taliban-fix-H2-webrtc-socket-2026-04-15
    #[test]
    fn peer_id_addr_round_trip() {
        for id in [0u32, 1, 7, 255, 65535, 0x0001_0000, u32::MAX] {
            let addr = peer_id_to_addr(id);
            let recovered = addr_to_peer_id(&addr);
            assert_eq!(recovered, Some(id), "round-trip failed for peer_id={id}");
        }
    }

    // ── Test 5: non-ULA address returns None from addr_to_peer_id ─────────────

    #[test]
    fn non_ula_addr_returns_none() {
        let v4 = "127.0.0.1:9000".parse::<SocketAddr>().unwrap();
        assert_eq!(addr_to_peer_id(&v4), None);
    }
}
