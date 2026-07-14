# Performance Budget Research Index
## 333 Platform + Minecraft P2P Browser Game

**Research Date**: 2026-04-13  
**KG Binding**: PERF_BUDGET_P2P_GAME, lesson-333-integration-profiling  
**Context**: Integration phase — wiring CRDT↔WebRTC↔BFT for real P2P game

---

## Quick Start (Choose Your Depth)

### 1-Minute Summary
**File**: PERF_BUDGET_SUMMARY.txt  
**For**: Developers starting integration  
**Contains**: Frame budget table, component CPU, memory limits, quick rules

### 5-Minute Deep Dive
**File**: FRAME_BUDGET_MATRIX.md  
**For**: QA, performance testing, load validation  
**Contains**: Instrumentation code, target ranges (P50/P95/P99), auto-scaling rules

### Full Specification (30 min read)
**File**: PERFORMANCE_BUDGET_P2P_GAME.md  
**For**: Architecture decisions, bottleneck analysis, design constraints  
**Contains**: 4 detailed tables, rationale, scenario analysis, validation checkpoints

---

## Document Structure Map

```
PERF_BUDGET_P2P_GAME.md (Full Analysis)
├── Frame Time Budget (16.67ms allocation)
│   ├── WASM Compute (5ms: CRDT 1.5ms + logic 2.5ms + GC <1ms)
│   ├── Rendering (8ms: WebGL state 5ms + shaders 2ms)
│   ├── Network I/O (2ms: async WebRTC, non-blocking)
│   └── Allocations Rationale
│
├── End-to-End Latency Budgets
│   ├── Input→Render (31ms, 69ms headroom)
│   └── P2P Sync (24-35ms point-to-point, 105ms gossip)
│
├── CPU Budget Breakdown
│   ├── CRDT Merge (LWW-Map O(n), 1.5ms/50-delta batch)
│   ├── BFT Consensus (0.8-2.9ms per-block, off-path)
│   └── WebRTC (enqueue/dequeue <1ms each)
│
├── Memory Budget (<225MB total)
│   └── WASM 50MB + JS 100MB + WebGL 50MB + buffers 5MB
│
└── Validation Checkpoints
    └── 95th frame, P99 RTT, memory drift <1MB/min

PERF_BUDGET_SUMMARY.txt (Quick Reference)
├── Frame Time Budget Table
├── Component Details (CRDT, BFT, WebRTC)
├── Memory Budget (WASM/JS/WebGL breakdown)
└── Quick Rules & Thresholds

FRAME_BUDGET_MATRIX.md (Measurement & QA)
├── JavaScript Instrumentation Code
├── Target Ranges (P50/P95/P99/P99.9)
├── Peak Load Scenario Analysis (8 peers, 100 ops/sec)
├── Auto-Scaling Triggers
└── Integration QA Checklist
```

---

## Key Metrics At A Glance

| Metric | Allocation | Typical | Peak | Status |
|--------|-----------|---------|------|--------|
| **Frame Time** | 16.67ms | 12-14ms | 16ms | ✓ Safe |
| **CRDT Merge** | 1.5ms | 1.5ms | 12ms (8 merges) | ⚠️ Peak over |
| **BFT Overhead** | 0ms (async) | 0ms | 2.9ms (deferred) | ✓ Off-path |
| **WebRTC RTT** | N/A | 2-5ms | 10ms (warn) | ✓ Safe LAN |
| **Memory Used** | <225MB | 147MB | 180MB | ✓ Safe |
| **Input Latency** | <100ms | 31ms | 100ms | ✓ Safe |
| **P2P Sync** | <200ms | 35ms | 105ms gossip | ✓ Safe |

**Color Legend**: ✓ Safe | ⚠️ Caution | ✗ Over

---

## Integration Phase Checklist

Use FRAME_BUDGET_MATRIX.md for:
- [ ] Frame profiler instrumentation (DevTools)
- [ ] CRDT merge time measurement (console.time)
- [ ] WebRTC RTT tracking (datagram timestamp)
- [ ] Memory growth monitoring (heap snapshots)
- [ ] Load test (8 peers, 100 ops/sec, 5min)
- [ ] Measure p50/p95/p99 percentiles
- [ ] Document any red-lines to KG lesson

---

## Critical Findings (Must Know)

### 1. CRDT Peak Load Risk
**Problem**: 8 peers simultaneously sending deltas = 8 merges/frame = 12ms (5ms over budget)  
**Mitigation**: Implement parallel merge (Rayon/Web Worker) OR reduce batch size  
**Trigger**: If measured P95 CRDT >2.5ms, enable parallel

### 2. BFT Must Be Off-Path
**Mistake**: Running HotStuff consensus in game frame loop  
**Reality**: Consensus produces ~1 block per 5-10 seconds (not per frame)  
**Rule**: Defer to background Web Worker. Frame budget: 0ms.

### 3. Frame Budget is 96% Utilization
**Problem**: Only 0.67ms spare for jitter  
**Reality**: Any miss is real; no hidden 10% buffer  
**Rule**: Optimize nearest 1-2% win (reduce draw calls, delta coalesce) before guessing

### 4. Memory Pressure from Delta Accumulation
**Watch**: WASM heap growth. Object pooling + arena allocator needed.  
**Target**: <1MB/min drift  
**Trigger**: If drift >1MB/min, enable heap pooling

