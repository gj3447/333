// KG: SPAN_333_L12_Gossip, plan-333-p2p-os-synthesis-execution-2026-04-18
// 333 P2P OS L12 Gossip — Plumtree epidemic broadcaster.
//
// Source: finding_333_synth_rte_prt_d14 (HyParView+Plumtree+PeerScorer).
//
// Protocol:
//   - Broadcast message to all eager peers (full payload, tree-link).
//   - Send IHAVE announcements to lazy peers (hash only, cheap).
//   - Receive IHAVE for unknown msg → request full via GRAFT.
//   - Receive duplicate full msg → send PRUNE (demote sender to lazy).
//
// This crate ships trait + in-memory reference impl. Real WebRTC/libp2p
// transport wraps the `Event` queue in a downstream crate.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use identity333::NodeId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GossipError {
    #[error("peer not in view")]
    UnknownPeer,
    #[error("message already seen")]
    Duplicate,
}

/// Content-addressed message id. Production: SHA-256/BLAKE3 of payload.
/// Here: caller-provided 32-byte id (keeps crate hash-impl-free).
pub type MsgId = [u8; 32];

#[derive(Debug, Clone)]
pub struct Msg {
    pub id: MsgId,
    pub payload: Vec<u8>,
    pub sender: NodeId,
}

/// Outbound events the broadcaster emits; transport layer consumes them.
#[derive(Debug, Clone)]
pub enum Event {
    /// Full payload delivery to an eager peer (tree edge).
    Eager { to: NodeId, msg: Msg },
    /// Hash announcement to a lazy peer.
    IHave { to: NodeId, id: MsgId, sender: NodeId },
    /// Request missing full body.
    Graft { to: NodeId, id: MsgId },
    /// Demote sender to lazy view.
    Prune { to: NodeId },
}

pub trait GossipProtocol {
    /// Originate a message at this node; returns events to send.
    fn broadcast(&self, payload: Vec<u8>, id: MsgId) -> Vec<Event>;
    /// Receive a full payload; returns events (forward to eager, IHAVE to lazy,
    /// or PRUNE on duplicate).
    fn on_receive(&self, msg: Msg) -> Vec<Event>;
    /// Receive an IHAVE announcement; returns GRAFT if we want the body.
    fn on_ihave(&self, from: NodeId, id: MsgId, origin: NodeId) -> Vec<Event>;
    /// Receive a PRUNE; demote `from` from eager to lazy.
    fn on_prune(&self, from: NodeId);
    /// Lookup delivered payload.
    fn get(&self, id: &MsgId) -> Option<Vec<u8>>;
}

#[derive(Debug, Default)]
struct Inner {
    me: Option<NodeId>,
    eager: HashSet<NodeId>,
    lazy: HashSet<NodeId>,
    seen: HashMap<MsgId, Vec<u8>>,
    /// Messages announced via IHAVE but not yet requested; drained on timer.
    pending_ihave: VecDeque<(NodeId, MsgId, NodeId)>,
}

#[derive(Debug, Default)]
pub struct InMemoryPlumtree {
    inner: Arc<Mutex<Inner>>,
}

impl InMemoryPlumtree {
    pub fn new(me: NodeId) -> Self {
        let t = Self::default();
        t.inner.lock().unwrap().me = Some(me);
        t
    }

    pub fn add_peer(&self, peer: NodeId) {
        let mut g = self.inner.lock().unwrap();
        g.eager.insert(peer);
    }

    pub fn remove_peer(&self, peer: &NodeId) {
        let mut g = self.inner.lock().unwrap();
        g.eager.remove(peer);
        g.lazy.remove(peer);
    }

    pub fn eager_peers(&self) -> Vec<NodeId> {
        self.inner.lock().unwrap().eager.iter().cloned().collect()
    }

    pub fn lazy_peers(&self) -> Vec<NodeId> {
        self.inner.lock().unwrap().lazy.iter().cloned().collect()
    }

    pub fn pending_ihaves(&self) -> usize {
        self.inner.lock().unwrap().pending_ihave.len()
    }

    /// Drain outstanding IHAVEs; called by a timer in downstream transport.
    /// Returns GRAFT events for any announcements we still don't have.
    pub fn tick(&self) -> Vec<Event> {
        let mut g = self.inner.lock().unwrap();
        let mut out = Vec::new();
        let items: Vec<_> = g.pending_ihave.drain(..).collect();
        for (from, id, _origin) in items {
            if !g.seen.contains_key(&id) {
                out.push(Event::Graft { to: from, id });
            }
        }
        out
    }
}

impl GossipProtocol for InMemoryPlumtree {
    fn broadcast(&self, payload: Vec<u8>, id: MsgId) -> Vec<Event> {
        let mut g = self.inner.lock().unwrap();
        let me = g.me.clone().expect("initialize with new(me)");
        if g.seen.insert(id, payload.clone()).is_some() {
            return Vec::new();
        }
        let msg = Msg { id, payload, sender: me.clone() };
        let mut events = Vec::new();
        for p in &g.eager {
            events.push(Event::Eager { to: p.clone(), msg: msg.clone() });
        }
        for p in &g.lazy {
            events.push(Event::IHave { to: p.clone(), id, sender: me.clone() });
        }
        events
    }

