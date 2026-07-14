# Browser-Native P2P: 5-Layer Architecture (Concise)
**KG: ARCHITECTURE_BrowserNative5Layer**

## The Stack

**Layer 5: Runtime** — Service Worker + PWA + 60fps Game Loop  
Manages lifecycle, offline resilience, background sync for pending transactions, user input dispatch.

**Layer 4: Storage** — OPFS (blocks) + IndexedDB (metadata/deltas) + Cache API (static content)  
Durable room state, 7-day rolling CRDT delta log, peer roster, pending tx queue.

**Layer 3: Compute** — Web Workers (4 max) + SharedArrayBuffer + WebGPU (future)  
Parallel validation: CRDT merge, signature batch verification, token burn. Non-blocking via task queue.

**Layer 2: Network** — WebRTC DataChannel Mesh + Signaling Server  
N:N peer graph (20 peer limit), binary wire protocol (4B header), backpressure-aware (50KB/peer buffer).

**Layer 1: Core** — WASM (Rust)  
HotStuff BFT (8–50 validators, 1500ms consensus), LWW-Map CRDT (delta-state sync), Ed25519 auth, token system (333M cap).

---

## Data Flow (Per Frame @60fps)

```
User Input → [5. Runtime] dispatch → [1. Core] delta_buffer
                                           ↓ (batched @20ms)
                        [2. Network] send to 8 peers (DataChannel)
                                           ↓
                        [2. Network] recv peer message → unserialize
                                           ↓
                        [3. Compute] Worker.merge_delta() (non-blocking)
                                           ↓
                        [1. Core] apply_delta() → state update
                                           ↓
                        [4. Storage] IndexedDB append + OPFS checkpoint @epoch
                                           ↓
                        [5. Runtime] WebGL render + Notification API
```

---

## Critical Design Decisions

1. **Consensus**: HotStuff BFT preserves safety; 8–50 validator limit avoids memory explosion.
2. **CRDT GC**: Epoch-based compaction + delta-state (Yjs model) prevents unbounded growth.
3. **Compute Offload**: Workers mandatory for >10 peers (signature verification, merge ops).
4. **Storage Strategy**: OPFS for block data (100MB/room), IndexedDB for metadata (append-only delta log).
5. **Offline Mode**: Optimistic updates + IndexedDB queue; reconcile via CRDT merge on reconnect.
6. **Latency Budget**: p99 <200ms end-to-end (20ms batching + 50ms WebRTC + 130ms BFT).

---

## Implementation Status (2026-04-13)

| Layer | Status | Gap |
|-------|--------|-----|
| 1. Core (WASM) | ✅ Complete | Unit tests only; E2E sync untested |
| 2. Network (WebRTC) | ⚠️ 60% | Backpressure + view sync TBD |
| 3. Compute (Workers) | ⚠️ Design | Stub implementations |
| 4. Storage (OPFS/IDB) | ⚠️ Design | Snapshot serialization TBD |
| 5. Runtime (PWA) | ⚠️ Tauri wrapper | Game loop + offline sync TBD |

**Next Phase**: INT_MemFix → INT_CrdtSync → INT_ConsensusNet → E2E validation.

---

*This architecture maximizes HTML5 APIs (WebRTC, OPFS, IndexedDB, Service Workers, Web Workers) for true P2P without servers.*
