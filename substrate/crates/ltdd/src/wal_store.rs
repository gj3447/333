//! `WalStore` — the durable [`Store`]: LTDD's single ship gate backed by
//! `p333-wal` (absorption API-12, design doc §1). Every externalization-judged
//! transition in the substrate already flows through `Store::ship` inline, so
//! implementing `Store` over the WAL event-sources the whole substrate with
//! no crate-API change: `ship` appends each event as one WAL record, `query`
//! reads an in-memory index rebuilt by full replay at [`WalStore::open`].
//!
//! ## The ack boundary, restated for LTDD (API-13 / DUR-6)
//!
//! `[in-memory mutation] → [ship → WalStore] → [sync() Ok] → only then
//! [externalize: sign/answer/forward]`. p333 has no consensus layer behind
//! it, so durable-before-ack is the *only* defense against amnesia
//! equivocation — a crash after signing an un-persisted spend lets the
//! restarted authority sign a conflicting spend of the same version.
//! [`WalStore::externalize`] is the typestate enforcement: the only way to
//! construct an [`Externalized`] value is through a successful sync, so an
//! API that demands `Externalized<T>` cannot be handed un-durable state.
//!
//! ## LTDD-semantics inversion, made explicit (design doc §2)
//!
//! On the *verification* path, `Inconclusive` must never be a hard failure.
//! On the *persistence* path the rule inverts: an append/sync failure means
//! the ack must be refused. `Store::ship` cannot return an error, so a ship
//! failure is **held** and surfaced as `Err` at the next [`WalStore::sync`] —
//! and the store stays failed from then on (mirroring `WalError::Poisoned`):
//! a trace with a silently-dropped event must never look durable-green.

use std::path::Path;

use crate::{Event, Store};

pub use p333_wal::{DurableReceipt, WalError, WalOptions, MAX_PAYLOAD};

/// Errors from the durable store: the WAL substrate itself, or an envelope
/// that came back from replay but no longer parses as an [`Event`] (version
/// skew or foreign writer — the crc chain already proved the bytes intact).
#[derive(Debug)]
pub enum WalStoreError {
    Wal(WalError),
    Envelope { seq: u64, detail: String },
}

impl std::fmt::Display for WalStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalStoreError::Wal(e) => write!(f, "wal store: {e}"),
            WalStoreError::Envelope { seq, detail } => {
                write!(f, "wal store: record {seq} is not an event envelope: {detail}")
            }
        }
    }
}

impl std::error::Error for WalStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WalStoreError::Wal(e) => Some(e),
            WalStoreError::Envelope { .. } => None,
        }
    }
}

impl From<WalError> for WalStoreError {
    fn from(e: WalError) -> Self {
        WalStoreError::Wal(e)
    }
}

/// Proof-carrying value (DUR-6 typestate): `T` paired with the receipt showing
/// the trace behind it reached stable storage. Constructible ONLY through
/// [`WalStore::externalize`] — an externalization API that takes
/// `Externalized<T>` is compile-time protected from un-synced state.
#[derive(Debug)]
pub struct Externalized<T> {
    value: T,
    receipt: DurableReceipt,
}

impl<T> Externalized<T> {
    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn receipt(&self) -> DurableReceipt {
        self.receipt
    }

    pub fn into_parts(self) -> (T, DurableReceipt) {
        (self.value, self.receipt)
    }
}

/// The durable [`Store`]. See the module docs for the ack-boundary and
/// held-failure contracts.
pub struct WalStore {
    wal: p333_wal::Wal,
    /// Replayed durable events + everything shipped this life. `query` reads
    /// this — so pre-sync events are visible in-process (they exist in the
    /// page cache), and a crash makes exactly the un-synced suffix vanish
    /// from the next life's index: the readback IS the durability judge.
    index: Vec<Event>,
    /// First ship failure, held until [`WalStore::sync`] surfaces it.
    pending: Option<WalError>,
}

impl WalStore {
    /// Create a fresh durable store (fails if `dir` exists — [`p333_wal::Wal::create`]).
    pub fn create(dir: &Path, metadata: &[u8], opts: WalOptions) -> Result<Self, WalStoreError> {
        let wal = p333_wal::Wal::create(dir, metadata, opts)?;
        Ok(WalStore { wal, index: Vec::new(), pending: None })
    }

