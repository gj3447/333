# 5-Layer Architecture: Visual Diagrams
**KG: ARCHITECTURE_BrowserNative5Layer, SPAN_333_Architecture**

---

## Diagram 1: Layer Stack

```
┌─────────────────────────────────────────────────────┐
│ LAYER 5: Runtime (Service Worker + PWA + Game Loop) │
│  ├─ requestAnimationFrame (60fps)                   │
│  ├─ SW: background sync, offline queue              │
│  └─ PWA: installable, push notifications            │
└──────────────────┬──────────────────────────────────┘
                   │ (user input, frame render)
┌──────────────────┴──────────────────────────────────┐
│ LAYER 4: Storage (OPFS + IndexedDB + Cache API)     │
│  ├─ OPFS: block data (100MB/room)                   │
│  ├─ IndexedDB: metadata, delta log (7d window)      │
│  └─ Cache API: static assets, offline bootstrap     │
└──────────────────┬──────────────────────────────────┘
                   │ (persist deltas, load snapshots)
┌──────────────────┴──────────────────────────────────┐
│ LAYER 3: Compute (Web Workers + SharedArrayBuffer)  │
│  ├─ Worker #1-4: merge_delta, batch_verify         │
│  ├─ SharedArrayBuffer: zero-copy CRDT snapshots    │
│  └─ Task queue: priority-based (consensus > CRDT)  │
└──────────────────┬──────────────────────────────────┘
                   │ (validation, offload CPU)
┌──────────────────┴──────────────────────────────────┐
│ LAYER 2: Network (WebRTC DataChannel Mesh)          │
│  ├─ 20 peer limit (memory constraint)               │
│  ├─ Binary protocol: 4B header + postcard payload   │
│  ├─ Backpressure: 50KB buffer/peer                  │
│  └─ Signaling: ws333 (SDP/ICE broker only)          │
└──────────────────┬──────────────────────────────────┘
                   │ (wire messages, peer sync)
┌──────────────────┴──────────────────────────────────┐
│ LAYER 1: Core (WASM Rust)                           │
│  ├─ HotStuff BFT: 8–50 validators, 1500ms consensus │
│  ├─ CRDT: LWW-Map, delta-state, epoch compaction    │
│  ├─ Ed25519: signatures, 32B public key ID          │
│  └─ Token: 333M cap, 15tok/epoch, burn baseline     │
└─────────────────────────────────────────────────────┘
```

---

## Diagram 2: Data Flow (Per Frame @60fps)

```
[USER] presses KB (place_block)
  │
  ↓
[LAYER 5: Runtime]
  ├─ event listener catches input
  ├─ dispatch to Layer 1
  │ 
  ↓ (+1ms)
[LAYER 1: Core WASM]
  ├─ place_block() → LWW-Map delta
  ├─ delta_buffer.push()
  ├─ schedule batch flush @20ms
  │
  ↓ (+20ms, batched)
[LAYER 2: Network]
  ├─ serialize batch (StateUpdate + Propose)
  ├─ DataChannel.send() to 8 peers
  │
  ║ Network transit (+50ms RTT)
  ║
  ↓
[PEER's LAYER 2: Network]
  ├─ DataChannel.message event
  ├─ unserialize (postcard)
  │
  ↓ (+0ms, async to Layer 3)
[LAYER 3: Compute]
  ├─ Worker.merge_delta() in background
  ├─ non-blocking; result goes to result channel
  │
  ↓ (meanwhile, Layer 1 continues)
[PEER's LAYER 1: Core]
  ├─ apply_delta() → new CRDT state
  ├─ (if merge result ready, apply verify result)
  │
  ↓ (+0ms async)
[LAYER 4: Storage]
  ├─ IndexedDB.append(delta log)
  ├─ OPFS.write() scheduled @epoch (async)
  │
  ↓ (+16ms, next 60fps frame)
[LAYER 5: Runtime]
  ├─ WebGL render with updated state
  └─ [USER SEES block placed on peer]

Total latency: 165ms p99 (under 200ms budget) ✅
```

