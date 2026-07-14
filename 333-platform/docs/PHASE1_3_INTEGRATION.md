# Phase 1-3 Integration: RtsSession Wiring

> Status: Skeleton complete. Game-logic stubs only. Phase 4 BFT checkpoint wiring pending.
> # KG: seed-rts-integration-wiring-2026-04-15

---

## Flow Diagram

```
advance_frame(inputs: &[u8])
│
├─ [1] GGRS save_state
│       TacticalState ──DeterministicSerialize──► Vec<u8>
│       blake3(Vec<u8>) ──────────────────────────► [u8;32] checksum
│       GgrsStub::save_state(buf) → GgrsSaveCell
│
├─ [2] Apply inputs (skeleton)
│       inputs.is_empty() → step = Fixed32(0) / Fixed32(1)
│       ∀ unit: unit.x += step, unit.y += step
│       tactical.turn_number += 1
│
├─ [3] Hash (TacticalHashView)
│       TacticalHashView(&tactical)
│         └─ turn_number + units + player_minerals + visibility
│              (HLC excluded — node-local, not shared game state)
│       frame_state_hash(frame_n, view) ──blake3──► [u8;32]
│       tactical.state_hash ← hash
│
├─ [4] bridge.capture(frame_n, TacticalDigest, PersistentDigest, CriticalDigest)
│       TacticalDigest  = { state_hash, unit_count, total_minerals }
│       PersistentDigest = { entry_count: 0, max_hlc_counter: 0 }  ← stub
│       CriticalDigest   = { bft_view: 0, ... }                     ← stub
│       → FrameEpoch stored in bridge.epoch_buffer (ring, capacity N)
│
└─ [5] desync.record_local(frame_n, hash)
        local_hashes[frame_n] ← hash  (eviction after retention cap)

observe_peer_digest(peer_id, frame_n, peer_hash)
        desync.observe_peer(peer_id, frame_n, peer_hash)
          └─ local_hashes[frame_n] != peer_hash
               → Some(DesyncEvent { frame_n, peer_id, local_hash, peer_hash })
               → eprintln! log  (Phase 4: escalate to BFT DesyncProof tx)
```

---

## Mermaid Diagram

```mermaid
sequenceDiagram
    participant Caller
    participant RtsSession
    participant GgrsStub
    participant TacticalState
    participant FrameEpochBridge
    participant DesyncDetector

    Caller->>RtsSession: advance_frame(inputs)
    RtsSession->>TacticalState: det_serialize → buf
    RtsSession->>GgrsStub: save_state(buf)
    GgrsStub-->>RtsSession: GgrsSaveCell (frame, checksum)
    RtsSession->>TacticalState: apply inputs (x/y += Fixed32 step)
    RtsSession->>RtsSession: frame_state_hash(frame_n, TacticalHashView)
    RtsSession->>FrameEpochBridge: capture(frame_n, digests...)
    FrameEpochBridge-->>RtsSession: FrameEpoch
    RtsSession->>DesyncDetector: record_local(frame_n, hash)
    RtsSession-->>Caller: [u8; 32] hash

    Caller->>RtsSession: observe_peer_digest(peer_id, frame_n, peer_hash)
    RtsSession->>DesyncDetector: observe_peer(peer_id, frame_n, peer_hash)
    DesyncDetector-->>RtsSession: Option<DesyncEvent>
    RtsSession-->>Caller: Option<DesyncEvent>
```

---

## Responsibility Table

| Component | Module | Responsibility | Phase |
|---|---|---|---|
| `TacticalState` | `apps::rts_state_tiers` | Per-frame deterministic game snapshot (Tier 1) | 1 |
| `UnitTactical` | `apps::rts_state_tiers` | Fixed32 unit positions, HP, cooldowns | 1 |
| `FrameEpochBridge` | `apps::rts_state_tiers` | Per-frame HLC anchor binding all 3 tiers | 1 |
| `TacticalDigest` | `apps::rts_state_tiers` | Minimal hash + unit_count for desync broadcast | 1 |
| `Fixed32` | `determinism::fixed_point` | Q16.16 deterministic arithmetic (no f32) | 1 |
| `DeterministicSerialize` | `determinism::state_hash` | Platform-stable byte serialization trait | 1 |
| `frame_state_hash` | `determinism::state_hash` | blake3 over (domain sep + frame_n + state) | 2 |
| `DesyncDetector` | `determinism::state_hash` | BTreeMap of local hashes, mismatch detection | 2 |
| `DesyncEvent` | `determinism::state_hash` | Frame + peer + local/peer hash diff record | 2 |
| `TacticalHashView` | `apps::rts_session` | Hash-view newtype (excludes node-local HLC) | 2 |
| `GgrsStub` | `apps::rts_session` | Inline GGRS mock (save/load/advance without dep) | 3 |
| `RtsSession` | `apps::rts_session` | Integration struct — wires 1-3 into one `advance_frame` | 3 |
| `BftGgrsSession` | `crates/333-ggrs-adapter` | Real rollback session (separate workspace crate) | 3→4 |

---

## Key Design Decisions

### HLC excluded from game-state hash
`TacticalState.hlc` is a node-local hybrid logical clock — its `node_id` and `wall_ms` differ
between peers even for identical game states. Including it in the hash would trigger false
desync events. `TacticalHashView` wraps `TacticalState` and serializes only the shared
game-logic fields: `turn_number`, `units`, `player_minerals`, `visibility`.

### GgrsStub as inline mock
`triple-three-ggrs-adapter` is a workspace member but is not yet a direct dependency of the
main `triple-three` crate (no `[dependencies]` entry in root `Cargo.toml`). `GgrsStub` provides
the minimum `save_state / load_state / advance` surface so `RtsSession` compiles and tests pass
without pulling in the adapter crate. Replace with `BftGgrsSession` when the dep is wired.

### No real game logic
`advance_frame` applies a skeleton: units advance x/y by `Fixed32(1)` if any input bytes are
present, `Fixed32(0)` otherwise. This is sufficient for determinism tests. Real command dispatch
(move, attack, build) belongs in the game engine layer above `RtsSession`.

---

## Next Step: Phase 4 BFT Checkpoint Integration

Phase 4 connects `DesyncEvent` to the BFT layer:

1. **DesyncEvent → DesyncProof tx**: when `observe_peer_digest` returns `Some(ev)`,
   construct a `CriticalEvent { action_type: CriticalActionType::DesyncProof, ... }` and
   submit to `HotStuff` via `CH_BFT`.

2. **ReplayAnchor tx at checkpoint frames**: every N frames (e.g., 100), emit a
   `CriticalActionType::ReplayAnchor` tx containing the `FrameEpoch` digest. This
   pins the BFT log to the lockstep state for deterministic replay from cold start.

3. **Wire real `BftGgrsSession`**: add `triple-three-ggrs-adapter` as a `[dependencies]`
   entry in the root `Cargo.toml`. Replace `Option<GgrsStub>` with
   `Option<triple_three_ggrs_adapter::BftGgrsSession>`.

4. **VictoryDeclaration quorum**: after BFT commits `VictoryDeclaration`, `RtsSession`
   calls `CriticalEventHandler::handle_critical(event)` to finalize match state.

Integration hook point in code:
```rust
// src/apps/rts_session.rs — observe_peer_digest
if let Some(ev) = event {
    // TODO Phase 4: escalate to BFT DesyncProof
    // self.bft_sender.send(build_desync_proof_tx(&ev));
}
```

> # KG: seed-rts-integration-wiring-2026-04-15
