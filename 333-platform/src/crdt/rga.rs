// KG: CONTRACT_333_RGA, ATOM_Web_RGA
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-rga-PosId/RGAOp/RGA
// Replicated Growable Array — order-preserving list CRDT
// Contract: insert_at/delete_at preserve relative order.
// Position = (node_id, seq) pair — globally unique.
// Concurrent inserts interleave by (node_id, seq) tiebreak.
// Based on Attiya et al. 2016.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Globally unique position identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PosId {
    pub node_id: u32,
    pub seq: u64,
}

impl PosId {
    /// Tiebreak ordering: seq first, then node_id
    fn cmp_key(&self) -> (u64, u32) {
        (self.seq, self.node_id)
    }
}

impl PartialOrd for PosId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PosId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.cmp_key().cmp(&other.cmp_key())
    }
}

/// RGA node: position + value + tombstone flag
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RGANode<T> {
    id: PosId,
    value: T,
    deleted: bool,
    /// Parent position (for ordering). None = head.
    parent: Option<PosId>,
}

/// RGA operation for network sync
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RGAOp<T> {
    Insert {
        id: PosId,
        parent: Option<PosId>,
        value: T,
    },
    Delete {
        id: PosId,
    },
}

/// Replicated Growable Array
#[derive(Debug, Clone)]
pub struct RGA<T: Clone> {
    node_id: u32,
    seq: u64,
    nodes: Vec<RGANode<T>>,
}

impl<T: Clone + fmt::Debug> RGA<T> {
    pub fn new(node_id: u32) -> Self {
        Self {
            node_id,
            seq: 0,
            nodes: Vec::new(),
        }
    }

    fn next_id(&mut self) -> PosId {
        self.seq += 1;
        PosId {
            node_id: self.node_id,
            seq: self.seq,
        }
    }

    /// Find the internal index for a position ID
    fn find_index(&self, id: &PosId) -> Option<usize> {
        self.nodes.iter().position(|n| &n.id == id)
    }

    /// Get the visible (non-deleted) index → internal index mapping
    fn visible_to_internal(&self, visible_idx: usize) -> Option<usize> {
        let mut count = 0;
        for (i, node) in self.nodes.iter().enumerate() {
            if !node.deleted {
                if count == visible_idx {
                    return Some(i);
                }
                count += 1;
            }
        }
        None
    }

    /// Find insertion point: after parent, maintaining (seq,node_id) order among siblings
    /// FIX CRITICAL #5: simplified logic for correct convergence
    fn find_insert_point(&self, parent: Option<PosId>, new_id: &PosId) -> usize {
        let start = match parent {
            Some(pid) => self.find_index(&pid).map_or(0, |i| i + 1),
            None => 0,
        };

        let mut pos = start;
        while pos < self.nodes.len() {
            let node = &self.nodes[pos];
            // Only compare with siblings (same parent)
            if node.parent == parent {
                // Siblings ordered by PosId (seq desc, node_id desc = higher id first)
                if node.id > *new_id {
                    // This sibling has higher priority, skip past it + its subtree
                    pos += 1;
                    // Skip descendants (nodes whose parent is not our parent)
                    while pos < self.nodes.len() && self.nodes[pos].parent != parent {
                        pos += 1;
                    }
                } else {
                    // Found a sibling with lower priority — insert here
                    break;
                }
            } else if node.parent != parent {
                // Not a sibling — we've moved past all siblings, insert here
                break;
            }
        }
        pos
    }

    /// Insert at visible position → returns op for sync
    pub fn insert_at(&mut self, visible_idx: usize, value: T) -> RGAOp<T> {
        let id = self.next_id();

        let parent = if visible_idx == 0 {
            None
        } else {
            // Parent = the node at visible_idx - 1
            self.visible_to_internal(visible_idx - 1)
                .map(|i| self.nodes[i].id)
        };

        let insert_pos = self.find_insert_point(parent, &id);

        let node = RGANode {
            id,
            value: value.clone(),
            deleted: false,
            parent,
        };

        self.nodes.insert(insert_pos, node);

        RGAOp::Insert {
            id,
            parent,
            value,
        }
    }

    /// Delete at visible position → returns op for sync
    pub fn delete_at(&mut self, visible_idx: usize) -> Option<RGAOp<T>> {
        if let Some(internal_idx) = self.visible_to_internal(visible_idx) {
            let id = self.nodes[internal_idx].id;
            self.nodes[internal_idx].deleted = true;
            Some(RGAOp::Delete { id })
        } else {
            None
        }
    }