### 5. WebRTC Latency Tolerance
**Good news**: 2-5ms LAN RTT leaves 195ms headroom in 200ms budget  
**Fallback**: TURN relay if RTT spikes (WebRTC auto-detects, no code needed)

---

## File Purposes & When to Use

| Document | Use When | Audience | Length |
|----------|----------|----------|--------|
| PERF_BUDGET_SUMMARY.txt | Quick ref, on-desk print | Devs, QA | 1 page |
| FRAME_BUDGET_MATRIX.md | Measuring, profiling, load test | QA, integration engineer | 3 pages |
| PERFORMANCE_BUDGET_P2P_GAME.md | Architecture decision, bottleneck analysis | Architects, tech lead | 5 pages |
| This index (PERF_RESEARCH_INDEX.md) | Navigating the research | Everyone | 2 pages |

---

## Context from Project History

### Related Documents
- **CRDT_SYNC_ARCHITECTURE.md** — Delta batching @ 20ms interval (rationale for 1.5ms merge budget)
- **BFT_TRANSPORT_WEBRTC_DESIGN.md** — HotStuff over WebRTC, view synchronization, timeouts
- **apt-progress.md** — Phase 8 (Integration) current status; performance budget added to checklist

### KG Lessons Involved
- **lesson-333-modules-not-integrated** (CRITICAL) — Modules exist, wiring incomplete
- **lesson-333-integration-profiling** (NEW) — Measurement instrumentation needed
- **lesson-333-hotstuff-p2p-network** — BFT consensus latency analysis

---

## Formulas & Calculations

### Frame Utilization
```
Frame budget = 16.67ms (60 FPS)
Used = 5 + 8 + 2 + 1 = 16ms
Spare = 0.67ms (4% of 16.67ms)
Utilization = 16/16.67 = 96%
```

### CRDT Merge Scaling
```
ops_per_sec = 100
ops_per_batch = 50
batch_interval_ms = 20
batches_per_sec = 100 / 50 = 2
batches_per_frame = 2 / 60 = 0.033 (1 batch per 30 frames, typical)
merge_time_per_batch = 1.5ms
typical_frame_crdt_time = 0.033 × 1.5ms ≈ 0.05ms ✓ Safe

peak_case = 8 peers × 1 delta/frame = 8 merges = 8 × 1.5ms = 12ms ✗ Over
```

### P2P Latency Path
```
point_to_point = local_delta(0.5ms) + batch_window(20ms) + send(2ms)
               + recv(2ms) + merge(1.5ms) + render(8ms)
               = 34.5ms (typical)

gossip_propagation @ 8_peers = 34.5 × log₂(8) = 34.5 × 3 = 103.5ms
budget = 200ms
headroom = 200 - 103.5 = 96.5ms ✓ Safe
```

### Memory Headroom
```
browser_limit = 300-500MB (default)
total_allocated = 225MB
safety_margin = (400 - 225) / 400 = 43.75% ✓ Safe
```

---

## Next Steps (Integration Phase)

### For Code Implementation
1. **Instrumentation** (FRAME_BUDGET_MATRIX.md has code)
   - Add performance.mark/measure in main loop
   - Log frame drops with breakdown

2. **Profiling** (DevTools Performance tab)
   - Baseline 8-peer load test
   - Capture CPU flame graph for CRDT

3. **Scaling** (Auto-scale triggers)
   - Watch P95 CRDT; activate Web Worker if >2.5ms
   - Monitor heap; enable object pool if drift >1MB/min

4. **Validation** (QA checklist)
   - Measure p50/p95/p99 for all 7 metrics
   - Document any red-lines to KG lesson

### For Architecture
- BFT consensus MUST be in Web Worker (not frame loop)
- CRDT merge may need parallelization (Rayon) for 8+ peers
- Memory pooling strategy needed before scale test

---

## References & Links

| Reference | In Document | Purpose |
|-----------|-------------|---------|
| Frame budget table | PERF_BUDGET_SUMMARY.txt | Component allocation |
| CRDT analysis | PERFORMANCE_BUDGET_P2P_GAME.md § CPU Budget | Merge breakdown |
| BFT overhead | PERFORMANCE_BUDGET_P2P_GAME.md § CPU Budget | Consensus cost |
| WebRTC latency | PERFORMANCE_BUDGET_P2P_GAME.md § CPU Budget | Network I/O |
| Measurement code | FRAME_BUDGET_MATRIX.md | Instrumentation |
| Auto-scale rules | FRAME_BUDGET_MATRIX.md § Auto-Scaling | Thresholds |
| QA checklist | FRAME_BUDGET_MATRIX.md § Checklist | Validation steps |

---

## Status

**Research**: ✓ Complete (2026-04-13)  
**Documents**: ✓ 3 files created + index  
**Integration**: ⏳ Ready for implementation (Phase 8)  
**Validation**: ⏳ Awaiting load test (QA Phase)

**Next**: Run 8-peer load test with instrumentation → compare to targets → optimize

---

**Created by**: Claude Research Agent  
**KG Binding**: PERF_BUDGET_P2P_GAME  
**Project**: 333 Platform (MetaHumotonic)
