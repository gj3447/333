// KG: sprint6C-observability-2026-04-15
//! 333 Platform Observability — lightweight metric registry.
//!
//! # Design principles
//! - Zero external deps beyond `std` (and optionally `once_cell` via `std::sync::OnceLock`
//!   which is stable since Rust 1.70).
//! - WASM-compatible: all primitives are `AtomicU64` / `Mutex<Vec<f64>>` which compile to
//!   wasm32 without issues.
//! - Native: `snapshot()` returns a Prometheus text-format string ready for `/metrics`.
//! - WASM: `get_metrics_json()` returns a JSON snapshot consumable from JS.
//!
//! # Metric types
//! - `Counter`  — monotonically increasing `AtomicU64`.
//! - `Gauge`    — signed integer stored as `AtomicI64`.
//! - `Histogram` — fixed-width buckets (0–1, 1–2, 2–5, 5–10, 10–25, 25–50, 50–100, 100–250, 250+
//!   millisecond boundaries).
//!
//! # Registry
//! A single process-global `REGISTRY` holds all metrics.  Each subsystem calls
//! `observability::counters()` / `observability::histograms()` to obtain a reference
//! to the pre-allocated statics.
//!
//! # KG: sprint6C-observability-2026-04-15

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;

// ── Histogram bucket boundaries (ms) ─────────────────────────────────────────

/// Upper bounds (ms) for histogram buckets.  Final bucket is +Inf.
pub const HIST_BUCKETS: &[f64] = &[1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0];

// ── Histogram ─────────────────────────────────────────────────────────────────

/// Thread-safe, fixed-bucket histogram for latency measurements (milliseconds).
///
/// KG: sprint6C-observability-2026-04-15
pub struct Histogram {
    /// Name used in Prometheus output.
    pub name: &'static str,
    /// Help text.
    pub help: &'static str,
    /// Counts per bucket (len = HIST_BUCKETS.len() + 1 for +Inf bucket).
    buckets: [AtomicU64; 9], // 8 bounds + 1 +Inf
    /// Running sum (in f64 bits stored as u64 via bit-cast, protected by Mutex).
    sum: Mutex<f64>,
    /// Total observation count.
    count: AtomicU64,
}

impl Histogram {
    pub const fn new_uninit(name: &'static str, help: &'static str) -> Self {
        // AtomicU64::new(0) is const since Rust 1.24.
        Self {
            name,
            help,
            buckets: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0), // +Inf
            ],
            sum: Mutex::new(0.0),
            count: AtomicU64::new(0),
        }
    }

    /// Record a single latency observation in milliseconds.
    pub fn observe(&self, ms: f64) {
        // Increment the first bucket whose upper bound >= ms, plus +Inf.
        for (i, &bound) in HIST_BUCKETS.iter().enumerate() {
            if ms <= bound {
                self.buckets[i].fetch_add(1, Ordering::Relaxed);
            }
        }
        // Always increment +Inf bucket.
        self.buckets[8].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut s) = self.sum.lock() {
            *s += ms;
        }
    }

    /// Render as Prometheus text exposition lines.
    pub fn render_prometheus(&self) -> String {
        let mut out = format!("# HELP {} {}\n", self.name, self.help);
        out.push_str(&format!("# TYPE {} histogram\n", self.name));
        for (i, &bound) in HIST_BUCKETS.iter().enumerate() {
            let count = self.buckets[i].load(Ordering::Relaxed);
            out.push_str(&format!("{}_bucket{{le=\"{}\"}} {}\n", self.name, bound, count));
        }
        let inf_count = self.buckets[8].load(Ordering::Relaxed);
        out.push_str(&format!("{}_bucket{{le=\"+Inf\"}} {}\n", self.name, inf_count));
        let sum = self.sum.lock().map(|s| *s).unwrap_or(0.0);
        out.push_str(&format!("{}_sum {}\n", self.name, sum));
        out.push_str(&format!("{}_count {}\n", self.name, inf_count));
        out
    }

    /// Render as a JSON-serializable string fragment.
    pub fn render_json(&self) -> String {
        let sum = self.sum.lock().map(|s| *s).unwrap_or(0.0);
        let count = self.buckets[8].load(Ordering::Relaxed);
        let mut buckets_json = String::from("{");
        for (i, &bound) in HIST_BUCKETS.iter().enumerate() {
            if i > 0 { buckets_json.push(','); }
            buckets_json.push_str(&format!(
                "\"le{}\": {}",
                bound as u64,
                self.buckets[i].load(Ordering::Relaxed)
            ));
        }
        buckets_json.push_str(&format!(",\"leInf\": {}}}", count));
        format!("{{\"sum\":{},\"count\":{},\"buckets\":{}}}", sum, count, buckets_json)
    }
}

// ── Counter ───────────────────────────────────────────────────────────────────

/// Monotonically increasing counter backed by `AtomicU64`.
///
/// KG: sprint6C-observability-2026-04-15
pub struct Counter {
    pub name: &'static str,
    pub help: &'static str,
    value: AtomicU64,
}

