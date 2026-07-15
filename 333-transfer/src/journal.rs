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
// * Compaction exists (`Journal::compact`) but only cuts the constant, it does
//   NOT bound growth. Measured 2026-07-15: log ~441 B per confirmed transfer,
//   compacted ~53 B — an ~8x cut, both still O(transfers). The residual is
//   `SnapshotData::confirmed`, one `(account, seq, order_id)` per slot ever
//   applied. Collapsing it to O(accounts) is possible — certificate uniqueness
//   means `seq < next_seq` already implies "this is the one we applied" — but
//   that trades away today's detector for a conflicting certificate on a spent
//   slot, so it is a deliberate call, not a cleanup. See
//   `tests/journal_compaction.rs::compaction_cuts_the_constant_but_does_not_bound_growth`.
// * No segment rotation: compaction rewrites the single file whole. Fine while
//   the file is small; a large one makes compaction O(state) and blocking.
// * `FileJournal` is native-only (`std::fs`). wasm32 targets keep `NullJournal`
//   until an IndexedDB-backed port exists.

use std::fmt;

use sha2::{Digest, Sha256};

use crate::owner::SignedTransfer;
use crate::wire::{decode_transfer, encode_transfer, WireError};

const JOURNAL_MAGIC: &[u8] = b"transfer333/journal/v4\0";

/// Header layout: `JOURNAL_MAGIC || identity(32)`.
///
/// The identity block is written by the first `bind` rather than by `open`,
/// because `open` does not know who is opening. A file that stops after the
/// magic is a journal created but never claimed — a fresh log, not a corrupt one.
/// # KG: fix-333-journal-identity-binding-2026-07-15
const IDENTITY_LEN: usize = 32;

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
const TAG_SNAPSHOT: u8 = 3;

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
    /// Complete authority state as of some point. Replaying it *replaces* state
    /// rather than adding to it; see [`SnapshotData`].
    Snapshot(SnapshotData),
}

/// Authority state captured whole, so the log before it can be discarded.
///
/// Why this is a log *frame* and not a sidecar file: compaction replaces the
/// journal by writing a new file and renaming it over the old one, which is
/// atomic. A snapshot in a separate file would create a window where both the
/// snapshot and the un-truncated log exist, and recovery would apply the log's
/// records *on top of* a state that already contains them — silently doubling
/// every transfer. With one file there is no such intermediate state: recovery
/// sees either the old journal or the new one.
///
/// `accounts` carries `next_seq` as well as the balance, which is why
/// [`Ledger::restore`] exists — `try_genesis` can only produce `next_seq = 0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotData {
    /// `(account, balance, next_seq)` for every account.
    pub accounts: Vec<(String, u128, u64)>,
    /// Slots locked but not yet confirmed. These are the live promises — dropping
    /// one reopens the equivocation window this module exists to close.
    pub locked: Vec<SignedTransfer>,
    /// `(account, seq, order_id)` for slots already applied. Retained so a
    /// re-delivered certificate still gets `AlreadyApplied` instead of a
    /// stale-sequence error; gossip re-delivers certificates routinely.
    ///
    /// Only the order id, because that is all the live state holds — snapshotting
    /// whole orders here would invent data the authority does not have and bloat
    /// the file for nothing.
    pub confirmed: Vec<(String, u64, [u8; 32])>,
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
    /// Claim this journal for one authority identity, or verify an existing claim.
    ///
    /// Callers must invoke this before `append` or `replay`; `Authority` does it
    /// in `with_journal`/`recover`. A fresh journal records the identity; an
    /// existing one must match it or the open fails.
    ///
    /// Without this a journal is anonymous, and an anonymous journal is unsafe:
    /// pointed at another authority's file an authority adopts that authority's
    /// locks *and loses its own*, so it will happily re-vote a slot it already
    /// voted on — the very equivocation this module exists to prevent, arrived at
    /// by a config mistake instead of a crash. A mismatched genesis is worse
    /// still: replay re-drives `Ledger::apply` from whatever the caller supplied,
    /// so the wrong genesis silently mints balances.
    /// # KG: fix-333-journal-identity-binding-2026-07-15
    fn bind(&mut self, identity: &[u8; 32]) -> Result<(), JournalError>;

    fn append(&mut self, record: &JournalRecord) -> Result<(), JournalError>;
    /// Every record ever appended, in append order.
    fn replay(&self) -> Result<Vec<JournalRecord>, JournalError>;
    /// Replace the whole log with a single snapshot frame, discarding the
    /// records it subsumes.
    ///
    /// Must be atomic: a crash leaves either the old log or the new one, never a
    /// state where the snapshot and the records it already contains coexist.
    fn compact(&mut self, snapshot: &SnapshotData) -> Result<(), JournalError>;
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
    let (tag, body) = match record {
        JournalRecord::Locked(o) => (TAG_LOCKED, encode_transfer(o)),
        JournalRecord::Confirmed(o) => (TAG_CONFIRMED, encode_transfer(o)),
        JournalRecord::Snapshot(sn) => (TAG_SNAPSHOT, encode_snapshot(sn)),
    };
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
pub fn decode_records(bytes: &[u8]) -> Result<Vec<JournalRecord>, JournalError> {
    Ok(scan(bytes)?.0)
}

