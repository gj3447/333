# WebRTC DataChannel web-sys API Reference
## Backpressure Monitoring & bufferedAmount Methods

> **Status**: API Reference | **Date**: 2026-04-13  
> **Crate**: web-sys v0.3  
> **Target**: wasm32-unknown-unknown  
> **Companion**: DATACHANNEL_BACKPRESSURE_RESEARCH.md

---

## RTCDataChannel Methods for Backpressure

### Core Read Methods

#### `buffered_amount() -> u32`
**Returns**: Number of bytes currently queued but not yet sent.

```rust
use web_sys::RtcDataChannel;

fn check_buffer(dc: &RtcDataChannel) {
    let bytes = dc.buffered_amount();
    println!("Buffered: {} bytes ({:.1} MB)", bytes, bytes as f64 / 1_000_000.0);
}
```

**Key Points**:
- Read-only (getter only)
- Returns `u32` (max ~4 GB)
- Updates asynchronously (may lag actual network queue by 1-10 ms)
- Includes both ordered and unordered chunks

**Browser Quirks**:
- Chrome: Very accurate (updates ~5 ms after send)
- Firefox: May report higher (includes browser-level buffering)
- Safari: Accurate but resets slower (slower network drain detection)

---

### Threshold Management

#### `set_buffered_amount_low_threshold(threshold: u32)`
**Sets**: The threshold below which `bufferedamountlow` event fires.

```rust
fn setup_backpressure(dc: &RtcDataChannel) {
    // Fire event when buffer drains below 256 KB
    dc.set_buffered_amount_low_threshold(256 * 1024);
}
```

**Default**: 0 (fires only when buffer is completely empty)

**Recommended Values**:
- Small rooms (< 10 peers): 256 KB
- Medium rooms (10-30 peers): 512 KB - 1 MB
- Large rooms (30+ peers): 1-2 MB
- Conservative: Always use 256 KB (fires often, but safe)

**Performance Note**: Setting threshold too low causes excessive `bufferedamountlow` events (browser JS callback overhead). Setting too high risks buffer overflow.

---

#### `buffered_amount_low_threshold() -> u32`
**Returns**: Current backpressure threshold.

```rust
fn get_threshold(dc: &RtcDataChannel) {
    let threshold = dc.buffered_amount_low_threshold();
    assert_eq!(threshold, 256 * 1024);  // Verify threshold was set
}
```

**Usage**: Confirm threshold before sending (defensive check).

---

### Event Callback Management

#### `set_onbufferedamountlow(callback: Option<&Function>)`
**Sets**: Callback when bufferedAmount drops below threshold.

```rust
use wasm_bindgen::prelude::*;
use wasm_bindgen::closure::Closure;
use web_sys::RtcDataChannel;

fn setup_flush_handler(dc: &RtcDataChannel) {
    let onbufferedlow = Closure::<dyn FnMut()>::new(move || {
        // Called when bufferedAmount <= threshold
        // Safe to resume sending queued messages here
        println!("Buffer drained: safe to send again");
    });
    
    dc.set_onbufferedamountlow(Some(onbufferedlow.as_ref().unchecked_ref()));
    onbufferedlow.forget();  // Keep closure alive
}
```

**Behavior**:
- Fires 0 or more times during DataChannel lifetime
- Fires **every time** bufferedAmount crosses below threshold
- Can fire multiple times per second (if threshold oscillates)
- Does NOT fire on every byte of change (only threshold crossing)

**Event Handling Pattern**:
1. Application tries to send message
2. `send()` succeeds, bufferedAmount increases
3. If bufferedAmount > threshold: application queues subsequent messages
4. Network drains buffer (application not sending)
5. Browser fires `bufferedamountlow` when bufferedAmount <= threshold
6. Application handler calls `try_flush()` to drain local queue

---

#### `onbufferedamountlow() -> Option<Function>`
**Returns**: Current bufferedamountlow callback (if set).

```rust
fn check_handler_registered(dc: &RtcDataChannel) -> bool {
    dc.onbufferedamountlow().is_some()
}
```

**Usage**: Verify handler is registered (defensive assertion).

---

### Send Methods (Existing)

#### `send_with_u8_array(data: &[u8]) -> Result<(), JsValue>`
**Sends**: Binary data to remote peer.

```rust
fn send_bytes(dc: &RtcDataChannel, data: &[u8]) -> Result<(), JsValue> {
    dc.send_with_u8_array(data)  // Returns immediately (non-blocking)
}
```

**Behavior**:
- Non-blocking: returns immediately
- Adds data to internal send queue (increases bufferedAmount)
- Does NOT guarantee delivery to remote (can fail if buffer full)
- Use before sending: check `buffered_amount() <= threshold`

