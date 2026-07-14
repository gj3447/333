# ADR-005: GGRS Rollback Netcode for 333 Platform
<!-- KG: seed-ggrs-bft-adapter-eval-2026-04-15 -->

**Status:** ACCEPTED (spike)
**Date:** 2026-04-15
**Deciders:** 333 Platform arch team
**Context:** 333 Platform requires frame-perfect deterministic state sync for real-time game sessions alongside its HotStuff BFT consensus layer.

---

## Problem Statement

333 needs two distinct consistency layers:

| Layer | Purpose | Latency budget |
|---|---|---|
| CRDT / HotStuff BFT | Token transfers, auction bids, ranked global actions | 200–500 ms |
| Rollback netcode (GGRS) | Per-frame game inputs (position, action) | < 50 ms |

CRDT handles eventually-consistent state; BFT handles totally-ordered events. Neither alone handles the _frame-locked_ game loop needed for deterministic real-time play. A rollback netcode layer fills that gap.

---

## Options Considered

### Option A: GGRS (`ggrs` crate, v0.10)
Rust-native GGRS protocol (GGPO-inspired). Pure rollback, no transport assumption. Designed for WebAssembly-hostile environments — it assumes the host provides a `NonBlockingSocket` trait.

### Option B: backroll-rs
Rust port of GGPO backroll. Lower-level, less maintained. Last meaningful commit ~2022. No WASM considerations.

### Option C: Custom deterministic lockstep
Build from scratch on top of existing HotStuff BFT. Simplest mental model but O(n²) message complexity and no rollback.

---

## Decision: GGRS (Option A)

### GGRS Advantages
- **Transport-agnostic**: `NonBlockingSocket` trait is a clean seam; 333's WebRTC DataChannel (`CH_POSITION`) drops in as the production impl.
- **WASM-compatible**: no `std::net::UdpSocket` in hot path; socket is caller-supplied.
- **Active maintenance**: 0.10.x release 2024; issues triaged.
- **Save/restore API**: `GGRSRequest::SaveGameState` / `LoadGameState` map directly to 333 ECS snapshot (bincode serialise → `GameStateSnapshot`).
- **Spectator support**: built-in; useful for 333 observer nodes.

### GGRS Disadvantages
- **crates.io reachability**: NAT hairpin at `bhgman.iptime.org` may block `cargo fetch` during spike. Mitigation: pre-fetch on a non-hairpin network, or use `git` dep pointing to GitHub mirror.
- **`PlayerHandle` is dense 0-indexed**: requires stable sorted validator ordering. 333's `ValidatorSet` already sorts by `NodeId`, so mapping is trivial (see § BFT Player ID Mapping).
- **No built-in Byzantine tolerance**: GGRS assumes honest peers. 333 compensates by running HotStuff BFT _in parallel_ for all economically significant events; GGRS only carries input frames.
- **Frame desync risk on WASM**: floating-point ops must be absent from ECS state; 333 ECS must use fixed-point or integer-only game logic.

### Backroll-rs Disadvantages
- Unmaintained (last commit 2022).
- No WASM documentation.
- No `NonBlockingSocket` abstraction; hardcoded to UDP.
- Smaller ecosystem; fewer examples for WebRTC integration.

---

## Performance Comparison (estimated)

| Metric | GGRS 0.10 | backroll-rs | Custom lockstep |
|---|---|---|---|
| Rollback window | configurable (default 8) | 8 frames fixed | N/A |
| Serialization overhead | caller-controlled | caller-controlled | N/A |
| WASM support | yes | unknown | yes |
| Input bandwidth (4p, 60fps) | ~240 bytes/s/peer | ~240 bytes/s/peer | ~960 bytes/s (lockstep) |
| Maintenance | active | stale ~2yr | internal |
| Integration complexity | low (trait boundary) | medium | high |

---

## BFT Player ID Mapping

**Rule:** `PlayerHandle = index of NodeId in sorted(ValidatorSet.validators)`.

Rationale:
1. `ValidatorSet` in `bft/crypto.rs` already sorts validators at construction time.
2. All peers share the same `ValidatorSet`, so sort order is globally agreed.
3. Mapping is O(1) lookup via `BftPlayerMap` (two `HashMap<u32,u32>` fields).

Consequence: validator set changes (join/leave) require a new GGRS session. Session lifetime equals one 333 room lifetime — acceptable for the current architecture.

Optional: `BftPlayerMap::register_pubkey(node_id, peer_id.0)` binds the Ed25519 public key to the handle for post-session audit (no runtime cost).

---

## Socket Adapter Architecture

```
WebRTC DataChannel (CH_POSITION, unreliable+unordered)
          │
          ▼
WebRtcNonBlockingSocket
  implements NonBlockingSocket trait
          │
          ▼
BftGgrsSession::advance_frame()
  → GgrsRequest::AdvanceFrame { inputs }
          │
          ▼
333 ECS world.advance(inputs)
  → GameStateSnapshot (bincode + blake3 checksum)
```

For confirmations / desync detection, `GgrsRequest::SaveGameState` checksums are broadcast on `CH_BFT` (reliable) and compared against remote checksums via HotStuff BFT vote payload.

---

## 333 ECS Compatibility Checklist

- [x] ECS state is serializable to `Vec<u8>` (bincode / postcard)
- [x] ECS contains no floating-point game logic (use i32 fixed-point)
- [x] ECS `advance(inputs)` is a pure function (no WASM random, no `Date.now()`)
- [ ] **TODO**: wire `CH_POSITION` DataChannel drain into `WebRtcNonBlockingSocket::receive_all_messages`
- [x] **DONE 2026-04-15**: replaced FNV-64 checksum in `compute_checksum` with blake3 — `*blake3::hash(data).as_bytes()`. <!-- KG: taliban-fix-C2-2026-04-15 -->

---

## Known Limitations (spike scope)

1. `WebRtcNonBlockingSocket` uses `std::sync::mpsc` — not the real WebRTC DataChannel.
2. `compute_checksum` uses FNV-64 padded to 32 bytes; production should use blake3.
3. No spectator session wiring in this spike.
4. `ggrs` dep is commented out in `Cargo.toml` pending network fetch verification.
5. No adaptive input delay (GGRS feature) tuning done; defaults assumed.

---

## ADR Links

- KG: `seed-ggrs-bft-adapter-eval-2026-04-15`
- KG: `SPAN_333_BFT_Types` (HotStuff message types)
- KG: `CONTRACT_SharedType_DataChannelSet` (CH_POSITION / CH_BFT)
- KG: `lesson-333-bft-keyring-exchange-2026-04-14`
