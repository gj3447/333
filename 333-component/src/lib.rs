// KG: SPAN_333_L13_Component, plan-333-p2p-os-synthesis-execution-2026-04-18
// 333 P2P OS L13 Component Model — WIT interface adapter + ComponentRegistry.
//
// Sources (prom32-333-p2p-2026-04-18):
//   - D16 SOTA: Component Model 0.3 RC, WASI Preview 2 stable, wasmtime 43+.
//   - D17 Theory: generative nominal resource types + linear borrow/owned +
//     Store-per-instance isolation (lift/lower membrane).
//   - D18 Port-333: identity.wit first (narrow surface), keep CRDT internal,
//     adapter in 333-platform (wasmtime::component runtime, separate from
//     wasm.rs JS bindings).
//   - D19 Pitfalls: CVE-2026-34943 flags lift, version skew, WASIp3 RC
//     unstable → pin WASI 0.2 for v1.0.
//
// This crate does NOT link wasmtime (heavy dep). It ships:
//   - the WIT source file (wit/identity.wit) as the IDL contract,
//   - Rust mirror types that mimic the WIT shape,
//   - `ComponentRegistry` trait + `InMemoryRegistry` reference impl that
//     models component lifecycle (load/instantiate/call) without actual
//     WASM execution.
// The real wasmtime::component binding lives in a downstream `component333-host`
// crate once WASI 0.2 dep budget is approved.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use identity333::NodeId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ComponentError {
    #[error("unknown component id: {0}")]
    UnknownComponent(String),
    #[error("unknown instance id: {0}")]
    UnknownInstance(String),
    #[error("unknown export: {0}")]
    UnknownExport(String),
    #[error("type mismatch on WIT boundary: expected {expected}, got {got}")]
    TypeMismatch { expected: &'static str, got: &'static str },
    #[error("canonical ABI error: {0}")]
    Abi(String),
}

// ============================================================================
// WIT-mirrored types (matches wit/identity.wit exactly)
// ============================================================================

