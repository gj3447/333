//! 333 v2 substrate reachability — the BROWSER gate (wasm32 compile receipt, PROM step 1b).
//!
//! The native relay receipt (`crates/relay`) proved Circuit-Relay-v2 RESERVATION +
//! relayed connectivity over TCP. A browser peer cannot use TCP and cannot `listen_on`;
//! it becomes reachable by DIALING OUT over WebSocket to a relay and reserving a
//! Circuit-Relay-v2 slot. Reservation needs no NAT hole, so this path is independent of
//! the DCUtR hole-punch question (which still needs a netns/browser milestone). This
//! crate is the relay CLIENT (relay-client + dcutr + identify + ping) compiled for the
//! browser target over the `websocket-websys` transport.
//!
//! HONEST SCOPE: this is the wasm32 COMPILE gate. Green proves the behaviour graph + a
//! browser-compatible transport type-check and link for `wasm32-unknown-unknown`, the
//! getrandom dual-pin / executor / transport wiring is correct, and the native tcp/tokio
//! chain is correctly excluded. It does NOT run: no relay is dialed, no reservation is
//! made in a browser, no ping traverses the relay, no DCUtR upgrade occurs. End-to-end
//! browser-reachable-via-relay additionally needs (a) a WSS-listening relay (server-side
//! `websocket` feature + `/tls/ws` + a CA-signed cert) and (b) a headless-Chrome harness
//! (wasm-bindgen-test + chromedriver) — both deferred frontier sub-receipts.
#![cfg(target_arch = "wasm32")]

use libp2p::core::upgrade;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{dcutr, identify, noise, ping, relay, yamux, Swarm, SwarmBuilder, Transport};
use wasm_bindgen::prelude::*;

/// Identify protocol string for the 333 substrate (same as the native relay).
pub const PROTOCOL: &str = "/p333/reachability/0.1.0";

/// The browser relay CLIENT behaviour: reserve/relay over WS + try DCUtR.
#[derive(NetworkBehaviour)]
pub struct ReachabilityClient {
    pub relay_client: relay::client::Behaviour,
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
    pub dcutr: dcutr::Behaviour,
}

/// Build a browser (wasm32) relay-client swarm over the `websocket-websys` transport.
///
/// Wasm differences vs the native relay: the executor is `with_wasm_bindgen()` (not
/// `with_tokio()`), and the transport is hand-wired via `with_other_transport(...)`
/// because there is no `with_websocket_websys()` builder method — the websys transport
/// must be upgraded with noise+yamux ourselves before the relay-client wraps it.
pub fn build_browser_relay_client() -> anyhow::Result<Swarm<ReachabilityClient>> {
    let swarm = SwarmBuilder::with_new_identity()
        .with_wasm_bindgen()
        .with_other_transport(|key| {
            Ok(libp2p::websocket_websys::Transport::default()
                .upgrade(upgrade::Version::V1)
                .authenticate(noise::Config::new(key)?)
                .multiplex(yamux::Config::default()))
        })?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|kp, relay_client| ReachabilityClient {
            relay_client,
            identify: identify::Behaviour::new(identify::Config::new(
                PROTOCOL.to_string(),
                kp.public(),
            )),
            ping: ping::Behaviour::default(),
            dcutr: dcutr::Behaviour::new(kp.public().to_peer_id()),
        })?
        .build();
    Ok(swarm)
}

/// wasm-bindgen entry: constructing the swarm is the smoke check that the browser bundle
/// links — it exercises the full transport+behaviour wiring at load time.
#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    let _swarm = build_browser_relay_client().map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(())
}
