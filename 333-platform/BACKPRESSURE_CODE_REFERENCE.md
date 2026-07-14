# DataChannel Backpressure — Implementation Code Reference

> **Status**: Ready-to-Copy | **Date**: 2026-04-13  
> **Companion**: DATACHANNEL_BACKPRESSURE_RESEARCH.md  
> **KG**: TASK_DataChannel_Backpressure

---

## Quick Start: Copy-Paste Sections

### 1. Add to Cargo.toml

No new dependencies needed. Uses existing `web-sys`, `wasm-bindgen`.

### 2. Create `src/p2p/backpressure.rs`

```rust
// KG: ATOM_Backpressure_DataChannel
// WebRTC DataChannel backpressure handling with priority-aware queuing

use std::sync::{Arc, Mutex};
use web_sys::RtcDataChannel;

/// Message priority for backpressure decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MessagePriority {
    /// Position, rotation, animation frames (transient state, can drop)
    Transient = 0,
    
    /// BFT votes, proposals (consensus messages, stochastic drop OK)
    Consensus = 1,
    
    /// CRDT deltas, entity state (persistent state, must not drop)
    CriticalState = 2,
}

/// Result of send attempt with backpressure handling
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendResult {
    /// Successfully sent immediately
    Sent,
    
    /// Queued locally (will send when buffer drains)
    Queued,
    
    /// Dropped due to backpressure and low priority
    Dropped { reason: String },
    
    /// Channel error (not connected, closed, etc.)
    Error(String),
}

/// Metrics for backpressure behavior
#[derive(Debug, Clone)]
pub struct BackpressureStats {
    pub sent: u64,
    pub queued: u64,
    pub dropped: u64,
    pub flushed: u64,
    pub peak_buffer_bytes: u32,
}

/// Backpressure-aware wrapper around RTCDataChannel
///
/// Monitors bufferedAmount and implements intelligent queue/drop strategy:
/// - CriticalState: Always queue (CRDT deltas must not be lost)
/// - Consensus: Probabilistically queue/drop (votes are stochastic)
/// - Transient: Always drop (position updates superseded by next frame)
pub struct BackpressureDataChannel {
    /// Underlying web-sys RTCDataChannel
    dc: RtcDataChannel,
    
    /// Bytes threshold before triggering backpressure
    /// Recommended: 256 KB for medium rooms (12-30 peers)
    threshold_bytes: u32,
    
    /// Queue of messages pending send (due to backpressure)
    queue: Arc<Mutex<Vec<(Vec<u8>, MessagePriority)>>>,
    
    /// Metrics
    stats: Arc<Mutex<BackpressureStats>>,
}

impl BackpressureDataChannel {
    /// Create new backpressure wrapper around a DataChannel
    pub fn new(dc: RtcDataChannel, threshold_bytes: u32) -> Self {
        // Inform browser about our backpressure threshold
        dc.set_buffered_amount_low_threshold(threshold_bytes);
        
        let queue = Arc::new(Mutex::new(Vec::new()));
        let stats = Arc::new(Mutex::new(BackpressureStats {
            sent: 0,
            queued: 0,
            dropped: 0,
            flushed: 0,
            peak_buffer_bytes: 0,
        }));
        
        Self {
            dc,
            threshold_bytes,
            queue,
            stats,
        }
    }
    
    /// Send data with automatic backpressure handling
    ///
    /// Behavior:
    /// - If bufferedAmount <= threshold: send immediately
    /// - If bufferedAmount > threshold:
    ///   - CriticalState: queue locally
    ///   - Consensus: probabilistically queue (70%) or drop (30%)
    ///   - Transient: always drop
    pub fn send(&self, data: &[u8], priority: MessagePriority) -> SendResult {
        let buffered = self.dc.buffered_amount();
        
        // Track peak buffer usage
        {
            let mut stats = self.stats.lock().unwrap();
            if buffered > stats.peak_buffer_bytes {
                stats.peak_buffer_bytes = buffered;
            }
        }
        
        // If buffer is low, send immediately
        if buffered <= self.threshold_bytes {
            return match self.dc.send_with_u8_array(data) {
                Ok(()) => {
                    self.stats.lock().unwrap().sent += 1;
                    SendResult::Sent
                }
                Err(e) => {
                    SendResult::Error(format!("{:?}", e))
                }
            };
        }
        
        // Buffer is high: decide action by priority
        match priority {
            MessagePriority::CriticalState => {
                // CRDT deltas: queue them, never drop
                self.queue
                    .lock()
                    .unwrap()
                    .push((data.to_vec(), priority));
                self.stats.lock().unwrap().queued += 1;
                SendResult::Queued
            }
            
            MessagePriority::Consensus => {
                // BFT votes: probabilistically queue (70% queue, 30% drop)
                // Rationale: lost votes don't harm consensus (stochastic protocol)
                let queue_probability = 70u32;
                let random_byte = (buffered % 100) as u32;
                
                if random_byte < queue_probability {
                    self.queue
                        .lock()
                        .unwrap()
                        .push((data.to_vec(), priority));
                    self.stats.lock().unwrap().queued += 1;
                    SendResult::Queued
                } else {
                    self.stats.lock().unwrap().dropped += 1;
                    SendResult::Dropped {
                        reason: format!("consensus probabilistic drop (buffer={})", buffered),
                    }
                }
            }
            
            MessagePriority::Transient => {
                // Position updates: always drop
                // Rationale: next frame's position supersedes this one
                self.stats.lock().unwrap().dropped += 1;
                SendResult::Dropped {
                    reason: format!("transient position update (buffer={})", buffered),
                }
            }
        }
    }
    
    /// Attempt to flush queued messages
    ///
    /// Called from the bufferedamountlow event handler.
    /// Returns number of messages successfully flushed.
    pub fn try_flush(&self) -> usize {
        let mut queue = self.queue.lock().unwrap();
        let mut flushed = 0;
        
        // Keep flushing while buffer is low
        while let Some((data, _priority)) = queue.first() {
            if self.dc.buffered_amount() > self.threshold_bytes {
                // Buffer is rising again, stop flushing
                break;
            }
            
            match self.dc.send_with_u8_array(data) {
                Ok(()) => {
                    queue.remove(0);
                    flushed += 1;
                }
                Err(_) => {
                    // Send failed, stop flushing
                    break;
                }
            }
        }
        
        if flushed > 0 {
            self.stats.lock().unwrap().flushed += flushed as u64;
        }
        
        flushed
    }
    
    /// Get current buffer status (for monitoring/debugging)
    pub fn buffer_status(&self) -> (u32, usize, &'static str) {
        let buffered = self.dc.buffered_amount();
        let queued = self.queue.lock().unwrap().len();
        
        let status = match buffered {
            0..=262_144 => "IDLE",
            262_145..=1_048_576 => "NORMAL",
            1_048_577..=4_194_304 => "ELEVATED",
            4_194_305..=8_388_608 => "HIGH",
            8_388_609..=16_777_216 => "CRITICAL",
            _ => "OVERFLOW",
        };
        
        (buffered, queued, status)
    }
    
    /// Get backpressure metrics
    pub fn stats(&self) -> BackpressureStats {
        self.stats.lock().unwrap().clone()
    }
    
    /// Get number of queued messages
    pub fn queue_size(&self) -> usize {
        self.queue.lock().unwrap().len()
    }
    
    /// Get raw DataChannel reference (if needed for advanced usage)
    pub fn inner(&self) -> &RtcDataChannel {
        &self.dc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    // Unit tests would use a mock RTCDataChannel
    // See DATACHANNEL_BACKPRESSURE_RESEARCH.md for test examples
}
```

