// KG: TASK_333_INT_MemFix, CONTRACT_333_INT_MemFix
// KG: CONTRACT_SharedType_DataChannelSet
// KG: VR_333_Integration_MemFix_Taliban — F1~F7 반영
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-p2p-webrtc-PeerState/CH_CRDT/CH_POSITION/CH_BFT/CH_CHAT/SyncInbox/ChannelInboxes/WebRtcPeer
//! WebRTC peer connection wrapper — WASM optimized.
//! Rc<RefCell> (not Arc<Mutex>) for single-threaded WASM.
//! Closure stored in struct (not forget()) + Drop trait for cleanup.
//! 4 DataChannels: crdt, position, bft, chat.
//! 채널별 inbox 분리 (F2: BFT 메시지 지연 방지).
//! SyncInbox 래퍼 (F3: borrow_mut 범위 제한).

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{
    RtcPeerConnection, RtcConfiguration,
    RtcDataChannel, RtcDataChannelInit, RtcDataChannelEvent,
    RtcSessionDescriptionInit, RtcSdpType,
    RtcIceCandidateInit,
    MessageEvent,
};
use js_sys::{Array, Object, Reflect};
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::VecDeque;

// KG: CONTRACT_333_INT_MemFix — PeerState enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    New,
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

/// Channel identifiers for 4-channel architecture
/// KG: CONTRACT_SharedType_DataChannelSet
pub const CH_CRDT: &str = "crdt";        // reliable, ordered
pub const CH_POSITION: &str = "position"; // unreliable, unordered, 50ms
pub const CH_BFT: &str = "bft";          // reliable, ordered, max 5 retry
pub const CH_CHAT: &str = "chat";        // reliable, ordered

/// F3 Fix: SyncInbox — borrow 범위를 enqueue/drain으로 제한
/// KG: VR_333_Integration_MemFix_Taliban — F3 borrow_mut 범위 제한
#[derive(Clone)]
pub struct SyncInbox {
    inner: Rc<RefCell<VecDeque<Vec<u8>>>>,
}

impl SyncInbox {
    pub fn new() -> Self {
        Self { inner: Rc::new(RefCell::new(VecDeque::new())) }
    }

    /// Enqueue a message — borrow released immediately
    pub fn enqueue(&self, data: Vec<u8>) {
        self.inner.borrow_mut().push_back(data);
    }

    /// Dequeue one message — borrow released immediately
    pub fn dequeue(&self) -> Option<Vec<u8>> {
        self.inner.borrow_mut().pop_front()
    }

    /// Drain all messages — borrow released immediately
    pub fn drain_all(&self) -> Vec<Vec<u8>> {
        self.inner.borrow_mut().drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.inner.borrow().len()
    }
}

/// Closure storage for proper lifecycle management
/// KG: TASK_333_INT_MemFix — Closure leak 방지
struct ClosureStore {
    onmessage: Option<Closure<dyn FnMut(MessageEvent)>>,
    onopen: Option<Closure<dyn FnMut()>>,
}

impl ClosureStore {
    fn new() -> Self {
        Self { onmessage: None, onopen: None }
    }

    fn cleanup(&mut self, dc: &RtcDataChannel) {
        dc.set_onmessage(None);
        dc.set_onopen(None);
        self.onmessage = None;
        self.onopen = None;
    }
}

/// Per-channel state
struct ChannelEntry {
    dc: RtcDataChannel,
    closures: ClosureStore,
}

/// F2 Fix: 채널별 inbox 분리 — BFT 메시지가 CRDT 뒤에 묻히지 않음
/// KG: VR_333_Integration_MemFix_Taliban — F2 단일 inbox 분리
pub struct ChannelInboxes {
    pub crdt: SyncInbox,
    pub bft: SyncInbox,
    pub chat: SyncInbox,
    pub position: SyncInbox,
}

impl ChannelInboxes {
    fn new() -> Self {
        Self {
            crdt: SyncInbox::new(),
            bft: SyncInbox::new(),
            chat: SyncInbox::new(),
            position: SyncInbox::new(),
        }
    }

    pub fn inbox_for(&self, label: &str) -> &SyncInbox {
        match label {
            CH_CRDT => &self.crdt,
            CH_BFT => &self.bft,
            CH_CHAT => &self.chat,
            CH_POSITION => &self.position,
            _ => &self.crdt,
        }
    }
}

