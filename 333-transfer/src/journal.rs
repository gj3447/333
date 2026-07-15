// KG: audit-333-fsm-vs-borg-k8s-2026-07-15 (P0: durable 0/26 FSM),
//     omc-open-problems-4track-2026-07-15 (Track B 판정 전제 보정),
//     prom16-333-consensusless-frontier (C3 quorum-certificate uniqueness)
//
// Write-ahead durability for the authority slot decision.
//
// WHY THIS EXISTS
// ---------------
// The FastPay safety argument rests on one invariant: an honest authority never
// signs two different orders for the same `(account, sequence)` slot. That
// invariant lived only in `Authority::locked`, an in-memory `HashMap`. A restart
// therefore forgot every lock, and an honest crash-restart became indistinguishable
// from Byzantine equivocation: the authority would happily sign a *second*,
// conflicting order for a slot it had already voted on. One honest restart spent
// one unit of the Byzantine budget `f`; more than `f` restarts across a slot's
// voters break certificate uniqueness outright.
//
// etcd states the contract this module restores: persist before you act
// (`etcd-io/raft` doc.go, Usage 1-4 — the caller must write entries/state to
// stable storage *before* sending messages). `Journal::append` returning `Ok`
// means the record is durable; only then may a vote leave the process.
//
// SCOPE / NON-GOALS
// -----------------
// * No wall-clock, no tokio, no async. Same determinism discipline as the rest of
//   the crate — replay is a pure fold over an append-only log.
// * `NullJournal` is the default and preserves the pre-existing (non-durable)
//   behaviour exactly, so the existing suite is an unchanged regression baseline.
//   Durability is opt-in via `Authority::with_journal` / `Authority::recover`.
// * Not a general log: no compaction, no snapshot, no segment rotation. A
//   confirmed slot's records stay in the log forever. Bounded by transfer volume,
//   not by time. Snapshotting is left open.
// * `FileJournal` is native-only (`std::fs`). wasm32 targets keep `NullJournal`
//   until an IndexedDB-backed port exists.

use std::fmt;

use sha2::{Digest, Sha256};

use crate::owner::SignedTransfer;
use crate::wire::{decode_transfer, encode_transfer, WireError};

const JOURNAL_MAGIC: &[u8] = b"transfer333/journal/v2\0";

/// Tag 0 is reserved and never written.
///
/// A power loss can extend a file's length while the data blocks never land
/// (ext4 `data=ordered`), leaving a run of zeros past the last good record. With
/// a zero tag that tail would parse as a well-formed zero-length frame — a
/// phantom record invented by the crash. Reserving 0 makes a zero-filled tail
/// self-identifying as "not a record".
const TAG_TORN: u8 = 0;
const TAG_LOCKED: u8 = 1;
const TAG_CONFIRMED: u8 = 2;

/// `tag(1) || len(4, BE) || crc(4, BE)`.
const HEADER_LEN: usize = 9;

/// Upper bound on one encoded record, mirroring the wire framing guard. A
/// corrupt or hostile length prefix must not drive an unbounded allocation.
const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;

/// One durable authority decision.
///
/// Both variants carry the full `SignedTransfer` rather than a digest: replay
/// must be able to rebuild the lock table *and* re-drive `Ledger::apply` from the
/// canonical genesis without consulting any other source. The slot and order id
/// are derived, never stored, so a record cannot disagree with itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalRecord {
    /// This authority installed a lock and is about to sign. Written *before* the
    /// vote is produced.
    Locked(SignedTransfer),
    /// This authority applied a quorum certificate for the slot.
    Confirmed(SignedTransfer),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalError {
    /// The underlying store rejected the write, or the durability barrier failed.
    /// The authority must fail-stop: its in-memory state is no longer backed.
    Io(String),
    /// The log is unreadable: truncated frame, bad magic, unknown tag, or a
    /// record body that does not decode.
    Corrupt(String),
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JournalError::Io(m) => write!(f, "journal io: {m}"),
            JournalError::Corrupt(m) => write!(f, "journal corrupt: {m}"),
        }
    }
}

impl std::error::Error for JournalError {}

