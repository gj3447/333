// KG: SPAN_333_L14_Incentive, plan-333-p2p-os-synthesis-execution-2026-04-18,
//     queue-p11-super-peer-incentive-2026-04-18
//
// 333 P2P OS L14 Incentive — super-peer selection and reward simulation.
//
// CANONICALITY NOTE (PROM 4, 2026-07-15): direct `TokenLedger::mint` distribution
// is not the ORRR settlement path. Production reward allocations must become a
// `payment333::ControlOperation::DistributeReward`, be BFT-certified once per
// epoch, and settle through one-shot reserve-backed vouchers. This crate remains
// useful for selecting peers and calculating candidate weights.
//
// Composition:
//   - reputation333::PeerScorer  → observed behaviour (bandwidth, latency, uptime).
//   - token333::EmissionCurve    → per-epoch reward budget.
//   - token333::TokenLedger      → credits rewards to selected peers.
//
// Flow per epoch:
//   1. `SuperPeerSelector::top_k` picks the k highest-scoring peers.
//   2. `IncentiveCurve::share` turns each peer's (score, stake_bonus) into a
//      weight. Weights are normalized over the selected set.
//   3. `RewardDistributor::distribute` multiplies the epoch emission by each
//      weight and mints it onto the ledger. Residue from integer division is
//      burned (conservative: never over-issue).

#![forbid(unsafe_code)]

use std::sync::Arc;

use identity333::NodeId;
use reputation333::PeerScorer;
use thiserror::Error;
use token333::{Amount, Epoch, EmissionCurve, TokenError, TokenLedger};

#[derive(Debug, Error)]
pub enum IncentiveError {
    #[error("no candidates to select from")]
    NoCandidates,
    #[error("selector k must be > 0")]
    ZeroK,
    #[error("token layer: {0}")]
    Token(#[from] TokenError),
}

// ============================================================================
// IncentiveCurve — maps (score, stake) to an unnormalized weight
// ============================================================================

pub trait IncentiveCurve: Send + Sync {
    /// Weight contribution for a peer with normalized reputation `score`
    /// (0.0..=1.0) and stake bonus `stake_bonus` (0.0..=1.0). Higher weight →
    /// larger share of the epoch reward.
    fn weight(&self, score: f64, stake_bonus: f64) -> f64;
}

/// Quadratic-bonus-on-reputation + linear stake bonus. Default incentive curve.
#[derive(Debug, Clone, Copy)]
pub struct QuadraticCurve {
    /// Coefficient on score^2. Default 1.0.
    pub score_coef: f64,
    /// Coefficient on stake_bonus. Default 0.5.
    pub stake_coef: f64,
    /// Additive floor so every selected peer earns something. Default 0.01.
    pub floor: f64,
}

impl Default for QuadraticCurve {
    fn default() -> Self {
        Self { score_coef: 1.0, stake_coef: 0.5, floor: 0.01 }
    }
}

impl IncentiveCurve for QuadraticCurve {
    fn weight(&self, score: f64, stake_bonus: f64) -> f64 {
        let s = score.clamp(0.0, 1.0);
        let b = stake_bonus.clamp(0.0, 1.0);
        self.floor + self.score_coef * s * s + self.stake_coef * b
    }
}

// ============================================================================
// SuperPeerSelector — top-K by reputation score
// ============================================================================

pub struct SuperPeerSelector<'a, S: PeerScorer> {
    pub scorer: &'a S,
}

impl<'a, S: PeerScorer> SuperPeerSelector<'a, S> {
    pub fn top_k(&self, candidates: &[NodeId], k: usize) -> Result<Vec<NodeId>, IncentiveError> {
        if k == 0 {
            return Err(IncentiveError::ZeroK);
        }
        if candidates.is_empty() {
            return Err(IncentiveError::NoCandidates);
        }
        let mut scored: Vec<(NodeId, f64)> =
            candidates.iter().map(|p| (p.clone(), self.scorer.score(p))).collect();
        // Descending score; ties broken by NodeId for determinism.
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.as_bytes().cmp(b.0.as_bytes()))
        });
        scored.truncate(k);
        Ok(scored.into_iter().map(|(n, _)| n).collect())
    }
}

// ============================================================================
// RewardDistributor — mints epoch emissions weighted by IncentiveCurve
// ============================================================================

