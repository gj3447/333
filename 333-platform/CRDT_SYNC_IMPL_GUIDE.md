# CRDT Sync Implementation Guide
## Ready-to-Code Companion to CRDT_SYNC_ARCHITECTURE.md

**KG: IMPL_333_CrdtSync**

---

## Quick Navigation

- **Architecture document**: CRDT_SYNC_ARCHITECTURE.md (read first)
- **This file**: Code patterns + integration points
- **Target**: `src/sync.rs` (new file) + modifications to `src/wasm.rs`, `src/platform.rs`

---

## File 1: New Module `src/sync.rs`

Copy this into a new file `src/sync.rs`:

```rust
// KG: MOD_333_SyncManager, CONTRACT_SyncManager
//! Delta-state sync loop for CRDT + WebRTC P2P
//! - Batches deltas every 20ms
//! - Maintains state vector for incremental sync
//! - Handles full state snapshots for reconnections

use crate::lww_map::{LwwDelta, LwwMap, LwwEntry};
use crate::p2p::mesh::MeshRoom;
use crate::p2p::channel::ChannelMode;
use crate::wire;
use serde::{Serialize, Deserialize};
use std::collections::{HashMap, VecDeque};

/// Payload sent in StateUpdate (MsgType::StateUpdate = 0x01)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPayload {
    /// Batch of deltas from local and remote peers
    pub deltas: Vec<LwwDelta<String, String>>,
    /// Piggybacked state vector: what we know peer has seen
    pub state_vector: HashMap<u32, u32>,
}

/// Payload sent in StateFull (MsgType::StateFull = 0x02)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullStatePayload {
    /// Entire map snapshot for new/reconnecting peer
    pub entries: HashMap<String, LwwEntry<String>>,
}

/// Manages delta batching and state vector for incremental sync
pub struct SyncManager {
    /// Local node ID (same as platform.node_id)
    local_node_id: u32,
    /// Buffer of local deltas pending transmission
    delta_buffer: VecDeque<LwwDelta<String, String>>,
    /// State vector: peer_id → max Lamport counter seen from that peer
    state_vector: HashMap<u32, u32>,
    /// Timestamp of last batch sent (milliseconds)
    last_send_ms: u64,
    /// Batch interval in milliseconds (default 20ms)
    batch_interval_ms: u64,
    /// Maximum deltas before force flush (backpressure)
    max_buffer_size: usize,
}

impl SyncManager {
    /// Create new sync manager
    pub fn new(node_id: u32) -> Self {
        Self {
            local_node_id: node_id,
            delta_buffer: VecDeque::new(),
            state_vector: HashMap::new(),
            last_send_ms: 0,
            batch_interval_ms: 20,  // 20ms batching
            max_buffer_size: 1000,  // Force flush if >1000 deltas
        }
    }

    /// Called immediately after place_block(), delete_block(), or any CRDT mutation
    /// Queues the delta for the next batch send
    pub fn on_local_delta(&mut self, delta: LwwDelta<String, String>) {
        self.delta_buffer.push_back(delta);

        // Backpressure: force flush if buffer grows too large
        if self.delta_buffer.len() >= self.max_buffer_size {
            // In real code, would call flush_now() if we had reference to room
            // For now, caller responsible for checking
        }
    }

    /// Called from main update loop (every ~16ms)
    /// Flushes batch if 20ms has elapsed since last send
    pub fn poll_and_send(&mut self, now_ms: u64, room: &MeshRoom) {
        let elapsed = now_ms.saturating_sub(self.last_send_ms);
        if elapsed >= self.batch_interval_ms {
            self.flush(room);
            self.last_send_ms = now_ms;
        }
    }

    /// Encode and broadcast all buffered deltas
    fn flush(&mut self, room: &MeshRoom) {
        if self.delta_buffer.is_empty() {
            return;  // Nothing to send
        }

        let deltas: Vec<_> = self.delta_buffer.drain(..).collect();
        let payload = SyncPayload {
            deltas,
            state_vector: self.state_vector.clone(),
        };

        // Serialize to JSON
        let json = match serde_json::to_string(&payload) {
            Ok(j) => j,
            Err(_) => return,  // Serialization failed, skip
        };

        // Encode with wire protocol
        let encoded = match wire::encode(wire::MsgType::StateUpdate, json.as_bytes()) {
            Ok(e) => e,
            Err(_) => return,  // Encoding failed, skip
        };

        // Broadcast to all peers (reliable channel, maintains FIFO order)
        let _ = room.broadcast(&encoded, ChannelMode::Reliable);
    }

    /// Called when receiving StateUpdate from a peer
    /// Merges deltas and updates state vector
    pub fn on_peer_sync(
        &mut self,
        from_peer: u32,
        payload: SyncPayload,
        world: &mut LwwMap<String, String>,
    ) {
        // 1. Apply all received deltas to world state
        for delta in payload.deltas {
            world.merge_delta(&delta);
        }

        // 2. Update state vector with what peer told us it has seen
        // This "piggybacking" allows peers to exchange knowledge without explicit ACKs
        for (peer_id, counter) in payload.state_vector {
            self.state_vector
                .entry(peer_id)
                .and_modify(|c| *c = (*c).max(counter))
                .or_insert(counter);
        }
    }

    /// Called when receiving StateFull from a peer
    /// Replaces entire local state with snapshot (used after reconnection or join)
    pub fn on_peer_full_state(
        &mut self,
        payload: FullStatePayload,
        world: &mut LwwMap<String, String>,
    ) {
        // Reconstruct state vector from snapshot
        let mut new_vector = HashMap::new();
        for (_, entry) in &payload.entries {
            let node_id = entry.timestamp.node_id;
            let counter = entry.timestamp.counter;
            new_vector
                .entry(node_id)
                .and_modify(|c| *c = (*c).max(counter))
                .or_insert(counter);
        }

        // Replace world state with snapshot
        *world = LwwMap::from_snapshot(payload.entries);

        // Update our state vector to reflect the snapshot
        self.state_vector = new_vector;
    }

    /// Determine if we should send full state to a reconnecting peer
    /// Returns true if gap is large (>1000 missing deltas)
    pub fn should_send_full_state(&self, peer_vector: &HashMap<u32, u32>) -> bool {
        let missing_count: u32 = self.state_vector
            .iter()
            .map(|(node_id, counter)| {
                let peer_counter = peer_vector.get(node_id).copied().unwrap_or(0);
                counter.saturating_sub(peer_counter)
            })
            .sum();

        missing_count > 1000
    }

    /// Get current state vector snapshot
    pub fn get_state_vector(&self) -> HashMap<u32, u32> {
        self.state_vector.clone()
    }

    /// For testing: check buffer size
    pub fn buffer_len(&self) -> usize {
        self.delta_buffer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sync_manager() {
        let mgr = SyncManager::new(1);
        assert_eq!(mgr.local_node_id, 1);
        assert_eq!(mgr.buffer_len(), 0);
        assert!(mgr.get_state_vector().is_empty());
    }

    #[test]
    fn on_local_delta_queues() {
        let mut mgr = SyncManager::new(1);
        let delta = LwwDelta {
            changes: vec![(
                "key1".to_string(),
                LwwEntry {
                    value: Some("val1".to_string()),
                    timestamp: crate::lamport::Lamport::new(1),
                },
            )],
        };

        mgr.on_local_delta(delta);
        assert_eq!(mgr.buffer_len(), 1);
    }

    #[test]
    fn should_send_full_state_large_gap() {
        let mut mgr = SyncManager::new(1);
        
        // Simulate large state vector from self
        for i in 1..=100 {
            mgr.state_vector.insert(i, i * 50);
        }

        // Peer only has seen 10% of our knowledge
        let peer_vector: HashMap<u32, u32> = (1..=100)
            .map(|i| (i, i * 5))
            .collect();

        // Should recommend full state
        assert!(mgr.should_send_full_state(&peer_vector));
    }

    #[test]
    fn should_send_incremental_small_gap() {
        let mut mgr = SyncManager::new(1);
        
        mgr.state_vector.insert(1, 100);
        mgr.state_vector.insert(2, 200);

        // Peer only missing ~50 deltas total
        let peer_vector = vec![(1, 90), (2, 180)].into_iter().collect();

        assert!(!mgr.should_send_full_state(&peer_vector));
    }
}
```

