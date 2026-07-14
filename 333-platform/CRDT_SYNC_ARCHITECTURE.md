# CRDT Delta-State Synchronization Loop Architecture
## 333 Platform (Rust WASM + WebRTC P2P)

**KG: SPAN_333_CrdtSync, ARCHITECTURE_CrdtSyncLoop**

**Date**: 2026-04-13 | **Context**: INT_CrdtSync AtomicSpan | **Status**: Research → Implementation Ready

---

## Executive Summary

This document provides a **concrete, production-ready sync loop** for the 333 Platform's LWW-Map CRDT. It answers all 6 key questions about delta-state synchronization:

1. ✅ **Sync loop design**: Batched (10-50ms) + periodic polling
2. ✅ **Delta batching**: In-memory VecDeque, flushed every 20ms
3. ✅ **State vector**: HashMap<peer_id, max_counter_seen> for incremental sync
4. ✅ **Reconnection sync**: StateFull (full snapshot) vs Incremental deltas based on gap size
5. ✅ **Full state snapshot**: Sent on new peer join OR gap >1000 deltas
6. ✅ **Ordering guarantees**: HLC + Lamport timestamps + Reliable channel = causal consistency

---

## 1. Immediate vs Batched Send: Design Decision

### Decision: **BATCHED with 20ms interval**

| Strategy | Latency | Throughput | Bandwidth | Use Case |
|----------|---------|-----------|-----------|----------|
| **Immediate** | <1ms | ❌ Poor | ❌ Wasteful | Consensus, not CRDT |
| **Batch 10ms** | 10ms | ✅ Good | ✅ Good | Real-time collaborative editing |
| **Batch 20ms** (chosen) | 20ms | ✅ Good | ✅ Excellent | Minecraft-like blocks, UI @60fps |
| **Batch 50ms** | 50ms | ✅ Excellent | ✅ Excellent | Non-interactive (analytics) |
| **Batch 100ms+** | >100ms | ✅ Excellent | ✅ Excellent | ❌ Perceptually laggy |

### Rationale for 20ms

- **60 FPS = 16.67ms frame**: 20ms batching = ~1.2 frames latency (imperceptible in Minecraft)
- **Compression**: Batching multiple place_block() ops into 1 StateUpdate reduces headers by 10-100x
- **Bandwidth**: At 100 ops/sec, immediate = 400B headers/sec; batched = 40B headers/sec
- **Network efficiency**: Most WebRTC DataChannels buffer anyway (30-100ms RTT typical)

### Polling Hook (integration point)

```rust
// In wasm.rs main update loop
pub fn on_frame(platform: &mut PlatformCore, room: &MeshRoom, now_ms: u64) {
    // ... handle user input, state updates ...
    
    // NEW: Poll sync manager at end of frame
    platform.sync_mgr.poll_and_send(now_ms, room);
}
```

---

## 2. Delta Batching: Accumulation Strategy

### Architecture: In-Memory Buffer + Periodic Flush

```rust
pub struct SyncManager {
    // Buffer of local deltas pending transmission
    delta_buffer: VecDeque<(u32, LwwDelta<String, String>)>,
    // peer_id → max Lamport counter seen from that peer
    state_vector: HashMap<u32, u32>,
    
    // Configuration
    local_node_id: u32,
    last_send_ms: u64,
    batch_interval_ms: u64,  // 20ms default
    max_buffer_size: usize,  // 1000 deltas before force flush
}

impl SyncManager {
    /// Called immediately after place_block() generates delta
    pub fn on_local_delta(&mut self, delta: LwwDelta<String, String>) {
        self.delta_buffer.push_back((self.local_node_id, delta));
        
        // Force flush if buffer grows too large (backpressure)
        if self.delta_buffer.len() >= self.max_buffer_size {
            self.flush_now();  // Don't wait for interval
        }
    }

    /// Called every ~16ms from main update loop
    pub fn poll_and_send(&mut self, now_ms: u64, room: &MeshRoom) {
        if now_ms - self.last_send_ms >= self.batch_interval_ms {
            self.flush(room);
            self.last_send_ms = now_ms;
        }
    }

    fn flush(&mut self, room: &MeshRoom) {
        if self.delta_buffer.is_empty() {
            return;  // No-op if nothing to send
        }

        // Batch all pending deltas into one message
        let deltas_batch: Vec<_> = self.delta_buffer.drain(..).collect();
        let payload = SyncPayload {
            deltas: deltas_batch,
            state_vector: self.state_vector.clone(),  // Piggyback our knowledge
        };

        // Serialize and send
        let json = serde_json::to_string(&payload).unwrap();
        let encoded = wire::encode(MsgType::StateUpdate, json.as_bytes()).unwrap();
        let _ = room.broadcast(&encoded, ChannelMode::Reliable);
    }
}
```

