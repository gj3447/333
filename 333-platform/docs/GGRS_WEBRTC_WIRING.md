# GGRS ↔ 333 WebRTC DataChannel Wiring

> KG: taliban-fix-H2-webrtc-socket-2026-04-15

## Problem (Taliban H2)

The mock `WebRtcNonBlockingSocket` in `crates/333-ggrs-adapter/src/socket.rs` stored
all inbound messages in a single untagged `mpsc::Receiver`. When
`receive_all_messages()` drained that queue, every message was attributed to the
sentinel `0.0.0.0:0`. GGRS cannot apply per-peer input attribution with a
uniform fake address — all inputs collapse to one phantom player.

## Solution

`crates/333-ggrs-adapter/src/webrtc_socket.rs` introduces `WebRtcNonBlockingSocket`
(re-exported as `RealWebRtcSocket`) that uses a `TaggedInbox` — a shared
`Arc<Mutex<Vec<(u32, Vec<u8>)>>>` — where every slot carries the originating
`peer_id`. The WebRTC `onmessage` handler enqueues `(peer_id, bytes)`, not bare
bytes.

## peer_id → SocketAddr Synthesis

```text
prefix : fd00::/16  (ULA, RFC 4193 — never publicly routed)
layout : fd00:0000:0000:0000:0000:0000:XXXX:YYYY
         where XXXX = peer_id >> 16, YYYY = peer_id & 0xFFFF
port   : 0
```

Examples:

| peer_id | Synthesised SocketAddr |
|---------|----------------------|
| 1       | `[fd00::1]:0`        |
| 7       | `[fd00::7]:0`        |
| 65537   | `[fd00::1:1]:0`      |
| 4294967295 | `[fd00::ffff:ffff]:0` |

`addr_to_peer_id()` reverses the mapping deterministically. Any non-ULA address
returns `None` so non-synthesised addresses are never misinterpreted.

## Channel Selection: CH_POSITION

GGRS lockstep input messages use `CH_POSITION` (unreliable, unordered, 50 ms
lifetime). Rationale:

- Input messages are latency-critical; a 50 ms old input is useless.
- GGRS handles missing inputs internally (prediction + rollback) — TCP-level
  retransmit adds latency without correctness benefit.
- `CH_BFT` (reliable) is reserved for consensus/confirmation messages.
- `CH_CRDT` (reliable, ordered) is reserved for state sync.

## Production Wiring (one-time setup)

```rust
// Inside game session init, after WebRTC peers are Connected:
let inbox = TaggedInbox::new();
let mut socket = RealWebRtcSocket::new(local_peer_id, inbox.clone());

for peer in connected_peers {
    let pid = peer.remote_id();
    let inbox_clone = inbox.clone(); // peer's onmessage handler uses this
    // Wire onmessage → inbox (already done in WebRtcPeer::attach_handlers
    // for local channels; for the GGRS socket, inject your own closure or
    // add a drain_channel(CH_POSITION) poll in the game tick):

    socket.register_peer(pid, Box::new(move |data| {
        peer.send_on(CH_POSITION, data)
            .map_err(|e| format!("{e:?}"))
    }));
}
```

For receiving, either:
1. Poll `peer.drain_channel(CH_POSITION)` each tick and call
   `inbox.enqueue(peer_id, bytes)` yourself (pull model), or
2. Wire the WebRtcPeer `onmessage` closure to call `inbox.enqueue(peer_id, …)`
   directly (push model — zero-latency, preferred).

## Fallback on Send Failure

`send_to()` on an unregistered or disconnected peer is a silent no-op (mirrors
UDP). The GGRS layer detects the missing input via its input-missing timeout and
falls back to prediction, then rollback when the late input arrives. No explicit
error propagation is needed at the socket layer.

## Files

| File | Purpose |
|------|---------|
| `crates/333-ggrs-adapter/src/socket.rs` | Original mock (mpsc, PRESERVED) |
| `crates/333-ggrs-adapter/src/webrtc_socket.rs` | H2 fix — real WebRTC wiring |
| `crates/333-ggrs-adapter/src/lib.rs` | `pub mod webrtc_socket` + re-exports |

<!-- KG: taliban-fix-H2-webrtc-socket-2026-04-15 -->