impl From<WireError> for JournalError {
    fn from(e: WireError) -> Self {
        JournalError::Corrupt(format!("record body: {e:?}"))
    }
}

/// Append-only durability port.
///
/// Contract: when `append` returns `Ok`, the record has reached stable storage.
/// An implementation that buffers without a durability barrier silently
/// reintroduces the equivocation bug this module exists to close.
pub trait Journal: fmt::Debug + Send {
    fn append(&mut self, record: &JournalRecord) -> Result<(), JournalError>;
    /// Every record ever appended, in append order.
    fn replay(&self) -> Result<Vec<JournalRecord>, JournalError>;
}

/// Truncated SHA-256 over `tag || len || body`. `sha2` is already a dependency;
/// four bytes is enough to catch a torn or rotted frame, which is all this needs
/// to do — it is an integrity check, not an authentication tag (the body carries
/// its own Ed25519 owner signature).
fn frame_crc(tag: u8, body: &[u8]) -> [u8; 4] {
    let mut d = Sha256::new();
    d.update(JOURNAL_MAGIC);
    d.update([tag]);
    d.update((body.len() as u32).to_be_bytes());
    d.update(body);
    let full = d.finalize();
    [full[0], full[1], full[2], full[3]]
}

/// Encode one record as `tag || u32-BE len || crc(4) || encode_transfer(order)`.
pub fn encode_record(record: &JournalRecord) -> Vec<u8> {
    let (tag, order) = match record {
        JournalRecord::Locked(o) => (TAG_LOCKED, o),
        JournalRecord::Confirmed(o) => (TAG_CONFIRMED, o),
    };
    let body = encode_transfer(order);
    let crc = frame_crc(tag, &body);
    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    out.push(tag);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&crc);
    out.extend_from_slice(&body);
    out
}

/// Decode a full log body (after the magic header) into records.
///
/// Tear vs corruption, following etcd's WAL stance:
///
/// * An **incomplete** trailing frame — short header, short body, or the
///   reserved zero tag from a zero-filled tail — is a *tear*. `append` fsyncs
///   every record, so only the last frame can be partial: it never became
///   durable, so the decision it carried never became externally visible.
///   Recovery stops cleanly at that point.
/// * A **complete** frame whose CRC does not match is *corruption*, not a tear,
///   and is a hard error. Silently truncating there would drop durable locks —
///   precisely the equivocation this module exists to prevent — so it fails
///   closed and lets an operator look.
///
/// Known limitation: without a chained hash this cannot detect corruption that
/// destroys a frame *and* everything after it. Append-plus-fsync makes that
/// shape unreachable in practice, but it is not proven away.
pub fn decode_records(mut bytes: &[u8]) -> Result<Vec<JournalRecord>, JournalError> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        // Torn header: fewer bytes than one frame header.
        if bytes.len() < HEADER_LEN {
            break;
        }
        let tag = bytes[0];
        // Zero-filled tail (or any reserved tag): not a record.
        if tag == TAG_TORN {
            break;
        }
        let len = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
        if len > MAX_RECORD_BYTES {
            return Err(JournalError::Corrupt(format!("record too large: {len}")));
        }
        let crc = [bytes[5], bytes[6], bytes[7], bytes[8]];
        // Torn body.
        if bytes.len() < HEADER_LEN + len {
            break;
        }
        let body = &bytes[HEADER_LEN..HEADER_LEN + len];
        if frame_crc(tag, body) != crc {
            return Err(JournalError::Corrupt(
                "frame checksum mismatch: complete frame with bad data".into(),
            ));
        }
        let order = decode_transfer(body)?;
        out.push(match tag {
            TAG_LOCKED => JournalRecord::Locked(order),
            TAG_CONFIRMED => JournalRecord::Confirmed(order),
            other => return Err(JournalError::Corrupt(format!("unknown tag: {other}"))),
        });
        bytes = &bytes[HEADER_LEN + len..];
    }
    Ok(out)
}

/// Non-durable journal. Preserves the crate's original behaviour: locks live only
/// in memory and a restart forgets them. Correct only for tests and for a
/// deployment that has accepted the honest-crash-equals-equivocation risk.
#[derive(Debug, Default, Clone)]
pub struct NullJournal;

