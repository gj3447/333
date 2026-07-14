// KG: SPAN_333_L9_IAM_Prod, finding_333_synth_sec_prt_d30, finding_333_synth_sec_pit_d31
// CRDT tombstone RevocationStore — eventually-consistent capability revocation.
//
// Design: maintain a growing set of revoked token nonces across replicas,
// merged via Or-Set union semantics (LWW-Map with tombstone flag). Any replica
// can revoke; revocations converge via gossip.
//
// D31 pitfall: revocation race (TOCTOU) — token used after broadcast but
// before consensus finality. This store treats revocation as "eventual";
// critical operations should additionally require a finality-proof from
// 333-consensus. For best-effort / low-value operations this is sufficient.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crdt333::Crdt;

pub type Nonce = String;

/// One revocation record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationEntry {
    pub revoked_at_ms: u64,
    pub reason: String,
}

/// CRDT revocation set: nonce → latest revocation record (LWW by timestamp).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RevocationMap {
    entries: BTreeMap<Nonce, RevocationEntry>,
}

impl RevocationMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn revoke(&mut self, nonce: Nonce, revoked_at_ms: u64, reason: String) {
        let entry = RevocationEntry { revoked_at_ms, reason };
        self.entries
            .entry(nonce)
            .and_modify(|e| {
                if revoked_at_ms > e.revoked_at_ms {
                    *e = entry.clone();
                }
            })
            .or_insert(entry);
    }

    pub fn is_revoked(&self, nonce: &Nonce) -> bool {
        self.entries.contains_key(nonce)
    }

    pub fn get(&self, nonce: &Nonce) -> Option<&RevocationEntry> {
        self.entries.get(nonce)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Crdt for RevocationMap {
    fn merge(&mut self, other: &Self) {
        for (k, v) in &other.entries {
            self.entries
                .entry(k.clone())
                .and_modify(|e| {
                    if v.revoked_at_ms > e.revoked_at_ms {
                        *e = v.clone();
                    }
                })
                .or_insert_with(|| v.clone());
        }
    }
}

pub trait RevocationStore {
    fn revoke(&self, nonce: Nonce, revoked_at_ms: u64, reason: String);
    fn is_revoked(&self, nonce: &Nonce) -> bool;
    fn merge_remote(&self, remote: &RevocationMap);
    fn snapshot(&self) -> RevocationMap;
}

#[derive(Debug, Default)]
pub struct InMemoryRevocationStore {
    inner: Arc<Mutex<RevocationMap>>,
}

impl InMemoryRevocationStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl RevocationStore for InMemoryRevocationStore {
    fn revoke(&self, nonce: Nonce, revoked_at_ms: u64, reason: String) {
        let mut g = self.inner.lock().unwrap();
        g.revoke(nonce, revoked_at_ms, reason);
    }

    fn is_revoked(&self, nonce: &Nonce) -> bool {
        self.inner.lock().unwrap().is_revoked(nonce)
    }

    fn merge_remote(&self, remote: &RevocationMap) {
        self.inner.lock().unwrap().merge(remote);
    }

    fn snapshot(&self) -> RevocationMap {
        self.inner.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revoke_and_check() {
        let s = InMemoryRevocationStore::new();
        s.revoke("n1".into(), 100, "compromised".into());
        assert!(s.is_revoked(&"n1".into()));
        assert!(!s.is_revoked(&"n2".into()));
    }

    #[test]
    fn merge_union_semantics() {
        let mut a = RevocationMap::new();
        a.revoke("n1".into(), 100, "r1".into());
        let mut b = RevocationMap::new();
        b.revoke("n2".into(), 200, "r2".into());
        a.merge(&b);
        assert_eq!(a.len(), 2);
        assert!(a.is_revoked(&"n1".into()));
        assert!(a.is_revoked(&"n2".into()));
    }

    #[test]
    fn merge_lww_on_duplicate_nonce() {
        let mut a = RevocationMap::new();
        a.revoke("n1".into(), 100, "reason-A".into());
        let mut b = RevocationMap::new();
        b.revoke("n1".into(), 200, "reason-B".into());
        a.merge(&b);
        assert_eq!(a.get(&"n1".into()).unwrap().reason, "reason-B");
    }

    #[test]
    fn merge_commutative() {
        let mut a = RevocationMap::new();
        a.revoke("x".into(), 100, "a".into());
        let mut b = RevocationMap::new();
        b.revoke("y".into(), 200, "b".into());

        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);
        assert_eq!(ab, ba);
    }

    #[test]
    fn merge_idempotent() {
        let mut a = RevocationMap::new();
        a.revoke("x".into(), 100, "r".into());
        let b = a.clone();
        a.merge(&b);
        a.merge(&b);
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn merge_remote_store() {
        let s = InMemoryRevocationStore::new();
        let mut remote = RevocationMap::new();
        remote.revoke("remote-n".into(), 500, "remote-reason".into());
        s.merge_remote(&remote);
        assert!(s.is_revoked(&"remote-n".into()));
    }

    #[test]
    fn snapshot_isolated_from_future_writes() {
        let s = InMemoryRevocationStore::new();
        s.revoke("a".into(), 100, "x".into());
        let snap = s.snapshot();
        s.revoke("b".into(), 200, "y".into());
        assert_eq!(snap.len(), 1);
        assert!(snap.is_revoked(&"a".into()));
        assert!(!snap.is_revoked(&"b".into()));
    }
}
