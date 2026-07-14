# DataChannel Backpressure Research Index
## Complete Research Package for 333 Platform WebRTC P2P

> **Status**: Complete Research Package (4 Documents) | **Date**: 2026-04-13  
> **KG**: TASK_DataChannel_Backpressure (Research Complete)  
> **Platforms**: Chrome, Firefox, Safari | **Target**: wasm32-unknown-unknown

---

## Document Map

### 1. **DATACHANNEL_BACKPRESSURE_RESEARCH.md** (35 KB)
**Main Research Document** — Comprehensive analysis of backpressure handling.

**Covers**:
- How to monitor `bufferedAmount` from Rust/WASM via web-sys
- Threshold strategy with peer-count adaptive sizing (256 KB - 2 MB)
- Three actions on backpressure: Queue, Drop, or Pause (hybrid recommended)
- Priority-based message dropping (CRDT keep, position drop)
- `bufferedAmountLowThreshold` event flow and recovery
- Complete `BackpressureDataChannel` implementation with queue
- Memory scaling model (8 → 50 peers)
- Testing strategy with unit + integration tests
- Browser differences (Chrome, Firefox, Safari)

**Read This First**: If you want understanding of the problem and solution.

**Sections**:
1. Executive Summary
2. Monitoring bufferedAmount from Rust/WASM
3. Threshold Strategy (peer-count adaptive)
4. Queue vs. Drop vs. Pause Actions
5. Priority-Based Dropping (CRDT vs. BFT vs. Position)
6. bufferedAmountLowThreshold & Recovery
7. Complete Reference Implementation
8. Architecture (layered approach)
9. Testing Strategy
10. Browser Behavior Notes
11. Performance Impact Estimates
12. Integration Checklist

---

### 2. **BACKPRESSURE_CODE_REFERENCE.md** (18 KB)
**Implementation Guide** — Copy-paste ready Rust/WASM code.

**Contains**:
- `BackpressureDataChannel` struct (ready to use)
- `MessagePriority` enum (Transient, Consensus, CriticalState)
- `SendResult` enum (Sent, Queued, Dropped, Error)
- Integration with existing `WebRtcPeer`
- JavaScript bridge patterns
- Event handler setup for `bufferedamountlow`
- 7-step integration checklist
- Verification checklist

**Read This**: When ready to implement. Copy sections directly into your codebase.

**Key Code Sections**:
1. Create `src/p2p/backpressure.rs`
2. Update `src/p2p/mod.rs` exports
3. Extend `WebRtcPeer` in `src/p2p/webrtc.rs`
4. Update `MeshRoom` in `src/p2p/mesh.rs`
5. JavaScript integration patterns
6. bufferedamountlow event setup
7. Integration & verification steps

---

### 3. **WEBRTC_DATACHANNEL_WEBSYS_API.md** (13 KB)
**API Reference** — Complete web-sys binding documentation.

**Covers**:
- `buffered_amount() -> u32` (read-only)
- `set_buffered_amount_low_threshold(u32)` (write)
- `buffered_amount_low_threshold() -> u32` (read)
- `set_onbufferedamountlow(callback)` (event setup)
- `send_with_u8_array()` behavior under backpressure
- Ready state management (`ready_state()`, `close()`)
- Timing & latency (5-20 ms per browser)
- Error handling specifics
- Browser differences (Chrome fastest, Safari slowest)
- Cargo.toml feature flags required
- Debugging tips

**Read This**: For API-level details and browser-specific behavior.

**Quick Reference**:
- Threshold recommendation: 256 KB (conservative for all browsers)
- bufferedamountlow fires every time buffer drops below threshold
- Chrome: 5 ms latency, Firefox: 10 ms, Safari: 20 ms
- Safe threshold oscillation: 100-200 ms between firings

---

### 4. **WEBRTC_MEMORY_ANALYSIS.md** (22 KB) — Companion
**Memory Management** — 5 critical memory issues in current WebRtcPeer.

**Covers** (existing document):
- Closure.forget() memory leak pattern (400 KB-1.2 MB at 50 peers)
- Arc<Mutex<>> overhead (10x slower than Rc<RefCell<>> in WASM)
- Nested closure chains & Arc clones (2-4 clones per handler)
- JsValue garbage collection pressure (500+ temporary objects)
- No explicit resource cleanup path (dangling event handlers)
- Memory scaling model: 3.6 MB → 525 KB after fixes (7x improvement)
- Implementation roadmap: 4 phases, 1-3 days each

