// KG: seed-rts-verify-bft-vote-peer-eject-2026-04-15
//! BFT-backed peer eject. AFK timeout → background BFT vote → null-input replay.
//! Non-blocking: fast lockstep continues while vote is in flight.
//! See docs/PEER_EJECT_PROTOCOL.md for AoE2/SC2 comparison and tuning guide.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
use crate::observability; // KG: sprint6C-observability-2026-04-15

/// Opaque peer identifier (u32 matches NodeId in bft::crypto).
// KG: seed-rts-verify-bft-vote-peer-eject-2026-04-15
pub type PeerId = u32;

/// Minimal game-input — null means "do nothing this frame".
// KG: seed-rts-verify-bft-vote-peer-eject-2026-04-15
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameInput {
    pub peer: PeerId,
    pub frame: u64,
    /// true → null / no-op; false → real input
    pub is_null: bool,
    pub payload: Vec<u8>,
}

impl GameInput {
    /// Deterministic null input — same (peer, frame) always produces identical bytes.
    pub fn null(peer: PeerId, frame: u64) -> Self {
        Self { peer, frame, is_null: true, payload: Vec::new() }
    }
}

/// In-flight eject vote for a single peer.
// KG: seed-rts-verify-bft-vote-peer-eject-2026-04-15
#[derive(Debug, Clone)]
pub struct EjectVote {
    pub target: PeerId,
    pub initiated_at: Instant,
    pub approvals: u32,
    pub rejections: u32,
}

/// AFK eject decision emitted by `tick()`.
// KG: seed-rts-verify-bft-vote-peer-eject-2026-04-15
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EjectDecision {
    pub peer: PeerId,
    /// Last known frame — anchors null-input replay.
    pub afk_since_frame: u64,
}

/// BFT vote–backed peer eject controller.
// KG: seed-rts-verify-bft-vote-peer-eject-2026-04-15
pub struct PeerEjectController {
    pub afk_timeout: Duration,
    pub last_input_seen: BTreeMap<PeerId, Instant>,
    last_frame: BTreeMap<PeerId, u64>,
    /// One vote at a time — keeps BFT traffic minimal.
    pub pending_vote: Option<EjectVote>,
    ejected: BTreeSet<PeerId>,
}

impl PeerEjectController {
    /// 500 ms (fast) / 1 s (standard) / 2 s (lenient). See PEER_EJECT_PROTOCOL.md.
    pub fn new(afk_timeout: Duration) -> Self {
        Self {
            afk_timeout,
            last_input_seen: BTreeMap::new(),
            last_frame: BTreeMap::new(),
            pending_vote: None,
            ejected: BTreeSet::new(),
        }
    }

    /// Record real input from `peer` at `frame`. Late-but-valid → cancels pending vote.
    pub fn record_input(&mut self, peer: PeerId, frame: u64) {
        self.last_input_seen.insert(peer, Instant::now());
        self.last_frame.insert(peer, frame);
        if matches!(&self.pending_vote, Some(v) if v.target == peer) {
            self.pending_vote = None;
        }
    }

    /// Poll for AFK peers (call once per game tick). Already-ejected excluded.
    pub fn tick(&self, now: Instant) -> Vec<EjectDecision> {
        self.last_input_seen.iter()
            .filter(|(&peer, &last)| {
                !self.ejected.contains(&peer)
                    && now.duration_since(last) > self.afk_timeout
            })
            .map(|(&peer, _)| EjectDecision {
                peer,
                afk_since_frame: self.last_frame.get(&peer).copied().unwrap_or(0),
            })
            .collect()
    }

    /// Enqueue BFT eject vote for `peer` (stub — wires to ConsensusKick OrderedTx).
    pub fn start_eject_vote(&mut self, peer: PeerId) {
        if self.pending_vote.is_none() && !self.ejected.contains(&peer) {
            self.pending_vote = Some(EjectVote {
                target: peer,
                initiated_at: Instant::now(),
                approvals: 0,
                rejections: 0,
            });
            // STUB: bft_transport.submit(OrderedTx::RankedAction { player: peer, … })
        }
    }

    /// Apply BFT vote result. `approved` → eject; `!approved` → retain.
    pub fn apply_vote_result(&mut self, peer: PeerId, approved: bool) {
        if matches!(&self.pending_vote, Some(v) if v.target == peer) {
            self.pending_vote = None;
        }
        if approved {
            self.ejected.insert(peer);
            // KG: sprint6C-observability-2026-04-15
            observability::RTS_PEER_EJECTED_TOTAL.inc();
        }
    }

