//! Record frame + envelope codec with the rolling crc32c chain.
//!
//! Format absorbed from etcd `server/storage/wal` @6006f405 (KG:
//! absorption-etcd-wal-invariants-2026-07-17, FMT-1/2/3/5/6):
//!
//! ```text
//! frame    = lenField:u64-LE | envelope | zero-pad
//! lenField = lower 56 bits: envelope byte length L
//!            top byte: 0x80|pad (3-bit pad count) when L % 8 != 0, else 0
//! envelope = kind:u8 | reserved:[u8;3] (zero) | crc:u32-LE | data
//! ```
//!
//! Every frame starts 8-byte aligned, so a torn write can never split the
//! lenField itself (FMT-2). The crc is *cumulative*: the running crc32c state
//! is updated with each data-bearing record's `data` and the post-update value
//! is stored in that record (FMT-3). A `CrcSeed` record carries the running
//! state across segment boundaries without updating it (FMT-4 handoff).
//!
//! Error taxonomy (post-review): a decode failure names its shape so the
//! recovery layer can classify it —
//! - [`WalError::TornRecord`]: genuinely *partial* bytes at EOF (torn candidate)
//! - [`WalError::CorruptFrame`]: complete-enough but structurally invalid
//!   (unknown kind, bogus/oversized length, reserved bits) — same fatality
//!   class as a crc break, NOT an automatic repair
//! - [`WalError::CrcMismatch`]: complete record, broken chain

use std::io::{self, Read};

use crate::error::WalError;

/// Max envelope length accepted before allocating (FMT-6 sanity gate). The
/// writer enforces the same bound at `append()` time (write/read symmetry).
pub const MAX_ENVELOPE_LEN: u64 = 16 * 1024 * 1024;

pub const ENVELOPE_HEADER: usize = 8; // kind(1) + reserved(3) + crc(4)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    /// Segment header: `crc` = running chain state at segment start, no data.
    CrcSeed,
    /// Opaque WAL-level metadata, written once per segment header.
    Metadata,
    /// A substrate record: data = seq:u64-LE | payload.
    Entry,
}

