# Browser-Native 5-Layer P2P Architecture: 333 Platform
## HTML5 APIs Maximization Model
**KG: ARCHITECTURE_BrowserNative5Layer, SPAN_333_Architecture**
**Date**: 2026-04-13 | **Status**: Research → Design Ready

---

## Executive Summary

The 333 Platform maximizes HTML5 APIs for a fully decentralized browser-based peer-to-peer platform without central servers. Five layers form a complete stack: **Core consensus logic (WASM) → P2P networking (WebRTC) → Parallel compute (Web Workers) → Persistent storage (OPFS/IndexedDB) → Runtime lifecycle (Service Worker/PWA)**. Data flows bidirectionally between layers via typed message queues and event channels.

---

## 5-Layer Architecture

### **Layer 1: Core (WASM) — Consensus + Cryptography + Token**

**Components**:
- **HotStuff BFT**: `state.rs`, `executor.rs` — 8–50 validators, 1500ms consensus latency
- **LWW-Map CRDT**: Delta-state sync, epoch-based compaction, causal ordering
- **Ed25519 Signatures**: Authentication, 32B pubkey identity
- **Token System**: Filecoin-style baseline burn, 333M cap, 15tok/epoch

**Data In**: Local user actions (block placement, transaction), peer consensus messages  
**Data Out**: Local state deltas, consensus votes, token transfers  
**API**: `wasm::Platform333` (Rust) exposed via `wasmModule.init_room()`, `on_frame()`, `consensus_round()`

---

### **Layer 2: Network (WebRTC) — P2P Mesh + Message Routing**

**Components**:
- **DataChannel Mesh**: N:N peer graph, 20 peer limit per browser (memory constraint)
- **Signaling Server** (`ws333`): Lightweight SDP/ICE broker, not data path
- **Wire Protocol**: 4-byte binary header + postcard serialization (140B/msg avg)
- **Transport Trait**: `send()`, `broadcast()`, `recv()`, `current_view()`, `validate_msg()`

**Data In**: WASM consensus/CRDT deltas, local user updates  
**Data Out**: Serialized messages to peer DataChannels  
**Backpressure**: VecDeque with 50KB buffer per peer; force flush on overflow

---

### **Layer 3: Compute (Web Workers) — Parallel Verification + GPU Acceleration**

**Components**:
- **Dedicated Workers** (max 4): CRDT merge validation, signature batch verification, token burn calculation
- **SharedArrayBuffer**: Zero-copy CRDT state snapshot for worker read-only access
- **WebGPU** (future): GPU-accelerated merkle tree hashing, batch crypto ops
- **Worker Pool**: Task queue with priority (consensus > CRDT > telemetry)

**Data In**: CRDT deltas from peers, transaction signature bundles  
**Data Out**: Validation verdicts (bool), merged state snapshots, verified checksums  
**Scheduling**: Non-blocking; long ops delegated immediately to prevent main-thread lag

---

### **Layer 4: Storage (OPFS + IndexedDB + Cache API) — Durable State + P2P DHT Index**

**Components**:
- **OPFS** (Origin Private File System): Block data files, snapshot checkpoints (100MB limit per room)
- **IndexedDB**: Room metadata, peer roster, CRDT delta log (append-only), local tx pending queue
- **Cache API**: HTTP GET responder for static resources (avatar, config) — enables offline work
- **Sync Protocol**: On-launch load full snapshot; periodic incremental checkpoints (every 100 epochs)

**Data In**: Consensus-finalized blocks, CRDT snapshots, peer join events  
**Data Out**: Loaded state on room entry, DHT advertisement of local content (via Headscale tunnel)  
**Retention**: 7-day rolling window for CRDT delta log; older = compacted into snapshots

---

### **Layer 5: Runtime (Service Worker + PWA + Game Loop) — Lifecycle + Offline Resilience**

**Components**:
- **Service Worker**: Background sync for pending tx (DataChannel offline queue → IndexedDB), push notifications
- **PWA Manifest**: Installable app, 60fps game loop via `requestAnimationFrame`
- **Tauri Bridge** (desktop): Filesystem persistence beyond browser quota, system tray integration
- **Notification API**: In-app toast (DataChannel) + optional native notification (browser permission)

**Data In**: User input (game events), resumed DataChannel after network recover, Notification API events  
**Data Out**: Frame render data (WebGL), background sync payloads, telemetry to Prometheus  
**Offline Mode**: Optimistic updates cached in IndexedDB; reconcile on reconnect via CRDT merge

---

## Data Flow Diagram

```
User Input (KB keypress)
    ↓
[5. Runtime] requestAnimationFrame
    ↓ (encoded KB → CRDT delta)
[1. Core] place_block() → delta_buffer
    ↓ (batched @20ms)
[2. Network] DataChannel.send() to 8 peers
    ↓ (broadcast message arrives)
[2. Network] recv() → unserialize
    ↓
[3. Compute] Worker.merge_delta() (non-blocking)
    ↓ (validation result)
[1. Core] apply_delta() → new CRDT state
    ↓
[4. Storage] IndexedDB append (delta log)
    ↓
[5. Runtime] Render state (WebGL) + trigger OPFS checkpoint @epoch
    ↓ (user sees block placed on all 50 peers within 200ms)
```

---

## Implementation Readiness Checklist

| Layer | Status | Gap |
|-------|--------|-----|
| **1. Core (WASM)** | ✅ Complete | Unit tests only; E2E sync untested |
| **2. Network (WebRTC)** | ⚠️ 60% | Needs backpressure integration, view sync |
| **3. Compute (Workers)** | ⚠️ Design only | Stub implementations, no perf profile |
| **4. Storage (OPFS)** | ⚠️ Design only | CRDT snapshot serialization format TBD |
| **5. Runtime (PWA)** | ⚠️ Tauri wrapper only | Game loop + offline sync not wired |

**Next**: INT_MemFix (Layer 2 stabilization) → INT_CrdtSync (Layer 1↔2 integration) → Full E2E

---

## Key Design Constraints

- **Memory**: <600KB heap @ 50 peers (Layer 3 Worker offload critical)
- **Latency**: p99 <200ms end-to-end (Layer 2 batching @ 20ms)
- **Consensus**: p99 <1500ms (Layer 1 HotStuff view change timeout)
- **Bandwidth**: ~40B/msg header (Layer 2 binary protocol) + payload
- **Offline**: 30min pending queue (Layer 4 IndexedDB) before data loss
- **Browser Quota**: 50GB available (2KB per user × 25M users); OPFS 100MB/room limit

---

**References**: CRDT_SYNC_ARCHITECTURE.md, BFT_TRANSPORT_WEBRTC_DESIGN.md, apt-progress.md  
**Longinus**: `src/wasm.rs` (Layer 1), `src/p2p/webrtc.rs` (Layer 2), `src/compute/worker-pool.rs` (Layer 3, TBD)
