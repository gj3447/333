// KG: SPAN_333_L6_Storage, plan-333-three-way-hybrid-2026-04-17
// FilerStore: path → Cid mapping (metadata).
// Pattern = Repository + Decorator (SeaweedFS FilerStore + VirtualFilerStore).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use content333::Cid;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("path not found: {0}")]
    NotFound(String),
    #[error("backend: {0}")]
    Backend(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub cid: Cid,
    pub size: u64,
}

pub trait FilerStore: Send + Sync {
    fn insert(&self, entry: Entry) -> Result<(), StorageError>;
    fn get(&self, path: &str) -> Result<Entry, StorageError>;
    fn delete(&self, path: &str) -> Result<Entry, StorageError>;
    fn list(&self, prefix: &str) -> Result<Vec<Entry>, StorageError>;
}

/// In-memory B-tree filer. Keys are full paths like "/buckets/alpha/x.txt".
pub struct BTreeFilerStore {
    inner: RwLock<BTreeMap<String, Entry>>,
}

impl Default for BTreeFilerStore {
    fn default() -> Self {
        Self { inner: RwLock::new(BTreeMap::new()) }
    }
}

impl BTreeFilerStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl FilerStore for BTreeFilerStore {
    fn insert(&self, entry: Entry) -> Result<(), StorageError> {
        let mut g = self.inner.write().map_err(|e| StorageError::Backend(e.to_string()))?;
        g.insert(entry.path.clone(), entry);
        Ok(())
    }

    fn get(&self, path: &str) -> Result<Entry, StorageError> {
        let g = self.inner.read().map_err(|e| StorageError::Backend(e.to_string()))?;
        g.get(path).cloned().ok_or_else(|| StorageError::NotFound(path.to_string()))
    }

    fn delete(&self, path: &str) -> Result<Entry, StorageError> {
        let mut g = self.inner.write().map_err(|e| StorageError::Backend(e.to_string()))?;
        g.remove(path).ok_or_else(|| StorageError::NotFound(path.to_string()))
    }

    fn list(&self, prefix: &str) -> Result<Vec<Entry>, StorageError> {
        let g = self.inner.read().map_err(|e| StorageError::Backend(e.to_string()))?;
        Ok(g.range(prefix.to_string()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(_, v)| v.clone())
            .collect())
    }
}

/// Counting decorator: transparent wrapper logging op counts. Demonstrates
/// the Decorator pattern (VirtualFilerStore in SeaweedFS).
///
/// Uses `AtomicU64` so read paths (`get`, `list`) do NOT serialize on each other.
pub struct CountingFiler<F: FilerStore> {
    inner: F,
    inserts: AtomicU64,
    gets: AtomicU64,
    deletes: AtomicU64,
    lists: AtomicU64,
}

impl<F: FilerStore> CountingFiler<F> {
    pub fn new(inner: F) -> Self {
        Self {
            inner,
            inserts: AtomicU64::new(0),
            gets: AtomicU64::new(0),
            deletes: AtomicU64::new(0),
            lists: AtomicU64::new(0),
        }
    }
    pub fn counts(&self) -> (u64, u64, u64, u64) {
        (
            self.inserts.load(Ordering::Relaxed),
            self.gets.load(Ordering::Relaxed),
            self.deletes.load(Ordering::Relaxed),
            self.lists.load(Ordering::Relaxed),
        )
    }
}

impl<F: FilerStore> FilerStore for CountingFiler<F> {
    fn insert(&self, entry: Entry) -> Result<(), StorageError> {
        self.inserts.fetch_add(1, Ordering::Relaxed);
        self.inner.insert(entry)
    }
    fn get(&self, path: &str) -> Result<Entry, StorageError> {
        self.gets.fetch_add(1, Ordering::Relaxed);
        self.inner.get(path)
    }
    fn delete(&self, path: &str) -> Result<Entry, StorageError> {
        self.deletes.fetch_add(1, Ordering::Relaxed);
        self.inner.delete(path)
    }
    fn list(&self, prefix: &str) -> Result<Vec<Entry>, StorageError> {
        self.lists.fetch_add(1, Ordering::Relaxed);
        self.inner.list(prefix)
    }
}

/// Trait object ergonomics.
pub type DynFiler = Arc<dyn FilerStore>;

#[cfg(test)]
mod tests {
    use super::*;

    fn e(path: &str) -> Entry {
        Entry { path: path.into(), cid: Cid::of(path.as_bytes()), size: path.len() as u64 }
    }

    #[test]
    fn insert_get_roundtrip() {
        let fs = BTreeFilerStore::new();
        fs.insert(e("/a.txt")).unwrap();
        assert_eq!(fs.get("/a.txt").unwrap().path, "/a.txt");
    }

    #[test]
    fn get_missing_errors() {
        let fs = BTreeFilerStore::new();
        assert!(matches!(fs.get("/nope"), Err(StorageError::NotFound(_))));
    }

    #[test]
    fn delete_removes() {
        let fs = BTreeFilerStore::new();
        fs.insert(e("/x")).unwrap();
        let removed = fs.delete("/x").unwrap();
        assert_eq!(removed.path, "/x");
        assert!(matches!(fs.get("/x"), Err(StorageError::NotFound(_))));
    }

    #[test]
    fn list_by_prefix() {
        let fs = BTreeFilerStore::new();
        fs.insert(e("/bucket/a")).unwrap();
        fs.insert(e("/bucket/b")).unwrap();
        fs.insert(e("/other/c")).unwrap();
        let bucket = fs.list("/bucket/").unwrap();
        assert_eq!(bucket.len(), 2);
        let all = fs.list("/").unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn decorator_counts_ops() {
        let fs = CountingFiler::new(BTreeFilerStore::new());
        fs.insert(e("/a")).unwrap();
        fs.insert(e("/b")).unwrap();
        let _ = fs.get("/a").unwrap();
        let _ = fs.list("/").unwrap();
        let _ = fs.delete("/a").unwrap();
        let (ins, get, del, lst) = fs.counts();
        assert_eq!((ins, get, del, lst), (2, 1, 1, 1));
    }
}
