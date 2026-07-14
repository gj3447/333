# 333 Platform — Determinism Contract

> KG: seed-rts-determinism-fixed-point-state-hash-2026-04-15

---

## 1. Floating-Point Elimination Policy

All game-simulation code (positions, velocities, health, cooldowns, probability checks) **MUST** use `Fixed32` (Q16.16) from `crate::determinism::fixed_point`.

| Rule | Rationale |
|------|-----------|
| **No `f32`/`f64` in simulation state** | IEEE 754 result ordering is not guaranteed across hardware/compilers |
| **No `std::cmp::partial_cmp` on floats** | `f32::NaN != f32::NaN` breaks total ordering |
| **No `f32` in `DeterministicSerialize`** | Padding/endianness may differ |
| **`sin`/`cos`/`sqrt` → `Fixed32` methods** | CORDIC/polynomial table, bitwise identical |

**Permitted float usage:**
- Rendering layer only (vertex buffers, shader uniforms)
- Audio DSP
- UI animations

---

## 2. Frame Hash Broadcast Protocol

Every **10 frames** (`HASH_BROADCAST_INTERVAL = 10`), each peer:

1. Calls `frame_state_hash(frame_n, &tactical_state) -> [u8; 32]`
2. Broadcasts the result over the signaling channel (`/ws333/`)
3. Feeds received peer hashes into `DesyncDetector::observe_peer()`

```
Frame 0:  [compute]
Frame 10: [compute] → hash → broadcast → peers compare
Frame 20: [compute] → hash → broadcast → peers compare
...
```

Peers that missed a frame's hash (e.g., late join) do **not** trigger false desyncs — `DesyncDetector` only fires when the local hash is known and the peer's hash differs.

---

## 3. Desync → Snapshot Resync Protocol

### 3.1 Detection

```
DesyncDetector::observe_peer(peer_id, frame_n, peer_hash)
  → Some(DesyncEvent { frame_n, peer_id, local_hash, peer_hash })
```

### 3.2 Resync Procedure

1. **Identify lagging node**: The node whose hash is the *minority* is treated as desynced (BFT majority rule — f < n/3 Byzantine tolerance inherited from platform BFT module).

2. **Request checkpoint**: The lagging node sends `SyncRequest { frame_n, peer_id }` to the *majority* nodes.

3. **BFT checkpoint lookup**: Each majority node checks its BFT checkpoint store for the last committed snapshot at or before `frame_n`.

4. **Snapshot transfer**: The majority node sends `SnapshotResponse { frame_n, state_bytes }` to the lagging peer.

5. **Full state replay**: The lagging node:
   a. Applies the received snapshot as the authoritative state.
   b. Replays any buffered `frame_inputs` from `snapshot_frame` to `current_frame`.
   c. Recomputes hashes for replayed frames and re-broadcasts to confirm resync.

6. **Confirmation**: If post-replay hash matches peers → resync complete. If still mismatched → escalate (kick peer or request from different majority set).

### 3.3 BFT Checkpoint Reference

Checkpoints are stored by the `crate::bft` module. Query:
```rust
bft_store.latest_checkpoint_before(frame_n) -> Option<BftCheckpoint>
```

A `BftCheckpoint` bundles `{ frame_n, state_hash, state_bytes, signatures[] }`.

---

## 4. Iteration Order Policy

All collections that contribute to `DeterministicSerialize` output **MUST** use sorted containers:

| Allowed | Forbidden |
|---------|-----------|
| `BTreeMap<K, V>` | `HashMap<K, V>` |
| `BTreeSet<T>` | `HashSet<T>` |
| `Vec<T>` (explicit sort) | `Vec<T>` (random order) |

Entity iteration order MUST be stable. Recommended: sort by `entity_id: u64` (monotonically increasing).

---

## 5. RNG Usage Contract

```rust
// CORRECT — frame-seeded, reproducible
let mut rng = DeterministicRng::seed_for_frame(session.global_seed, frame_n);
let roll = rng.next_u32_below(100);

// WRONG — platform-random, nondeterministic
let roll = rand::random::<u32>() % 100;
```

- One `DeterministicRng` per frame, re-seeded at frame start.
- Do not carry RNG state across frames (re-seed each time).
- RNG calls inside a frame must occur in a **deterministic execution order** (BTreeMap sorted entity iteration).

---

## 6. RAF Budget Enforcement (Browser)

Each game tick is triggered by `requestAnimationFrame`. The simulation step must complete within the RAF budget (typically 16ms at 60 fps):

- Fixed simulation step: 50ms logical tick (`frame_n` increments independent of wall time)
- If wall time exceeds budget: skip render, continue simulation (simulation never drops frames)
- If simulation is > 5 frames behind wall time: pause simulation, display "catching up" indicator

---

## 7. Summary: Do / Don't

| Do | Don't |
|----|-------|
| Use `Fixed32` for all simulation math | Use `f32`/`f64` in game logic |
| Use `BTreeMap` for entity maps | Use `HashMap` for serialized state |
| Use `DeterministicRng::seed_for_frame` | Use `getrandom` in simulation |
| Broadcast hash every 10 frames | Skip hash broadcasts |
| Resync from BFT checkpoint on desync | Ignore `DesyncEvent` |
| Sort entity iteration by `entity_id` | Rely on allocation order |

---

*This document is bound to KG node `seed-rts-determinism-fixed-point-state-hash-2026-04-15`.*
*Last updated: 2026-04-15*
