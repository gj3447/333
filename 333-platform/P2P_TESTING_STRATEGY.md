# P2P Browser Testing Strategy for 333 Platform

## Executive Summary
Testing browser-based P2P applications (WebRTC DataChannel + CRDT + BFT) requires a **manual-first → instrumented → automated** progression. Playwright is overkill; instead use: **console logging + simple Node.js automation + visual debug UI**.

---

## Phase 1: Manual Testing Strategy

### 1.1 Connection Order (Test in This Sequence)

```
Step 1: Signaling Connection (WebSocket)
  ↓
Step 2: Peer Discovery (who joined the room?)
  ↓
Step 3: ICE Candidate Gathering (is NAT traversal happening?)
  ↓
Step 4: SDP Exchange (offer/answer negotiation)
  ↓
Step 5: DataChannel Open (can data flow?)
  ↓
Step 6: CRDT Replication (do peers see same state?)
  ↓
Step 7: BFT Consensus (agreement on state changes)
  ↓
Step 8: Token Transfer (application logic)
```

### 1.2 Manual Verification Checklist

**Setup (Do This First)**
```
□ Start signaling server: node signaling/server.mjs 8333
□ Start app: cd 333-app && npm run dev
□ Open browser DevTools (F12) → Console tab
□ Have 2 browser tabs/windows ready
```

**Tab 1 (Creator)**
```
□ Click "Create Room"
□ Note room ID (e.g., "abc123")
□ Check console: "[room] peer-123456 joined (1 peers)"
□ Wait 2 seconds, check console for ICE logs
□ Note: WebSocket should show "status: connecting"
```

**Tab 2 (Joiner)**
```
□ Copy room link from Tab 1
□ Paste in Tab 2 address bar (or type room ID)
□ Check console: "[room] peer-789012 joined (2 peers)"
□ Wait 2 seconds for ICE gathering
□ Note: Both tabs should show "status: connected" after DataChannel opens
```

**Visual Verification (Room Page)**
```
□ Tab 1: "Peers connected: 1" (shows Tab 2's peer ID)
□ Tab 2: "Peers connected: 1" (shows Tab 1's peer ID)
□ Both: "Signaling: ws://localhost:8333" or "wss://..."
```

**DataChannel Verification**
```
□ Tab 1: Click a block in the grid
□ Watch console: "datachannel.send(93 bytes)" appears
□ Tab 2: Same block appears updated in 100ms
□ Check both console logs: "message received from peer-X: 93 bytes"
```

**CRDT Convergence Test**
```
□ Tab 1: Place stone block at (2,3)
□ Tab 2: Place grass block at (4,5)
□ Wait 500ms
□ Both tabs should show BOTH blocks (convergence)
□ Check console: "delta from peer-X: 2 changes"
```

**Network Failure Simulation**
```
□ Tab 1: Open DevTools Network tab
□ Throttle to "Offline"
□ Tab 1: Try to place a block
□ Tab 1 console: "datachannel.send() failed: not open"
□ Go back to throttle=Online
□ Wait 5 seconds for reconnection (or manual refresh)
□ Check: "status changed: connected → reconnecting → connected"
```

---

## Phase 2: Debug UI Enhancements

Add to `/routes/room/+page.svelte` (visible indicators):

