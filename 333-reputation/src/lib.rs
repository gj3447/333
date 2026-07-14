// KG: SPAN_333_L12_Reputation, plan-333-p2p-os-synthesis-execution-2026-04-18
// 333 P2P OS L12 Reputation — peer scoring + fanout selection.
//
// Source: finding_333_synth_rte_prt_d14 (trait-based PeerScorer, GossipFanout
// selector integrated with 333-topology).
//
// Score formula (weighted sum, normalized to [0, 1]):
//   score = w_bw * bw_success + w_lat * (1 - lat_norm) + w_up * uptime_frac
//
// Caller provides weights; default: 0.4 / 0.3 / 0.3 (D15 pitfall: hot-spot —
// cap score at 0.99 to avoid oscillation and spread load).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use identity333::NodeId;

#[derive(Debug, Clone, Copy)]
pub struct ScoreWeights {
    pub bandwidth: f64,
    pub latency: f64,
    pub uptime: f64,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self { bandwidth: 0.4, latency: 0.3, uptime: 0.3 }
    }
}

/// Cumulative metrics collected per peer.
#[derive(Debug, Clone, Copy, Default)]
pub struct PeerMetrics {
    pub bw_success: u64,
    pub bw_failure: u64,
    pub latency_ms_sum: u64,
    pub latency_samples: u64,
    pub uptime_seconds: u64,
    pub downtime_seconds: u64,
}

impl PeerMetrics {
    fn bw_success_rate(&self) -> f64 {
        let total = self.bw_success + self.bw_failure;
        if total == 0 {
            0.5 // neutral prior
        } else {
            self.bw_success as f64 / total as f64
        }
    }

    fn latency_normalized(&self, cap_ms: u64) -> f64 {
        if self.latency_samples == 0 {
            0.5
        } else {
            let avg = self.latency_ms_sum as f64 / self.latency_samples as f64;
            (avg / cap_ms as f64).min(1.0)
        }
    }

    fn uptime_frac(&self) -> f64 {
        let total = self.uptime_seconds + self.downtime_seconds;
        if total == 0 {
            0.5
        } else {
            self.uptime_seconds as f64 / total as f64
        }
    }
}

pub trait PeerScorer {
    fn record_success(&self, peer: &NodeId, latency_ms: u64);
    fn record_failure(&self, peer: &NodeId);
    fn record_uptime(&self, peer: &NodeId, seconds: u64);
    fn record_downtime(&self, peer: &NodeId, seconds: u64);
    fn score(&self, peer: &NodeId) -> f64;
    fn metrics(&self, peer: &NodeId) -> PeerMetrics;
}

#[derive(Debug)]
pub struct InMemoryScorer {
    weights: ScoreWeights,
    latency_cap_ms: u64,
    max_score: f64,
    inner: Arc<Mutex<HashMap<NodeId, PeerMetrics>>>,
}

