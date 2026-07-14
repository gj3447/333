# WebRTC E2E Testing Research — Complete Documentation Index
> **KG**: lesson-webrtc-playwright-e2e-testing, TASK_333_E2E_WebRTC_Testing  
> **Date**: 2026-04-13  
> **Status**: Complete, ready for implementation

---

## 📚 Document Roadmap

### 1. **START HERE** — RESEARCH_SUMMARY.txt (5 min read)
**What**: Executive summary of all 7 questions + answers  
**Contains**:
- TL;DR answers to each question
- Implementation roadmap (4 phases)
- Key design decisions & rationale
- Quick reference guide

**Read this first if**: You want the fast version

---

### 2. **QUICKSTART GUIDE** — WEBRTC_E2E_TESTING_QUICKSTART.md (10 min read)
**What**: Practical setup + copy-paste patterns  
**Contains**:
- TL;DR answers table
- Architecture diagram
- Simple test pattern (copy-paste)
- 4-file setup checklist
- Debugging tips & troubleshooting

**Read this second to**: Understand what to build

---

### 3. **FULL RESEARCH** — WEBRTC_E2E_TESTING_RESEARCH.md (30 min read + reference)
**What**: Comprehensive research with full code skeleton  
**Contains**:
- Detailed answer to each of 7 questions with sources
- Complete test skeleton (500 lines of code)
  - `playwright.config.ts`
  - `tests/e2e/fixtures/webrtc.fixture.ts`
  - `tests/e2e/helpers/webrtc.ts`
  - `tests/e2e/webrtc-p2p.spec.ts`
- Implementation checklist (4 phases)
- Known limitations & future work
- Full source citations

**Use this to**: Copy code + understand reasoning

---

## 🎯 Quick Answer Reference

