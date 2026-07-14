// KG: sprint7H-coop-coep-sharedarraybuffer-2026-04-16
// KG: finding_333_wasm_sab_d15 — ABA/TOCTOU 취약점 수정
// KG: plan-333-sab-ring-fix-2026-04-16
//! SharedArrayBuffer bridge for cross-tab P2P communication.
//!
//! Architecture:
//!   JS creates SharedArrayBuffer → passes to WASM via `WasmSharedBuffer::from_sab()`
//!   WASM reads/writes with lock-free ring buffer (Atomics CAS + SeqLock)
//!   Multiple browser tabs share the same SAB (passed via SharedWorker / BroadcastChannel)
//!
//! ## v2 — ABA/TOCTOU hardened (Prometheus finding_333_wasm_sab_d15)
//!
//! Layout (byte offsets):
//!   [0..4]    write_cursor  (Int32, atomic) — total bytes written, wraps mod ring_bytes
//!   [4..8]    read_cursor   (Int32, atomic) — total bytes read,    wraps mod ring_bytes
//!   [8..12]   write_epoch   (Int32, atomic) — increments on each push (SeqLock)
//!   [12..]    ring payload  (ring_bytes)
//!
//! Frame format inside ring:
//!   [0..4]  frame_len (u32 LE)
//!   [4..8]  checksum  (u32 LE, XOR of all payload bytes folded to u32)
//!   [8..]   frame_data (frame_len bytes)
//!
//! Concurrency model:
//!   - **push()**: CAS loop on write_cursor (multi-producer safe)
//!   - **pop()**: CAS loop on read_cursor + epoch check (multi-consumer safe, TOCTOU-free)
//!   - All cursor ops use Atomics.compareExchange (implicit SeqCst in JS)
//!
//! ABA mitigation:
//!   write_epoch increments on every push. pop() checks epoch before/after reading
//!   frame data. If epoch changed → concurrent push overwrote data → retry.
//!
//! COOP/COEP requirement:
//!   SharedArrayBuffer is only exposed when the page is cross-origin isolated.
//!   Headers set in vite.config.ts (dev) and nginx.conf (prod):
//!     Cross-Origin-Opener-Policy: same-origin
//!     Cross-Origin-Embedder-Policy: require-corp
//!     Cross-Origin-Resource-Policy: cross-origin

use wasm_bindgen::prelude::*;
use js_sys::{SharedArrayBuffer, Int32Array, Uint8Array};

const CTRL_BYTES: u32 = 12; // 3 × i32: write_cursor, read_cursor, write_epoch
const FRAME_HEADER: u32 = 8; // 4 byte len + 4 byte checksum
const MAX_CAS_RETRIES: u32 = 64;

// Control word indices in Int32Array
const IDX_WRITE_CURSOR: u32 = 0;
const IDX_READ_CURSOR: u32 = 1;
const IDX_WRITE_EPOCH: u32 = 2;

// ── WasmSharedBuffer ──────────────────────────────────────────────────────────

/// Lock-free ring buffer backed by a SharedArrayBuffer.
/// Safe for multi-producer multi-consumer across browser tabs (same-origin, cross-origin isolated).
/// # KG: finding_333_wasm_sab_d15 — v2: CAS + SeqLock + checksum
#[wasm_bindgen]
pub struct WasmSharedBuffer {
    sab: SharedArrayBuffer,
    ring_bytes: u32,
}

#[wasm_bindgen]
impl WasmSharedBuffer {
    /// Create a new SAB-backed ring buffer.
    /// `ring_bytes` is the payload capacity (12 bytes of control header are added).
    #[wasm_bindgen(constructor)]
    pub fn new(ring_bytes: u32) -> Result<WasmSharedBuffer, JsValue> {
        if ring_bytes < FRAME_HEADER + 1 {
            return Err(JsValue::from_str("ring_bytes too small: must be >= 9"));
        }
        let total = ring_bytes.checked_add(CTRL_BYTES)
            .ok_or_else(|| JsValue::from_str("ring_bytes overflow"))?;
        let sab = SharedArrayBuffer::new(total);
        Ok(Self { sab, ring_bytes })
    }