impl InMemoryScorer {
    pub fn new(weights: ScoreWeights, latency_cap_ms: u64) -> Self {
        Self {
            weights,
            latency_cap_ms,
            max_score: 0.99, // D15 anti-hot-spot cap
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn default_weights() -> Self {
        Self::new(ScoreWeights::default(), 1000)
    }
}

impl Default for InMemoryScorer {
    fn default() -> Self {
        Self::default_weights()
    }
}

impl PeerScorer for InMemoryScorer {
    fn record_success(&self, peer: &NodeId, latency_ms: u64) {
        let mut g = self.inner.lock().unwrap();
        let m = g.entry(peer.clone()).or_default();
        m.bw_success = m.bw_success.saturating_add(1);
        m.latency_ms_sum = m.latency_ms_sum.saturating_add(latency_ms);
        m.latency_samples = m.latency_samples.saturating_add(1);
    }

    fn record_failure(&self, peer: &NodeId) {
        let mut g = self.inner.lock().unwrap();
        let m = g.entry(peer.clone()).or_default();
        m.bw_failure = m.bw_failure.saturating_add(1);
    }

    fn record_uptime(&self, peer: &NodeId, seconds: u64) {
        let mut g = self.inner.lock().unwrap();
        let m = g.entry(peer.clone()).or_default();
        m.uptime_seconds = m.uptime_seconds.saturating_add(seconds);
    }

    fn record_downtime(&self, peer: &NodeId, seconds: u64) {
        let mut g = self.inner.lock().unwrap();
        let m = g.entry(peer.clone()).or_default();
        m.downtime_seconds = m.downtime_seconds.saturating_add(seconds);
    }

    fn score(&self, peer: &NodeId) -> f64 {
        let g = self.inner.lock().unwrap();
        let m = g.get(peer).copied().unwrap_or_default();
        let bw = m.bw_success_rate();
        let lat = 1.0 - m.latency_normalized(self.latency_cap_ms);
        let up = m.uptime_frac();
        let raw = self.weights.bandwidth * bw
            + self.weights.latency * lat
            + self.weights.uptime * up;
        raw.min(self.max_score).max(0.0)
    }

    fn metrics(&self, peer: &NodeId) -> PeerMetrics {
        self.inner.lock().unwrap().get(peer).copied().unwrap_or_default()
    }
}

/// Select top-k peers by score for eager gossip fanout. Remaining peers
/// (below threshold) get lazy fanout via IHAVE.
pub fn gossip_fanout_top_k<S: PeerScorer>(
    scorer: &S,
    peers: &[NodeId],
    k: usize,
) -> Vec<NodeId> {
    let mut with_score: Vec<(NodeId, f64)> =
        peers.iter().map(|p| (p.clone(), scorer.score(p))).collect();
    with_score.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    with_score.truncate(k);
    with_score.into_iter().map(|(p, _)| p).collect()
}

/// Partition peers into (eager, lazy) by a score threshold.
pub fn partition_by_threshold<S: PeerScorer>(
    scorer: &S,
    peers: &[NodeId],
    threshold: f64,
) -> (Vec<NodeId>, Vec<NodeId>) {
    let mut eager = Vec::new();
    let mut lazy = Vec::new();
    for p in peers {
        if scorer.score(p) >= threshold {
            eager.push(p.clone());
        } else {
            lazy.push(p.clone());
        }
    }
    (eager, lazy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use identity333::Keypair;

    fn mk() -> NodeId {
        Keypair::generate().node_id()
    }

    #[test]
    fn neutral_prior_for_unknown_peer() {
        let s = InMemoryScorer::default();
        let score = s.score(&mk());
        // All priors = 0.5 → weighted sum = 0.5
        assert!((score - 0.5).abs() < 1e-9);
    }

    #[test]
    fn success_raises_score() {
        let s = InMemoryScorer::default();
        let p = mk();
        s.record_success(&p, 10);
        s.record_success(&p, 20);
        s.record_uptime(&p, 3600);
        let score = s.score(&p);
        assert!(score > 0.5, "expected >0.5, got {score}");
    }

    #[test]
    fn failure_lowers_bw_component() {
        let s = InMemoryScorer::default();
        let p = mk();
        for _ in 0..10 {
            s.record_failure(&p);
        }
        let m = s.metrics(&p);
        assert!(m.bw_success_rate() < 0.1);
    }

    #[test]
    fn hot_spot_capped_at_max() {
        let s = InMemoryScorer::default();
        let p = mk();
        for _ in 0..1000 {
            s.record_success(&p, 0);
        }
        s.record_uptime(&p, 1_000_000);
        let score = s.score(&p);
        assert!(score <= 0.99, "hot-spot cap violated: {score}");
    }

    #[test]
    fn top_k_selects_best() {
        let s = InMemoryScorer::default();
        let a = mk();
        let b = mk();
        let c = mk();
        // a = best, c = worst
        for _ in 0..5 {
            s.record_success(&a, 10);
        }
        s.record_uptime(&a, 1000);
        for _ in 0..5 {
            s.record_failure(&c);
        }
        let top = gossip_fanout_top_k(&s, &[a.clone(), b.clone(), c.clone()], 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0], a);
        assert_ne!(top[1], c);
    }

    #[test]
    fn threshold_partition_splits() {
        let s = InMemoryScorer::default();
        let good = mk();
        let bad = mk();
        for _ in 0..10 {
            s.record_success(&good, 5);
        }
        s.record_uptime(&good, 10_000);
        for _ in 0..10 {
            s.record_failure(&bad);
        }
        let (eager, lazy) =
            partition_by_threshold(&s, &[good.clone(), bad.clone()], 0.6);
        assert!(eager.contains(&good));
        assert!(lazy.contains(&bad));
    }

    #[test]
    fn saturating_no_panic_under_overflow() {
        let s = InMemoryScorer::default();
        let p = mk();
        s.record_success(&p, u64::MAX);
        s.record_success(&p, 100);
        // No panic.
        let _ = s.score(&p);
    }
}