pub struct RewardDistributor<C: IncentiveCurve, L: TokenLedger> {
    pub curve: C,
    pub ledger: Arc<L>,
    pub emission: EmissionCurve,
}

#[derive(Debug, Clone)]
pub struct Distribution {
    pub epoch: Epoch,
    pub total_reward: Amount,
    pub per_peer: Vec<(NodeId, Amount)>,
    pub residue_burned: Amount,
}

impl<C: IncentiveCurve, L: TokenLedger> RewardDistributor<C, L> {
    pub fn new(curve: C, ledger: Arc<L>, emission: EmissionCurve) -> Self {
        Self { curve, ledger, emission }
    }

    /// Distribute the epoch reward over the selected peers.
    ///
    /// `score_of(peer)` returns the peer's reputation score (0..=1) and
    /// `stake_bonus_of(peer)` returns a stake-derived bonus (0..=1). Both are
    /// callbacks so the distributor stays decoupled from the specific scorer /
    /// stake pool types.
    pub fn distribute(
        &self,
        epoch: Epoch,
        peers: &[NodeId],
        score_of: impl Fn(&NodeId) -> f64,
        stake_bonus_of: impl Fn(&NodeId) -> f64,
    ) -> Result<Distribution, IncentiveError> {
        if peers.is_empty() {
            return Err(IncentiveError::NoCandidates);
        }
        let total = self.emission.reward_at(epoch)?;
        if total == 0 {
            return Ok(Distribution {
                epoch,
                total_reward: 0,
                per_peer: peers.iter().map(|p| (p.clone(), 0)).collect(),
                residue_burned: 0,
            });
        }

        let weights: Vec<(NodeId, f64)> = peers
            .iter()
            .map(|p| (p.clone(), self.curve.weight(score_of(p), stake_bonus_of(p))))
            .collect();
        let sum: f64 = weights.iter().map(|(_, w)| w).sum();
        if sum <= 0.0 {
            // Degenerate: all weights zero → split evenly.
            let share = total / peers.len() as Amount;
            let residue = total - share * peers.len() as Amount;
            let per_peer: Vec<(NodeId, Amount)> =
                peers.iter().map(|p| (p.clone(), share)).collect();
            for (p, amt) in &per_peer {
                if *amt > 0 {
                    self.ledger.mint(p, *amt)?;
                }
            }
            return Ok(Distribution {
                epoch,
                total_reward: total,
                per_peer,
                residue_burned: residue,
            });
        }

        let mut per_peer: Vec<(NodeId, Amount)> = Vec::with_capacity(peers.len());
        let mut minted_total: Amount = 0;
        for (p, w) in &weights {
            let share = ((*w / sum) * total as f64).floor() as Amount;
            per_peer.push((p.clone(), share));
            minted_total += share;
        }
        for (p, amt) in &per_peer {
            if *amt > 0 {
                self.ledger.mint(p, *amt)?;
            }
        }
        let residue = total - minted_total;
        Ok(Distribution {
            epoch,
            total_reward: total,
            per_peer,
            residue_burned: residue,
        })
    }
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use identity333::Keypair;
    use reputation333::{InMemoryScorer, PeerScorer as RepPeerScorer, ScoreWeights};
    use token333::InMemoryLedger;

    fn node() -> NodeId {
        Keypair::generate().node_id()
    }

    fn scorer_with_good_bad() -> (InMemoryScorer, NodeId, NodeId, NodeId) {
        let s = InMemoryScorer::new(ScoreWeights::default(), 500);
        let good = node();
        let meh = node();
        let bad = node();
        // good: high bw success, low latency, high uptime
        for _ in 0..50 {
            s.record_success(&good, 20);
        }
        s.record_uptime(&good, 10_000);
        // meh: medium across the board
        for _ in 0..10 {
            s.record_success(&meh, 100);
        }
        for _ in 0..10 {
            s.record_failure(&meh);
        }
        s.record_uptime(&meh, 500);
        s.record_downtime(&meh, 500);
        // bad: mostly failures and downtime
        for _ in 0..50 {
            s.record_failure(&bad);
        }
        s.record_downtime(&bad, 10_000);
        (s, good, meh, bad)
    }

    // ---- IncentiveCurve ----