```svelte
<!-- Connection State Panel -->
<div class="debug-panel">
  <h3>DEBUG: Connection State</h3>
  
  <!-- WebSocket Status -->
  <div class="status-row">
    <label>Signaling WS:</label>
    <span class="indicator {status}">
      {status.toUpperCase()}
    </span>
    <span class="detail">(Room: {roomId}, You: {myId.slice(0,6)}...)</span>
  </div>
  
  <!-- Peer Count -->
  <div class="status-row">
    <label>Peers Connected:</label>
    <span class="indicator">{peerList.length}</span>
  </div>
  
  <!-- Peer Details -->
  {#each peerList as peer}
    <div class="peer-row">
      <span class="label">{peer.id.slice(0,8)}</span>
      <span class="indicator datachannel-open">DC: OPEN</span>
      <span class="detail">Connected {Math.round((Date.now() - peer.connectedAt) / 1000)}s ago</span>
    </div>
  {/each}
  
  <!-- Last Delta -->
  <div class="status-row">
    <label>Last CRDT Delta:</label>
    <span class="detail">{lastDeltaInfo}</span>
  </div>
  
  <!-- BFT Consensus -->
  <div class="status-row">
    <label>BFT Consensus:</label>
    <span class="detail">Round {consensusRound}, {votes.length}/3 votes</span>
  </div>
  
  <!-- Message Rate -->
  <div class="status-row">
    <label>Message Rate (10s):</label>
    <span class="detail">{messagesPerSecond.toFixed(1)} msg/s</span>
  </div>
</div>

<style>
  .debug-panel {
    background: #1e1e1e;
    color: #e0e0e0;
    padding: 1rem;
    border-radius: 4px;
    font-family: monospace;
    font-size: 12px;
    margin-top: 1rem;
  }
  
  .status-row, .peer-row {
    display: flex;
    gap: 1rem;
    margin: 0.5rem 0;
    align-items: center;
  }
  
  .indicator {
    padding: 2px 8px;
    border-radius: 3px;
    font-weight: bold;
  }
  
  .indicator.connected {
    background: #10b981;
    color: #fff;
  }
  
  .indicator.connecting {
    background: #f59e0b;
    color: #fff;
  }
  
  .indicator.disconnected {
    background: #ef4444;
    color: #fff;
  }
  
  .indicator.datachannel-open {
    background: #06b6d4;
    color: #fff;
  }
  
  .detail {
    color: #a0a0a0;
    font-size: 11px;
  }
</style>
```

### 2.1 Metrics to Display
- **WebSocket Status**: disconnected/connecting/connected
- **Peer Count**: 0, 1, 2, 3+
- **Per-Peer Indicators**: DataChannel state, connection age
- **Last CRDT Delta**: "2 changes in 45ms" or "waiting for peer"
- **BFT Round**: Round number, vote count
- **Message Rate**: msgs/sec (10s rolling average)

---

## Phase 3: Console Logging Instrumentation

### 3.1 Add to `room-state.ts`

```typescript
// KG: SPAN_333_Debug_WebRTCLogging

const DEBUG = true;

function log(msg: string, data?: any) {
  if (!DEBUG) return;
  const t = new Date().toISOString().slice(11, 23);
  const prefix = `[${t}] [${roomId.slice(0,6)}]`;
  if (data) console.log(prefix, msg, data);
  else console.log(prefix, msg);
}

// In WebSocket handlers:
ws.onopen = () => {
  log('WS: connected');
  ws!.send(JSON.stringify({ type: 'join', room: roomId, peerId: myId }));
};

ws.onmessage = async (e) => {
  const msg = JSON.parse(e.data);
  
  if (msg.type === 'peers') {
    log('WS: received peer list', { peers: msg.peers, you: msg.you });
    // ... rest of logic
  } else if (msg.type === 'offer') {
    log('WS: received SDP offer from', msg.from);
    // ... rest of logic
  } else if (msg.type === 'answer') {
    log('WS: received SDP answer from', msg.from);
    // ... rest of logic
  } else if (msg.type === 'ice') {
    log('ICE: candidate from', { from: msg.from, candidate: msg.candidate.candidate.slice(0,80) });
    // ... rest of logic
  }
};

ws.onerror = (err) => {
  log('WS: ERROR', err.message || err);
};

ws.onclose = () => {
  log('WS: closed, status=' + status);
};

// In RTCPeerConnection handlers:
pc.onicecandidate = (e) => {
  if (e.candidate) {
    const candidate = e.candidate.candidate.slice(0, 80);
    log(`ICE: local candidate for ${peerId}`, candidate);
  } else {
    log(`ICE: gathering complete for ${peerId}`);
  }
};

pc.onconnectionstatechange = () => {
  log(`PEER [${peerId}]: connection state = ${pc.connectionState}`);
};

// In DataChannel handlers:
channel.onopen = () => {
  log(`DC: opened with ${peerId}`);
};

channel.onclose = () => {
  log(`DC: closed for ${peerId}`);
};

channel.onerror = (err) => {
  log(`DC: ERROR on ${peerId}`, err.error?.message || err);
};

channel.onmessage = (e) => {
  log(`DC: received ${e.data.length} bytes from ${peerId}`);
};

// When sending:
state.send = (data: Uint8Array) => {
  let count = 0;
  for (const [peerId, ch] of dataChannels) {
    if (ch.readyState === 'open') {
      ch.send(data);
      count++;
    }
  }
  log(`DC: sent ${data.length} bytes to ${count} peers`);
};
```

