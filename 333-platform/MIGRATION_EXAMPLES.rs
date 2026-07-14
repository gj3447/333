// KG: REFACTORING_333_Arc_to_Rc_migration_WASM
// Concrete code patterns for Arc<Mutex<T>> → Rc<RefCell<T>> migration
// Apply these patterns directly to your codebase

// ============================================================================
// PATTERN 1: Basic State Wrapper (webrtc.rs struct definitions)
// ============================================================================

// BEFORE
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;

pub struct WebRtcPeerBefore {
    inbox: Arc<Mutex<VecDeque<Vec<u8>>>>,
    state: Arc<Mutex<PeerState>>,
}

impl WebRtcPeerBefore {
    pub fn new() -> Self {
        Self {
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            state: Arc::new(Mutex::new(PeerState::New)),
        }
    }

    pub fn recv(&self) -> Option<Vec<u8>> {
        self.inbox.lock().unwrap().pop_front()
    }

    pub fn peer_state(&self) -> PeerState {
        *self.state.lock().unwrap()
    }

    pub fn set_state(&self, new_state: PeerState) {
        *self.state.lock().unwrap() = new_state;
    }
}

// AFTER
use std::rc::Rc;
use std::cell::RefCell;

pub struct WebRtcPeerAfter {
    inbox: Rc<RefCell<VecDeque<Vec<u8>>>>,
    state: Rc<RefCell<PeerState>>,
}

impl WebRtcPeerAfter {
    pub fn new() -> Self {
        Self {
            inbox: Rc::new(RefCell::new(VecDeque::new())),
            state: Rc::new(RefCell::new(PeerState::New)),
        }
    }

    pub fn recv(&self) -> Option<Vec<u8>> {
        self.inbox.borrow_mut().pop_front()
    }

    pub fn peer_state(&self) -> PeerState {
        *self.state.borrow()
    }

    pub fn set_state(&self, new_state: PeerState) {
        *self.state.borrow_mut() = new_state;
    }
}

// ============================================================================
// PATTERN 2: Closure Capture in Event Handlers (webrtc.rs lines 115-134)
// ============================================================================

// BEFORE: Multiple Arc::clone and lock chains
#[wasm_bindgen]
impl WebRtcPeerBefore {
    pub async fn accept_offer(&mut self, offer_sdp: &str) -> Result<String, JsValue> {
        let inbox = Arc::clone(&self.inbox);
        let state = Arc::clone(&self.state);

        let ondc = Closure::<dyn FnMut(RtcDataChannelEvent)>::new(move |evt| {
            let dc = evt.channel();
            let inbox2 = Arc::clone(&inbox);
            let state2 = Arc::clone(&state);

            let onmsg = Closure::<dyn FnMut(MessageEvent)>::new(move |e| {
                if let Ok(text) = e.data().dyn_into::<js_sys::JsString>() {
                    let s: String = text.into();
                    inbox2.lock().unwrap().push_back(s.into_bytes());
                }
            });
            dc.set_onmessage(Some(onmsg.as_ref().unchecked_ref()));
            onmsg.forget();

            let onopen = Closure::<dyn FnMut()>::new(move || {
                *state2.lock().unwrap() = PeerState::Connected;
            });
            dc.set_onopen(Some(onopen.as_ref().unchecked_ref()));
            onopen.forget();
        });
        self.pc.set_ondatachannel(Some(ondc.as_ref().unchecked_ref()));
        ondc.forget();

        // ... rest of method ...
        Ok(String::new())
    }
}

// AFTER: Cleaner Rc::clone and borrow_mut
#[wasm_bindgen]
impl WebRtcPeerAfter {
    pub async fn accept_offer(&mut self, offer_sdp: &str) -> Result<String, JsValue> {
        let inbox = Rc::clone(&self.inbox);
        let state = Rc::clone(&self.state);

        let ondc = Closure::<dyn FnMut(RtcDataChannelEvent)>::new(move |evt| {
            let dc = evt.channel();
            let inbox2 = Rc::clone(&inbox);
            let state2 = Rc::clone(&state);

            let onmsg = Closure::<dyn FnMut(MessageEvent)>::new(move |e| {
                if let Ok(text) = e.data().dyn_into::<js_sys::JsString>() {
                    let s: String = text.into();
                    inbox2.borrow_mut().push_back(s.into_bytes());
                }
            });
            dc.set_onmessage(Some(onmsg.as_ref().unchecked_ref()));
            onmsg.forget();

            let onopen = Closure::<dyn FnMut()>::new(move || {
                *state2.borrow_mut() = PeerState::Connected;
            });
            dc.set_onopen(Some(onopen.as_ref().unchecked_ref()));
            onopen.forget();
        });
        self.pc.set_ondatachannel(Some(ondc.as_ref().unchecked_ref()));
        ondc.forget();

        // ... rest of method ...
        Ok(String::new())
    }
}