### How Yjs/Automerge Compare

| System | Batching | Storage | GC |
|--------|----------|---------|-----|
| **Yjs** | Async update queue (5-10ms) + manual flush | IndexedDB | State vector + bloom filter |
| **Automerge** | Actor-based, commit-level | In-memory snapshot | Heads vector |
| **333 (Proposed)** | VecDeque + interval timer | In-memory + persistent (storage.put) | Epoch-based compaction |

---

## 3. State Vector / Version Vector: Incremental Sync Knowledge

### Concept

A **state vector** is a map `peer_id → max_counter_seen` that tracks causality. It answers: "What is the latest clock value I've seen from peer P_i?"

### Data Structure

```rust
type StateVector = HashMap<u32, u32>;

// Example after 3 peers have been active:
// {
//   peer_1: 42,  // P1 has written 42 ops total
//   peer_2: 38,  // P2 has written 38 ops total
//   peer_3: 0,   // P3 is new (seen nothing yet)
// }
```

### Algorithm: How It Enables Incremental Sync

#### Normal Operation (continuous sync)

```rust
pub fn on_peer_sync(&mut self, from_peer: u32, payload: SyncPayload, world: &mut LwwMap) {
    // 1. Apply all deltas from peer
    for (sender_node_id, delta) in &payload.deltas {
        world.merge_delta(delta);
    }
    
    // 2. Update state vector with what peer told us it has seen
    //    Merge their knowledge into ours
    for (peer_id, counter) in &payload.state_vector {
        let local_counter = self.state_vector.entry(*peer_id).or_insert(0);
        *local_counter = (*local_counter).max(*counter);
    }
    
    // 3. Update what we know THIS peer has seen from us
    //    (Infer from delta timestamps)
    let max_our_counter = payload.deltas.iter()
        .filter_map(|(sender, delta)| {
            if *sender == self.local_node_id {
                delta.changes.iter().map(|(_, entry)| entry.timestamp.counter).max()
            } else {
                None
            }
        })
        .max()
        .unwrap_or(0);
    
    if max_our_counter > 0 {
        let entry = self.state_vector.entry(self.local_node_id).or_insert(0);
        *entry = (*entry).max(max_our_counter);
    }
}
```

#### Reconnection (peer was offline, now back)

```rust
pub fn on_peer_reconnect(
    &self,
    peer_id: u32,
    peer_state_vector: StateVector,
    world: &LwwMap,
) -> (MsgType, Vec<u8>) {
    // Peer tells us: "I have seen up to: {P1:20, P2:15, P3:10}"
    
    // Decision: incremental or full state?
    let missing_count = self.compute_missing_delta_count(&peer_state_vector);
    
    if missing_count < 1000 {
        // Small gap: send only missing deltas
        let deltas = self.compute_missing_deltas(&peer_state_vector);
        let payload = SyncPayload {
            deltas,
            state_vector: self.state_vector.clone(),
        };
        (MsgType::StateUpdate, serde_json::to_vec(&payload).unwrap())
    } else {
        // Large gap: send full state snapshot
        let snapshot = world.snapshot();  // HashMap<K, LwwEntry<V>>
        let payload = FullStatePayload {
            entries: snapshot.clone(),
        };
        (MsgType::StateFull, serde_json::to_vec(&payload).unwrap())
    }
}

fn compute_missing_delta_count(&self, peer_vector: &StateVector) -> usize {
    // Sum of (what_we_know - what_peer_knows) for all peers
    self.state_vector.iter().map(|(peer_id, our_count)| {
        let peer_count = peer_vector.get(peer_id).copied().unwrap_or(0);
        our_count.saturating_sub(peer_count)
    }).sum()
}
```

