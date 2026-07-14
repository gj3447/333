# P2P Notifications: Technical Reference & Code Patterns
# KG: RESEARCH_333_P2PNotifications_Technical

**Date**: 2026-04-13  
**Purpose**: Implementation guide for in-app peer messaging (DataChannel + optional Notification API)

---

## 1. WebRTC DataChannel Messaging (Primary)

### Why DataChannel for Notifications?
- Direct peer-to-peer (no relay needed)
- Encrypted by DTLS automatically
- Ordered delivery (perfect for notifications)
- Works during active connection
- Part of existing 333 architecture

### Code Pattern (TypeScript)
```typescript
// Peer A sends notification to Peer B
const notifyPeer = (peerId: string, message: string) => {
  const notification = {
    type: 'notification',
    id: crypto.randomUUID(),
    timestamp: Date.now(),
    text: message,
  };
  
  const dataChannel = peerConnections.get(peerId)?.dataChannel;
  if (dataChannel?.readyState === 'open') {
    dataChannel.send(JSON.stringify(notification));
  } else {
    // Queue for later (see section 3)
    queueOfflineNotification(peerId, notification);
  }
};

// Peer B receives and displays
dataChannel.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  if (msg.type === 'notification') {
    showLocalNotification(msg.text);  // In-app toast/badge
  }
};

const showLocalNotification = (text: string) => {
  // React/Svelte: setState or Svelte store update
  // No Notification API, just DOM
  displayToast(text, { duration: 5000, variant: 'info' });
};
```

### Limitations
- Only works while peer is online
- Only works while browser tab is open (no background worker for messages)
- No silent notification (requires visual indicator in tab)

---

## 2. Notification API (Optional Bonus, Foreground Only)

### When to Use
- Tab is open (Notification.permission already granted)
- Additional UX polish (system-level alert)

### Code Pattern
```typescript
const sendNotificationIfPermitted = (title: string, text: string) => {
  if (Notification.permission === 'granted') {
    new Notification(title, {
      body: text,
      icon: '/logo.png',
      tag: 'message',  // Replace previous notification with same tag
    });
  }
};

// On app init: request permission once
if ('Notification' in window && Notification.permission === 'default') {
  Notification.requestPermission().then(permission => {
    console.log('Notification permission:', permission);
  });
}

// Send via DataChannel + Notification API
const notifyPeerWithNotification = (peerId: string, message: string) => {
  const notification = {
    type: 'notification',
    id: crypto.randomUUID(),
    timestamp: Date.now(),
    text: message,
  };
  
  const dataChannel = peerConnections.get(peerId)?.dataChannel;
  if (dataChannel?.readyState === 'open') {
    dataChannel.send(JSON.stringify(notification));
    // BONUS: also show Notification API alert
    sendNotificationIfPermitted('Message', message);
  } else {
    queueOfflineNotification(peerId, notification);
  }
};
```

### Limitations
- Only works if tab is **open** (not background)
- Requires user permission (one-time prompt)
- Only shows while browser has focus or periodically

---

## 3. Offline Queue (IndexedDB)

### Why Queue?
- Peer goes offline → store notification
- Peer comes back online → replay from queue

### Code Pattern
```typescript
// IndexedDB schema
const dbConfig = {
  name: 'room333-notifications',
  version: 1,
  stores: {
    'offline-notifications': { keyPath: 'id' },
  },
};

const queueOfflineNotification = async (peerId: string, notif: any) => {
  const db = await openIndexedDB(dbConfig);
  const tx = db.transaction(['offline-notifications'], 'readwrite');
  const store = tx.objectStore('offline-notifications');
  
  await store.add({
    id: notif.id,
    peerId,
    notification: notif,
    queuedAt: Date.now(),
  });
};

const replayQueuedNotifications = async (peerId: string) => {
  const db = await openIndexedDB(dbConfig);
  const tx = db.transaction(['offline-notifications'], 'readwrite');
  const store = tx.objectStore('offline-notifications');
  
  // Get all notifications for this peer
  const allNotifs = await store.getAll();
  const forPeer = allNotifs.filter(n => n.peerId === peerId);
  
  for (const item of forPeer) {
    // Send via DataChannel
    const dataChannel = peerConnections.get(peerId)?.dataChannel;
    if (dataChannel?.readyState === 'open') {
      dataChannel.send(JSON.stringify(item.notification));
      await store.delete(item.id);  // Remove after sending
    }
  }
};

// Call on peer reconnect
onPeerReconnect((peerId: string) => {
  replayQueuedNotifications(peerId);
});
```

---

## 4. Design Tradeoffs

| Aspect | Accepted Limitation |
|--------|---|
| Offline notifications | None — messages lost if peer offline |
| Background delivery | None — tab must be open |
| Delivery guarantee | Best-effort via DataChannel ordering |
| Encryption | DTLS (automatic) |
| Latency | <100ms (peer-to-peer) |
| Infrastructure | Zero (pure P2P) |

---

## 5. Why NOT Web Push API for 333

**Web Push API Architecture**:
```
Peer A → [your server] → [browser vendor push service] → Peer B device
```

**Problems for 333**:
1. Requires VAPID keys (server-side only, can't be peer-held)
2. Requires registering endpoint with centralized service
3. Breaks "no server" philosophy
4. Adds latency (2-hop relay)
5. Domain isolation prevents peer-to-peer push

**Better for P2P**: SimpleX, Briar, Jami, Signal all use **in-app messages + optional notification API**, not Web Push.

---

## 6. Implementation Checklist

- [ ] Add notification message type to wire protocol
- [ ] Implement DataChannel sender (notifyPeer function)
- [ ] Implement DataChannel receiver (showLocalNotification)
- [ ] Add Notification API request on app init
- [ ] Create IndexedDB offline queue schema
- [ ] Implement queueOfflineNotification
- [ ] Implement replayQueuedNotifications
- [ ] Call replay on peer reconnect
- [ ] Test: send notification while peer connected (should arrive <100ms)
- [ ] Test: queue notification while peer offline, reconnect, verify replay
- [ ] Test: Notification API alert shows on MacOS/Linux/Windows
- [ ] E2E: send 100 notifications, verify all arrive

---

## References

- [MDN: Using the Notifications API](https://developer.mozilla.org/en-US/docs/Web/API/Notifications_API/Using_the_Notifications_API)
- [MDN: WebRTC Data Channels](https://developer.mozilla.org/en-US/docs/Web/API/WebRTC_API/Using_data_channels)
- [W3C: Notifications Spec](https://notifications.spec.whatwg.org/)
- [W3C: Push API Spec](https://www.w3.org/TR/push-api/)

---

*Technical reference complete. Ready for ST → SCW phases.*
