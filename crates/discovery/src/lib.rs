//! 333 v2 substrate discovery — PROM step 0, receipt #2.
//!
//! Announce/resolve a Super-Peer **location** for an identity via PKARR, keyed by the
//! SAME Ed25519 key as the DID. One root, two encodings: the libp2p PeerId
//! (`12D3KooW…`) addresses the *peer*; the pkarr key (z-base32) addresses its
//! *record*. PKARR records are ephemeral pointers — discovery only, never value
//! storage (respects the no-value-in-CRDT / no-value-in-discovery hard rule).

use pkarr::{Client, Keypair, PublicKey, SignedPacket};

/// DNS record name under which a Super-Peer multiaddr is announced.
pub const SUPERPEER_TXT: &str = "_p333";

/// Build a pkarr [`Keypair`] from a 32-byte Ed25519 seed — the same seed the DID uses.
pub fn keypair_from_seed(seed: &[u8; 32]) -> Keypair {
    Keypair::from_secret_key(seed)
}

/// Build a signed PKARR packet announcing a Super-Peer multiaddr for this identity.
pub fn superpeer_packet(keypair: &Keypair, multiaddr: &str) -> anyhow::Result<SignedPacket> {
    let packet = SignedPacket::builder()
        .txt(SUPERPEER_TXT.try_into()?, multiaddr.try_into()?, 3600)
        .sign(keypair)?;
    Ok(packet)
}

/// Publish the announcement to the DHT.
pub async fn publish(client: &Client, packet: &SignedPacket) -> anyhow::Result<()> {
    client.publish(packet, None).await?;
    Ok(())
}

/// Resolve the Super-Peer multiaddr announced by a public key, if any.
pub async fn resolve_superpeer(client: &Client, key: &PublicKey) -> anyhow::Result<Option<String>> {
    let Some(packet) = client.resolve(key).await else {
        return Ok(None);
    };
    for rr in packet.all_resource_records() {
        if let pkarr::dns::rdata::RData::TXT(txt) = &rr.rdata {
            return Ok(Some(String::try_from(txt.clone())?));
        }
    }
    Ok(None)
}
