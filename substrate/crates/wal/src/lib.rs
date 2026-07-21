//! p333-wal — durability substrate for the 333 v2 platform.
//!
//! *Design-invariant absorption* from etcd `server/storage/wal` @6006f405
//! (KG: absorption-etcd-wal-invariants-2026-07-17, doc:
//! THEORY/ETCD_WAL_ABSORPTION/ in SYMPOSIUM). Not a code port: the record
//! format, rolling crc chain, torn-tail recovery, and the ack boundary are
//! kept; the raft coupling (HardState headers, entry override) is removed —
//! records are flat `(seq, payload)`.
//!
//! ## The one contract that matters (DUR-1 / API-3)
//!
//! `append()` promises nothing. **`sync()` returning `Ok(DurableReceipt)` is
//! the ack boundary**: every record appended before it is on stable storage
//! (fdatasync on Linux; on apple targets std's `sync_data` issues
//! F_FULLFSYNC, matching etcd's darwin behavior). Nothing may be externalized
//! — signed, answered, forwarded — before holding the receipt (DUR-6:
//! p333 has no consensus layer behind it, so it is always sync-then-send;
//! etcd's leader-parallel optimization is deliberately not ported).
//!
//! After ANY I/O failure in `append`/`sync`/segment cut, the handle is
//! **poisoned** and refuses further work: a Linux fdatasync error can mark
//! dirty pages clean, so a retried sync would hand out a receipt for data
//! that never reached disk (fsyncgate — etcd panics here for the same
//! reason; we return [`WalError::Poisoned`] instead and the caller decides).
//!
//! ## Recovery model (DUR-8)
//!
//! Reopening a WAL can lose **only the un-synced suffix**. A torn tail is
//! repaired by truncation only when the evidence says "torn": genuinely
//! partial bytes at EOF, or a failing record whose own extent contains an
//! all-zero disk sector (etcd's `isTornEntry`, FMT-7 — the scan covers the
//! failing record only, on absolute 512-byte sector boundaries, so zeros
//! inside *later intact records* can never launder real corruption). A
//! broken crc chain or structurally corrupt frame in the synced region is
//! fatal (`CrcMismatch` / `CorruptFrame`, FMT-8) and never auto-repaired.
//!
//! ## Mode FSM (API-1)
//!
//! There is no way to obtain an appendable [`Wal`] without replaying the
//! whole log: [`Wal::open`] *is* read-all, and returns the entries together
//! with the appendable handle. (etcd expresses this as "must ReadAll before
//! append"; we make the illegal state unrepresentable.)
//!
//! Deliberate v1 deltas from etcd (recorded in the absorption doc §3):
//! no HardState in segment headers, no preallocation/filePipeline (torn-tail
//! detection does not depend on it), single directory-level `LOCK` flock
//! instead of per-segment locks (via std `File::try_lock`), snapshots
//! deferred. Retention/maintenance surface: [`Wal::release_older`] (sealed
//! segments fully `<= upto`), read-only [`verify`], and [`repair`] with a
//! mandatory full backup (FMT-10).
#![cfg(unix)]

mod error;
mod record;

use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub use error::WalError;
use record::{Decoded, DecodeFailure, RecordKind, ENVELOPE_HEADER, MAX_ENVELOPE_LEN};

/// Default segment roll-over size, same order as etcd's SegmentSizeBytes.
pub const DEFAULT_SEGMENT_SIZE: u64 = 64 * 1024 * 1024;

/// Largest payload `append()` accepts — chosen so the decoder's
/// [`MAX_ENVELOPE_LEN`] gate can never reject a record that was acked
/// (write/read symmetry; review finding #0).
pub const MAX_PAYLOAD: usize = MAX_ENVELOPE_LEN as usize - ENVELOPE_HEADER - 8;

const LOCK_FILE: &str = "LOCK";
/// Zero-sector scan cap when a garbage length field hides the frame extent.
const UNKNOWN_EXTENT_SCAN: u64 = 64 * 1024;

#[derive(Debug, Clone)]
pub struct WalOptions {
    /// Roll to a new segment once the current one exceeds this many bytes.
    pub segment_size: u64,
}

impl Default for WalOptions {
    fn default() -> Self {
        WalOptions { segment_size: DEFAULT_SEGMENT_SIZE }
    }
}