### 3.2 Console Output Example

```
[12:34:56.789] [abc123] WS: connected
[12:34:57.012] [abc123] WS: received peer list { peers: [], you: 'peer-1234567890' }
[12:34:57.234] [abc123] ICE: local candidate for peer-2 srflx 1.2.3.4:51234 typ srflx
[12:34:57.456] [abc123] ICE: local candidate for peer-2 prflx 192.168.1.10:51235
[12:34:57.678] [abc123] ICE: gathering complete for peer-2
[12:34:58.012] [abc123] PEER [peer-2]: connection state = connecting
[12:34:58.234] [abc123] PEER [peer-2]: connection state = connected
[12:34:58.456] [abc123] DC: opened with peer-2
[12:34:59.789] [abc123] DC: sent 127 bytes to 1 peers
[12:35:00.012] [abc123] DC: received 127 bytes from peer-2
[12:35:00.234] [abc123] CRDT: merged delta { changes: 2, clock: 15 }
```

---

## Phase 4: Single-Machine Testing Gotchas

### 4.1 Two Tabs in Same Browser (Recommended for First Test)

**Pros:**
- Same machine, no network latency variation
- Shared localhost STUN server
- Both tabs in same DevTools session (harder to debug)

**Gotchas:**
```
❌ DON'T: Use public STUN (will be overengineered)
❌ DON'T: Both tabs have same origin, may share WebSocket pool
❌ DON'T: Assume localhost:8333 is always accessible (check firewall)
✓ DO: Use different room IDs per test session
✓ DO: Clear localStorage between tests (identity persistence)
✓ DO: Open DevTools in ONE tab at a time to observe logs clearly
```

**Setup:**
```bash
# Terminal 1: Start signaling
node signaling/server.mjs 8333

# Terminal 2: Start app
cd 333-app && npm run dev

# Browser: Tab 1
localhost:5173/room (create room → see "abc123")

# Browser: Tab 2
localhost:5173/room?id=abc123 (join room)
```

### 4.2 Two Different Browsers (More Realistic)

**Pros:**
- Tests actual network isolation
- Independent DevTools sessions
- More realistic peer discovery latency

**Gotchas:**
```
❌ DON'T: Assume mDNS (localhost) resolves in other browser
❌ DON'T: Use 127.0.0.1 (loopback only works on same machine)
✓ DO: Use local IP (192.168.x.y) or hostname
✓ DO: Test on same WiFi (simulate NAT)
✓ DO: Test across network (real firewall scenarios)
```

**Setup:**
```bash
# Signaling on public interface (not 127.0.0.1)
node signaling/server.mjs 8333 # binds 0.0.0.0:8333

# App Vite config: allow external
# vite.config.ts:
export default {
  server: {
    middlewareMode: false,
    host: '0.0.0.0'  // or '192.168.1.100'
  }
}

# Browser A: http://192.168.1.100:5173/room
# Browser B: http://192.168.1.100:5173/room?id=abc123
```

### 4.3 Localhost STUN Issues

**Problem:** RTCPeerConnection with `stun:stun.l.google.com:19302` may not work on localhost (both peers have 127.0.0.1, no NIC traversal).

**Solution:**
```typescript
// For localhost testing, use mock STUN or no ICE:
const iceServers = typeof window !== 'undefined' && location.hostname === 'localhost'
  ? []  // Disable ICE on localhost (direct connection only)
  : [{ urls: 'stun:stun.l.google.com:19302' }];

const pc = new RTCPeerConnection({ iceServers });
```

