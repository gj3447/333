# CRDT Sync Loop: Quick Reference Card

**KG: QUICKREF_333_CrdtSync** | Bookmark this!

---

## The 6 Questions + Answers (TL;DR)

| Q | A | Why |
|---|---|-----|
| 1. **Immediate or batched?** | **Batched 20ms** | 1.2 frame latency, 50-100x BW savings |
| 2. **Delta batching?** | **VecDeque + IndexedDB** | Speed + durability |
| 3. **State vector?** | **HashMap<u32, u32>** | O(1), <50 peers typical |
| 4. **Reconnection?** | **Threshold 1000 deltas** | Cost crossover |
| 5. **Full snapshot?** | **On join + gap >1000** | Balance cost/frequency |
| 6. **Ordering?** | **Reliable ordered channel** | Simplicity + efficiency |

---

## One-Page Architecture

```
place_block("0,0,0", stone)
        ↓
world.set(key, value) → LwwDelta
        ↓
sync_mgr.on_local_delta(delta)
        ↓
[VecDeque buffer: delta_1, delta_2, ..., delta_10]
        ↓
[20ms timer elapsed?]
        ↓
sync_mgr.flush()
├─ batch deltas
├─ add state_vector
├─ encode to JSON
├─ wire::encode(StateUpdate)
└─ room.broadcast() → WebRTC DC (Reliable)
        ↓
        ↓ [Network ~5-20ms]
        ↓
[Peer B receives]
        ↓
wire::decode() → SyncPayload
        ↓
for delta in payload.deltas:
  world.merge_delta(delta)
        ↓
update state_vector
        ↓
[B's world now matches A's]
```

---

## Message Types (4 total)

```
MsgType::StateUpdate (0x01)  ← Regular sync (deltas)
├─ payload: JSON { deltas: [LwwDelta], state_vector: {...} }
├─ frequency: Every 20ms (batched)
└─ cost: ~50 bytes per delta

MsgType::StateFull (0x02)    ← Join or reconnection
├─ payload: JSON { entries: HashMap<K, (V, timestamp)> }
├─ frequency: On join OR gap >1000 deltas
└─ cost: ~50 bytes × unique_keys

(Other types: Presence, Consensus, Heartbeat — out of scope for sync)
```

---

## Code Touchpoints

### Must Create
- **src/sync.rs** — New SyncManager struct + module

### Must Modify
- **src/lib.rs** — Add `pub mod sync;`
- **src/wasm.rs** — Add `sync_mgr` field, call `poll_and_send()` + handlers
- **src/lww_map.rs** — Add `from_snapshot()` + `snapshot()` methods