/// A recovered record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub seq: u64,
    pub payload: Vec<u8>,
}

/// Proof that everything up to `last_seq` is on stable storage.
///
/// Externalization APIs should demand this token (typestate form of the
/// DUR-6 contract): a function that signs/answers/forwards takes a
/// `&DurableReceipt` whose `last_seq` covers the state it externalizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurableReceipt {
    pub last_seq: u64,
}

/// Append-mode WAL. Obtainable only through [`Wal::create`] (fresh) or
/// [`Wal::open`] (full replay) — see the mode FSM note above.
pub struct Wal {
    dir: PathBuf,
    dir_file: File,
    _lock: File,
    metadata: Vec<u8>,
    tail: File,
    tail_offset: u64,
    crc: u32,
    segment_seq: u64,
    next_seq: u64,
    last_synced: u64,
    dirty: bool,
    poisoned: bool,
    opts: WalOptions,
}

impl Wal {
    /// Create a fresh WAL directory. Fails if `dir` already exists.
    ///
    /// Atomicity (LIF-1/API-2): the directory is fully built under a
    /// `<name>.waltmp` sibling — initial segment with CrcSeed(0)+Metadata
    /// already fsynced — then renamed into place, and the parent directory
    /// is fsynced. A crash anywhere before the rename leaves only a tmp
    /// husk, never a half-initialized WAL.
    pub fn create(dir: &Path, metadata: &[u8], opts: WalOptions) -> Result<Wal, WalError> {
        if dir.exists() {
            return Err(WalError::NotAWal(format!("{} already exists", dir.display())));
        }
        let parent = dir.parent().ok_or_else(|| WalError::NotAWal("no parent dir".into()))?;
        let dir_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| WalError::NotAWal("unusable dir name".into()))?;
        // NOT with_extension(): that would *replace* an existing extension
        // ("wal.v2" → "wal.waltmp") and collide across sibling WALs.
        let tmp = parent.join(format!("{dir_name}.waltmp"));
        if tmp.exists() {
            fs::remove_dir_all(&tmp)?;
        }
        fs::create_dir(&tmp)?;

        // Lock file exists from birth so open() always finds it.
        let lock = File::create(tmp.join(LOCK_FILE))?;
        lock_exclusive(&lock)?;

        let seg_path = tmp.join(segment_name(0, 0));
        let mut seg = OpenOptions::new().create_new(true).read(true).write(true).open(&seg_path)?;
        let mut crc = 0u32;
        let mut buf = Vec::new();
        record::encode_record(&mut crc, RecordKind::CrcSeed, &[], &mut buf);
        record::encode_record(&mut crc, RecordKind::Metadata, metadata, &mut buf);
        seg.write_all(&buf)?;
        seg.sync_data()?;

        fs::rename(&tmp, dir)?;
        let dir_file = File::open(dir)?;
        dir_file.sync_all()?; // fsync the WAL dir itself
        File::open(parent)?.sync_all()?; // …and the parent that saw the rename (DUR-3)