// ============================================================================
// PATTERN 3: Safe Error Handling with try_borrow_mut()
// ============================================================================

// ❌ DANGEROUS: Will panic if borrow conflict
pub fn recv_unsafe(peer: &WebRtcPeerAfter) -> Option<Vec<u8>> {
    peer.inbox.borrow_mut().pop_front()
}

// ✅ SAFE: Returns Result, doesn't panic
pub fn recv_safe(peer: &WebRtcPeerAfter) -> Result<Option<Vec<u8>>, &'static str> {
    peer.inbox
        .try_borrow_mut()
        .map(|mut q| q.pop_front())
        .map_err(|_| "Inbox currently borrowed")
}

// ✅ SAFE: Scope-based borrow release
pub fn recv_scoped(peer: &WebRtcPeerAfter) -> Option<Vec<u8>> {
    {
        let mut q = peer.inbox.borrow_mut();
        q.pop_front()
        // q is dropped here, borrow released
    }
}

// ✅ SAFE: Use methods instead of exposing RefCell
impl WebRtcPeerAfter {
    pub fn try_recv(&self) -> Result<Option<Vec<u8>>, &'static str> {
        self.inbox
            .try_borrow_mut()
            .map(|mut q| q.pop_front())
            .map_err(|_| "Inbox currently borrowed")
    }
}

// ============================================================================
// PATTERN 4: Channel.rs SharedQueue Type Alias
// ============================================================================

// BEFORE (Line 66 in channel.rs)
type SharedQueueBefore = Arc<Mutex<VecDeque<(Vec<u8>, ChannelMode)>>>;

pub struct InMemoryChannelBefore {
    local_id: u32,
    remote_id: u32,
    outbox: SharedQueueBefore,
    inbox: SharedQueueBefore,
}

impl InMemoryChannelBefore {
    pub fn send(&self, data: &[u8], mode: ChannelMode) -> Result<(), ChannelError> {
        self.outbox.lock().unwrap().push_back((data.to_vec(), mode));
        Ok(())
    }

    pub fn recv(&self) -> Option<(Vec<u8>, ChannelMode)> {
        self.inbox.lock().unwrap().pop_front()
    }
}

// AFTER (Line 66 in channel.rs)
type SharedQueueAfter = Rc<RefCell<VecDeque<(Vec<u8>, ChannelMode)>>>;

pub struct InMemoryChannelAfter {
    local_id: u32,
    remote_id: u32,
    outbox: SharedQueueAfter,
    inbox: SharedQueueAfter,
}

impl InMemoryChannelAfter {
    pub fn send(&self, data: &[u8], mode: ChannelMode) -> Result<(), ChannelError> {
        self.outbox.borrow_mut().push_back((data.to_vec(), mode));
        Ok(())
    }

    pub fn recv(&self) -> Option<(Vec<u8>, ChannelMode)> {
        self.inbox.borrow_mut().pop_front()
    }
}

// ============================================================================
// PATTERN 5: Handling BorrowMutError in Hot Paths
// ============================================================================

use std::cell::BorrowMutError;

// ❌ PROBLEMATIC: Panics on borrow conflict
fn process_all_messages_unsafe(peer: &WebRtcPeerAfter) {
    while let Some(msg) = peer.recv() {
        // Process msg
    }
}

// ✅ GOOD: Graceful degradation
fn process_all_messages_safe(peer: &WebRtcPeerAfter) {
    loop {
        match peer.try_recv() {
            Ok(Some(msg)) => {
                // Process msg
            }
            Ok(None) => break,  // Queue empty
            Err(e) => {
                // Log and skip this cycle
                web_sys::console::log_1(&format!("Skipped: {}", e).into());
                break;
            }
        }
    }
}