impl NullJournal {
    pub fn new() -> Self {
        Self
    }
}

impl Journal for NullJournal {
    fn append(&mut self, _record: &JournalRecord) -> Result<(), JournalError> {
        Ok(())
    }
    fn replay(&self) -> Result<Vec<JournalRecord>, JournalError> {
        Ok(Vec::new())
    }
}

/// In-memory journal that *does* retain records. Not durable across a process,
/// but lets a test drive the recover path without touching a filesystem.
#[derive(Debug, Default, Clone)]
pub struct MemJournal {
    records: Vec<JournalRecord>,
}

impl MemJournal {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

impl Journal for MemJournal {
    fn append(&mut self, record: &JournalRecord) -> Result<(), JournalError> {
        self.records.push(record.clone());
        Ok(())
    }
    fn replay(&self) -> Result<Vec<JournalRecord>, JournalError> {
        Ok(self.records.clone())
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use file::FileJournal;

#[cfg(not(target_arch = "wasm32"))]
mod file {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};

    /// Append-only file journal. Every `append` writes the frame and then issues a
    /// durability barrier (`sync_data`) before returning `Ok`.
    #[derive(Debug)]
    pub struct FileJournal {
        path: PathBuf,
        file: File,
    }

    impl FileJournal {
        /// Open or create the log at `path`. A new file gets the magic header; an
        /// existing file must start with it.
        pub fn open(path: impl AsRef<Path>) -> Result<Self, JournalError> {
            let path = path.as_ref().to_path_buf();
            let existed = path.exists();
            let mut file = OpenOptions::new()
                .create(true)
                .read(true)
                .append(true)
                .open(&path)
                .map_err(|e| JournalError::Io(format!("open {}: {e}", path.display())))?;
            if existed {
                Self::check_magic(&path)?;
            } else {
                file.write_all(JOURNAL_MAGIC)
                    .map_err(|e| JournalError::Io(format!("write magic: {e}")))?;
                file.sync_data()
                    .map_err(|e| JournalError::Io(format!("sync magic: {e}")))?;
            }
            Ok(Self { path, file })
        }

        pub fn path(&self) -> &Path {
            &self.path
        }

        fn check_magic(path: &Path) -> Result<(), JournalError> {
            let mut f =
                File::open(path).map_err(|e| JournalError::Io(format!("reopen: {e}")))?;
            let mut head = vec![0u8; JOURNAL_MAGIC.len()];
            f.read_exact(&mut head)
                .map_err(|e| JournalError::Corrupt(format!("missing magic: {e}")))?;
            if head != JOURNAL_MAGIC {
                return Err(JournalError::Corrupt("bad magic".into()));
            }
            Ok(())
        }

        fn read_body(&self) -> Result<Vec<u8>, JournalError> {
            let mut f = File::open(&self.path)
                .map_err(|e| JournalError::Io(format!("read {}: {e}", self.path.display())))?;
            let mut all = Vec::new();
            f.read_to_end(&mut all)
                .map_err(|e| JournalError::Io(format!("read_to_end: {e}")))?;
            if all.len() < JOURNAL_MAGIC.len() || &all[..JOURNAL_MAGIC.len()] != JOURNAL_MAGIC {
                return Err(JournalError::Corrupt("bad magic".into()));
            }
            Ok(all[JOURNAL_MAGIC.len()..].to_vec())
        }
    }

    impl Journal for FileJournal {
        fn append(&mut self, record: &JournalRecord) -> Result<(), JournalError> {
            let frame = encode_record(record);
            self.file
                .write_all(&frame)
                .map_err(|e| JournalError::Io(format!("append: {e}")))?;
            // The durability barrier IS the contract. Without it `Ok` would be a
            // lie and the equivocation window reopens.
            self.file
                .sync_data()
                .map_err(|e| JournalError::Io(format!("sync: {e}")))?;
            Ok(())
        }

        fn replay(&self) -> Result<Vec<JournalRecord>, JournalError> {
            decode_records(&self.read_body()?)
        }
    }
}
