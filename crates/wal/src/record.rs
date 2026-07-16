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

use std::io::{self, Read};

use crate::error::WalError;

/// Max envelope length accepted before allocating (FMT-6 sanity gate).
/// etcd bounds records at ~10MB per entry batch; we keep the same order.
pub const MAX_ENVELOPE_LEN: u64 = 16 * 1024 * 1024;

const ENVELOPE_HEADER: usize = 8; // kind(1) + reserved(3) + crc(4)

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
    /// A complete, crc-consistent record.
    Record(Record, usize),
    /// lenField == 0 → valid end of data (prealloc/zero-fill contract, FMT-5).
    ZeroLen,
    /// Clean EOF exactly on a frame boundary.
    Eof,
}

/// Decode one frame from `r`, verifying the rolling crc chain.
///
/// `state` is advanced for data-bearing records; a `CrcSeed` *resets* it to
/// the stored value (segment handoff, FMT-4).
///
/// Errors:
/// - `WalError::TornRecord` — partial frame/envelope (candidate torn tail; the
///   caller decides repairability by file position, FMT-7/8).
/// - `WalError::CrcMismatch` — chain broke on a complete record (fatal unless
///   the caller's zero-sector heuristic proves a torn tail).
pub fn decode_record(state: &mut u32, r: &mut impl Read) -> Result<Decoded, WalError> {
    let mut len_buf = [0u8; 8];
    match read_exact_or_eof(r, &mut len_buf)? {
        ReadOutcome::Eof => return Ok(Decoded::Eof),
        ReadOutcome::Partial => return Err(WalError::TornRecord),
        ReadOutcome::Full => {}
    }
    let len_field = u64::from_le_bytes(len_buf);
    if len_field == 0 {
        return Ok(Decoded::ZeroLen);
    }

    let (env_len, pad) = split_len_field(len_field)?;
    if env_len < ENVELOPE_HEADER as u64 || env_len > MAX_ENVELOPE_LEN {
        // Bogus length: classify before allocating (FMT-6). A torn/garbage
        // lenField must not drive an OOM-sized allocation.
        return Err(WalError::TornRecord);
    }

    let mut env = vec![0u8; env_len as usize + pad];
    match read_exact_or_eof(r, &mut env)? {
        ReadOutcome::Full => {}
        _ => return Err(WalError::TornRecord),
    }
    env.truncate(env_len as usize);

    let kind = RecordKind::from_u8(env[0]).ok_or(WalError::TornRecord)?;
    let crc = u32::from_le_bytes([env[4], env[5], env[6], env[7]]);
    let data = env[ENVELOPE_HEADER..].to_vec();

    match kind {
        RecordKind::CrcSeed => {
            // Handoff gate: the seed must equal the chain state we arrived
            // with, except at the very first record where the reader adopts it.
            *state = crc;
        }
        _ => {
            let next = crc32c::crc32c_append(*state, &data);
            if next != crc {
                return Err(WalError::CrcMismatch { expected: crc, actual: next });
            }
            *state = next;
        }
    }

    let consumed = 8 + env_len as usize + pad;
    Ok(Decoded::Record(Record { kind, crc, data }, consumed))
}

fn split_len_field(len_field: u64) -> Result<(u64, usize), WalError> {
    if len_field & (1 << 63) != 0 {
        let pad = ((len_field >> 56) & 0x7) as usize;
        Ok((len_field & 0x00FF_FFFF_FFFF_FFFF, pad))
    } else {
        Ok((len_field, 0))
    }
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
        return Err(WalError::TornRecord);
    }
    let seq = u64::from_le_bytes(data[..8].try_into().expect("length checked"));
    Ok((seq, data[8..].to_vec()))
}
