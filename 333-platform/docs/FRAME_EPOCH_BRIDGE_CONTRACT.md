# Frame Epoch Bridge — Synchronization Contract

> # KG: seed-rts-3tier-state-classification-2026-04-15
> Design phase contract. Integration (wiring to RtsGame, SyncManager, HotStuffNode) is
> a separate subsequent task. This document is the authoritative specification for that
> integration.

---

## 1. Purpose

`FrameEpochBridge` provides a single per-turn causal anchor that binds the three
independent synchronization tiers:

```
Tier 1 (CH_POSITION, lockstep)   ─┐
Tier 2 (CH_CRDT, LwwMap delta)   ─┤── FrameEpoch { frame N, epoch_hlc, digests }
Tier 3 (CH_BFT, HotStuff order)  ─┘
```

Without the bridge, each tier drifts on its own timeline. With it, every peer can
reconstruct exactly which CRDT entries, BFT commits, and tactical states belong to
frame N.

---

## 2. HLC Binding Contract

### Rule: epoch_hlc is the causal upper bound for frame N

```
epoch_hlc > any HLC embedded in:
  - TacticalState.hlc (from lockstep Turn N)
  - LwwEntry.timestamp for any delta flushed during Turn N's 20 ms window
  - CriticalEvent.commit_hlc for any BFT Decide during Turn N
```

**Implementation requirement**: `FrameEpochBridge::capture()` MUST call
`self.hlc.tick()` AFTER all of the following for frame N are complete:

1. `RtsGame::process_turn(turn_N)` — Tier 1 frozen
2. `SyncManager::poll_outgoing()` — Tier 2 batch flushed
3. `HotStuffNode` has emitted all `ProcessResult::Committed` events from Turn N's
   message window — Tier 3 settled

Violation: any tier event whose HLC exceeds `epoch_hlc` for its claimed frame is
**invalid** and must be discarded or re-bucketed to the next frame.

### Remote epoch merge

When a `FrameEpoch` arrives from a peer:
```
bridge.recv_remote_epoch(&remote_epoch)
  → self.hlc.recv(&remote_epoch.epoch_hlc)
```

This preserves the happens-before order: if peer P committed frame N with
`epoch_hlc = T`, our subsequent local tick will satisfy `local_hlc > T`.

---

## 3. Snapshot Timing Diagram (Frame N)

```
t=0ms   ─── Turn N-1 ends ───────────────────────────────────────────────────
t=0ms        FrameEpochBridge::capture(N-1, ...)  ← epoch_hlc ticked
t=0ms        Lockstep buffer: Turn N commands collected from all peers

t=50ms  ─── Turn N window ──────────────────────────────────────────────────
t=50ms       RtsGame::process_turn(Turn N)         ← Tier 1 frozen
t=50ms       BFT window closes: collect ProcessResult::Committed for frame N
t=50ms   [20ms]  SyncManager::poll_outgoing()      ← Tier 2 flush #1
t=70ms   [20ms]  SyncManager::poll_outgoing()      ← Tier 2 flush #2  ┐
             (one 50ms turn may span up to 3× 20ms CRDT batches)       │
             All CRDT deltas tagged to frame N must have HLC < epoch_hlc│

t=100ms ─── Turn N ends ─────────────────────────────────────────────────────
t=100ms      FrameEpochBridge::capture(N, tactical_digest, crdt_digest, bft_digest)
             → hlc.tick()  ← epoch_hlc: causally after all above events
```

---

## 4. TacticalDigest Population Contract

| Field | Source | Invariant |
|---|---|---|
| `state_hash` | `RtsGame::state_hash()` after `process_turn()` | Must match all peers (lockstep guarantee) |
| `unit_count` | `RtsGame::all_units().len()` | Cross-check only; mismatch ≠ desync by itself |
| `total_minerals` | Sum of `player_minerals` values | Cross-check only |

`state_hash` is the **only authoritative desync detector** at Tier 1. A mismatch
on `state_hash` between two peers' `FrameEpoch` for the same `frame` MUST trigger
the desync escalation path (§6).

---

## 5. PersistentDigest Population Contract

| Field | Source | Invariant |
|---|---|---|
| `entry_count` | `LwwMap::len()` at CRDT flush time | Eventual — may differ transiently across peers |
| `max_hlc_counter` | Max of `StateVector` values after `poll_outgoing()` | Monotonically increasing per node |

