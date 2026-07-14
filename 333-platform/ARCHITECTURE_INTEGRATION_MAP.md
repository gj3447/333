# 5-Layer Architecture Integration Map
## How Data Flows Between Layers (333 Platform)
**KG: ARCHITECTURE_BrowserNative5Layer, INTEGRATION_DataFlowMap**
**Date**: 2026-04-13

---

## Layer-to-Layer Data Flow

### **Layer 5 (Runtime) ↔ Layer 1 (Core)**

| Direction | Data | Frequency | Format |
|-----------|------|-----------|--------|
| 5→1 | User input (KB press, mouse click) | Per event | Enum (PlaceBlock, ...) |
| 1→5 | Local CRDT delta, block placed | Per action | LwwDelta struct |
| 1→5 | Consensus vote ready | Per round | Vote(block_hash) |

**Integration Point**: `on_frame()` callback in wasm.rs; Runtime calls Core's update loop.

---

### **Layer 1 (Core) ↔ Layer 2 (Network)**

| Direction | Data | Frequency | Format |
|-----------|------|-----------|--------|
| 1→2 | CRDT delta batch, BFT message | @20ms batch | StateUpdate + Propose |
| 2→1 | Peer message (CRDT/consensus) | Per peer msg | Deserialize via postcard |
| 2→1 | View number, leader status | @heartbeat | current_view(), is_leader_alive() |

**Integration Point**: Transport trait in bft/transport.rs; Layer 2 implements recv()/send().

---

### **Layer 2 (Network) ↔ Layer 3 (Compute)**

| Direction | Data | Frequency | Format |
|-----------|------|-----------|--------|
| 2→3 | CRDT delta bundle, sig bundle | Per batch | Vec<LwwDelta> + Vec<(NodeId, Sig)> |
| 3→2 | Merge verdict, sig valid/invalid | After work | bool result to Layer 1 via channel |

**Integration Point**: Worker pool in compute/worker-pool.rs; Layer 2 queues tasks, doesn't wait.

---

### **Layer 1 (Core) ↔ Layer 4 (Storage)**

| Direction | Data | Frequency | Format |
|-----------|------|-----------|--------|
| 1→4 | Finalized delta, new block | Per consensus | (epoch, height, block_data) |
| 4→1 | Snapshot on room load | Once/session | Full CRDT state |
| 4→1 | Delta log (last 100 epochs) | Reconnect | Vec<Delta> for catch-up |

**Integration Point**: IndexedDB store in wasm.rs; append on Layer 1 apply_delta().

---

### **Layer 4 (Storage) ↔ Layer 5 (Runtime)**

| Direction | Data | Frequency | Format |
|-----------|------|-----------|--------|
| 4→5 | Loaded room state | Room enter | OPFS file handle + IndexedDB rows |
| 5→4 | Checkpoint trigger | @epoch | Signal to commit (async via SW) |
| 5→4 | Pending tx queue | Offline | IndexedDB append |

**Integration Point**: Service Worker for background persistence; OPFS.getFile() on startup.

---

### **Layer 5 (Runtime) ↔ Layer 2 (Network) [Offline Bridge]**

| Direction | Data | Frequency | Format |
|-----------|------|-----------|--------|
| 5→2 | Resume connection (after network recover) | On reconnect | Reconnect signal |
| 2→5 | DataChannel status (open/closed) | Per event | event listener callback |
| 5→4→2 | Replay pending tx from IndexedDB | Reconnect | Full CRDT merge reconciles |

**Integration Point**: Service Worker detects network change; wakes up Layer 2 / Layer 4.

---

## Critical Integration Points (Bottlenecks)

### 1. **Layer 1 ↔ Layer 2: Message Serialization**
- **Bottleneck**: Postcard serialization + wire protocol overhead
- **Mitigation**: 4-byte binary header (type + len), reuse buffers
- **Test**: p99 <50μs per msg (unblock in Layer 3 if >100μs)

### 2. **Layer 2 ↔ Layer 3: Worker Queue Depth**
- **Bottleneck**: Main thread stalls if merge_delta() blocks >1ms
- **Mitigation**: Task queue with priority (consensus > CRDT > telemetry)
- **Test**: Queue depth <10; reject new tasks if >100 pending

### 3. **Layer 3 ↔ Layer 1: Result Channel Latency**
- **Bottleneck**: Worker result waiting in Layer 1 state machine
- **Mitigation**: Non-blocking; store result, apply next frame
- **Test**: p99 <500ms from work submission to apply

### 4. **Layer 4 ↔ Layer 5: OPFS Write Latency**
- **Bottleneck**: Synchronous writes during consensus round block main thread
- **Mitigation**: Async Service Worker; append-only IndexedDB first
- **Test**: Main thread never waits on OPFS (async only)

### 5. **Layer 5 ↔ Layer 2: Offline Queue Reconciliation**
- **Bottleneck**: Pending tx replay during reconnect causes merge storms
- **Mitigation**: Throttle replay to 100ms batches; CRDT handles out-of-order
- **Test**: Offline 30min → reconnect → full consistency within 5s

---

## Data Flow Timing (Latency Budget: p99 <200ms)

```
User press KB          0ms
Layer 5 (dispatch)    +1ms (event listener)
Layer 1 (place_block) +1ms (WASM call)
delta_buffer flush    +20ms (batched)
Layer 2 (send)        +5ms (DataChannel.send)
Network transit       +50ms (WebRTC RTT)
Peer recv             +0ms (event immediate)
Layer 3 (merge)       +20ms (Worker non-blocking)
Layer 1 (apply)       +2ms (in next frame)
Layer 4 (IndexedDB)   +50ms (async, background)
Layer 5 (render)      +16ms (next 60fps frame)
─────────────────────────
Total p99             ~165ms (well under 200ms budget)
```

---

## Implementation Dependency Graph

```
Layer 5 (Runtime)
    ↑ depends on
Layer 1 (Core)  ←→  Layer 2 (Network)
    ↑ depends on          ↑ depends on
Layer 3 (Compute)   [Signaling Server ws333]
    ↑ depends on
Layer 4 (Storage)

Critical path: Core → Network → Runtime (user sees result)
Offload path: Network → Compute → Core (validation)
Persistence: Core → Storage → Runtime (durability)
```

---

## Next Integration Phases

| Phase | Task | Integrates | Duration |
|-------|------|-----------|----------|
| **INT_MemFix** | Stabilize Layer 2 heap | Layer 2 memory ops | 1d |
| **INT_CrdtSync** | Wire Layers 1+2 | CRDT real-time sync | 2d |
| **INT_ConsensusNet** | Wire Layers 1+2 (BFT) | Full consensus loop | 2d |
| **INT_E2E** | Run full 2-browser sync + Layer 4+5 | All layers | 2d |

**KG References**: apt-progress.md (SP decomposition), CRDT_SYNC_ARCHITECTURE.md, BFT_TRANSPORT_WEBRTC_DESIGN.md