---

## File 2: Modifications to `src/lib.rs`

Add sync module to lib:

```rust
// In src/lib.rs, add to module list:
pub mod sync;
```

---

## File 3: Modifications to `src/wasm.rs`

Integration point for the sync loop:

```rust
// KG: IMPL_333_WasmIntegration

use crate::sync::{SyncManager, SyncPayload, FullStatePayload};
use crate::platform::PlatformCore;
use crate::p2p::mesh::MeshRoom;

pub struct PlatformWasm {
    platform: PlatformCore,
    room: MeshRoom,
    sync_mgr: SyncManager,  // NEW FIELD
    last_update_ms: u64,
}

impl PlatformWasm {
    pub fn new(node_id: u32) -> Self {
        Self {
            platform: PlatformCore::new(node_id, &[1, 2, 3]),  // Example validators
            room: MeshRoom::new(node_id, Default::default()),
            sync_mgr: SyncManager::new(node_id),  // NEW
            last_update_ms: 0,
        }
    }

    /// Main update loop (called every frame from browser)
    pub fn on_update(&mut self, now_ms: u64) {
        // 1. Handle user input, update platform state, etc.
        // ... (existing code) ...

        // 2. NEW: Poll and send any batched deltas
        self.sync_mgr.poll_and_send(now_ms, &self.room);

        // 3. Poll room for incoming messages
        let events = self.room.poll();
        for event in events {
            self.handle_mesh_event(event);
        }

        self.last_update_ms = now_ms;
    }

    /// Called when user places a block
    pub fn place_block(&mut self, pos: String, block_type: String) {
        // 1. Execute CRDT operation
        let delta = self.platform.world.set(pos.clone(), block_type);

        // 2. NEW: Queue delta for network sync
        self.sync_mgr.on_local_delta(delta);

        // 3. Persist to local storage
        // ... (existing code) ...
    }

    /// Called when user deletes a block
    pub fn delete_block(&mut self, pos: String) {
        // 1. Execute CRDT delete
        let delta = self.platform.world.delete(pos.clone());

        // 2. NEW: Queue delta for network sync
        self.sync_mgr.on_local_delta(delta);
    }

    /// Handle incoming mesh event (peer message, join, leave, etc.)
    fn handle_mesh_event(&mut self, event: crate::p2p::mesh::MeshEvent) {
        use crate::p2p::mesh::MeshEvent;

        match event {
            MeshEvent::PeerJoined(peer_id) => {
                // NEW: Send full state to new peer
                self.send_full_state_to(peer_id);
            }
            MeshEvent::MessageReceived { from: _peer_id, data } => {
                // Decode and route message
                self.on_receive_message(&data);
            }
            MeshEvent::PeerLeft(peer_id) => {
                // Handle peer leaving
                log::info!("Peer {} left", peer_id);
            }
            _ => {}
        }
    }

    /// Receive and decode incoming sync message
    fn on_receive_message(&mut self, data: &[u8]) {
        // Decode wire frame
        let decoded = match crate::wire::decode(data) {
            crate::wire::DecodeResult::Ok(msg) => msg,
            crate::wire::DecodeResult::SkipVersion(_) => return,
            crate::wire::DecodeResult::SkipType(_) => return,
            crate::wire::DecodeResult::Err(_) => return,
        };

        // Route by message type
        match crate::wire::MsgType::from_u8(decoded.header.msg_type) {
            Some(crate::wire::MsgType::StateUpdate) => {
                // NEW: Handle incremental sync
                self.on_state_update(&decoded.payload);
            }
            Some(crate::wire::MsgType::StateFull) => {
                // NEW: Handle full state snapshot
                self.on_full_state(&decoded.payload);
            }
            _ => {
                // Other message types (Consensus, Presence, etc.)
            }
        }
    }

    /// Handle StateUpdate message (incremental sync)
    fn on_state_update(&mut self, payload_bytes: &[u8]) {
        let payload: SyncPayload = match serde_json::from_slice(payload_bytes) {
            Ok(p) => p,
            Err(_) => return,
        };

        // NEW: Merge deltas and update state vector
        self.sync_mgr.on_peer_sync(0, payload, &mut self.platform.world);
    }

    /// Handle StateFull message (full state snapshot)
    fn on_full_state(&mut self, payload_bytes: &[u8]) {
        let payload: FullStatePayload = match serde_json::from_slice(payload_bytes) {
            Ok(p) => p,
            Err(_) => return,
        };

        // NEW: Replace entire state with snapshot
        self.sync_mgr.on_peer_full_state(payload, &mut self.platform.world);
    }

    /// Send full state to peer (used when peer joins or reconnects with large gap)
    fn send_full_state_to(&self, peer_id: u32) {
        let entries = self.platform.world.snapshot().clone();
        let payload = FullStatePayload { entries };

        let json = serde_json::to_string(&payload).unwrap_or_default();
        if let Ok(encoded) = crate::wire::encode(crate::wire::MsgType::StateFull, json.as_bytes()) {
            let _ = self.room.send_to(peer_id, &encoded, crate::p2p::channel::ChannelMode::Reliable);
        }
    }
}
```

