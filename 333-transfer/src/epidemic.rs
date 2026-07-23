// KG: transport-plan Step 4 (2026-07-14)
//
// Plumtree-style epidemic dissemination for transfer authority messages.
// Reuses `gossip333::InMemoryPlumtree` (eager-push + lazy IHAVE + GRAFT pull +
// PRUNE on duplicate). Maps transfer AuthorityId strings ↔ identity333::NodeId
// via a deterministic hash (NodeId is a 32-byte key, not a string).
//
// Delivery:
//   Event::Eager  → InMemoryAuthorityMesh::deliver_to_from(to, Some(me), payload)
//   Event::IHave / Graft / Prune → targeted control plane (same mesh peer set)
// On receive: feed into local Plumtree and re-emit. Deterministic tick/round —
// no tokio, no wall-clock.
//
// Composes with existing certify flow: `EpidemicEndpoint: AuthorityNet` can
// replace full-fanout `MeshEndpoint` broadcasts.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use gossip333::{Event, GossipProtocol, InMemoryPlumtree, Msg, MsgId};
use identity333::NodeId;

use crate::authority::{Certificate, Vote};
use crate::effect::EffectAttestation;
use crate::net::{AuthorityNet, InMemoryAuthorityMesh, NetError};
use crate::wire::{decode_authority_msg, encode_authority_msg, AuthorityMsg};
use crate::SignedTransfer;

/// Deterministic AuthorityId → gossip NodeId bridge.
///
/// `identity333::NodeId` is a 32-byte Ed25519 key blob, not a string. Transfer
/// peers use free-form AuthorityId strings (`"a0"`, `"client"`). We hash the
/// string into 32 bytes so Plumtree can key peers without changing gossip333.
pub fn peer_node_id(peer: &str) -> NodeId {
    NodeId::from_bytes(stable_hash32(peer.as_bytes()))
}

/// Content-addressed id for an encoded AuthorityMsg payload (32 bytes).
pub fn msg_id_of(payload: &[u8]) -> MsgId {
    stable_hash32(payload)
}

/// Deterministic non-crypto 32-byte mix (debug-overflow-safe; no new hash dep).
fn stable_hash32(input: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    // FNV-1a 64-bit seeds expanded across 4 lanes so short ids stay distinct.
    let mut h = 0xcbf29ce484222325u64;
    for &b in input {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= input.len() as u64;
    h = h.wrapping_mul(0x100000001b3);
    for i in 0..4u64 {
        let mut x = h.wrapping_add(i.wrapping_mul(0x9e3779b97f4a7c15));
        x ^= x >> 30;
        x = x.wrapping_mul(0xbf58476d1ce4e5b9);
        x ^= x >> 27;
        x = x.wrapping_mul(0x94d049bb133111eb);
        x ^= x >> 31;
        let i = i as usize;
        out[i * 8..(i + 1) * 8].copy_from_slice(&x.to_le_bytes());
    }
    for (i, &b) in input.iter().enumerate() {
        out[i % 32] ^= b.wrapping_mul(31).wrapping_add(i as u8);
        let j = (i.wrapping_mul(7) + 13) % 32;
        out[j] = out[j].wrapping_add(b.wrapping_mul(3));
    }
    out
}

#[derive(Debug, Clone)]
enum ControlMsg {
    IHave { from: String, id: MsgId, origin: String },
    Graft { from: String, id: MsgId },
    Prune { from: String },
}

#[derive(Default)]
struct ControlPlane {
    mailboxes: HashMap<String, VecDeque<ControlMsg>>,
}

/// Shared epidemic fabric: underlying targeted mesh + Plumtree control plane.
pub struct EpidemicNetwork {
    mesh: Arc<InMemoryAuthorityMesh>,
    control: Mutex<ControlPlane>,
}

impl EpidemicNetwork {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            mesh: InMemoryAuthorityMesh::new(),
            control: Mutex::new(ControlPlane::default()),
        })
    }

    pub fn mesh(&self) -> &Arc<InMemoryAuthorityMesh> {
        &self.mesh
    }

    /// Register `peer` on the mesh + control plane; return an epidemic endpoint.
    pub fn join(self: &Arc<Self>, peer: impl Into<String>) -> EpidemicEndpoint {
        let id = peer.into();
        let _mesh_ep = self.mesh.join(id.clone());
        {
            let mut g = self.control.lock().expect("epidemic control lock");
            g.mailboxes.entry(id.clone()).or_default();
        }
        let plumtree = InMemoryPlumtree::new(peer_node_id(&id));
        EpidemicEndpoint {
            me: id,
            net: Arc::clone(self),
            plumtree,
            peers: Mutex::new(HashMap::new()),
            app_inbox: Mutex::new(Vec::new()),
            // How many times each msg id was delivered to the app (dedup tests).
            deliver_counts: Mutex::new(HashMap::new()),
        }
    }

    fn control_deliver(&self, to: &str, msg: ControlMsg) -> Result<(), NetError> {
        let mut g = self.control.lock().expect("epidemic control lock");
        let mb = g
            .mailboxes
            .get_mut(to)
            .ok_or_else(|| NetError::UnknownPeer(to.to_owned()))?;
        mb.push_back(msg);
        Ok(())
    }

    fn control_drain(&self, me: &str) -> Vec<ControlMsg> {
        let mut g = self.control.lock().expect("epidemic control lock");
        g.mailboxes
            .get_mut(me)
            .map(|q| q.drain(..).collect())
            .unwrap_or_default()
    }
}

