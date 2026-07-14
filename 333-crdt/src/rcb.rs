// KG: SPAN_333_L5_CRDT_Extensions, finding_333_synth_crd_sota_d8, finding_333_synth_crd_prt_d10
// Reliable Causal Broadcast — deliver messages only when all causal prerequisites
// have been received.
//
// Sources:
//   - D8 SOTA: Kleppmann framework for causal broadcast in P2P.
//   - D10 Port-333: CausalBroadcast middleware trait.
//
// Protocol (simplified):
//   1. Sender tags each message with its VersionVector AFTER incrementing
//      its own counter.
//   2. Receiver delivers a message only when its VersionVector dominates
//      (sender counter - 1) at the sender's replica AND the receiver's
//      clocks are ≥ the message's other-replica clocks.
//   3. Early messages sit in a pending buffer until predecessors arrive.

use std::collections::VecDeque;

use crate::dvv::{ReplicaId, VersionVector};

#[derive(Debug, Clone)]
pub struct CausalMsg<T: Clone> {
    pub sender: ReplicaId,
    pub sender_counter: u64,
    pub context: VersionVector,
    pub payload: T,
}

/// Trait: application-facing causal deliverer.
pub trait CausalBroadcast<T: Clone> {
    /// Submit a local message to broadcast. Returns the tagged message.
    fn submit(&mut self, payload: T) -> CausalMsg<T>;
    /// Feed a received message. Returns messages now ready for delivery.
    fn receive(&mut self, msg: CausalMsg<T>) -> Vec<CausalMsg<T>>;
    /// Number of messages still waiting for causal predecessors.
    fn pending(&self) -> usize;
}

#[derive(Debug)]
pub struct VectorClockBroadcaster<T: Clone> {
    me: ReplicaId,
    delivered: VersionVector,
    pending: VecDeque<CausalMsg<T>>,
}

impl<T: Clone> VectorClockBroadcaster<T> {
    pub fn new(me: ReplicaId) -> Self {
        Self { me, delivered: VersionVector::new(), pending: VecDeque::new() }
    }

    pub fn delivered(&self) -> &VersionVector {
        &self.delivered
    }

    fn can_deliver(&self, msg: &CausalMsg<T>) -> bool {
        // Need: delivered[sender] == sender_counter - 1, and for every other
        // replica r in msg.context, delivered[r] >= msg.context[r].
        if self.delivered.get(&msg.sender) + 1 != msg.sender_counter {
            return false;
        }
        // Dominate-check over the message's context (excluding sender).
        let mut ctx = msg.context.clone();
        // Remove the sender's own counter from the check; we already verified it.
        // Simplest: iterate over all and skip the sender.
        // VersionVector API doesn't expose iter, so we use dominates on a
        // context with sender's clock set to our delivered[sender] (which, by
        // the first check, equals sender_counter-1, i.e. msg.context[sender]-1;
        // but we don't need to prove that — just check that every non-sender
        // clock in ctx is <= delivered).
        ctx.tick(&msg.sender); // push sender clock up one (safe — we've verified that dimension)
        // ctx is "delivered can cover everything except sender's latest write".
        // To keep the invariant simple, replace sender's context entry with
        // what we will have after delivery.
        let _ = ctx;
        // Practical check: every non-sender clock in msg.context must be ≤ self.delivered.
        // We don't have a ctx iterator, so we conservatively enforce via the
        // dominates API: a VV with all sender entries zeroed out.
        let mut stripped = msg.context.clone();
        // No direct zero-out API; simulate by checking: for the caller's "other
        // replicas", the global dominates check works when we also count our
        // own delivered[sender] = sender_counter (post-delivery). Let's do it
        // that way.
        let mut hypothetical = self.delivered.clone();
        // Post-delivery, delivered[sender] will equal sender_counter.
        // Write that into hypothetical (we need a way; only `tick` exists,
        // which is stateful increment). Use tick from current delivered[sender]
        // up to sender_counter.
        while hypothetical.get(&msg.sender) < msg.sender_counter {
            hypothetical.tick(&msg.sender);
        }
        hypothetical.dominates(&stripped) || {
            // Strip sender entry out by ticking stripped[sender] back down
            // isn't possible; rely on dominates semantics.
            stripped.tick(&msg.sender); // bump to match, harmless
            hypothetical.dominates(&stripped)
        }
    }