    #[test]
    fn quadratic_curve_monotone_in_score() {
        let c = QuadraticCurve::default();
        assert!(c.weight(0.9, 0.0) > c.weight(0.3, 0.0));
    }

    #[test]
    fn quadratic_curve_floor_applies_to_zero_inputs() {
        let c = QuadraticCurve::default();
        assert!(c.weight(0.0, 0.0) >= c.floor - f64::EPSILON);
    }

    #[test]
    fn quadratic_curve_clamps_out_of_range() {
        let c = QuadraticCurve::default();
        let a = c.weight(2.0, 2.0); // clamped to 1.0, 1.0
        let b = c.weight(1.0, 1.0);
        assert!((a - b).abs() < 1e-9);
    }

    #[test]
    fn quadratic_curve_quadratic_in_score() {
        let c = QuadraticCurve { score_coef: 1.0, stake_coef: 0.0, floor: 0.0 };
        // w(0.5) = 0.25, w(1.0) = 1.0 → ratio 4x, not 2x, proves quadratic term.
        let w1 = c.weight(0.5, 0.0);
        let w2 = c.weight(1.0, 0.0);
        assert!((w2 / w1 - 4.0).abs() < 1e-9);
    }

    // ---- SuperPeerSelector ----

    #[test]
    fn top_k_picks_highest_scoring() {
        let (s, good, meh, bad) = scorer_with_good_bad();
        let selector = SuperPeerSelector { scorer: &s };
        let picked = selector
            .top_k(&[good.clone(), meh.clone(), bad.clone()], 2)
            .unwrap();
        assert_eq!(picked.len(), 2);
        assert_eq!(picked[0], good);
        // The second slot should be meh, not bad.
        assert_eq!(picked[1], meh);
    }

    #[test]
    fn top_k_of_one_returns_best() {
        let (s, good, meh, bad) = scorer_with_good_bad();
        let selector = SuperPeerSelector { scorer: &s };
        let picked = selector.top_k(&[good.clone(), meh, bad], 1).unwrap();
        assert_eq!(picked, vec![good]);
    }

    #[test]
    fn top_k_zero_rejected() {
        let s = InMemoryScorer::new(ScoreWeights::default(), 500);
        let selector = SuperPeerSelector { scorer: &s };
        assert!(matches!(selector.top_k(&[node()], 0), Err(IncentiveError::ZeroK)));
    }

    #[test]
    fn top_k_no_candidates_rejected() {
        let s = InMemoryScorer::new(ScoreWeights::default(), 500);
        let selector = SuperPeerSelector { scorer: &s };
        assert!(matches!(
            selector.top_k(&[], 1),
            Err(IncentiveError::NoCandidates)
        ));
    }

    #[test]
    fn top_k_larger_than_population_returns_all() {
        let (s, good, meh, bad) = scorer_with_good_bad();
        let selector = SuperPeerSelector { scorer: &s };
        let picked = selector.top_k(&[good, meh, bad], 10).unwrap();
        assert_eq!(picked.len(), 3);
    }

    // ---- RewardDistributor ----

    #[test]
    fn distribute_allocates_by_weight() {
        let (s, good, meh, bad) = scorer_with_good_bad();
        let ledger = Arc::new(InMemoryLedger::new());
        let rd = RewardDistributor::new(
            QuadraticCurve::default(),
            ledger.clone(),
            EmissionCurve::new(1_000_000, 100),
        );
        let peers = vec![good.clone(), meh.clone(), bad.clone()];
        let scores: Vec<f64> = peers.iter().map(|p| s.score(p)).collect();
        let dist = rd
            .distribute(
                0,
                &peers,
                |p| s.score(p),
                |_| 0.0,
            )
            .unwrap();
        assert_eq!(dist.total_reward, 1_000_000);
        assert_eq!(dist.per_peer.len(), 3);
        // Good should get the most; bad the least.
        let good_share = dist.per_peer.iter().find(|(p, _)| *p == good).unwrap().1;
        let meh_share = dist.per_peer.iter().find(|(p, _)| *p == meh).unwrap().1;
        let bad_share = dist.per_peer.iter().find(|(p, _)| *p == bad).unwrap().1;
        assert!(good_share >= meh_share);
        assert!(meh_share >= bad_share);
        assert!(good_share > bad_share);
        // Sum (minted) + residue == total_reward.
        let minted: Amount = dist.per_peer.iter().map(|(_, a)| *a).sum();
        assert_eq!(minted + dist.residue_burned, dist.total_reward);
        // Sanity: scores are at least set.
        assert!(scores.iter().all(|&x| (0.0..=1.0).contains(&x)));
    }