/// Per-peer Plumtree + mesh handle implementing [`AuthorityNet`].
///
/// Call [`Self::process`] (or free [`pump`]) between logical rounds so IHAVE →
/// GRAFT recovery and multi-hop eager forwarding make progress.
pub struct EpidemicEndpoint {
    me: String,
    net: Arc<EpidemicNetwork>,
    plumtree: InMemoryPlumtree,
    peers: Mutex<HashMap<NodeId, String>>,
    app_inbox: Mutex<Vec<AuthorityMsg>>,
    deliver_counts: Mutex<HashMap<MsgId, usize>>,
}

impl EpidemicEndpoint {
    pub fn id(&self) -> &str {
        &self.me
    }

    pub fn plumtree(&self) -> &InMemoryPlumtree {
        &self.plumtree
    }

    /// True if this peer has stored `id` in the Plumtree seen set.
    pub fn has_msg(&self, id: &MsgId) -> bool {
        self.plumtree.get(id).is_some()
    }

    /// Add `peer` to the eager push set (full mesh for honest path).
    pub fn add_peer(&self, peer: &str) {
        if peer == self.me {
            return;
        }
        let n = peer_node_id(peer);
        self.peers.lock().expect("peers").insert(n, peer.to_owned());
        self.plumtree.add_peer(n);
    }

    /// Remove peer from both eager and lazy views.
    pub fn remove_peer(&self, peer: &str) {
        let n = peer_node_id(peer);
        self.peers.lock().expect("peers").remove(&n);
        self.plumtree.remove_peer(&n);
    }

    /// Demote `peer` from eager → lazy (dropped eager edge). IHAVE still flows.
    ///
    /// This is the Step 4 fault model: A can no longer eager-deliver to D, but
    /// lazy announcements + GRAFT recovery still reach D.
    pub fn remove_eager_edge(&self, peer: &str) {
        let n = peer_node_id(peer);
        // Ensure the peer is known so on_prune demotes rather than no-ops on empty.
        {
            let mut peers = self.peers.lock().expect("peers");
            peers.entry(n).or_insert_with(|| peer.to_owned());
        }
        if !self.plumtree.eager_peers().contains(&n) && !self.plumtree.lazy_peers().contains(&n) {
            self.plumtree.add_peer(n);
        }
        self.plumtree.on_prune(n);
    }

    pub fn eager_peers(&self) -> Vec<String> {
        self.plumtree
            .eager_peers()
            .into_iter()
            .filter_map(|n| self.peers.lock().expect("peers").get(&n).cloned())
            .collect()
    }

    pub fn lazy_peers(&self) -> Vec<String> {
        self.plumtree
            .lazy_peers()
            .into_iter()
            .filter_map(|n| self.peers.lock().expect("peers").get(&n).cloned())
            .collect()
    }

    /// Application-level delivery count for a message id (dedup assertions).
    pub fn deliver_count(&self, id: &MsgId) -> usize {
        self.deliver_counts
            .lock()
            .expect("deliver_counts")
            .get(id)
            .copied()
            .unwrap_or(0)
    }

    fn resolve_peer(&self, n: &NodeId) -> Result<String, NetError> {
        self.peers
            .lock()
            .expect("peers")
            .get(n)
            .cloned()
            .ok_or_else(|| NetError::UnknownPeer(format!("node:{n}")))
    }

