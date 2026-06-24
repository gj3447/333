//! PROM · receipt — meter REAL relayed bytes. A request-response payload is forwarded over an
//! actual Circuit-Relay-v2 connection (relay server = p333-relay), and on delivery it is metered +
//! billed (p333-billing). The relay genuinely forwarded the bytes — proven by the ack arriving over
//! the relayed path — so this is the in-process Loopback receipt, now on the real relay. The verdict
//! is read back from the LTDD store, not the swarm's return value.

use futures::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, Message};
use libp2p::swarm::SwarmEvent;
use libp2p::Multiaddr;
use p333_billing::Account;
use p333_ltdd::{verify_present, MemoryStore, Store, Verdict};
use p333_relay::build_relay_server;
use p333_relay_billing::{build_metered_client, MeteredClientEvent};
use std::time::Duration;
use tokio::time::timeout;

const PAYLOAD_BYTES: usize = 4096; // cost(4096, rate 1) = 4 credits

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_relayed_payload_is_metered_and_billed() {
    let (mut relay, relay_id) = build_relay_server().expect("relay server");
    let (mut listener, listener_id) = build_metered_client().expect("listener");
    let (mut dialer, _dialer_id) = build_metered_client().expect("dialer");

    relay.listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap()).expect("relay listen");

    // billing state the dialer charges as the relayed bytes are confirmed delivered.
    let mut store = MemoryStore::default();
    let mut account = Account::new("relay-credits", 100);
    let cid = "relayed-session";

    let scenario = async {
        let mut relay_addr: Option<Multiaddr> = None;
        let mut reserved = false;
        let mut dialed = false;
        let mut sent = false;

        loop {
            tokio::select! {
                ev = relay.select_next_some() => {
                    if let SwarmEvent::NewListenAddr { address, .. } = ev {
                        if relay_addr.is_none() {
                            relay_addr = Some(address.clone());
                            relay.add_external_address(address.clone()); // so the reservation carries a real addr
                            let circuit = address.with(Protocol::P2p(relay_id)).with(Protocol::P2pCircuit);
                            listener.listen_on(circuit).expect("listener reserve");
                        }
                    }
                }
                ev = listener.select_next_some() => {
                    match ev {
                        SwarmEvent::Behaviour(MeteredClientEvent::RelayClient(
                            libp2p::relay::client::Event::ReservationReqAccepted { .. })) => {
                            reserved = true;
                            if !dialed {
                                let relayed = relay_addr.clone().unwrap()
                                    .with(Protocol::P2p(relay_id))
                                    .with(Protocol::P2pCircuit)
                                    .with(Protocol::P2p(listener_id));
                                dialer.dial(relayed).expect("dial via relay");
                                dialed = true;
                            }
                        }
                        // the listener ACKs the relayed payload it received
                        SwarmEvent::Behaviour(MeteredClientEvent::Rr(request_response::Event::Message {
                            message: Message::Request { request, channel, .. }, ..
                        })) => {
                            assert_eq!(request.len(), PAYLOAD_BYTES, "relay forwarded the full payload");
                            listener.behaviour_mut().rr.send_response(channel, vec![1u8]).ok();
                        }
                        _ => {}
                    }
                }
                ev = dialer.select_next_some() => {
                    match ev {
                        SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == listener_id && !sent => {
                            // the relayed connection is up: push PAYLOAD_BYTES over it
                            dialer.behaviour_mut().rr.send_request(&listener_id, vec![7u8; PAYLOAD_BYTES]);
                            sent = true;
                        }
                        // the ack proves the bytes traversed the relay end-to-end: meter + bill them
                        SwarmEvent::Behaviour(MeteredClientEvent::Rr(request_response::Event::Message {
                            message: Message::Response { .. }, ..
                        })) => {
                            assert!(reserved);
                            account.meter_and_bill(&mut store, cid, PAYLOAD_BYTES as u64, 1);
                            return;
                        }
                        _ => {}
                    }
                }
            }
        }
    };

    timeout(Duration::from_secs(30), scenario).await.expect("relayed billing scenario timed out");

    // the verdict, read back from the store: the relayed bytes were metered + billed.
    assert_eq!(account.balance(), 100 - 4, "cost(4096,1)=4 credits debited for the relayed payload");
    assert_eq!(verify_present(&store, cid, "relay_forwarded", 1), Verdict::Present);
    assert_eq!(verify_present(&store, cid, "credit_debited", 1), Verdict::Present);
    assert_eq!(verify_present(&store, cid, "spend_finalized", 1), Verdict::Present); // a finalized owned-object debit
    let debited: u64 = store.query(cid).iter().filter(|e| e.event == "credit_debited")
        .filter_map(|e| e.attrs.get("amount").and_then(|v| v.as_u64())).sum();
    assert_eq!(debited, 4);
}
