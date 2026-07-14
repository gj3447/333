# 333 Platform: P2P Testing Documentation

> WebRTC + CRDT + BFT Testing Framework

## 📚 Documentation Map

### Quick Start (Start Here)
- **[TESTING_QUICKSTART.md](./TESTING_QUICKSTART.md)** — 5-minute setup guide
  - Terminal setup: signaling + app + browser
  - Manual 2-tab test
  - Automated smoke test
  - Docker option

### Comprehensive Testing Strategy
- **[P2P_TESTING_STRATEGY.md](./P2P_TESTING_STRATEGY.md)** — Full 80+ page guide covering:

  **Phase 1: Manual Testing**
  - Connection order checklist (Signaling → Peer Discovery → ICE → SDP → DataChannel → CRDT → BFT)
  - 8-step verification procedure
  - Network failure simulation

  **Phase 2: Debug UI Enhancements**
  - Connection state panel (status, peer count, delta info)
  - Styled indicators (green/yellow/red)
  - Metrics display (message rate, BFT round)

  **Phase 3: Console Logging Instrumentation**
  - WebRTC event logging (WS, ICE, DataChannel, PEER state)
  - Timestamped output format
  - Before/after examples

  **Phase 4: Single-Machine Testing Gotchas**
  - 2 tabs vs 2 browsers (pros/cons)
  - Localhost STUN issues (fix included)
  - Setup for each scenario

  **Phase 5: Automated Smoke Test**
  - Puppeteer-core script (6.9KB)
  - Test for: connection, peer discovery, DataChannel, CRDT
  - No Playwright overhead

  **Phase 6: CI Integration**
  - GitHub Actions workflow (multiversion testing)
  - Docker Compose for local CI
  - Service healthchecks

  **Phase 7: Test Coverage Checklist**
  - Manual tests (must pass)
  - Automated tests (smoke + load)
  - Edge cases (dropout, split, burst)

### Testing Checklist
- **[TESTING_CHECKLIST.md](./TESTING_CHECKLIST.md)** — Ready-to-use checklists
  - Pre-test setup
  - Connection tests
  - DataChannel tests
  - CRDT tests (convergence, tombstones)
  - Failure scenarios (offline, dropout)
  - Console logging reference
  - Common issues & fixes
  - Test coverage matrix
  - Test results template

---

## 🛠 Implementation Files

### Test Scripts

**[tests/p2p-smoke.js](./tests/p2p-smoke.js)** (6.9 KB)
- Puppeteer-core automation
- Opens 2 headless tabs
- Tests: peer connection, block exchange, CRDT replication
- No JavaScript errors check
- CI-safe (headless mode)
- Usage: `node tests/p2p-smoke.js ws://localhost:8333`

### CI/CD

**[.github/workflows/p2p-test.yml](../.github/workflows/p2p-test.yml)** (2.9 KB)
- GitHub Actions workflow
- Runs on: push (main/develop), pull requests, manual dispatch
- Matrix: Node 18 + 20
- Services: signaling (8333), app (5173)
- Auto-cleanup of processes
- Status check job

**[docker-compose.test.yml](./docker-compose.test.yml)** (1.8 KB)
- Complete stack in containers
- Services: signaling, app, test
- Healthchecks on all services
- Usage: `docker-compose -f docker-compose.test.yml up --profile test`

---

## 📋 Quick Reference

### Test Phases Summary

| Phase | Duration | Effort | Coverage | Manual |
|-------|----------|--------|----------|--------|
| 1: Manual | 5-10min | Low | WS+Peer+ICE+DC+CRDT | Required |
| 2: Debug UI | 2-4 hrs | Medium | Visual indicators | Optional |
| 3: Logging | 1-2 hrs | Low | Event tracing | Optional |
| 4: Single-Machine | 10 min | Low | localhost gotchas | Required |
| 5: Automation | 1-2 hrs | Low | Smoke test CI | Recommended |
| 6: CI/CD | 1-2 hrs | Medium | GitHub Actions | Recommended |
| 7: Scale | 4-8 hrs | Medium | 3+ peers, BFT | Advanced |

### Signaling Server

```bash
# Start: Terminal A
node signaling/server.mjs 8333

# Server: WebSocket relay (no state, no auth)
# Routes: join, offer, answer, ice, broadcast
# Max peers: Unlimited (memory-bound)
# Logs: [room] peer joined/left events
```

**Key Files:**
- `signaling/server.mjs` — 91 lines, minimal, WebSocket.Server

### Frontend App

```bash
# Start: Terminal B
cd 333-app && npm run dev

# App: SvelteKit 2.0
# Page: /room (create/join room)
# State: RTCPeerConnection + DataChannel per peer
# CRDT: LWW-Map (Last-Writer-Wins) for block state
```

**Key Files:**
- `333-app/src/lib/room-state.ts` — RTCPeerConnection setup (136 lines)
- `333-app/src/routes/room/+page.svelte` — Room UI + demo (block grid)

### Test Execution

```bash
# Manual (Tabs in browser)
http://localhost:5173/room (Tab 1: Create)
http://localhost:5173/room?id=ABC123 (Tab 2: Join)

# Automated (CLI)
node tests/p2p-smoke.js ws://localhost:8333

# Docker
docker-compose -f docker-compose.test.yml up

# CI (automatic)
git push → GitHub Actions → Matrix (Node 18, 20)
```

---

## 🎯 Practical Workflow

### For Development

```bash
# Session start
node signaling/server.mjs 8333 &
cd 333-app && npm run dev &

# Manual test (2 tabs)
# Edit code → Vite hot-reload → Refresh tabs → Re-test

# Before commit
npm install --save-dev puppeteer-core
node tests/p2p-smoke.js ws://localhost:8333
# Expect: "=== ALL SMOKE TESTS PASSED ==="
```

