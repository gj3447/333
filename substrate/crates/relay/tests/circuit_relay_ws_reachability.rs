//! Browser-reachability server-side half: a relay that listens over WebSocket accepts a
//! Circuit-Relay-v2 RESERVATION from a WS client, and a (WS) dialer reaches the reserved
//! listener THROUGH the relay (a ping survives the relayed `/ws` path).
//!
//! This is the native stand-in for the browser path: the browser wasm client
//! (crates/relay-client-wasm) is dial-only and reserves on a relay it dials OUT to over
//! WebSocket. Here both clients are native and use the `/ws` transport, proving the relay
//! offers a browser-class WS listener and that reservation + relayed connectivity work
//! over WebSocket exactly as they do over TCP. (HONEST SCOPE: native WS, not a real
//! browser; WSS-with-cert + headless-Chrome e2e remain deferred frontier sub-receipts.)
use futures::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::SwarmEvent;
use libp2p::{ping, relay, Multiaddr};
use p333_relay::{build_relay_client_ws, build_relay_server_ws, RelayClientEvent};
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn listener_reachable_via_ws_circuit_relay_then_ping_succeeds() {
    let (mut relay, relay_id) = build_relay_server_ws().await.expect("relay server (ws)");
    let (mut listener, listener_id) = build_relay_client_ws().await.expect("listener (ws)");
    let (mut dialer, dialer_id) = build_relay_client_ws().await.expect("dialer (ws)");
    eprintln!("R={relay_id} L={listener_id} D={dialer_id}");

    relay
        .listen_on("/ip4/127.0.0.1/tcp/0/ws".parse().unwrap())
        .expect("relay ws listen");

    let scenario = async {
        let mut relay_addr: Option<Multiaddr> = None;
        let mut reservation_ok = false;
        let mut dialed = false;
        let mut connected_via_relay = false;

        loop {
            tokio::select! {
                ev = relay.select_next_some() => {
                    eprintln!("[R] {ev:?}");
                    if let SwarmEvent::NewListenAddr { address, .. } = ev {
                        if relay_addr.is_none() {
                            relay_addr = Some(address.clone());
                            // Same fix as the TCP receipt: on localhost there is no AutoNAT
                            // to confirm the relay's own address, so confirm it ourselves or
                            // the reservation carries 0 addresses (NoAddressesInReservation).
                            relay.add_external_address(address.clone());
                            let circuit = address
                                .with(Protocol::P2p(relay_id))
                                .with(Protocol::P2pCircuit);
                            eprintln!("[L] listen_on circuit {circuit}");
                            listener.listen_on(circuit).expect("listener reserve (ws)");
                        }
                    }
                }
                ev = listener.select_next_some() => {
                    eprintln!("[L] {ev:?}");
                    if let SwarmEvent::Behaviour(RelayClientEvent::RelayClient(
                        relay::client::Event::ReservationReqAccepted { .. })) = ev
                    {
                        reservation_ok = true;
                        if !dialed {
                            let relayed = relay_addr.clone().unwrap()
                                .with(Protocol::P2p(relay_id))
                                .with(Protocol::P2pCircuit)
                                .with(Protocol::P2p(listener_id));
                            eprintln!("[D] dial {relayed}");
                            dialer.dial(relayed).expect("dial via ws relay");
                            dialed = true;
                        }
                    }
                }
                ev = dialer.select_next_some() => {
                    eprintln!("[D] {ev:?}");
                    match ev {
                        SwarmEvent::ConnectionEstablished { peer_id, .. }
                            if peer_id == listener_id =>
                        {
                            connected_via_relay = true;
                        }
                        SwarmEvent::Behaviour(RelayClientEvent::Ping(ping::Event {
                            peer,
                            result: Ok(_),
                            ..
                        })) if peer == listener_id => {
                            assert!(reservation_ok);
                            assert!(connected_via_relay);
                            return;
                        }
                        _ => {}
                    }
                }
            }
        }
    };

    timeout(Duration::from_secs(20), scenario)
        .await
        .expect("ws scenario timed out");
}
