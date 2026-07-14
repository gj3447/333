# P2P Testing Quick Start

> Get running with manual tests in 5 minutes

## Prerequisites

```bash
# Node.js 18+
node --version

# Check repo is cloned
cd /Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform
ls signaling/ 333-app/ src/
```

## Step 1: Start Services (3 terminals)

**Terminal A: Signaling Server**
```bash
cd /Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform
node signaling/server.mjs 8333
# Output: "333 Signaling Server on ws://localhost:8333"
```

**Terminal B: Frontend App**
```bash
cd /Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform/333-app
npm install  # if needed
npm run dev
# Output: "Local: http://localhost:5173"
```

**Terminal C: (Reserve for testing)**
```bash
# Ready to run tests
```

## Step 2: Manual Test (2 Browser Tabs)

**Tab 1: Create Room**
```
1. Go to: http://localhost:5173/room
2. Click "Create Room"
3. Note room ID (e.g., "abc123")
4. Open DevTools (F12) → Console
5. Look for: "[room] peer-XXXX joined (1 peers)"
```

**Tab 2: Join Room**
```
1. Go to: http://localhost:5173/room?id=abc123
2. Both tabs should show: "Peers connected: 1"
3. Console in Tab 2: "[room] peer-YYYY joined (2 peers)"
4. Console in Tab 1: "peer-joined YYYY"
```

**Exchange Messages**
```
1. Tab 1: Click a block in the grid
2. Watch Tab 2: Block should appear within 1 second
3. Console: "received NN bytes from peer-..."
4. ✓ CRDT replication works!
```

## Step 3: Automated Smoke Test

```bash
# Terminal C: Install test dependencies
cd /Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform
npm install --save-dev puppeteer-core

# Run smoke test (auto opens 2 headless tabs, tests, closes)
node tests/p2p-smoke.js ws://localhost:8333

# Expected: "=== ALL SMOKE TESTS PASSED ===" at the end
```

## Step 4: Docker Test (Optional)

```bash
# All-in-one: Signaling + App + Test in containers
docker-compose -f docker-compose.test.yml up

# Logs will show:
# - signaling_1 starting
# - app_1 starting
# - test_1 waiting for services
# - test_1 running p2p-smoke.js
# - test_1 PASSED
```

---

## Common Issues

**"WebSocket: connection refused"**
- [ ] Is signaling server running? Check Terminal A for: "ws://localhost:8333"
- [ ] Try: `nc -zv localhost 8333` (should say "succeeded")

**"Peers don't connect (tabs both show 0 peers)"**
- [ ] Both tabs open? Check both console for "WS: connected"
- [ ] Wait 3 seconds (ICE gathering)
- [ ] Refresh tab 2

**"Block doesn't appear on other tab"**
- [ ] DataChannel open? Check console: "DC: opened with peer-..."
- [ ] No errors? Look for red messages in console
- [ ] Try: Place block on Tab 1, then manually refresh Tab 2

**"puppeteer-core: not found"**
- [ ] Run: `npm install --save-dev puppeteer-core`
- [ ] Make sure you're in the 333-platform root, not 333-app

---

## Next Steps

1. **Add Debug UI** → See console indicators visibly on the page
   - Follow Phase 2 in `P2P_TESTING_STRATEGY.md`

2. **Add Console Logging** → Track WebRTC events
   - Follow Phase 3 in `P2P_TESTING_STRATEGY.md`

3. **Scale to 3+ Peers** → Test full BFT consensus
   - Open 3rd tab, verify all converge to same state

4. **Network Failure Test** → Simulate offline, reconnect
   - DevTools → Network → Offline, then Online

---

## Docs

- **Full Guide**: `P2P_TESTING_STRATEGY.md`
- **Checklist**: `TESTING_CHECKLIST.md`
- **Signaling**: `signaling/server.mjs` (stateless relay)
- **Frontend**: `333-app/src/lib/room-state.ts` (RTCPeerConnection)

---

**Last updated: 2026-04-13**