    fn dispatch_events(&self, events: Vec<Event>) -> Result<(), NetError> {
        for ev in events {
            match ev {
                Event::Eager { to, msg } => {
                    let peer = self.resolve_peer(&to)?;
                    let auth = decode_authority_msg(&msg.payload).map_err(|e| {
                        NetError::Io(format!("epidemic eager payload decode: {e:?}"))
                    })?;
                    self.net
                        .mesh
                        .deliver_to_from(&peer, Some(&self.me), auth)?;
                }
                Event::IHave { to, id, sender } => {
                    let peer = self.resolve_peer(&to)?;
                    let origin = self
                        .resolve_peer(&sender)
                        .unwrap_or_else(|_| self.me.clone());
                    self.net.control_deliver(
                        &peer,
                        ControlMsg::IHave {
                            from: self.me.clone(),
                            id,
                            origin,
                        },
                    )?;
                }
                Event::Graft { to, id } => {
                    let peer = self.resolve_peer(&to)?;
                    self.net.control_deliver(
                        &peer,
                        ControlMsg::Graft {
                            from: self.me.clone(),
                            id,
                        },
                    )?;
                }
                Event::Prune { to } => {
                    let peer = self.resolve_peer(&to)?;
                    self.net.control_deliver(
                        &peer,
                        ControlMsg::Prune {
                            from: self.me.clone(),
                        },
                    )?;
                }
            }
        }
        Ok(())
    }

    fn push_app(&self, msg: AuthorityMsg, id: MsgId) {
        {
            let mut counts = self.deliver_counts.lock().expect("deliver_counts");
            *counts.entry(id).or_insert(0) += 1;
        }
        self.app_inbox.lock().expect("app_inbox").push(msg);
    }

    fn on_eager_payload(&self, from: &str, msg: AuthorityMsg) -> Result<(), NetError> {
        // Remember hop so Graft/Prune/IHave reverse lookups work.
        {
            let n = peer_node_id(from);
            self.peers
                .lock()
                .expect("peers")
                .entry(n)
                .or_insert_with(|| from.to_owned());
        }
        let payload = encode_authority_msg(&msg);
        let id = msg_id_of(&payload);
        let already = self.plumtree.get(&id).is_some();
        let gossip_msg = Msg {
            id,
            payload,
            sender: peer_node_id(from),
        };
        let events = self.plumtree.on_receive(gossip_msg);
        if !already {
            self.push_app(msg, id);
        }
        self.dispatch_events(events)
    }

    fn on_control(&self, ctrl: ControlMsg) -> Result<(), NetError> {
        match ctrl {
            ControlMsg::IHave { from, id, origin } => {
                {
                    let n = peer_node_id(&from);
                    self.peers
                        .lock()
                        .expect("peers")
                        .entry(n)
                        .or_insert_with(|| from.clone());
                }
                let events =
                    self.plumtree
                        .on_ihave(peer_node_id(&from), id, peer_node_id(&origin));
                self.dispatch_events(events)
            }
            ControlMsg::Graft { from, id } => {
                {
                    let n = peer_node_id(&from);
                    self.peers
                        .lock()
                        .expect("peers")
                        .entry(n)
                        .or_insert_with(|| from.clone());
                }
                // GRAFT is double-purpose (Plumtree): retransmission trigger
                // AND tree-edge repair. `on_graft` re-promotes `from` from lazy
                // back to eager (dual of PRUNE) and returns an Event::Eager
                // retransmitting the body when we hold it — unknown ids yield
                // no events (no panic, no reply), matching the old bare-`get`
                // behavior. The retransmission then flows through the same
                // Event::Eager dispatch path as any eager push.
                let events = self.plumtree.on_graft(peer_node_id(&from), id);
                self.dispatch_events(events)
            }
            ControlMsg::Prune { from } => {
                self.plumtree.on_prune(peer_node_id(&from));
                Ok(())
            }
        }
    }

    /// One deterministic step: drain mesh payloads + control, handle, re-emit.
    /// Does **not** call Plumtree `tick` (IHAVE→GRAFT); use [`Self::tick`] for that.
    pub fn process(&self) -> Result<(), NetError> {
        let hops = self.net.mesh.drain_with_hops(&self.me);
        for (from, msg) in hops {
            let from = from.unwrap_or_else(|| self.me.clone());
            self.on_eager_payload(&from, msg)?;
        }
        for ctrl in self.net.control_drain(&self.me) {
            self.on_control(ctrl)?;
        }
        Ok(())
    }

    /// Drain pending IHAVEs into GRAFT events and dispatch them.
    pub fn tick(&self) -> Result<(), NetError> {
        let events = self.plumtree.tick();
        self.dispatch_events(events)
    }

    /// `process` then `tick` — one full epidemic round.
    pub fn process_and_tick(&self) -> Result<(), NetError> {
        self.process()?;
        self.tick()
    }