    /// Reopen after a crash or restart: full replay → rebuilt event index.
    /// Only the un-synced suffix of the previous life can be missing (DUR-8).
    pub fn open(dir: &Path, metadata: &[u8], opts: WalOptions) -> Result<Self, WalStoreError> {
        let (entries, wal) = p333_wal::Wal::open(dir, metadata, opts)?;
        let mut index = Vec::with_capacity(entries.len());
        for e in entries {
            let value: serde_json::Value = serde_json::from_slice(&e.payload)
                .map_err(|err| WalStoreError::Envelope { seq: e.seq, detail: err.to_string() })?;
            let event = Event::from_trace_json(&value).ok_or_else(|| WalStoreError::Envelope {
                seq: e.seq,
                detail: "missing cid/event keys".into(),
            })?;
            index.push(event);
        }
        Ok(WalStore { wal, index, pending: None })
    }

    /// The ack boundary. Surfaces any held ship failure as `Err` (and keeps
    /// the store failed); otherwise fdatasyncs and returns the receipt
    /// covering every event shipped so far.
    pub fn sync(&mut self) -> Result<DurableReceipt, WalStoreError> {
        if let Some(e) = &self.pending {
            return Err(WalStoreError::Wal(clone_walerror(e)));
        }
        Ok(self.wal.sync()?)
    }

    /// Sync, then wrap `value` with the proving receipt — the only
    /// constructor of [`Externalized`].
    pub fn externalize<T>(&mut self, value: T) -> Result<Externalized<T>, WalStoreError> {
        let receipt = self.sync()?;
        Ok(Externalized { value, receipt })
    }

    /// Highest seq known durable (0 = none yet).
    pub fn last_synced(&self) -> u64 {
        self.wal.last_synced()
    }
}

/// `WalError` deliberately has no `Clone` (it wraps `io::Error`); the held
/// failure is surfaced repeatedly, so re-materialize a faithful copy.
fn clone_walerror(e: &WalError) -> WalError {
    match e {
        WalError::Io(io) => WalError::Io(std::io::Error::new(io.kind(), io.to_string())),
        WalError::Locked => WalError::Locked,
        WalError::BadSegmentName(s) => WalError::BadSegmentName(s.clone()),
        WalError::SegmentSeqGap { expected, found } => {
            WalError::SegmentSeqGap { expected: *expected, found: *found }
        }
        WalError::EntrySeqGap { expected, found } => {
            WalError::EntrySeqGap { expected: *expected, found: *found }
        }
        WalError::CrcMismatch { expected, actual } => {
            WalError::CrcMismatch { expected: *expected, actual: *actual }
        }
        WalError::TornRecord => WalError::TornRecord,
        WalError::CorruptFrame => WalError::CorruptFrame,
        WalError::ZeroLenMidLog => WalError::ZeroLenMidLog,
        WalError::RecordTooLarge { len, max } => WalError::RecordTooLarge { len: *len, max: *max },
        WalError::Poisoned => WalError::Poisoned,
        WalError::NotAWal(s) => WalError::NotAWal(s.clone()),
        WalError::MetadataConflict => WalError::MetadataConflict,
    }
}

impl Store for WalStore {
    /// Append each event as one WAL record. Infallible by signature — a
    /// failure is held (first one wins, later ships are ignored) and surfaces
    /// at [`WalStore::sync`]; the failed event never enters the index.
    fn ship(&mut self, events: &[Event]) {
        if self.pending.is_some() {
            return; // already failing: hold the line, never resume silently
        }
        for e in events {
            let payload = e.to_trace_json().to_string();
            match self.wal.append(payload.as_bytes()) {
                Ok(_seq) => self.index.push(e.clone()),
                Err(err) => {
                    self.pending = Some(err);
                    return;
                }
            }
        }
    }

    fn query(&self, cid: &str) -> Vec<Event> {
        self.index.iter().filter(|e| e.cid == cid).cloned().collect()
    }
    // reachable() stays `true`: the in-memory read view always answers.
    // Durability is judged at sync()/externalize(), not by demoting reads
    // to Inconclusive (the §2 inversion is about acks, not observations).
}
