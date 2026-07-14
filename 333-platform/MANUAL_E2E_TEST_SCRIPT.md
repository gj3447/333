# Manual E2E Test Script — 333 Platform P2P + CRDT + BFT
> KG: TASK_333_E2E_ManualTest, CONTRACT_333_E2E_Validation
> Minimum viable manual test: 30 minutes, 2 browser tabs, 2 terminals

---

## SETUP (5 min)

**Terminal A: Signaling Server**
```bash
cd /Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform
node signaling/server.mjs 8333
# PASS: "333 Signaling Server on ws://localhost:8333"
```

**Terminal B: Frontend**
```bash
cd 333-app
npm install  # if needed
npm run dev
# PASS: "Local: http://localhost:5173"
```

---

## TEST SUITE

### TEST 1: Room Creation & P2P Connection (5 min)
**Tab A:**
1. Open `http://localhost:5173/room`
2. Click "Create Room" → Copy room ID (e.g., `abc123`)
3. Console should show: `[room] peer-XXXX joined (1 peers)`
4. **PASS**: `status === 'connected'`, `myId` visible

**Tab B:**
1. Open `http://localhost:5173/room?id=abc123`
2. Console shows: `[room] peer-YYYY joined (2 peers)`
3. **PASS**: Both tabs show `Peers connected: 1`, identical room ID

---

### TEST 2: CRDT Sync — Block Placement (10 min)
1. Tab A: Click any grid cell → place "stone" block
2. **Console check**: Tab A logs `Block placed: x,y | worldSize++`
3. **UI check**: Tab A shows block colored gray
4. Tab B: Within 500ms, same block appears gray in same cell
5. **Console check**: Tab B logs `received NNN bytes | block sync`
6. **PASS**: `blocks` Map identical on both tabs, `worldSize` matches

**Variation**: Toggle block (click again) → disappears on both tabs in <500ms

---

### TEST 3: BFT Consensus State (5 min)
*(Requires WASM integration complete)*

1. Tab A + B both running
2. Check console: `consensusState = {nodeId, view, isLeader, committedBlocks, worldSize, syncPending}`
3. Place block on Tab A
4. Check Tab B's `view` number after 2 seconds
5. **PASS**: `view` increments ≥1 after each block (HotStuff view change), `committedBlocks` increases

---

### TEST 4: Failure & Reconnection (5 min)
1. Tab A + B connected, both showing blocks
2. Tab B: Close tab
3. Tab A console: `[room] peer-YYYY left (0 peers)` + status changes to `connecting`
4. Tab B: Reopen to `http://localhost:5173/room?id=abc123`
5. Tab A console: `peer-joined YYYY` again
6. **PASS**: Tab A receives full state snapshot from Tab B, all blocks appear, `worldSize` restored

---

## VALIDATION CHECKLIST

| Check | Location | Expected |
|-------|----------|----------|
| Signaling alive | Terminal A | ws://localhost:8333 listening |
| App loaded | Tab A/B | No red errors, console clean |
| P2P connected | Tab A/B console | "DC: opened with peer-" |
| Block sync <1s | Tab B after action | Block appears before 500ms |
| View increment | Tab B `consensusState` | `view > 0` after block placement |
| Recon state recovery | Tab B after rejoin | `worldSize` equals Tab A |

---

## FAIL CRITERIA

- Block doesn't appear within 1 second → **CRDT sync broken**
- Tab A doesn't show peer left → **Connection tracking broken**
- Console red errors → **Code exception**
- `view` doesn't increment → **BFT not integrated** (mark TODO)

---

## CONSOLE DEBUG SHORTCUTS

```javascript
// In browser console Tab A:
console.log(window.room?.status)          // "connected" ✓
console.log(window.blocks)                 // Map size = # blocks placed
console.log(window.consensusState?.view)   // BFT view number
```

---

**Estimated time**: 25–30 minutes  
**Last updated**: 2026-04-13  
**Status**: Ready to run
