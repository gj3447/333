// KG: sprint5C-editor-wasm-ui-2026-04-15
//! WASM bindings for CollabDocument — exposes RGA-backed collaborative text
//! editor to JavaScript.
//!
//! Wraps `CollabDocument` (char-level RGA CRDT, HLC timestamps, cursor
//! tracking) behind a `#[wasm_bindgen]` facade.  All JSON serialisation
//! lives here; inner crate types stay JS-free.
//!
//! # API surface
//! - `CollabEditorWasm::new(peer_id)` — create editor for local peer
//! - `insert_text(pos, text)` → insert chars at visible position
//! - `delete_range(start, len)` → delete visible characters
//! - `get_text()` → current document string
//! - `get_local_ops_json(since)` → JSON array of op-JSON strings since index
//! - `apply_remote_op_json(op_json)` → apply a remote RGAOp<char>
//! - `peer_id()` → local peer id
//! - `char_count()` → visible character count
//! - `update_cursor(pos, color)` / `cursors_json()` — cursor tracking
//! - `local_op_count()` → length of local op log

use wasm_bindgen::prelude::*;
use crate::apps::collab_editor::CollabDocument;
use crate::rga::RGAOp;

// ---------------------------------------------------------------------------
// CollabEditorWasm
// ---------------------------------------------------------------------------

/// WASM-exposed wrapper around `CollabDocument`.
///
/// Created via JS: `new wasm.CollabEditorWasm(peerId)`
///
/// KG: sprint5C-editor-wasm-ui-2026-04-15
#[wasm_bindgen]
pub struct CollabEditorWasm {
    doc: CollabDocument,
    peer_id_val: u32,
    /// All ops produced locally, stored as JSON strings for `get_local_ops_json`.
    local_ops: Vec<String>,
}

#[wasm_bindgen]
impl CollabEditorWasm {
    /// Create a new collaborative editor for `peer_id`.
    ///
    /// KG: sprint5C-editor-wasm-ui-2026-04-15
    #[wasm_bindgen(constructor)]
    pub fn new(peer_id: u32) -> Self {
        console_error_panic_hook::set_once();
        Self {
            doc: CollabDocument::new("default", peer_id),
            peer_id_val: peer_id,
            local_ops: Vec::new(),
        }
    }

    // -------------------------------------------------------------------------
    // Edit operations
    // -------------------------------------------------------------------------

    /// Insert `text` starting at visible position `pos`.
    ///
    /// Each character in `text` is inserted sequentially so RGA ordering is
    /// preserved.  Returns a JSON array of the produced op JSON strings.
    ///
    /// KG: sprint5C-editor-wasm-ui-2026-04-15
    pub fn insert_text(&mut self, pos: u32, text: &str) -> Result<String, JsValue> {
        let mut serialized: Vec<String> = Vec::new();
        let mut cur_pos = pos as usize;
        for ch in text.chars() {
            if let Some(op) = self.doc.insert(cur_pos, ch) {
                let s = serde_json::to_string(&op)
                    .unwrap_or_else(|_| "{}".to_string());
                self.local_ops.push(s.clone());
                serialized.push(s);
                cur_pos += 1;
            }
        }
        serde_json::to_string(&serialized)
            .map_err(|e| JsValue::from_str(&format!("serialize error: {e}")))
    }

    /// Delete `len` visible characters starting at `start`.
    ///
    /// Returns a JSON array of the produced delete op JSON strings.
    ///
    /// KG: sprint5C-editor-wasm-ui-2026-04-15
    pub fn delete_range(&mut self, start: u32, len: u32) -> Result<String, JsValue> {
        let mut serialized: Vec<String> = Vec::new();
        for _ in 0..len {
            // After each delete visible indices shift left, so always delete at `start`.
            if let Some(op) = self.doc.delete(start as usize) {
                let s = serde_json::to_string(&op)
                    .unwrap_or_else(|_| "{}".to_string());
                self.local_ops.push(s.clone());
                serialized.push(s);
            }
        }
        serde_json::to_string(&serialized)
            .map_err(|e| JsValue::from_str(&format!("serialize error: {e}")))
    }

    // -------------------------------------------------------------------------
    // Read
    // -------------------------------------------------------------------------

    /// Return the current document text.
    ///
    /// KG: sprint5C-editor-wasm-ui-2026-04-15
    pub fn get_text(&self) -> String {
        self.doc.text()
    }

    /// Return this peer's id.
    ///
    /// KG: sprint5C-editor-wasm-ui-2026-04-15
    pub fn peer_id(&self) -> u32 {
        self.peer_id_val
    }

    /// Return visible character count.
    ///
    /// KG: sprint5C-editor-wasm-ui-2026-04-15
    pub fn char_count(&self) -> u32 {
        self.doc.len() as u32
    }

    // -------------------------------------------------------------------------
    // Op sync
    // -------------------------------------------------------------------------

    /// Return a JSON array of locally-produced op JSON strings generated
    /// at-or-after index `since` (0-based).
    ///
    /// Pass `since = 0` to get all ops.  Pass the previous `local_op_count()`
    /// to get only new ops since last poll.
    ///
    /// KG: sprint5C-editor-wasm-ui-2026-04-15
    pub fn get_local_ops_json(&self, since: u32) -> String {
        let start = (since as usize).min(self.local_ops.len());
        let slice = &self.local_ops[start..];
        serde_json::to_string(slice).unwrap_or_else(|_| "[]".to_string())
    }

    /// Apply a remote op (JSON-encoded `RGAOp<char>`) to the local document.
    ///
    /// KG: sprint5C-editor-wasm-ui-2026-04-15
    pub fn apply_remote_op_json(&mut self, op_json: &str) -> Result<(), JsValue> {
        let op: RGAOp<char> = serde_json::from_str(op_json)
            .map_err(|e| JsValue::from_str(&format!("parse error: {e}")))?;
        self.doc.apply_remote(&op);
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Cursor
    // -------------------------------------------------------------------------

    /// Update the local peer's cursor position.
    ///
    /// KG: sprint5C-editor-wasm-ui-2026-04-15
    pub fn update_cursor(&mut self, position: u32, color: &str) {
        let pid = self.peer_id_val;
        self.doc.update_cursor(pid, position as usize, color);
    }

    /// Return all peer cursors as a JSON array of `{peer_id, position, color}`.
    ///
    /// KG: sprint5C-editor-wasm-ui-2026-04-15
    pub fn cursors_json(&self) -> String {
        serde_json::to_string(self.doc.cursors())
            .unwrap_or_else(|_| "[]".to_string())
    }

    /// Return total count of locally-produced ops (use as `since` on next poll).
    ///
    /// KG: sprint5C-editor-wasm-ui-2026-04-15
    pub fn local_op_count(&self) -> u32 {
        self.local_ops.len() as u32
    }
}