### No Changes Needed
- **src/p2p/** — DataChannel already supports Reliable
- **src/wire.rs** — StateUpdate + StateFull already defined
- **src/platform.rs** — Exec layer unchanged

---

## SyncManager API Cheat Sheet

```rust
let mut sync = SyncManager::new(node_id);

// When user places block:
sync.on_local_delta(delta);

// Every frame (~16ms):
sync.poll_and_send(now_ms, &room);

// When peer message arrives:
sync.on_peer_sync(from_peer, payload, &mut world);

// When full state arrives:
sync.on_peer_full_state(payload, &mut world);

// Utility:
sync.should_send_full_state(&peer_vector);
sync.get_state_vector();
sync.buffer_len();
```

---

## State Vector (The Key Trick)

```rust
state_vector = {
    peer_1: 42,   // P1 wrote 42 operations total
    peer_2: 38,   // P2 wrote 38 operations total
    peer_3: 5,    // P3 wrote 5 operations (new)
}

// On receive StateUpdate from peer:
//   Merge their SV into ours → learn about other peers' clocks
// On reconnect:
//   "I've seen up to: {P1:20, P2:15}"
//   → Send only deltas where (sender, counter) > their vector
```

---

## Threshold: When to Send Full State

```
missing_deltas = sum of (our_SV[p] - their_SV[p]) for all p

if missing_deltas > 1000:
    → send StateFull (full snapshot)
else:
    → send StateUpdate (deltas only)
```

**Why 1000?**
- 1000 deltas × 50 bytes = 50KB
- Full state ≈ 5MB (at 100K blocks)
- Threshold = cost crossover point

---

## Convergence = Guaranteed

Even if peers receive deltas in different order:
```
P1 writes: stone (timestamp 1:10)
P2 writes: dirt (timestamp 2:5)

P1→P2→P3 order:
├─ Apply 1:10 → stone
├─ Apply 2:5 → skip (5<10)
└─ Final: stone ✓

P2→P1→P3 order:
├─ Apply 2:5 → dirt
├─ Apply 1:10 → overwrite (10>5)
└─ Final: stone ✓

LWW + Lamport = deterministic winner (node_id as tiebreaker)
```

---

## Testing Checklist

```
Unit Tests (Rust)
 ☐ SyncManager::on_local_delta() queues delta
 ☐ SyncManager::poll_and_send() flushes every 20ms
 ☐ State vector updates correctly
 ☐ should_send_full_state() threshold at 1000
 ☐ Idempotency: merge same delta twice = merge once

Integration Tests (Browser)
 ☐ Two browsers in room
 ☐ Block placed on A
 ☐ Received on B within 50ms
 ☐ Offline 10min, reconnect → B catches up
 ☐ 100 concurrent blocks → both converge
```

---

## Debugging Commands

```bash
# Build and test
cargo test --lib sync

# WASM build
wasm-pack build --target web --release

# Browser: Open DevTools Console
place_block("0,0,0", "stone");
// [20ms later...]
console.log(world_state);
// Should see stone at 0,0,0 in both browsers
```

---

## Performance Targets

| Metric | Target | Status |
|--------|--------|--------|
| Batching overhead | <5% | ✓ 20ms amortized |
| Merge latency | <10ms per 1000 ops | ✓ O(n) acceptable |
| Bandwidth | <100 B/op | ✓ 50B + headers |
| Memory | <10MB for 100K blocks | ✓ O(keys) |
| Convergence latency | <50ms (20ms batch + RTT) | ✓ Imperceptible |

---

## Common Pitfalls

| ❌ Wrong | ✅ Right | Why |
|---------|---------|-----|
| Immediate send each op | Batch 20ms | BW savings, simple |
| Keep all deltas forever | Persist + GC by epoch | Memory bounded |
| Don't send state vector | Piggyback in every msg | Enables incremental |
| Require total order | Use HLC + reliable chan | Unnecessary, complex |
| Send full state every time | Threshold 1000 deltas | Cost efficient |
| Unreliable channel | Reliable ordered | Simplicity + durability |

---

## Link Map

| Document | Purpose |
|----------|---------|
| **CRDT_SYNC_ARCHITECTURE.md** | Deep dive: design rationale, algorithms |
| **CRDT_SYNC_DECISIONS.md** | Comparison tables, risk assessment |
| **CRDT_SYNC_IMPL_GUIDE.md** | Copy-paste code, integration steps |
| **CRDT_SYNC_QUICKREF.md** | This page: bookmark it! |
| **apt-progress.md** | Status of 333 Platform (INT phase) |

---

## Questions to Ask Yourself

- [ ] Do I understand why 20ms batching is better than immediate?
- [ ] Can I explain what state_vector does in <30 seconds?
- [ ] Why does Lamport timestamp guarantee convergence even out-of-order?
- [ ] When would we send StateFull vs StateUpdate?
- [ ] What's the cost of 1000 missing deltas vs full snapshot?

---

## Gotchas & Gotcha-Preventers

### Gotcha 1: State Vector Skew
**Problem**: If peer says "I've seen 1000 ops" but has seen only 900, we send wrong deltas.
**Prevention**: Store state vector in durable log + verify with checksums.

### Gotcha 2: Memory Explosion
**Problem**: Delta buffer grows unbounded if flush() never runs.
**Prevention**: Force flush if buffer > 1000 deltas (backpressure).

### Gotcha 3: Ordering Assumption
**Problem**: Code assumes FIFO, but DataChannel dropped a packet.
**Prevention**: Use reliable ordered channel (default in modern WebRTC).

### Gotcha 4: Convergence Delay
**Problem**: User expects instant sync but we batch 20ms.
**Prevention**: 20ms is imperceptible at 60fps; explain to stakeholders.

### Gotcha 5: Full State Size
**Problem**: Sending 100K blocks × 50 bytes = 5MB takes seconds.
**Prevention**: Only send on join or gap >1000; compress if needed later.

---

## Ready to Code?

1. Read **CRDT_SYNC_IMPL_GUIDE.md** (copy-paste src/sync.rs)
2. Follow the **Integration Checklist** section
3. Run `cargo test`
4. Open browser `/333/wasm/p2p-demo.html`
5. Place block on peer A, see it on peer B ✓

---

**Created**: 2026-04-13 | **For**: 333 Platform INT_CrdtSync | **Status**: Ready to implement

**KG**: SPAN_333_CrdtSync, ARCHITECTURE_CrdtSyncLoop, DECISIONS_333_CrdtSync, IMPL_333_CrdtSync