        Ok(Wal {
            dir: dir.to_path_buf(),
            dir_file,
            _lock: lock,
            metadata: metadata.to_vec(),
            tail_offset: buf.len() as u64,
            tail: seg,
            crc,
            segment_seq: 0,
            next_seq: 1,
            last_synced: 0,
            dirty: false,
            poisoned: false,
            opts,
        })
    }

    /// Open an existing WAL: replay everything, repair a proven torn tail,
    /// and return the surviving entries plus the appendable handle
    /// positioned after the last valid record.
    pub fn open(dir: &Path, metadata: &[u8], opts: WalOptions) -> Result<(Vec<Entry>, Wal), WalError> {
        let lock = File::options()
            .read(true)
            .write(true)
            .open(dir.join(LOCK_FILE))
            .map_err(|_| WalError::NotAWal(format!("{} has no LOCK file", dir.display())))?;
        lock_exclusive(&lock)?;

        let segs = list_segments(dir)?;

        let mut entries: Vec<Entry> = Vec::new();
        let mut crc = 0u32;
        let mut tail: Option<(File, u64)> = None;
        let last_idx = segs.len() - 1;

        for (i, (_seq, _first, path)) in segs.iter().enumerate() {
            let is_last = i == last_idx;
            let mut f = OpenOptions::new().read(true).write(true).open(path)?;
            let scan = scan_segment(&mut f, i, is_last, metadata, &mut crc, &mut entries)?;

            if is_last {
                // Drop the un-synced/torn suffix and persist the truncation —
                // sync_all: the size change is metadata (absorption DUR-8 note).
                f.set_len(scan.valid_off)?;
                f.sync_all()?;
                f.seek(SeekFrom::Start(scan.valid_off))?;
                tail = Some((f, scan.valid_off));
            }
        }

        let (tail, tail_offset) = tail.expect("segments verified non-empty");
        let dir_file = File::open(dir)?;
        let next_seq = entries.last().map(|e| e.seq + 1).unwrap_or(1);
        let last_synced = next_seq - 1; // everything replayed is durable by definition

        Ok((
            entries,
            Wal {
                dir: dir.to_path_buf(),
                dir_file,
                _lock: lock,
                metadata: metadata.to_vec(),
                tail,
                tail_offset,
                crc,
                segment_seq: segs[last_idx].0,
                next_seq,
                last_synced,
                dirty: false,
                poisoned: false,
                opts,
            },
        ))
    }

    /// Append one record. **No durability claim** — the record is in the OS
    /// page cache at best until [`Wal::sync`] returns.
    ///
    /// Oversized payloads are rejected up front (`RecordTooLarge`, no state
    /// touched, handle stays usable). Any actual I/O failure poisons the
    /// handle: the in-memory crc chain may no longer match the file.
    pub fn append(&mut self, payload: &[u8]) -> Result<u64, WalError> {
        self.guard()?;
        if payload.len() > MAX_PAYLOAD {
            return Err(WalError::RecordTooLarge { len: payload.len(), max: MAX_PAYLOAD });
        }
        match self.append_inner(payload) {
            Ok(seq) => Ok(seq),
            Err(e) => {
                self.poisoned = true;
                Err(e)
            }
        }
    }

    fn append_inner(&mut self, payload: &[u8]) -> Result<u64, WalError> {
        let seq = self.next_seq;
        let data = record::encode_entry_data(seq, payload);
        let mut buf = Vec::with_capacity(16 + data.len() + 8);
        record::encode_record(&mut self.crc, RecordKind::Entry, &data, &mut buf);
        self.tail.write_all(&buf)?;
        self.tail_offset += buf.len() as u64;
        self.next_seq = seq + 1;
        self.dirty = true;
        if self.tail_offset >= self.opts.segment_size {
            self.cut()?; // implies a full sync of the old tail (DUR-4)
        }
        Ok(seq)
    }

    /// The ack boundary: fdatasync the tail. Everything appended so far is on
    /// stable storage when this returns Ok (DUR-1). A no-op (receipt only)
    /// when nothing was appended since the last sync.
    ///
    /// On failure the handle is poisoned permanently — see the fsyncgate
    /// note in the crate docs.
    pub fn sync(&mut self) -> Result<DurableReceipt, WalError> {
        self.guard()?;
        if !self.dirty {
            return Ok(DurableReceipt { last_seq: self.last_synced });
        }
        if let Err(e) = self.tail.sync_data() {
            self.poisoned = true;
            return Err(WalError::Io(e));
        }
        self.dirty = false;
        self.last_synced = self.next_seq - 1;
        Ok(DurableReceipt { last_seq: self.last_synced })
    }

    /// Highest seq known durable (0 = none yet).
    pub fn last_synced(&self) -> u64 {
        self.last_synced
    }

    /// Explicit close = final sync. (Drop also tries, but cannot report.)
    pub fn close(mut self) -> Result<DurableReceipt, WalError> {
        self.sync()
    }

    /// Release sealed segments whose entries are ALL `<= upto` (LIF-5/API-9).
    /// Returns how many segment files were removed.
    ///
    /// The active tail segment is never removed, and a segment survives as
    /// long as it holds even one entry `> upto` — so a reopen after release
    /// replays a contiguous run that still covers everything above `upto`.
    /// Deletions go oldest-first with a directory fsync after each removal:
    /// a crash at any point leaves a contiguous (possibly longer) suffix,
    /// never a gapped one. Snapshot pinning is deferred with snapshots
    /// themselves (v1 delta): the caller's `upto` is the retention contract.
    pub fn release_older(&mut self, upto: u64) -> Result<usize, WalError> {
        self.guard()?;
        let segs = list_segments(&self.dir)?;
        let mut removed = 0usize;
        for i in 0..segs.len().saturating_sub(1) {
            // Segment i's entries end right before the next segment's first
            // seq (the `<seq>-<first>.wal` name is authoritative, FMT-9).
            let next_first = segs[i + 1].1;
            if next_first > upto.saturating_add(1) {
                break; // this segment still holds live entries — stop (contiguity)
            }
            fs::remove_file(&segs[i].2)?;
            self.dir_file.sync_all()?;
            removed += 1;
        }
        Ok(removed)
    }

    fn guard(&self) -> Result<(), WalError> {
        if self.poisoned {
            return Err(WalError::Poisoned);
        }
        Ok(())
    }

    /// Roll to a new segment (LIF-3 / API-8, inline — no filePipeline):
    /// finish + fsync the old tail, build the new segment **fully in a .tmp
    /// file** (CrcSeed handoff + Metadata, fsynced), then rename + dir fsync.
    /// A segment file is never visible under its `.wal` name without a valid
    /// header.
    fn cut(&mut self) -> Result<(), WalError> {
        self.tail.sync_data()?;
        self.dirty = false;
        self.last_synced = self.next_seq - 1;

        let new_seq = self.segment_seq + 1;
        let final_name = segment_name(new_seq, self.next_seq);
        let tmp_path = self.dir.join(format!("{final_name}.tmp"));
        let final_path = self.dir.join(&final_name);

        let mut seg = OpenOptions::new().create_new(true).read(true).write(true).open(&tmp_path)?;
        let mut buf = Vec::new();
        record::encode_record(&mut self.crc, RecordKind::CrcSeed, &[], &mut buf);
        record::encode_record(&mut self.crc, RecordKind::Metadata, &self.metadata, &mut buf);
        seg.write_all(&buf)?;
        seg.sync_data()?;
        fs::rename(&tmp_path, &final_path)?;
        self.dir_file.sync_all()?;

        self.tail = seg;
        self.tail_offset = buf.len() as u64;
        self.segment_seq = new_seq;
        Ok(())
    }
}