impl Counter {
    pub const fn new(name: &'static str, help: &'static str) -> Self {
        Self { name, help, value: AtomicU64::new(0) }
    }

    #[inline]
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_by(&self, n: u64) {
        self.value.fetch_add(n, Ordering::Relaxed);
    }

    #[inline]
    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    pub fn render_prometheus(&self) -> String {
        format!(
            "# HELP {} {}\n# TYPE {} counter\n{} {}\n",
            self.name, self.help, self.name, self.name, self.get()
        )
    }
}

// ── Gauge ─────────────────────────────────────────────────────────────────────

/// Gauge backed by `AtomicI64`.
///
/// KG: sprint6C-observability-2026-04-15
pub struct Gauge {
    pub name: &'static str,
    pub help: &'static str,
    value: AtomicI64,
}

impl Gauge {
    pub const fn new(name: &'static str, help: &'static str) -> Self {
        Self { name, help, value: AtomicI64::new(0) }
    }

    #[inline]
    pub fn set(&self, v: i64) {
        self.value.store(v, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn dec(&self) {
        self.value.fetch_sub(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn get(&self) -> i64 {
        self.value.load(Ordering::Relaxed)
    }

    pub fn render_prometheus(&self) -> String {
        format!(
            "# HELP {} {}\n# TYPE {} gauge\n{} {}\n",
            self.name, self.help, self.name, self.name, self.get()
        )
    }
}

// ── Global metric statics ─────────────────────────────────────────────────────
//
// All metrics live here as process-global statics.  This avoids heap allocation
// on WASM and keeps binary size minimal.  Native and WASM use the same statics;
// only the *export* path differs (HTTP vs. JS callback).
//
// KG: sprint6C-observability-2026-04-15

// ── RTS counters ──────────────────────────────────────────────────────────────

/// Total frame advance calls across all sessions.
/// Label `peer_id` is not encoded in the static; callers use `rts_frame_advance_total_inc`.
pub static RTS_FRAME_ADVANCE_TOTAL: Counter = Counter::new(
    "rts_frame_advance_total",
    "Total frames advanced by RtsSession::advance_frame",
);

/// Total desync events detected by FrameDivergenceDetector.
pub static RTS_DESYNC_EVENTS_TOTAL: Counter = Counter::new(
    "rts_desync_events_total",
    "Total desync events (hash mismatch between local and peer state)",
);

/// Total state hash mismatches (alias for desync events from DesyncDetector path).
pub static RTS_STATE_HASH_MISMATCHES_TOTAL: Counter = Counter::new(
    "rts_state_hash_mismatches_total",
    "Total state hash mismatches detected by DesyncDetector::observe_peer",
);

/// Total peer eject decisions issued by PeerEjectController.
pub static RTS_PEER_EJECTED_TOTAL: Counter = Counter::new(
    "rts_peer_ejected_total",
    "Total peers ejected via BFT-backed AFK vote",
);

/// Frame advance latency in milliseconds (histogram).
pub static RTS_FRAME_ADVANCE_LATENCY_MS: Histogram = Histogram::new_uninit(
    "rts_frame_advance_latency_ms",
    "Latency of RtsSession::advance_frame in milliseconds",
);

// ── BFT counters ─────────────────────────────────────────────────────────────

/// Total BFT checkpoints proposed.
pub static BFT_CHECKPOINT_PROPOSED_TOTAL: Counter = Counter::new(
    "bft_checkpoint_proposed_total",
    "Total BFT checkpoints proposed via HotStuffCheckpointProvider",
);

/// Total BFT checkpoints successfully committed (QC reached quorum).
pub static BFT_CHECKPOINT_COMMITTED_TOTAL: Counter = Counter::new(
    "bft_checkpoint_committed_total",
    "Total BFT checkpoints committed with valid QuorumCert",
);

/// Total BFT vote rejections (stale_view, equivocation, bad_sig combined).
pub static BFT_VOTE_REJECTED_TOTAL: Counter = Counter::new(
    "bft_vote_rejected_total",
    "Total BFT votes rejected (stale view, equivocation, or bad signature)",
);

/// Checkpoint QC latency: propose → commit (milliseconds).
pub static BFT_CHECKPOINT_QC_LATENCY_MS: Histogram = Histogram::new_uninit(
    "bft_checkpoint_qc_latency_ms",
    "Latency from BFT checkpoint proposal to QuorumCert commit in milliseconds",
);

// ── GGRS adapter counters ─────────────────────────────────────────────────────

/// Total GGRS rollback events (save_state slot restored).
pub static GGRS_ROLLBACK_EVENTS_TOTAL: Counter = Counter::new(
    "ggrs_rollback_events_total",
    "Total GGRS rollback events (save state slot loaded for replay)",
);

/// Total save slots evicted from GgrsStub H3 ring buffer.
pub static GGRS_SAVE_SLOT_EVICTED_TOTAL: Counter = Counter::new(
    "ggrs_save_slot_evicted_total",
    "Total GgrsStub save slots evicted from the rolling window",
);

// ── Snapshot helpers ──────────────────────────────────────────────────────────

/// Generate a full Prometheus text format snapshot of all metrics.
///
/// Intended for the native `/metrics` HTTP endpoint.  Not included in WASM
/// (export path uses `get_metrics_json()`).
///
/// KG: sprint6C-observability-2026-04-15
pub fn prometheus_snapshot() -> String {
    let mut out = String::with_capacity(2048);

    // RTS
    out.push_str(&RTS_FRAME_ADVANCE_TOTAL.render_prometheus());
    out.push_str(&RTS_DESYNC_EVENTS_TOTAL.render_prometheus());
    out.push_str(&RTS_STATE_HASH_MISMATCHES_TOTAL.render_prometheus());
    out.push_str(&RTS_PEER_EJECTED_TOTAL.render_prometheus());
    out.push_str(&RTS_FRAME_ADVANCE_LATENCY_MS.render_prometheus());

    // BFT
    out.push_str(&BFT_CHECKPOINT_PROPOSED_TOTAL.render_prometheus());
    out.push_str(&BFT_CHECKPOINT_COMMITTED_TOTAL.render_prometheus());
    out.push_str(&BFT_VOTE_REJECTED_TOTAL.render_prometheus());
    out.push_str(&BFT_CHECKPOINT_QC_LATENCY_MS.render_prometheus());

    // GGRS
    out.push_str(&GGRS_ROLLBACK_EVENTS_TOTAL.render_prometheus());
    out.push_str(&GGRS_SAVE_SLOT_EVICTED_TOTAL.render_prometheus());

    out
}

/// Generate a JSON snapshot of all Rust-side metrics.
///
/// Used by the WASM `get_metrics_json()` export and optional native debug page.
///
/// KG: sprint6C-observability-2026-04-15
pub fn json_snapshot() -> String {
    format!(
        concat!(
            "{{",
            "\"rts_frame_advance_total\":{},",
            "\"rts_desync_events_total\":{},",
            "\"rts_state_hash_mismatches_total\":{},",
            "\"rts_peer_ejected_total\":{},",
            "\"rts_frame_advance_latency_ms\":{},",
            "\"bft_checkpoint_proposed_total\":{},",
            "\"bft_checkpoint_committed_total\":{},",
            "\"bft_vote_rejected_total\":{},",
            "\"bft_checkpoint_qc_latency_ms\":{},",
            "\"ggrs_rollback_events_total\":{},",
            "\"ggrs_save_slot_evicted_total\":{}",
            "}}"
        ),
        RTS_FRAME_ADVANCE_TOTAL.get(),
        RTS_DESYNC_EVENTS_TOTAL.get(),
        RTS_STATE_HASH_MISMATCHES_TOTAL.get(),
        RTS_PEER_EJECTED_TOTAL.get(),
        RTS_FRAME_ADVANCE_LATENCY_MS.render_json(),
        BFT_CHECKPOINT_PROPOSED_TOTAL.get(),
        BFT_CHECKPOINT_COMMITTED_TOTAL.get(),
        BFT_VOTE_REJECTED_TOTAL.get(),
        BFT_CHECKPOINT_QC_LATENCY_MS.render_json(),
        GGRS_ROLLBACK_EVENTS_TOTAL.get(),
        GGRS_SAVE_SLOT_EVICTED_TOTAL.get(),
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_increments() {
        let c = Counter::new("test_counter", "test");
        assert_eq!(c.get(), 0);
        c.inc();
        c.inc_by(4);
        assert_eq!(c.get(), 5);
    }

    #[test]
    fn gauge_set_inc_dec() {
        let g = Gauge::new("test_gauge", "test");
        g.set(10);
        g.inc();
        g.dec();
        assert_eq!(g.get(), 10);
    }

    #[test]
    fn histogram_observe_buckets() {
        let h = Histogram::new_uninit("test_hist", "test");
        h.observe(3.0);  // goes into le5, le10, le25, le50, le100, le250, leInf
        h.observe(60.0); // goes into le100, le250, leInf
        assert_eq!(h.buckets[8].load(Ordering::Relaxed), 2); // count
        // le5 bucket (index 2): 3.0 <= 5.0 → 1
        assert_eq!(h.buckets[2].load(Ordering::Relaxed), 1);
        // le100 bucket (index 6): both 3 and 60 <= 100 → 2
        assert_eq!(h.buckets[6].load(Ordering::Relaxed), 2);
    }

    #[test]
    fn prometheus_snapshot_contains_all_metric_names() {
        let snap = prometheus_snapshot();
        assert!(snap.contains("rts_frame_advance_total"));
        assert!(snap.contains("rts_desync_events_total"));
        assert!(snap.contains("bft_checkpoint_proposed_total"));
        assert!(snap.contains("ggrs_rollback_events_total"));
    }

    #[test]
    fn json_snapshot_is_valid_object() {
        let snap = json_snapshot();
        assert!(snap.starts_with('{'));
        assert!(snap.ends_with('}'));
        assert!(snap.contains("\"rts_frame_advance_total\""));
        assert!(snap.contains("\"ggrs_save_slot_evicted_total\""));
    }
}