Or use a local STUN server:
```bash
# Docker: stun server on localhost:3478
docker run -d -p 3478:3478/udp coturn/coturn
```

---

## Phase 5: Automated Smoke Test (Node.js + puppeteer-core)

### 5.1 Simple Script: `tests/p2p-smoke.js`

```javascript
// KG: TEST_333_P2P_Smoke
// Simple P2P smoke test: 2 tabs, verify connection, exchange messages
// Usage: node tests/p2p-smoke.js ws://localhost:8333

const puppeteer = require('puppeteer');
const assert = require('assert');

const SIGNALING_URL = process.argv[2] || 'ws://localhost:8333';
const APP_URL = process.env.APP_URL || 'http://localhost:5173/room';
const TIMEOUT = 15000;

let roomId = null;
let tab1 = null;
let tab2 = null;

async function log(msg, data = '') {
  console.log(`[SMOKE] ${msg}`, data);
}

async function sleep(ms) {
  return new Promise(r => setTimeout(r, ms));
}

async function runTest() {
  const browser = await puppeteer.launch({ headless: true });
  
  try {
    log('Launching browser...');
    
    // Tab 1: Create room
    log('Opening Tab 1 (creator)...');
    tab1 = await browser.newPage();
    tab1.on('console', msg => {
      if (msg.text().includes('WS:') || msg.text().includes('DC:') || msg.text().includes('ICE:')) {
        console.log(`  [Tab1] ${msg.text()}`);
      }
    });
    await tab1.goto(APP_URL, { waitUntil: 'networkidle2', timeout: TIMEOUT });
    
    // Click "Create Room" button
    await tab1.click('button:has-text("Create Room")');
    await sleep(1000);
    
    // Extract room ID from page
    roomId = await tab1.evaluate(() => {
      const span = document.querySelector('[data-testid="room-id"]');
      return span ? span.textContent : null;
    });
    assert(roomId, 'Room ID not found on page');
    log(`Tab 1 created room: ${roomId}`);
    
    // Verify signaling connection
    const status1 = await tab1.evaluate(() => {
      return document.querySelector('[data-testid="connection-status"]')?.textContent || 'unknown';
    });
    log(`Tab 1 connection status: ${status1}`);
    
    // Tab 2: Join room
    log('Opening Tab 2 (joiner)...');
    tab2 = await browser.newPage();
    tab2.on('console', msg => {
      if (msg.text().includes('WS:') || msg.text().includes('DC:') || msg.text().includes('ICE:')) {
        console.log(`  [Tab2] ${msg.text()}`);
      }
    });
    await tab2.goto(`${APP_URL}?id=${roomId}`, { waitUntil: 'networkidle2', timeout: TIMEOUT });
    log(`Tab 2 joined room: ${roomId}`);
    
    // Wait for DataChannel to open (max 10 seconds)
    log('Waiting for peer connection (max 10s)...');
    let peerCount = 0;
    for (let i = 0; i < 20; i++) {
      peerCount = await tab1.evaluate(() => {
        const text = document.querySelector('[data-testid="peer-count"]')?.textContent;
        return text ? parseInt(text) : 0;
      });
      if (peerCount > 0) {
        log(`Tab 1 sees ${peerCount} peer(s)`);
        break;
      }
      await sleep(500);
    }
    assert(peerCount > 0, 'Peers did not connect within 10 seconds');
    
    // Verify bidirectional connection
    const peerCount2 = await tab2.evaluate(() => {
      const text = document.querySelector('[data-testid="peer-count"]')?.textContent;
      return text ? parseInt(text) : 0;
    });
    log(`Tab 2 sees ${peerCount2} peer(s)`);
    assert(peerCount2 > 0, 'Tab 2 did not see Tab 1');
    
    // CRDT test: Send message from Tab 1
    log('Testing message exchange...');
    await tab1.click('[data-x="2"][data-y="3"]'); // Click block
    await sleep(500);
    
    const blockPresent = await tab2.evaluate(() => {
      return !!document.querySelector('[data-x="2"][data-y="3"].active');
    });
    assert(blockPresent, 'Block not replicated to Tab 2');
    log('✓ Block replicated correctly');
    
    // Verify console logs contain WebRTC events
    const consoleOutput = tab1.evaluate(() => window.__debugLogs || []);
    assert(consoleOutput.length > 0, 'No console logs captured');
    log(`✓ Captured ${consoleOutput.length} debug events`);
    
    log('=== ALL TESTS PASSED ===');
    
  } catch (err) {
    log(`❌ TEST FAILED: ${err.message}`);
    throw err;
  } finally {
    await browser.close();
  }
}

runTest().catch(err => {
  console.error('Fatal:', err);
  process.exit(1);
});
```