### Correctness: Happens-Before Relation

With state vectors and Lamport clocks:

1. If `StateVector[P_i] >= ts.counter`, we have seen P_i's operation #counter
2. Lamport timestamps encode causal order (T1 < T2 → T1 happened before T2)
3. Merging is **idempotent**: applying same delta twice = applying once (CRDT property)
4. Convergence guaranteed: all peers eventually reach same state if all deltas delivered

---

## 4. Reconnection Sync: Full vs Incremental

### Decision Tree

```
┌─ Peer reconnects with state_vector S_peer
│
├─ Can we compute missing deltas?
│  │  (deltas where peer_id ∈ S_peer AND counter > S_peer[peer_id])
│  │
│  ├─ gap_size < 1000 deltas
│  │  └─→ Send Incremental (StateUpdate)
│  │      Cost: O(gap_size)
│  │      Benefit: Low bandwidth, fast
│  │
│  └─ gap_size >= 1000 deltas
│     └─→ Send Full Snapshot (StateFull)
│         Cost: O(unique_keys) = O(100K keys at most)
│         Benefit: Simpler, one message
│
└─ Fallback: state_vector indicates peer has future clock
   (clock skew or Byzantine peer?)
   → Send full state to overwrite
```

### Implementation: Incremental Case

```rust
fn compute_missing_deltas(&self, peer_vector: &StateVector) -> Vec<(u32, LwwDelta<String, String>)> {
    // Query delta history: only return deltas peer hasn't seen
    self.delta_buffer.iter()
        .filter(|(sender_node_id, delta)| {
            let peer_counter = peer_vector.get(sender_node_id).copied().unwrap_or(0);
            // Only include if this delta's clock > what peer has seen from this sender
            delta.changes.iter().any(|(_, entry)| entry.timestamp.counter > peer_counter)
        })
        .cloned()
        .collect()
}
```

**Issue**: delta_buffer is VecDeque (append-only), but we need to query by sender + counter. 

**Solution**: Use persistent delta log (IndexedDB or RocksDB on desktop)

```rust
pub struct DeltaLog {
    // In-memory: recent deltas
    recent: VecDeque<(u32, LwwDelta)>,
    // Persistent: all deltas >= some epoch
    storage: Box<dyn KVStore>,  // key="delta_{peer_id}_{counter}", value=LwwDelta
}

impl DeltaLog {
    pub fn query(&self, peer_vector: &StateVector) -> Vec<(u32, LwwDelta)> {
        let mut result = Vec::new();
        for (sender_node_id, min_counter) in peer_vector {
            // Load from storage: delta_{sender_node_id}_{min_counter+1..current}
            for counter in (*min_counter + 1)..=self.max_counter[sender_node_id] {
                let key = format!("delta_{}_{}", sender_node_id, counter);
                if let Some(bytes) = self.storage.get(key.as_bytes()) {
                    let delta = bincode::deserialize(&bytes).unwrap();
                    result.push((*sender_node_id, delta));
                }
            }
        }
        result
    }
}
```

### Implementation: Full Snapshot Case

