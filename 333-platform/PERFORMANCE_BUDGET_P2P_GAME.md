# Performance Budget: Browser P2P Game Platform
## Target: 60 FPS, <100ms Input Latency, <200ms Sync Latency

**Date**: 2026-04-13 | **Context**: 333 Platform + Minecraft P2P WASM | **KG**: PERF_BUDGET_P2P_GAME

---

## Frame Time Budget (60 FPS = 16.67ms/frame)

| Component | Allocation | CPU Time | Notes |
|-----------|-----------|----------|-------|
| **WASM Compute** | 5ms | CRDT merge (1.5ms) + game logic (2.5ms) + GC pause <1ms | LWW-Map merges, block state |
| **Rendering** | 8ms | WebGL rasterize (5ms) + vertex/frag (2ms) + draw calls <50 | Three.js batch, frustum cull |
| **Network I/O** | 2ms | WS/WebRTC send (0.5ms) + recv buffer (1.5ms) | No blocking; async queue |
| **GC Pressure** | 1ms | Incremental marking | Budgeted separately; real target ~<0.5ms |
| **System Overhead** | 0.67ms | Timer, event loop, V8 internals | Tight budget; no blocking I/O |
| **Total** | **16.67ms** | **66ms @ 60fps** | Spare: ~0.3ms for jitter |

### Allocations Rationale
- **WASM 5ms**: 3 CRDT merges/frame (~1.7ms each). At 50 ops/sec + 20ms batch interval, 1 merge is typical.
- **Rendering 8ms**: WebGL state binding cheap; bottleneck is fill rate + shader complexity.
- **Network 2ms**: WebRTC DataChannel is async; main thread cost is just enqueue/dequeue.
- **GC 1ms**: Incremental GC in V8/SpiderMonkey. WASM heap segregation helps.

---

## End-to-End Latency Budgets

### Input → Render Pipeline (<100ms target)
```
Input capture (2ms) → WASM compute (5ms) → Local render (8ms) → Display (16ms) = 31ms
```
Headroom: 69ms for network propagation + peer computation. ✓ Safe margin.

### P2P Sync (<200ms target)
```
Local delta (0.5ms) → Buffer/batch (10-20ms) → WebRTC send (2ms) 
→ Peer recv (2ms) → CRDT merge (1.5ms) → Peer render (8ms) 
= 24-35ms point-to-point
```
At 6-8 peers (Minecraft chunk ownership): 35 × log₂(8) ≈ 105ms gossip propagation. ✓ Within budget.

---

## CPU Budget Breakdown: CRDT + BFT + WebRTC

### CRDT Merge (LWW-Map: O(n) per merge)
| Operation | Time | Scalability |
|-----------|------|-------------|
| Merge 50 deltas (typical batch) | 0.8ms | Linear in delta count |
| State vector compare | 0.2ms | O(peer_count), 8 peers = 1.6μs each |
| Serialization (postcard) | 0.5ms | 5KB payload → 40μs/KB |
| **Total per frame** | **1.5ms** | **50 deltas/20ms = 2500 ops/sec** |

At 60 FPS with 20ms batching: 1.2 merges/frame expected. ✓ Fits 5ms allocation.

### BFT Consensus (HotStuff: proof + QC creation)
| Operation | Time @ 8 validators | Time @ 50 validators |
|-----------|-------------------|------------------|
| QC aggregation (sig verification) | 0.4ms | 2.5ms (5 sigs → 25 sigs) |
| Vote creation (1 sig) | 0.1ms | 0.1ms |
| Block proposal hashing | 0.3ms | 0.3ms |
| **Per-block overhead** | **0.8ms** | **2.9ms** |

**Reality check**: HotStuff produces 1 block per epoch (~5-10sec on low-throughput consensus). Budget **0ms/frame** in game loop; defer consensus to background worker. Use Rayon or Web Workers if Rust WASM.

### WebRTC DataChannel (latency + throughput)
| Metric | Budget | Reality |
|--------|--------|---------|
| Send enqueue | <0.5ms | V8 interop cost; zero-copy with Uint8Array |
| Recv dequeue | <0.5ms | Async callback fired from browser event loop |
| RTT (LAN) | <10ms | Tailscale/local mesh ideal; 2-5ms typical |
| **Throughput @ 8 peers** | **280 KB/s** | Minecraft updates ~4KB/peer/20ms; 8×4=32KB batch |

**Headroom**: 280 / 32 = 8.75× capacity. ✓ Safe at 10× throughput if burst.

---

## Memory Budget (Heap + WASM + Graphics)

| Layer | Target | Typical | Notes |
|-------|--------|---------|-------|
| **WASM Linear Memory** | <50MB | 32MB (16K chunks × 2KB each) | Stack + CRDT state |
| **WASM Heap (Objects)** | <20MB | 15MB (peer state + deltas) | GC every ~500ms |
| **JavaScript Heap** | <100MB | 60MB (DOM + Three.js scene) | Browser limit: 300-500MB |
| **WebGL Textures** | <50MB | 40MB (16K chunks × 256×256 @ 4B) | VRAM on GPU |
| **Network Buffers** | <5MB | 2MB (receive window × 8 peers) | Ring buffer per channel |
| **Total** | **<225MB** | **~147MB** | Leaves 150MB safety margin |

### GC Pressure
- **WASM heap**: Generational (objects <1sec = young gen, fast collect)
- **JS heap**: Full GC ~every 1sec (interruptible; budget <50ms at 60fps)
- **Strategy**: Reuse delta buffers (object pooling), arena allocator for CRDT nodes

---

## Validation Checkpoints

### Critical Path (must measure)
1. **Frame latency**: 95th percentile <17ms (detect GC stalls)
2. **CRDT merge time**: Percentile <2ms (detect pathological deltas)
3. **Network round-trip**: P99 <100ms (detect WebRTC issues)
4. **Memory growth**: Linear drift <1MB/min (detect leak)

### Test Scenario (60fps validation)
```
8 peers, 100 blocks/sec modified, 20ms batch interval
→ 1 block delta per frame × 8 peers = 8 merges/frame
→ Expected: 8 × 1.5ms = 12ms CRDT time ✗ OVER BUDGET

Mitigation: Process merges in parallel (Rayon/Web Worker).
Or: Reduce batch merges to <4/frame via streaming.
```

---

## Summary Table: Component CPU % of 16.67ms Frame

| Component | Time (ms) | % Frame | Headroom |
|-----------|-----------|---------|----------|
| WASM Compute | 5.0 | 30% | 0.5ms |
| Rendering | 8.0 | 48% | 0.5ms |
| Network I/O | 2.0 | 12% | 0.5ms |
| GC (amortized) | 1.0 | 6% | Spiky |
| **Total** | **16.0** | **96%** | **0.67ms** |

**Bottom line**: 4% spare budget for jitter. Any 60fps miss → optimize nearest 1-2% win (e.g., reduce draw call count, CRDT delta coalescing). Measure with profiler before guessing. # KG: PERF_BUDGET_P2P_GAME