    /// Pure null input — deterministic; same (peer, frame) → identical bytes.
    pub fn null_input_for_ejected(&self, peer: PeerId, frame: u64) -> GameInput {
        GameInput::null(peer, frame)
    }

    pub fn is_ejected(&self, peer: PeerId) -> bool {
        self.ejected.contains(&peer)
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// KG: seed-rts-verify-bft-vote-peer-eject-2026-04-15
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const TIMEOUT: Duration = Duration::from_millis(500);

    fn make_ctrl() -> PeerEjectController {
        PeerEjectController::new(TIMEOUT)
    }

    // Test 1 — AFK detection accuracy: within timeout → no decision; past it → decision.
    #[test]
    fn afk_detection_within_and_beyond_timeout() {
        let mut ctrl = make_ctrl();
        let t0 = Instant::now();
        ctrl.record_input(1, 10);
        ctrl.last_input_seen.insert(1, t0);

        // Just inside timeout → no decision
        let just_inside = t0 + Duration::from_millis(499);
        assert!(ctrl.tick(just_inside).is_empty(), "should not trigger before timeout");

        // Just outside timeout → decision
        let just_outside = t0 + Duration::from_millis(501);
        let decisions = ctrl.tick(just_outside);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].peer, 1);
        assert_eq!(decisions[0].afk_since_frame, 10);
    }

    // Test 2 — Two peers AFK simultaneously both appear in decisions.
    #[test]
    fn two_peers_afk_simultaneously() {
        let mut ctrl = make_ctrl();
        let t0 = Instant::now();
        ctrl.record_input(1, 5);
        ctrl.record_input(2, 7);
        ctrl.last_input_seen.insert(1, t0);
        ctrl.last_input_seen.insert(2, t0);

        let later = t0 + Duration::from_millis(600);
        let mut decisions = ctrl.tick(later);
        decisions.sort_by_key(|d| d.peer);
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].peer, 1);
        assert_eq!(decisions[1].peer, 2);
    }

    // Test 3 — vote approved → ejected; vote rejected → retained.
    #[test]
    fn vote_approved_ejects_peer_vote_rejected_retains() {
        let mut ctrl = make_ctrl();
        ctrl.record_input(42, 1);

        // Simulate vote for peer 42 — approved
        ctrl.start_eject_vote(42);
        assert!(ctrl.pending_vote.is_some());
        ctrl.apply_vote_result(42, true);
        assert!(ctrl.is_ejected(42), "approved vote must eject peer");
        assert!(ctrl.pending_vote.is_none());

        // Peer 99 — rejected
        ctrl.record_input(99, 2);
        ctrl.start_eject_vote(99);
        ctrl.apply_vote_result(99, false);
        assert!(!ctrl.is_ejected(99), "rejected vote must retain peer");
    }

    // Test 4 — false positive: late but valid input arrives → pending vote cancelled.
    #[test]
    fn late_valid_input_cancels_pending_vote() {
        let mut ctrl = make_ctrl();
        let t0 = Instant::now();
        ctrl.record_input(7, 3);
        ctrl.last_input_seen.insert(7, t0);

        // AFK detected, vote started
        let later = t0 + Duration::from_millis(600);
        let decisions = ctrl.tick(later);
        assert!(!decisions.is_empty());
        ctrl.start_eject_vote(7);
        assert!(ctrl.pending_vote.is_some());

        // Late but valid input arrives — vote must be cancelled
        ctrl.record_input(7, 4);
        assert!(ctrl.pending_vote.is_none(), "late valid input must cancel vote");
        assert!(!ctrl.is_ejected(7), "peer must not be ejected after valid input");
    }

    // Test 5 — null-input replay determinism: two calls → identical GameInput.
    #[test]
    fn null_input_replay_is_deterministic() {
        let ctrl = make_ctrl();
        let peer = 55u32;
        let frame = 100u64;
        let input_a = ctrl.null_input_for_ejected(peer, frame);
        let input_b = ctrl.null_input_for_ejected(peer, frame);
        assert_eq!(input_a, input_b, "null_input_for_ejected must be deterministic");
        assert!(input_a.is_null);
        assert_eq!(input_a.peer, peer);
        assert_eq!(input_a.frame, frame);
        assert!(input_a.payload.is_empty());
    }
}