/// WebRTC peer connection wrapper — WASM optimized
/// KG: CONTRACT_333_INT_MemFix
#[wasm_bindgen]
pub struct WebRtcPeer {
    pc: RtcPeerConnection,
    channels: Vec<ChannelEntry>,
    /// F1 Fix: remote channel closures stored here (not forgotten)
    /// KG: VR_333_Integration_MemFix_Taliban — F1 closure 저장
    remote_channels: Vec<ChannelEntry>,
    remote_id: u32,
    /// F2 Fix: 채널별 분리된 inbox
    inboxes: Rc<ChannelInboxes>,
    state: Rc<RefCell<PeerState>>,
    ondatachannel: Option<Closure<dyn FnMut(RtcDataChannelEvent)>>,
}

/// ICE server configuration — STUN + TURN fallback
/// KG: TASK_333_PRE_TURN, CONTRACT_333_PRE_TURN
/// Taliban C3 fix: 대칭 NAT/방화벽에서 TURN relay로 BFT 합의 유지
fn default_ice_config() -> RtcConfiguration {
    let config = RtcConfiguration::new();
    let ice_servers = Array::new();

    // STUN (free, direct P2P ~80% success)
    let stun = Object::new();
    let stun_urls = Array::new();
    stun_urls.push(&JsValue::from_str("stun:stun.l.google.com:19302"));
    stun_urls.push(&JsValue::from_str("stun:stun1.l.google.com:19302"));
    // Taliban F6: propagate Reflect::set errors instead of silently swallowing
    Reflect::set(&stun, &"urls".into(), &stun_urls).expect("set stun urls");
    ice_servers.push(&stun);

    // TURN (relay fallback for symmetric NAT ~15% of users)
    // KG: CONTRACT_333_PRE_TURN
    // Taliban F4: coturn 미배포 상태. STUN-only 동작. coturn 배포 후 활성화.
    // TODO: kubeadm apps namespace에 coturn Pod 배포 후 아래 주석 해제
    // TODO: credentials을 서버사이드 ephemeral token으로 교체 (RFC 5766 §10)
    // let turn = Object::new();
    // let turn_urls = Array::new();
    // turn_urls.push(&JsValue::from_str("turn:turn.metahumotonic.com:3478"));
    // turn_urls.push(&JsValue::from_str("turns:turn.metahumotonic.com:5349"));
    // Reflect::set(&turn, &"urls".into(), &turn_urls).expect("set turn urls");
    // Reflect::set(&turn, &"username".into(), &JsValue::from_str("333peer")).expect("set turn user");
    // Reflect::set(&turn, &"credential".into(), &JsValue::from_str("333turnpass")).expect("set turn cred");
    // ice_servers.push(&turn);

    config.set_ice_servers(&ice_servers);
    config
}

/// DataChannelInit per channel type
/// KG: CONTRACT_SharedType_DataChannelSet
fn channel_config(label: &str) -> RtcDataChannelInit {
    let mut init = RtcDataChannelInit::new();
    match label {
        CH_CRDT => { init.ordered(true); }
        CH_POSITION => { init.ordered(false); init.max_packet_life_time(50); }
        CH_BFT => { init.ordered(true); /* reliable — no maxRetransmits cap (F5 fix) */ }
        CH_CHAT => { init.ordered(true); }
        _ => { init.ordered(true); }
    }
    init
}

#[wasm_bindgen]
impl WebRtcPeer {
    /// Create a new WebRTC peer connection
    /// KG: CONTRACT_333_INT_MemFix
    #[wasm_bindgen(constructor)]
    pub fn new(remote_id: u32) -> Result<WebRtcPeer, JsValue> {
        let config = default_ice_config();
        let pc = RtcPeerConnection::new_with_configuration(&config)?;
        let inboxes = Rc::new(ChannelInboxes::new());
        let state = Rc::new(RefCell::new(PeerState::New));

        Ok(Self {
            pc,
            channels: Vec::new(),
            remote_channels: Vec::new(),
            remote_id,
            inboxes,
            state,
            ondatachannel: None,
        })
    }

    /// Create 4 DataChannels (caller/offerer side)
    /// KG: CONTRACT_SharedType_DataChannelSet
    pub fn create_channels(&mut self) -> Result<(), JsValue> {
        for label in &[CH_CRDT, CH_BFT, CH_CHAT, CH_POSITION] {
            let init = channel_config(label);
            let dc = self.pc.create_data_channel_with_data_channel_dict(label, &init);
            let closures = self.attach_handlers(&dc);
            self.channels.push(ChannelEntry { dc, closures });
        }
        *self.state.borrow_mut() = PeerState::Connecting;
        Ok(())
    }

