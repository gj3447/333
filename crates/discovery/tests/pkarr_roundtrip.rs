//! PROM step 0 · receipt #2 — PKARR publish/resolve round-trip over a LOCAL DHT.
//!
//! One Ed25519 root: the DID (libp2p PeerId, `12D3KooW…`) and the pkarr key are two
//! encodings of the SAME public key. We announce a Super-Peer multiaddr under that
//! key and resolve it back.
//!
//! Deterministic / no-fake-green: uses `mainline::Testnet` (an in-process DHT), never
//! the public Mainline network, so the round-trip is a real DHT put+get with no
//! external dependency. The seed/public are RFC 8032 §7.1 Test 1 (shared with the
//! identity crate's receipt #1), tying the two receipts to one identity.

use p333_discovery::{keypair_from_seed, publish, resolve_superpeer, superpeer_packet};
use pkarr::{Client, Keypair};

const T1_SEED: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const T1_PUBLIC: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

fn arr32(s: &str) -> [u8; 32] {
    hex::decode(s).unwrap().try_into().unwrap()
}

/// The pkarr key and the DID are the SAME Ed25519 key, just encoded differently.
#[test]
fn pkarr_key_shares_the_did_ed25519_root() {
    let kp = keypair_from_seed(&arr32(T1_SEED));
    assert_eq!(
        hex::encode(kp.public_key().to_bytes()),
        T1_PUBLIC,
        "pkarr public key must be the RFC 8032 Test 1 Ed25519 public key"
    );
    let did = p333_identity::ed25519_did(&kp.public_key().to_bytes()).unwrap();
    assert!(
        did.starts_with("12D3KooW"),
        "the DID derived from the same key must be an Ed25519 libp2p PeerId"
    );
}

/// Announce a Super-Peer multiaddr, then resolve it back from the DHT.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_then_resolve_superpeer_multiaddr() {
    let testnet = mainline::Testnet::new(3).expect("local dht testnet");
    // Two independent clients sharing only the testnet: publishing from one and
    // resolving from the OTHER forces a real DHT put+get (no local-cache shortcut),
    // which is what makes this a genuine round-trip receipt, not a serialize-then-read.
    let publisher = Client::builder()
        .no_default_network()
        .bootstrap(testnet.bootstrap.as_slice())
        .build()
        .expect("publisher client");
    let resolver = Client::builder()
        .no_default_network()
        .bootstrap(testnet.bootstrap.as_slice())
        .build()
        .expect("resolver client");

    let kp = keypair_from_seed(&arr32(T1_SEED));
    let did = p333_identity::ed25519_did(&kp.public_key().to_bytes()).unwrap();
    let multiaddr = format!("/ip4/127.0.0.1/udp/4001/quic-v1/p2p/{did}");

    let packet = superpeer_packet(&kp, &multiaddr).expect("signed packet");
    publish(&publisher, &packet).await.expect("publish to dht");

    let resolved = resolve_superpeer(&resolver, &kp.public_key())
        .await
        .expect("resolve ok");
    assert_eq!(
        resolved.as_deref(),
        Some(multiaddr.as_str()),
        "the announced Super-Peer multiaddr must round-trip through PKARR"
    );

    // a key that never announced must resolve to nothing (no phantom records)
    let stranger = Keypair::random();
    let none = resolve_superpeer(&resolver, &stranger.public_key())
        .await
        .expect("resolve ok");
    assert!(none.is_none(), "an unpublished key must not resolve to a location");
}