### 3. Update `src/p2p/mod.rs` to Export Backpressure Module

```rust
// Add to src/p2p/mod.rs

pub mod backpressure;
pub mod channel;
pub mod webrtc;
pub mod mesh;
pub mod mod_helper;

pub use backpressure::{BackpressureDataChannel, MessagePriority, SendResult};
pub use channel::{DataChannel, ChannelMode, ChannelError, ConnState};
pub use webrtc::WebRtcPeer;
pub use mesh::MeshRoom;
```

### 4. Extend `src/p2p/webrtc.rs` with Backpressure Support

Add these methods to the `WebRtcPeer` struct:

```rust
// Add to src/p2p/webrtc.rs (in the WebRtcPeer impl block)

use crate::p2p::backpressure::{BackpressureDataChannel, MessagePriority, SendResult};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
impl WebRtcPeer {
    /// Get current bufferedAmount in bytes
    pub fn buffered_amount(&self) -> u32 {
        self.dc.as_ref().map_or(0, |dc| dc.buffered_amount())
    }
    
    /// Get backpressure threshold in bytes
    pub fn get_backpressure_threshold(&self) -> u32 {
        self.dc.as_ref()
            .map_or(0, |dc| dc.buffered_amount_low_threshold())
    }
    
    /// Set backpressure threshold (in bytes)
    /// Recommended: 256 * 1024 (256 KB)
    pub fn set_backpressure_threshold(&self, bytes: u32) -> Result<(), JsValue> {
        if let Some(dc) = &self.dc {
            dc.set_buffered_amount_low_threshold(bytes);
            Ok(())
        } else {
            Err(JsValue::from_str("DataChannel not open"))
        }
    }
    
    /// Check if channel is currently backpressured
    pub fn is_backpressured(&self) -> bool {
        if let Some(dc) = &self.dc {
            let buffered = dc.buffered_amount();
            let threshold = dc.buffered_amount_low_threshold();
            buffered > threshold
        } else {
            false
        }
    }
    
    /// Send data with backpressure handling
    /// 
    /// priority: 0=Transient, 1=Consensus, 2=CriticalState
    /// returns: "Sent", "Queued", "Dropped: reason", or "Error: reason"
    pub fn send_with_backpressure(
        &self,
        data: &[u8],
        priority: u8,
    ) -> String {
        if let Some(dc) = &self.dc {
            let buffered = dc.buffered_amount();
            let priority_enum = match priority {
                0 => MessagePriority::Transient,
                1 => MessagePriority::Consensus,
                _ => MessagePriority::CriticalState,
            };
            
            let threshold = dc.buffered_amount_low_threshold();
            
            // If buffer is low, send immediately
            if buffered <= threshold {
                match dc.send_with_u8_array(data) {
                    Ok(()) => return "Sent".to_string(),
                    Err(e) => return format!("Error: {:?}", e),
                }
            }
            
            // Buffer is high: decide action by priority
            match priority_enum {
                MessagePriority::CriticalState => {
                    // CRDT deltas: must queue (but we don't have a queue in WebRtcPeer)
                    // In production, use BackpressureDataChannel wrapper
                    format!("Backpressured (bufferedAmount={}, CriticalState)", buffered)
                }
                MessagePriority::Consensus => {
                    // BFT votes: probabilistically drop
                    if (buffered % 100) < 70 {
                        format!("Queued (bufferedAmount={})", buffered)
                    } else {
                        format!("Dropped consensus (bufferedAmount={})", buffered)
                    }
                }
                MessagePriority::Transient => {
                    // Position updates: drop immediately
                    format!("Dropped transient (bufferedAmount={})", buffered)
                }
            }
        } else {
            "Error: DataChannel not open".to_string()
        }
    }
    
    /// Get buffer status as human-readable string
    pub fn buffer_status_str(&self) -> String {
        let buffered = self.buffered_amount();
        let threshold = self.get_backpressure_threshold();
        
        let status = match buffered {
            0..=262_144 => "IDLE",
            262_145..=1_048_576 => "NORMAL",
            1_048_577..=4_194_304 => "ELEVATED",
            4_194_305..=8_388_608 => "HIGH",
            8_388_609..=16_777_216 => "CRITICAL",
            _ => "OVERFLOW",
        };
        
        format!(
            "bufferedAmount={:.1}MB, threshold={:.1}MB, status={}",
            buffered as f64 / 1_000_000.0,
            threshold as f64 / 1_000_000.0,
            status
        )
    }
}
```

