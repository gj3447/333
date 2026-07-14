# 333 Platform — Observability

<!-- # KG: sprint6C-observability-2026-04-15 -->

## Overview

The 333 Platform exposes metrics from three subsystems:

1. **Rust core** (`src/observability.rs`) — RTS, BFT, GGRS metrics as process-global atomics.
2. **WASM export** (`get_metrics_json()`) — JS-callable snapshot, polled every 5 s from the 333-app overlay.
3. **Signaling server** (`signaling/server.mjs`) — WebSocket + TURN metrics, GET `/metrics` in Prometheus text format.

---

## Metric Catalogue

### RTS (Real-Time Strategy lockstep layer)

| Metric | Type | Instrumentation point |
|---|---|---|
| `rts_frame_advance_total` | counter | `RtsSession::advance_frame` — every frame regardless of game state |
| `rts_frame_advance_latency_ms` | histogram | `RtsSession::advance_frame` wall-clock (native only; WASM skips) |
| `rts_desync_events_total` | counter | `FrameDivergenceDetector::observe_peer` — on hash mismatch |
| `rts_state_hash_mismatches_total` | counter | `RtsSession::observe_peer_digest` — mirror of desync events from session layer |
| `rts_peer_ejected_total` | counter | `PeerEjectController::apply_vote_result` — BFT-approved ejections only |

**Alert thresholds (suggested):**
- `rts_desync_events_total` > 3 in 1 min → WARN (potential determinism bug)
- `rts_peer_ejected_total` rate > 0.1/min → INFO (AFK peers or network issues)
- `rts_frame_advance_latency_ms{p99}` > 16 ms → WARN (target: < 1/60 s = 16.7 ms)

### BFT (HotStuff consensus checkpoint layer)

| Metric | Type | Instrumentation point |
|---|---|---|
| `bft_checkpoint_proposed_total` | counter | `HotStuffCheckpointProvider::propose_checkpoint` + `MultiNodeBftHarness::drive_checkpoint` |
| `bft_checkpoint_committed_total` | counter | On `Ok(qc)` return from propose/drive |
| `bft_checkpoint_qc_latency_ms` | histogram | propose → commit wall-clock (native only) |
| `bft_vote_rejected_total` | counter | Reserved for future view-change / equivocation detection |

**Alert thresholds (suggested):**
- `bft_checkpoint_committed_total` / `bft_checkpoint_proposed_total` < 0.9 → WARN (>10% checkpoint failure)
- `bft_checkpoint_qc_latency_ms{p99}` > 500 ms → WARN (slow consensus)

### GGRS adapter (rollback netcode)

| Metric | Type | Instrumentation point |
|---|---|---|
| `ggrs_rollback_events_total` | counter | `rts_ggrs_rollback_stub` WASM call (stub; replace with real adapter in Phase 4) |
| `ggrs_save_slot_evicted_total` | counter | `GgrsStub::save_state` — ring-buffer eviction (MAX_SAVE_SLOTS = 8) |

**Alert thresholds (suggested):**
- `ggrs_rollback_events_total` rate > 5/min → WARN (excessive rollbacks)
- `ggrs_save_slot_evicted_total` / `rts_frame_advance_total` > 0.125 → INFO (save window saturation)

### Signaling server (Node.js WebSocket relay)

| Metric | Type | Instrumentation point |
|---|---|---|
| `signaling_ws_connections` | gauge | `wss.on('connection')` / `ws.on('close')` |
| `signaling_rooms_active` | gauge | `rooms.size` after join/leave |
| `signaling_turn_credentials_issued_total` | counter | Successful `GET /turn-credentials` response |
| `signaling_rate_limited_total` | counter | `GET /turn-credentials` rejected by rate limiter |

**Alert thresholds (suggested):**
- `signaling_ws_connections` > 1000 → WARN
- `signaling_rate_limited_total` rate > 20/min → WARN (possible DDoS)

---

## Scrape Configuration

### Rust core (signaling-333 sidecar or embedded)

The Rust `prometheus_snapshot()` function is not yet exposed as an HTTP endpoint in the binary; the signaling-333 process is the natural host. Wire it by adding a `/metrics` route to the future native server, or use the WASM `get_metrics_json()` path for browser-side collection.

**Recommended scrape interval:** 15 s (metrics are low-cardinality counters; no need for sub-second polling).

### Signaling server

```yaml
# prometheus/scrape_configs addition:
- job_name: '333-signaling'
  scrape_interval: 15s
  static_configs:
    - targets: ['signaling-333.apps.svc.cluster.local:8333']
  metrics_path: '/metrics'
  # Note: /metrics is exposed only on the internal ClusterIP, not via Traefik.
  # Prometheus scrapes directly via ClusterIP; no BasicAuth needed at this path.
```

