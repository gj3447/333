# 333 Platform: P2P Testing Checklist

> Quick reference for manual and automated testing of WebRTC + CRDT + BFT

## Pre-Test Setup

```bash
# Terminal 1: Start signaling server
cd /Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform
node signaling/server.mjs 8333

# Terminal 2: Start app (dev server)
cd 333-app
npm run dev

# Browser: Open http://localhost:5173/room
# DevTools: F12 → Console tab (for WebRTC logs)
```

---

## Manual Testing (Phase 1)

### Connection Tests

**Create Room (Tab 1)**
- [ ] Open http://localhost:5173/room
- [ ] Click "Create Room" button
- [ ] Note room ID (e.g., "abc123")
- [ ] Console shows: `[room] peer-XXXX joined (1 peers)`
- [ ] Wait 2 seconds
- [ ] Console shows: `ICE: local candidate for peer-2...`
- [ ] Page shows: "Peers connected: 0" (before Tab 2 joins)

**Join Room (Tab 2)**
- [ ] Open new tab: http://localhost:5173/room?id=abc123
- [ ] Page auto-loads room
- [ ] Both tabs show: "Peers connected: 1"
- [ ] Console logs should show ICE candidates
- [ ] WebRTC connection state: "connected"

### DataChannel Tests

**Message Exchange**
- [ ] Tab 1: Click a block in the grid
- [ ] Tab 1 console: `DC: sent NNN bytes to 1 peers`
- [ ] Tab 2 console: `DC: received NNN bytes from peer-...`
- [ ] Tab 2: Same block appears within 500ms
- [ ] No console errors (red messages)

**Concurrent Writes (CRDT)**
- [ ] Tab 1: Place stone block at (2,3)
- [ ] Tab 2: Place grass block at (4,5) simultaneously
- [ ] Wait 500ms
- [ ] Both tabs show: both blocks placed
- [ ] Both show same grid state (convergence)

**Deletion (Tombstones)**
- [ ] Tab 1: Click to delete a placed block
- [ ] Tab 2: Block disappears within 500ms
- [ ] No "undefined" blocks appear

### Failure Scenarios

**Offline Simulation**
- [ ] Tab 1: Open DevTools Network tab
- [ ] Throttle to "Offline"
- [ ] Try to place block on Tab 1
- [ ] Tab 1 console: `DC: sent failed (channel not open)`
- [ ] Switch Network back to Online
- [ ] Throttle to Normal
- [ ] Wait 3 seconds
- [ ] Connection re-establishes automatically
- [ ] New blocks sync again

**Peer Dropout**
- [ ] Both tabs connected (show 1 peer each)
- [ ] Close Tab 2 browser tab
- [ ] Tab 1 console: `DC: closed for peer-...`
- [ ] Tab 1 shows: "Peers connected: 0"
- [ ] No errors in console

---

## Automated Testing (Phase 2-3)

### Smoke Test (puppeteer-core)

```bash
cd /Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform
npm install --save-dev puppeteer-core

# Terminal 3: Run smoke test
node tests/p2p-smoke.js ws://localhost:8333

# Expected output:
# [SMOKE] Starting P2P smoke test...
# [SMOKE] >>> Opening Tab 1 (creator)...
# [SMOKE] Tab 1 created room: abc123
# [SMOKE] >>> Opening Tab 2 (joiner)...
# [SMOKE] >>> Waiting for peer connection...
# [SMOKE] Tab 1 detected 1 peer(s) ✓
# [SMOKE] >>> Testing CRDT message exchange...
# [SMOKE] ✓ Block replicated to Tab 2 (CRDT working)
# [SMOKE] === ALL SMOKE TESTS PASSED ===
```

### Docker Compose Test

```bash
cd /Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform

# Run full stack in containers (signaling + app + test)
docker-compose -f docker-compose.test.yml --profile test up --abort-on-container-exit

# Or run without test (just signaling + app):
docker-compose -f docker-compose.test.yml up

# Then manually test in browser: http://localhost:5173/room
```

### GitHub Actions (CI)

```bash
# Automatically runs on:
# - Push to main or develop
# - Pull requests to main or develop
# - Manual trigger (Workflow Dispatch)

# View logs at:
# https://github.com/YOUR_ORG/333-platform/actions/workflows/p2p-test.yml
```

---

## Debug Indicators Checklist

Look for these on the room page (requires Phase 2 UI enhancements):

- [ ] **Signaling WS**: GREEN (connected) | YELLOW (connecting) | RED (disconnected)
- [ ] **Peers Connected**: Number shown (0, 1, 2, 3+)
- [ ] **Per-Peer Status**: Each peer shows "DC: OPEN" (blue badge)
- [ ] **Connection Age**: Time since peer connected (e.g., "2s ago")
- [ ] **Last CRDT Delta**: "X changes in Yms" or "waiting for peer"
- [ ] **BFT Consensus**: Round number and vote count
- [ ] **Message Rate**: msgs/sec (should be < 100 for normal grid clicks)

---

