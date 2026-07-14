// KG: seed-rts-determinism-fixed-point-state-hash-2026-04-15
//! Frame state hashing + desync detection.
//!
//! Every 10 frames, each node broadcasts `frame_state_hash()` to all peers.
//! If any peer's hash differs → DesyncEvent emitted → snapshot resync triggered.
//!
//! Hashing uses blake3 (256-bit, WASM-friendly, no_std compatible).
//! Iteration order is enforced via BTreeMap (sorted keys) in `DeterministicSerialize`.

use std::collections::BTreeMap;
use std::vec::Vec;
use std::string::String;

/// Broadcast interval: hash every N frames.
pub const HASH_BROADCAST_INTERVAL: u64 = 10;

// ── DeterministicSerialize trait ─────────────────────────────────────────────

/// Trait for types that can be serialized in a deterministic (order-stable) byte form.
///
/// Implementors MUST guarantee:
/// - No HashMap/HashSet iteration (use BTreeMap/BTreeSet only)
/// - No platform-dependent padding or pointer-sized types
/// - All floats replaced by Fixed32
///
/// KG: seed-rts-determinism-fixed-point-state-hash-2026-04-15
pub trait DeterministicSerialize {
    /// Append a canonical byte representation to `buf`.
    /// The output must be identical on all nodes for the same logical state.
    fn det_serialize(&self, buf: &mut Vec<u8>);
}

// ── Blanket implementations for common types ─────────────────────────────────

impl DeterministicSerialize for u8 {
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        buf.push(*self);
    }
}

impl DeterministicSerialize for u32 {
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.to_be_bytes());
    }
}

impl DeterministicSerialize for u64 {
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.to_be_bytes());
    }
}

impl DeterministicSerialize for i32 {
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.to_be_bytes());
    }
}

impl DeterministicSerialize for i64 {
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.to_be_bytes());
    }
}

impl DeterministicSerialize for bool {
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        buf.push(*self as u8);
    }
}

impl<T: DeterministicSerialize> DeterministicSerialize for Vec<T> {
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        // Length prefix (u32 big-endian)
        (self.len() as u32).det_serialize(buf);
        for item in self.as_slice() {
            DeterministicSerialize::det_serialize(item, buf);
        }
    }
}

/// BTreeMap implementation — sorted iteration guarantees order independence.
impl<K: DeterministicSerialize + Ord, V: DeterministicSerialize> DeterministicSerialize
    for BTreeMap<K, V>
{
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        (self.len() as u32).det_serialize(buf);
        // BTreeMap iteration is sorted by key — deterministic across all platforms.
        for (k, v) in self.iter() {
            DeterministicSerialize::det_serialize(k, buf);
            DeterministicSerialize::det_serialize(v, buf);
        }
    }
}

impl DeterministicSerialize for String {
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        (self.len() as u32).det_serialize(buf);
        buf.extend_from_slice(self.as_bytes());
    }
}

impl DeterministicSerialize for [u8; 32] {
    fn det_serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(self);
    }
}

// ── DeterministicDeserialize trait ───────────────────────────────────────────
//
// Companion to DeterministicSerialize.  Reads bytes written by det_serialize
// back into a value.  Returns (value, bytes_consumed) or Err on truncation /
// corruption.  Callers must verify the full buffer is consumed for round-trip
// safety.
//
// Format contract: every impl must consume *exactly* as many bytes as
// DeterministicSerialize::det_serialize() writes for the same value.
//
// KG: prod-wiring-payload-ggrs-serde-2026-04-15

/// Fallible deterministic deserialization — mirror of [`DeterministicSerialize`].
///
/// Reads bytes produced by `det_serialize` and returns the parsed value plus
/// the number of bytes consumed, or an error string on parse failure.
///
/// KG: prod-wiring-payload-ggrs-serde-2026-04-15
pub trait DeterministicDeserialize: Sized {
    fn det_deserialize(buf: &[u8]) -> Result<(Self, usize), String>;
}

// ── Blanket DeterministicDeserialize impls ────────────────────────────────────

impl DeterministicDeserialize for u8 {
    fn det_deserialize(buf: &[u8]) -> Result<(Self, usize), String> {
        if buf.is_empty() {
            return Err("det_deserialize u8: buffer empty".to_string());
        }
        Ok((buf[0], 1))
    }
}

impl DeterministicDeserialize for u32 {
    fn det_deserialize(buf: &[u8]) -> Result<(Self, usize), String> {
        if buf.len() < 4 {
            return Err(format!("det_deserialize u32: need 4 bytes, got {}", buf.len()));
        }
        Ok((u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]), 4))
    }
}

impl DeterministicDeserialize for u64 {
    fn det_deserialize(buf: &[u8]) -> Result<(Self, usize), String> {
        if buf.len() < 8 {
            return Err(format!("det_deserialize u64: need 8 bytes, got {}", buf.len()));
        }
        Ok((u64::from_be_bytes(buf[..8].try_into().unwrap()), 8))
    }
}