/// Byte length of the leading run of complete, checksum-valid frames.
///
/// Everything past it is a torn tail: bytes an `append` was still writing when
/// the machine died, which therefore never became durable and never carried a
/// decision anywhere. Callers truncate there.
///
/// This is deliberately *not* "everything decode_records could not read". A
/// complete frame with a bad checksum is corruption, not a tear — [`scan`]
/// propagates it as an error, because silently cutting there would discard
/// durable records that follow.
pub fn good_prefix_len(bytes: &[u8]) -> Result<usize, JournalError> {
    Ok(scan(bytes)?.1)
}

/// Single parser behind [`decode_records`] and [`good_prefix_len`].
///
/// Returns the records and the offset where scanning stopped — the start of a
/// torn tail, or `bytes.len()` when every byte belonged to a complete frame.
/// Keeping one parser is the point: two would drift, and the whole safety
/// argument rests on "where the log ends" having exactly one answer.
fn scan(bytes: &[u8]) -> Result<(Vec<JournalRecord>, usize), JournalError> {
    let mut out = Vec::new();
    let mut rest = bytes;
    let mut good = 0usize;
    while !rest.is_empty() {
        // Torn header: fewer bytes than one frame header.
        if rest.len() < HEADER_LEN {
            break;
        }
        let tag = rest[0];
        // Zero-filled tail (or any reserved tag): not a record.
        if tag == TAG_TORN {
            break;
        }
        let len = u32::from_be_bytes([rest[1], rest[2], rest[3], rest[4]]) as usize;
        if len > MAX_RECORD_BYTES {
            return Err(JournalError::Corrupt(format!("record too large: {len}")));
        }
        let crc = [rest[5], rest[6], rest[7], rest[8]];
        // Torn body.
        if rest.len() < HEADER_LEN + len {
            break;
        }
        let body = &rest[HEADER_LEN..HEADER_LEN + len];
        if frame_crc(tag, body) != crc {
            return Err(JournalError::Corrupt(
                "frame checksum mismatch: complete frame with bad data".into(),
            ));
        }
        out.push(match tag {
            TAG_LOCKED => JournalRecord::Locked(decode_transfer(body)?),
            TAG_CONFIRMED => JournalRecord::Confirmed(decode_transfer(body)?),
            TAG_SNAPSHOT => JournalRecord::Snapshot(decode_snapshot(body)?),
            other => return Err(JournalError::Corrupt(format!("unknown tag: {other}"))),
        });
        rest = &rest[HEADER_LEN + len..];
        good += HEADER_LEN + len;
    }
    Ok((out, good))
}


// --- snapshot body codec -------------------------------------------------
//
// Hand-rolled, like `wire.rs`: the crate has no serde dependency and a durable
// on-disk format is exactly where an implicit derive would be a liability.
// Layout: u32 counts + length-prefixed items, all big-endian.