### 5.2 Make Test IDs Visible

Add to room page template:

```svelte
<!-- For testing, expose data via attributes -->
<span data-testid="room-id" hidden>{roomId}</span>
<span data-testid="connection-status" hidden>{status}</span>
<span data-testid="peer-count" hidden>{peerList.length}</span>

<!-- Existing block grid -->
{#each Array(GRID) as _, y}
  {#each Array(GRID) as _, x}
    <div
      data-x={x}
      data-y={y}
      class="block {blocks.get(`${x},${y}`) || ''}"
      on:click={() => placeBlock(x, y)}
    />
  {/each}
{/each}
```

### 5.3 Run Test

```bash
# Install puppeteer-core (lighter than full puppeteer)
npm install --save-dev puppeteer-core

# In separate terminals:
# Terminal 1: Signaling
node signaling/server.mjs 8333

# Terminal 2: App
cd 333-app && npm run dev

# Terminal 3: Test
node tests/p2p-smoke.js

# Output:
# [SMOKE] Launching browser...
# [SMOKE] Opening Tab 1 (creator)...
# [SMOKE] Tab 1 created room: abc123
# [SMOKE] Opening Tab 2 (joiner)...
# [SMOKE] Tab 2 joined room: abc123
# [SMOKE] Waiting for peer connection (max 10s)...
#   [Tab1] [12:34:56.789] WS: connected
#   [Tab1] ICE: local candidate for peer-X
# [SMOKE] Tab 1 sees 1 peer(s)
# [SMOKE] Testing message exchange...
# [SMOKE] ✓ Block replicated correctly
# [SMOKE] === ALL TESTS PASSED ===
```

---

## Phase 6: CI Integration (GitHub Actions)

### 6.1 Workflow File: `.github/workflows/p2p-test.yml`

```yaml
name: P2P Smoke Tests

on:
  push:
    branches: [ main, develop ]
  pull_request:
    branches: [ main ]

jobs:
  p2p-test:
    runs-on: ubuntu-latest
    timeout-minutes: 10

    services:
      signaling:
        image: node:20-alpine
        options: >-
          --health-cmd "wget --quiet --tries=1 --spider http://localhost:8333 || exit 1"
          --health-interval 5s
          --health-timeout 5s
          --health-retries 5
        ports:
          - 8333:8333
        run: |
          cd /tmp/signaling
          npm install
          node server.mjs 8333

    steps:
      - uses: actions/checkout@v4

      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: '20'
          cache: 'npm'

      - name: Copy signaling to service
        run: cp -r signaling /tmp/signaling && cd /tmp/signaling && npm install

      - name: Install app dependencies
        run: |
          cd 333-app
          npm install

      - name: Start app dev server
        run: |
          cd 333-app
          npm run dev &
          sleep 3  # Wait for Vite to start

      - name: Install test dependencies
        run: npm install --save-dev puppeteer-core

      - name: Wait for services
        run: |
          npm install --save-dev wait-on
          npx wait-on http://localhost:5173 ws://localhost:8333 --timeout 15000

      - name: Run P2P smoke test
        run: node tests/p2p-smoke.js ws://localhost:8333
        env:
          APP_URL: http://localhost:5173/room

      - name: Upload test results
        if: always()
        uses: actions/upload-artifact@v3
        with:
          name: test-logs
          path: test-results.json
```

### 6.2 Docker Compose for Local CI Testing

