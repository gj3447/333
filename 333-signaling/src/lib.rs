// KG: SPAN_333_L12plus_Signaling, plan-333-p2p-os-synthesis-execution-2026-04-18,
//     queue-p9-signaling-mesh-2026-04-18, seed-333-gossipsub-control-plane-2026-04-17
//
// 333 P2P OS L12+ Signaling — WebRTC signaling mesh over Plumtree gossip.
//
// Design sources:
//   - seed-333-gossipsub-control-plane-2026-04-17: libp2p GossipSub v1.1 topic
//     management (/333/offer, /333/answer, /333/ice), 7-param peer scoring.
//   - finding_333_synth_rte_d{12..15}: Plumtree + HyParView + PeerScorer.
//
// Topics are strings ("/333/offer", etc). Each envelope is signed by its
// sender so receivers reject impersonation without trusting the transport.
// PeerScorer tracks 7 GossipSub-inspired metrics: time-in-mesh,
// first-message-deliveries, mesh-message-deliveries, mesh-failure-penalty,
// invalid-message-deliveries, app-specific-score, behaviour-penalty.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use identity333::{Keypair, NodeId, Signature};
use thiserror::Error;

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum SignalingError {
    #[error("unknown topic: {0}")]
    UnknownTopic(String),
    #[error("signature invalid")]
    BadSignature,
    #[error("peer below score threshold: {0:?}")]
    PeerBanned(NodeId),
    #[error("invalid envelope: {0}")]
    Invalid(String),
}

// ============================================================================
// Topics
// ============================================================================

/// Well-known signaling topics. Free-form topics are also allowed via
/// `Topic::Custom` — the mesh itself is topic-agnostic.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Topic {
    Offer,
    Answer,
    Ice,
    Custom(String),
}

impl Topic {
    pub fn as_str(&self) -> &str {
        match self {
            Topic::Offer => "/333/offer",
            Topic::Answer => "/333/answer",
            Topic::Ice => "/333/ice",
            Topic::Custom(s) => s.as_str(),
        }
    }

    pub fn from_str(s: &str) -> Topic {
        match s {
            "/333/offer" => Topic::Offer,
            "/333/answer" => Topic::Answer,
            "/333/ice" => Topic::Ice,
            other => Topic::Custom(other.to_string()),
        }
    }
}

// ============================================================================
// Envelope — signed signaling payload
// ============================================================================

#[derive(Debug, Clone)]
pub struct Envelope {
    pub topic: Topic,
    pub from: NodeId,
    /// Optional direct-addressee; if None, the envelope is topic-broadcast.
    pub to: Option<NodeId>,
    pub payload: Vec<u8>,
    pub seq: u64,
    pub sig: Signature,
}

impl Envelope {
    pub fn canonical_bytes(
        topic: &Topic,
        from: &NodeId,
        to: &Option<NodeId>,
        payload: &[u8],
        seq: u64,
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(128);
        out.extend_from_slice(topic.as_str().as_bytes());
        out.push(0);
        out.extend_from_slice(from.as_bytes());
        out.push(0);
        if let Some(t) = to {
            out.extend_from_slice(t.as_bytes());
        }
        out.push(0);
        out.extend_from_slice(&seq.to_be_bytes());
        out.push(0);
        out.extend_from_slice(payload);
        out
    }

    pub fn sign(
        kp: &Keypair,
        topic: Topic,
        to: Option<NodeId>,
        payload: Vec<u8>,
        seq: u64,
    ) -> Self {
        let from = kp.node_id();
        let msg = Self::canonical_bytes(&topic, &from, &to, &payload, seq);
        let sig = kp.sign(&msg);
        Self { topic, from, to, payload, seq, sig }
    }

    pub fn verify(&self) -> Result<(), SignalingError> {
        let msg = Self::canonical_bytes(&self.topic, &self.from, &self.to, &self.payload, self.seq);
        Keypair::verify(self.from.as_bytes(), &msg, &self.sig)
            .map_err(|_| SignalingError::BadSignature)
    }
}

// ============================================================================
// PeerScorer — 7 GossipSub v1.1 parameters
// ============================================================================

