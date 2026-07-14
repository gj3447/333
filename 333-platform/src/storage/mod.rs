// KG: CONTRACT_333_IndexedDB, CONTRACT_333_DHT
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-storage-KVStore/MemoryStore/StorageManager
// Storage module — trait-based abstraction
// Hot: IndexedDB (browser), Cold: DHT (P2P Kademlia k=5)
// Test: InMemory HashMap

use std::collections::HashMap;

// IndexedDB impl — browser only
#[cfg(target_arch = "wasm32")]
pub mod idb;

pub mod kademlia;

/// Abstract key-value store trait
pub trait KVStore: Send + Sync {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>>;
    fn put(&mut self, key: &[u8], value: &[u8]) -> bool;
    fn delete(&mut self, key: &[u8]) -> bool;
    fn contains(&self, key: &[u8]) -> bool;
    fn keys(&self) -> Vec<Vec<u8>>;
    fn size(&self) -> usize;
}

/// In-memory store (for testing)
#[derive(Debug, Default)]
pub struct MemoryStore {
    data: HashMap<Vec<u8>, Vec<u8>>,
}

impl MemoryStore {
    pub fn new() -> Self { Self::default() }
}

impl KVStore for MemoryStore {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.data.get(key).cloned()
    }

    fn put(&mut self, key: &[u8], value: &[u8]) -> bool {
        self.data.insert(key.to_vec(), value.to_vec());
        true
    }

    fn delete(&mut self, key: &[u8]) -> bool {
        self.data.remove(key).is_some()
    }

    fn contains(&self, key: &[u8]) -> bool {
        self.data.contains_key(key)
    }

    fn keys(&self) -> Vec<Vec<u8>> {
        self.data.keys().cloned().collect()
    }

    fn size(&self) -> usize {
        self.data.len()
    }
}

/// 3-layer storage manager
pub struct StorageManager {
    hot: Box<dyn KVStore>,
}

impl StorageManager {
    pub fn new(hot: Box<dyn KVStore>) -> Self {
        Self { hot }
    }

    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.hot.get(key)
    }

    pub fn put(&mut self, key: &[u8], value: &[u8]) -> bool {
        self.hot.put(key, value)
    }

    pub fn delete(&mut self, key: &[u8]) -> bool {
        self.hot.delete(key)
    }

    pub fn size(&self) -> usize {
        self.hot.size()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_crud() {
        let mut store = MemoryStore::new();
        assert!(!store.contains(b"key1"));
        store.put(b"key1", b"value1");
        assert!(store.contains(b"key1"));
        assert_eq!(store.get(b"key1"), Some(b"value1".to_vec()));
        store.put(b"key1", b"value2");
        assert_eq!(store.get(b"key1"), Some(b"value2".to_vec()));
        assert!(store.delete(b"key1"));
        assert!(!store.contains(b"key1"));
        assert_eq!(store.size(), 0);
    }

    #[test]
    fn memory_store_keys() {
        let mut store = MemoryStore::new();
        store.put(b"a", b"1");
        store.put(b"b", b"2");
        store.put(b"c", b"3");
        assert_eq!(store.size(), 3);
        assert_eq!(store.keys().len(), 3);
    }

    #[test]
    fn storage_manager_hot_layer() {
        let mut mgr = StorageManager::new(Box::new(MemoryStore::new()));
        mgr.put(b"block:0,0,0", b"stone");
        assert_eq!(mgr.get(b"block:0,0,0"), Some(b"stone".to_vec()));
        assert_eq!(mgr.size(), 1);
    }

    #[test]
    fn delete_nonexistent() {
        let mut store = MemoryStore::new();
        assert!(!store.delete(b"ghost"));
    }
}