    fn on_receive(&self, msg: Msg) -> Vec<Event> {
        let mut g = self.inner.lock().unwrap();
        let me = g.me.clone().expect("initialize with new(me)");
        if g.seen.contains_key(&msg.id) {
            // Duplicate: demote the sender to lazy (tree-pruning).
            if g.eager.remove(&msg.sender) {
                g.lazy.insert(msg.sender.clone());
                return vec![Event::Prune { to: msg.sender }];
            }
            return Vec::new();
        }
        g.seen.insert(msg.id, msg.payload.clone());
        let origin = msg.sender.clone();
        let payload = msg.payload.clone();
        let id = msg.id;
        let mut events = Vec::new();
        for p in &g.eager {
            if p != &origin {
                events.push(Event::Eager {
                    to: p.clone(),
                    msg: Msg { id, payload: payload.clone(), sender: me.clone() },
                });
            }
        }
        for p in &g.lazy {
            if p != &origin {
                events.push(Event::IHave { to: p.clone(), id, sender: me.clone() });
            }
        }
        events
    }

    fn on_ihave(&self, from: NodeId, id: MsgId, origin: NodeId) -> Vec<Event> {
        let mut g = self.inner.lock().unwrap();
        if g.seen.contains_key(&id) {
            return Vec::new();
        }
        // Queue for GRAFT on next tick.
        g.pending_ihave.push_back((from, id, origin));
        Vec::new()
    }

    fn on_prune(&self, from: NodeId) {
        let mut g = self.inner.lock().unwrap();
        if g.eager.remove(&from) {
            g.lazy.insert(from);
        }
    }

    fn get(&self, id: &MsgId) -> Option<Vec<u8>> {
        self.inner.lock().unwrap().seen.get(id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use identity333::Keypair;

    fn mk_node() -> NodeId {
        Keypair::generate().node_id()
    }

    #[test]
    fn broadcast_sends_eager_to_all() {
        let me = mk_node();
        let t = InMemoryPlumtree::new(me);
        t.add_peer(mk_node());
        t.add_peer(mk_node());
        let events = t.broadcast(b"hello".to_vec(), [1u8; 32]);
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| matches!(e, Event::Eager { .. })));
    }

    #[test]
    fn on_receive_forwards_to_other_peers_not_origin() {
        let me = mk_node();
        let p1 = mk_node();
        let p2 = mk_node();
        let p3 = mk_node();
        let t = InMemoryPlumtree::new(me);
        t.add_peer(p1.clone());
        t.add_peer(p2.clone());
        t.add_peer(p3.clone());
        let msg = Msg { id: [7u8; 32], payload: b"x".to_vec(), sender: p1.clone() };
        let events = t.on_receive(msg);
        // Forward to p2, p3 but not p1 (origin)
        assert_eq!(events.len(), 2);
        for e in &events {
            match e {
                Event::Eager { to, .. } => assert_ne!(to, &p1),
                _ => panic!("expected Eager"),
            }
        }
    }

    #[test]
    fn duplicate_receive_prunes_sender() {
        let me = mk_node();
        let p1 = mk_node();
        let t = InMemoryPlumtree::new(me);
        t.add_peer(p1.clone());
        let msg = Msg { id: [9u8; 32], payload: b"y".to_vec(), sender: p1.clone() };
        let _ = t.on_receive(msg.clone());
        // Second delivery from same peer = duplicate.
        let events = t.on_receive(msg);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Event::Prune { to } if to == &p1));
        assert!(!t.eager_peers().contains(&p1));
        assert!(t.lazy_peers().contains(&p1));
    }

    #[test]
    fn ihave_for_unknown_queues_graft_on_tick() {
        let me = mk_node();
        let from = mk_node();
        let t = InMemoryPlumtree::new(me);
        let _ = t.on_ihave(from.clone(), [2u8; 32], from.clone());
        assert_eq!(t.pending_ihaves(), 1);
        let evs = t.tick();
        assert_eq!(evs.len(), 1);
        assert!(matches!(&evs[0], Event::Graft { id, .. } if id == &[2u8; 32]));
    }

    #[test]
    fn ihave_for_known_no_graft() {
        let me = mk_node();
        let peer = mk_node();
        let t = InMemoryPlumtree::new(me);
        t.add_peer(peer.clone());
        let _ = t.broadcast(b"p".to_vec(), [3u8; 32]);
        assert!(t.on_ihave(peer.clone(), [3u8; 32], peer).is_empty());
        assert_eq!(t.pending_ihaves(), 0);
    }

    #[test]
    fn on_prune_demotes_to_lazy() {
        let me = mk_node();
        let p = mk_node();
        let t = InMemoryPlumtree::new(me);
        t.add_peer(p.clone());
        t.on_prune(p.clone());
        assert!(!t.eager_peers().contains(&p));
        assert!(t.lazy_peers().contains(&p));
    }

    #[test]
    fn broadcast_idempotent_on_known_id() {
        let me = mk_node();
        let t = InMemoryPlumtree::new(me);
        t.add_peer(mk_node());
        let ev1 = t.broadcast(b"a".to_vec(), [4u8; 32]);
        let ev2 = t.broadcast(b"a".to_vec(), [4u8; 32]);
        assert_eq!(ev1.len(), 1);
        assert_eq!(ev2.len(), 0);
    }

    #[test]
    fn get_returns_delivered_payload() {
        let me = mk_node();
        let t = InMemoryPlumtree::new(me);
        t.broadcast(b"stored".to_vec(), [5u8; 32]);
        assert_eq!(t.get(&[5u8; 32]), Some(b"stored".to_vec()));
        assert_eq!(t.get(&[6u8; 32]), None);
    }
}
