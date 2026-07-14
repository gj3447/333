# WebRTC E2E Testing with Playwright — Quick Reference
> **Status**: Ready to implement  
> **KG**: TASK_333_E2E_WebRTC_Testing  
> **Location**: /Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform/

---

## TL;DR Answers

| Question | Answer | Key Point |
|----------|--------|-----------|
| **Q1: Two contexts?** | YES ✓ | Playwright natively supports multiple isolated contexts in one test |
| **Q2: ICE/STUN?** | Use REAL server | Localhost STUN works fine; don't mock |
| **Q3: Mock signaling?** | NO — use REAL | Your server is simple enough (91 lines, pure relay) |
| **Q4: Wait for connection?** | DC open + timeout | `dataChannel.readyState === 'open'` + 15s timeout |
| **Q5: Intercept messages?** | Logging API needed | Expose `getReceivedMessages()` from WASM + evaluate in test |
| **Q6: Examples/libraries?** | MockRTC exists | But overkill; write simple custom harness (100-200 lines) |
| **Q7: Best tool?** | Playwright > Puppeteer > Cypress | Multi-context architecture perfect for P2P |

---

## Architecture

```
Test File (Playwright)
  ├─ Browser (Chrome)
  │  ├─ Context A (peer1)
  │  │  └─ Page 1 (localhost:5173)
  │  │     └─ WASM → RTCPeerConnection
  │  │        └─ DataChannel (label: "data")
  │  │
  │  └─ Context B (peer2)
  │     └─ Page 2 (localhost:5173)
  │        └─ WASM → RTCPeerConnection
  │           └─ DataChannel (label: "data")
  │
  └─ Signaling Server (ws://localhost:8333)
     └─ Room relay → SDP/ICE exchange
```

---

## Test Pattern (Copy-Paste)

```typescript
import { test, expect } from '@playwright/test';

test('P2P connection established', async ({ browser }) => {
  // Create peer 1
  const ctx1 = await browser.newContext();
  const peer1 = await ctx1.newPage();
  await peer1.goto('http://localhost:5173');
  
  // Create peer 2
  const ctx2 = await browser.newContext();
  const peer2 = await ctx2.newPage();
  await peer2.goto('http://localhost:5173');
  
  // Peer 1 initiates
  await peer1.evaluate(() => window.initiateP2P?.());
  
  // Wait for connection (max 15 seconds)
  const dcState = await peer2.waitForFunction(() =>
    window.peerConnection?.dataChannel?.readyState === 'open',
    { timeout: 15000 }
  );
  
  expect(dcState).toBe('open');
  
  await ctx1.close();
  await ctx2.close();
});
```

---

## Files to Create (4 files, ~500 lines total)

### 1. `playwright.config.ts`
```typescript
export default defineConfig({
  testDir: './tests/e2e',
  workers: 1,  // ← Critical: disable parallelism
  timeout: 30_000,
  webServer: {
    command: 'npm run dev',
    url: 'http://localhost:5173',
  },
});
```

### 2. `tests/e2e/fixtures/webrtc.fixture.ts` (120 lines)
- Inject logging API into window
- Create peer1 & peer2 fixtures
- Setup page fixtures with test harness

### 3. `tests/e2e/helpers/webrtc.ts` (150 lines)
- `waitForP2pConnection(peer1, peer2, timeout)`
- `sendMessage(peer, msg)`
- `waitForMessage(peer, pattern, timeout)`
- `getConnectionState(peer)`

### 4. `tests/e2e/webrtc-p2p.spec.ts` (200 lines)
- Test 1: "two peers connect"
- Test 2: "DataChannel message delivery"
- Test 3: "CRDT convergence"
- Test 4: "BFT consensus 3 peers"
- Test 5: "Reliability (ordering)"

---

## Setup (5 mins)

```bash
cd 333-app

# 1. Install Playwright
npm install --save-dev @playwright/test

# 2. Update package.json scripts
npm set-script test:e2e "playwright test"
npm set-script test:e2e:ui "playwright test --ui"

# 3. Create test directory
mkdir -p tests/e2e/{fixtures,helpers}

# 4. Start services (2 terminals)
# Terminal A:
node ../signaling/server.mjs          # ws://localhost:8333

# Terminal B:
npm run dev                             # http://localhost:5173

# Terminal C (run tests):
npm run test:e2e
```

---

## Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **Real signaling server** | Avoids mock complexity; server is trivial relay |
| **Two contexts** (not pages) | True network isolation, simulates real P2P |
| **Logging API in WASM** | Playwright can't intercept DataChannel directly |
| **DC open wait** | More reliable than connection state |
| **Single worker** | Shared signaling server (simple relay, not parallel-safe) |
| **15s timeout** | ICE gathering can take 5-10s on first run |

---

## Waiting for Connection (Implementation Details)

### Option A: Playwright's waitForFunction (Simplest)

```typescript
await peer2.waitForFunction(
  () => window.peerConnection?.dataChannel?.readyState === 'open',
  { timeout: 15000 }
);
```

**Pros**: One-liner  
**Cons**: No visibility into intermediate states (gathering, connecting)

### Option B: Custom Loop (Best for Debugging)