---

## Diagram 3: Inter-Layer APIs

```
┌─ Layer 5 ─────────────────────┐
│ on_frame()                     │  ← requestAnimationFrame hook
│ notify(event: UserInput)       │  ← DOM event dispatch
│ render(state: CrdtState)       │  ← WebGL draw call
└───────────────┬────────────────┘
                │ wasm::Platform333 instance
                ↓
┌─ Layer 1 ─────────────────────────────┐
│ platform.update(input)                 │
│ platform.consensus_round()             │
│ platform.sync_mgr.poll_and_send()      │
└───────────────┬──────────────┬─────────┘
                │              │
         (Layer 2 impl)   (Layer 4 via IndexedDB)
                │              │
                ↓              ↓
┌─ Layer 2 ─────────────────┐  ┌─ Layer 4 ─────────────┐
│ Transport::send()          │  │ IdbStore::append()    │
│ Transport::broadcast()     │  │ OPFSSnapshot::write() │
│ Transport::recv()          │  └───────────────────────┘
│ on_view_change()           │
│ validate_msg()             │
└───────────────┬────────────┘
                │ (serialized message)
                ↓
        [WebRTC DataChannel]

┌─ Layer 3 (async) ──────────────────┐
│ worker.post_message({               │
│   task: "merge_delta",              │
│   deltas: Vec<LwwDelta>             │
│ })                                  │
│                                     │
│ worker.onmessage = (result) =>      │
│   layer1.apply_verify_result()      │
└─────────────────────────────────────┘
```

---

## Diagram 4: Latency Budget Breakdown

```
Frame: 0ms ──────────────────────────→ 16ms (60fps) ──────→ 32ms (next frame)
       │                                  │
       └─ User presses KB                └─ [RENDER READY]
          │
          ├─ [L5] Event fire: +1ms
          │
          ├─ [L1] place_block(): +1ms (now ~2ms into frame 0)
          │
          │  (batch accumulates... 18ms pass)
          │
          ├─ [L2] send @20ms: +5ms serialize, +1ms DataChannel.send
          │
          │  (meanwhile: network transit +50ms)
          │
          └─ Peer receives: ~55ms into frame 3 or 4
             │
             ├─ [L2] recv event: +0ms
             │
             ├─ [L3] Worker.merge: +20ms (off-main-thread)
             │
             ├─ [L1] apply_delta: +2ms (next update cycle)
             │
             ├─ [L4] IndexedDB.append: +50ms async
             │
             └─ [L5] Render: +16ms (when render cycle comes around)

Total wall-clock: 165ms (p99) within 200ms budget ✅
```

---

## Diagram 5: Memory Constraint (500KB limit @ 50 peers)

```
Per Peer:
┌─────────────────────────────────┐
│ WebRTC DataChannel state: 2KB   │  (buffer + metadata)
│ Tx buffer (50KB limit): 50KB    │  (backpressure cutoff)
│ Rx buffer: 10KB                 │  (message queue)
├─────────────────────────────────┤
│ Subtotal per peer: ~62KB        │
├─────────────────────────────────┤
│ 8 active peers: 8 × 62KB = 496KB│  ← stays under 600KB
├─────────────────────────────────┤
│ Shared (CRDT state, BFT state): │
│   CRDT: 50KB (1000 LWW entries) │
│   BFT: 20KB (votes, proposals)  │
│   Token ledger: 10KB (balances) │
├─────────────────────────────────┤
│ Worker pool overhead: 30KB      │  (4 workers)
└─────────────────────────────────┘

Total @ 8 peers: ~606KB (triggers worker offload for merge_delta)
```

**Key**: With >10 peers, CRDT merge must run in Worker (Layer 3) to free main-thread heap.

---

**References**: BROWSER_NATIVE_5LAYER_ARCHITECTURE.md, ARCHITECTURE_INTEGRATION_MAP.md, apt-progress.md