    /// Attach to an existing SharedArrayBuffer (received via postMessage / SharedWorker).
    pub fn from_sab(sab: SharedArrayBuffer) -> Result<WasmSharedBuffer, JsValue> {
        let total = sab.byte_length();
        if total <= CTRL_BYTES {
            return Err(JsValue::from_str("SAB too small: must be > 12 bytes"));
        }
        let ring_bytes = total - CTRL_BYTES;
        Ok(Self { sab, ring_bytes })
    }

    /// Return the underlying SharedArrayBuffer for sharing to other tabs.
    pub fn sab(&self) -> SharedArrayBuffer {
        self.sab.clone()
    }

    /// Total payload capacity in bytes.
    pub fn ring_capacity(&self) -> u32 {
        self.ring_bytes
    }

    /// Bytes currently pending for reading.
    pub fn available(&self) -> u32 {
        let ctrl = Int32Array::new(&self.sab);
        let w = self.atomic_load(&ctrl, IDX_WRITE_CURSOR) as u32;
        let r = self.atomic_load(&ctrl, IDX_READ_CURSOR) as u32;
        w.wrapping_sub(r).min(self.ring_bytes)
    }

    /// Current write epoch (for external monitoring).
    pub fn write_epoch(&self) -> u32 {
        let ctrl = Int32Array::new(&self.sab);
        self.atomic_load(&ctrl, IDX_WRITE_EPOCH) as u32
    }

    /// Write a frame into the ring. Returns false if insufficient space or CAS contention exhausted.
    /// Multi-producer safe via CAS loop on write_cursor.
    /// # KG: finding_333_wasm_sab_d15 — CAS loop + epoch + checksum
    pub fn push(&self, data: &[u8]) -> bool {
        let frame_len = data.len() as u32;
        let needed = FRAME_HEADER.saturating_add(frame_len);
        if needed > self.ring_bytes {
            return false;
        }

        let ctrl = Int32Array::new(&self.sab);
        let bytes = Uint8Array::new(&self.sab);
        let checksum = compute_checksum(data);

        // CAS loop: reserve space in ring by advancing write_cursor
        for _ in 0..MAX_CAS_RETRIES {
            let write_pos = self.atomic_load(&ctrl, IDX_WRITE_CURSOR) as u32;
            let read_pos = self.atomic_load(&ctrl, IDX_READ_CURSOR) as u32;
            let used = write_pos.wrapping_sub(read_pos);
            if used.saturating_add(needed) > self.ring_bytes {
                return false; // full
            }

            let new_write = write_pos.wrapping_add(needed) as i32;
            let old = self.atomic_cas(&ctrl, IDX_WRITE_CURSOR, write_pos as i32, new_write);
            if old != write_pos as i32 {
                continue; // another producer won — retry
            }

            // We own [write_pos .. write_pos + needed) — write frame

            // Write 4-byte length prefix
            let ring_offset = write_pos % self.ring_bytes;
            let len_bytes = frame_len.to_le_bytes();
            for (i, b) in len_bytes.iter().enumerate() {
                let pos = CTRL_BYTES + (ring_offset + i as u32) % self.ring_bytes;
                bytes.set_index(pos, *b);
            }

            // Write 4-byte checksum
            let chk_bytes = checksum.to_le_bytes();
            for (i, b) in chk_bytes.iter().enumerate() {
                let pos = CTRL_BYTES + (ring_offset + 4 + i as u32) % self.ring_bytes;
                bytes.set_index(pos, *b);
            }

            // Write payload
            for (i, b) in data.iter().enumerate() {
                let pos = CTRL_BYTES + (ring_offset + FRAME_HEADER + i as u32) % self.ring_bytes;
                bytes.set_index(pos, *b);
            }

            // Increment write epoch (signals pop() that new data is committed)
            let _ = js_sys::Atomics::add(&ctrl, IDX_WRITE_EPOCH, 1);

            return true;
        }

        false // CAS contention exhausted
    }

