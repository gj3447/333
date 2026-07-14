# Manual E2E Debug Reference Card
> KG: TASK_333_E2E_ManualTest
> Quick console/UI checks during manual testing

---

## REAL-TIME CHECKS (Browser Console)

### P2P Connection Status
```javascript
// What to type/check
window.room?.status                    // "connected" = PASS
window.room?.peers?.size               // 1 = other peer connected
window.peerList?.length                // same as peers.size

// DataChannel state
window.room?.peerChannels  // Map<peerId, Map<chName, RTCDataChannel>>
// Look for:
//   DC "crdt" → readyState: "open" ✓
//   DC "bft"  → readyState: "open" ✓
```

### CRDT World State
```javascript
window.blocks              // Map<"x,y", blockType>
window.blocks?.size        // # blocks placed
window.consensusState?.worldSize  // should match blocks.size

// Check convergence:
// Tab A: console.log(Array.from(window.blocks.entries()))
// Tab B: console.log(Array.from(window.blocks.entries()))
// Compare → must be identical (same keys, same values)
```

### BFT Consensus
```javascript
window.consensusState      // {nodeId, view, isLeader, committedBlocks, worldSize, syncPending}

// Before block placement: view = N
// After block placed: view = N+1 (or N+2)
// committedBlocks incrementing = HotStuff working

// Check if leader elected:
window.consensusState?.isLeader     // true if this node is leader
```

### Message Logging
```javascript
// UI shows last 20 messages
window.messageLog          // Array<string>
// Each entry: "peerId[channel]: NNN bytes"

// Example healthy sequence:
// "WS: connected"
// "peer-YYYY joined"
// "DC: opened with peer-YYYY [crdt]"
// "DC: opened with peer-YYYY [bft]"
// "from-YYYY[crdt]: 128B"
```

---

## UI DEBUG PANEL (On-Page)

**Top-left corner** shows:
```
Room: abc123
Peers: 1/3
Status: connected

CRDT
worldSize: 42
syncPending: 0

BFT
view: 7
isLeader: true
committedBlocks: 3

Grid (8x8)
[visual block map]
```

---

## TIMELINE: What to See When

### After Tab B Opens (joining existing room)

**Tab A Console** (0-2s):
```
[room] peer-YYYY joined (2 peers)
DC: opened with peer-YYYY [crdt]
DC: opened with peer-YYYY [bft]
```

**Tab B Console** (0-2s):
```
WS: connected
peers: [peer-XXXX]  ← discovers existing peer
DC: opened with peer-XXXX [crdt]
DC: opened with peer-XXXX [bft]
```

**Both UIs** (3s):
- `status = connected`
- `Peers: 1` (the other one)
- Grid renders (empty initially)

---

### After Tab A Places Block "stone" at (3,4)

**Tab A** (immediate):
```
blocks: new entry "3,4" → "stone"
block colored gray at grid position
UI shows: worldSize: 1
```

**Tab B** (within 500ms):
```
from-XXXX[crdt]: 128B
blocks: new entry "3,4" → "stone" appears
UI shows: worldSize: 1
```

**Both consensusState** (within 2s):
```
view: 1 → 2  (incremented, HotStuff view change)
committedBlocks: 0 → 1
```

---

### After Tab B Closed + Reopened

**Tab A Console** (when B closes):
```
[room] peer-YYYY left (0 peers)
status: connecting (rebuilding)
```

**Tab B Reopened** (0-3s):
```
WS: connected
peers: [peer-XXXX]
```

**Tab A Console** (after B rejoins):
```
[room] peer-YYYY joined (1 peers) ← re-joined, reassigned same peerId
DC: opened with peer-YYYY [crdt]
```

**Tab B UI** (within 1s):
- All blocks from Tab A appear (snapshot transferred)
- `worldSize` matches Tab A
- `view` matches (caught up to current consensus)

---

## FAIL SIGNATURES (What to See if Broken)

| Symptom | Root Cause | Fix |
|---------|-----------|-----|
| Tab B status stays "connecting" | ICE timeout / firewall | Restart signaling server, check `nc -zv localhost 8333` |
| DC says "failed" | STUN server down | Check signaling console for STUN errors; fallback to GCP STUN |
| Block placed on Tab A, Tab B nothing | CRDT channel issue | Verify `DC crdt readyState = "open"` on both; check backpressure queue |
| view never increments | WASM not integrated | Check `wasm?.pollSync()` in frontend loop; may be stub function |
| worldSize mismatch Tab A/B | State divergence | Block placed but not replicated; check DataChannel receive handler |

---

## QUICK VALIDATION SCRIPT

Run in Tab A console after tests:

```javascript
function validate() {
  const checks = {
    roomId: window.roomId !== '',
    connected: window.room?.status === 'connected',
    dcOpen: Array.from(window.room?.peerChannels?.values() || [])
      .flatMap(m => Array.from(m.values()))
      .every(dc => dc.readyState === 'open'),
    blocksExist: window.blocks?.size > 0,
    worldSizeMatches: window.blocks?.size === window.consensusState?.worldSize,
    viewIncremented: window.consensusState?.view > 0,
  };
  console.table(checks);
  return Object.values(checks).every(v => v);
}
validate();
// Output: All true = PASS ✓
```

---

**Last updated**: 2026-04-13