**Error Cases**:
```javascript
JsValue::from_str("Network error: ...")      // DataChannel not open
JsValue::from_str("QuotaExceededError")      // Buffer full (rare)
```

---

#### `send_with_str(data: &str) -> Result<(), JsValue>`
**Sends**: UTF-8 string to remote peer.

```rust
fn send_text(dc: &RtcDataChannel, msg: &str) -> Result<(), JsValue> {
    dc.send_with_str(msg)
}
```

**Note**: For CRDT/BFT messages, prefer `send_with_u8_array` (binary protocol).

---

### Related Methods (Connection State)

#### `ready_state() -> RtcDataChannelState`
**Returns**: Current DataChannel state.

```rust
use web_sys::RtcDataChannelState;

fn is_open(dc: &RtcDataChannel) -> bool {
    dc.ready_state() == RtcDataChannelState::Open
}
```

**States**:
- `Connecting` — handshake in progress
- `Open` — ready to send/receive
- `Closing` — close() called, pending local cleanup
- `Closed` — closed, no sends allowed

**Important**: `buffered_amount()` is meaningful only in `Open` state.

---

#### `close()`
**Closes**: The DataChannel (orderly shutdown).

```rust
fn close_channel(dc: &RtcDataChannel) {
    dc.close();
    // After this, ready_state != Open
    // Buffered data may still be sent (depends on browser)
}
```

**Note**: After `close()`, `buffered_amount()` is still readable but not meaningful.

---

## Complete Backpressure Flow in web-sys

### 1. Setup Phase (in `create_data_channel`)

```rust
fn setup_with_backpressure(dc: &RtcDataChannel) -> Result<(), JsValue> {
    // Verify channel is open
    assert_eq!(dc.ready_state(), RtcDataChannelState::Open);
    
    // Set backpressure threshold (256 KB)
    dc.set_buffered_amount_low_threshold(256 * 1024);
    
    // Register flush handler
    let onbufferedlow = Closure::<dyn FnMut()>::new(move || {
        // Try to flush queued messages
        // (Implementation in BackpressureDataChannel::try_flush())
    });
    dc.set_onbufferedamountlow(Some(onbufferedlow.as_ref().unchecked_ref()));
    onbufferedlow.forget();
    
    Ok(())
}
```

### 2. Send Phase (in application send path)

```rust
fn send_message(
    dc: &RtcDataChannel,
    data: &[u8],
    priority: MessagePriority,
) -> SendResult {
    // Check buffer first
    let buffered = dc.buffered_amount();
    let threshold = dc.buffered_amount_low_threshold();
    
    if buffered > threshold {
        // Buffer is high: decide action by priority
        match priority {
            MessagePriority::CriticalState => SendResult::Queued,
            MessagePriority::Consensus => SendResult::MaybeDrop,
            MessagePriority::Transient => SendResult::Dropped,
        }
    } else {
        // Buffer is low: try send
        match dc.send_with_u8_array(data) {
            Ok(()) => SendResult::Sent,
            Err(e) => SendResult::Error(e),
        }
    }
}
```

### 3. Drain Phase (called from `bufferedamountlow` event)

```rust
fn flush_queued_messages(
    dc: &RtcDataChannel,
    queue: &mut Vec<Vec<u8>>,
    threshold: u32,
) -> usize {
    let mut flushed = 0;
    
    while let Some(msg) = queue.first() {
        // Stop if buffer is rising again
        if dc.buffered_amount() > threshold {
            break;
        }
        
        // Try to send next queued message
        match dc.send_with_u8_array(msg) {
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
    
    flushed
}
```

---

## Timing & Latency

### bufferedAmount Accuracy

| Browser | Latency | Accuracy | Notes |
|---------|---------|----------|-------|
| Chrome | ~5 ms | ±1-2% | V8 updates on tick |
| Firefox | ~10 ms | ±5% | SpiderMonkey batches updates |
| Safari | ~20 ms | ±3% | Slower update cadence |

**Implication**: If you send 1 MB, `buffered_amount()` might still report 0 for 5-20 ms. **Don't rely on instant feedback.**

### bufferedamountlow Event Timing

| Scenario | Latency | Notes |
|----------|---------|-------|
| Network recovers | 50-500 ms | Browser-dependent, network-dependent |
| Threshold set to 0 | 1-5 ms | Only fires when truly empty |
| Threshold set to 256 KB | 100-1000 ms | Depends on network bandwidth |

**Implication**: Application must be tolerant of brief delays (100-1000 ms) between local send and remote receipt.