```rust
// wire.rs already has StateFull message type (0x02)
pub fn send_full_state(&self, room: &MeshRoom, world: &LwwMap) {
    let entries = world.snapshot().clone();  // HashMap<String, LwwEntry<String>>
    
    let payload = serde_json::json!({
        "entries": entries,
    });
    
    let encoded = wire::encode(MsgType::StateFull, serde_json::to_string(&payload).unwrap().as_bytes()).unwrap();
    let _ = room.broadcast(&encoded, ChannelMode::Reliable);
}

// Receiver side
pub fn on_full_state(&mut self, payload: FullStatePayload, world: &mut LwwMap) {
    // Reset local state and apply snapshot
    *world = LwwMap::from_snapshot(payload.entries);
    
    // Reset state vector (we now know all deltas)
    self.state_vector = payload.entries.iter()
        .flat_map(|(_, entry)| {
            vec![(entry.timestamp.node_id, entry.timestamp.counter)]
        })
        .fold(HashMap::new(), |mut acc, (node_id, counter)| {
            let max = acc.entry(node_id).or_insert(0);
            *max = (*max).max(counter);
            acc
        });
}
```

---

## 5. Full State Snapshot: When to Send

### Triggers for StateFull

1. **New peer joins room** (MeshEvent::PeerJoined)
   - Immediately send full state to bring new peer up to date
   - Cost: One-time O(unique_keys), acceptable

2. **Reconnection with large gap** (compute_missing_deltas_count() > 1000)
   - Cheaper than storing/sending 1000s of small updates
   - Threshold: ~1000 deltas = ~50KB JSON

3. **Periodic snapshot for compaction** (optional)
   - Every 1 hour or 100K deltas, take snapshot and discard old deltas
   - Keeps delta log bounded

4. **Memory pressure** (if delta buffer too large)
   - If VecDeque exceeds memory limit, write oldest batch to storage and truncate

### Cost Analysis: Snapshot vs Incremental

```
Scenario: 100K blocks placed, peer offline 10 minutes, 500 new blocks placed

Full Snapshot:
- Size: ~100K entries × 50 bytes (key + timestamp) = ~5MB
- Encoding: bincode vs JSON (bincode ~2MB better)
- Send time: 2-5s over WebRTC DC (1-2 Mbps typical)
- Decode: merge_full() is O(unique_keys) = ~100ms

Incremental (500 deltas):
- Size: 500 × 50 bytes = ~25KB
- Send time: <1ms
- Decode: 500 × merge_delta() = ~1ms
- Threshold: 500 deltas is clearly cheaper

BUT: 10 minutes = more realistically 50,000 new ops from 10 peers
- Incremental: 50K deltas × 10 peers = 500K delta objects to store
- Full: 100K snapshot (deduplicated)
- Full becomes cheaper at ~10K missing deltas
```

**Recommendation**: Threshold = 5,000 missing deltas OR gap > 1 hour offline

---

## 6. Ordering Guarantees: HLC + Lamport + Reliable Channel

### Guarantee Matrix

| Property | Mechanism | Guaranteed? |
|----------|-----------|-------------|
| **FIFO within peer** | Lamport.counter increments | ✅ Yes |
| **Causal order** | HLC + Lamport timestamps | ✅ Yes |
| **Total order across peers** | Not needed for LWW | ❌ No (unnecessary) |
| **Idempotency** | CRDT merge_delta is idempotent | ✅ Yes (can resend) |
| **Durability** | Reliable channel + persistence | ✅ Yes |

### Why Ordered DataChannel Not Strictly Needed

LWW-Map doesn't require **total order**; it requires **causal consistency**:

```
Timeline: P1 and P2 concurrently write the same key
P1: set("block", value=stone) @ LamportClock(peer=1, counter=5)
P2: set("block", value=dirt)  @ LamportClock(peer=2, counter=3)

Case 1: Messages arrive P1 then P2
- P2: apply P1 (timestamp 1:5), then P2 (3 < 5, ignore)
- Final: stone ✓

Case 2: Messages arrive P2 then P1
- P2: apply P2 first (timestamp 2:3), then P1 (5 > 3, overwrite)
- Final: stone ✓

Total order not needed! LWW ensures same result regardless of order.
Lamport timestamp is sufficient.
```

### Recommended: Use Reliable Channel Anyway

**But use reliable channel anyway** for these reasons:

1. **Simplicity**: FIFO guarantees easier mental model
2. **Durability**: TCP-like semantics = retransmit on loss
3. **Efficiency**: Deltas arrive in order = fewer out-of-order merges
4. **WebRTC default**: RTCDataChannel with ordered=true is standard

