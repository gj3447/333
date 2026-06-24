//! 333 v2 substrate reachability — PROM step 0 (step 1 scope), receipt #3.
//!
//! Circuit-Relay-v2: a node that is not directly dialable becomes reachable to others
//! THROUGH a relay. This is the mechanism browser / NAT'd peers depend on — the survey
//! was blunt that "browser IS the server" is false and that reachability lives on the
//! always-on relay (Super-Peer) tier.
//!
//! HONEST SCOPE: DCUtR is wired (it upgrades a relayed connection to a direct one WHEN
//! a direct path is reachable), but a genuine NAT hole-punch PROOF needs simulated NAT
//! (separate network namespaces) and is deferred to a netns/browser integration
//! milestone — see README. On a single host both peers are directly dialable, so we do
//! NOT claim hole-punching here; this receipt proves relay RESERVATION + relayed
//! CONNECTIVITY end-to-end (a ping survives the relayed path).

use libp2p::swarm::NetworkBehaviour;
use libp2p::{dcutr, identify, noise, ping, relay, tcp, yamux, PeerId, Swarm, SwarmBuilder};
use std::time::Duration;

/// Identify protocol string for the 333 substrate.
pub const PROTOCOL: &str = "/p333/reachability/0.1.0";

/// The relay SERVER behaviour (the always-on Super-Peer role).
#[derive(NetworkBehaviour)]
pub struct RelayServer {
    pub relay: relay::Behaviour,
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
}

/// The relay CLIENT behaviour (an ephemeral peer that reserves/relays + tries DCUtR).
#[derive(NetworkBehaviour)]
pub struct RelayClient {
    pub relay_client: relay::client::Behaviour,
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
    pub dcutr: dcutr::Behaviour,
}

/// Build a relay-server swarm (TCP), returning it and its PeerId.
pub fn build_relay_server() -> anyhow::Result<(Swarm<RelayServer>, PeerId)> {
    let swarm = SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
        .with_behaviour(|kp| RelayServer {
            relay: relay::Behaviour::new(kp.public().to_peer_id(), Default::default()),
            identify: identify::Behaviour::new(identify::Config::new(
                PROTOCOL.to_string(),
                kp.public(),
            )),
            ping: ping::Behaviour::default(),
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();
    let peer_id = *swarm.local_peer_id();
    Ok((swarm, peer_id))
}

/// Build a relay-client swarm (TCP + relay transport + DCUtR), returning it and its PeerId.
pub fn build_relay_client() -> anyhow::Result<(Swarm<RelayClient>, PeerId)> {
    let swarm = SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|kp, relay_client| RelayClient {
            relay_client,
            identify: identify::Behaviour::new(identify::Config::new(
                PROTOCOL.to_string(),
                kp.public(),
            )),
            ping: ping::Behaviour::default(),
            dcutr: dcutr::Behaviour::new(kp.public().to_peer_id()),
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();
    let peer_id = *swarm.local_peer_id();
    Ok((swarm, peer_id))
}