### For Code Review

```bash
# Reviewer: Run smoke test locally
node tests/p2p-smoke.js ws://localhost:8333

# Or: Use Docker
docker-compose -f docker-compose.test.yml up

# Or: Wait for GitHub Actions to pass
# https://github.com/YOUR_ORG/333-platform/actions
```

### For Regression Testing

```bash
# Automated CI runs on every push to main/develop
# Manual scale testing (3+ peers) on release milestones
# Performance baseline established via load test (Phase 7)
```

---

## 🔍 What Gets Tested

### Connection Layer
```
✓ WebSocket signaling (join/leave)
✓ Peer discovery (who joined room)
✓ ICE candidate gathering
✓ SDP offer/answer exchange
```

### Transport Layer
```
✓ RTCPeerConnection creation
✓ RTCDataChannel open/close
✓ Binary message transmission
✓ Channel buffering (sendBuffer)
```

### CRDT Layer
```
✓ Last-Writer-Wins replication
✓ Concurrent writes convergence
✓ Tombstone handling (deletions)
✓ Delta merging
```

### Application Layer (Block Grid Demo)
```
✓ Block placement (CRDT mutation)
✓ Block deletion (tombstone)
✓ Grid convergence (3+ peers)
✓ Message serialization
```

### BFT Layer (Planned)
```
✓ Voting round initiation
✓ Consensus reaching (majority)
✓ Byzantine peer handling
✓ View change on leader failure
```

---

## 🚀 Incremental Implementation Timeline

**Week 1: Manual Testing**
- [ ] Review TESTING_QUICKSTART.md
- [ ] Run manual 2-tab test
- [ ] Add console logging (Phase 3)
- [ ] Verify connection order (Phase 1)

**Week 2: Automation**
- [ ] Add debug UI (Phase 2)
- [ ] Create smoke test (Phase 5)
- [ ] Test locally: `node tests/p2p-smoke.js`
- [ ] Document test IDs in HTML

**Week 3: CI/CD**
- [ ] Set up GitHub Actions (Phase 6.1)
- [ ] Set up Docker Compose (Phase 6.2)
- [ ] Verify on every push
- [ ] Add to PR checks

**Week 4+: Scale & Advanced**
- [ ] Load test (10 peers)
- [ ] Failure scenarios (network split, Byzantine)
- [ ] Performance baseline
- [ ] Stress test (1000 blocks/s)

---

## 📞 Support

### Debugging Tips

1. **Connection not establishing?**
   - Check console: "WS: connected"?
   - Check: `nc -zv localhost 8333`
   - Check firewall: port 8333 open?

2. **Block not syncing?**
   - Check console: "DC: opened"?
   - Check console: "DC: received NNN bytes"?
   - Wait 1 second (async replication)

3. **Smoke test failing?**
   - Run: `node tests/p2p-smoke.js` with app running
   - Check: "npm install --save-dev puppeteer-core"
   - Check: "http://localhost:5173" loads

4. **Docker test not working?**
   - Prune images: `docker system prune`
   - Rebuild: `docker-compose build --no-cache`
   - Check logs: `docker-compose logs -f`

### Troubleshooting Matrix

| Symptom | Check | Fix |
|---------|-------|-----|
| "Cannot find module 'ws'" | signaling/package.json | `cd signaling && npm install` |
| "Port 8333 already in use" | `lsof -i :8333` | `killall node` or use different port |
| "Peers don't connect" | Console: WS connected? | Wait 3s for ICE, refresh tab |
| "Block sync fails" | Console: DC open? | Check message size < 65KB |
| "Smoke test timeout" | Services running? | `nc -zv localhost 5173 8333` |

---

## 📖 References

### WebRTC Standards
- [MDN: RTCPeerConnection](https://developer.mozilla.org/en-US/docs/Web/API/RTCPeerConnection)
- [MDN: RTCDataChannel](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel)
- [RFC 5245: ICE](https://tools.ietf.org/html/rfc5245)

### CRDT & Distributed Systems
- [CRDT Papers](https://crdt.tech/)
- [Last-Writer-Wins Map](https://arxiv.org/abs/1805.06358)
- [Conflict-free RDTs](https://arxiv.org/abs/1805.06358)

### Testing Tools
- [Puppeteer](https://pptr.dev/) (full browser automation)
- [puppeteer-core](https://github.com/puppeteer/puppeteer/tree/main/packages/puppeteer-core) (lightweight)
- [GitHub Actions](https://docs.github.com/en/actions)
- [Docker Compose](https://docs.docker.com/compose/)

### 333 Platform
- **Signaling**: `signaling/server.mjs` (91 lines, stateless relay)
- **Frontend**: `333-app/src/routes/room/+page.svelte` (main UI)
- **CRDT**: `src/lww_map.rs` (Rust implementation)
- **BFT**: `src/bft/state.rs` (Byzantine Fault Tolerance)

---

## 🏁 Summary

This testing framework provides:

1. **Manual verification** — Visual, step-by-step testing (5-10 min)
2. **Instrumentation** — Console logs + debug UI for visibility
3. **Automation** — Puppeteer smoke test (0-touch testing)
4. **CI/CD** — GitHub Actions + Docker for continuous verification
5. **Documentation** — Guides for every phase + troubleshooting

**Start with** `TESTING_QUICKSTART.md`, then progress to full strategy as needed.

---

**Created: 2026-04-13**  
**KG: WORKFLOW_333_P2P_Testing**  
**Status: Ready for implementation**