pub type WitNodeId = Vec<u8>;
pub type WitSignature = Vec<u8>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyedEvent {
    pub author: WitNodeId,
    pub payload: Vec<u8>,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub event: KeyedEvent,
    pub sig: WitSignature,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRoot {
    pub root_hash: Vec<u8>,
    pub author: WitNodeId,
    pub sig: WitSignature,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    InvalidPublicKey,
    InvalidSignature,
    VerificationFailed,
}

// Canonical ABI: Rust NodeId ↔ WIT node-id (list<u8>).
pub fn node_id_to_wit(id: &NodeId) -> WitNodeId {
    id.as_bytes().to_vec()
}

pub fn node_id_from_wit(wit: &WitNodeId) -> Result<NodeId, ComponentError> {
    if wit.len() != 32 {
        return Err(ComponentError::TypeMismatch {
            expected: "node-id:list<u8>[32]",
            got: "list<u8>[other]",
        });
    }
    let mut buf = [0u8; 32];
    buf.copy_from_slice(wit);
    Ok(NodeId::from_bytes(buf))
}

// ============================================================================
// Component metadata + registry
// ============================================================================

pub type ComponentId = String;
pub type InstanceId = String;

#[derive(Debug, Clone)]
pub struct ComponentManifest {
    pub id: ComponentId,
    pub name: String,
    pub wit_world: String,
    pub version: String,
    pub exports: Vec<String>,
    pub imports: Vec<String>,
    /// Blake3/SHA-256 of the compiled .wasm; 32 bytes.
    pub binary_hash: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceStatus {
    Created,
    Running,
    Dropped,
}

/// Component lifecycle trait. Narrow surface — mirrors what wasmtime::component
/// exposes, sans the actual WASM exec.
pub trait ComponentRegistry {
    fn register(&self, manifest: ComponentManifest) -> Result<(), ComponentError>;
    fn unregister(&self, id: &ComponentId) -> Result<(), ComponentError>;
    fn list(&self) -> Vec<ComponentId>;

    fn instantiate(&self, id: &ComponentId) -> Result<InstanceId, ComponentError>;
    fn drop_instance(&self, instance: &InstanceId) -> Result<(), ComponentError>;
    fn instance_status(&self, instance: &InstanceId) -> Result<InstanceStatus, ComponentError>;

    /// Invoke an export on an instance. Returns canonical-ABI-encoded bytes.
    /// Real backend performs lift/lower; this reference impl just echoes input.
    fn call(
        &self,
        instance: &InstanceId,
        export: &str,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, ComponentError>;
}

#[derive(Debug)]
struct RegEntry {
    manifest: ComponentManifest,
}

#[derive(Debug)]
struct InstEntry {
    component_id: ComponentId,
    status: InstanceStatus,
}

#[derive(Debug, Default)]
struct Inner {
    components: HashMap<ComponentId, RegEntry>,
    instances: HashMap<InstanceId, InstEntry>,
    counter: u64,
}

#[derive(Debug, Default)]
pub struct InMemoryRegistry {
    inner: Arc<Mutex<Inner>>,
}

impl InMemoryRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ComponentRegistry for InMemoryRegistry {
    fn register(&self, manifest: ComponentManifest) -> Result<(), ComponentError> {
        let mut g = self.inner.lock().unwrap();
        g.components.insert(manifest.id.clone(), RegEntry { manifest });
        Ok(())
    }

    fn unregister(&self, id: &ComponentId) -> Result<(), ComponentError> {
        let mut g = self.inner.lock().unwrap();
        g.components
            .remove(id)
            .map(|_| ())
            .ok_or_else(|| ComponentError::UnknownComponent(id.clone()))
    }

    fn list(&self) -> Vec<ComponentId> {
        self.inner.lock().unwrap().components.keys().cloned().collect()
    }

    fn instantiate(&self, id: &ComponentId) -> Result<InstanceId, ComponentError> {
        let mut g = self.inner.lock().unwrap();
        if !g.components.contains_key(id) {
            return Err(ComponentError::UnknownComponent(id.clone()));
        }
        g.counter += 1;
        let inst_id = format!("inst-{}-{}", id, g.counter);
        g.instances.insert(
            inst_id.clone(),
            InstEntry { component_id: id.clone(), status: InstanceStatus::Running },
        );
        Ok(inst_id)
    }

    fn drop_instance(&self, instance: &InstanceId) -> Result<(), ComponentError> {
        let mut g = self.inner.lock().unwrap();
        let e = g
            .instances
            .get_mut(instance)
            .ok_or_else(|| ComponentError::UnknownInstance(instance.clone()))?;
        e.status = InstanceStatus::Dropped;
        Ok(())
    }

    fn instance_status(&self, instance: &InstanceId) -> Result<InstanceStatus, ComponentError> {
        self.inner
            .lock()
            .unwrap()
            .instances
            .get(instance)
            .map(|e| e.status)
            .ok_or_else(|| ComponentError::UnknownInstance(instance.clone()))
    }

    fn call(
        &self,
        instance: &InstanceId,
        export: &str,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, ComponentError> {
        let g = self.inner.lock().unwrap();
        let inst = g
            .instances
            .get(instance)
            .ok_or_else(|| ComponentError::UnknownInstance(instance.clone()))?;
        if inst.status != InstanceStatus::Running {
            return Err(ComponentError::Abi("instance not running".into()));
        }
        let comp = g
            .components
            .get(&inst.component_id)
            .ok_or_else(|| ComponentError::UnknownComponent(inst.component_id.clone()))?;
        if !comp.manifest.exports.iter().any(|e| e == export) {
            return Err(ComponentError::UnknownExport(export.into()));
        }
        // Reference impl: echo args (real backend: canonical ABI lift/lower).
        Ok(args)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use identity333::Keypair;

    fn manifest(id: &str, exports: &[&str]) -> ComponentManifest {
        ComponentManifest {
            id: id.into(),
            name: format!("{id}-name"),
            wit_world: "identity-component".into(),
            version: "0.1.0".into(),
            exports: exports.iter().map(|s| s.to_string()).collect(),
            imports: vec![],
            binary_hash: [0u8; 32],
        }
    }

    #[test]
    fn node_id_canonical_abi_roundtrip() {
        let kp = Keypair::generate();
        let id = kp.node_id();
        let wit = node_id_to_wit(&id);
        assert_eq!(wit.len(), 32);
        let back = node_id_from_wit(&wit).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn node_id_from_wit_rejects_wrong_length() {
        let bad = vec![0u8; 16];
        assert!(matches!(
            node_id_from_wit(&bad),
            Err(ComponentError::TypeMismatch { .. })
        ));
    }

    #[test]
    fn register_and_list() {
        let r = InMemoryRegistry::new();
        r.register(manifest("id-a", &["sign", "verify"])).unwrap();
        r.register(manifest("id-b", &["verify"])).unwrap();
        let mut list = r.list();
        list.sort();
        assert_eq!(list, vec!["id-a".to_string(), "id-b".to_string()]);
    }

    #[test]
    fn instantiate_and_drop_lifecycle() {
        let r = InMemoryRegistry::new();
        r.register(manifest("id", &["sign"])).unwrap();
        let inst = r.instantiate(&"id".into()).unwrap();
        assert_eq!(r.instance_status(&inst).unwrap(), InstanceStatus::Running);
        r.drop_instance(&inst).unwrap();
        assert_eq!(r.instance_status(&inst).unwrap(), InstanceStatus::Dropped);
    }

    #[test]
    fn instantiate_unknown_errors() {
        let r = InMemoryRegistry::new();
        let err = r.instantiate(&"nope".into()).unwrap_err();
        assert!(matches!(err, ComponentError::UnknownComponent(_)));
    }

    #[test]
    fn call_unknown_export_errors() {
        let r = InMemoryRegistry::new();
        r.register(manifest("id", &["sign"])).unwrap();
        let inst = r.instantiate(&"id".into()).unwrap();
        let err = r.call(&inst, "not-exported", vec![1, 2]).unwrap_err();
        assert!(matches!(err, ComponentError::UnknownExport(_)));
    }

    #[test]
    fn call_dropped_instance_errors() {
        let r = InMemoryRegistry::new();
        r.register(manifest("id", &["sign"])).unwrap();
        let inst = r.instantiate(&"id".into()).unwrap();
        r.drop_instance(&inst).unwrap();
        let err = r.call(&inst, "sign", vec![]).unwrap_err();
        assert!(matches!(err, ComponentError::Abi(_)));
    }

    #[test]
    fn call_echoes_in_reference_impl() {
        let r = InMemoryRegistry::new();
        r.register(manifest("id", &["echo"])).unwrap();
        let inst = r.instantiate(&"id".into()).unwrap();
        let out = r.call(&inst, "echo", vec![1, 2, 3]).unwrap();
        assert_eq!(out, vec![1, 2, 3]);
    }

    #[test]
    fn unregister_removes() {
        let r = InMemoryRegistry::new();
        r.register(manifest("id", &["x"])).unwrap();
        r.unregister(&"id".into()).unwrap();
        assert!(r.list().is_empty());
        assert!(matches!(
            r.unregister(&"id".into()),
            Err(ComponentError::UnknownComponent(_))
        ));
    }

    #[test]
    fn envelope_roundtrip_via_wit_types() {
        let kp = Keypair::generate();
        let event = KeyedEvent {
            author: node_id_to_wit(&kp.node_id()),
            payload: b"hello".to_vec(),
            timestamp_ms: 1_700_000_000_000,
        };
        let env = Envelope { event: event.clone(), sig: vec![0u8; 64] };
        assert_eq!(env.event, event);
        assert_eq!(env.sig.len(), 64);
    }

    #[test]
    fn signed_root_structure() {
        let kp = Keypair::generate();
        let sr = SignedRoot {
            root_hash: vec![7u8; 32],
            author: node_id_to_wit(&kp.node_id()),
            sig: vec![0u8; 64],
        };
        assert_eq!(sr.root_hash.len(), 32);
        assert_eq!(sr.author.len(), 32);
    }
}
