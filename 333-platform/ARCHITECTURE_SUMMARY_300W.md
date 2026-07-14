# Browser-Native 5-Layer P2P Architecture (Concise)
**KG: ARCHITECTURE_BrowserNative5Layer | 333 Platform**

## Five-Layer Stack

**Layer 1: Core (WASM)** — HotStuff BFT (8–50 validators, 1500ms), CRDT delta-state sync, Ed25519 auth, token system (333M cap).

**Layer 2: Network (WebRTC)** — DataChannel mesh (20 peer limit), binary protocol (4B header), signaling server (broker only).

**Layer 3: Compute (Web Workers)** — 4 workers: CRDT merge, signature batch verify, token burn. SharedArrayBuffer for zero-copy snapshots.

**Layer 4: Storage (OPFS + IndexedDB)** — OPFS (blocks, 100MB/room), IndexedDB (metadata, 7-day delta log), Cache API (offline static).

**Layer 5: Runtime (Service Worker + PWA)** — 60fps game loop, offline resilience, background sync (DataChannel online / IndexedDB offline).

## Data Flow

User input → Runtime → Core delta_buffer (batched 20ms) → Network send (5 peers) → Peer recv → Worker validate (non-blocking) → Core apply → Storage commit → Render.

**Latency Budget**: p99 <200ms (20ms batch + 50ms WebRTC + 130ms consensus).  
**Memory**: <600KB @ 50 peers (Worker offload mandatory for >10 peers).

## Key Decisions

- BFT: 8–50 validator ceiling prevents explosion; HotStuff ensures safety
- CRDT GC: Epoch-based compaction (Yjs model) prevents unbounded growth
- Worker offload: Merge_delta + batch_verify must run off-main-thread (>1ms)
- Storage async: OPFS writes never block main thread; Service Worker commits
- Offline: IndexedDB queue + optimistic updates; CRDT merge reconciles on reconnect

## Status (2026-04-13)

Core ✅ Complete. Layers 2-5: ⚠️ Design ready → INT phases (INT_MemFix → INT_CrdtSync → INT_ConsensusNet → INT_E2E).

**See**: BROWSER_NATIVE_5LAYER_ARCHITECTURE.md (full), ARCHITECTURE_INTEGRATION_MAP.md (data flows)