---

## WASM metrics overlay

```js
// Poll every 5 seconds from 333-app:
import init, { get_metrics_json } from './pkg/triple_three.js';
await init();

setInterval(() => {
  const m = JSON.parse(get_metrics_json());
  document.getElementById('metric-frames').textContent =
    `Frames: ${m.rts_frame_advance_total}`;
  document.getElementById('metric-desyncs').textContent =
    `Desyncs: ${m.rts_desync_events_total}`;
}, 5000);
```

---

## /metrics endpoint example output

```
# HELP rts_frame_advance_total Total frames advanced by RtsSession::advance_frame
# TYPE rts_frame_advance_total counter
rts_frame_advance_total 1200

# HELP rts_desync_events_total Total desync events (hash mismatch between local and peer state)
# TYPE rts_desync_events_total counter
rts_desync_events_total 0

# HELP rts_state_hash_mismatches_total Total state hash mismatches detected by DesyncDetector::observe_peer
# TYPE rts_state_hash_mismatches_total counter
rts_state_hash_mismatches_total 0

# HELP rts_peer_ejected_total Total peers ejected via BFT-backed AFK vote
# TYPE rts_peer_ejected_total counter
rts_peer_ejected_total 1

# HELP rts_frame_advance_latency_ms Latency of RtsSession::advance_frame in milliseconds
# TYPE rts_frame_advance_latency_ms histogram
rts_frame_advance_latency_ms_bucket{le="1.0"} 1195
rts_frame_advance_latency_ms_bucket{le="2.0"} 1199
rts_frame_advance_latency_ms_bucket{le="5.0"} 1200
rts_frame_advance_latency_ms_bucket{le="10.0"} 1200
rts_frame_advance_latency_ms_bucket{le="25.0"} 1200
rts_frame_advance_latency_ms_bucket{le="50.0"} 1200
rts_frame_advance_latency_ms_bucket{le="100.0"} 1200
rts_frame_advance_latency_ms_bucket{le="250.0"} 1200
rts_frame_advance_latency_ms_bucket{le="+Inf"} 1200
rts_frame_advance_latency_ms_sum 743.2
rts_frame_advance_latency_ms_count 1200

# HELP bft_checkpoint_proposed_total Total BFT checkpoints proposed via HotStuffCheckpointProvider
# TYPE bft_checkpoint_proposed_total counter
bft_checkpoint_proposed_total 40

# HELP bft_checkpoint_committed_total Total BFT checkpoints committed with valid QuorumCert
# TYPE bft_checkpoint_committed_total counter
bft_checkpoint_committed_total 40

# HELP bft_vote_rejected_total Total BFT votes rejected (stale view, equivocation, or bad signature)
# TYPE bft_vote_rejected_total counter
bft_vote_rejected_total 0

# HELP bft_checkpoint_qc_latency_ms Latency from BFT checkpoint proposal to QuorumCert commit in milliseconds
# TYPE bft_checkpoint_qc_latency_ms histogram
bft_checkpoint_qc_latency_ms_bucket{le="1.0"} 38
bft_checkpoint_qc_latency_ms_bucket{le="2.0"} 40
bft_checkpoint_qc_latency_ms_bucket{le="+Inf"} 40
bft_checkpoint_qc_latency_ms_sum 58.1
bft_checkpoint_qc_latency_ms_count 40

# HELP ggrs_rollback_events_total Total GGRS rollback events (save state slot loaded for replay)
# TYPE ggrs_rollback_events_total counter
ggrs_rollback_events_total 3

# HELP ggrs_save_slot_evicted_total Total GgrsStub save slots evicted from the rolling window
# TYPE ggrs_save_slot_evicted_total counter
ggrs_save_slot_evicted_total 1192
```

*(Signaling metrics are served separately from `GET /metrics` on the Node.js process port.)*

---

## WASM size budget

Target: WASM binary size increase < 30 KB from baseline 323 KB.

The `observability` module adds only:
- 11 static `AtomicU64` / `AtomicI64` values (no heap allocation)
- String formatting in `json_snapshot()` / `render_prometheus()` (stack-only)
- No new dependencies

Estimated WASM overhead: **< 2 KB** (string format functions may add 4–8 KB before `wasm-opt -Oz`; post-optimization expected < 2 KB net).

---

*2026-04-15 · sprint6C · # KG: sprint6C-observability-2026-04-15*