```javascript
// Browser: RTCDataChannel configuration
const dc = pc.createDataChannel('sync', {
    ordered: true,      // Reliable, in-order delivery
    maxRetransmits: -1  // Unlimited retransmits (TCP-like)
});
```

---

## Complete Architecture Diagram

```
┌─────────────────────── Browser A ───────────────────────┐
│                                                           │
│  User: place_block("0,0,0", stone)                      │
│              │                                           │
│              ↓                                           │
│  Platform333::execute(CrdtUpdate)                       │
│              │                                           │
│              ├─→ world.set(key, value) → LwwDelta       │
│              │                                           │
│              └─→ sync_mgr.on_local_delta(delta)        │
│                      │                                  │
│              ┌───────┴────────────┐                     │
│              ↓                     ↓                     │
│         [Buffer: delta_0]    [tick counter]             │
│              │                     │                     │
│         (accumulate               (check time)           │
│          10 ops)                   │                     │
│              │                     │                     │
│              └─────────────────────┤                     │
│                                    │                     │
│          If (now - last_send > 20ms)                    │
│                    │                                    │
│                    ↓                                    │
│        sync_mgr.flush()                                │
│        ├─ batch deltas → SyncPayload {deltas, SV}      │
│        ├─ wire::encode(StateUpdate, JSON)              │
│        └─ room.broadcast() → WebRTC DC (Reliable)      │
│                    │                                    │
│                    └──────────→ Wire: [V|T|Len|JSON]   │
│                                    │                    │
└────────────────────────────────────┼────────────────────┘
                                     │
                    ┌────────────────┴────────────────┐
                    │                                 │
                    ↓                                 ↓
        ┌─────────────────────────┐  ┌─────────────────────────┐
        │   Browser B             │  │   Browser C             │
        │                         │  │                         │
        │ DataChannel.onmessage   │  │ DataChannel.onmessage   │
        │         │               │  │         │               │
        │         ↓               │  │         ↓               │
        │ wire::decode(bytes)     │  │ wire::decode(bytes)     │
        │         │               │  │         │               │
        │         ↓               │  │         ↓               │
        │ SyncPayload {D, SV}     │  │ SyncPayload {D, SV}     │
        │         │               │  │         │               │
        │         ↓               │  │         ↓               │
        │ for delta in D:         │  │ for delta in D:         │
        │   world.merge_delta()   │  │   world.merge_delta()   │
        │         │               │  │         │               │
        │         ↓               │  │         ↓               │
        │ update state_vector     │  │ update state_vector     │
        │         │               │  │         │               │
        │         ↓               │  │         ↓               │
        │ [B has stone @ 0,0,0]   │  │ [C has stone @ 0,0,0]   │
        │                         │  │                         │
        └─────────────────────────┘  └─────────────────────────┘
             (convergence)                (convergence)
```

---

## Reference: Message Types

### StateUpdate (0x01) — Regular sync

```json
{
  "deltas": [
    {
      "sender": 1,
      "changes": [
        ["0,0,0", { "value": "stone", "timestamp": {"node_id": 1, "counter": 5} }]
      ]
    }
  ],
  "state_vector": {
    "1": 5,
    "2": 3,
    "3": 0
  }
}
```

### StateFull (0x02) — New peer or large reconnection gap

```json
{
  "entries": {
    "0,0,0": { "value": "stone", "timestamp": {"node_id": 1, "counter": 5} },
    "0,0,1": { "value": "dirt", "timestamp": {"node_id": 2, "counter": 3} }
  }
}
```

### SyncRequest (new) — Peer reconnection signal

```json
{
  "from_peer": 3,
  "state_vector": {
    "1": 2,
    "2": 1,
    "3": 10
  }
}
```

---

## Concrete Pseudocode: Full Sync Loop

