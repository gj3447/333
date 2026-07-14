// KG: seed-rts-conflict-loro-vs-custom-crdt-2026-04-15
//! Inlined LWW-Map — mirrors src/lww_map.rs.
//! Needed because triple-three has unconditional wasm-bindgen deps.
//! Logic changes MUST be mirrored to src/lww_map.rs.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::Hash;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lamport { pub counter: u64, pub node_id: u32 }

impl Lamport {
    pub fn new(node_id: u32) -> Self { Self { counter: 0, node_id } }
    pub fn tick(&mut self) -> Self { self.counter += 1; *self }
    pub fn recv(&mut self, r: &Lamport) {
        if r.counter >= self.counter { self.counter = r.counter + 1; }
    }
}
impl PartialOrd for Lamport {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(o)) }
}
impl Ord for Lamport {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.counter.cmp(&o.counter).then_with(|| self.node_id.cmp(&o.node_id))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LwwEntry<V> { pub value: Option<V>, pub timestamp: Lamport }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LwwDelta<K: Eq + Hash, V> { pub changes: Vec<(K, LwwEntry<V>)> }

#[derive(Debug, Clone)]
pub struct LwwMap<K: Eq + Hash, V> { entries: HashMap<K, LwwEntry<V>>, clock: Lamport }

impl<K: Eq + Hash + Clone, V: Clone> LwwMap<K, V> {
    pub fn new(node_id: u32) -> Self {
        Self { entries: HashMap::new(), clock: Lamport::new(node_id) }
    }
    pub fn set(&mut self, key: K, value: V) -> LwwDelta<K, V> {
        let ts = self.clock.tick();
        let entry = LwwEntry { value: Some(value), timestamp: ts };
        self.entries.insert(key.clone(), entry.clone());
        LwwDelta { changes: vec![(key, entry)] }
    }
    pub fn get(&self, key: &K) -> Option<&V> {
        self.entries.get(key).and_then(|e| e.value.as_ref())
    }
    pub fn merge_delta(&mut self, delta: &LwwDelta<K, V>) {
        for (key, remote) in &delta.changes {
            self.clock.recv(&remote.timestamp);
            match self.entries.get(key) {
                Some(l) if l.timestamp > remote.timestamp => {}
                Some(l) if l.timestamp == remote.timestamp
                    && l.timestamp.node_id >= remote.timestamp.node_id => {}
                _ => { self.entries.insert(key.clone(), remote.clone()); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn convergence_3_peers() {
        let mut p1: LwwMap<String, String> = LwwMap::new(1);
        let mut p2: LwwMap<String, String> = LwwMap::new(2);
        let mut p3: LwwMap<String, String> = LwwMap::new(3);
        let d1 = p1.set("x".into(), "a".into());
        let d2 = p2.set("x".into(), "b".into());
        let d3 = p3.set("x".into(), "c".into());
        p1.merge_delta(&d2); p1.merge_delta(&d3);
        p2.merge_delta(&d1); p2.merge_delta(&d3);
        p3.merge_delta(&d1); p3.merge_delta(&d2);
        assert_eq!(p1.get(&"x".to_string()), p2.get(&"x".to_string()));
        assert_eq!(p2.get(&"x".to_string()), p3.get(&"x".to_string()));
    }
    #[test]
    fn tiebreak_deterministic() {
        let mut p1: LwwMap<String, i32> = LwwMap::new(1);
        let mut p2: LwwMap<String, i32> = LwwMap::new(2);
        p1.clock.counter = 5; p2.clock.counter = 5;
        let d1 = p1.set("k".into(), 10);
        let d2 = p2.set("k".into(), 20);
        let mut r1 = p1.clone(); r1.merge_delta(&d2);
        let mut r2 = p2.clone(); r2.merge_delta(&d1);
        assert_eq!(r1.get(&"k".to_string()), r2.get(&"k".to_string()));
    }
}