```typescript
async function waitForP2pConnection(peer1, peer2, timeoutMs = 15000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    const [dc1, dc2] = await Promise.all([
      peer1.evaluate(() => window.peerConnection?.dataChannel?.readyState),
      peer2.evaluate(() => window.peerConnection?.dataChannel?.readyState),
    ]);
    if (dc1 === 'open' && dc2 === 'open') return true;
    // Log every 2 seconds
    console.log(`[${Date.now() - start}ms] states: ${dc1}, ${dc2}`);
    await new Promise(r => setTimeout(r, 100));
  }
  throw new Error(`P2P timeout after ${timeoutMs}ms`);
}
```

**Pros**: Full visibility, easier to debug  
**Cons**: More code

**Recommendation**: Use Option B for development, Option A for CI.

---

## Exposing Logging API from Rust WASM

In `src/p2p/webrtc.rs`:

```rust
#[wasm_bindgen]
impl WebRtcPeer {
    /// For testing only: return copy of received messages
    #[cfg(test)]
    pub fn get_received_messages(&self) -> Vec<String> {
        self.inbox.lock().unwrap()
            .iter()
            .map(|msg| String::from_utf8_lossy(msg).to_string())
            .collect()
    }
}
```

Then in Playwright:

```typescript
await page.addInitScript(() => {
  window.__testHarness = {
    getMessages: () => window.peerConnection?.getReceivedMessages?.() || [],
  };
});

// In test:
const messages = await peer2.evaluate(() =>
  window.__testHarness.getMessages()
);
```

---

## Verifying CRDT Convergence

```typescript
test('CRDT sync', async ({ peer1, peer2 }) => {
  // Setup & connect...
  
  // Peer 1 modifies
  await peer1.evaluate(() => {
    window.crdtDocument?.insert(0, 'hello');
  });
  
  // Wait for DataChannel delivery
  await new Promise(r => setTimeout(r, 500));
  
  // Both docs should match
  const [doc1, doc2] = await Promise.all([
    peer1.evaluate(() => window.crdtDocument?.toString() || ''),
    peer2.evaluate(() => window.crdtDocument?.toString() || ''),
  ]);
  
  expect(doc1).toBe(doc2);  // Convergence!
});
```

---

## Verifying BFT Consensus

```typescript
test('BFT consensus', async ({ peer1, peer2, browser }) => {
  // Create peer 3
  const ctx3 = await browser.newContext();
  const peer3 = await ctx3.newPage();
  // ... setup peer3 ...
  
  // All three broadcast votes
  const vote = JSON.stringify({ type: 'vote', value: 'accept' });
  await Promise.all([
    sendMessage(peer1, vote),
    sendMessage(peer2, vote),
    sendMessage(peer3, vote),
  ]);
  
  // Each peer sees ≥2 other votes (BFT tolerance)
  const votes1 = await peer1.evaluate(() =>
    window.__testHarness.getMessages()
      .filter(m => JSON.parse(m).type === 'vote').length
  );
  
  expect(votes1).toBeGreaterThanOrEqual(2);  // 2/3 consensus
});
```

---

## Network Simulation (Future)

For packet loss / latency (not implemented yet):

```typescript
// Throttle network
await page.route('**/*', route => {
  const request = route.request();
  if (request.url().includes('signaling')) {
    setTimeout(() => route.continue(), 200);  // 200ms latency
  } else {
    route.continue();
  }
});
```

Or use Playwright's built-in network throttling:

```typescript
await page.context().route('**/*', async route => {
  await new Promise(r => setTimeout(r, 100));
  await route.continue();
});
```

---

## Debugging Tips

### Enable Playwright Inspector

```bash
PWDEBUG=1 npm run test:e2e
```

Pauses before each action. Use `Step Over` button.

### Run Single Test

```bash
npm run test:e2e -- --grep "P2P connection established"
```

### Run with UI Mode (Recommended for P2P)

```bash
npm run test:e2e:ui
```

Shows timeline, page state, logs all side-by-side.

### Check Browser Console Logs

```typescript
peer1.on('console', msg => console.log(msg.text()));
```

### Capture WebRTC Stats

```typescript
const stats = await peer1.evaluate(() =>
  window.peerConnection?.pc?.getStats?.()
);
```

---

## Troubleshooting

| Problem | Solution |
|---------|----------|
| "WebSocket connection failed" | Start signaling server: `node signaling/server.mjs` |
| "DataChannel stuck in 'connecting'" | Verify SDP exchange via browser console logs |
| "Messages not delivered" | Check `dataChannel.readyState` (must be 'open') |
| "Test timeout 30s" | Increase `timeout: 60_000` in config; check network logs |
| "Connection works locally, fails in CI" | Need headless mode; add `--headless` to config |
| "Port 5173 already in use" | Kill: `lsof -ti:5173 \| xargs kill -9` |

---

## Next Steps

1. **Today**: Copy skeleton files from WEBRTC_E2E_TESTING_RESEARCH.md
2. **Tomorrow**: Run Phase 0 setup + first test
3. **This week**: Implement CRDT + BFT tests
4. **Next week**: Add network simulation + cross-browser testing

---

## Reference

Full research document with code: `/Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform/WEBRTC_E2E_TESTING_RESEARCH.md`