    /// Create SDP offer (caller side)
    pub async fn create_offer(&self) -> Result<String, JsValue> {
        let offer = wasm_bindgen_futures::JsFuture::from(self.pc.create_offer()).await?;
        let offer_sdp = Reflect::get(&offer, &"sdp".into())?.as_string().unwrap_or_default();
        let mut desc = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
        desc.sdp(&offer_sdp);
        wasm_bindgen_futures::JsFuture::from(self.pc.set_local_description(&desc)).await?;
        Ok(offer_sdp)
    }

    /// Accept SDP offer and create answer (callee side)
    /// KG: TASK_333_INT_MemFix — F1 Fix: remote closures stored in remote_channels
    pub async fn accept_offer(&mut self, offer_sdp: &str) -> Result<String, JsValue> {
        let mut offer_desc = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
        offer_desc.sdp(offer_sdp);
        wasm_bindgen_futures::JsFuture::from(self.pc.set_remote_description(&offer_desc)).await?;

        // F1 Fix: remote channels are stored via shared Vec, not forgotten
        let remote_store = Rc::new(RefCell::new(Vec::<ChannelEntry>::new()));
        let inboxes = Rc::clone(&self.inboxes);
        let state = Rc::clone(&self.state);
        let store_ref = Rc::clone(&remote_store);

        let ondc = Closure::<dyn FnMut(RtcDataChannelEvent)>::new(move |evt: RtcDataChannelEvent| {
            let dc = evt.channel();
            let label = dc.label();
            let inbox = inboxes.inbox_for(&label).clone();
            let state2 = Rc::clone(&state);

            // F3 Fix: SyncInbox.enqueue() — borrow scope is atomic
            let onmsg = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                if let Some(abuf) = e.data().dyn_ref::<js_sys::ArrayBuffer>() {
                    let arr = js_sys::Uint8Array::new(abuf);
                    inbox.enqueue(arr.to_vec());
                } else if let Ok(text) = e.data().dyn_into::<js_sys::JsString>() {
                    let s: String = text.into();
                    inbox.enqueue(s.into_bytes());
                }
            });
            dc.set_onmessage(Some(onmsg.as_ref().unchecked_ref()));

            let onopen = Closure::<dyn FnMut()>::new(move || {
                *state2.borrow_mut() = PeerState::Connected;
            });
            dc.set_onopen(Some(onopen.as_ref().unchecked_ref()));

            // F1 Fix: store closures in shared Vec — NOT forget()
            store_ref.borrow_mut().push(ChannelEntry {
                dc,
                closures: ClosureStore {
                    onmessage: Some(onmsg),
                    onopen: Some(onopen),
                },
            });
        });
        self.pc.set_ondatachannel(Some(ondc.as_ref().unchecked_ref()));
        self.ondatachannel = Some(ondc);

        // Create answer
        let answer = wasm_bindgen_futures::JsFuture::from(self.pc.create_answer()).await?;
        let answer_sdp = Reflect::get(&answer, &"sdp".into())?.as_string().unwrap_or_default();
        let mut desc = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
        desc.sdp(&answer_sdp);
        wasm_bindgen_futures::JsFuture::from(self.pc.set_local_description(&desc)).await?;

        // Move remote channels into struct
        self.remote_channels = remote_store.borrow_mut().drain(..).collect();

        *self.state.borrow_mut() = PeerState::Connecting;
        Ok(answer_sdp)
    }

    /// Accept SDP answer (caller side)
    pub async fn accept_answer(&self, answer_sdp: &str) -> Result<(), JsValue> {
        let mut desc = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
        desc.sdp(answer_sdp);
        wasm_bindgen_futures::JsFuture::from(self.pc.set_remote_description(&desc)).await?;
        Ok(())
    }

    /// Add ICE candidate from remote
    pub async fn add_ice_candidate(&self, candidate_json: &str) -> Result<(), JsValue> {
        let init = RtcIceCandidateInit::new(candidate_json);
        let candidate = web_sys::RtcIceCandidate::new(&init)?;
        wasm_bindgen_futures::JsFuture::from(
            self.pc.add_ice_candidate_with_opt_rtc_ice_candidate(Some(&candidate))
        ).await?;
        Ok(())
    }

    /// Send binary data on a specific channel
    /// KG: CONTRACT_SharedType_DataChannelSet
    pub fn send_on(&self, channel_label: &str, data: &[u8]) -> Result<(), JsValue> {
        // Check local (initiator) channels first, then remote (callee) channels
        for entry in self.channels.iter().chain(self.remote_channels.iter()) {
            if entry.dc.label() == channel_label
                && entry.dc.ready_state() == web_sys::RtcDataChannelState::Open
            {
                entry.dc.send_with_u8_array(data)?;
                return Ok(());
            }
        }
        Ok(())
    }

    /// Send binary data on crdt channel (backward compat)
    pub fn send_bytes(&self, data: &[u8]) -> Result<(), JsValue> {
        self.send_on(CH_CRDT, data)
    }

    /// Send text on crdt channel
    pub fn send(&self, data: &str) -> Result<(), JsValue> {
        for entry in self.channels.iter().chain(self.remote_channels.iter()) {
            if entry.dc.label() == CH_CRDT
                && entry.dc.ready_state() == web_sys::RtcDataChannelState::Open
            {
                entry.dc.send_with_str(data)?;
                return Ok(());
            }
        }
        Ok(())
    }

    /// Get buffered amount for backpressure monitoring
    pub fn buffered_amount(&self, channel_label: &str) -> u32 {
        for entry in self.channels.iter().chain(self.remote_channels.iter()) {
            if entry.dc.label() == channel_label {
                return entry.dc.buffered_amount();
            }
        }
        0
    }

    /// F2 Fix: Receive from specific channel inbox
    /// KG: VR_333_Integration_MemFix_Taliban — F2 채널별 inbox
    pub fn recv_from(&self, channel: &str) -> Option<Vec<u8>> {
        self.inboxes.inbox_for(channel).dequeue()
    }

    /// Receive from crdt channel (backward compat)
    pub fn recv(&self) -> Option<Vec<u8>> {
        self.recv_from(CH_CRDT)
    }

    /// Get connection state
    pub fn peer_state(&self) -> String {
        format!("{:?}", *self.state.borrow())
    }

    /// Remote peer ID
    pub fn remote_id(&self) -> u32 {
        self.remote_id
    }

    /// Close with proper cleanup — F1+F6 Fix
    /// KG: TASK_333_INT_MemFix — set_handler(None) before close
    pub fn close(&mut self) {
        self.pc.set_ondatachannel(None);
        self.ondatachannel = None;

        for entry in &mut self.channels {
            entry.closures.cleanup(&entry.dc);
            entry.dc.close();
        }
        self.channels.clear();

        // F1+F6 Fix: cleanup remote channels too
        for entry in &mut self.remote_channels {
            entry.closures.cleanup(&entry.dc);
            entry.dc.close();
        }
        self.remote_channels.clear();

        self.pc.close();
        *self.state.borrow_mut() = PeerState::Disconnected;
    }
}

