// KG: SPAN_333_L3_Topology, plan-333-three-way-hybrid-2026-04-17
// Composite topology tree: Zone > Rack > Node > Volume.
// Pattern = Composite (uniform interface over leaf + container), inspired by
// SeaweedFS weed/topology/node.go. Uniform queries: free_space, available, children.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use identity333::NodeId;

pub trait TopoElement {
    fn id(&self) -> &str;
    fn free_space(&self) -> u64;
    fn capacity(&self) -> u64;
    fn available(&self) -> bool;
}

#[derive(Debug, Clone)]
pub struct Volume {
    pub id: String,
    pub size: u64,
    pub used: u64,
    pub down: bool,
}

impl TopoElement for Volume {
    fn id(&self) -> &str {
        &self.id
    }
    fn free_space(&self) -> u64 {
        self.size.saturating_sub(self.used)
    }
    fn capacity(&self) -> u64 {
        self.size
    }
    fn available(&self) -> bool {
        !self.down
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub node_id: NodeId,
    pub volumes: BTreeMap<String, Volume>,
}

impl Node {
    pub fn new(id: impl Into<String>, node_id: NodeId) -> Self {
        Self {
            id: id.into(),
            node_id,
            volumes: BTreeMap::new(),
        }
    }

    pub fn add_volume(&mut self, v: Volume) {
        self.volumes.insert(v.id.clone(), v);
    }
}

impl TopoElement for Node {
    fn id(&self) -> &str {
        &self.id
    }
    fn free_space(&self) -> u64 {
        self.volumes.values().map(|v| v.free_space()).sum()
    }
    fn capacity(&self) -> u64 {
        self.volumes.values().map(|v| v.capacity()).sum()
    }
    fn available(&self) -> bool {
        self.volumes.values().any(|v| v.available())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Rack {
    pub id: String,
    pub nodes: BTreeMap<String, Node>,
}

impl Rack {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into(), nodes: BTreeMap::new() }
    }
    pub fn add_node(&mut self, n: Node) {
        self.nodes.insert(n.id.clone(), n);
    }
}

impl TopoElement for Rack {
    fn id(&self) -> &str {
        &self.id
    }
    fn free_space(&self) -> u64 {
        self.nodes.values().map(|n| n.free_space()).sum()
    }
    fn capacity(&self) -> u64 {
        self.nodes.values().map(|n| n.capacity()).sum()
    }
    fn available(&self) -> bool {
        self.nodes.values().any(|n| n.available())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Zone {
    pub id: String,
    pub racks: BTreeMap<String, Rack>,
}

impl Zone {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into(), racks: BTreeMap::new() }
    }
    pub fn add_rack(&mut self, r: Rack) {
        self.racks.insert(r.id.clone(), r);
    }

    /// Find the node with the most free space (simple placement heuristic).
    pub fn best_fit_node(&self) -> Option<&Node> {
        self.racks
            .values()
            .flat_map(|r| r.nodes.values())
            .max_by_key(|n| n.free_space())
    }
}

impl TopoElement for Zone {
    fn id(&self) -> &str {
        &self.id
    }
    fn free_space(&self) -> u64 {
        self.racks.values().map(|r| r.free_space()).sum()
    }
    fn capacity(&self) -> u64 {
        self.racks.values().map(|r| r.capacity()).sum()
    }
    fn available(&self) -> bool {
        self.racks.values().any(|r| r.available())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use identity333::Keypair;

    fn node_with(id: &str, vols: &[(u64, u64)]) -> Node {
        let mut n = Node::new(id, Keypair::from_seed([id.as_bytes()[0]; 32]).node_id());
        for (i, (size, used)) in vols.iter().enumerate() {
            n.add_volume(Volume {
                id: format!("{id}-v{i}"),
                size: *size,
                used: *used,
                down: false,
            });
        }
        n
    }

    #[test]
    fn composite_sum_propagates() {
        let mut rack = Rack::new("r1");
        rack.add_node(node_with("n1", &[(1000, 100), (1000, 200)]));
        rack.add_node(node_with("n2", &[(500, 50)]));
        assert_eq!(rack.capacity(), 2500);
        assert_eq!(rack.free_space(), 2500 - 350);

        let mut zone = Zone::new("dc1");
        zone.add_rack(rack);
        assert_eq!(zone.capacity(), 2500);
    }

    #[test]
    fn available_any_child_available() {
        let mut n = Node::new("n1", Keypair::from_seed([0; 32]).node_id());
        n.add_volume(Volume { id: "v".into(), size: 100, used: 0, down: true });
        assert!(!n.available());
        n.add_volume(Volume { id: "v2".into(), size: 100, used: 0, down: false });
        assert!(n.available());
    }

    #[test]
    fn best_fit_picks_most_free() {
        let mut rack = Rack::new("r1");
        rack.add_node(node_with("n1", &[(1000, 900)])); // 100 free
        rack.add_node(node_with("n2", &[(1000, 200)])); // 800 free
        rack.add_node(node_with("n3", &[(1000, 500)])); // 500 free
        let mut zone = Zone::new("z1");
        zone.add_rack(rack);
        assert_eq!(zone.best_fit_node().unwrap().id, "n2");
    }

    #[test]
    fn empty_zone_no_best_fit() {
        let zone = Zone::new("empty");
        assert!(zone.best_fit_node().is_none());
    }
}
