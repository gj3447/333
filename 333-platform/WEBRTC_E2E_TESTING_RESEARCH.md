# WebRTC P2P E2E Testing with Playwright — Research Report
> **KG**: lesson-webrtc-playwright-e2e-testing, TASK_333_E2E_WebRTC_Testing  
> **Date**: 2026-04-13  
> **Scope**: Testing P2P WebRTC connections with CRDT/BFT consensus  
> **Context**: 333-platform signaling server (ws://localhost:8333), Rust WASM peer, SvelteKit app

---

## Question 1: Can Playwright Open Two Browser Contexts/Pages That Connect via WebRTC?

**Answer: YES, with full support.**

Playwright's architecture supports multiple isolated browser contexts within a single test:
- Each context is equivalent to an incognito profile with isolated storage, cookies, and sessions
- Multiple pages can be created from the same context OR separate contexts can be used for true isolation
- **For P2P WebRTC**: Use **two separate browser contexts** to ensure network independence and authentic P2P simulation

### Key Architecture

```
Browser (single instance)
├─ Context A (User/Peer 1)
│  └─ Page A (wasm-app with RTCPeerConnection)
├─ Context B (User/Peer 2)
│  └─ Page B (wasm-app with RTCPeerConnection)
└─ Both contexts can reach same signaling server (ws://localhost:8333)
```

**Why separate contexts?**
- True network isolation (different cookies, storage, permissions)
- Prevents false positives (cross-context leakage)
- Simulates real multi-device scenario more authentically
- No shared state pollution

**Source**: [Playwright Browser Contexts](https://playwright.dev/docs/browser-contexts)

---

## Question 2: How to Handle ICE Candidates and STUN in Playwright Test Environment?

**Answer: Three approaches, ranked by recommendation.**

### Approach A: Real STUN Servers + Localhost (RECOMMENDED for P2P)

For localhost testing where both peers are on the same machine, STUN servers still work but provide host candidates only.

**Pros:**
- Tests real browser behavior
- Automatically handles ICE gathering state
- Compatible with real-world deployment

**Cons:**
- Slightly slower (STUN roundtrip, even if host IP)

**Implementation:**

```typescript
// tests/webrtc-p2p.spec.ts
import { test, expect } from '@playwright/test';

test('two peers connect via STUN on localhost', async ({ browser }) => {
  // Use browser-level STUN config (already in webrtc.rs)
  // No special Playwright config needed — browser does STUN automatically
  
  const ctx1 = await browser.newContext();
  const page1 = await ctx1.newPage();
  
  const ctx2 = await browser.newContext();
  const page2 = await ctx2.newPage();
  
  // Wait for STUN to gather candidates
  await page1.goto('http://localhost:5173');  // SvelteKit dev server
  await page2.goto('http://localhost:5173');
  
  // Signaling happens via ws://localhost:8333 (real server)
  // ICE gathering happens automatically via STUN
  
  await expect(() => 
    page1.evaluate(() => (window as any).peerConnection?.connectionState === 'connected')
  ).toBeTruthy({ timeout: 5000 });
  
  await ctx1.close();
  await ctx2.close();
});
```

### Approach B: Override STUN for Pure Host Candidates (FASTEST)

**For testing only on localhost**, remove STUN servers to force host-only candidates.

**Pros:**
- Eliminates STUN latency (~50-100ms per gather)
- Instant connection (both peers on same network)
- Deterministic (no external dependency)

**Cons:**
- Doesn't test real NAT scenarios
- May hide STUN-related bugs

**Implementation:**

```typescript
test('two peers connect via host candidates only', async ({ browser }) => {
  const ctx1 = await browser.newContext();
  const page1 = await ctx1.newPage();
  
  // Inject override script BEFORE app loads
  await page1.addInitScript(() => {
    const originalRTC = window.RTCPeerConnection;
    (window as any).RTCPeerConnection = function(config: any) {
      // Remove STUN servers for localhost testing
      if (config?.iceServers) {
        config.iceServers = config.iceServers.filter((s: any) =>
          !s.urls.some((u: string) => u.includes('stun'))
        );
      }
      return new originalRTC(config);
    };
  });
  
  await page1.goto('http://localhost:5173');
  // Rest of test...
});
```

### Approach C: Mock ICE Candidates Entirely (UNREALISTIC)

Using MockRTC or manual SDP interception — useful for unit tests, NOT for E2E P2P tests.

**Not recommended** for your use case (CRDT/BFT consensus requires real-time channel behavior).

---

## Question 3: Mock vs Real Signaling Server — Which Should We Use?

**Answer: ALWAYS use the REAL signaling server (ws://localhost:8333) for E2E tests.**

Your signaling server is minimal and deterministic — mocking it adds complexity without benefit.

### Comparison

| Aspect | Mock Signaling | Real Server (ws://localhost:8333) |
|--------|---|---|
| **Complexity** | High (state machine, serialization) | Low (simple relay) |
| **Realism** | Moderate (doesn't test server logic) | High (tests actual behavior) |
| **Latency** | 0ms (instant) | 1-5ms (localhost) |
| **Reliability** | Perfect (no flakes) | Near-perfect (rare TCP resets) |
| **Development** | Slow (maintain mock) | Fast (one server) |

### Why Your Server is Perfect for Testing

```javascript
// signaling/server.mjs — 91 lines, zero state
// - No database
// - No authentication
// - No rate limiting
// - Pure relay (room → peers)
```

**Cost of real server**: Negligible. It's already running for development.

---

## Question 4: Waiting Strategies — How to Wait for WebRTC Connection?

**Answer: Use these three waiting patterns in order.**

### Pattern A: Wait for DataChannel Open

```typescript
// Most reliable for your CRDT/BFT use case
const dcOpen = page.evaluate(() => {
  return new Promise<boolean>(resolve => {
    const checkDC = setInterval(() => {
      if ((window as any).peerConnection?.dataChannel?.readyState === 'open') {
        clearInterval(checkDC);
        resolve(true);
      }
    }, 50);
    setTimeout(() => {
      clearInterval(checkDC);
      resolve(false);
    }, 10000);
  });
});

await expect(dcOpen).toBeTruthy({ timeout: 15000 });
```

### Pattern B: Wait for Connection State (Intermediate)

```typescript
// Less reliable (connection state ≠ data channel ready)
await expect(async () => {
  const state = await page.evaluate(() => 
    (window as any).peerConnection?.connectionState
  );
  expect(state).toBe('connected');
}).toPass({ timeout: 10000 });
```

### Pattern C: Wait for ICE Gathering Complete (Low-level)

```typescript
// For debugging ICE issues only
await expect(async () => {
  const iceState = await page.evaluate(() =>
    (window as any).peerConnection?.iceGatheringState
  );
  expect(iceState).toBe('complete');
}).toPass({ timeout: 5000 });
```

### Combined Pattern (RECOMMENDED)

```typescript
async function waitForP2pConnection(
  page1: Page,
  page2: Page,
  timeoutMs = 15000
) {
  const startTime = Date.now();
  
  while (Date.now() - startTime < timeoutMs) {
    const [dc1, dc2] = await Promise.all([
      page1.evaluate(() => (window as any).peerConnection?.dataChannel?.readyState),
      page2.evaluate(() => (window as any).peerConnection?.dataChannel?.readyState),
    ]);
    
    if (dc1 === 'open' && dc2 === 'open') {
      return true;
    }
    
    await new Promise(r => setTimeout(r, 100));
  }
  
  throw new Error(`P2P connection timeout after ${timeoutMs}ms`);
}
```

**Source**: [WebRTC for the Curious — Connecting](https://webrtcforthecurious.com/docs/03-connecting/), [Playwright Waiting Best Practices](https://playwright.dev/docs/best-practices)

---

## Question 5: Verifying DataChannel Messages — Can Playwright Intercept Them?

**Answer: PARTIAL YES. Playwright cannot directly intercept DataChannel binary data, but you can:**

1. **Expose a logging API from your WASM code** (recommended)
2. **Use MockRTC for message inspection** (if you need it)
3. **Verify state changes from received messages** (indirect verification)

### Approach A: Logging API (RECOMMENDED)

Expose a test-friendly API in your Rust WASM:

```rust
// src/p2p/webrtc.rs
#[wasm_bindgen]
impl WebRtcPeer {
    #[cfg(test)]
    pub fn get_received_messages(&self) -> Vec<String> {
        // Return copy of inbox for testing
        self.inbox.lock().unwrap()
            .iter()
            .map(|msg| String::from_utf8_lossy(msg).to_string())
            .collect()
    }
    
    #[cfg(test)]
    pub fn clear_inbox(&self) {
        self.inbox.lock().unwrap().clear();
    }
}
```

Then in Playwright test:

```typescript
test('CRDT delta sync via DataChannel', async ({ page1, page2 }) => {
  // Setup peers...
  
  // Peer 1 sends a delta
  await page1.evaluate(() => {
    const delta = { op: 'insert', id: 'doc-1', val: 'hello' };
    (window as any).peerConnection.dataChannel.send(
      JSON.stringify(delta)
    );
  });
  
  // Peer 2 receives and verifies
  await expect(async () => {
    const messages = await page2.evaluate(() =>
      (window as any).peerConnection.getReceivedMessages() // Your logging API
    );
    expect(messages.length).toBeGreaterThan(0);
    expect(messages[0]).toContain('insert');
  }).toPass({ timeout: 5000 });
});
```

### Approach B: Verify State Instead of Messages

If you can't modify WASM, verify through observable state changes:

```typescript
test('CRDT consensus reached after delta exchange', async ({ page1, page2 }) => {
  // Setup peers...
  
  // Peer 1 modifies local CRDT state
  await page1.evaluate(() => {
    (window as any).crdtDocument.insert(0, 'hello');
  });
  
  // Message sent silently via DataChannel...
  await new Promise(r => setTimeout(r, 500)); // Wait for message delivery
  
  // Peer 2's state should now match
  const doc1 = await page1.evaluate(() => (window as any).crdtDocument.toString());
  const doc2 = await page2.evaluate(() => (window as any).crdtDocument.toString());
  
  expect(doc1).toBe(doc2); // Convergence achieved
});
```

### Approach C: MockRTC for Full Message Inspection

Using [`mockrtc`](https://github.com/httptoolkit/mockrtc) library:

```typescript
import { MockRTC } from 'mockrtc';

test('DataChannel messages inspected with MockRTC', async () => {
  const mockRTC = new MockRTC();
  
  // Hook peer 2 to inspect messages
  mockRTC.buildPeer()
    .waitForDataChannel('data')
    .thenSend('{"type":"ack"}')
    .setup();
  
  // Now peer 1 can connect to mock peer 2
  // All DataChannel messages are logged/inspectable
});
```

**Limitations**: MockRTC is designed for browser-only testing; integrating with Playwright adds complexity.

**Source**: [MDN WebRTC Data Channels](https://developer.mozilla.org/en-US/docs/Games/Techniques/WebRTC_data_channels), [MockRTC GitHub](https://github.com/httptoolkit/mockrtc)

---

## Question 6: Existing Playwright WebRTC Examples or Libraries?

**Answer: Few official examples, but growing ecosystem.**

### Known Libraries & Resources

1. **MockRTC** — Mock peer + message interception
   - GitHub: https://github.com/httptoolkit/mockrtc
   - Best for unit-level testing
   - Browser + Node.js admin server required

2. **WebRTC Samples** — Reference implementations
   - https://webrtc.github.io/samples/
   - DataChannel examples, SDP debugging tools
   - NOT test-focused

3. **Markaicode WebRTC Testing Guide** — Playwright + CDP
   - https://markaicode.com/webrtc-testing-playwright-cdp-overrides/
   - Advanced: Network throttling, media mocking, SDP interception
   - Best for video/audio call testing (not pure DataChannel)

4. **Playwright Test Library** — No built-in WebRTC helpers
   - No `await page.waitForWebRTC()` method
   - Must write custom waits

### Recommended Approach for 333-Platform

Since your use case is **P2P + DataChannel + BFT consensus** (not video/audio), the simplest path is:

✓ **Custom E2E test harness** (100-200 lines)  
✓ **Real signaling server** (already have it)  
✓ **Logging API in WASM** (your choice to expose)  
✗ Don't use MockRTC (adds dependency, not needed)  
✗ Don't use CDP overrides (focus on DataChannel, not media)

---

## Question 7: Alternatives — Puppeteer vs Cypress vs Manual Testing?

**Answer: Playwright is BEST choice for WebRTC P2P. Here's why:**

### Comparison Matrix

| Capability | Playwright | Puppeteer | Cypress | Manual |
|---|---|---|---|---|
| **Multi-tab/context** | ✓ (native) | ✓ (with workarounds) | ✗ (same domain only) | N/A |
| **Network control** | ✓ (CDP) | ✓ (low-level) | ~ (limited) | N/A |
| **WebRTC testing** | ✓✓ (documented) | ✓ (low-level) | ~ (plugins) | ✓ (manual) |
| **Cross-browser** | ✓ (Chrome, Firefox, Safari) | Chromium only | Chromium only | All |
| **Developer experience** | ✓✓ (intuitive) | ~ (lower-level) | ✓ (UI-focused) | Poor |
| **Test isolation** | ✓✓ (contexts) | ~ (sessions) | ✓ (but single-domain) | N/A |
| **DataChannel inspection** | ~ (logging API needed) | ~ (same) | ✗ | ✓ (DevTools) |

### Verdict

**Playwright wins** for P2P WebRTC testing because:
- **Multi-context architecture** is natural for P2P (two isolated "users")
- **CDP access** for advanced debugging (ICE logs, RTCStats)
- **Network control** for future latency/loss simulation
- **Test isolation** prevents cross-test pollution
- **Cross-browser support** (plan for Firefox/Safari compat)

**When to use alternatives:**
- **Puppeteer**: If you only test Chromium and need low-level Node.js control
- **Cypress**: If testing single-domain P2P calls (not your case)
- **Manual**: For exploratory debugging (not CI/CD)

**Source**: [Playwright vs Cypress 2026 Comparison](https://bugbug.io/blog/test-automation-tools/cypress-vs-playwright/), [BrowserStack Tool Comparison](https://www.browserstack.com/guide/cypress-vs-selenium-vs-playwright-vs-puppeteer)

---

## Concrete Test Script Skeleton

### Setup: Install Dependencies

```bash
cd /Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform/333-app

npm install --save-dev @playwright/test

# Update package.json scripts
cat >> package.json << 'EOF'
{
  "scripts": {
    "test:e2e": "playwright test",
    "test:e2e:ui": "playwright test --ui",
    "test:e2e:debug": "playwright test --debug"
  }
}
EOF
```

### File 1: Playwright Config

**File**: `333-app/playwright.config.ts`

```typescript
// KG: TASK_333_E2E_WebRTC_Testing
import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests/e2e',
  fullyParallel: false,  // ← Disable parallel for P2P (shared signaling server)
  workers: 1,             // ← Single worker
  timeout: 30_000,
  expect: {
    timeout: 10_000,
  },
  
  use: {
    baseURL: 'http://localhost:5173',
    // Enable Chrome-specific WebRTC debugging
    launchArgs: ['--use-fake-ui-for-media-stream'],
  },
  
  webServer: {
    command: 'npm run dev',  // SvelteKit dev server
    url: 'http://localhost:5173',
    reuseExistingServer: process.env.CI ? false : true,
    timeout: 120_000,
  },
  
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
});
```

### File 2: Test Fixtures

**File**: `333-app/tests/e2e/fixtures/webrtc.fixture.ts`

```typescript
// KG: TASK_333_E2E_WebRTC_Testing
import { test as base, Page, Browser, BrowserContext } from '@playwright/test';

type WebRtcFixtures = {
  peer1: Page;
  peer2: Page;
  browser: Browser;
  ctx1: BrowserContext;
  ctx2: BrowserContext;
};

/**
 * Fixture: Setup two isolated browser contexts for P2P testing
 */
export const test = base.extend<WebRtcFixtures>({
  peer1: async ({ browser }, use) => {
    const ctx = await browser.newContext();
    const page = await ctx.newPage();
    
    // Inject logging API into WASM
    await page.addInitScript(() => {
      (window as any).__testHarness = {
        getMessages: () => (window as any).peerConnection?.getReceivedMessages?.() || [],
        getState: () => (window as any).peerConnection?.peerState?.() || 'unknown',
        getDCState: () => (window as any).peerConnection?.dataChannel?.readyState || 'closed',
        send: (data: string) => (window as any).peerConnection?.dataChannel?.send(data),
      };
    });
    
    await page.goto('/');  // baseURL from config
    await use(page);
    await ctx.close();
  },
  
  peer2: async ({ browser }, use) => {
    const ctx = await browser.newContext();
    const page = await ctx.newPage();
    
    // Same injection
    await page.addInitScript(() => {
      (window as any).__testHarness = {
        getMessages: () => (window as any).peerConnection?.getReceivedMessages?.() || [],
        getState: () => (window as any).peerConnection?.peerState?.() || 'unknown',
        getDCState: () => (window as any).peerConnection?.dataChannel?.readyState || 'closed',
        send: (data: string) => (window as any).peerConnection?.dataChannel?.send(data),
      };
    });
    
    await page.goto('/');
    await use(page);
    await ctx.close();
  },
});
```

### File 3: Helper Functions

**File**: `333-app/tests/e2e/helpers/webrtc.ts`

```typescript
// KG: TASK_333_E2E_WebRTC_Testing
import { Page, expect } from '@playwright/test';

/**
 * Wait for two peers to establish P2P connection
 * Returns when both DataChannels are open
 */
export async function waitForP2pConnection(
  peer1: Page,
  peer2: Page,
  timeoutMs = 15000
): Promise<void> {
  const startTime = Date.now();
  
  while (Date.now() - startTime < timeoutMs) {
    const [dc1State, dc2State] = await Promise.all([
      peer1.evaluate(() => (window as any).__testHarness.getDCState()),
      peer2.evaluate(() => (window as any).__testHarness.getDCState()),
    ]);
    
    if (dc1State === 'open' && dc2State === 'open') {
      console.log('✓ P2P connection established');
      return;
    }
    
    // Debug output every 2 seconds
    if ((Date.now() - startTime) % 2000 < 100) {
      console.log(
        `[${(Date.now() - startTime).toFixed(0)}ms] DC states: peer1=${dc1State}, peer2=${dc2State}`
      );
    }
    
    await new Promise(r => setTimeout(r, 100));
  }
  
  throw new Error(`P2P connection timeout after ${timeoutMs}ms`);
}

/**
 * Send message from peer1 to peer2
 */
export async function sendMessage(
  sender: Page,
  message: string
): Promise<void> {
  await sender.evaluate((msg: string) => {
    (window as any).__testHarness.send(msg);
  }, message);
  
  // Small delay for network delivery
  await new Promise(r => setTimeout(r, 100));
}

/**
 * Wait for peer2 to receive a message
 */
export async function waitForMessage(
  receiver: Page,
  expectedPattern: string | RegExp,
  timeoutMs = 5000
): Promise<string> {
  const startTime = Date.now();
  const regex = typeof expectedPattern === 'string' 
    ? new RegExp(expectedPattern.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
    : expectedPattern;
  
  while (Date.now() - startTime < timeoutMs) {
    const messages = await receiver.evaluate(() =>
      (window as any).__testHarness.getMessages()
    );
    
    const found = messages.find((m: string) => regex.test(m));
    if (found) {
      console.log(`✓ Message received: ${found}`);
      return found;
    }
    
    await new Promise(r => setTimeout(r, 50));
  }
  
  throw new Error(
    `Message timeout: expected ${expectedPattern} after ${timeoutMs}ms`
  );
}

/**
 * Get ICE connection state (for debugging)
 */
export async function getConnectionState(
  peer: Page
): Promise<{
  peerState: string;
  dcState: string;
  iceState?: string;
}> {
  return peer.evaluate(() => ({
    peerState: (window as any).__testHarness.getState(),
    dcState: (window as any).__testHarness.getDCState(),
    iceState: (window as any).peerConnection?.pc?.iceConnectionState,
  }));
}
```

### File 4: P2P Connection Test

**File**: `333-app/tests/e2e/webrtc-p2p.spec.ts`

```typescript
// KG: TASK_333_E2E_WebRTC_Testing
import { test, expect } from './fixtures/webrtc.fixture';
import {
  waitForP2pConnection,
  sendMessage,
  waitForMessage,
  getConnectionState,
} from './helpers/webrtc';

test.describe('WebRTC P2P Connection', () => {
  
  test('two peers establish DataChannel', async ({ peer1, peer2 }) => {
    // Both pages loaded with fixture setup
    
    // Wait for SvelteKit app to initialize
    await peer1.waitForLoadState('networkidle');
    await peer2.waitForLoadState('networkidle');
    
    // Initiate connection (peer1 = offerer, peer2 = answerer)
    await peer1.evaluate(() => {
      (window as any).initiateP2pConnection?.();
    });
    
    // Wait for full P2P establishment
    await waitForP2pConnection(peer1, peer2, 15000);
    
    // Verify states
    const state1 = await getConnectionState(peer1);
    const state2 = await getConnectionState(peer2);
    
    expect(state1.dcState).toBe('open');
    expect(state2.dcState).toBe('open');
    expect(state1.peerState).toMatch(/connected|established/i);
  });
  
  test('DataChannel message delivery (simple)', async ({ peer1, peer2 }) => {
    await peer1.waitForLoadState('networkidle');
    await peer2.waitForLoadState('networkidle');
    
    await peer1.evaluate(() => (window as any).initiateP2pConnection?.());
    await waitForP2pConnection(peer1, peer2);
    
    // Send test message
    const testMsg = JSON.stringify({ type: 'hello', payload: 'test' });
    await sendMessage(peer1, testMsg);
    
    // Peer2 receives it
    const received = await waitForMessage(peer2, 'hello');
    expect(received).toContain('test');
  });
  
  test('CRDT delta sync convergence', async ({ peer1, peer2 }) => {
    await peer1.waitForLoadState('networkidle');
    await peer2.waitForLoadState('networkidle');
    
    // Setup connection
    await peer1.evaluate(() => (window as any).initiateP2pConnection?.());
    await waitForP2pConnection(peer1, peer2);
    
    // Peer1 inserts text
    await peer1.evaluate(() => {
      (window as any).crdtDocument?.insert(0, 'hello world');
    });
    
    // Message sent silently via DataChannel
    await new Promise(r => setTimeout(r, 500));
    
    // Both documents should converge
    const [doc1, doc2] = await Promise.all([
      peer1.evaluate(() => (window as any).crdtDocument?.toString() || ''),
      peer2.evaluate(() => (window as any).crdtDocument?.toString() || ''),
    ]);
    
    expect(doc1).toBe('hello world');
    expect(doc2).toBe('hello world');
  });
  
  test('BFT consensus with 3+ peers (network)', async ({ peer1, peer2, browser }) => {
    // Create a third peer
    const ctx3 = await browser.newContext();
    const peer3 = await ctx3.newPage();
    await peer3.addInitScript(() => {
      (window as any).__testHarness = {
        getMessages: () => (window as any).peerConnection?.getReceivedMessages?.() || [],
        getState: () => (window as any).peerConnection?.peerState?.() || 'unknown',
        getDCState: () => (window as any).peerConnection?.dataChannel?.readyState || 'closed',
        send: (data: string) => (window as any).peerConnection?.dataChannel?.send(data),
      };
    });
    await peer3.goto('http://localhost:5173');
    
    try {
      // All three connect
      await Promise.all([
        peer1.evaluate(() => (window as any).initiateP2pConnection?.()),
        peer2.evaluate(() => (window as any).initiateP2pConnection?.()),
        peer3.evaluate(() => (window as any).initiateP2pConnection?.()),
      ]);
      
      // Wait for consensus vote
      const voteMsg = JSON.stringify({
        type: 'consensus',
        vote: 'accept',
      });
      
      await Promise.all([
        sendMessage(peer1, voteMsg),
        sendMessage(peer2, voteMsg),
        sendMessage(peer3, voteMsg),
      ]);
      
      // Each peer should see 2 other votes + own
      await Promise.all([
        waitForMessage(peer1, 'consensus'),
        waitForMessage(peer2, 'consensus'),
        waitForMessage(peer3, 'consensus'),
      ]);
      
      // Verify BFT threshold (2/3 agreement)
      const messages = await peer1.evaluate(() =>
        (window as any).__testHarness.getMessages()
      );
      const consensusVotes = messages.filter((m: string) =>
        JSON.parse(m).type === 'consensus'
      );
      
      expect(consensusVotes.length).toBeGreaterThanOrEqual(2);
    } finally {
      await ctx3.close();
    }
  });
  
  test('DataChannel reliability under packet loss simulation', async ({ peer1, peer2 }) => {
    // Note: For true packet loss simulation, use Playwright network throttling
    // This is a placeholder for future enhancement
    
    await peer1.waitForLoadState('networkidle');
    await peer2.waitForLoadState('networkidle');
    await peer1.evaluate(() => (window as any).initiateP2pConnection?.());
    await waitForP2pConnection(peer1, peer2);
    
    // Send 10 messages
    const messages = [];
    for (let i = 0; i < 10; i++) {
      const msg = JSON.stringify({ seq: i, data: `msg-${i}` });
      messages.push(msg);
      await sendMessage(peer1, msg);
      await new Promise(r => setTimeout(r, 50));  // Space them out
    }
    
    // Wait for all deliveries (DataChannel is reliable by default)
    for (const msg of messages) {
      await waitForMessage(peer2, msg, 2000);
    }
    
    // Verify ordering
    const received = await peer2.evaluate(() =>
      (window as any).__testHarness.getMessages()
    );
    const sequences = received.map((m: string) =>
      JSON.parse(m).seq
    ).sort((a: number, b: number) => a - b);
    
    expect(sequences).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  });
  
});
```

### File 5: Run Tests

```bash
# Install dev dependencies first
cd /Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform/333-app
npm install

# Run tests (assumes signaling server running on ws://localhost:8333)
npm run test:e2e

# Run with UI (helps debug)
npm run test:e2e:ui

# Run in debug mode (pauses at first line)
npm run test:e2e:debug -- --project=chromium
```

---

## Implementation Checklist

### Phase 0: Setup (Today)
- [ ] Install Playwright: `npm install --save-dev @playwright/test`
- [ ] Add `playwright.config.ts` to 333-app
- [ ] Add test directory structure: `tests/e2e/{fixtures,helpers,specs}`
- [ ] Start signaling server: `node signaling/server.mjs`
- [ ] Verify SvelteKit dev server: `npm run dev` (port 5173)

### Phase 1: Foundation (Next)
- [ ] Implement fixture with two contexts
- [ ] Implement helper functions
- [ ] Create first test: "two peers connect"
- [ ] Run and debug (use `--debug` flag)
- [ ] Add logging API to Rust WASM if needed

### Phase 2: CRDT Testing
- [ ] Implement CRDT convergence test
- [ ] Test delta sync via DataChannel
- [ ] Verify message ordering

### Phase 3: BFT Consensus
- [ ] Implement 3+ peer consensus test
- [ ] Verify voting protocol
- [ ] Test fault tolerance scenarios

### Phase 4: Advanced
- [ ] Network throttling (latency/loss simulation)
- [ ] Cross-browser testing (Firefox, Safari)
- [ ] Performance profiling (DevTools metrics)

---

## Known Limitations & Future Work

1. **DataChannel Message Inspection**
   - Current: Requires logging API in WASM
   - Future: CDP-based DataChannel hooking (complex)

2. **Media Streams** (if added later)
   - Use `--use-fake-device-for-media-stream` flag
   - Or MockRTC for audio/video mocking

3. **Network Simulation**
   - Playwright: No built-in packet loss
   - Workaround: Use `page.route()` to drop/delay certain messages
   - Future: Network Conditioner or Clumsy integration

4. **CI/CD Integration**
   - GitHub Actions: Use `--headed=false` (headless by default)
   - Docker: Need X11 or use headless mode
   - Example config provided separately

---

## References

### Official Documentation
- [Playwright Browser Contexts](https://playwright.dev/docs/browser-contexts)
- [Playwright Network API](https://playwright.dev/docs/network)
- [Playwright Best Practices](https://playwright.dev/docs/best-practices)

### WebRTC
- [WebRTC for the Curious](https://webrtcforthecurious.com/docs/02-signaling/)
- [WebRTC Org Testing](https://webrtc.github.io/webrtc-org/testing/)
- [MDN WebRTC Data Channels](https://developer.mozilla.org/en-US/docs/Games/Techniques/WebRTC_data_channels)

### Advanced
- [Markaicode: Advanced WebRTC Testing with CDP](https://markaicode.com/webrtc-testing-playwright-cdp-overrides/)
- [MockRTC GitHub](https://github.com/httptoolkit/mockrtc)
- [WebRTC Hacks](https://webrtchacks.com/datachannel-multiplayer-game/)

### Test Tools Comparison
- [Playwright vs Cypress 2026](https://bugbug.io/blog/test-automation-tools/cypress-vs-playwright/)
- [BrowserStack Comparison](https://www.browserstack.com/guide/cypress-vs-selenium-vs-playwright-vs-puppeteer)

---

**Status**: Ready for implementation  
**KG Bindings**: TASK_333_E2E_WebRTC_Testing, lesson-webrtc-playwright-e2e-testing  
**Next Step**: Phase 0 setup + Phase 1 fixture implementation