    fn deliver_now(&mut self, msg: &CausalMsg<T>) {
        // Advance delivered[sender] to sender_counter.
        while self.delivered.get(&msg.sender) < msg.sender_counter {
            self.delivered.tick(&msg.sender);
        }
    }
}

impl<T: Clone> CausalBroadcast<T> for VectorClockBroadcaster<T> {
    fn submit(&mut self, payload: T) -> CausalMsg<T> {
        let counter = self.delivered.tick(&self.me);
        CausalMsg {
            sender: self.me.clone(),
            sender_counter: counter,
            context: self.delivered.clone(),
            payload,
        }
    }

    fn receive(&mut self, msg: CausalMsg<T>) -> Vec<CausalMsg<T>> {
        self.pending.push_back(msg);
        let mut delivered_now = Vec::new();
        // Re-scan pending repeatedly until no more can fire (causal chains).
        loop {
            let before = self.pending.len();
            let mut remaining = VecDeque::new();
            while let Some(m) = self.pending.pop_front() {
                if self.can_deliver(&m) {
                    self.deliver_now(&m);
                    delivered_now.push(m);
                } else {
                    remaining.push_back(m);
                }
            }
            self.pending = remaining;
            if self.pending.len() == before {
                break;
            }
        }
        delivered_now
    }

    fn pending(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_order_delivery_trivial() {
        let mut a: VectorClockBroadcaster<u32> = VectorClockBroadcaster::new("A".into());
        let m = a.submit(42);
        // Sender sees its own message as already delivered via submit side;
        // receiver side runs on a different replica.
        let mut b: VectorClockBroadcaster<u32> = VectorClockBroadcaster::new("B".into());
        let out = b.receive(m);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].payload, 42);
    }

    #[test]
    fn out_of_order_buffered_then_delivered() {
        let mut a: VectorClockBroadcaster<u32> = VectorClockBroadcaster::new("A".into());
        let m1 = a.submit(1);
        let m2 = a.submit(2);
        let m3 = a.submit(3);

        let mut b: VectorClockBroadcaster<u32> = VectorClockBroadcaster::new("B".into());
        // Arrive out of order: 3, 1, 2
        let out1 = b.receive(m3.clone());
        assert!(out1.is_empty());
        assert_eq!(b.pending(), 1);

        let out2 = b.receive(m1.clone());
        assert_eq!(out2.len(), 1);
        assert_eq!(out2[0].payload, 1);

        let out3 = b.receive(m2.clone());
        // m2 delivers, then m3 becomes deliverable.
        assert_eq!(out3.len(), 2);
        assert_eq!(out3[0].payload, 2);
        assert_eq!(out3[1].payload, 3);
        assert_eq!(b.pending(), 0);
    }

    #[test]
    fn pending_stays_until_predecessor_arrives() {
        let mut a: VectorClockBroadcaster<&'static str> =
            VectorClockBroadcaster::new("A".into());
        let _m1 = a.submit("x");
        let m2 = a.submit("y");

        let mut b: VectorClockBroadcaster<&'static str> =
            VectorClockBroadcaster::new("B".into());
        let out = b.receive(m2);
        assert!(out.is_empty());
        assert_eq!(b.pending(), 1);
    }

    #[test]
    fn submit_monotonic_counter() {
        let mut a: VectorClockBroadcaster<u32> = VectorClockBroadcaster::new("A".into());
        let m1 = a.submit(1);
        let m2 = a.submit(2);
        assert_eq!(m1.sender_counter, 1);
        assert_eq!(m2.sender_counter, 2);
    }
}