fn put_u32(out: &mut Vec<u8>, v: usize) {
    out.extend_from_slice(&(v as u32).to_be_bytes());
}

fn take_u32(bytes: &mut &[u8]) -> Result<usize, JournalError> {
    if bytes.len() < 4 {
        return Err(JournalError::Corrupt("snapshot: truncated u32".into()));
    }
    let v = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    *bytes = &bytes[4..];
    Ok(v)
}

fn take_slice<'a>(bytes: &mut &'a [u8], n: usize) -> Result<&'a [u8], JournalError> {
    if bytes.len() < n {
        return Err(JournalError::Corrupt("snapshot: truncated field".into()));
    }
    let (head, rest) = bytes.split_at(n);
    *bytes = rest;
    Ok(head)
}

pub fn encode_snapshot(sn: &SnapshotData) -> Vec<u8> {
    let mut out = Vec::new();
    put_u32(&mut out, sn.accounts.len());
    for (id, balance, next_seq) in &sn.accounts {
        put_u32(&mut out, id.len());
        out.extend_from_slice(id.as_bytes());
        out.extend_from_slice(&balance.to_be_bytes());
        out.extend_from_slice(&next_seq.to_be_bytes());
    }
    put_u32(&mut out, sn.locked.len());
    for order in &sn.locked {
        let b = encode_transfer(order);
        put_u32(&mut out, b.len());
        out.extend_from_slice(&b);
    }
    put_u32(&mut out, sn.confirmed.len());
    for (account, seq, order_id) in &sn.confirmed {
        put_u32(&mut out, account.len());
        out.extend_from_slice(account.as_bytes());
        out.extend_from_slice(&seq.to_be_bytes());
        out.extend_from_slice(order_id);
    }
    out
}

pub fn decode_snapshot(mut bytes: &[u8]) -> Result<SnapshotData, JournalError> {
    let n = take_u32(&mut bytes)?;
    let mut accounts = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let len = take_u32(&mut bytes)?;
        let id = String::from_utf8(take_slice(&mut bytes, len)?.to_vec())
            .map_err(|_| JournalError::Corrupt("snapshot: account id not utf-8".into()))?;
        let balance = u128::from_be_bytes(
            take_slice(&mut bytes, 16)?
                .try_into()
                .expect("16 bytes checked"),
        );
        let next_seq = u64::from_be_bytes(
            take_slice(&mut bytes, 8)?
                .try_into()
                .expect("8 bytes checked"),
        );
        accounts.push((id, balance, next_seq));
    }
    let n = take_u32(&mut bytes)?;
    let mut locked = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let len = take_u32(&mut bytes)?;
        locked.push(decode_transfer(take_slice(&mut bytes, len)?)?);
    }
    let n = take_u32(&mut bytes)?;
    let mut confirmed = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let len = take_u32(&mut bytes)?;
        let account = String::from_utf8(take_slice(&mut bytes, len)?.to_vec())
            .map_err(|_| JournalError::Corrupt("snapshot: confirmed account not utf-8".into()))?;
        let seq = u64::from_be_bytes(
            take_slice(&mut bytes, 8)?.try_into().expect("8 bytes checked"),
        );
        let order_id: [u8; 32] = take_slice(&mut bytes, 32)?
            .try_into()
            .expect("32 bytes checked");
        confirmed.push((account, seq, order_id));
    }
    if !bytes.is_empty() {
        return Err(JournalError::Corrupt("snapshot: trailing bytes".into()));
    }
    Ok(SnapshotData {
        accounts,
        locked,
        confirmed,
    })
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
    /// Nothing is retained, so there is no file for a different authority to
    /// adopt and nothing to check.
    fn bind(&mut self, _identity: &[u8; 32]) -> Result<(), JournalError> {
        Ok(())
    }
    fn append(&mut self, _record: &JournalRecord) -> Result<(), JournalError> {
        Ok(())
    }
    fn replay(&self) -> Result<Vec<JournalRecord>, JournalError> {
        Ok(Vec::new())
    }
    fn compact(&mut self, _snapshot: &SnapshotData) -> Result<(), JournalError> {
        // Nothing is retained, so there is nothing to compact.
        Ok(())
    }
}