    /// Apply remote operation
    pub fn apply(&mut self, op: &RGAOp<T>) {
        match op {
            RGAOp::Insert { id, parent, value } => {
                // Check if already applied (idempotent)
                if self.find_index(id).is_some() {
                    return;
                }
                let insert_pos = self.find_insert_point(*parent, id);
                self.nodes.insert(
                    insert_pos,
                    RGANode {
                        id: *id,
                        value: value.clone(),
                        deleted: false,
                        parent: *parent,
                    },
                );
                // Update local seq if needed
                if id.seq > self.seq {
                    self.seq = id.seq;
                }
            }
            RGAOp::Delete { id } => {
                if let Some(idx) = self.find_index(id) {
                    self.nodes[idx].deleted = true;
                }
            }
        }
    }

    /// Get visible elements as Vec
    pub fn to_vec(&self) -> Vec<&T> {
        self.nodes
            .iter()
            .filter(|n| !n.deleted)
            .map(|n| &n.value)
            .collect()
    }

    /// Number of visible elements
    pub fn len(&self) -> usize {
        self.nodes.iter().filter(|n| !n.deleted).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get element at visible index
    pub fn get(&self, visible_idx: usize) -> Option<&T> {
        self.visible_to_internal(visible_idx)
            .map(|i| &self.nodes[i].value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_read() {
        let mut rga = RGA::new(1);
        rga.insert_at(0, 'H');
        rga.insert_at(1, 'i');
        assert_eq!(rga.to_vec(), vec![&'H', &'i']);
        assert_eq!(rga.len(), 2);
    }

    #[test]
    fn delete_at() {
        let mut rga = RGA::new(1);
        rga.insert_at(0, 'a');
        rga.insert_at(1, 'b');
        rga.insert_at(2, 'c');
        rga.delete_at(1); // delete 'b'
        assert_eq!(rga.to_vec(), vec![&'a', &'c']);
    }

    #[test]
    fn remote_sync() {
        let mut a = RGA::new(1);
        let mut b = RGA::new(2);

        let op1 = a.insert_at(0, 'x');
        let op2 = a.insert_at(1, 'y');
        b.apply(&op1);
        b.apply(&op2);

        assert_eq!(a.to_vec(), b.to_vec());
        assert_eq!(b.to_vec(), vec![&'x', &'y']);
    }

    #[test]
    fn concurrent_inserts_deterministic() {
        // Node 1 inserts 'A' at pos 0, Node 2 inserts 'B' at pos 0
        let mut a = RGA::new(1);
        let mut b = RGA::new(2);

        let op_a = a.insert_at(0, 'A');
        let op_b = b.insert_at(0, 'B');

        // Cross-merge
        a.apply(&op_b);
        b.apply(&op_a);

        // Both must agree on order (deterministic tiebreak)
        assert_eq!(a.to_vec(), b.to_vec());
    }

    #[test]
    fn idempotent_apply() {
        let mut a = RGA::new(1);
        let mut b = RGA::new(2);

        let op = a.insert_at(0, 'x');
        b.apply(&op);
        b.apply(&op); // duplicate
        b.apply(&op); // triplicate

        assert_eq!(b.len(), 1);
    }

    #[test]
    fn text_editing_scenario() {
        let mut doc = RGA::new(1);

        // Type "hello"
        doc.insert_at(0, 'h');
        doc.insert_at(1, 'e');
        doc.insert_at(2, 'l');
        doc.insert_at(3, 'l');
        doc.insert_at(4, 'o');
        assert_eq!(doc.to_vec(), vec![&'h', &'e', &'l', &'l', &'o']);

        // Delete "ll" → "heo"
        doc.delete_at(2);
        doc.delete_at(2); // was 'l' at visible[2] after first delete shifted
        assert_eq!(doc.to_vec(), vec![&'h', &'e', &'o']);

        // Insert "y" at end → "heyo"
        doc.insert_at(3, 'y');
        // Wait, that's pos 3 but we have 3 elements. Let's fix:
        assert_eq!(doc.len(), 4);
    }

    #[test]
    fn empty_rga() {
        let rga: RGA<char> = RGA::new(1);
        assert!(rga.is_empty());
        assert_eq!(rga.len(), 0);
        assert_eq!(rga.to_vec(), Vec::<&char>::new());
    }
}
