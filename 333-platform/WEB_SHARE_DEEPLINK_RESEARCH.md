# Web Share API + Deep Linking for P2P Room Invitations
## Research Findings — 333 Platform

# KG: 333-platform-web-share-deeplink-research-2026-04-13

---

## 1. Web Share API (navigator.share)

**Native Share Sheet Integration:**
- `navigator.share({title, text, url})` invokes OS-native share UI (iOS Share Sheet, Android ShareSheet)
- Requires transient user activation (click event only, no auto-invoke)
- Target URL passed: `https://333.app/room?id=<roomId>&key=<inviteKey>`
- Browser support: ~92% mobile (iOS 13+, Android 6+), desktop limited (Windows/Linux/macOS have no native UI)
- Fallback: Clipboard API for unsupported devices

**Implementation:**
```typescript
// Share room invitation
if (navigator.share) {
  await navigator.share({
    title: "Join 333 Room",
    text: `Join my room: ${roomName}`,
    url: `https://333.app/room?id=${roomId}&key=${inviteKey}`
  });
}
```

---

## 2. Deep Link URL Format

**Recommended Structure:**
```
https://333.app/room?id=<roomId>&key=<inviteKey>&name=<displayName>
```
- `id`: Room identifier (alphanumeric, 12-16 chars, URL-safe)
- `key`: Invite token (HMAC-signed ephemeral, 10min TTL)
- `name`: Pre-populated display name (optional)

---

## 3. Platform-Specific Verification

**iOS Universal Links:**
- Host verification file: `https://333.app/.well-known/apple-app-site-association`
- JSON format:
```json
{
  "applinks": {
    "apps": [],
    "details": [
      {
        "appID": "TEAM_ID.com.333.app",
        "paths": ["/room*"]
      }
    ]
  }
}
```
- Content-Type: `application/json`, no redirects

**Android App Links:**
- Host: `https://333.app/.well-known/assetlinks.json`
- Requires SHA256 fingerprint of signing certificate
- Format:
```json
[{
  "relation": ["delegate_permission/common.handle_all_urls"],
  "target": {
    "namespace": "android_app",
    "package_name": "com.triple333.app",
    "sha256_cert_fingerprints": ["AA:BB:CC:..."]
  }
}]
```

---

## 4. Tauri Integration (PWA + Desktop)

**Tauri Deep Linking Plugin (@tauri-apps/plugin-deep-link):**
- Configure in `tauri.conf.json`:
```json
{
  "plugins": {
    "deep-link": {
      "mobile": {
        "domains": ["333.app"],
        "paths": {"/room": {"multipleInstances": false}}
      },
      "desktop": {
        "schemes": ["333"]
      }
    }
  }
}
```
- Handler: `onOpenUrl()` callback fires on deep link activation
- Desktop fallback: Custom protocol `333://room?id=...`

---

## 5. Room Lifecycle (Invitation → Join)

1. Host generates room (UUID) + ephemeral invite token (HMAC + 10min TTL)
2. User clicks "Share" → `navigator.share()` sends URL via native apps
3. Recipient clicks link → OS routes to app (or fallback to web)
4. App validates token (TTL + HMAC signature)
5. WebRTC peer connection established + CRDT sync begins

---

## References

- [Web Share API - MDN](https://developer.mozilla.org/en-US/docs/Web/API/Navigator/share)
- [Apple Universal Links](https://developer.apple.com/documentation/xcode/allowing-apps-and-websites-to-link-to-your-content/)
- [Android App Links](https://developer.android.com/training/app-links)
- [Tauri Deep Linking Plugin](https://v2.tauri.app/plugin/deep-linking/)
- [2026 Deep Linking Guide](https://app.smler.io/blogs/deep-linking/how-deep-linking-works-complete-technical-guide-2026)

---

**Status:** ✓ Ready for Tauri + Web integration. Verify signing certificates before deploying assetlinks.json.