---

## File 4: Required Changes to `src/lww_map.rs`

Add `from_snapshot` method:

```rust
// In src/lww_map.rs, add to impl LwwMap:

impl<K, V> LwwMap<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    // ... existing methods ...

    /// Create a new map from a snapshot (used after receiving StateFull)
    pub fn from_snapshot(entries: HashMap<K, LwwEntry<V>>) -> Self {
        let mut map = Self::new(0);  // node_id=0 for reconstructed state
        map.entries = entries;
        map
    }

    /// Export state as HashMap for snapshots
    pub fn snapshot(&self) -> &HashMap<K, LwwEntry<V>> {
        &self.entries
    }
}
```

---

## Integration Checklist

- [ ] Create `src/sync.rs` with SyncManager
- [ ] Add `pub mod sync;` to `src/lib.rs`
- [ ] Modify `src/wasm.rs`:
  - [ ] Add `sync_mgr: SyncManager` field
  - [ ] Call `sync_mgr.poll_and_send()` in `on_update()`
  - [ ] Call `sync_mgr.on_local_delta()` in `place_block()` and `delete_block()`
  - [ ] Implement `handle_mesh_event()` → `send_full_state_to()`
  - [ ] Implement `on_receive_message()` with StateUpdate/StateFull routing
- [ ] Modify `src/lww_map.rs`:
  - [ ] Add `from_snapshot()` constructor
  - [ ] Expose `snapshot()` method
