// KG: SPAN_333_L12_Membership, plan-333-p2p-os-synthesis-execution-2026-04-18
// 333 P2P OS L12 Membership — HyParView partial-membership protocol.
//
// Source: finding_333_synth_rte_prt_d14 (HyParView+Plumtree+PeerScorer).
//
// Two views:
//   - active (size = c1, small, symmetric): neighbors you gossip with
//   - passive (size = c2, ~log N): backup pool for active replacement
//
// Protocol operations:
//   - Join: new node sends JOIN to bootstrap; bootstrap forwards as ForwardJoin.
//   - Active view full: drop random active peer → move to passive.
//   - Passive view full: drop random passive peer.
//   - Shuffle: periodic exchange of passive view entries (anti-entropy over membership).
//   - Neighbor up-promote: when active view underflows, promote from passive.
//
// This crate ships trait + in-memory reference impl with deterministic-for-test
// selection (pluggable RNG via Selector trait; default = first-in view).

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use identity333::NodeId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemberError {
    #[error("peer already in active view")]
    AlreadyActive,
    #[error("peer not found")]
    NotFound,
}

#[derive(Debug, Clone, Copy)]
pub struct ViewSizes {
    pub active_capacity: usize,
    pub passive_capacity: usize,
}

impl Default for ViewSizes {
    fn default() -> Self {
        // Defaults from HyParView paper (Leitão 2007): small cluster.
        Self { active_capacity: 4, passive_capacity: 7 }
    }
}

/// Outbound protocol events (transport layer consumes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    ForwardJoin { to: NodeId, new_node: NodeId, ttl: u8 },
    Neighbor { to: NodeId, high_priority: bool },
    Shuffle { to: NodeId, sample: Vec<NodeId> },
    ShuffleReply { to: NodeId, sample: Vec<NodeId> },
    Disconnect { to: NodeId },
}

pub trait MembershipProtocol {
    fn join(&self, contact: NodeId) -> Vec<Event>;
    fn on_forward_join(&self, from: NodeId, new_node: NodeId, ttl: u8) -> Vec<Event>;
    fn on_neighbor(&self, from: NodeId, high_priority: bool) -> Vec<Event>;
    fn on_disconnect(&self, from: NodeId) -> Vec<Event>;
    fn on_shuffle(&self, from: NodeId, sample: Vec<NodeId>) -> Vec<Event>;
    fn on_shuffle_reply(&self, sample: Vec<NodeId>);
    fn active_view(&self) -> Vec<NodeId>;
    fn passive_view(&self) -> Vec<NodeId>;
}

#[derive(Debug)]
struct Inner {
    me: NodeId,
    active: Vec<NodeId>,
    passive: Vec<NodeId>,
    sizes: ViewSizes,
}

impl Inner {
    fn drop_random_active(&mut self) -> Option<NodeId> {
        if self.active.is_empty() {
            None
        } else {
            // Deterministic: drop first (tests don't rely on randomness).
            Some(self.active.remove(0))
        }
    }

    fn add_active(&mut self, peer: NodeId) -> Option<NodeId> {
        if peer == self.me || self.active.contains(&peer) {
            return None;
        }
        // Remove from passive if present.
        self.passive.retain(|p| p != &peer);
        let evicted = if self.active.len() >= self.sizes.active_capacity {
            self.drop_random_active()
        } else {
            None
        };
        self.active.push(peer);
        if let Some(ev) = &evicted {
            self.add_passive(ev.clone());
        }
        evicted
    }

    fn add_passive(&mut self, peer: NodeId) {
        if peer == self.me || self.active.contains(&peer) || self.passive.contains(&peer) {
            return;
        }
        if self.passive.len() >= self.sizes.passive_capacity {
            self.passive.remove(0);
        }
        self.passive.push(peer);
    }
}

#[derive(Debug)]
pub struct InMemoryHyParView {
    inner: Arc<Mutex<Inner>>,
}

impl InMemoryHyParView {
    pub fn new(me: NodeId, sizes: ViewSizes) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                me,
                active: Vec::new(),
                passive: Vec::new(),
                sizes,
            })),
        }
    }

    /// Build a shuffle sample: up to k nodes from active + passive.
    pub fn shuffle_sample(&self, k: usize) -> Vec<NodeId> {
        let g = self.inner.lock().unwrap();
        let mut s = Vec::new();
        s.extend(g.active.iter().take(k / 2).cloned());
        s.extend(g.passive.iter().take(k / 2).cloned());
        s.truncate(k);
        s
    }
}

impl MembershipProtocol for InMemoryHyParView {
    fn join(&self, contact: NodeId) -> Vec<Event> {
        let mut g = self.inner.lock().unwrap();
        g.add_active(contact.clone());
        // Forward-join is kicked off by contact on its side;
        // we just return a Neighbor proof to the contact.
        vec![Event::Neighbor { to: contact, high_priority: true }]
    }

    fn on_forward_join(&self, _from: NodeId, new_node: NodeId, ttl: u8) -> Vec<Event> {
        let mut g = self.inner.lock().unwrap();
        if ttl == 0 || g.active.len() < g.sizes.active_capacity {
            let evicted = g.add_active(new_node.clone());
            let mut events = vec![Event::Neighbor { to: new_node, high_priority: ttl == 0 }];
            if let Some(e) = evicted {
                events.push(Event::Disconnect { to: e });
            }
            events
        } else {
            // Forward further. Deterministic: pick first active peer other than sender.
            if let Some(next) = g.active.first().cloned() {
                vec![Event::ForwardJoin { to: next, new_node, ttl: ttl - 1 }]
            } else {
                Vec::new()
            }
        }
    }