#[derive(Debug, Clone, Copy)]
pub struct ScoreParams {
    /// Weight for "time in mesh" (positive per tick).
    pub time_in_mesh_weight: f64,
    pub time_in_mesh_cap: f64,
    /// Weight for first-to-forward an unseen message.
    pub first_delivery_weight: f64,
    pub first_delivery_cap: f64,
    /// Weight for mesh message deliveries (rate shaped).
    pub mesh_delivery_weight: f64,
    pub mesh_delivery_cap: f64,
    /// Negative weight for mesh failure (sent PRUNE, abandoned).
    pub mesh_failure_weight: f64,
    /// Strong negative weight for invalid (bad-sig) messages.
    pub invalid_msg_weight: f64,
    /// App-specific adjustment (e.g. reputation module input).
    pub app_specific_weight: f64,
    /// Generic behaviour penalty (e.g. flood, protocol abuse).
    pub behaviour_weight: f64,
    /// Peers scoring below this threshold are banned until their counters decay
    /// back above it. Recovery requires the embedder to drive [`PeerScorer::decay`];
    /// without it a ban is permanent, because the `publish` ban gate denies a
    /// banned peer any chance to earn positive score back.
    pub ban_threshold: f64,

    // -- Decay factors (GossipSub v1.1 `*Decay`) -----------------------------
    // Per-second multipliers applied by `PeerScorer::decay`. A counter c after
    // dt seconds is `c * factor.powf(dt)`. Factor 1.0 disables decay for that
    // counter; < 1.0 decays it toward zero. `time_in_mesh` and `app_specific`
    // are deliberately absent: GossipSub's P1 accrues while a peer stays in the
    // mesh, and P5 is owned by the application, not by this clock.
    pub first_delivery_decay: f64,
    pub mesh_delivery_decay: f64,
    pub mesh_failure_decay: f64,
    /// Slower than the positive counters — misbehaviour must outlive good deeds,
    /// or an attacker alternates spam and honest traffic to sit at threshold.
    pub invalid_msg_decay: f64,
    pub behaviour_decay: f64,
    /// Counters with magnitude below this snap to exactly 0, so a decayed peer
    /// reaches a clean state instead of an infinite asymptotic tail.
    pub decay_to_zero: f64,
}

