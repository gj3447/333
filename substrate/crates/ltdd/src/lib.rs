//! 333 v2 substrate verification — Rust-native **LTDD** (log/trace-based TDD).
//!
//! HARD RULE: a receipt asserts what the substrate **actually emitted**, read back from a
//! store, never a return value. A function returning `"ok"` is a *claim*; the store is the
//! judge. This is the substrate's own form of ooptdd's *positive (arrival-asserted) gate* —
//! the same discipline the discovery receipt already uses ("a real DHT put+get, not a
//! serialize-then-read"), generalized into a reusable primitive.
//!
//! Three-valued on purpose ([`Verdict`], LTL3): `Present` (⊤, observed), `Absent` (⊥, the
//! store answered but the record never came — silent loss), `Inconclusive` (?, the store
//! itself was unreachable). `Inconclusive` MUST NEVER be a hard failure — demoting "couldn't
//! observe" to "falsified" is how an infra blip becomes a flaky receipt.
//!
//! It bridges out of Rust too: [`Event::to_ooptdd_json`] / [`to_ooptdd_jsonl`] emit the exact
//! envelope the `ooptdd` (Python) reference verifier reads, so a trace this Rust substrate
//! emitted can be judged by a gate in a *different language and process* — the strongest
//! generator-≠-verifier separation a verdict can have (it attacks ooptdd's own
//! single-authority blind spot: the judge no longer descends from the same authority).
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

    /// The ooptdd-compatible flat JSON envelope: `cid` + `cycle_id` + `event` + flattened
    /// `attrs`. This is the wire shape ooptdd's reader expects (it resolves the cid from
    /// `cycle_id`/`cid`), so a Python ooptdd gate can judge a Rust-emitted trace unchanged.
    pub fn to_ooptdd_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("cid".into(), Value::String(self.cid.clone()));
        o.insert("cycle_id".into(), Value::String(self.cid.clone()));
        o.insert("event".into(), Value::String(self.event.clone()));
        for (k, v) in &self.attrs {
            o.insert(k.clone(), v.clone());
        }
        Value::Object(o)
    }

    /// Inverse of [`Event::to_ooptdd_json`]: rebuild an event from the flat envelope
    /// (`cid` — falling back to `cycle_id` — plus `event`; every other key becomes an
    /// attr). `None` when the value is not an envelope. Round-trip law:
    /// `from_ooptdd_json(&e.to_ooptdd_json()) == Some(e)`.
    pub fn from_ooptdd_json(v: &Value) -> Option<Event> {
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

/// The reference in-process store — zero infra, deterministic (the substrate's equivalent of
/// ooptdd's memory backend). Test hooks: `dropping()` accepts every ship and silently discards
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

/// The LTL3 three-valued verdict — exactly ooptdd's lattice. `Inconclusive` (store unreachable)
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

/// Serialize a trace to the ooptdd JSONL wire shape — one envelope per line. This is the bridge
/// an external `ooptdd` (Python) verifier reads to judge a trace this Rust substrate emitted.
pub fn to_ooptdd_jsonl(events: &[Event]) -> String {
    events
        .iter()
        .map(|e| e.to_ooptdd_json().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}