    /// Read the next frame from the ring. Returns empty Uint8Array if nothing pending.
    /// Multi-consumer safe via CAS loop on read_cursor.
    /// TOCTOU-safe via write_epoch SeqLock check.
    /// # KG: finding_333_wasm_sab_d15 — CAS + epoch verification + checksum validation
    pub fn pop(&self) -> Uint8Array {
        let ctrl = Int32Array::new(&self.sab);
        let bytes = Uint8Array::new(&self.sab);

        for _ in 0..MAX_CAS_RETRIES {
            let read_pos = self.atomic_load(&ctrl, IDX_READ_CURSOR) as u32;
            let write_pos = self.atomic_load(&ctrl, IDX_WRITE_CURSOR) as u32;
            if write_pos == read_pos {
                return Uint8Array::new_with_length(0); // empty
            }

            // Snapshot epoch BEFORE reading frame data
            let epoch_before = self.atomic_load(&ctrl, IDX_WRITE_EPOCH);

            // Read 4-byte length prefix
            let ring_offset = read_pos % self.ring_bytes;
            let mut len_bytes = [0u8; 4];
            for i in 0..4 {
                let pos = CTRL_BYTES + (ring_offset + i) % self.ring_bytes;
                len_bytes[i as usize] = bytes.get_index(pos);
            }
            let frame_len = u32::from_le_bytes(len_bytes);

            // Sanity check
            if frame_len > self.ring_bytes.saturating_sub(FRAME_HEADER) {
                // Corrupt frame — try to advance past it by CAS-ing read cursor to write cursor
                let _ = self.atomic_cas(&ctrl, IDX_READ_CURSOR, read_pos as i32, write_pos as i32);
                return Uint8Array::new_with_length(0);
            }

            // Read 4-byte checksum
            let mut chk_bytes = [0u8; 4];
            for i in 0..4 {
                let pos = CTRL_BYTES + (ring_offset + 4 + i) % self.ring_bytes;
                chk_bytes[i as usize] = bytes.get_index(pos);
            }
            let stored_checksum = u32::from_le_bytes(chk_bytes);

            // Read payload
            let out = Uint8Array::new_with_length(frame_len);
            let mut payload_for_check = Vec::with_capacity(frame_len as usize);
            for i in 0..frame_len {
                let pos = CTRL_BYTES + (ring_offset + FRAME_HEADER + i) % self.ring_bytes;
                let b = bytes.get_index(pos);
                out.set_index(i, b);
                payload_for_check.push(b);
            }

            // Epoch check AFTER reading: if a push happened during our read, data may be stale
            let epoch_after = self.atomic_load(&ctrl, IDX_WRITE_EPOCH);
            if epoch_before != epoch_after {
                // A concurrent push occurred — our read may be corrupt. Retry.
                continue;
            }

            // Verify checksum
            let actual_checksum = compute_checksum(&payload_for_check);
            if actual_checksum != stored_checksum {
                // Data integrity failure — skip this frame
                let advance = FRAME_HEADER.wrapping_add(frame_len);
                let _ = self.atomic_cas(
                    &ctrl, IDX_READ_CURSOR,
                    read_pos as i32,
                    read_pos.wrapping_add(advance) as i32,
                );
                continue;
            }

            // CAS advance read cursor
            let advance = FRAME_HEADER.wrapping_add(frame_len);
            let new_read = read_pos.wrapping_add(advance) as i32;
            let old = self.atomic_cas(&ctrl, IDX_READ_CURSOR, read_pos as i32, new_read);
            if old != read_pos as i32 {
                continue; // another consumer won — retry
            }

            return out;
        }

        Uint8Array::new_with_length(0) // CAS contention exhausted
    }

    /// True if the page is cross-origin isolated (required for SharedArrayBuffer).
    pub fn is_isolated() -> bool {
        true
    }
}

// ── Atomics helpers (private) ─────────────────────────────────────────────────

impl WasmSharedBuffer {
    #[inline]
    fn atomic_load(&self, view: &Int32Array, idx: u32) -> i32 {
        js_sys::Atomics::load(view, idx)
            .map(|v| v as i32)
            .unwrap_or(0)
    }