impl Drop for Wal {
    fn drop(&mut self) {
        if !self.poisoned {
            let _ = self.tail.sync_data();
        }
    }
}

/// What a read-only [`verify`] pass found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub segments: usize,
    pub entries: usize,
    /// 0 when the log holds no entries.
    pub first_seq: u64,
    /// 0 when the log holds no entries.
    pub last_seq: u64,
    /// A repairable torn/un-synced suffix follows the last valid record —
    /// [`Wal::open`] (or [`repair`]) would truncate it away.
    pub torn_tail: bool,
}

/// Read-only integrity check (API-10). Replays every record without touching
/// a byte on disk: a torn tail is *reported* (`torn_tail: true`), while
/// corruption of the synced region (broken crc chain, corrupt frame with no
/// zero-sector evidence, seq gaps, metadata conflict) is an `Err` — exactly
/// the classification [`Wal::open`] uses, minus the truncation.
///
/// Takes the writer lock: verifying under a live appender would race the
/// tail, so a held lock surfaces as [`WalError::Locked`].
pub fn verify(dir: &Path, metadata: &[u8]) -> Result<VerifyReport, WalError> {
    let lock = File::options()
        .read(true)
        .write(true)
        .open(dir.join(LOCK_FILE))
        .map_err(|_| WalError::NotAWal(format!("{} has no LOCK file", dir.display())))?;
    lock_exclusive(&lock)?;
    scan_all(dir, metadata)
}

/// What [`repair`] did.
#[derive(Debug)]
pub struct RepairReport {
    /// Untouched pre-repair copy of every file in the WAL directory.
    pub backup: PathBuf,
    pub entries: usize,
    pub last_seq: u64,
    /// Whether a torn suffix was actually truncated (false = clean no-op).
    pub truncated: bool,
}