PersistentDigest fields are **informational**. A difference does not constitute a
fault — CRDT convergence will resolve it. The digest exists for drift monitoring
(e.g., Grafana alert if two peers' `entry_count` diverges by > 100 for > 5 frames).

---

## 6. CriticalDigest Population Contract

| Field | Source | Invariant |
|---|---|---|
| `bft_view` | `HotStuffNode::current_view()` at end of Turn N | View is monotonically increasing |
| `last_committed_block_hash` | Most recent `ProcessResult::Committed` block hash this frame | 0 if no commit this frame |
| `committed_tx_count` | Sum of `Vec<OrderedTx>` lengths from all `Committed` results this frame | 0 if no commits |

If `committed_tx_count > 0`, the bridge epoch serves as a **replay anchor**. The
integrating layer SHOULD submit a `RankedAction { action_type: 0x03 }` to BFT
every `REPLAY_ANCHOR_INTERVAL` frames (recommended: 100 frames = ~5 seconds at
50 ms/turn).

---

## 7. Desync Detection and Escalation

```
Peer A sends FrameEpoch(N) to Peer B
  → B calls bridge.check_desync(&local_N, &remote_N)

check_desync returns false
  → B builds DesyncProof payload:
      - local_epoch: FrameEpoch (serialized)
      - remote_epoch: FrameEpoch (serialized)
      - diverging_turn_hash: local.tactical.state_hash XOR-diff
  → B submits OrderedTx::RankedAction {
        player: B.node_id,
        action_type: 0x02,  // CriticalActionType::DesyncProof
        payload: postcard::to_allocvec(&desync_proof)
    }
  → BFT consensus determines authoritative epoch
  → Minority peer(s) replay from last clean FrameEpoch in their buffer
```

---

## 8. Reconnect / New Peer Recovery Protocol

```
New peer P joins at frame M (where the game is at frame N, N >> M)

Step 1 — Tier 3 (BFT history)
  P requests missing blocks from any honest peer (view 0..current_view)
  Replays all ProcessResult::Committed events

Step 2 — Tier 2 (CRDT full state)
  P requests SyncPayload::FullState from any peer
  SyncManager::create_full_snapshot() → CH_CRDT

Step 3 — Tier 1 (turn replay)
  P requests Turn buffer from any peer (last BUFFER_CAPACITY turns)
  Replays process_turn() for each buffered turn
  Verifies TacticalDigest.state_hash matches FrameEpoch for latest turn

Step 4 — Epoch sync
  P calls bridge.recv_remote_epoch() for the latest received FrameEpoch
  P is now causally aligned and can participate in Turn N+1
```

---

## 9. Channel Routing Summary

```rust
// Pseudo-routing dispatch (not yet implemented — integration task)
match channel {
    "CH_POSITION" => {
        // Decode Turn, call RtsGame::process_turn()
        // Then: bridge.capture(frame, tactical_digest, ...)
    }
    "CH_CRDT" => {
        // Decode SyncPayload via SyncManager::process_incoming()
        // Tier 2 digest updated asynchronously; captured at next epoch
    }
    "CH_BFT" => {
        // Decode HotStuffMsg, feed to HotStuffNode::process()
        // On ProcessResult::Committed → push CriticalEvent
        // CriticalDigest updated; captured at next epoch
    }
}
```

---

## 10. Key Invariants (summary)

| # | Invariant | Enforced by |
|---|---|---|
| I-1 | `epoch_hlc` > all HLCs from frame N events | `FrameEpochBridge::capture()` tick order |
| I-2 | `TacticalDigest.state_hash` identical across all peers for same frame | Lockstep determinism |
| I-3 | `PersistentDigest` eventually converges | LwwMap CRDT properties |
| I-4 | `CriticalDigest` reflects committed BFT log | HotStuff safety guarantee |
| I-5 | Desync on I-2 triggers BFT DesyncProof (action_type 0x02) | `check_desync()` caller |
| I-6 | Replay anchors submitted every `REPLAY_ANCHOR_INTERVAL` frames | Integration layer |
| I-7 | New peer recovery follows Steps 1→2→3→4 in order | Protocol contract |