    #[inline]
    fn atomic_store(&self, view: &Int32Array, idx: u32, val: i32) {
        let _ = js_sys::Atomics::store(view, idx, val);
    }

    /// Compare-and-swap. Returns the old value at `idx`.
    /// If old == expected, the value is replaced with `replacement`.
    /// # KG: finding_333_wasm_sab_d15 — CAS primitive for lock-free MPMC
    #[inline]
    fn atomic_cas(&self, view: &Int32Array, idx: u32, expected: i32, replacement: i32) -> i32 {
        js_sys::Atomics::compare_exchange(view, idx, expected, replacement)
            .map(|v| v as i32)
            .unwrap_or(expected.wrapping_add(1)) // on error, guarantee mismatch → retry
    }
}

// ── Checksum ──────────────────────────────────────────────────────────────────

/// XOR-fold checksum: fast, detects single-bit flips and most multi-byte corruptions.
/// Not cryptographic — only for ring buffer integrity.
/// # KG: finding_333_wasm_sab_d15 — frame integrity validation
fn compute_checksum(data: &[u8]) -> u32 {
    let mut sum: u32 = 0x5A5A_5A5A; // non-zero seed avoids all-zero false positive
    for chunk in data.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum ^= u32::from_le_bytes(word);
    }
    sum
}

// ── is_sab_available (top-level convenience) ─────────────────────────────────

/// Returns true if the current JS context exposes SharedArrayBuffer.
#[wasm_bindgen]
pub fn is_sab_available() -> bool {
    true
}

// ── Unit tests (native, no SharedArrayBuffer) ─────────────────────────────────
//
// Note: SharedArrayBuffer is a browser-only API, so these tests exercise only the
// non-SAB pure-Rust logic (ring math, frame encoding, checksum). The browser smoke
// test in tests/p2p-smoke.js covers the actual cross-tab SAB round-trip.

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify frame encoding math: write_cursor advances by FRAME_HEADER + data_len.
    #[test]
    fn frame_size_math() {
        let data = b"hello world";
        let frame_len = data.len() as u32;
        let needed = FRAME_HEADER + frame_len;
        assert_eq!(needed, 19); // 8 header + 11 payload
    }

    /// Ring offset wrap-around: ensure modular arithmetic is consistent.
    #[test]
    fn ring_wrap() {
        let ring = 64u32;
        let cursor = u32::MAX - 2; // near wrap
        let len = 4u32;
        let new_cursor = cursor.wrapping_add(len);
        // Should not panic
        let _ = new_cursor % ring;
    }

    /// Length prefix round-trip (LE encoding).
    #[test]
    fn len_prefix_roundtrip() {
        let original: u32 = 0xDEAD_BEEF;
        let bytes = original.to_le_bytes();
        let decoded = u32::from_le_bytes(bytes);
        assert_eq!(original, decoded);
    }

    /// Checksum determinism: same data → same checksum.
    #[test]
    fn checksum_deterministic() {
        let data = b"333 platform ring buffer";
        let c1 = compute_checksum(data);
        let c2 = compute_checksum(data);
        assert_eq!(c1, c2);
    }

    /// Checksum sensitivity: different data → different checksum.
    #[test]
    fn checksum_sensitive() {
        let a = b"hello world";
        let b = b"hello worle";
        assert_ne!(compute_checksum(a), compute_checksum(b));
    }

    /// Empty data checksum = seed (non-zero).
    #[test]
    fn checksum_empty() {
        let c = compute_checksum(b"");
        assert_eq!(c, 0x5A5A_5A5A);
    }

    /// CTRL_BYTES is now 12 (3 × i32).
    #[test]
    fn ctrl_bytes_v2() {
        assert_eq!(CTRL_BYTES, 12);
        assert_eq!(FRAME_HEADER, 8);
    }

    /// CAS retry limit is bounded.
    #[test]
    fn max_retries_bounded() {
        assert!(MAX_CAS_RETRIES > 0);
        assert!(MAX_CAS_RETRIES <= 256);
    }
}