/// In-memory journal that *does* retain records. Not durable across a process,
/// but lets a test drive the recover path without touching a filesystem.
#[derive(Debug, Default, Clone)]
pub struct MemJournal {
    records: Vec<JournalRecord>,
    identity: Option<[u8; 32]>,
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
    /// In-process only, so it cannot be handed to a different authority by a path
    /// mistake — but it is still checked, so tests exercise the same rule the
    /// file journal enforces.
    fn bind(&mut self, identity: &[u8; 32]) -> Result<(), JournalError> {
        match self.identity {
            None => {
                self.identity = Some(*identity);
                Ok(())
            }
            Some(existing) if existing == *identity => Ok(()),
            Some(_) => Err(JournalError::Corrupt(
                "journal belongs to a different authority, committee or genesis".into(),
            )),
        }
    }
    fn append(&mut self, record: &JournalRecord) -> Result<(), JournalError> {
        self.records.push(record.clone());
        Ok(())
    }
    fn replay(&self) -> Result<Vec<JournalRecord>, JournalError> {
        Ok(self.records.clone())
    }
    fn compact(&mut self, snapshot: &SnapshotData) -> Result<(), JournalError> {
        self.records = vec![JournalRecord::Snapshot(snapshot.clone())];
        Ok(())
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
        /// Set by `bind`; needed so `compact` can rewrite the header intact.
        identity: Option<[u8; 32]>,
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
            let me = Self { path, file, identity: None };
            if existed {
                me.truncate_torn_tail()?;
            }
            Ok(me)
        }

        /// Cut a torn tail off before anything can append past it.
        ///
        /// Replay stops at a tear; `append` writes at EOF. Left alone the two
        /// disagree about where the log ends, and a record written after a tear
        /// is durable but unreachable forever. For a `Locked` record that is the
        /// crash-restart equivocation this journal exists to prevent: the
        /// authority forgets it voted and signs a second order for the slot.
        /// Truncating here gives "the end of the log" one answer.
        ///
        /// Only a tear is cut. A complete frame with a bad checksum is
        /// corruption — [`good_prefix_len`] raises it rather than letting us
        /// discard durable records behind it.
        ///
        /// Idempotent, and safe to lose: a crash before the new length is
        /// durable just leaves the tear for the next open to cut again.
        fn truncate_torn_tail(&self) -> Result<(), JournalError> {
            let body = self.read_body()?;
            // A corrupt log is `recover`'s to report, not `open`'s. Swallowing the
            // error here keeps `open`'s contract (it validates the header, not the
            // whole log) and leaves the bytes untouched for the replay to fail
            // closed on, exactly as before. We are only here to cut a tear.
            let Ok(good) = good_prefix_len(&body) else {
                return Ok(());
            };
            if good == body.len() {
                return Ok(());
            }
            let keep = (JOURNAL_MAGIC.len() + IDENTITY_LEN + good) as u64;
            self.file
                .set_len(keep)
                .map_err(|e| JournalError::Io(format!("truncate torn tail: {e}")))?;
            // The size change is metadata, so sync_all rather than sync_data.
            self.file
                .sync_all()
                .map_err(|e| JournalError::Io(format!("sync truncation: {e}")))?;
            Ok(())
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
            // Records start after the identity block. A file that stops inside or
            // before it was created but never claimed, so it holds no records.
            let start = JOURNAL_MAGIC.len() + IDENTITY_LEN;
            if all.len() < start {
                return Ok(Vec::new());
            }
            Ok(all[start..].to_vec())
        }