impl Default for ScoreParams {
    fn default() -> Self {
        Self {
            time_in_mesh_weight: 0.01,
            time_in_mesh_cap: 10.0,
            first_delivery_weight: 1.0,
            first_delivery_cap: 100.0,
            mesh_delivery_weight: 0.5,
            mesh_delivery_cap: 50.0,
            mesh_failure_weight: -1.0,
            invalid_msg_weight: -10.0,
            app_specific_weight: 1.0,
            behaviour_weight: -2.0,
            ban_threshold: -50.0,
            first_delivery_decay: 0.99,
            mesh_delivery_decay: 0.99,
            mesh_failure_decay: 0.99,
            invalid_msg_decay: 0.995,
            behaviour_decay: 0.995,
            decay_to_zero: 0.01,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PeerStats {
    pub time_in_mesh: f64,
    pub first_deliveries: f64,
    pub mesh_deliveries: f64,
    pub mesh_failures: f64,
    pub invalid_messages: f64,
    pub app_specific: f64,
    pub behaviour_penalty: f64,
}

pub struct PeerScorer {
    params: ScoreParams,
    stats: Mutex<HashMap<NodeId, PeerStats>>,
}

impl PeerScorer {
    pub fn new(params: ScoreParams) -> Self {
        Self { params, stats: Mutex::new(HashMap::new()) }
    }

    pub fn observe_first_delivery(&self, peer: &NodeId) {
        self.mutate(peer, |s| s.first_deliveries += 1.0);
    }

    pub fn observe_mesh_delivery(&self, peer: &NodeId) {
        self.mutate(peer, |s| s.mesh_deliveries += 1.0);
    }

    pub fn observe_mesh_failure(&self, peer: &NodeId) {
        self.mutate(peer, |s| s.mesh_failures += 1.0);
    }

    pub fn observe_invalid(&self, peer: &NodeId) {
        self.mutate(peer, |s| s.invalid_messages += 1.0);
    }

    /// Accrue time-in-mesh for one peer. Time is supplied by the caller, never
    /// read from a clock, so a simulation and a live node score identically.
    ///
    /// This does not decay: an embedder advancing time runs `decay(dt)` once for
    /// the whole mesh and `tick(peer, dt)` only for peers currently in it.
    pub fn tick(&self, peer: &NodeId, dt_seconds: f64) {
        self.mutate(peer, |s| s.time_in_mesh += dt_seconds);
    }

    /// Decay every peer's decaying counters by `dt_seconds` (GossipSub v1.1
    /// `refreshScores`). This is the only path by which a banned peer regains
    /// standing, since the `publish` ban gate denies it any way to earn score.
    ///
    /// Caller-driven like [`tick`](Self::tick) — a deterministic core owns no
    /// clock. Reachable from the trait surface via `SignalingMesh::scorer()`.
    ///
    /// Continuous in `dt`: `decay(a)` then `decay(b)` equals `decay(a + b)` up to
    /// float rounding, so scores do not depend on how finely the embedder ticks.
    /// Non-positive `dt` is a no-op — a negative exponent would *inflate*
    /// penalties back into existence.
    pub fn decay(&self, dt_seconds: f64) {
        if !(dt_seconds > 0.0) {
            return;
        }
        let p = &self.params;
        let mut g = self.stats.lock().unwrap();
        for s in g.values_mut() {
            decay_counter(&mut s.first_deliveries, p.first_delivery_decay, dt_seconds, p.decay_to_zero);
            decay_counter(&mut s.mesh_deliveries, p.mesh_delivery_decay, dt_seconds, p.decay_to_zero);
            decay_counter(&mut s.mesh_failures, p.mesh_failure_decay, dt_seconds, p.decay_to_zero);
            decay_counter(&mut s.invalid_messages, p.invalid_msg_decay, dt_seconds, p.decay_to_zero);
            decay_counter(&mut s.behaviour_penalty, p.behaviour_decay, dt_seconds, p.decay_to_zero);
        }
    }

    pub fn set_app_specific(&self, peer: &NodeId, v: f64) {
        self.mutate(peer, |s| s.app_specific = v);
    }

    pub fn add_behaviour_penalty(&self, peer: &NodeId, v: f64) {
        self.mutate(peer, |s| s.behaviour_penalty += v);
    }

    pub fn score(&self, peer: &NodeId) -> f64 {
        let g = self.stats.lock().unwrap();
        let s = g.get(peer).copied().unwrap_or_default();
        let p = &self.params;
        let tim = (s.time_in_mesh * p.time_in_mesh_weight).min(p.time_in_mesh_cap);
        let fd = (s.first_deliveries * p.first_delivery_weight).min(p.first_delivery_cap);
        let md = (s.mesh_deliveries * p.mesh_delivery_weight).min(p.mesh_delivery_cap);
        let mf = s.mesh_failures * p.mesh_failure_weight;
        let iv = s.invalid_messages * p.invalid_msg_weight;
        let app = s.app_specific * p.app_specific_weight;
        let beh = s.behaviour_penalty * p.behaviour_weight;
        tim + fd + md + mf + iv + app + beh
    }

    pub fn is_banned(&self, peer: &NodeId) -> bool {
        self.score(peer) < self.params.ban_threshold
    }

    pub fn snapshot(&self) -> HashMap<NodeId, PeerStats> {
        self.stats.lock().unwrap().clone()
    }

    fn mutate(&self, peer: &NodeId, f: impl FnOnce(&mut PeerStats)) {
        let mut g = self.stats.lock().unwrap();
        let s = g.entry(peer.clone()).or_default();
        f(s);
    }
}

/// Counters are stored as unsigned magnitudes (negativity lives in the weights),
/// so `abs` guards only against an app writing a negative value directly.
fn decay_counter(c: &mut f64, factor: f64, dt_seconds: f64, decay_to_zero: f64) {
    *c *= factor.powf(dt_seconds);
    if c.abs() < decay_to_zero {
        *c = 0.0;
    }
}

// ============================================================================
// SignalingMesh — topic-based signed delivery
// ============================================================================

pub trait SignalingMesh: Send + Sync {
    fn subscribe(&self, topic: Topic) -> Result<(), SignalingError>;
    fn unsubscribe(&self, topic: &Topic) -> Result<(), SignalingError>;
    fn publish(&self, env: Envelope) -> Result<(), SignalingError>;
    /// Drain envelopes buffered for `topic`.
    fn drain(&self, topic: &Topic) -> Vec<Envelope>;
    fn scorer(&self) -> Arc<PeerScorer>;
}

pub struct InMemorySignalingMesh {
    me: NodeId,
    topics: Mutex<HashSet<Topic>>,
    inbox: Mutex<BTreeMap<String, VecDeque<Envelope>>>,
    /// seen[(topic, from, seq)] dedupes replays.
    seen: Mutex<HashSet<(String, NodeId, u64)>>,
    scorer: Arc<PeerScorer>,
}

impl InMemorySignalingMesh {
    pub fn new(me: NodeId, params: ScoreParams) -> Arc<Self> {
        Arc::new(Self {
            me,
            topics: Mutex::new(HashSet::new()),
            inbox: Mutex::new(BTreeMap::new()),
            seen: Mutex::new(HashSet::new()),
            scorer: Arc::new(PeerScorer::new(params)),
        })
    }

    pub fn node_id(&self) -> &NodeId {
        &self.me
    }

    fn topic_key(topic: &Topic) -> String {
        topic.as_str().to_string()
    }
}

impl SignalingMesh for InMemorySignalingMesh {
    fn subscribe(&self, topic: Topic) -> Result<(), SignalingError> {
        self.topics.lock().unwrap().insert(topic);
        Ok(())
    }

    fn unsubscribe(&self, topic: &Topic) -> Result<(), SignalingError> {
        self.topics.lock().unwrap().remove(topic);
        Ok(())
    }

    fn publish(&self, env: Envelope) -> Result<(), SignalingError> {
        // 1. Signature check — rejects forgeries before any side-effect.
        if env.verify().is_err() {
            self.scorer.observe_invalid(&env.from);
            return Err(SignalingError::BadSignature);
        }
        // 2. Banned-peer gate.
        if self.scorer.is_banned(&env.from) {
            return Err(SignalingError::PeerBanned(env.from.clone()));
        }
        // 3. Subscribed?
        let topic_key = Self::topic_key(&env.topic);
        if !self.topics.lock().unwrap().contains(&env.topic) {
            return Err(SignalingError::UnknownTopic(topic_key));
        }
        // 4. Direct-addressed to a different peer → drop silently.
        if let Some(to) = &env.to {
            if to != &self.me {
                return Ok(());
            }
        }
        // 5. Dedup.
        let k = (topic_key.clone(), env.from.clone(), env.seq);
        {
            let mut seen = self.seen.lock().unwrap();
            if seen.contains(&k) {
                return Ok(()); // duplicate — silent accept
            }
            seen.insert(k);
        }
        // 6. Deliver + scoring.
        self.scorer.observe_first_delivery(&env.from);
        self.inbox
            .lock()
            .unwrap()
            .entry(topic_key)
            .or_default()
            .push_back(env);
        Ok(())
    }

    fn drain(&self, topic: &Topic) -> Vec<Envelope> {
        let mut g = self.inbox.lock().unwrap();
        g.get_mut(&Self::topic_key(topic))
            .map(|q| q.drain(..).collect())
            .unwrap_or_default()
    }

    fn scorer(&self) -> Arc<PeerScorer> {
        self.scorer.clone()
    }
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn two_nodes() -> (Keypair, Keypair) {
        (Keypair::generate(), Keypair::generate())
    }

    fn mesh_for(kp: &Keypair) -> Arc<InMemorySignalingMesh> {
        InMemorySignalingMesh::new(kp.node_id(), ScoreParams::default())
    }

    #[test]
    fn topic_string_roundtrip() {
        assert_eq!(Topic::Offer.as_str(), "/333/offer");
        assert_eq!(Topic::from_str("/333/answer"), Topic::Answer);
        assert_eq!(Topic::from_str("/333/ice"), Topic::Ice);
        match Topic::from_str("/custom/x") {
            Topic::Custom(s) => assert_eq!(s, "/custom/x"),
            _ => panic!(),
        }
    }

    #[test]
    fn envelope_sign_verify_roundtrip() {
        let (a, _) = two_nodes();
        let env = Envelope::sign(&a, Topic::Offer, None, b"sdp=...".to_vec(), 1);
        env.verify().unwrap();
    }

    #[test]
    fn envelope_tampered_payload_fails_verify() {
        let (a, _) = two_nodes();
        let mut env = Envelope::sign(&a, Topic::Offer, None, b"sdp".to_vec(), 1);
        env.payload = b"evil".to_vec();
        assert!(matches!(env.verify(), Err(SignalingError::BadSignature)));
    }

    #[test]
    fn publish_requires_subscription() {
        let (a, b) = two_nodes();
        let mesh = mesh_for(&a);
        let env = Envelope::sign(&b, Topic::Offer, None, b"sdp".to_vec(), 1);
        assert!(matches!(
            mesh.publish(env),
            Err(SignalingError::UnknownTopic(_))
        ));
    }

    #[test]
    fn publish_delivers_on_subscribed_topic() {
        let (a, b) = two_nodes();
        let mesh = mesh_for(&a);
        mesh.subscribe(Topic::Offer).unwrap();
        let env = Envelope::sign(&b, Topic::Offer, None, b"sdp".to_vec(), 1);
        mesh.publish(env).unwrap();
        let got = mesh.drain(&Topic::Offer);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].payload, b"sdp");
    }

    #[test]
    fn bad_signature_rejected_and_scored() {
        let (a, b) = two_nodes();
        let mesh = mesh_for(&a);
        mesh.subscribe(Topic::Offer).unwrap();
        let mut env = Envelope::sign(&b, Topic::Offer, None, b"sdp".to_vec(), 1);
        env.payload = b"evil".to_vec();
        assert!(mesh.publish(env).is_err());
        // Invalid penalty applied.
        assert!(mesh.scorer().score(&b.node_id()) < 0.0);
    }

    #[test]
    fn direct_addressed_other_peer_silently_dropped() {
        let (a, b) = two_nodes();
        let (_, c) = two_nodes();
        let mesh = mesh_for(&a);
        mesh.subscribe(Topic::Answer).unwrap();
        // Envelope sent by b, but addressed to c — a should drop silently.
        let env = Envelope::sign(&b, Topic::Answer, Some(c.node_id()), b"ans".to_vec(), 1);
        mesh.publish(env).unwrap();
        assert!(mesh.drain(&Topic::Answer).is_empty());
    }

    #[test]
    fn direct_addressed_to_me_delivered() {
        let (a, b) = two_nodes();
        let mesh = mesh_for(&a);
        mesh.subscribe(Topic::Answer).unwrap();
        let env = Envelope::sign(&b, Topic::Answer, Some(a.node_id()), b"ans".to_vec(), 1);
        mesh.publish(env).unwrap();
        assert_eq!(mesh.drain(&Topic::Answer).len(), 1);
    }

    #[test]
    fn duplicate_seq_deduped() {
        let (a, b) = two_nodes();
        let mesh = mesh_for(&a);
        mesh.subscribe(Topic::Ice).unwrap();
        let env = Envelope::sign(&b, Topic::Ice, None, b"cand".to_vec(), 1);
        mesh.publish(env.clone()).unwrap();
        mesh.publish(env).unwrap(); // silent accept, no duplicate delivery
        assert_eq!(mesh.drain(&Topic::Ice).len(), 1);
    }

    #[test]
    fn unsubscribe_stops_delivery() {
        let (a, b) = two_nodes();
        let mesh = mesh_for(&a);
        mesh.subscribe(Topic::Offer).unwrap();
        mesh.unsubscribe(&Topic::Offer).unwrap();
        let env = Envelope::sign(&b, Topic::Offer, None, b"x".to_vec(), 1);
        assert!(mesh.publish(env).is_err());
    }

    #[test]
    fn scorer_accumulates_first_deliveries() {
        let scorer = PeerScorer::new(ScoreParams::default());
        let (_, b) = two_nodes();
        scorer.observe_first_delivery(&b.node_id());
        scorer.observe_first_delivery(&b.node_id());
        assert!(scorer.score(&b.node_id()) > 0.0);
    }

    #[test]
    fn scorer_time_in_mesh_capped() {
        let scorer = PeerScorer::new(ScoreParams::default());
        let (_, b) = two_nodes();
        scorer.tick(&b.node_id(), 100_000.0);
        // time_in_mesh cap = 10.0 at weight 0.01 → capped score contribution 10.0
        // so total score should remain bounded by cap.
        assert!(scorer.score(&b.node_id()) <= 10.0 + f64::EPSILON);
    }

    #[test]
    fn scorer_banning_blocks_publish() {
        let (a, b) = two_nodes();
        let mesh = mesh_for(&a);
        mesh.subscribe(Topic::Offer).unwrap();
        // Drive score below ban threshold via repeated invalid messages.
        for _ in 0..10 {
            mesh.scorer().observe_invalid(&b.node_id());
        }
        assert!(mesh.scorer().is_banned(&b.node_id()));
        let env = Envelope::sign(&b, Topic::Offer, None, b"x".to_vec(), 1);
        assert!(matches!(
            mesh.publish(env),
            Err(SignalingError::PeerBanned(_))
        ));
    }

    #[test]
    fn scorer_app_specific_and_behaviour_apply() {
        let scorer = PeerScorer::new(ScoreParams::default());
        let (_, b) = two_nodes();
        scorer.set_app_specific(&b.node_id(), 5.0);
        scorer.add_behaviour_penalty(&b.node_id(), 1.0);
        let s = scorer.score(&b.node_id());
        // app_specific_weight = 1.0, behaviour_weight = -2.0
        // → 5.0 * 1.0 + 1.0 * (-2.0) = 3.0
        assert!((s - 3.0).abs() < 1e-9);
    }

    #[test]
    fn three_topic_fanout_independent() {
        let (a, b) = two_nodes();
        let mesh = mesh_for(&a);
        mesh.subscribe(Topic::Offer).unwrap();
        mesh.subscribe(Topic::Answer).unwrap();
        mesh.subscribe(Topic::Ice).unwrap();
        mesh.publish(Envelope::sign(&b, Topic::Offer, None, b"o".to_vec(), 1)).unwrap();
        mesh.publish(Envelope::sign(&b, Topic::Answer, None, b"a".to_vec(), 2)).unwrap();
        mesh.publish(Envelope::sign(&b, Topic::Ice, None, b"i".to_vec(), 3)).unwrap();
        assert_eq!(mesh.drain(&Topic::Offer).len(), 1);
        assert_eq!(mesh.drain(&Topic::Answer).len(), 1);
        assert_eq!(mesh.drain(&Topic::Ice).len(), 1);
    }

    // -- Decay / ban recovery (S1) -------------------------------------------

    /// The defect this crate shipped with: 10 invalid messages → score -100, and
    /// no amount of time brought it back, because time_in_mesh caps at +10.
    #[test]
    fn banned_peer_recovers_once_decay_is_driven() {
        let (a, b) = two_nodes();
        let mesh = mesh_for(&a);
        mesh.subscribe(Topic::Offer).unwrap();
        for _ in 0..10 {
            mesh.scorer().observe_invalid(&b.node_id());
        }
        assert!(mesh.scorer().is_banned(&b.node_id()));

        // 0.995^t on 10 invalid msgs crosses the -50 threshold at t ≈ 138s.
        mesh.scorer().decay(200.0);

        assert!(
            !mesh.scorer().is_banned(&b.node_id()),
            "score {} should have decayed above ban_threshold",
            mesh.scorer().score(&b.node_id())
        );
        let env = Envelope::sign(&b, Topic::Offer, None, b"x".to_vec(), 1);
        mesh.publish(env).expect("recovered peer may publish again");
        assert_eq!(mesh.drain(&Topic::Offer).len(), 1);
    }

    /// Decay must not unban instantly, or the invalid-message penalty is theatre.
    #[test]
    fn decay_keeps_peer_banned_while_penalty_is_fresh() {
        let scorer = PeerScorer::new(ScoreParams::default());
        let (_, b) = two_nodes();
        for _ in 0..10 {
            scorer.observe_invalid(&b.node_id());
        }
        scorer.decay(10.0);
        assert!(scorer.is_banned(&b.node_id()));
    }

    #[test]
    fn decay_is_continuous_in_dt() {
        let (_, b) = two_nodes();
        let fine = PeerScorer::new(ScoreParams::default());
        let coarse = PeerScorer::new(ScoreParams::default());
        for s in [&fine, &coarse] {
            for _ in 0..10 {
                s.observe_invalid(&b.node_id());
            }
        }
        for _ in 0..60 {
            fine.decay(1.0);
        }
        coarse.decay(60.0);
        // Tolerance, not equality: powf rounding differs across step counts.
        assert!((fine.score(&b.node_id()) - coarse.score(&b.node_id())).abs() < 1e-6);
    }

    #[test]
    fn decay_snaps_to_zero_and_clears_ban_completely() {
        let scorer = PeerScorer::new(ScoreParams::default());
        let (_, b) = two_nodes();
        for _ in 0..10 {
            scorer.observe_invalid(&b.node_id());
        }
        scorer.decay(10_000.0);
        let s = scorer.snapshot().get(&b.node_id()).copied().unwrap();
        assert_eq!(s.invalid_messages, 0.0, "decay_to_zero must snap, not asymptote");
        assert_eq!(scorer.score(&b.node_id()), 0.0);
    }

    #[test]
    fn decay_ignores_non_positive_dt() {
        let scorer = PeerScorer::new(ScoreParams::default());
        let (_, b) = two_nodes();
        scorer.observe_invalid(&b.node_id());
        let before = scorer.score(&b.node_id());
        scorer.decay(0.0);
        scorer.decay(-1000.0);
        // A negative exponent would inflate the penalty instead of decaying it.
        assert_eq!(scorer.score(&b.node_id()), before);
    }

    #[test]
    fn decay_does_not_touch_time_in_mesh_or_app_specific() {
        let scorer = PeerScorer::new(ScoreParams::default());
        let (_, b) = two_nodes();
        scorer.tick(&b.node_id(), 500.0);
        scorer.set_app_specific(&b.node_id(), -7.0);
        scorer.decay(10_000.0);
        let s = scorer.snapshot().get(&b.node_id()).copied().unwrap();
        assert_eq!(s.time_in_mesh, 500.0, "P1 accrues while in mesh; clock must not erase it");
        assert_eq!(s.app_specific, -7.0, "P5 belongs to the app, not to decay");
    }

    /// An app-set score is not a decaying counter, so an app-driven ban stays
    /// put until the app itself lifts it. Pinning this so nobody "fixes" it.
    #[test]
    fn app_specific_ban_survives_decay() {
        let scorer = PeerScorer::new(ScoreParams::default());
        let (_, b) = two_nodes();
        scorer.set_app_specific(&b.node_id(), -1000.0);
        scorer.decay(1e6);
        assert!(scorer.is_banned(&b.node_id()));
    }

    #[test]
    fn decay_applies_to_every_peer_not_just_one() {
        let scorer = PeerScorer::new(ScoreParams::default());
        let (x, y) = two_nodes();
        for kp in [&x, &y] {
            for _ in 0..10 {
                scorer.observe_invalid(&kp.node_id());
            }
        }
        scorer.decay(200.0);
        assert!(!scorer.is_banned(&x.node_id()));
        assert!(!scorer.is_banned(&y.node_id()));
    }

    #[test]
    fn decay_erodes_positive_counters_too() {
        let scorer = PeerScorer::new(ScoreParams::default());
        let (_, b) = two_nodes();
        for _ in 0..20 {
            scorer.observe_first_delivery(&b.node_id());
        }
        let before = scorer.score(&b.node_id());
        scorer.decay(100.0);
        let after = scorer.score(&b.node_id());
        // Reputation is earned continuously, not banked forever.
        assert!(after < before && after > 0.0, "before {before}, after {after}");
    }

    #[test]
    fn scorer_snapshot_reflects_state() {
        let scorer = PeerScorer::new(ScoreParams::default());
        let (_, b) = two_nodes();
        scorer.observe_first_delivery(&b.node_id());
        scorer.observe_mesh_delivery(&b.node_id());
        let snap = scorer.snapshot();
        let s = snap.get(&b.node_id()).copied().unwrap();
        assert_eq!(s.first_deliveries, 1.0);
        assert_eq!(s.mesh_deliveries, 1.0);
    }
}