---

## Error Handling Specifics

### Common Send Errors

```rust
fn handle_send_error(dc: &RtcDataChannel, err: JsValue) {
    let error_str = format!("{:?}", err);
    
    match error_str.as_str() {
        s if s.contains("InvalidStateError") => {
            // DataChannel not open (ready_state != Open)
            eprintln!("DC not open, cannot send");
        }
        s if s.contains("QuotaExceededError") => {
            // Buffer full (rare, indicates severe backpressure)
            eprintln!("Buffer full, queuing required");
        }
        _ => {
            eprintln!("Unknown send error: {}", error_str);
        }
    }
}
```

### Handling send_with_u8_array Failure

```rust
loop {
    match dc.send_with_u8_array(data) {
        Ok(()) => {
            // Success: bufferedAmount increased
            break;
        }
        Err(e) => {
            let is_state_error = format!("{:?}", e).contains("InvalidStateError");
            
            if is_state_error {
                // DC closed, give up
                return Err("DataChannel closed");
            } else {
                // Transient error, retry in 100 ms
                sleep(Duration::from_millis(100)).await;
            }
        }
    }
}
```

---

## Browser Differences

### Chrome

```rust
// Chrome: Fast bufferedAmount updates (5 ms), accurate
if dc.buffered_amount() > 256 * 1024 {
    // Very accurate indicator of real buffer state
    queue_message();
}
```

### Firefox

```rust
// Firefox: Slower updates (10 ms), may report high even if some drained
// More conservative approach:
const SAFETY_MARGIN = 50 * 1024;  // Extra 50 KB margin
if dc.buffered_amount() > (256 * 1024 + SAFETY_MARGIN) {
    queue_message();
}
```

### Safari

```rust
// Safari: Slowest updates (20 ms), less predictable
// Most conservative approach:
const SAFETY_MARGIN = 100 * 1024;  // Extra 100 KB margin
if dc.buffered_amount() > (256 * 1024 + SAFETY_MARGIN) {
    queue_message();
}
```

---

## Cargo.toml Feature Flags

Ensure these web-sys features are enabled:

```toml
[dependencies]
web-sys = { version = "0.3", features = [
  "RtcDataChannel",
  "RtcDataChannelInit",
  "RtcDataChannelState",  # For ready_state()
  "RtcDataChannelEvent",  # For ondatachannel callback
  "MessageEvent",         # For onmessage callback
  # ... other features
] }
```

**Note**: No additional Cargo features needed for backpressure — `bufferedAmount` and `bufferedAmountLowThreshold` are standard WebRTC APIs.

---

## Debugging Tips

### Log Buffer Status Periodically

```rust
use web_sys::window;

fn setup_buffer_monitor(dc: &RtcDataChannel) {
    let dc_clone = dc.clone();
    
    // Check every 1 second
    let interval = window()
        .unwrap()
        .set_interval_with_callback_and_timeout_and_arguments_0(
            &Closure::<dyn FnMut()>::new(move || {
                let buffered = dc_clone.buffered_amount();
                let threshold = dc_clone.buffered_amount_low_threshold();
                
                if buffered > threshold {
                    web_sys::console::warn_2(
                        &"Backpressured: bufferedAmount=".into(),
                        &buffered.into(),
                    );
                }
            }).as_ref().unchecked_ref(),
            1000
        )
        .unwrap();
}
```

### Inspect readyState Changes

```rust
fn setup_state_monitor(dc: &RtcDataChannel) {
    // Monitor channel state
    let onopen = Closure::<dyn FnMut()>::new(move || {
        web_sys::console::log_1(&"DataChannel OPEN".into());
    });
    dc.set_onopen(Some(onopen.as_ref().unchecked_ref()));
    onopen.forget();
    
    let onclose = Closure::<dyn FnMut()>::new(move || {
        web_sys::console::log_1(&"DataChannel CLOSED".into());
    });
    dc.set_onclose(Some(onclose.as_ref().unchecked_ref()));
    onclose.forget();
}
```

---

## References

- [MDN RTCDataChannel](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel)
- [MDN bufferedAmount](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel/bufferedAmount)
- [MDN bufferedAmountLowThreshold](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel/bufferedAmountLowThreshold)
- [MDN bufferedamountlow Event](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel/bufferedamountlow_event)
- [web-sys RTCDataChannel Binding](https://docs.rs/web-sys/latest/web_sys/struct.RtcDataChannel.html)
- [WebRTC Stats Report API](https://w3c.github.io/webrtc-pc/#rtcstats-object)

---

*Last Updated: 2026-04-13*