## Console Logging Reference

### WebSocket Events

```javascript
[12:34:56.789] [abc123] WS: connected
[12:34:57.012] [abc123] WS: received peer list { peers: [], you: 'peer-1234567890' }
[12:34:57.234] [abc123] WS: closed, status=connected
```

### ICE Candidate Gathering

```javascript
[12:34:57.234] [abc123] ICE: local candidate for peer-2 srflx 1.2.3.4:51234 typ srflx
[12:34:57.456] [abc123] ICE: gathering complete for peer-2
```

### DataChannel State

```javascript
[12:34:58.456] [abc123] DC: opened with peer-2
[12:34:59.789] [abc123] DC: sent 127 bytes to 1 peers
[12:35:00.012] [abc123] DC: received 127 bytes from peer-2
[12:35:00.234] [abc123] DC: closed for peer-2
```

### RTCPeerConnection State

```javascript
[12:34:58.012] [abc123] PEER [peer-2]: connection state = connecting
[12:34:58.234] [abc123] PEER [peer-2]: connection state = connected
```

---

## Common Issues & Fixes

| Issue | Cause | Fix |
|-------|-------|-----|
| "Failed to create offer" | STUN server unreachable on localhost | Disable ICE for localhost (use empty ICE list) |
| Peers don't connect (timeout) | WebSocket port 8333 blocked/firewall | Check: `nc -zv localhost 8333` should return success |
| DataChannel doesn't open | SDP negotiation failed | Check browser console for "addIceCandidate" errors |
| Block doesn't replicate | Delta message too large | Check message size in console logs (should be < 65KB) |
| High latency (>1s) | Network throttling or packet loss | Check Network tab in DevTools for throttling |
| Memory leak (growing) | DataChannels not closing | Manually close Tab 2, check Tab 1 console for "DC: closed" |

---

## Test Coverage Matrix

### Must-Pass Manual Tests (Before Deploy)

```
Signaling + Peer Discovery:
  ✓ WS connects
  ✓ Peer list received
  ✓ 2+ peers see each other

WebRTC + DataChannel:
  ✓ ICE candidates gathered
  ✓ SDP exchange succeeds
  ✓ DataChannel opens
  ✓ Data flows bidirectionally

CRDT Replication:
  ✓ Single write: Tab 1 → Tab 2 (< 1s)
  ✓ Concurrent writes: both converge
  ✓ Deletion: tombstones sync

BFT Consensus:
  ✓ Voting round completes
  ✓ Consensus reaches majority
  ✓ State committed
```

### Optional Load Tests

```
Scale (3+ peers):
  ✓ 3 tabs in same room
  ✓ All see each other (3 bidirectional pairs)
  ✓ State converges with 3+ sources

Performance:
  ✓ 100 blocks placed in 10s
  ✓ Latency p99 < 500ms
  ✓ Memory < 300MB per tab

Resilience:
  ✓ Peer dropout handled
  ✓ Reconnection works
  ✓ No data loss
```

---

## Quick Commands

```bash
# Start all services (manual testing)
(cd signaling && node server.mjs 8333) & \
(cd 333-app && npm run dev) & \
echo "Services running. Open: http://localhost:5173/room"

# Run smoke test
node tests/p2p-smoke.js ws://localhost:8333

# Run Docker test stack
docker-compose -f docker-compose.test.yml up

# Kill all Node processes
killall node

# Check if port 8333 is in use
lsof -i :8333

# View recent console logs (bash history)
history | grep "p2p-smoke"
```

---

## Test Results Template

Copy this to document test runs:

```markdown
## Test Run: [DATE] [TESTER]

### Setup
- Node version: [run: node --version]
- App at: [http://localhost:5173/room]
- Signaling at: [ws://localhost:8333]
- Network: [localhost | 192.168.x.y | VPN]

### Manual Tests
- [ ] Connection: PASS / FAIL / N/A
- [ ] DataChannel: PASS / FAIL / N/A
- [ ] CRDT Sync: PASS / FAIL / N/A
- [ ] Failures: PASS / FAIL / N/A

### Automated Test
- Smoke test: PASS / FAIL
- Errors: [none | list]

### Notes
[Any anomalies, timing issues, etc.]

### Verdict
✓ READY TO MERGE / ✗ NEEDS FIXES
```

---

## References

- **P2P Testing Strategy**: `P2P_TESTING_STRATEGY.md` (comprehensive guide)
- **Signaling Server**: `signaling/server.mjs` (WebSocket relay)
- **Frontend State**: `333-app/src/lib/room-state.ts` (RTCPeerConnection setup)
- **Room Page**: `333-app/src/routes/room/+page.svelte` (UI + demo app)
- **Smoke Test**: `tests/p2p-smoke.js` (automated puppeteer test)
- **CI Workflow**: `.github/workflows/p2p-test.yml` (GitHub Actions)
- **Local CI**: `docker-compose.test.yml` (Docker-based testing)

---

*Last updated: 2026-04-13 | KG: TEST_333_P2P_Strategy*