    #[test]
    fn distribute_credits_ledger() {
        let ledger = Arc::new(InMemoryLedger::new());
        let rd = RewardDistributor::new(
            QuadraticCurve::default(),
            ledger.clone(),
            EmissionCurve::new(1000, 100),
        );
        let a = node();
        let b = node();
        rd.distribute(0, &[a.clone(), b.clone()], |_| 1.0, |_| 0.0).unwrap();
        assert!(ledger.balance(&a) > 0);
        assert!(ledger.balance(&b) > 0);
        // Total minted should match total emission (up to residue).
        assert!(ledger.total_supply() <= 1000);
        assert!(ledger.total_supply() >= 998);
    }

    #[test]
    fn distribute_no_peers_rejected() {
        let ledger = Arc::new(InMemoryLedger::new());
        let rd = RewardDistributor::new(
            QuadraticCurve::default(),
            ledger,
            EmissionCurve::new(1000, 100),
        );
        let err = rd.distribute(0, &[], |_| 1.0, |_| 0.0).unwrap_err();
        assert!(matches!(err, IncentiveError::NoCandidates));
    }

    #[test]
    fn distribute_zero_emission_epoch_gives_zero_rewards() {
        let ledger = Arc::new(InMemoryLedger::new());
        // halving every 1 epoch → by epoch 128 reward is zero (saturated).
        let rd = RewardDistributor::new(
            QuadraticCurve::default(),
            ledger.clone(),
            EmissionCurve::new(1000, 1),
        );
        let a = node();
        let dist = rd.distribute(1000, &[a.clone()], |_| 1.0, |_| 0.0).unwrap();
        assert_eq!(dist.total_reward, 0);
        assert_eq!(ledger.balance(&a), 0);
    }

    #[test]
    fn distribute_all_zero_weight_splits_evenly() {
        let ledger = Arc::new(InMemoryLedger::new());
        let degenerate = QuadraticCurve { score_coef: 0.0, stake_coef: 0.0, floor: 0.0 };
        let rd = RewardDistributor::new(
            degenerate,
            ledger.clone(),
            EmissionCurve::new(100, 100),
        );
        let a = node();
        let b = node();
        let dist = rd
            .distribute(0, &[a.clone(), b.clone()], |_| 1.0, |_| 1.0)
            .unwrap();
        let ashr = dist.per_peer.iter().find(|(p, _)| *p == a).unwrap().1;
        let bshr = dist.per_peer.iter().find(|(p, _)| *p == b).unwrap().1;
        assert_eq!(ashr, bshr);
    }

    #[test]
    fn stake_bonus_shifts_allocation() {
        let ledger = Arc::new(InMemoryLedger::new());
        let rd = RewardDistributor::new(
            QuadraticCurve::default(),
            ledger,
            EmissionCurve::new(10_000, 100),
        );
        let a = node();
        let b = node();
        // Same score for both, but a has stake_bonus=1.0, b has 0.0.
        let dist = rd
            .distribute(
                0,
                &[a.clone(), b.clone()],
                |_| 0.5,
                |p| if *p == a { 1.0 } else { 0.0 },
            )
            .unwrap();
        let ashr = dist.per_peer.iter().find(|(p, _)| *p == a).unwrap().1;
        let bshr = dist.per_peer.iter().find(|(p, _)| *p == b).unwrap().1;
        assert!(ashr > bshr);
    }

    #[test]
    fn selector_is_deterministic_on_ties() {
        // Two nodes with identical scores → sort by NodeId for determinism.
        let s = InMemoryScorer::new(ScoreWeights::default(), 500);
        let p1 = node();
        let p2 = node();
        for _ in 0..5 {
            s.record_success(&p1, 50);
            s.record_success(&p2, 50);
        }
        let selector = SuperPeerSelector { scorer: &s };
        let a = selector.top_k(&[p1.clone(), p2.clone()], 2).unwrap();
        let b = selector.top_k(&[p2.clone(), p1.clone()], 2).unwrap();
        assert_eq!(a, b);
    }
}