/// Explicit torn-tail repair with a mandatory full backup first (FMT-10).
///
/// Only the same states [`Wal::open`] auto-repairs are repairable — a torn
/// record / zero-sector tail of the LAST segment (etcd: only
/// `ErrUnexpectedEOF` is repairable). Fatal corruption of the synced region
/// returns the verify error with the directory untouched and **no backup
/// made** — nothing was going to be modified. The backup lands beside the
/// WAL as `<dir>.walbak` and an existing one is never clobbered.
///
/// This is a maintenance entry point for a downed writer; any live appender
/// makes it fail with [`WalError::Locked`].
pub fn repair(dir: &Path, metadata: &[u8], opts: WalOptions) -> Result<RepairReport, WalError> {
    // Read-only classification first: fatal corruption must abort before any
    // mutation (FMT-8), and `torn_tail` tells us whether open() will truncate.
    let report = verify(dir, metadata)?;

    let parent = dir.parent().ok_or_else(|| WalError::NotAWal("no parent dir".into()))?;
    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| WalError::NotAWal("unusable dir name".into()))?;
    let backup = parent.join(format!("{dir_name}.walbak"));
    if backup.exists() {
        return Err(WalError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("backup {} already exists — move it away first", backup.display()),
        )));
    }
    fs::create_dir(&backup)?;
    for ent in fs::read_dir(dir)? {
        let ent = ent?;
        if !ent.file_type()?.is_file() {
            continue;
        }
        let p = ent.path();
        fs::copy(&p, backup.join(p.file_name().expect("file has a name")))?;
    }
    File::open(&backup)?.sync_all()?;

    let (entries, wal) = Wal::open(dir, metadata, opts)?;
    let receipt = wal.close()?;
    Ok(RepairReport {
        backup,
        entries: entries.len(),
        last_seq: receipt.last_seq,
        truncated: report.torn_tail,
    })
}

/// List, sort, and contiguity-check the `*.wal` segments of `dir`.
///
/// Foreign files (.DS_Store, editor droppings, subdirs) are ignored — only
/// `*.wal` names participate (review finding #9; etcd does the same). A
/// malformed `*.wal` name is still fatal: real ambiguity.
fn list_segments(dir: &Path) -> Result<Vec<(u64, u64, PathBuf)>, WalError> {
    let mut segs: Vec<(u64, u64, PathBuf)> = Vec::new();
    for ent in fs::read_dir(dir)? {
        let ent = ent?;
        if !ent.file_type()?.is_file() {
            continue;
        }
        let p = ent.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.ends_with(".wal") {
            continue;
        }
        let (seq, first) = parse_segment_name(name)?;
        segs.push((seq, first, p));
    }
    if segs.is_empty() {
        return Err(WalError::NotAWal(format!("{} has no segments", dir.display())));
    }
    segs.sort_by_key(|s| s.0);
    for (i, s) in segs.iter().enumerate() {
        let expected = segs[0].0 + i as u64;
        if s.0 != expected {
            return Err(WalError::SegmentSeqGap { expected, found: s.0 });
        }
    }
    Ok(segs)
}

/// One segment's scan outcome.
struct SegScan {
    /// Offset just past the last fully-validated record.
    valid_off: u64,
    /// A repairable torn/un-synced suffix follows `valid_off` (last segment
    /// only — anywhere else the scan returns the fatal error instead).
    torn: bool,
}