- [ ] Add tests:
  - [ ] `src/sync.rs` unit tests (already provided)
  - [ ] Integration test: two PlatformWasm instances, place block on one, verify on other
- [ ] Update WASM bindings (wasm.rs exports if needed for JS integration)

---

## Browser Integration (TypeScript/JavaScript)

Once Rust code is compiled to WASM:

```typescript
// 333-app/src/lib/wasm-client.ts
import type { WasmModule } from '../wasm/platform333';

export class Collaborator {
    wasm: WasmModule;
    
    async init() {
        const wasm = await import('../wasm/platform333.js');
        this.wasm = wasm;
    }
    
    placeBlock(x: number, y: number, z: number, blockType: string) {
        const key = `${x},${y},${z}`;
        this.wasm.place_block(key, blockType);
        // WASM internally:
        // 1. Generate LwwDelta
        // 2. sync_mgr.on_local_delta(delta)
        // 3. Delta buffered, sent in next 20ms batch
    }
    
    update(nowMs: number) {
        this.wasm.on_update(nowMs);
        // WASM internally:
        // 1. sync_mgr.poll_and_send() if 20ms elapsed
        // 2. Broadcasts batch to all peers via WebRTC DC
    }
    
    // Browser's WebRTC DataChannel event handler
    onDataChannelMessage(event: MessageEvent) {
        const bytes = new Uint8Array(event.data);
        this.wasm.on_receive_message(bytes);
        // WASM internally:
        // 1. wire::decode(bytes)
        // 2. Route to on_state_update() or on_full_state()
        // 3. Merge deltas into world state
        // 4. UI renders updated blocks
    }
}
```