impl DeterministicDeserialize for u16 {
    fn det_deserialize(buf: &[u8]) -> Result<(Self, usize), String> {
        if buf.len() < 2 {
            return Err(format!("det_deserialize u16: need 2 bytes, got {}", buf.len()));
        }
        Ok((u16::from_be_bytes([buf[0], buf[1]]), 2))
    }
}

impl<T: DeterministicDeserialize> DeterministicDeserialize for Vec<T> {
    fn det_deserialize(buf: &[u8]) -> Result<(Self, usize), String> {
        let (len, mut pos) = u32::det_deserialize(buf)?;
        let mut result = Vec::with_capacity(len as usize);
        for _ in 0..len {
            let (item, consumed) = T::det_deserialize(&buf[pos..])?;
            result.push(item);
            pos += consumed;
        }
        Ok((result, pos))
    }
}

impl<K, V> DeterministicDeserialize for BTreeMap<K, V>
where
    K: DeterministicDeserialize + Ord,
    V: DeterministicDeserialize,
{
    fn det_deserialize(buf: &[u8]) -> Result<(Self, usize), String> {
        let (len, mut pos) = u32::det_deserialize(buf)?;
        let mut map = BTreeMap::new();
        for _ in 0..len {
            let (k, k_consumed) = K::det_deserialize(&buf[pos..])?;
            pos += k_consumed;
            let (v, v_consumed) = V::det_deserialize(&buf[pos..])?;
            pos += v_consumed;
            map.insert(k, v);
        }
        Ok((map, pos))
    }
}

impl DeterministicDeserialize for [u8; 32] {
    fn det_deserialize(buf: &[u8]) -> Result<(Self, usize), String> {
        if buf.len() < 32 {
            return Err(format!("det_deserialize [u8;32]: need 32 bytes, got {}", buf.len()));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&buf[..32]);
        Ok((arr, 32))
    }
}

// ── Core hashing function ─────────────────────────────────────────────────────

/// Compute a 32-byte Blake3 hash of the game state at `frame_n`.
///
/// The hash covers:
/// 1. Frame number (u64 big-endian) — prevents hash collisions across frames
/// 2. Serialized tactical state (via `DeterministicSerialize`)
///
/// KG: seed-rts-determinism-fixed-point-state-hash-2026-04-15
pub fn frame_state_hash<T: DeterministicSerialize>(frame_n: u64, tactical: &T) -> [u8; 32] {
    let mut buf = Vec::with_capacity(256);
    // Domain separator
    buf.extend_from_slice(b"333:frame_state_hash:v1:");
    // Frame number as prefix
    frame_n.det_serialize(&mut buf);
    // State
    tactical.det_serialize(&mut buf);

    // Blake3 hash
    let hash = blake3::hash(&buf);
    *hash.as_bytes()
}

/// Returns true if this frame should broadcast a state hash.
#[inline]
pub fn should_broadcast_hash(frame_n: u64) -> bool {
    frame_n.is_multiple_of(HASH_BROADCAST_INTERVAL)
}

// ── DesyncDetector ────────────────────────────────────────────────────────────

/// Describes a detected desynchronization between the local node and a peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesyncEvent {
    /// Frame at which desync was detected.
    pub frame_n: u64,
    /// Peer node identifier.
    pub peer_id: u32,
    /// Local hash at that frame.
    pub local_hash: [u8; 32],
    /// Peer's hash at that frame.
    pub peer_hash: [u8; 32],
}

/// Tracks local and peer hashes; detects mismatches.
///
/// # Protocol
/// 1. Local node calls `record_local(frame, hash)` every `HASH_BROADCAST_INTERVAL` frames.
/// 2. When a peer hash arrives from the network, call `observe_peer(peer_id, frame, hash)`.
/// 3. If `Some(DesyncEvent)` is returned, initiate snapshot resync with that peer.
///
/// KG: seed-rts-determinism-fixed-point-state-hash-2026-04-15
pub struct DesyncDetector {
    /// Local hashes indexed by frame_n.
    local_hashes: BTreeMap<u64, [u8; 32]>,
    /// Maximum number of hashes to retain per direction (ring buffer behavior).
    retention: usize,
}

impl DesyncDetector {
    /// Create a new detector. `retention` controls how many frames of hashes to keep.
    pub fn new(retention: usize) -> Self {
        Self {
            local_hashes: BTreeMap::new(),
            retention,
        }
    }

    /// Record the local hash for `frame_n`.
    pub fn record_local(&mut self, frame_n: u64, hash: [u8; 32]) {
        self.local_hashes.insert(frame_n, hash);
        self.evict();
    }

    /// Observe a peer's hash.  Returns `Some(DesyncEvent)` if it mismatches local.
    /// Returns `None` if the local hash for that frame is not yet recorded (unknown = no alarm).
    pub fn observe_peer(
        &self,
        peer_id: u32,
        frame_n: u64,
        peer_hash: [u8; 32],
    ) -> Option<DesyncEvent> {
        if let Some(&local_hash) = self.local_hashes.get(&frame_n) {
            if local_hash != peer_hash {
                return Some(DesyncEvent {
                    frame_n,
                    peer_id,
                    local_hash,
                    peer_hash,
                });
            }
        }
        None
    }

