// KG: CONTRACT_333_DataChannel, CONTRACT_333_MeshRoom, CONTRACT_333_Signaling
// P2P module — trait abstraction + WebRTC + Signaling

pub mod channel;
pub mod mesh;
pub mod signaling_client;

pub mod super_peer;

#[cfg(target_arch = "wasm32")]
pub mod webrtc;
