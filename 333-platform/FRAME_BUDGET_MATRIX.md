# Frame Time Budget Matrix: Interactive Validation

**For integration phase**: Developers measure actual timings against this matrix.

## Measurement Points (instrumentation required)

```javascript
// In main game loop (wasm.rs + Svelte)

performance.mark("frame_start");

// WASM Compute
performance.mark("wasm_start");
platform.update(delta_time);  // CRDT merge, game logic
crdt_merge_time = performance.measure("wasm", "wasm_start").duration;

// Rendering  
performance.mark("render_start");
renderer.render(scene, camera);
render_time = performance.measure("render", "render_start").duration;

// Network (async, but measure pending queue depth)
pending_sends = webrtc_manager.pending_messages();
pending_size_kb = pending_sends.reduce((sum, msg) => sum + msg.len(), 0) / 1024;

performance.mark("frame_end");
frame_time = performance.measure("frame", "frame_start", "frame_end").duration;

// Log if > 16.67ms
if (frame_time > 16.67) {
  console.warn(`Frame miss: ${frame_time.toFixed(1)}ms (budget 16.67ms)`, {
    wasm: crdt_merge_time,
    render: render_time,
    pending_kb: pending_size_kb
  });
}
```

## Target Ranges vs Reality

| Metric | Budget | P50 Target | P95 Target | P99 Warn | P99.9 Critical |
|--------|--------|-----------|-----------|----------|----------------|
| **Frame Time (ms)** | 16.67 | <14 | <16 | >18 | >20 |
| **CRDT Merge (ms)** | 5.0 | <1.5 | <2.5 | >3.5 | >4.5 |
| **Render (ms)** | 8.0 | <6 | <7.5 | >8.5 | >9.5 |
| **Network Enqueue (ms)** | 2.0 | <0.5 | <1.0 | >1.5 | >2.0 |
| **WebRTC RTT (ms)** | N/A (not frame) | <5 | <8 | >15 | >30 |
| **WASM Heap (MB)** | <50 | <30 | <40 | >45 | >50 |
| **JS Heap (MB)** | <100 | <50 | <70 | >90 | >100 |

## Scenario: 8 Peers, 100 ops/sec (Minecraft load)

At 20ms batch interval with 100 modifications/sec:
- **Expected**: 100 ops/sec ÷ 50 ops/batch = 2 batches/sec = 0.033 batches/frame (1 batch per 30 frames)
- **Peak case**: 8 peers each send 1 CRDT delta in same frame → 8 merges = 12ms (OVER budget)
- **Mitigation trigger**: If P95 CRDT >2.5ms, enable parallel merge (Rayon)

## Gas Gauge: Visual Frame Budget

```
0ms                  5ms (WASM)        13ms (Render)              16.67ms
|──────────────────────┼──────────────────────────────┼───────────┼─────|
  WASM (5ms)                         Render (8ms)      Net+GC(3ms) Spare(0.67)
  ├─ CRDT:1.5ms                      ├─ State:5ms
  ├─ Logic:2.5ms                     ├─ Shaders:2ms
  └─ GC:  <1ms                       └─ Draw calls
  
HEADROOM: Red-line at 16.67ms. Green if <14ms. Yellow if 14-16.67ms. Red if >16.67ms.
```

## Auto-Scaling: CRDT Merge Strategy

If measured P95 CRDT merge > 2.5ms:

| Condition | Action |
|-----------|--------|
| Single merge > 3ms | Reduce batch size from 50→25 ops, increase frequency 20ms→10ms |
| Multiple merges (>4/frame) | Spawn Web Worker, process in parallel |
| Heap growth > 1MB/min | Enable object pooling in CRDT state |
| RTT spike (p99 > 100ms) | Check network; fallback to TURN relay |

## Integration Checklist (QA)

- [ ] Frame profiler active (DevTools Performance tab)
- [ ] CRDT merge instrumented (console.time/timeEnd)
- [ ] WebRTC RTT tracked (datagram timestamp)
- [ ] Memory monitoring (heap snapshots @ 5min)
- [ ] Load test: 8 peers, 100 ops/sec, 5min run
- [ ] Measure p50/p95/p99 for each metric above
- [ ] Document any red-lines in KG lesson

## KG Reference
**PERF_BUDGET_P2P_GAME**, **lesson-333-integration-profiling**, integration phase diagnostics