    /// Remove entries older than the retention window.
    fn evict(&mut self) {
        while self.local_hashes.len() > self.retention {
            // Remove the smallest (oldest) key
            if let Some(&oldest) = self.local_hashes.keys().next() {
                self.local_hashes.remove(&oldest);
            } else {
                break;
            }
        }
    }

    /// Number of locally stored frame hashes.
    pub fn stored_count(&self) -> usize {
        self.local_hashes.len()
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: simple deterministic state
    struct SimpleState {
        values: Vec<u64>,
    }

    impl DeterministicSerialize for SimpleState {
        fn det_serialize(&self, buf: &mut Vec<u8>) {
            self.values.det_serialize(buf);
        }
    }

    /// Test 1: Hash stability — same frame + state always yields same hash
    #[test]
    fn hash_stability_same_inputs() {
        let state = SimpleState { values: vec![1, 2, 3, 42] };
        let h1 = frame_state_hash(100, &state);
        let h2 = frame_state_hash(100, &state);
        assert_eq!(h1, h2, "same frame+state must produce identical hash");
    }

    /// Test 2: Different frames → different hashes (even with same state)
    #[test]
    fn hash_changes_with_frame() {
        let state = SimpleState { values: vec![1, 2, 3] };
        let h100 = frame_state_hash(100, &state);
        let h101 = frame_state_hash(101, &state);
        assert_ne!(h100, h101, "different frames must yield different hashes");
    }

    /// Test 3: Different states → different hashes (same frame)
    #[test]
    fn hash_changes_with_state() {
        let state_a = SimpleState { values: vec![1, 2, 3] };
        let state_b = SimpleState { values: vec![1, 2, 4] }; // one byte changed
        let ha = frame_state_hash(50, &state_a);
        let hb = frame_state_hash(50, &state_b);
        assert_ne!(ha, hb, "different states must yield different hashes");
    }

    /// Test 4: DesyncDetector — same hash → no event
    #[test]
    fn desync_no_event_when_matching() {
        let mut det = DesyncDetector::new(32);
        let hash = [0xABu8; 32];
        det.record_local(10, hash);
        let event = det.observe_peer(2, 10, hash);
        assert!(event.is_none(), "matching hashes must not trigger desync");
    }

    /// Test 5: DesyncDetector — different hash → Some(DesyncEvent)
    #[test]
    fn desync_event_on_mismatch() {
        let mut det = DesyncDetector::new(32);
        let local_hash = [0x11u8; 32];
        let peer_hash = [0x22u8; 32];
        det.record_local(20, local_hash);
        let event = det.observe_peer(5, 20, peer_hash);
        assert!(event.is_some(), "mismatched hashes must trigger desync");
        let ev = event.unwrap();
        assert_eq!(ev.frame_n, 20);
        assert_eq!(ev.peer_id, 5);
        assert_eq!(ev.local_hash, local_hash);
        assert_eq!(ev.peer_hash, peer_hash);
    }

    /// Test 6: DesyncDetector — unknown frame → no event (not enough info)
    #[test]
    fn desync_no_event_unknown_frame() {
        let det = DesyncDetector::new(32);
        let event = det.observe_peer(1, 999, [0xFF; 32]);
        assert!(event.is_none(), "unknown frame must not trigger false desync");
    }

    /// Test 7: should_broadcast_hash cadence
    #[test]
    fn broadcast_every_10_frames() {
        let broadcast_frames: Vec<u64> = (0..50).filter(|&f| should_broadcast_hash(f)).collect();
        assert_eq!(broadcast_frames, vec![0, 10, 20, 30, 40]);
    }

    /// Test 8: BTreeMap DeterministicSerialize — insertion order doesn't matter
    #[test]
    fn btreemap_order_independent() {
        let mut map_a: BTreeMap<u32, u64> = BTreeMap::new();
        map_a.insert(1, 100);
        map_a.insert(2, 200);
        map_a.insert(3, 300);

        let mut map_b: BTreeMap<u32, u64> = BTreeMap::new();
        map_b.insert(3, 300); // different insertion order
        map_b.insert(1, 100);
        map_b.insert(2, 200);

        let mut buf_a = Vec::new();
        let mut buf_b = Vec::new();
        map_a.det_serialize(&mut buf_a);
        map_b.det_serialize(&mut buf_b);
        assert_eq!(buf_a, buf_b, "BTreeMap must serialize identically regardless of insertion order");
    }

    /// Test 9: Retention eviction
    #[test]
    fn detector_retention_evicts_old() {
        let mut det = DesyncDetector::new(5);
        for frame in 0..10u64 {
            det.record_local(frame * 10, [frame as u8; 32]);
        }
        assert!(det.stored_count() <= 5, "retention must cap stored hashes");
        // Old frame 0 should have been evicted; its peer report must not fire
        let event = det.observe_peer(1, 0, [0xFF; 32]); // frame 0 evicted → no event
        assert!(event.is_none(), "evicted frame should not trigger event");
    }
}