```rust
// ============ SyncManager (new file: src/sync.rs) ============

use crate::lww_map::{LwwDelta, LwwMap};
use crate::p2p::mesh::MeshRoom;
use crate::wire;
use serde::{Serialize, Deserialize};
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPayload {
    pub deltas: Vec<LwwDelta<String, String>>,
    pub state_vector: HashMap<u32, u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullStatePayload {
    pub entries: HashMap<String, crate::lww_map::LwwEntry<String>>,
}

pub struct SyncManager {
    local_node_id: u32,
    delta_buffer: VecDeque<LwwDelta<String, String>>,
    state_vector: HashMap<u32, u32>,  // peer_id → max_counter_seen
    last_send_ms: u64,
    batch_interval_ms: u64,
}

impl SyncManager {
    pub fn new(node_id: u32) -> Self {
        Self {
            local_node_id: node_id,
            delta_buffer: VecDeque::new(),
            state_vector: HashMap::new(),
            last_send_ms: 0,
            batch_interval_ms: 20,  // 20ms batching interval
        }
    }

    /// Called immediately after place_block() or any CRDT mutation
    pub fn on_local_delta(&mut self, delta: LwwDelta<String, String>) {
        self.delta_buffer.push_back(delta);
    }

    /// Called from main update loop (16ms ticks)
    pub fn poll_and_send(&mut self, now_ms: u64, room: &MeshRoom) {
        if now_ms.saturating_sub(self.last_send_ms) >= self.batch_interval_ms {
            self.flush(room);
            self.last_send_ms = now_ms;
        }
    }

    fn flush(&mut self, room: &MeshRoom) {
        if self.delta_buffer.is_empty() {
            return;
        }

        let deltas: Vec<_> = self.delta_buffer.drain(..).collect();
        let payload = SyncPayload {
            deltas,
            state_vector: self.state_vector.clone(),
        };

        let json = serde_json::to_string(&payload).unwrap_or_default();
        if let Ok(encoded) = wire::encode(wire::MsgType::StateUpdate, json.as_bytes()) {
            let _ = room.broadcast(&encoded, crate::p2p::channel::ChannelMode::Reliable);
        }
    }

    /// Called when receiving StateUpdate from peer
    pub fn on_peer_delta(
        &mut self,
        from_peer: u32,
        payload: SyncPayload,
        world: &mut LwwMap<String, String>,
    ) {
        for delta in payload.deltas {
            world.merge_delta(&delta);
        }

        // Update state vector with peer's knowledge
        for (peer_id, counter) in payload.state_vector {
            self.state_vector.entry(peer_id).and_modify(|c| *c = (*c).max(counter))
                .or_insert(counter);
        }
    }

    /// Called when new peer joins or reconnection detected
    pub fn should_send_full_state(&self, peer_id: u32, peer_vector: &HashMap<u32, u32>) -> bool {
        let missing_deltas = self.state_vector.iter()
            .map(|(id, counter)| counter.saturating_sub(*peer_vector.get(id).unwrap_or(&0)))
            .sum::<u32>();
        
        missing_deltas > 1000
    }

    pub fn get_peer_state_vector(&self) -> HashMap<u32, u32> {
        self.state_vector.clone()
    }
}

// ============ Integration in wasm.rs ============

pub struct PlatformWasm {
    platform: PlatformCore,
    room: MeshRoom,
    sync_mgr: SyncManager,  // NEW
}

impl PlatformWasm {
    pub fn on_update(&mut self, now_ms: u64) {
        // ... user input handling ...
        
        // NEW: Poll sync manager every frame
        self.sync_mgr.poll_and_send(now_ms, &self.room);
    }

    pub fn on_place_block(&mut self, key: String, value: String) {
        // Execute CRDT operation
        let delta = self.platform.world.set(key, value);
        
        // NEW: Queue delta for network sync
        self.sync_mgr.on_local_delta(delta);
    }

    pub fn on_receive_sync(&mut self, payload: SyncPayload) {
        // NEW: Merge deltas from peer
        self.sync_mgr.on_peer_delta(0, payload, &mut self.platform.world);
    }
}
```

---

## Testing Strategy