/// Drop — F6 Fix: cleanup ALL channels (local + remote)
/// KG: TASK_333_INT_MemFix
impl Drop for WebRtcPeer {
    fn drop(&mut self) {
        self.pc.set_ondatachannel(None);
        for entry in &mut self.channels {
            entry.closures.cleanup(&entry.dc);
            entry.dc.close();
        }
        for entry in &mut self.remote_channels {
            entry.closures.cleanup(&entry.dc);
            entry.dc.close();
        }
        self.pc.close();
    }
}

impl WebRtcPeer {
    /// Drain all messages from a channel (Rust-only, not exported to JS)
    pub fn drain_channel(&self, channel: &str) -> Vec<Vec<u8>> {
        self.inboxes.inbox_for(channel).drain_all()
    }

    /// Attach handlers to a DataChannel — routes to channel-specific inbox
    fn attach_handlers(&self, dc: &RtcDataChannel) -> ClosureStore {
        let label = dc.label();
        let inbox = self.inboxes.inbox_for(&label).clone();
        let state = Rc::clone(&self.state);

        // F3 Fix: SyncInbox.enqueue() has atomic borrow scope
        let onmsg = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
            if let Some(abuf) = e.data().dyn_ref::<js_sys::ArrayBuffer>() {
                let arr = js_sys::Uint8Array::new(abuf);
                inbox.enqueue(arr.to_vec());
            } else if let Ok(text) = e.data().dyn_into::<js_sys::JsString>() {
                let s: String = text.into();
                inbox.enqueue(s.into_bytes());
            }
        });
        dc.set_onmessage(Some(onmsg.as_ref().unchecked_ref()));

        let onopen = Closure::<dyn FnMut()>::new(move || {
            *state.borrow_mut() = PeerState::Connected;
        });
        dc.set_onopen(Some(onopen.as_ref().unchecked_ref()));

        ClosureStore {
            onmessage: Some(onmsg),
            onopen: Some(onopen),
        }
    }
}
