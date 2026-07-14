# Peer Eject Protocol
<!-- # KG: seed-rts-verify-bft-vote-peer-eject-2026-04-15 -->

BFT-backed peer eject for the 333-platform RTS netcode layer.

---

## 1. AoE2 / SC2 Drop Handling Comparison

| Dimension | AoE2 DE | SC2 | 333-Platform |
|---|---|---|---|
| **Detection trigger** | Host notices missing packets for ~5 s | ~30 s grace period | AFK timeout (configurable 500 ms–2 s) |
| **Who decides** | Host unilaterally | Majority vote among clients | BFT quorum (f < n/3 Byzantine) |
| **Blocking the game** | Game pauses briefly while host re-syncs | Observer popup; optional resume | Non-blocking — fast lockstep continues during background vote |
| **Input replacement** | Host injects "resign" action | Null / observer input | `null_input_for_ejected(peer, frame)` — deterministic no-op |
| **False positive guard** | Reconnect window (limited) | Reconnect / spectate offered | Late-but-valid input cancels pending vote |
| **Replay correctness** | Host is source of truth | Server-authoritative replay | Null input is deterministic: same (peer, frame) → identical bytes |

---

## 2. Timeout Tuning: 500 ms / 1000 ms / 2000 ms

### 500 ms — Fast (competitive / tournament)

**Pros:** Dropped peer does not stall the game meaningfully; minimal ghost-input frames.  
**Cons:** High false-positive risk on mobile / high-latency connections; a lag spike easily triggers an eject vote.  
**Best for:** LAN tournaments, low-latency fiber-only lobbies.

### 1000 ms — Standard (default recommendation)

**Pros:** Balances responsiveness with tolerance for brief packet loss bursts (router jitter, WiFi).  
**Cons:** Up to ~60 null frames at 16 ms/frame before the vote even starts.  
**Best for:** General online multiplayer, ranked mode.

### 2000 ms — Lenient (casual / mobile)

**Pros:** Comfortable for mobile connections; reconnect-before-eject is achievable.  
**Cons:** 125 null frames before detection; opponents may experience a ghost peer for 2+ seconds.  
**Best for:** Casual lobbies, regions with high baseline latency.

---

## 3. False Positive Prevention

### 3.1 Grace Period via `record_input` Cancellation

`PeerEjectController::record_input(peer, frame)` always updates `last_input_seen` to `Instant::now()`.
If a late-but-valid packet arrives **after** `start_eject_vote` has been called but **before** `apply_vote_result`, the pending vote is cancelled immediately.
The peer is never added to the ejected set.

### 3.2 BFT Quorum Requirement

`start_eject_vote` is only a stub broadcast; the actual eject only fires after `apply_vote_result(peer, approved=true)` is called.
In production this maps to a `ConsensusKick` `OrderedTx` through HotStuff's 3-phase commit.
Quorum requirement: ⌊(n-1)/3⌋ + 1 approvals → eject. A single Byzantine peer cannot force an eject.

### 3.3 One Vote at a Time

`pending_vote: Option<EjectVote>` enforces serial votes. A second AFK detection while a vote is in-flight is ignored until the current vote resolves. This prevents vote flooding that could be exploited to eject legitimate slow peers in rapid succession.

---

## 4. Background Vote Does Not Block the Fast Lockstep Loop

The `PeerEjectController` operates entirely in the **slow BFT layer** (see `docs/FRAME_EPOCH_BRIDGE_CONTRACT.md`).

```
Fast layer  : frame N → frame N+1 → … (16–33 ms, no wait for vote)
              ↓ (every 10 frames)
              state_hash broadcast → desync check
                                          ↓ (every N×10 frames)
Slow layer  : BFT checkpoint / eject vote (runs concurrently, 500 ms – 2 s window)
```

Key properties:
- `tick(now)` reads `last_input_seen` (immutable during fast loop) — zero contention.
- `start_eject_vote` enqueues a BFT message; it does not acquire any fast-loop lock.
- `null_input_for_ejected` is a pure function with no shared state — it can be called from the deterministic replay path without any synchronization.
- The game only "feels" the eject at the BFT-commit boundary (a `FrameEpoch` checkpoint), not at the per-frame level.

This matches the SC2 observer model: the eject decision is globally ordered via BFT, but individual frames keep advancing with null inputs until the commit is applied.

---

*# KG: seed-rts-verify-bft-vote-peer-eject-2026-04-15*