/// The shared per-segment replay: decode records, advance the rolling crc
/// chain, gate the CrcSeed handoff, accumulate entries — and classify the
/// end of the file exactly as documented on [`Wal::open`]. Mutates nothing.
fn scan_segment(
    f: &mut File,
    seg_index: usize,
    is_last: bool,
    metadata: &[u8],
    crc: &mut u32,
    entries: &mut Vec<Entry>,
) -> Result<SegScan, WalError> {
    let file_len = f.metadata()?.len();
    let mut valid_off = 0u64; // offset after the last fully-validated record
    let mut chain_at_entry = *crc; // state we arrived with (handoff gate)
    let mut first_record = true;

    enum End {
        Clean,
        ZeroTail,
        Fail(DecodeFailure),
    }

    let end = {
        let mut rd = BufReader::new(&mut *f);
        loop {
            match record::decode_record(crc, &mut rd) {
                Ok(Decoded::Record(rec, consumed)) => {
                    match rec.kind {
                        RecordKind::CrcSeed => {
                            // FMT-4 handoff gate. A complete-but-wrong
                            // seed is fatal by design: segment headers
                            // are fsynced before the file becomes
                            // visible (create/cut protocol), so a seed
                            // can be corrupt but never torn.
                            if !(seg_index == 0 && first_record) && rec.crc != chain_at_entry {
                                return Err(WalError::CrcMismatch {
                                    expected: rec.crc,
                                    actual: chain_at_entry,
                                });
                            }
                        }
                        RecordKind::Metadata => {
                            if rec.data != metadata {
                                return Err(WalError::MetadataConflict);
                            }
                        }
                        RecordKind::Entry => {
                            let (seq, payload) = record::decode_entry_data(&rec.data)?;
                            if let Some(last) = entries.last() {
                                if seq != last.seq + 1 {
                                    return Err(WalError::EntrySeqGap {
                                        expected: last.seq + 1,
                                        found: seq,
                                    });
                                }
                            }
                            entries.push(Entry { seq, payload });
                        }
                    }
                    valid_off += consumed as u64;
                    chain_at_entry = *crc;
                    first_record = false;
                }
                Ok(Decoded::Eof) => break End::Clean,
                Ok(Decoded::ZeroLen) => {
                    break if is_last { End::ZeroTail } else { End::Clean }
                    // Non-last note: we never preallocate, so zeros in
                    // a sealed segment can only be trailing garbage a
                    // repair once left behind — data past a zero field
                    // is unreachable by construction, matching etcd's
                    // decoder which also stops at the first zero field.
                }
                Err(df) => break End::Fail(df),
            }
        }
    };

    let torn = match end {
        End::Clean => false,
        End::ZeroTail => true, // torn zero tail of the last file
        End::Fail(df) => {
            if !is_last {
                return Err(df.error);
            }
            let repair = match &df.error {
                // Genuinely partial bytes at EOF = torn by definition.
                WalError::TornRecord => true,
                // Complete-but-invalid record: repair ONLY when its own
                // extent contains an all-zero disk sector (FMT-7).
                WalError::CrcMismatch { .. } | WalError::CorruptFrame => {
                    let scan_end = match df.frame_len {
                        Some(fl) => (valid_off + fl).min(file_len),
                        None => (valid_off + 8 + UNKNOWN_EXTENT_SCAN).min(file_len),
                    };
                    // Skip the 8-byte lenField; a frame's own trailing
                    // pad (≤7B) can never fill a 512B sector alone.
                    zero_sector_in_window(f, valid_off + 8, scan_end)?
                }
                _ => false,
            };
            if !repair {
                return Err(df.error);
            }
            true
        }
    };

    Ok(SegScan { valid_off, torn })
}

/// The full read-only replay behind [`verify`] (caller holds the lock).
fn scan_all(dir: &Path, metadata: &[u8]) -> Result<VerifyReport, WalError> {
    let segs = list_segments(dir)?;
    let mut entries: Vec<Entry> = Vec::new();
    let mut crc = 0u32;
    let mut torn = false;
    let last_idx = segs.len() - 1;
    for (i, (_seq, _first, path)) in segs.iter().enumerate() {
        let mut f = OpenOptions::new().read(true).open(path)?;
        let scan = scan_segment(&mut f, i, i == last_idx, metadata, &mut crc, &mut entries)?;
        if i == last_idx {
            torn = scan.torn;
        }
    }
    Ok(VerifyReport {
        segments: segs.len(),
        entries: entries.len(),
        first_seq: entries.first().map(|e| e.seq).unwrap_or(0),
        last_seq: entries.last().map(|e| e.seq).unwrap_or(0),
        torn_tail: torn,
    })
}

/// Scan `[start, end)` of `f` for one all-zero chunk on **absolute file
/// offset** 512-byte sector boundaries (etcd `isTornEntry`). The window is
/// the failing record's own extent — never the whole remaining file — so
/// legitimate zeros in later intact records cannot launder corruption, and a
/// real torn sector cannot hide by straddling a misaligned grid.
fn zero_sector_in_window(f: &mut File, start: u64, end: u64) -> Result<bool, WalError> {
    if end <= start {
        return Ok(false);
    }
    f.seek(SeekFrom::Start(start))?;
    let mut win = vec![0u8; (end - start) as usize];
    f.read_exact(&mut win)?;
    let mut pos = start;
    while pos < end {
        let sector_end = ((pos / 512) + 1) * 512;
        let chunk_end = sector_end.min(end);
        let a = (pos - start) as usize;
        let b = (chunk_end - start) as usize;
        if win[a..b].iter().all(|x| *x == 0) {
            return Ok(true);
        }
        pos = chunk_end;
    }
    Ok(false)
}