impl RecordKind {
    fn to_u8(self) -> u8 {
        match self {
            RecordKind::CrcSeed => 1,
            RecordKind::Metadata => 2,
            RecordKind::Entry => 3,
        }
    }

    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(RecordKind::CrcSeed),
            2 => Some(RecordKind::Metadata),
            3 => Some(RecordKind::Entry),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Record {
    pub kind: RecordKind,
    pub crc: u32,
    pub data: Vec<u8>,
}

/// A decode failure plus how many bytes the failing frame spans when that is
/// knowable — the recovery layer scans exactly this window for the
/// zero-sector torn heuristic (etcd `isTornEntry` scans only the failing
/// record, never the whole remaining file).
#[derive(Debug)]
pub struct DecodeFailure {
    pub error: WalError,
    /// Total frame bytes (lenField + envelope + pad) when the length field
    /// was readable and sane; `None` when the length itself is garbage.
    pub frame_len: Option<u64>,
}

impl DecodeFailure {
    fn bare(error: WalError) -> Self {
        DecodeFailure { error, frame_len: None }
    }
}

/// Encode one record into `out`, updating the rolling crc `state` for
/// data-bearing kinds. Returns bytes appended (always a multiple of 8).
pub fn encode_record(state: &mut u32, kind: RecordKind, data: &[u8], out: &mut Vec<u8>) -> usize {
    let crc = match kind {
        RecordKind::CrcSeed => *state,
        _ => {
            *state = crc32c::crc32c_append(*state, data);
            *state
        }
    };

    let env_len = ENVELOPE_HEADER + data.len();
    let pad = (8 - (env_len % 8)) % 8;
    let mut len_field = env_len as u64;
    if pad != 0 {
        len_field |= (0x80 | pad as u64) << 56;
    }

    let before = out.len();
    out.extend_from_slice(&len_field.to_le_bytes());
    out.push(kind.to_u8());
    out.extend_from_slice(&[0u8; 3]);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(&[0u8; 8][..pad]);
    out.len() - before
}

/// Outcome of trying to decode a single frame.
pub enum Decoded {
    /// A complete, crc-consistent record (`usize` = frame bytes consumed).
    Record(Record, usize),
    /// lenField == 0 → no more data was ever written here (FMT-5).
    ZeroLen,
    /// Clean EOF exactly on a frame boundary.
    Eof,
}

/// Decode one frame from `r`, verifying the rolling crc chain.
///
/// `state` is advanced for data-bearing records; a `CrcSeed` *resets* it to
/// the stored value (segment handoff — the caller must gate the handoff
/// value against the state it arrived with, FMT-4).
pub fn decode_record(state: &mut u32, r: &mut impl Read) -> Result<Decoded, DecodeFailure> {
    let mut len_buf = [0u8; 8];
    match read_exact_or_eof(r, &mut len_buf).map_err(DecodeFailure::bare)? {
        ReadOutcome::Eof => return Ok(Decoded::Eof),
        ReadOutcome::Partial => return Err(DecodeFailure::bare(WalError::TornRecord)),
        ReadOutcome::Full => {}
    }
    let len_field = u64::from_le_bytes(len_buf);
    if len_field == 0 {
        return Ok(Decoded::ZeroLen);
    }

    let (env_len, pad) = match split_len_field(len_field) {
        Some(v) => v,
        // Reserved bits set, pad flag without pad, etc: structurally invalid,
        // and the true frame extent is unknowable.
        None => return Err(DecodeFailure::bare(WalError::CorruptFrame)),
    };
    if env_len < ENVELOPE_HEADER as u64 || env_len > MAX_ENVELOPE_LEN {
        // Bogus length: classify before allocating (FMT-6). A garbage
        // lenField must not drive an OOM-sized allocation, and its claimed
        // extent cannot be trusted.
        return Err(DecodeFailure::bare(WalError::CorruptFrame));
    }
    let frame_len = 8 + env_len + pad as u64;

    let mut env = vec![0u8; env_len as usize + pad];
    match read_exact_or_eof(r, &mut env).map_err(DecodeFailure::bare)? {
        ReadOutcome::Full => {}
        _ => return Err(DecodeFailure { error: WalError::TornRecord, frame_len: Some(frame_len) }),
    }
    env.truncate(env_len as usize);

    let kind = match RecordKind::from_u8(env[0]) {
        Some(k) => k,
        None => {
            return Err(DecodeFailure { error: WalError::CorruptFrame, frame_len: Some(frame_len) })
        }
    };
    let crc = u32::from_le_bytes([env[4], env[5], env[6], env[7]]);
    let data = env[ENVELOPE_HEADER..].to_vec();

    match kind {
        RecordKind::CrcSeed => {
            *state = crc;
        }
        _ => {
            let next = crc32c::crc32c_append(*state, &data);
            if next != crc {
                return Err(DecodeFailure {
                    error: WalError::CrcMismatch { expected: crc, actual: next },
                    frame_len: Some(frame_len),
                });
            }
            *state = next;
        }
    }

    Ok(Decoded::Record(Record { kind, crc, data }, frame_len as usize))
}

/// Returns `(envelope_len, pad)` or `None` when the field is structurally
/// invalid: reserved high bits set without the pad flag, pad flag with a
/// zero pad count, or non-flag garbage in the top byte.
fn split_len_field(len_field: u64) -> Option<(u64, usize)> {
    let top = (len_field >> 56) as u8;
    if top == 0 {
        return Some((len_field, 0));
    }
    if top & 0x80 == 0 {
        return None; // high bits without the pad flag: not something we write
    }
    let pad = (top & 0x7) as usize;
    if pad == 0 || top != 0x80 | pad as u8 {
        return None; // pad flag demands 1..=7, and bits 3..7 must stay clear
    }
    Some((len_field & 0x00FF_FFFF_FFFF_FFFF, pad))
}

enum ReadOutcome {
    Full,
    Partial,
    Eof,
}

fn read_exact_or_eof(r: &mut impl Read, buf: &mut [u8]) -> Result<ReadOutcome, WalError> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => {
                return Ok(if filled == 0 { ReadOutcome::Eof } else { ReadOutcome::Partial });
            }
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(WalError::Io(e)),
        }
    }
    Ok(ReadOutcome::Full)
}

/// Entry payload codec: data = seq:u64-LE | payload.
pub fn encode_entry_data(seq: u64, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + payload.len());
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

pub fn decode_entry_data(data: &[u8]) -> Result<(u64, Vec<u8>), WalError> {
    if data.len() < 8 {
        return Err(WalError::CorruptFrame);
    }
    let seq = u64::from_le_bytes(data[..8].try_into().expect("length checked"));
    Ok((seq, data[8..].to_vec()))
}
