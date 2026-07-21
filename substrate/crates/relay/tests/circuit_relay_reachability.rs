//! DIAGNOSTIC build — instrumented to locate where the relay scenario stalls.
use futures::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::SwarmEvent;
use libp2p::{ping, relay, Multiaddr};
use p333_relay::{build_relay_client, build_relay_server, RelayClientEvent};
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn listener_reachable_via_circuit_relay_then_ping_succeeds() {
    let (mut relay, relay_id) = build_relay_server().expect("relay server");
    let (mut listener, listener_id) = build_relay_client().expect("listener");
    let (mut dialer, dialer_id) = build_relay_client().expect("dialer");
    eprintln!("R={relay_id} L={listener_id} D={dialer_id}");

    relay
        .listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap())
        .expect("relay listen");

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
                            // FIX: confirm the relay's own address as external. On localhost there is no
                            // AutoNAT to confirm it, so relay::Behaviour has only a NewExternalAddrCandidate
                            // and puts ZERO addresses in the reservation -> the client's circuit listener
                            // closes with Reservation(NoAddressesInReservation). Confirming it makes the
                            // reservation carry a real addr so the listener stays reachable via the relay.
                            relay.add_external_address(address.clone());
                            let circuit = address
                                .with(Protocol::P2p(relay_id))
                                .with(Protocol::P2pCircuit);
                            eprintln!("[L] listen_on circuit {circuit}");
                            listener.listen_on(circuit).expect("listener reserve");
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
                            dialer.dial(relayed).expect("dial via relay");
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
        .expect("scenario timed out");
}