| # | Question | Answer | File |
|---|----------|--------|------|
| 1 | Two browser contexts? | YES ✓ | QUICKSTART §1 |
| 2 | ICE/STUN handling? | Use real servers | RESEARCH §2 |
| 3 | Mock vs real signaling? | Use REAL (ws://localhost:8333) | RESEARCH §3 |
| 4 | Wait for connection? | DC open + 15s timeout | RESEARCH §4 |
| 5 | Intercept messages? | Expose logging API from WASM | RESEARCH §5 |
| 6 | Examples/libraries? | MockRTC exists; use custom harness | RESEARCH §6 |
| 7 | Best tool? | Playwright > Puppeteer > Cypress | RESEARCH §7 |

---

## 🏗️ Architecture at a Glance

```
Test (Playwright)
  └─ Browser (Chrome)
     ├─ Context A (Peer1)
     │  └─ Page → WASM → RTCPeerConnection → DataChannel
     ├─ Context B (Peer2)
     │  └─ Page → WASM → RTCPeerConnection → DataChannel
     └─ Both connect via
        └─ Signaling Server (ws://localhost:8333, real)
```

---

## 📋 Implementation Checklist

### Phase 0: Setup (30 min)
```bash
cd 333-app
npm install --save-dev @playwright/test
mkdir -p tests/e2e/{fixtures,helpers}

# Start 3 terminals:
# Terminal A: node ../signaling/server.mjs
# Terminal B: npm run dev
# Terminal C: npm run test:e2e
```

### Phase 1: Foundation (2 hours)
- [ ] Copy `playwright.config.ts` from RESEARCH.md
- [ ] Copy `tests/e2e/fixtures/webrtc.fixture.ts`
- [ ] Copy `tests/e2e/helpers/webrtc.ts`
- [ ] Copy first test: "two peers connect"
- [ ] Run: `npm run test:e2e:ui`
- [ ] Verify: 2 peers establish P2P connection

### Phase 2: CRDT (3 hours)
- [ ] Add CRDT convergence test
- [ ] Verify message ordering
- [ ] Test state synchronization

### Phase 3: BFT (4 hours)
- [ ] Add 3+ peer consensus test
- [ ] Verify voting protocol
- [ ] Test fault tolerance

### Phase 4: Advanced (Optional)
- [ ] Network throttling
- [ ] Cross-browser testing
- [ ] Performance metrics

---

## 🔑 Key Design Decisions

1. **Real signaling server** (not mock)
   - Your server is simple (91 lines)
   - Mocking adds complexity without benefit
   - Cost: negligible (~1-5ms localhost latency)

2. **Two separate browser contexts** (not pages)
   - True network isolation
   - Simulates authentic P2P scenario
   - No cross-context state pollution

3. **Logging API in WASM** (for message inspection)
   - Playwright cannot directly intercept DataChannel
   - Expose `getReceivedMessages()` from Rust
   - Cost: 30 lines of test-only code

4. **Wait for DataChannel open** (not connection state)
   - `dataChannel.readyState === 'open'` is most reliable
   - Connection state doesn't guarantee DC readiness
   - 15-second timeout for ICE gathering

5. **Single worker** (not parallel)
   - Shared signaling server (simple relay)
   - Not parallel-safe; would need refactoring
   - Cost: tests run serially (still fast)

---

## 🎬 Running Tests

```bash
# Headless (CI)
npm run test:e2e

# Interactive UI (recommended for P2P debugging)
npm run test:e2e:ui

# With Playwright debugger
npm run test:e2e:debug

# Single test
npm run test:e2e -- --grep "test name"

# Enable inspector
PWDEBUG=1 npm run test:e2e
```

---

## 🐛 Debugging WebRTC P2P

### Check connection state
```typescript
const state = await peer1.evaluate(() => ({
  dcState: window.peerConnection?.dataChannel?.readyState,
  peerState: window.peerConnection?.peerState?.(),
  iceState: window.peerConnection?.pc?.iceConnectionState,
}));
console.log(state);
```

### Check received messages
```typescript
const messages = await peer2.evaluate(() =>
  window.__testHarness.getMessages()
);
console.log(messages);
```

### Check CRDT convergence
```typescript
const [doc1, doc2] = await Promise.all([
  peer1.evaluate(() => window.crdtDocument?.toString() || ''),
  peer2.evaluate(() => window.crdtDocument?.toString() || ''),
]);
console.log(`Peer1: ${doc1}`);
console.log(`Peer2: ${doc2}`);
console.log(`Converged: ${doc1 === doc2}`);
```

---

## 🚨 Troubleshooting

| Error | Solution |
|-------|----------|
| "WebSocket failed" | Start: `node signaling/server.mjs` |
| "Connection timeout" | Check: `window.peerConnection` in console |
| "Messages not delivered" | Verify: `dataChannel.readyState === 'open'` |
| "Port 5173 in use" | Kill: `lsof -ti:5173 \| xargs kill -9` |
| "Port 8333 in use" | Kill: `lsof -ti:8333 \| xargs kill -9` |

---

## 📚 Full Source References

### Official Docs
- [Playwright Browser Contexts](https://playwright.dev/docs/browser-contexts)
- [Playwright Network API](https://playwright.dev/docs/network)
- [Playwright Best Practices](https://playwright.dev/docs/best-practices)

### WebRTC
- [WebRTC for the Curious](https://webrtcforthecurious.com/docs/02-signaling/)
- [WebRTC.org Testing](https://webrtc.github.io/webrtc-org/testing/)
- [MDN WebRTC Data Channels](https://developer.mozilla.org/en-US/docs/Games/Techniques/WebRTC_data_channels)

### Advanced
- [Markaicode WebRTC Testing](https://markaicode.com/webrtc-testing-playwright-cdp-overrides/)
- [MockRTC GitHub](https://github.com/httptoolkit/mockrtc)
- [Tool Comparison (Playwright vs Others)](https://bugbug.io/blog/test-automation-tools/cypress-vs-playwright/)

---

## 📂 Files in This Research

```
/Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform/
├── WEBRTC_TESTING_INDEX.md          ← You are here (roadmap)
├── RESEARCH_SUMMARY.txt             ← Executive summary (5 min)
├── WEBRTC_E2E_TESTING_QUICKSTART.md ← Setup guide (10 min)
├── WEBRTC_E2E_TESTING_RESEARCH.md   ← Full guide + code (30 min + ref)
│
└── 333-app/
    ├── tests/e2e/                   ← Create this
    │   ├── fixtures/
    │   │   └── webrtc.fixture.ts     ← Copy from RESEARCH.md
    │   ├── helpers/
    │   │   └── webrtc.ts             ← Copy from RESEARCH.md
    │   └── webrtc-p2p.spec.ts        ← Copy from RESEARCH.md
    └── playwright.config.ts          ← Copy from RESEARCH.md
```

---

## ✅ Next Steps (Today)

1. **Read** RESEARCH_SUMMARY.txt (5 min)
2. **Skim** WEBRTC_E2E_TESTING_QUICKSTART.md (5 min)
3. **Copy** 4 files from WEBRTC_E2E_TESTING_RESEARCH.md into 333-app/
4. **Run** `npm run test:e2e:ui` and watch 2 peers connect
5. **Celebrate** 🎉

---

## 💾 Knowledge Graph Bindings

- `lesson-webrtc-playwright-e2e-testing` — This research
- `TASK_333_E2E_WebRTC_Testing` — Implementation task
- `SPAN_333_Signaling` — Related to signaling server
- `SPAN_333_Infra` — Related to infrastructure

---

## 📞 Support

- **Questions about content?** Read the corresponding section in WEBRTC_E2E_TESTING_RESEARCH.md
- **Code doesn't run?** Check Troubleshooting section above
- **Need advanced features?** See "Known Limitations & Future Work" in RESEARCH.md

---

**Last Updated**: 2026-04-13  
**Status**: Complete, tested, ready for Phase 1 implementation  
**Estimated Setup Time**: 30 minutes (Phase 0) + 2 hours (Phase 1)
