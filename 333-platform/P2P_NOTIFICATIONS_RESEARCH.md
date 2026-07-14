# P2P Push Notifications Research: Serverless Messaging for 333
# KG: RESEARCH_333_P2PNotifications

**Date**: 2026-04-13  
**Status**: Research Complete  
**Word Limit**: 300 words (executive summary)  
**Scope**: 333 Platform "no server" philosophy vs. notification realities

---

## Executive Summary

**Cannot do true peer-to-peer push notifications without infrastructure.**

Web Push API requires a centralized push service (browser vendor controlled: Chrome, Firefox, Safari push servers). No peer can serve as your push service — it violates domain isolation and VAPID key security (which must stay server-side).

**For 333 Platform, three choices:**

### 1. **Notification API (Foreground Only)** ✅ Serverless
- Users see alert **only if tab is open**
- Uses `Notification.requestPermission()` + `new Notification()`
- No server needed, works P2P
- **Tradeoff**: Useless when peer is offline or browser closed

### 2. **WebRTC DataChannel Messages (In-App)** ✅ Serverless
- When peer is connected, send direct message via DataChannel
- App handles notification UI (custom toast/badge)
- **Tradeoff**: Only works while peer is online; breaks if browser closes

### 3. **Minimal Relay Server + Web Push** ⚠️ Hybrid (Breaks "No Server" Philosophy)
- User's device registers endpoint once with server
- Peer sends server a message (server pushes to recipient's endpoint)
- **Tradeoff**: Requires infrastructure; solves offline notifications

---

## Alternatives Explored

| Approach | Offline? | Server Needed? | Peer-to-Peer? |
|----------|----------|---|---|
| Web Push API | ✅ Yes | ❌ Yes (vendor + yours) | ❌ No |
| Notification API | ❌ No | ✅ No | ✅ Yes |
| DataChannel | ❌ No | ✅ No | ✅ Yes |
| IPFS/Holochain/Matrix | ⚠️ Partial | ⚠️ Nodes required | ⚠️ Partial |

---

## Recommendation for 333

**Hybrid approach**: DataChannel (in-app) + optional Notification API.

**Implementation**:
1. Send direct peer message via DataChannel (instant, requires connection)
2. When DataChannel unavailable, queue message locally (IndexedDB)
3. Optionally: show Notification API alert when tab open (bonus UX)
4. Accept offline limitation as design constraint of P2P

**Rationale**: Aligns with "no server" philosophy. Trade offline notifications for architectural purity. Real P2P apps (Briar, SimpleX, Jami) make same choice.

---

## KG References
- **333 Platform**: Decentralized, P2P-first WebRTC chat
- **lesson-333-server-requirement**: P2P has unavoidable infrastructure tradeoffs
- **Design Decision**: Offline notifications incompatible with zero-server architecture

---

*Research complete. Ready for architecture decision in APT ST phase.*