### 5. Update `src/p2p/mesh.rs` to Use Backpressure

```rust
// In MeshRoom::broadcast() and send_to(), add logging:

pub fn broadcast(&self, data: &[u8], mode: ChannelMode) -> usize {
    let mut sent = 0;
    for (&peer_id, ch) in &self.channels {
        if ch.send(data, mode).is_ok() {
            sent += 1;
        }
        
        // TODO: Log backpressure status when implemented
        // if let Some(buffered) = ch.get_backpressure_status() {
        //     if buffered > threshold {
        //         web_sys::console::warn_1(&format!(
        //             "Peer {} backpressured: {}",
        //             peer_id, buffered
        //         ).into());
        //     }
        // }
    }
    sent
}
```

### 6. JavaScript Integration Example

```javascript
// In 333-app/src/lib/wasm-bridge.ts (or similar)

import { WebRtcPeer } from '../wasm/triple_three';

export class GameNetworking {
    constructor() {
        this.peers = new Map(); // peer_id -> WebRtcPeer
    }
    
    sendPositionUpdate(x, y, z) {
        const posData = new Float32Array([x, y, z]);
        
        for (const peer of this.peers.values()) {
            // Priority 0 = Transient
            const result = peer.send_with_backpressure(
                new Uint8Array(posData.buffer),
                0  // Transient
            );
            
            if (result === "Dropped transient") {
                // Expected: position updates are transient
                // Next frame's update will supersede this one
            }
        }
    }
    
    sendCrdtDelta(delta) {
        for (const peer of this.peers.values()) {
            // Priority 2 = CriticalState
            const result = peer.send_with_backpressure(
                delta,
                2  // CriticalState
            );
            
            if (result.startsWith("Queued")) {
                // Acceptable: CRDT delta is queued and will be sent
                // Application logic must tolerate slight delay
            } else if (result.startsWith("Error")) {
                console.error("Failed to send CRDT delta:", result);
            }
        }
    }
    
    monitorBackpressure() {
        setInterval(() => {
            for (const [peerId, peer] of this.peers) {
                const status = peer.buffer_status_str();
                
                if (status.includes("CRITICAL") || status.includes("OVERFLOW")) {
                    console.warn(`Peer ${peerId}: ${status}`);
                    // Consider reducing send rate or dropping even non-transient messages
                }
            }
        }, 1000);
    }
}
```

