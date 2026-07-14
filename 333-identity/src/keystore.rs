// KG: TASK_ATOM_L0_Identity_KeyStore, CONTRACT_ATOM_L0_Identity_KeyStore
// Pluggable keypair persistence. Kubo's KeyAPI → trait + in-memory impl.

use std::collections::BTreeMap;
use std::sync::RwLock;

use crate::keypair::Keypair;
use crate::node_id::NodeId;

#[derive(Debug, thiserror::Error)]
pub enum KeyStoreError {
    #[error("key not found: {0}")]
    NotFound(String),
    #[error("key name already exists: {0}")]
    AlreadyExists(String),
    #[error("storage backend failure: {0}")]
    Backend(String),
}

pub trait KeyStore: Send + Sync {
    fn put(&self, name: &str, key: Keypair) -> Result<(), KeyStoreError>;
    fn get(&self, name: &str) -> Result<Keypair, KeyStoreError>;
    fn remove(&self, name: &str) -> Result<Keypair, KeyStoreError>;
    fn list(&self) -> Result<Vec<(String, NodeId)>, KeyStoreError>;
    fn contains(&self, name: &str) -> bool {
        self.get(name).is_ok()
    }
}

pub struct InMemoryKeyStore {
    inner: RwLock<BTreeMap<String, Keypair>>,
}

impl Default for InMemoryKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryKeyStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(BTreeMap::new()),
        }
    }
}

impl KeyStore for InMemoryKeyStore {
    fn put(&self, name: &str, key: Keypair) -> Result<(), KeyStoreError> {
        let mut g = self.inner.write().map_err(|e| KeyStoreError::Backend(e.to_string()))?;
        if g.contains_key(name) {
            return Err(KeyStoreError::AlreadyExists(name.to_string()));
        }
        g.insert(name.to_string(), key);
        Ok(())
    }

    fn get(&self, name: &str) -> Result<Keypair, KeyStoreError> {
        let g = self.inner.read().map_err(|e| KeyStoreError::Backend(e.to_string()))?;
        g.get(name)
            .cloned()
            .ok_or_else(|| KeyStoreError::NotFound(name.to_string()))
    }

    fn remove(&self, name: &str) -> Result<Keypair, KeyStoreError> {
        let mut g = self.inner.write().map_err(|e| KeyStoreError::Backend(e.to_string()))?;
        g.remove(name)
            .ok_or_else(|| KeyStoreError::NotFound(name.to_string()))
    }

    fn list(&self) -> Result<Vec<(String, NodeId)>, KeyStoreError> {
        let g = self.inner.read().map_err(|e| KeyStoreError::Backend(e.to_string()))?;
        Ok(g.iter().map(|(k, v)| (k.clone(), v.node_id())).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kp(seed: u8) -> Keypair {
        Keypair::from_seed([seed; 32])
    }

    #[test]
    fn put_get_roundtrip() {
        let ks = InMemoryKeyStore::new();
        let k = kp(1);
        let nid = k.node_id();
        ks.put("alice", k).unwrap();
        let got = ks.get("alice").unwrap();
        assert_eq!(got.node_id(), nid);
    }

    #[test]
    fn put_duplicate_rejected() {
        let ks = InMemoryKeyStore::new();
        ks.put("dup", kp(1)).unwrap();
        assert!(matches!(
            ks.put("dup", kp(2)),
            Err(KeyStoreError::AlreadyExists(_))
        ));
    }

    #[test]
    fn get_missing_errors() {
        let ks = InMemoryKeyStore::new();
        assert!(matches!(ks.get("ghost"), Err(KeyStoreError::NotFound(_))));
    }

    #[test]
    fn list_returns_all() {
        let ks = InMemoryKeyStore::new();
        ks.put("a", kp(1)).unwrap();
        ks.put("b", kp(2)).unwrap();
        let l = ks.list().unwrap();
        assert_eq!(l.len(), 2);
        let names: Vec<_> = l.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"a") && names.contains(&"b"));
    }

    #[test]
    fn remove_then_gone() {
        let ks = InMemoryKeyStore::new();
        ks.put("zap", kp(5)).unwrap();
        assert!(ks.contains("zap"));
        ks.remove("zap").unwrap();
        assert!(!ks.contains("zap"));
    }

    #[test]
    fn remove_missing_errors() {
        let ks = InMemoryKeyStore::new();
        assert!(matches!(ks.remove("nope"), Err(KeyStoreError::NotFound(_))));
    }

    #[test]
    fn contains_reflects_state() {
        let ks = InMemoryKeyStore::new();
        assert!(!ks.contains("x"));
        ks.put("x", kp(9)).unwrap();
        assert!(ks.contains("x"));
    }
}