### Unit Tests (Rust)
- [ ] SyncManager batching: 10 ops in 20ms = 1 batch
- [ ] State vector updates: merge payload with multiple peers
- [ ] Idempotency: apply same delta twice = apply once
- [ ] Convergence: 3 peers concurrent writes → same final state

### Integration Tests (Browser)
- [ ] Two browsers place blocks → both see same state
- [ ] Browser offline 10min → reconnect → catch up
- [ ] 100 concurrent ops → merge correctly
- [ ] Bandwidth: measure bytes sent (target <10KB/sec at 100 ops/sec)

### Example Unit Test

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batching_accumulates() {
        let mut mgr = SyncManager::new(1);
        
        // Simulate 10 place_block ops
        for i in 0..10 {
            let delta = LwwDelta { changes: vec![(
                format!("0,0,{}", i),
                crate::lww_map::LwwEntry {
                    value: Some("stone".to_string()),
                    timestamp: crate::lamport::Lamport::new(1),
                }
            )] };
            mgr.on_local_delta(delta);
        }
        
        // Verify buffered but not sent yet
        assert_eq!(mgr.delta_buffer.len(), 10);
        
        // Simulate 20ms elapsed
        // (would call flush in real code, but we verify buffer state)
    }

    #[test]
    fn convergence_3_peers() {
        let mut w1 = LwwMap::new(1);
        let mut w2 = LwwMap::new(2);
        let mut w3 = LwwMap::new(3);

        // P1 writes key1=v1
        let d1 = w1.set("key1".into(), "v1".into());
        // P2 writes key1=v2 (concurrent)
        let d2 = w2.set("key1".into(), "v2".into());
        // P3 writes key1=v3 (concurrent)
        let d3 = w3.set("key1".into(), "v3".into());

        // All peers merge all deltas (any order)
        w1.merge_delta(&d2);
        w1.merge_delta(&d3);
        
        w2.merge_delta(&d1);
        w2.merge_delta(&d3);
        
        w3.merge_delta(&d1);
        w3.merge_delta(&d2);

        // All must converge to same value (highest timestamp wins)
        assert_eq!(w1.get(&"key1".into()), w2.get(&"key1".into()));
        assert_eq!(w2.get(&"key1".into()), w3.get(&"key1".into()));
    }
}
```

---

## Implementation Roadmap

### Phase 1: Core SyncManager (1-2 days)
- [ ] Define SyncPayload, FullStatePayload types
- [ ] Implement SyncManager::on_local_delta, poll_and_send, flush
- [ ] Unit tests (batching, state vector)
- [ ] Integration in wasm.rs on_update()

### Phase 2: Peer Sync (1-2 days)
- [ ] on_peer_delta() implementation
- [ ] State vector merge logic
- [ ] Full state snapshot code path
- [ ] Integration tests (two browsers sync)

### Phase 3: Reconnection Handling (1 day)
- [ ] should_send_full_state() heuristic
- [ ] Delta log persistence (IndexedDB)
- [ ] Reconnection message flow
- [ ] Integration test: offline/online cycle

### Phase 4: Optimization (optional)
- [ ] Delta compression (bincode vs JSON)
- [ ] State vector bloom filters (for 1000+ peers)
- [ ] Epoch-based GC of old deltas
- [ ] Bandwidth profiling and tuning

---

## References

- **Yjs**: https://docs.yjs.dev/api/updates (state vector update protocol)
- **Automerge**: https://automerge.org/docs/cookbook/loading/ (snapshot + incremental)
- **CRDTs in Practice**: Shapiro et al., "A comprehensive study of CRDT"
- **LWW-Element-Set**: Conflict-free replicated data types (Shapiro et al., 2011)
- **HLC**: https://cse.buffalo.edu/tech-reports/2014-04.pdf (Hybrid Logical Clocks)

---

**KG Binding**: This document is bound to:
- **Codebase**: src/lww_map.rs, src/p2p/mesh.rs, src/wire.rs
- **Architecture**: INT_CrdtSync AtomicSpan
- **Progress**: APT 333 Platform, Phase 8 Integration
- **Status**: Ready for APT-ST → APT-SCW gate approval