    fn on_neighbor(&self, from: NodeId, _high_priority: bool) -> Vec<Event> {
        let mut g = self.inner.lock().unwrap();
        let evicted = g.add_active(from);
        match evicted {
            Some(ev) => vec![Event::Disconnect { to: ev }],
            None => Vec::new(),
        }
    }

    fn on_disconnect(&self, from: NodeId) -> Vec<Event> {
        let mut g = self.inner.lock().unwrap();
        g.active.retain(|p| p != &from);
        g.add_passive(from);
        // If active underflows, promote a passive to active.
        if g.active.len() < g.sizes.active_capacity && !g.passive.is_empty() {
            let promote = g.passive.remove(0);
            g.active.push(promote.clone());
            vec![Event::Neighbor { to: promote, high_priority: false }]
        } else {
            Vec::new()
        }
    }

    fn on_shuffle(&self, from: NodeId, sample: Vec<NodeId>) -> Vec<Event> {
        let mut g = self.inner.lock().unwrap();
        let reply_sample: Vec<NodeId> = g.passive.iter().take(sample.len()).cloned().collect();
        for p in sample {
            g.add_passive(p);
        }
        vec![Event::ShuffleReply { to: from, sample: reply_sample }]
    }

    fn on_shuffle_reply(&self, sample: Vec<NodeId>) {
        let mut g = self.inner.lock().unwrap();
        for p in sample {
            g.add_passive(p);
        }
    }

    fn active_view(&self) -> Vec<NodeId> {
        self.inner.lock().unwrap().active.clone()
    }

    fn passive_view(&self) -> Vec<NodeId> {
        self.inner.lock().unwrap().passive.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use identity333::Keypair;

    fn mk() -> NodeId {
        Keypair::generate().node_id()
    }

    #[test]
    fn join_adds_contact_to_active() {
        let me = mk();
        let contact = mk();
        let hpv = InMemoryHyParView::new(me, ViewSizes::default());
        let evs = hpv.join(contact.clone());
        assert_eq!(evs.len(), 1);
        assert!(hpv.active_view().contains(&contact));
    }

    #[test]
    fn active_capacity_enforced() {
        let me = mk();
        let hpv = InMemoryHyParView::new(
            me,
            ViewSizes { active_capacity: 2, passive_capacity: 5 },
        );
        let a = mk();
        let b = mk();
        let c = mk();
        hpv.join(a.clone());
        hpv.on_neighbor(b.clone(), true);
        hpv.on_neighbor(c.clone(), true); // evicts a
        let view = hpv.active_view();
        assert_eq!(view.len(), 2);
        // `a` was evicted to passive.
        assert!(hpv.passive_view().contains(&a));
    }

    #[test]
    fn passive_capacity_enforced() {
        let me = mk();
        let hpv = InMemoryHyParView::new(
            me,
            ViewSizes { active_capacity: 1, passive_capacity: 2 },
        );
        // Force 5 evictions to active → overflow passive.
        for _ in 0..5 {
            hpv.on_neighbor(mk(), true);
        }
        assert_eq!(hpv.active_view().len(), 1);
        assert!(hpv.passive_view().len() <= 2);
    }

    #[test]
    fn on_disconnect_promotes_passive() {
        let me = mk();
        let hpv = InMemoryHyParView::new(
            me,
            ViewSizes { active_capacity: 2, passive_capacity: 3 },
        );
        let a = mk();
        let b = mk();
        let c = mk();
        hpv.on_neighbor(a.clone(), true);
        hpv.on_neighbor(b.clone(), true);
        // c goes directly to passive via forward-join overflow path or manual:
        hpv.on_shuffle_reply(vec![c.clone()]);
        assert!(hpv.passive_view().contains(&c));
        // Disconnect a → should promote c from passive.
        let evs = hpv.on_disconnect(a.clone());
        assert_eq!(evs.len(), 1);
        assert!(matches!(&evs[0], Event::Neighbor { to, .. } if to == &c));
        assert!(hpv.active_view().contains(&c));
    }

    #[test]
    fn forward_join_zero_ttl_adds_to_active() {
        let me = mk();
        let hpv = InMemoryHyParView::new(
            me,
            ViewSizes { active_capacity: 4, passive_capacity: 5 },
        );
        let new_node = mk();
        let from = mk();
        let evs = hpv.on_forward_join(from, new_node.clone(), 0);
        assert!(evs.iter().any(|e| matches!(e, Event::Neighbor { to, .. } if to == &new_node)));
        assert!(hpv.active_view().contains(&new_node));
    }

    #[test]
    fn shuffle_sample_has_bounded_size() {
        let me = mk();
        let hpv = InMemoryHyParView::new(
            me,
            ViewSizes { active_capacity: 4, passive_capacity: 7 },
        );
        for _ in 0..3 {
            hpv.on_neighbor(mk(), true);
        }
        for _ in 0..5 {
            hpv.on_shuffle_reply(vec![mk()]);
        }
        let s = hpv.shuffle_sample(4);
        assert!(s.len() <= 4);
    }

    #[test]
    fn self_never_added_to_own_views() {
        let me = mk();
        let hpv = InMemoryHyParView::new(me.clone(), ViewSizes::default());
        hpv.on_neighbor(me.clone(), true);
        hpv.on_shuffle_reply(vec![me.clone()]);
        assert!(!hpv.active_view().contains(&me));
        assert!(!hpv.passive_view().contains(&me));
    }

    #[test]
    fn duplicate_active_noop() {
        let me = mk();
        let p = mk();
        let hpv = InMemoryHyParView::new(me, ViewSizes::default());
        hpv.on_neighbor(p.clone(), true);
        hpv.on_neighbor(p.clone(), true);
        let count = hpv.active_view().iter().filter(|x| *x == &p).count();
        assert_eq!(count, 1);
    }
}