---

## Testing: Example Integration Test

```rust
// tests/integration_sync.rs
#[test]
fn two_peers_sync_blocks() {
    use 333_platform::sync::{SyncManager, SyncPayload};
    use 333_platform::lww_map::LwwMap;
    use 333_platform::p2p::channel::{InMemoryChannel, DataChannel};
    use 333_platform::p2p::mesh::MeshRoom;

    // Create two peers with in-memory channels
    let (dc_ab, dc_ba) = InMemoryChannel::create_pair(1, 2);
    
    let mut room_a = MeshRoom::new(1, Default::default());
    room_a.add_peer(2, Box::new(dc_ab), 0);
    
    let mut room_b = MeshRoom::new(2, Default::default());
    room_b.add_peer(1, Box::new(dc_ba), 0);

    let mut world_a: LwwMap<String, String> = LwwMap::new(1);
    let mut world_b: LwwMap<String, String> = LwwMap::new(2);
    
    let mut sync_a = SyncManager::new(1);
    let mut sync_b = SyncManager::new(2);

    // Peer A places block
    let delta = world_a.set("0,0,0".into(), "stone".into());
    sync_a.on_local_delta(delta);

    // Peer A flushes batch
    sync_a.poll_and_send(20, &room_a);

    // Peer B receives message
    let events_b = room_b.poll();
    assert!(!events_b.is_empty());

    // (In real code, would decode wire message and call on_state_update)
    // For now, verify structure:
    for event in events_b {
        match event {
            333_platform::p2p::mesh::MeshEvent::MessageReceived { data, .. } => {
                // data contains encoded StateUpdate
                // Would decode and merge in real integration
                assert!(!data.is_empty());
            }
            _ => {}
        }
    }
}
```

---

## Debugging Tips

### 1. Log Batch Sends

Add to `SyncManager::flush()`:

```rust
fn flush(&mut self, room: &MeshRoom) {
    let batch_size = self.delta_buffer.len();
    if batch_size > 0 {
        web_sys::console::log_1(&format!("SYNC: flushing {} deltas", batch_size).into());
    }
    // ... rest of flush ...
}
```

### 2. Log State Vector Updates

Add to `SyncManager::on_peer_sync()`:

```rust
pub fn on_peer_sync(...) {
    // ... merging ...
    web_sys::console::log_1(&format!("SYNC: state_vector now: {:?}", self.state_vector).into());
}
```

### 3. Verify Convergence

Browser console:

```javascript
// After placing blocks on peer A
const stateA = wasm.get_world_state();  // Requires export
console.log("Peer A state:", stateA);

// Wait for sync
setTimeout(() => {
    const stateB = wasm.get_world_state();  // In peer B's WASM instance
    console.log("Peer B state:", stateB);
    console.assert(JSON.stringify(stateA) === JSON.stringify(stateB), "States diverged!");
}, 100);
```

---

## Performance Targets

- **Batching overhead**: <5% (20ms batch on 1000+ ops/sec is negligible)
- **Merge latency**: <10ms per batch (1000 deltas)
- **Bandwidth**: <100 bytes/op with batching (vs 1KB immediate)
- **Memory**: <10MB for 100K blocks + metadata

---

## Next Steps

1. Implement `src/sync.rs` from code above
2. Integrate into `src/wasm.rs` with the checklist
3. Run `cargo test` to verify Rust unit tests pass
4. Build WASM: `wasm-pack build --target web`
5. Test in browser: open `/333/wasm/p2p-demo.html`
6. Verify two peers → place block on A → see on B within 20ms

---

**KG Binding**: This guide is linked to:
- Architecture: CRDT_SYNC_ARCHITECTURE.md
- Code files: src/sync.rs (new), src/wasm.rs, src/lww_map.rs, src/lib.rs
- Task: INT_CrdtSync (AtomicSpan from APT SP)
- Status: Ready to implement (SCW phase after Taliban approval)