fn segment_name(seq: u64, first: u64) -> String {
    format!("{seq:016x}-{first:016x}.wal")
}

fn parse_segment_name(name: &str) -> Result<(u64, u64), WalError> {
    let bad = || WalError::BadSegmentName(name.to_string());
    let stem = name.strip_suffix(".wal").ok_or_else(bad)?;
    let (a, b) = stem.split_once('-').ok_or_else(bad)?;
    if a.len() != 16 || b.len() != 16 {
        return Err(bad());
    }
    Ok((u64::from_str_radix(a, 16).map_err(|_| bad())?, u64::from_str_radix(b, 16).map_err(|_| bad())?))
}

fn lock_exclusive(f: &File) -> Result<(), WalError> {
    match f.try_lock() {
        Ok(()) => Ok(()),
        Err(std::fs::TryLockError::WouldBlock) => Err(WalError::Locked),
        Err(std::fs::TryLockError::Error(e)) => Err(WalError::Io(e)),
    }
}

#[cfg(test)]
mod codec_tests {
    use super::record::{self, Decoded, RecordKind};

    #[test]
    fn frame_roundtrip_with_padding() {
        // data lengths chosen to hit pad = 0..7
        for n in 0..16usize {
            let data: Vec<u8> = (0..n as u8).collect();
            let mut enc_state = 0u32;
            let mut buf = Vec::new();
            let written = record::encode_record(&mut enc_state, RecordKind::Entry, &data, &mut buf);
            assert_eq!(written % 8, 0, "frames stay 8-byte aligned (FMT-2)");

            let mut dec_state = 0u32;
            let mut cur = std::io::Cursor::new(buf);
            match record::decode_record(&mut dec_state, &mut cur).unwrap() {
                Decoded::Record(rec, consumed) => {
                    assert_eq!(rec.data, data);
                    assert_eq!(consumed, written);
                    assert_eq!(dec_state, enc_state, "rolling chain agrees (FMT-3)");
                }
                _ => panic!("expected a record"),
            }
        }
    }

    #[test]
    fn chain_break_is_crc_mismatch() {
        let mut enc_state = 0u32;
        let mut buf = Vec::new();
        record::encode_record(&mut enc_state, RecordKind::Entry, b"hello wal", &mut buf);
        // decoder arriving with a different chain state must reject
        let mut dec_state = 0xDEAD_BEEFu32;
        let mut cur = std::io::Cursor::new(buf);
        match record::decode_record(&mut dec_state, &mut cur) {
            Err(df) => assert!(matches!(df.error, crate::WalError::CrcMismatch { .. })),
            Ok(_) => panic!("chain break must be rejected"),
        }
    }

    #[test]
    fn unknown_kind_is_corrupt_frame_with_extent() {
        let mut enc_state = 0u32;
        let mut buf = Vec::new();
        let n = record::encode_record(&mut enc_state, RecordKind::Entry, b"payload!", &mut buf);
        buf[8] = 7; // kind byte → unknown
        let mut dec_state = 0u32;
        let mut cur = std::io::Cursor::new(buf);
        match record::decode_record(&mut dec_state, &mut cur) {
            Err(df) => {
                assert!(matches!(df.error, crate::WalError::CorruptFrame));
                assert_eq!(df.frame_len, Some(n as u64), "extent known for complete frames");
            }
            Ok(_) => panic!("unknown kind must be CorruptFrame"),
        }
    }

    #[test]
    fn bogus_len_field_is_corrupt_frame_without_extent() {
        // reserved bits set without the pad flag
        let mut buf = (0x40u64 << 56 | 32).to_le_bytes().to_vec();
        buf.extend_from_slice(&[0u8; 64]);
        let mut dec_state = 0u32;
        let mut cur = std::io::Cursor::new(buf);
        match record::decode_record(&mut dec_state, &mut cur) {
            Err(df) => {
                assert!(matches!(df.error, crate::WalError::CorruptFrame));
                assert_eq!(df.frame_len, None);
            }
            Ok(_) => panic!("garbage lenField must be CorruptFrame"),
        }
    }
}
