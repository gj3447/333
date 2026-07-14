# RTS UI Skeleton — Section Responsibilities & TODO List

> KG: seed-post-rts-ui-skeleton-2026-04-15
> KG: seed-rts-integration-wiring-2026-04-15

## Files Created

| File | Purpose |
|---|---|
| `src/routes/rts/+page.svelte` | Main RTS page — 6-section UI skeleton |
| `src/routes/rts/+page.ts` | SvelteKit load — URL param injection for harness testing |
| `src/routes/rts/rts_controller.ts` | UI↔WASM bridge (currently MOCK, see TODO_WASM) |
| `src/routes/rts/+page.scss` | Minimal grid + dark theme styles (not auto-imported) |

---

## Section Responsibilities

### Section 1 · Lobby
- Room name input (create / join via signaling-333 WebSocket)
- Peer list display (live-polled from `RoomState.peers` every 500ms)
- Start Match / End Match / Leave Room controls
- Mirrors `room/+page.svelte` signaling pattern exactly (`window.__333_signaling` override supported)

### Section 2 · Match
- Current frame number (60fps RAF loop via `RtsController`)
- HLC timestamp (mock: `frame.wallMs%100000.nodeId`)
- Local unit table: 4 units with Fixed32 x/y (displayed as `integer.fraction` with 3 decimal places)
- WASD + arrow key input → `setInput({ dx, dy })` → consumed each frame by `advanceFrame`
- Unit 0 is highlighted as the player-controlled unit

### Section 3 · Desync Monitor
- Last 10 frame state_hash entries (ring buffer, newest first)
- Per-frame: local hash (truncated 10 chars), per-peer hash, OK/DESYNC status
- `DesyncEvent` alerts shown below table (last 3 events)
- Demo: simulated desync every 47 frames for visual validation

### Section 4 · BFT Checkpoint
- Last checkpoint frame number
- QC status badge: `approved` / `pending` / `failed`
- Validator quorum count (approved/total)
- Mock: fires every 30 frames with 3/4 approved

### Section 5 · Peer Eject
- AFK peer list with vote progress bar
- Vote button increments local vote count
- Threshold = 3 votes → eject (mock, not networked yet)
- Demo peer injected 2 seconds after match start

### Section 6 · Debug Log
- Last 50 log lines (newest first)
- Color-coded: cyan = info, gold = warn (bft/eject), red = error/desync

---

## TODO List — Not Yet Implemented

### TODO_WASM — requires RtsSession WASM export

1. **`initRtsSession`** — call `wasm.rts_session_new(peers, mySeed)` instead of mock RAF loop.
   - RtsSession defined in `src/apps/rts_session.rs` (Rust).
   - Needs WASM export bindings in `wasm-bridge.ts`.

2. **`advanceFrame`** — call `wasm.rts_advance_frame(input)` and parse JSON result.
   - Result shape: `{ frame, hlc, units: [{id,x,y}], state_hash }`.
   - Fixed32 values are `i32` (scale 1000); display as `v/1000.v%1000`.

3. **`onFrameStateHash`** — wire to WASM callback / WebRTC `position` channel broadcast.
   - Peers broadcast their `(frame, hash)` pairs after each `advance_frame`.
   - `rts_controller.ts:onFrameStateHash(peerId, frame, hash)` is ready to receive.

4. **BFT Checkpoint** — connect to `GgrsStub.save_state` + BFT quorum verification.
   - `RtsSession.ggrs: Option<GgrsStub>` — call `save_state` every N frames.
   - QC: collect BFT-signed checkpoint confirmations from peers via `bft` channel.

5. **Peer Eject** — wire to peer liveness tracking + BFT-signed eject proposal.
   - Liveness: detect peers with no `position`-channel message for >3 seconds.
   - Eject proposal: BFT vote broadcast → collect signatures → execute.

6. **GGRS rollback** — `GgrsStub.load_state(frame)` for input prediction rollback.
   - Currently frame loop is lockstep (no prediction). GGRS integration is Phase 4.

### TODO_UI — UX improvements not in skeleton

- Mini-map / unit position visualizer (canvas or SVG grid)
- Replay button for desync frames (load snapshot + re-simulate)
- Peer latency display (RTT from WebRTC `getStats()`)
- Mobile touch controls (D-pad overlay)

---

## WASM Bridge Status

**Current state: MOCK**

`rts_controller.ts` runs a pure-TypeScript 60fps frame loop (`requestAnimationFrame`).
No actual WASM calls are made. All `TODO_WASM` markers indicate where real calls go.

When `RtsSession` is exported from the WASM module (`wasm-bridge.ts`), replace the
mock sections following the existing `Platform333` pattern in `wasm-bridge.ts`.

---

## Test Harness URL Parameters

```
/rts?room=abc123&nodeId=1&validators=1,2,3,4
```

`window.__333_signaling` override is respected (inherited from `room/+page.svelte` pattern).

---

*KG: seed-post-rts-ui-skeleton-2026-04-15*
