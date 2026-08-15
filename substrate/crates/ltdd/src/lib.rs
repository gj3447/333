//! 333 v2 substrate verification — Rust-native **LTDD** (log/trace-based TDD).
//!
//! HARD RULE: a receipt asserts what the substrate **actually emitted**, read back from a
//! store, never a return value. A function returning `"ok"` is a *claim*; the store is the
//! judge. This is a native arrival assertion using the same discipline the discovery
//! receipt already uses ("a real DHT put+get, not a
//! serialize-then-read"), generalized into a reusable primitive.
//!
//! Three-valued on purpose ([`Verdict`], LTL3): `Present` (⊤, observed), `Absent` (⊥, the
//! store answered but the record never came — silent loss), `Inconclusive` (?, the store
//! itself was unreachable). `Inconclusive` MUST NEVER be a hard failure — demoting "couldn't
//! observe" to "falsified" is how an infra blip becomes a flaky receipt.
//!
//! [`Event::to_trace_json`] and [`to_trace_jsonl`] provide a transparent diagnostic
//! envelope. They do not delegate the verdict to an external rule engine; the native
//! tests assert observed state directly.
//!
//! Scope: this is for **side-effect/arrival** facts (a packet was published, a peer resolved
//! it, a relay forwarded N bytes). It is NOT for the substrate's crypto-correctness invariants
//! (Ed25519/RFC-8032 conformance, DID==PeerId) — those are a *log-free zone*, asserted directly
//! against known-answer test vectors, exactly as the `identity`/`discovery` receipts do.

use serde_json::{Map, Value};

#[cfg(all(feature = "wal", unix))]
pub mod wal_store;

/// A structured trace event — the LTDD envelope. `cid` correlates one cycle across peers,
/// `event` names the step, `attrs` are structured fields. Never assert on free-text logs:
/// that resurrects the oracle problem (rewording a log line breaks the receipt).
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub cid: String,
    pub event: String,
    pub attrs: Map<String, Value>,
}

impl Event {
    /// A new event under correlation id `cid`.
    pub fn new(cid: impl Into<String>, event: impl Into<String>) -> Self {
        Self { cid: cid.into(), event: event.into(), attrs: Map::new() }
    }

    /// Attach a structured attribute (builder style).
    pub fn with(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.attrs.insert(key.into(), value.into());
        self
    }

    /// A flat JSON envelope: `cid` + `cycle_id` + `event` + flattened `attrs`.
    pub fn to_trace_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("cid".into(), Value::String(self.cid.clone()));
        o.insert("cycle_id".into(), Value::String(self.cid.clone()));
        o.insert("event".into(), Value::String(self.event.clone()));
        for (k, v) in &self.attrs {
            o.insert(k.clone(), v.clone());
        }
        Value::Object(o)
    }

    /// Inverse of [`Event::to_trace_json`]: rebuild an event from the flat envelope
    /// (`cid` — falling back to `cycle_id` — plus `event`; every other key becomes an
    /// attr). `None` when the value is not an envelope. Round-trip law:
    /// `from_trace_json(&e.to_trace_json()) == Some(e)`.
    pub fn from_trace_json(v: &Value) -> Option<Event> {
        let o = v.as_object()?;
        let cid = o.get("cid").or_else(|| o.get("cycle_id"))?.as_str()?.to_string();
        let event = o.get("event")?.as_str()?.to_string();
        let mut attrs = Map::new();
        for (k, val) in o {
            if k != "cid" && k != "cycle_id" && k != "event" {
                attrs.insert(k.clone(), val.clone());
            }
        }
        Some(Event { cid, event, attrs })
    }
}

/// Where emitted events land and are read back from. `ship` is the write path; `query` is the
/// read-back the verdict reads — never the return value of the code under test. `reachable`
/// distinguishes "the store says no such event" (⊥ absent) from "I could not ask" (? inconclusive).
pub trait Store {
    fn ship(&mut self, events: &[Event]);
    fn query(&self, cid: &str) -> Vec<Event>;
    /// False iff the store could not be reached at all. Default: reachable.
    fn reachable(&self) -> bool {
        true
    }
}

/// The reference in-process store is zero-infrastructure and deterministic. Test hooks:
/// `dropping()` accepts every ship and silently discards
/// it (the 401 nobody noticed); `unreachable()` fails the read entirely (→ `Inconclusive`).
#[derive(Default)]
pub struct MemoryStore {
    events: Vec<Event>,
    drop: bool,
    unreachable: bool,
}

impl MemoryStore {
    /// A store that silently drops every shipped event (simulates silent ingest loss).
    pub fn dropping() -> Self {
        Self { drop: true, ..Default::default() }
    }

    /// A store whose read side is unreachable (every query fails → inconclusive).
    pub fn unreachable() -> Self {
        Self { unreachable: true, ..Default::default() }
    }
}

impl Store for MemoryStore {
    fn ship(&mut self, events: &[Event]) {
        if !self.drop {
            self.events.extend_from_slice(events);
        }
    }

    fn query(&self, cid: &str) -> Vec<Event> {
        self.events.iter().filter(|e| e.cid == cid).cloned().collect()
    }

    fn reachable(&self) -> bool {
        !self.unreachable
    }
}

/// The LTL3 three-valued verdict. `Inconclusive` (store unreachable)
/// must never be treated as failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// ⊤ — at least `min_count` of the event arrived.
    Present,
    /// ⊥ — the store answered, but the record never came (silent loss suspected).
    Absent,
    /// ? — the store could not be queried; never a hard failure.
    Inconclusive,
}

/// Positive arrival assertion: did `>= min_count` of `event` for `cid` actually land in the
/// store? Reads the store back; it does not trust any return value.
pub fn verify_present(store: &impl Store, cid: &str, event: &str, min_count: usize) -> Verdict {
    if !store.reachable() {
        return Verdict::Inconclusive;
    }
    let n = store.query(cid).iter().filter(|e| e.event == event).count();
    if n >= min_count {
        Verdict::Present
    } else {
        Verdict::Absent
    }
}

/// Serialize a trace to the native JSONL diagnostic shape, one envelope per line.
pub fn to_trace_jsonl(events: &[Event]) -> String {
    events
        .iter()
        .map(|e| e.to_trace_json().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}