        /// The identity this file is claimed by, or `None` if only the magic is
        /// present (created, never bound). `Err` if the block is half-written —
        /// a torn identity cannot be compared, so it must not be guessed.
        fn read_identity(&self) -> Result<Option<[u8; 32]>, JournalError> {
            let mut f = File::open(&self.path)
                .map_err(|e| JournalError::Io(format!("read {}: {e}", self.path.display())))?;
            let mut all = Vec::new();
            f.read_to_end(&mut all)
                .map_err(|e| JournalError::Io(format!("read_to_end: {e}")))?;
            let start = JOURNAL_MAGIC.len();
            match all.len() {
                n if n == start => Ok(None),
                n if n >= start + IDENTITY_LEN => {
                    let mut id = [0u8; IDENTITY_LEN];
                    id.copy_from_slice(&all[start..start + IDENTITY_LEN]);
                    Ok(Some(id))
                }
                _ => Err(JournalError::Corrupt("torn identity block".into())),
            }
        }
    }

    impl Journal for FileJournal {
        fn bind(&mut self, identity: &[u8; 32]) -> Result<(), JournalError> {
            match self.read_identity()? {
                Some(existing) if existing == *identity => {}
                Some(_) => {
                    return Err(JournalError::Corrupt(
                        "journal belongs to a different authority, committee or genesis".into(),
                    ))
                }
                None => {
                    // Fresh file: claim it. Durable before any record follows, so a
                    // crash cannot leave records under an unrecorded identity.
                    self.file
                        .write_all(identity)
                        .map_err(|e| JournalError::Io(format!("write identity: {e}")))?;
                    self.file
                        .sync_data()
                        .map_err(|e| JournalError::Io(format!("sync identity: {e}")))?;
                }
            }
            self.identity = Some(*identity);
            Ok(())
        }

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

        /// Rewrite the journal as `magic || snapshot`, atomically.
        ///
        /// The temp-file-then-rename dance is the whole crash-safety argument:
        /// `rename(2)` within a directory is atomic, so a power loss leaves either
        /// the complete old journal or the complete new one. Truncating in place
        /// would expose a window where the file holds a snapshot *and* the records
        /// it already accounts for, and recovery would apply those records on top
        /// of a state that contains them — doubling every transfer in the window.
        ///
        /// The tmp file is fsynced before the rename so the rename cannot publish
        /// a name that points at unwritten blocks. The directory is fsynced after,
        /// so the rename itself survives.
        fn compact(&mut self, snapshot: &SnapshotData) -> Result<(), JournalError> {
            let tmp = self.path.with_extension("compact-tmp");
            {
                let mut f = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&tmp)
                    .map_err(|e| JournalError::Io(format!("open tmp: {e}")))?;
                f.write_all(JOURNAL_MAGIC)
                    .map_err(|e| JournalError::Io(format!("tmp magic: {e}")))?;
                // The compacted file must carry the same claim, or the next open
                // would see an unclaimed journal and adopt whoever asked first.
                let id = self
                    .identity
                    .ok_or_else(|| JournalError::Corrupt("compact before bind".into()))?;
                f.write_all(&id)
                    .map_err(|e| JournalError::Io(format!("tmp identity: {e}")))?;
                f.write_all(&encode_record(&JournalRecord::Snapshot(snapshot.clone())))
                    .map_err(|e| JournalError::Io(format!("tmp snapshot: {e}")))?;
                f.sync_all()
                    .map_err(|e| JournalError::Io(format!("sync tmp: {e}")))?;
            }
            std::fs::rename(&tmp, &self.path)
                .map_err(|e| JournalError::Io(format!("rename compacted journal: {e}")))?;
            if let Some(dir) = self.path.parent() {
                // Best-effort: a missing dir fsync loses the rename on a power cut,
                // but the old journal is still intact in that case, so recovery is
                // correct either way. Not worth failing compaction over.
                if let Ok(d) = File::open(dir) {
                    let _ = d.sync_all();
                }
            }
            // The old handle points at the unlinked inode; reopen for appends.
            self.file = OpenOptions::new()
                .create(true)
                .read(true)
                .append(true)
                .open(&self.path)
                .map_err(|e| JournalError::Io(format!("reopen after compact: {e}")))?;
            Ok(())
        }
    }
}
