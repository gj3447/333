// KG: SPAN_333_KillerApps — CollabEditor: P2P text editing with RGA CRDT
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-apps-editor-Cursor/CollabDocument
// Uses RGA for ordered text, HLC for causal ordering

use crate::rga::{RGA, RGAOp};
use crate::hlc::Hlc;
use serde::{Deserialize, Serialize};

/// Cursor position for a peer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cursor {
    pub peer_id: u32,
    pub position: usize,
    pub color: String,
}

/// Editor document state
pub struct CollabDocument {
    pub doc_id: String,
    rga: RGA<char>,
    hlc: Hlc,
    cursors: Vec<Cursor>,
}

impl CollabDocument {
    pub fn new(doc_id: &str, peer_id: u32) -> Self {
        Self {
            doc_id: doc_id.to_string(),
            rga: RGA::new(peer_id),
            hlc: Hlc::new(peer_id),
            cursors: Vec::new(),
        }
    }

    /// Insert character at position, returns op for broadcast
    pub fn insert(&mut self, pos: usize, ch: char) -> Option<RGAOp<char>> {
        self.hlc.tick();
        Some(self.rga.insert_at(pos, ch))
    }

    /// Delete character at position, returns op for broadcast
    pub fn delete(&mut self, pos: usize) -> Option<RGAOp<char>> {
        self.hlc.tick();
        self.rga.delete_at(pos)
    }

    /// Apply remote operation
    pub fn apply_remote(&mut self, op: &RGAOp<char>) {
        self.rga.apply(op);
    }

    /// Get current text
    pub fn text(&self) -> String {
        self.rga.to_vec().into_iter().collect()
    }

    /// Get text length
    pub fn len(&self) -> usize {
        self.rga.len()
    }

    /// Returns true if the document contains no characters.
    pub fn is_empty(&self) -> bool {
        self.rga.len() == 0
    }

    /// Update cursor position for a peer
    pub fn update_cursor(&mut self, peer_id: u32, position: usize, color: &str) {
        if let Some(c) = self.cursors.iter_mut().find(|c| c.peer_id == peer_id) {
            c.position = position;
        } else {
            self.cursors.push(Cursor {
                peer_id, position, color: color.to_string(),
            });
        }
    }

    /// Remove cursor for disconnected peer
    pub fn remove_cursor(&mut self, peer_id: u32) {
        self.cursors.retain(|c| c.peer_id != peer_id);
    }

    /// Get all cursors
    pub fn cursors(&self) -> &[Cursor] {
        &self.cursors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_read() {
        let mut doc = CollabDocument::new("test-doc", 1);
        doc.insert(0, 'H');
        doc.insert(1, 'i');
        assert_eq!(doc.text(), "Hi");
        assert_eq!(doc.len(), 2);
    }

    #[test]
    fn delete_character() {
        let mut doc = CollabDocument::new("test-doc", 1);
        doc.insert(0, 'A');
        doc.insert(1, 'B');
        doc.insert(2, 'C');
        doc.delete(1); // delete 'B'
        assert_eq!(doc.text(), "AC");
    }

    #[test]
    fn concurrent_edits_converge() {
        let mut doc1 = CollabDocument::new("doc", 1);
        let mut doc2 = CollabDocument::new("doc", 2);

        let op1 = doc1.insert(0, 'A').unwrap();
        let op2 = doc2.insert(0, 'B').unwrap();

        doc1.apply_remote(&op2);
        doc2.apply_remote(&op1);

        // Both should have same content (order may vary by peer_id tiebreak)
        assert_eq!(doc1.text(), doc2.text());
        assert_eq!(doc1.len(), 2);
    }

    // ── Additional tests (sprint5C) ──────────────────────────────────────────
    // # KG: sprint5C-editor-wasm-ui-2026-04-15

    #[test]
    fn insert_delete_text_result() {
        // Task: insert "Hello", delete middle chars, verify text()
        let mut doc = CollabDocument::new("doc", 1);
        for (i, ch) in "Hello".chars().enumerate() {
            doc.insert(i, ch);
        }
        assert_eq!(doc.text(), "Hello");
        doc.delete(1); // remove 'e' → "Hllo"
        doc.delete(1); // remove first 'l' → "Hlo"
        assert_eq!(doc.text(), "Hlo");
        assert_eq!(doc.len(), 3);
    }

    #[test]
    fn two_peers_concurrent_insert_converge() {
        // Both peers insert independently then exchange — must converge to same text.
        let mut doc1 = CollabDocument::new("doc", 10);
        let mut doc2 = CollabDocument::new("doc", 20);

        // Peer 10 types "AB"
        let op_a1 = doc1.insert(0, 'A').unwrap();
        let op_a2 = doc1.insert(1, 'B').unwrap();

        // Peer 20 types "XY" concurrently
        let op_x1 = doc2.insert(0, 'X').unwrap();
        let op_x2 = doc2.insert(1, 'Y').unwrap();

        // Cross-apply
        doc1.apply_remote(&op_x1);
        doc1.apply_remote(&op_x2);
        doc2.apply_remote(&op_a1);
        doc2.apply_remote(&op_a2);

        assert_eq!(doc1.text(), doc2.text(), "both peers must converge");
        assert_eq!(doc1.len(), 4);
    }

    #[test]
    fn deterministic_same_ops_same_text() {
        // Identical sequence of ops applied to two fresh docs → identical text.
        let mut doc_a = CollabDocument::new("doc", 5);
        let mut doc_b = CollabDocument::new("doc", 5);

        for (i, ch) in "rust".chars().enumerate() {
            let op_a = doc_a.insert(i, ch).unwrap();
            // Apply same op to doc_b as a remote op
            doc_b.apply_remote(&op_a);
        }

        assert_eq!(doc_a.text(), doc_b.text());
        assert_eq!(doc_a.text(), "rust");
    }

    #[test]
    fn cursor_tracking() {
        let mut doc = CollabDocument::new("doc", 1);
        doc.update_cursor(1, 5, "#f472b6");
        doc.update_cursor(2, 10, "#67e8f9");
        assert_eq!(doc.cursors().len(), 2);

        doc.update_cursor(1, 7, "#f472b6"); // update position
        assert_eq!(doc.cursors()[0].position, 7);

        doc.remove_cursor(2);
        assert_eq!(doc.cursors().len(), 1);
    }
}