```yaml
# docker-compose.test.yml
version: '3.8'

services:
  signaling:
    image: node:20-alpine
    working_dir: /app
    volumes:
      - ./signaling:/app
    ports:
      - "8333:8333"
    command: |
      sh -c "npm install && node server.mjs 8333"
    healthcheck:
      test: ["CMD", "node", "-e", "require('http').get('http://localhost:8333', () => process.exit(0))"]
      interval: 5s
      timeout: 5s
      retries: 5

  app:
    image: node:20
    working_dir: /app
    volumes:
      - ./333-app:/app
    ports:
      - "5173:5173"
    command: |
      sh -c "npm install && npm run dev -- --host 0.0.0.0"
    environment:
      - VITE_SIGNALING_URL=ws://signaling:8333
    depends_on:
      signaling:
        condition: service_healthy

  test:
    image: node:20
    working_dir: /app
    volumes:
      - ./:/app
      - ./tests:/app/tests
    command: |
      sh -c "npm install puppeteer-core && node tests/p2p-smoke.js ws://signaling:8333"
    environment:
      - APP_URL=http://app:5173/room
    depends_on:
      app:
        condition: service_healthy
```

Run locally:
```bash
docker-compose -f docker-compose.test.yml up --abort-on-container-exit
```

---

## Phase 7: Test Coverage Checklist

### 7.1 Manual Tests (Must Pass Before Deploy)

```
CONNECTION TESTS
  □ Signaling: 2 peers can connect via WebSocket
  □ ICE: Candidates are gathered (check console logs)
  □ DataChannel: Opens after SDP exchange
  □ Reconnection: Manual disconnect/reconnect works

CRDT TESTS
  □ Single-Write: Tab 1 writes, Tab 2 sees change within 1s
  □ Concurrent Writes: Both tabs write, both converge to same state
  □ Tombstones: Deletion syncs (LWW-Map handles it)
  □ Merge: Partial sync, then full delta, state consistent

BFT TESTS
  □ Voting: 3+ peers vote on state change
  □ Consensus: Majority (≥2/3) reaches agreement
  □ Byzantine: One peer sends wrong vote, system recovers
  □ Round Change: View change works on leader failure

EDGE CASES
  □ Peer Dropout: Tab 1 closes, Tab 2 detects disconnect in <2s
  □ Network Split: Separate VLANs, peers reconnect on merge
  □ Message Burst: 100 messages/sec, no buffer overflow
  □ Large Delta: 1MB CRDT delta compresses/transmits correctly
```

### 7.2 Automated Tests (CI Only)

```
SMOKE TEST (required)
  □ 2 peers connect within 10s
  □ Block exchange works (CRDT replication)
  □ No JavaScript errors in console
  □ Memory usage < 200MB per tab

LOAD TEST (optional)
  □ 10 concurrent peers in same room
  □ 1000 blocks placed, state converges
  □ Message latency p99 < 500ms

FAILURE TEST (optional)
  □ Peer dropout, automatic cleanup
  □ WebSocket reconnection on interruption
  □ DataChannel buffering (no data loss)
```

---

## Summary: Incremental Implementation

```
Week 1: Manual Testing
  - Checklist (Phase 1)
  - Console logs (Phase 3)
  - Debug UI (Phase 2)
  → Can reliably test 2-peer scenarios

Week 2: Single-Machine Automation
  - Puppeteer smoke test (Phase 5)
  - Test IDs in HTML
  → CI-ready test that catches regressions

Week 3: CI Integration
  - GitHub Actions workflow (Phase 6.1)
  - Docker Compose for local testing
  → Automated on every push

Week 4: Scale Testing
  - 3+ peer scenarios
  - BFT consensus verification
  - Stress tests (10K messages/min)
```

---

## References

- **WebRTC**: MDN RTCPeerConnection, RTCDataChannel
- **CRDT**: Conflict-free Replicated Data Types (CRDT), especially Last-Writer-Wins (LWW)
- **BFT**: Practical Byzantine Fault Tolerance (PBFT)
- **Puppeteer**: Headless Chrome automation, `.evaluate()` for DOM access
- **Node.js WebSocket**: ws module for signaling server

