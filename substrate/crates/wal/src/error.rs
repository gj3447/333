//! Typed error surface — etcd's `MustUnmarshal` panic paths deliberately do
//! not exist here (absorption API-15: all corruption is a value, the caller
//! decides whether to die).

use std::fmt;
use std::io;

#[derive(Debug)]
pub enum WalError {
    Io(io::Error),
    /// Another live writer holds the directory lock (single-writer, API-7).
    Locked,
    /// A `*.wal` file name does not parse as `<seq:016x>-<first:016x>.wal`.
    BadSegmentName(String),
    /// Segment sequence numbers are not contiguous (LIF-2).
    SegmentSeqGap { expected: u64, found: u64 },
    /// Entry seq numbers are not strictly increasing by 1.
    EntrySeqGap { expected: u64, found: u64 },
    /// Rolling crc chain broke on a *complete* record — fatal corruption of
    /// the synced region (FMT-8) unless the zero-sector heuristic proves a
    /// torn tail; never silently repaired.
    CrcMismatch { expected: u32, actual: u32 },
    /// Partial frame/envelope at EOF. Repairable only as the tail of the
    /// last segment (torn write); fatal anywhere else (FMT-7/8).
    TornRecord,
    /// A structurally invalid but complete-enough frame: unknown record kind,
    /// bogus length field, reserved bits set. Same fatality rules as
    /// `CrcMismatch` (review finding: must NOT be lumped with `TornRecord`).
    CorruptFrame,
    /// A zero length field before the end of a non-last segment — this port
    /// never preallocates, so zeros can only be a torn tail of the LAST file.
    ZeroLenMidLog,
    /// Payload would exceed the decoder's `MAX_ENVELOPE_LEN` — rejected at
    /// append() time so an unreadable record can never be acked (review
    /// finding: write/read validation must be symmetric).
    RecordTooLarge { len: usize, max: usize },
    /// A previous I/O failure left in-memory state (rolling crc, offsets)
    /// possibly out of sync with disk; the handle refuses all further work
    /// (fsyncgate — a retried fdatasync after EIO can lie).
    Poisoned,
    /// The directory does not look like a WAL (missing/foreign files).
    NotAWal(String),
    /// Metadata record disagrees with the metadata the WAL was opened with.
    MetadataConflict,
}

impl fmt::Display for WalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WalError::Io(e) => write!(f, "wal io: {e}"),
            WalError::Locked => write!(f, "wal directory is locked by another writer"),
            WalError::BadSegmentName(n) => write!(f, "bad wal segment name: {n}"),
            WalError::SegmentSeqGap { expected, found } => {
                write!(f, "segment seq gap: expected {expected}, found {found}")
            }
            WalError::EntrySeqGap { expected, found } => {
                write!(f, "entry seq gap: expected {expected}, found {found}")
            }
            WalError::CrcMismatch { expected, actual } => {
                write!(f, "crc chain mismatch: record says {expected:#010x}, chain says {actual:#010x}")
            }
            WalError::TornRecord => write!(f, "torn/partial record"),
            WalError::CorruptFrame => write!(f, "structurally corrupt record frame"),
            WalError::ZeroLenMidLog => write!(f, "zero length field before end of log"),
            WalError::RecordTooLarge { len, max } => {
                write!(f, "record payload {len} bytes exceeds max {max}")
            }
            WalError::Poisoned => write!(f, "wal handle poisoned by a previous I/O failure"),
            WalError::NotAWal(d) => write!(f, "not a wal directory: {d}"),
            WalError::MetadataConflict => write!(f, "wal metadata conflict"),
        }
    }
}

impl std::error::Error for WalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WalError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for WalError {
    fn from(e: io::Error) -> Self {
        WalError::Io(e)
    }
}