// ============================================================================
// PATTERN 6: Global State with thread_local (if needed)
// ============================================================================

use std::cell::RefCell;

thread_local! {
    static GLOBAL_STATE: RefCell<Vec<String>> = RefCell::new(Vec::new());
}

pub fn add_to_global(s: String) -> Result<(), &'static str> {
    GLOBAL_STATE.with(|state| {
        state
            .try_borrow_mut()
            .map(|mut v| v.push(s))
            .map_err(|_| "Global state locked")
    })
}

pub fn get_from_global(index: usize) -> Result<Option<String>, &'static str> {
    GLOBAL_STATE.with(|state| {
        state
            .try_borrow()
            .map(|v| v.get(index).cloned())
            .map_err(|_| "Global state locked")
    })
}

// ============================================================================
// PATTERN 7: Debugging BorrowMutError (for Development)
// ============================================================================

// Add this helper during debugging to catch borrow panics early
#[cfg(debug_assertions)]
pub fn debug_borrow<T: std::fmt::Debug>(
    cell: &RefCell<T>,
    label: &str,
) -> Result<std::cell::Ref<T>, &'static str> {
    match cell.try_borrow() {
        Ok(r) => {
            web_sys::console::log_1(&format!("{}: borrow ok", label).into());
            Ok(r)
        }
        Err(_) => {
            web_sys::console::error_1(&format!("{}: BORROW ERROR!", label).into());
            Err("BorrowError in debug mode")
        }
    }
}

// Usage during debugging:
// let state = debug_borrow(&peer.state, "peer_state")?;
// println!("{:?}", *state);

// ============================================================================
// PATTERN 8: Migration Test (Verify same behavior)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recv_empty() {
        let peer = WebRtcPeerAfter::new();
        assert_eq!(peer.try_recv(), Ok(None));
    }

    #[test]
    fn test_state_change() {
        let peer = WebRtcPeerAfter::new();
        peer.set_state(PeerState::Connected);
        assert_eq!(peer.peer_state(), PeerState::Connected);
    }

    #[test]
    fn test_concurrent_recv_send() {
        let peer = Rc::new(WebRtcPeerAfter::new());

        // Send
        {
            let p = Rc::clone(&peer);
            p.inbox.borrow_mut().push_back(vec![1, 2, 3]);
        }

        // Recv
        {
            let p = Rc::clone(&peer);
            assert_eq!(p.try_recv(), Ok(Some(vec![1, 2, 3])));
        }
    }

    #[test]
    #[should_panic(expected = "already borrowed")]
    fn test_borrow_panic_detects() {
        let data = Rc::new(RefCell::new(vec![1, 2, 3]));
        let _a = data.borrow_mut();
        let _b = data.borrow_mut();  // PANIC!
    }
}

// ============================================================================
// PATTERN 9: Type-level Distinction (Optional: New Type Pattern)
// ============================================================================

// If you want to distinguish between Arc<Mutex> code and Rc<RefCell> code:
mod wasm {
    use std::rc::Rc;
    use std::cell::RefCell;

    pub type Shared<T> = Rc<RefCell<T>>;

    pub fn new_shared<T>(value: T) -> Shared<T> {
        Rc::new(RefCell::new(value))
    }
}

mod native {
    use std::sync::{Arc, Mutex};

    pub type Shared<T> = Arc<Mutex<T>>;

    pub fn new_shared<T: Send + 'static>(value: T) -> Shared<T> {
        Arc::new(Mutex::new(value))
    }
}

// Usage (compile-time platform selection):
#[cfg(target_arch = "wasm32")]
use wasm::Shared;

#[cfg(not(target_arch = "wasm32"))]
use native::Shared;

pub struct PlatformAgnosticPeer {
    inbox: Shared<VecDeque<Vec<u8>>>,
}

// ============================================================================
// END OF PATTERNS
// ============================================================================
// Apply these patterns directly to:
// - src/p2p/webrtc.rs
// - src/p2p/channel.rs
// - src/bft/transport.rs
//
// See WASM_REFACTORING_GUIDE.md for detailed explanation and checklist