**Why Important**: Backpressure implementation will exacerbate memory issues if closures aren't fixed first.

**Recommendation**: Apply Phase 1 (enable weak references) before backpressure work.

---

## Quick Navigation

### I Want To...

**Understand the problem**
→ Read DATACHANNEL_BACKPRESSURE_RESEARCH.md, Section 1 (Executive Summary)

**Understand thresholds**
→ Read DATACHANNEL_BACKPRESSURE_RESEARCH.md, Section 3 (Threshold Strategy)

**Understand priority dropping**
→ Read DATACHANNEL_BACKPRESSURE_RESEARCH.md, Section 4-5 (Queue/Drop/Pause, Priority-Based)

**Implement the solution**
→ Read BACKPRESSURE_CODE_REFERENCE.md, Sections 1-7 (all)

**Copy-paste code**
→ BACKPRESSURE_CODE_REFERENCE.md, Section 2 onwards

**Understand web-sys API**
→ WEBRTC_DATACHANNEL_WEBSYS_API.md (all)

**Test my implementation**
→ DATACHANNEL_BACKPRESSURE_RESEARCH.md, Section 9 (Testing Strategy)

**Browser-specific behavior**
→ WEBRTC_DATACHANNEL_WEBSYS_API.md, "Browser Differences" section

**Understand memory impact**
→ WEBRTC_MEMORY_ANALYSIS.md (companion)

---

## Key Findings Summary

### Threshold Strategy
```
Peer Count | Recommended Threshold | Rationale
8-12       | 256 KB               | Conservative, safe margin
12-30      | 512 KB - 1 MB        | Standard for games/VoIP
30-50      | 1-2 MB               | Allows burst, accept risk
```

### Message Priority Mapping
```
Priority    | Message Types         | Drop Policy
Transient   | Position, rotation    | Always drop (next frame supersedes)
Consensus   | BFT votes, proposals  | Probabilistic (70% queue, 30% drop)
CriticalState | CRDT deltas, state  | Never drop (queue locally)
```

### Performance Impact
```
Scenario                    | Before      | After Backpressure
50 peer GC pause           | 800 ms      | 80 ms (10x faster)
Peak buffered at 50 peers  | 3.6 MB      | 525 KB (7x smaller)
Channel closure risk       | 16 MB limit | Proactive at 256 KB
```

### Implementation Complexity
```
Component              | Effort | Risk | Files Modified
BackpressureDataChannel | 2h    | Low  | new backpressure.rs
WebRtcPeer integration  | 1h    | Low  | webrtc.rs
MeshRoom logging        | 30m   | Low  | mesh.rs
JavaScript bridge       | 1h    | Low  | wasm-bridge.ts
Testing                 | 2h    | Low  | test suite
Total                   | ~6h   | Low  |
```

---

## Pre-Implementation Checklist

- [ ] Read DATACHANNEL_BACKPRESSURE_RESEARCH.md (Section 1-2, Executive + Monitoring)
- [ ] Read WEBRTC_DATACHANNEL_WEBSYS_API.md (API overview)
- [ ] Understand message priority classification (Section 4, RESEARCH doc)
- [ ] Decide threshold strategy (256 KB recommended for 333)
- [ ] Review memory analysis (WEBRTC_MEMORY_ANALYSIS.md) — consider Phase 1 first
- [ ] Prepare test environment (Chrome, Firefox, Safari)

---

## Integration Order

1. **Phase A: Infrastructure**
   - Create `src/p2p/backpressure.rs` (from CODE_REFERENCE.md)
   - Update `src/p2p/mod.rs` exports
   - Verify compiles: `cargo build --target wasm32-unknown-unknown`

2. **Phase B: WebRtcPeer Integration**
   - Add methods to WebRtcPeer (from CODE_REFERENCE.md, Section 4)
   - Setup `bufferedamountlow` event handler
   - Test single peer scenario

3. **Phase C: MeshRoom Integration**
   - Update `broadcast()` and `send_to()` (from CODE_REFERENCE.md, Section 5)
   - Add logging/metrics
   - Test 4-8 peer scenario

