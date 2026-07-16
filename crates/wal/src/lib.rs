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
//! ## Recovery model (DUR-8)
//!
//! Reopening a WAL can lose **only the un-synced suffix**. A torn tail is
//! detected (partial record, or zero-sector heuristic FMT-7) and truncated;
//! a broken crc chain inside the synced region is fatal `CrcMismatch` and is
//! never auto-repaired (FMT-8).
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
//! instead of per-segment locks, snapshots/release_older deferred.
#![cfg(unix)]

mod error;
mod record;

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub use error::WalError;
use record::{Decoded, RecordKind};

/// Default segment roll-over size, same order as etcd's SegmentSizeBytes.
pub const DEFAULT_SEGMENT_SIZE: u64 = 64 * 1024 * 1024;

const LOCK_FILE: &str = "LOCK";

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
/// Externalization APIs should demand this token (typestate court of the
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
    opts: WalOptions,
}

impl Wal {
    /// Create a fresh WAL directory. Fails if `dir` already exists.
    ///
    /// Atomicity (LIF-1/API-2): the directory is fully built under a `.tmp`
    /// sibling — initial segment with CrcSeed(0)+Metadata already fsynced —
    /// then renamed into place, and the parent directory is fsynced. A crash
    /// anywhere before the rename leaves only a `.tmp` husk, never a
    /// half-initialized WAL.
    pub fn create(dir: &Path, metadata: &[u8], opts: WalOptions) -> Result<Wal, WalError> {
        if dir.exists() {
            return Err(WalError::NotAWal(format!("{} already exists", dir.display())));
        }
        let parent = dir.parent().ok_or_else(|| WalError::NotAWal("no parent dir".into()))?;
        let tmp = dir.with_extension("waltmp");
        if tmp.exists() {
            fs::remove_dir_all(&tmp)?;
        }
        fs::create_dir(&tmp)?;

        // Lock file exists from birth so open() always finds it.
        let lock = File::create(tmp.join(LOCK_FILE))?;
        flock_exclusive(&lock)?;

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
            opts,
        })
    }

    /// Open an existing WAL: replay everything, repair a torn tail if the
    /// zero-sector heuristic proves one (FMT-7), and return the surviving
    /// entries plus the appendable handle positioned after the last valid
    /// record.
    pub fn open(dir: &Path, metadata: &[u8], opts: WalOptions) -> Result<(Vec<Entry>, Wal), WalError> {
        let lock = File::options()
            .read(true)
            .write(true)
            .open(dir.join(LOCK_FILE))
            .map_err(|_| WalError::NotAWal(format!("{} has no LOCK file", dir.display())))?;
        flock_exclusive(&lock)?;

        let mut segs: Vec<(u64, u64, PathBuf)> = Vec::new();
        for ent in fs::read_dir(dir)? {
            let p = ent?.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == LOCK_FILE || name.ends_with(".tmp") {
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

        let mut entries: Vec<Entry> = Vec::new();
        let mut crc = 0u32;
        let mut tail: Option<(File, u64)> = None;
        let last_idx = segs.len() - 1;

        for (i, (_seq, _first, path)) in segs.iter().enumerate() {
            let is_last = i == last_idx;
            let mut f = OpenOptions::new().read(true).write(true).open(path)?;
            let file_len = f.metadata()?.len();
            let mut valid_off = 0u64; // offset after the last fully-validated record
            let mut chain_at_entry = crc; // state we arrived with (handoff gate)
            let mut first_record = true;

            loop {
                let res = record::decode_record(&mut crc, &mut f);
                match res {
                    Ok(Decoded::Record(rec, consumed)) => {
                        match rec.kind {
                            RecordKind::CrcSeed => {
                                // FMT-4 handoff gate: a seed must equal the
                                // chain state we carried in — except the very
                                // first record of the whole log, which defines it.
                                if !(i == 0 && first_record) && rec.crc != chain_at_entry {
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
                        chain_at_entry = crc;
                        first_record = false;
                    }
                    Ok(Decoded::ZeroLen) | Ok(Decoded::Eof) => break, // clean end (FMT-5)
                    Err(e) => {
                        if is_last && torn_tail(&mut f, valid_off, file_len, &e)? {
                            // Repairable torn tail: drop the un-synced suffix
                            // (DUR-8 — the only thing recovery may destroy).
                            f.set_len(valid_off)?;
                            f.sync_data()?;
                            break;
                        }
                        return Err(e);
                    }
                }
            }

            if is_last {
                f.seek(SeekFrom::Start(valid_off))?;
                // Also drop any trailing zero-fill so appends continue at the
                // last valid record (etcd ZeroToEnd equivalent, FMT-11 delta:
                // we truncate instead of zero-filling since we don't prealloc).
                f.set_len(valid_off)?;
                f.sync_data()?;
                tail = Some((f, valid_off));
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
                opts,
            },
        ))
    }

    /// Append one record. **No durability claim** — the record is in the OS
    /// page cache at best until [`Wal::sync`] returns.
    pub fn append(&mut self, payload: &[u8]) -> Result<u64, WalError> {
        let seq = self.next_seq;
        let data = record::encode_entry_data(seq, payload);
        let mut buf = Vec::with_capacity(16 + data.len() + 8);
        record::encode_record(&mut self.crc, RecordKind::Entry, &data, &mut buf);
        self.tail.write_all(&buf)?;
        self.tail_offset += buf.len() as u64;
        self.next_seq += 1;
        if self.tail_offset >= self.opts.segment_size {
            self.cut()?; // implies a full sync of the old tail (DUR-4)
        }
        Ok(seq)
    }

    /// The ack boundary: fdatasync the tail. Everything appended so far is on
    /// stable storage when this returns Ok (DUR-1).
    pub fn sync(&mut self) -> Result<DurableReceipt, WalError> {
        self.tail.sync_data()?;
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

    /// Roll to a new segment (LIF-3 / API-8, inline — no filePipeline):
    /// finish + fsync the old tail, build the new segment **fully in a .tmp
    /// file** (CrcSeed handoff + Metadata, fsynced), then rename + dir fsync.
    /// A segment file is never visible under its `.wal` name without a valid
    /// header.
    fn cut(&mut self) -> Result<(), WalError> {
        self.tail.sync_data()?;
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
        let _ = self.tail.sync_data();
    }
}

/// FMT-7: a decode failure at the tail of the LAST segment is a repairable
/// torn write iff it is a partial record, or the bytes after the last valid
/// record contain at least one all-zero 512-byte sector. A failure that is a
/// complete-but-mismatching record surrounded by fully non-zero bytes is
/// treated as real corruption (fatal).
fn torn_tail(f: &mut File, valid_off: u64, file_len: u64, err: &WalError) -> Result<bool, WalError> {
    if matches!(err, WalError::TornRecord) {
        return Ok(true);
    }
    if !matches!(err, WalError::CrcMismatch { .. }) {
        return Ok(false);
    }
    f.seek(SeekFrom::Start(valid_off))?;
    let mut rest = vec![0u8; (file_len - valid_off) as usize];
    f.read_exact(&mut rest)?;
    Ok(rest.chunks(512).any(|c| c.iter().all(|b| *b == 0)))
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

fn flock_exclusive(f: &File) -> Result<(), WalError> {
    rustix::fs::flock(f, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| WalError::Locked)
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
        assert!(matches!(
            record::decode_record(&mut dec_state, &mut cur),
            Err(crate::WalError::CrcMismatch { .. })
        ));
    }
}
