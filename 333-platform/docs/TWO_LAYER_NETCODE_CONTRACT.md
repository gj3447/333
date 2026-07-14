# Two-Layer Netcode Contract

> KG: seed-rts-bft-checkpoint-not-per-frame-2026-04-15

## Overview

The 333 RTS uses a **two-layer netcode** to balance latency and Byzantine fault tolerance:

| Layer | Mechanism | Period | Trust model |
|---|---|---|---|
| **Fast** | P2P direct lockstep + speculative execution | 16–33 ms/frame | Optimistic (rollback on mismatch) |
| **Slow** | BFT checkpoint (HotStuff) | 10–30 s | Byzantine-tolerant (f < n/3) |

---

## Fast / Slow Boundary

```
Frame advance (16-33 ms)
  ↓
Every 10 frames → state_hash broadcast to all peers  ← FAST HASH BOUNDARY
  ↓
Every 1000 frames (~16-33 s) → BFT checkpoint proposal  ← SLOW BFT BOUNDARY
```

- The **fast layer** trusts all peers speculatively. Hash mismatches trigger rollback.
- The **slow layer** audits: once a `QuorumCert` is obtained for a checkpoint, that frame becomes **immutable**. No rollback can cross the BFT floor.
- Input enters the fast queue immediately via `submit_input(local)` then is included in `advance_frame(inputs)`. After `BFT_CHECKPOINT_INTERVAL_INTERVALS × FAST_HASH_INTERVAL` frames the slow layer proposes the checkpoint.

---

## Latency Budget

| Event | Target |
|---|---|
| Frame advance | 16–33 ms |
| State hash broadcast | every 160–330 ms (10 frames) |
| Rollback detection | ≤ 160–330 ms (next hash interval) |
| BFT checkpoint quorum | ≤ 2 s (HotStuff 3-phase, LAN) |
| BFT checkpoint period | 10–30 s |

---

## Rollback Window — 5-Frame Cap

```rust
pub const MAX_ROLLBACK_FRAMES: u64 = 5;
```

`rollback_on_desync(target_frame)` returns `Err` if `current_frame - target_frame > 5`.

**Rationale:** Beyond 5 frames (~80–165 ms), speculative state has diverged too far for
cheap replay. Instead, `restore_from_checkpoint()` performs a hard reset to the last BFT
floor — more expensive but Byzantine-safe.

**Implication:** A peer that consistently diverges more than 5 frames is either a Byzantine
node or has catastrophic lag. The slow layer's checkpoint audit will detect this within
one BFT period.

---

## Input Flow

```
Player action
  │
  ▼
submit_input(local)        → InputQueue (VecDeque<u8>)
  │
  ▼
advance_frame(inputs)      → apply + bump frame_n
  │
  ├─ frame_n % 10 == 0?   → broadcast state_hash to peers
  │                           └─ FrameDivergenceDetector::record_local()
  │                               peers call observe_peer() → DesyncEvent?
  │                                 └─ YES → rollback_on_desync(target)
  │
  └─ frame_n % 1000 == 0? → SlowBftCheckpoint::create_checkpoint(frame_n, hash)
                              └─ BftCheckpointProvider::propose_checkpoint()
                                  └─ QuorumCert obtained
                                      └─ FastLockstepLoop::set_checkpoint_floor()
```

---

## Byzantine Scenarios

| Scenario | Fast layer response | Slow layer response |
|---|---|---|
| Peer sends wrong hash (packet loss / bug) | `DesyncEvent` → rollback if within 5 frames | No action (transient) |
| Peer consistently diverges > 5 frames | `rollback_on_desync` fails → `restore_from_checkpoint` | BFT period audit; peer's checkpoint rejected if hash differs |
| Peer submits false checkpoint hash | Fast layer unaware | `BftCheckpointProvider::propose_checkpoint` fails — Byzantine node's proposal lacks quorum |
| Peer forges QC signatures | Fast layer unaware | `verify_checkpoint_qc` returns false → `accept_bft_checkpoint` returns `Err` |
| n/3 or more Byzantine nodes | Fast layer may accept invalid frames | BFT safety fails (by assumption: f < n/3) — out of scope |

**Fast layer trusts, slow layer audits.** The slow layer is the authoritative record.
A Byzantine node that passes the fast layer will be caught at the next checkpoint epoch
provided f < n/3.

---

## Phase 1–3 Integration Points

### Phase 1 — TacticalState (rts_state_tiers.rs)
- `FastLockstepLoop::state_accumulator` (stub `u64`) must be replaced with `TacticalState`.
- `compute_hash()` must call `frame_state_hash(frame_n, &tactical_state)`.
- `FrameSnapshot::state_bytes` must store a serialised `TacticalState` clone.

### Phase 2 — Determinism (determinism/state_hash.rs)
- Already integrated: `frame_state_hash` and `should_broadcast_hash` are imported.
- `DesyncDetector` is wrapped by `FrameDivergenceDetector`.
- No changes to determinism layer required.

### Phase 3 — GGRS / BFT integration
- Replace `StubBftProvider` / `OkProvider` with a real `HotStuffAdapter` that:
  - Wraps `crate::bft::state::HotStuffState` for consensus rounds.
  - Calls `crate::bft::executor::Executor::execute_block` on committed BFT blocks.
  - Returns a real `QuorumCert` with Ed25519 multi-signatures.
- Replace `FrameDivergenceDetector::verify_bft_signature` stub with:
  ```rust
  slow_layer.verify_peer_checkpoint(&handle, qc)
  ```
- GGRS rollback frames should be bounded by `MAX_ROLLBACK_FRAMES`.

---

## Known Limitations

1. **No real BFT consensus in this seed.** `BftCheckpointProvider` is a trait with stub
   implementations. Actual HotStuff wiring is a separate seed
   (`seed-rts-hotstuff-adapter-YYYY-MM-DD`).

2. **state_accumulator is a stub.** `FastLockstepLoop` uses a `u64` XOR accumulator
   instead of `TacticalState`. The hash is structurally correct but not game-meaningful
   until Phase 1 integration.

3. **No network transport.** Hash broadcasts and checkpoint messages are method calls
   within the same process. P2P transport integration is a Phase 3 concern.

4. **Rollback restores only fast-layer stub state.** `restore_from_checkpoint` resets
   `state_accumulator` from `checkpoint_hash[0..8]`. Real restore must replay
   `TacticalState` from a full snapshot stored off-heap (MinIO / local-path).

5. **BFT signature verification is always-true in tests.** The `verify_bft_signature`
   stub accepts genesis QC and any non-empty QC. Production must replace with
   Ed25519 multi-sig verification via `ValidatorKeyring`.

---

*Seed: seed-rts-bft-checkpoint-not-per-frame-2026-04-15*
*Files: src/netcode/mod.rs, src/netcode/two_layer.rs, src/netcode/divergence.rs*