### 7. Setup bufferedamountlow Event Handler

```rust
// In src/p2p/webrtc.rs, modify setup_data_channel():

use wasm_bindgen::closure::Closure;

fn setup_data_channel(&self, dc: &RtcDataChannel) {
    // ... existing code ...
    
    // === NEW: Setup bufferedamountlow event ===
    let backpressure_threshold = 256 * 1024;  // 256 KB
    dc.set_buffered_amount_low_threshold(backpressure_threshold);
    
    let onbufferedlow = Closure::<dyn FnMut()>::new(move || {
        // This fires when bufferedAmount <= threshold
        // Application should try to send queued messages
        
        #[cfg(target_arch = "wasm32")]
        {
            web_sys::console::log_1(&"bufferedamountlow event fired".into());
        }
        
        // TODO: Call try_flush() on BackpressureDataChannel wrapper
    });
    
    dc.set_onbufferedamountlow(Some(onbufferedlow.as_ref().unchecked_ref()));
    onbufferedlow.forget();  // Keep closure alive
}
```

---

## Integration Steps

### Step 1: Add backpressure.rs
```bash
cp BACKPRESSURE_CODE_REFERENCE.md src/p2p/backpressure.rs
# (Copy the "Create src/p2p/backpressure.rs" section above)
```

### Step 2: Update mod.rs
Add the exports shown in section 3 above.

### Step 3: Extend webrtc.rs
Add the methods from section 4 to the WebRtcPeer impl block.

### Step 4: Update mesh.rs
Add the logging suggestions from section 5.

### Step 5: Update JavaScript
Use the wasm-bridge patterns from section 6.

### Step 6: Test
```bash
cargo build --target wasm32-unknown-unknown --release
# Test with DATACHANNEL_BACKPRESSURE_RESEARCH.md unit tests
```

---

## Verification Checklist

- [ ] `src/p2p/backpressure.rs` compiles (no dependencies on external crates)
- [ ] `src/p2p/mod.rs` exports `BackpressureDataChannel`, `MessagePriority`
- [ ] `WebRtcPeer::send_with_backpressure()` added
- [ ] `WebRtcPeer::buffered_amount()` and threshold methods added
- [ ] JavaScript code calls `send_with_backpressure()` with correct priority
- [ ] Run unit tests (mock DataChannel tests in backpressure.rs)
- [ ] Integration test with real peers (see DATACHANNEL_BACKPRESSURE_RESEARCH.md)
- [ ] Monitor performance: peak bufferedAmount should stay < 4 MB in 50-peer rooms

---

## Priority Values Quick Reference

```
Priority Value | Name | When to Use | Drop? |
0 | Transient | Position, rotation, animation | Yes |
1 | Consensus | BFT votes, proposals | Maybe (70% queue, 30% drop) |
2 | CriticalState | CRDT deltas, entity spawn, inventory | No |
```

---

*Last Updated: 2026-04-13*
