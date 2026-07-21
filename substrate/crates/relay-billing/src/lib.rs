//! 333 v2 substrate — meter REAL relayed bytes (closes the metering↔relay wiring on the real relay,
//! not the in-process Loopback). A request-response payload of known size is forwarded over an
//! actual Circuit-Relay-v2 connection (the relay SERVER is `p333-relay`), and on delivery the bytes
//! are metered + billed as a credits-as-owned-objects debit (`p333-billing`). The relay genuinely
//! forwarded the bytes — proven by the response arriving over the relayed path.

use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::NetworkBehaviour;
use libp2p::{identify, noise, ping, relay, tcp, yamux, PeerId, StreamProtocol, Swarm, SwarmBuilder};
use std::time::Duration;

/// The request-response protocol carrying the metered payload over the relayed connection.
pub const RR_PROTOCOL: &str = "/p333/relay-billing/1.0.0";
/// Identify protocol (informational; does not need to match the relay's).
pub const ID_PROTOCOL: &str = "/p333/relay-billing-id/0.1.0";

/// A relay CLIENT that also speaks a request-response data protocol — so it can push (and meter)
/// real bytes over the relayed connection. Request and response are raw byte vectors (CBOR-framed).
#[derive(NetworkBehaviour)]
pub struct MeteredClient {
    pub relay_client: relay::client::Behaviour,
    pub rr: request_response::cbor::Behaviour<Vec<u8>, Vec<u8>>,
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
}

/// Build a metered relay-client swarm (TCP + relay transport + request-response).
pub fn build_metered_client() -> anyhow::Result<(Swarm<MeteredClient>, PeerId)> {
    let swarm = SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|kp, relay_client| MeteredClient {
            relay_client,
            rr: request_response::cbor::Behaviour::new(
                [(StreamProtocol::new(RR_PROTOCOL), ProtocolSupport::Full)],
                request_response::Config::default(),
            ),
            identify: identify::Behaviour::new(identify::Config::new(
                ID_PROTOCOL.to_string(),
                kp.public(),
            )),
            ping: ping::Behaviour::default(),
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();
    let id = *swarm.local_peer_id();
    Ok((swarm, id))
}