    fn broadcast_msg(&self, msg: AuthorityMsg) -> Result<(), NetError> {
        let payload = encode_authority_msg(&msg);
        let id = msg_id_of(&payload);
        // Local originator sees its own message once (full-mesh fanout parity).
        let already = self.plumtree.get(&id).is_some();
        let events = self.plumtree.broadcast(payload, id);
        if !already {
            self.push_app(msg, id);
        }
        self.dispatch_events(events)
    }
}

impl AuthorityNet for EpidemicEndpoint {
    fn broadcast_order(&self, order: SignedTransfer) -> Result<(), NetError> {
        self.broadcast_msg(AuthorityMsg::Order(order))
    }

    fn broadcast_vote(&self, v: Vote) -> Result<(), NetError> {
        self.broadcast_msg(AuthorityMsg::Vote(v))
    }

    fn broadcast_cert(&self, c: Certificate) -> Result<(), NetError> {
        self.broadcast_msg(AuthorityMsg::Cert(c))
    }

    fn broadcast_attestation(&self, a: EffectAttestation) -> Result<(), NetError> {
        self.broadcast_msg(AuthorityMsg::Attestation(a))
    }

    fn poll(&self) -> Vec<AuthorityMsg> {
        // Process pending network traffic first so poll sees freshly delivered msgs.
        let _ = self.process();
        std::mem::take(&mut *self.app_inbox.lock().expect("app_inbox"))
    }
}

/// Fully connect `endpoints` as an eager Plumtree mesh (committee-full eager set).
pub fn mesh_all_eager(endpoints: &[&EpidemicEndpoint]) {
    for ep in endpoints {
        for other in endpoints {
            if ep.id() != other.id() {
                ep.add_peer(other.id());
            }
        }
    }
}

/// Drive epidemic progress for `rounds` deterministic steps (process + tick each).
pub fn pump(endpoints: &[&EpidemicEndpoint], rounds: usize) {
    for _ in 0..rounds {
        for ep in endpoints {
            let _ = ep.process_and_tick();
        }
    }
}

/// Wire a committee where `from`→`to` is **lazy only** (dropped eager edge), and
/// no other peer has `to` as eager — so a broadcast from `from` reaches `to`
/// only via IHAVE → GRAFT (not multi-hop eager). Remaining pairs among
/// `{endpoints \ to}` stay full-eager.
pub fn mesh_with_dropped_eager_edge(endpoints: &[&EpidemicEndpoint], from: &str, to: &str) {
    let ids: Vec<String> = endpoints.iter().map(|e| e.id().to_owned()).collect();
    for ep in endpoints {
        for other in &ids {
            if ep.id() == other.as_str() {
                continue;
            }
            if other == to && ep.id() != to {
                // Everyone who is not `to`: know `to` only as lazy (IHAVE path).
                ep.add_peer(other);
                ep.remove_eager_edge(other);
            } else if ep.id() == to {
                // `to` keeps peers so Graft reverse delivery resolves names.
                ep.add_peer(other);
            } else {
                // Core committee (excluding the isolated `to` target): full eager.
                ep.add_peer(other);
            }
        }
    }
    // Explicit: `from`→`to` must be lazy (already done above). Sanity for callers.
    let _ = (from, to);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NetworkId, OwnerRegistry, Transfer, TransferPolicy};
    use ed25519_dalek::SigningKey;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn policy() -> TransferPolicy {
        TransferPolicy::new(
            NetworkId::new("epidemic-testnet").unwrap(),
            OwnerRegistry::new([
                ("alice", key(42).verifying_key()),
                ("bob", key(43).verifying_key()),
            ])
            .unwrap(),
        )
    }

    fn signed_tx(policy: &TransferPolicy) -> SignedTransfer {
        SignedTransfer::sign(
            policy,
            Transfer {
                from: "alice".into(),
                from_seq: 0,
                to: "bob".into(),
                amount: 1,
            },
            &key(42),
        )
    }

    #[test]
    fn peer_node_id_stable_and_distinct() {
        assert_eq!(peer_node_id("a0"), peer_node_id("a0"));
        assert_ne!(peer_node_id("a0"), peer_node_id("a1"));
        assert_ne!(peer_node_id("a0"), peer_node_id("client"));
    }

    #[test]
    fn msg_id_stable() {
        let policy = policy();
        let p = encode_authority_msg(&AuthorityMsg::Order(signed_tx(&policy)));
        assert_eq!(msg_id_of(&p), msg_id_of(&p));
    }
}