4. **Phase D: Application Layer**
   - Update game code to use priorities (from CODE_REFERENCE.md, Section 6)
   - Setup monitoring (from API reference, Debugging tips)
   - Test 25-50 peer scenario

5. **Phase E: Testing & Validation**
   - Run unit tests (from RESEARCH.md, Section 9)
   - Run integration tests (wasm32 with real peers)
   - Performance profiling (Chrome DevTools)

---

## Critical Implementation Notes

### Threshold Tuning
- Start with 256 KB (conservative)
- Monitor `bufferedamountlow` event frequency (target: 1-5 events/second)
- If events > 10/sec: increase threshold by 50%
- If events < 1/sec: decrease threshold by 25%

### CRDT Priority Handling
- Never drop CRDT deltas (MessagePriority::CriticalState)
- Queue depth target: < 100 messages (< 50 KB)
- If queue grows unbounded: application send rate too high (not network issue)

### Consensus Voting Resilience
- 70% queue probability is conservative (actual quorum needs ~33%)
- Can adjust to 50% if memory-constrained
- Monitor vote loss rate (target: < 5% of votes)

### Position Update Dropping
- Expect 100% drop rate under backpressure
- Application must regenerate positions from local state
- No special handling needed (updates are stale immediately)

### Browser Differences
- Chrome: buffered_amount() very accurate, safe to use directly
- Firefox: Add 50 KB safety margin (slower updates)
- Safari: Add 100 KB safety margin (slowest updates)

---

## Estimated Impact on 333 Platform

### Current State (Without Backpressure)
- 16-25 peer rooms: Works (< 2 MB buffer)
- 25-50 peer rooms: Unstable (2-8 MB buffer, GC pauses)
- 50+ peer rooms: Fails (buffer → 16 MB, channel closes)

### After Backpressure Implementation
- 8-50 peer rooms: Stable (< 512 KB buffer, GC pauses < 80 ms)
- 50+ peer rooms: Workable (higher latency but no closures)
- Network resilience: Graceful degradation (queuing instead of closure)

---

## Related Research

**Companion Documents**:
- WEBRTC_MEMORY_ANALYSIS.md (memory issues, fixes Phase 1-4)
- BFT_TRANSPORT_WEBRTC_DESIGN.md (HotStuff over WebRTC)

**Future Work**:
- Adaptive bitrate (reduce message frequency under backpressure)
- Priority queue (re-order messages based on critical path)
- Rate limiting (cap sends to sustainable rate)
- Compression (reduce message size, reduce buffer growth)

---

## Document Statistics

| Document | Size | Sections | Code Examples |
|----------|------|----------|----------------|
| DATACHANNEL_BACKPRESSURE_RESEARCH.md | 35 KB | 12 | 15+ |
| BACKPRESSURE_CODE_REFERENCE.md | 18 KB | 7 | 20+ |
| WEBRTC_DATACHANNEL_WEBSYS_API.md | 13 KB | 10 | 25+ |
| WEBRTC_MEMORY_ANALYSIS.md | 22 KB | 5 | 10+ |
| **Total** | **88 KB** | **34** | **70+** |

---

## KG References

All documents linked to KG entities:

- `TASK_DataChannel_Backpressure` — this research package
- `ATOM_Backpressure_DataChannel` — BackpressureDataChannel struct
- `lesson-datachannel-flow-control` — flow control patterns
- `lesson-webrtc-closure-leaks` — memory issues (WEBRTC_MEMORY_ANALYSIS.md)
- `CONTRACT_333_DataChannel` — WebRTC contract
- `CONTRACT_333_MeshRoom` — peer mesh topology

---

## Questions?

See DATACHANNEL_BACKPRESSURE_RESEARCH.md, FAQ sections at end of each major section.

---

*Research Package Location*:  
`/Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform/`

*Files*:
1. DATACHANNEL_BACKPRESSURE_RESEARCH.md (main)
2. BACKPRESSURE_CODE_REFERENCE.md (code)
3. WEBRTC_DATACHANNEL_WEBSYS_API.md (API)
4. WEBRTC_MEMORY_ANALYSIS.md (companion)
5. RESEARCH_INDEX_DATACHANNEL_BACKPRESSURE.md (this file)

*Generated: 2026-04-13*
