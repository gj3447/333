// KG: SPAN_333_L1_Transport
// Transport trait + in-memory mesh.
//
// Real libp2p integration is out of scope — this crate provides a synchronous
// trait so higher layers (kademlia RPC, signed_routing gossip) can be written
// against a stable interface and unit-tested against an in-process mesh.
// A real backend (tokio TCP / libp2p / QUIC) only needs to implement `Transport`.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use identity333::NodeId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("unreachable: {0}")]
    Unreachable(NodeId),
    #[error("network down")]
    Down,
    #[error("backend: {0}")]
    Backend(String),
}

pub trait Transport: Send + Sync {
    /// Send `payload` to `dest`. Synchronous for simplicity; real backends queue.
    fn send(&self, dest: NodeId, payload: Vec<u8>) -> Result<(), TransportError>;

    /// Drain any payloads addressed to `me`.
    fn recv(&self, me: NodeId) -> Result<Vec<(NodeId, Vec<u8>)>, TransportError>;
}

/// Deterministic in-process mesh. Every participating NodeId shares a single
/// `Arc<InMemoryMesh>`. `send` delivers immediately into the recipient's
/// mailbox; `recv` drains it.
#[derive(Default)]
pub struct InMemoryMesh {
    inner: Mutex<InMemoryState>,
}

#[derive(Default)]
struct InMemoryState {
    /// recipient -> queue of (sender, payload)
    mailboxes: HashMap<NodeId, Vec<(NodeId, Vec<u8>)>>,
    down: bool,
}

impl InMemoryMesh {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register a participant so `send` can route to it.
    pub fn join(&self, node: NodeId) {
        let mut g = self.inner.lock().unwrap();
        g.mailboxes.entry(node).or_default();
    }

    pub fn participants(&self) -> Vec<NodeId> {
        self.inner.lock().unwrap().mailboxes.keys().copied().collect()
    }

    /// Fault injection: mark entire mesh as down.
    pub fn set_down(&self, down: bool) {
        self.inner.lock().unwrap().down = down;
    }

    /// Each call consumes `(sender, payload)` for a specific sender NodeId.
    /// Use via `MeshHandle` in real code — exposed for mesh administration.
    fn send_from(&self, sender: NodeId, dest: NodeId, payload: Vec<u8>) -> Result<(), TransportError> {
        let mut g = self.inner.lock().unwrap();
        if g.down {
            return Err(TransportError::Down);
        }
        let mailbox = g
            .mailboxes
            .get_mut(&dest)
            .ok_or(TransportError::Unreachable(dest))?;
        mailbox.push((sender, payload));
        Ok(())
    }

    fn drain_for(&self, me: NodeId) -> Result<Vec<(NodeId, Vec<u8>)>, TransportError> {
        let mut g = self.inner.lock().unwrap();
        if g.down {
            return Err(TransportError::Down);
        }
        let mb = g
            .mailboxes
            .get_mut(&me)
            .ok_or(TransportError::Unreachable(me))?;
        Ok(std::mem::take(mb))
    }
}

/// Per-node handle on a shared mesh. Binds the sender's NodeId so the
/// `Transport` trait doesn't need to carry identity at each call.
pub struct MeshHandle {
    me: NodeId,
    mesh: Arc<InMemoryMesh>,
}

impl MeshHandle {
    pub fn new(me: NodeId, mesh: Arc<InMemoryMesh>) -> Self {
        mesh.join(me);
        Self { me, mesh }
    }

    pub fn node_id(&self) -> NodeId {
        self.me
    }
}

impl Transport for MeshHandle {
    fn send(&self, dest: NodeId, payload: Vec<u8>) -> Result<(), TransportError> {
        self.mesh.send_from(self.me, dest, payload)
    }

    fn recv(&self, me: NodeId) -> Result<Vec<(NodeId, Vec<u8>)>, TransportError> {
        // Enforce that recv is for this handle's owner only.
        if me != self.me {
            return Err(TransportError::Backend(format!(
                "recv id mismatch: handle={}, asked={}",
                self.me, me
            )));
        }
        self.mesh.drain_for(me)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use identity333::Keypair;

    fn nid(seed: u8) -> NodeId {
        Keypair::from_seed([seed; 32]).node_id()
    }

    #[test]
    fn send_recv_roundtrip() {
        let mesh = InMemoryMesh::new();
        let alice = MeshHandle::new(nid(1), mesh.clone());
        let bob = MeshHandle::new(nid(2), mesh.clone());

        alice.send(bob.node_id(), b"hello bob".to_vec()).unwrap();
        let msgs = bob.recv(bob.node_id()).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, alice.node_id());
        assert_eq!(msgs[0].1, b"hello bob");
    }

    #[test]
    fn unknown_destination_errors() {
        let mesh = InMemoryMesh::new();
        let alice = MeshHandle::new(nid(1), mesh.clone());
        let result = alice.send(nid(99), b"ghost".to_vec());
        assert!(matches!(result, Err(TransportError::Unreachable(_))));
    }

    #[test]
    fn recv_only_own_mailbox() {
        let mesh = InMemoryMesh::new();
        let alice = MeshHandle::new(nid(1), mesh.clone());
        let bob = MeshHandle::new(nid(2), mesh.clone());

        alice.send(bob.node_id(), b"x".to_vec()).unwrap();
        let err = alice.recv(bob.node_id()).unwrap_err();
        assert!(matches!(err, TransportError::Backend(_)));
    }

    #[test]
    fn fault_injection_down() {
        let mesh = InMemoryMesh::new();
        let alice = MeshHandle::new(nid(1), mesh.clone());
        let bob = MeshHandle::new(nid(2), mesh.clone());

        mesh.set_down(true);
        assert!(matches!(
            alice.send(bob.node_id(), b"p".to_vec()),
            Err(TransportError::Down)
        ));
        mesh.set_down(false);
        alice.send(bob.node_id(), b"p".to_vec()).unwrap();
    }

    #[test]
    fn drain_is_one_shot() {
        let mesh = InMemoryMesh::new();
        let alice = MeshHandle::new(nid(1), mesh.clone());
        let bob = MeshHandle::new(nid(2), mesh.clone());

        alice.send(bob.node_id(), b"m".to_vec()).unwrap();
        let first = bob.recv(bob.node_id()).unwrap();
        assert_eq!(first.len(), 1);
        let second = bob.recv(bob.node_id()).unwrap();
        assert!(second.is_empty());
    }
}
