// KG: seed-rts-bft-checkpoint-not-per-frame-2026-04-15
// KG: sprint4D-ed25519-real-verify-2026-04-15
//! FrameDivergenceDetector — wraps `DesyncDetector` with two-layer coordination.
//!
//! # Responsibilities
//! 1. **Fast layer**: record local frame hashes; on peer hash arrival, detect mismatch
//!    and request `rollback_on_desync`.
//! 2. **Slow layer**: when a BFT-signed checkpoint arrives, verify its QC signature
//!    with real Ed25519 via `ValidatorKeyring` and mark that frame as the new rollback floor.
//!
//! # BFT signature verification
//! `verify_bft_signature` performs real Ed25519 multi-signature verification using
//! `ValidatorKeyring` (injected via `with_keyring`) and `ValidatorSet` for quorum check.
//! Without a keyring (default construction), genesis QC (empty sigs, block_hash=0) is
//! accepted and all other QCs are rejected — safe fail-closed default.

use crate::determinism::state_hash::{DesyncDetector, DesyncEvent};
use crate::bft::crypto::{phase_tag, verify_vote, QuorumCert, ValidatorKeyring, ValidatorSet};
use crate::bft::types::Phase;
use crate::observability; // KG: sprint6C-observability-2026-04-15

/// Coordinates fast-layer desync detection with slow-layer BFT checkpoint validation.
///
/// KG: seed-rts-bft-checkpoint-not-per-frame-2026-04-15
/// KG: sprint4D-ed25519-real-verify-2026-04-15
pub struct FrameDivergenceDetector {
    /// Underlying desync detector from the determinism layer.
    inner: DesyncDetector,
    /// Latest BFT-blessed checkpoint frame.
    bft_floor: u64,
    /// Hash at `bft_floor` (from the last verified QC).
    bft_floor_hash: [u8; 32],
    /// Pending desync events awaiting rollback handling.
    pending_desyncs: Vec<DesyncEvent>,
    /// Keyring for Ed25519 signature verification (None = no keyring, fail-closed).
    keyring: Option<ValidatorKeyring>,
    /// Validator set for quorum-size computation (None = no set, fail-closed).
    validator_set: Option<ValidatorSet>,
}

impl FrameDivergenceDetector {
    /// Create a new detector. `retention` = how many frames of local hashes to keep.
    pub fn new(retention: usize) -> Self {
        Self {
            inner: DesyncDetector::new(retention),
            bft_floor: 0,
            bft_floor_hash: [0u8; 32],
            pending_desyncs: Vec::new(),
            keyring: None,
            validator_set: None,
        }
    }

    /// Inject a `ValidatorKeyring` and `ValidatorSet` for real Ed25519 QC verification.
    ///
    /// Must be called before accepting non-genesis BFT checkpoints in production.
    /// Without calling this, only genesis QC (block_hash=0, no signatures) is accepted.
    ///
    /// KG: sprint4D-ed25519-real-verify-2026-04-15
    pub fn with_keyring(mut self, keyring: ValidatorKeyring, vs: ValidatorSet) -> Self {
        self.keyring = Some(keyring);
        self.validator_set = Some(vs);
        self
    }

    // ── Fast layer interface ──────────────────────────────────────────────────

    /// Record the local state hash for `frame_n`.
    /// Called by `FastLockstepLoop::advance_frame` when `should_broadcast_hash` is true.
    pub fn record_local(&mut self, frame_n: u64, hash: [u8; 32]) {
        self.inner.record_local(frame_n, hash);
    }

    /// Observe a peer's hash at `frame_n`.
    /// Returns `Some(DesyncEvent)` if a mismatch is detected (enqueued in `pending_desyncs`).
    pub fn observe_peer(
        &mut self,
        peer_id: u32,
        frame_n: u64,
        peer_hash: [u8; 32],
    ) -> Option<DesyncEvent> {
        let event = self.inner.observe_peer(peer_id, frame_n, peer_hash);
        if let Some(ref ev) = event {
            self.pending_desyncs.push(ev.clone());
            // KG: sprint6C-observability-2026-04-15
            observability::RTS_DESYNC_EVENTS_TOTAL.inc();
        }
        event
    }

    /// Drain all pending desync events. Caller (`FastLockstepLoop`) processes them.
    pub fn drain_desyncs(&mut self) -> Vec<DesyncEvent> {
        std::mem::take(&mut self.pending_desyncs)
    }

    // ── Slow layer interface ──────────────────────────────────────────────────

    /// Accept a BFT checkpoint: verify its QC (stub) and advance the rollback floor.
    ///
    /// # Stub note
    /// `verify_bft_signature` is a placeholder. Replace with:
    /// `SlowBftCheckpoint::verify_peer_checkpoint(&handle, &qc)` which delegates to
    /// `BftCheckpointProvider::verify_checkpoint_qc`.
    ///
    /// KG: seed-rts-bft-checkpoint-not-per-frame-2026-04-15
    pub fn accept_bft_checkpoint(
        &mut self,
        frame_n: u64,
        tactical_hash: [u8; 32],
        qc: &QuorumCert,
    ) -> Result<(), String> {
        if !self.verify_bft_signature(frame_n, tactical_hash, qc) {
            return Err(format!(
                "BFT QC verification failed for frame {} — Byzantine node suspected",
                frame_n
            ));
        }
        if frame_n < self.bft_floor {
            return Err(format!(
                "checkpoint frame {} is older than current BFT floor {}",
                frame_n, self.bft_floor
            ));
        }
        self.bft_floor = frame_n;
        self.bft_floor_hash = tactical_hash;
        Ok(())
    }

    /// Returns the current BFT floor frame (rollback cannot go below this).
    pub fn bft_floor(&self) -> u64 {
        self.bft_floor
    }

    /// Returns the hash at the BFT floor.
    pub fn bft_floor_hash(&self) -> [u8; 32] {
        self.bft_floor_hash
    }

    /// Number of stored local hashes (for diagnostics).
    pub fn stored_count(&self) -> usize {
        self.inner.stored_count()
    }

    // ── Stub: BFT signature verification ─────────────────────────────────────

    /// Real Ed25519 BFT QC verifier.
    ///
    /// Verifies that `qc` carries at least `quorum` valid Ed25519 signatures from
    /// registered validators (via `ValidatorKeyring`).
    ///
    /// Special case: genesis QC (`block_hash=0`, `view=0`, `signatures=[]`) is
    /// accepted unconditionally as a trusted root (no key material required).
    ///
    /// Without a keyring injected via `with_keyring()`, all non-genesis QCs are
    /// rejected (fail-closed default).
    ///
    /// KG: sprint4D-ed25519-real-verify-2026-04-15
    fn verify_bft_signature(
        &self,
        _frame_n: u64,
        _tactical_hash: [u8; 32],
        qc: &QuorumCert,
    ) -> bool {
        // Genesis QC is always accepted as the trusted initial checkpoint.
        if qc.block_hash == 0 && qc.view == 0 && qc.signatures.is_empty() {
            return true;
        }

        // Without a keyring we cannot verify — fail closed.
        let keyring = match &self.keyring {
            Some(k) => k,
            None => return false,
        };

        // Must have at least one signature.
        if qc.signatures.is_empty() {
            return false;
        }

        // Check quorum count if we have a validator set.
        if let Some(vs) = &self.validator_set {
            if !qc.has_quorum(vs.n()) {
                return false;
            }
        }

        // A BFT checkpoint QC is proof of a COMMIT — the QCs peers ship here are
        // the leader's post-Decide high_qc, formed from Commit-phase votes. A
        // Prepare/PreCommit QC must not raise the rollback floor: 2f+1 captured
        // Prepare votes are not evidence that anything committed.
        // # KG: fix-333-bft-vote-signature-phase-binding-2026-07-15
        if qc.phase != Phase::Commit {
            return false;
        }

        // Ed25519 verify each signature against the QC's block_hash, bound to
        // the phase the QC's votes were cast in (Commit, enforced above).
        // Any invalid sig → reject the entire QC.
        let block_hash = qc.block_hash;
        for sig in &qc.signatures {
            if !verify_vote(sig, block_hash, phase_tag(qc.phase), keyring) {
                return false;
            }
        }

        true
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bft::crypto::QuorumCert;

    fn genesis_qc() -> QuorumCert {
        QuorumCert::genesis()
    }

    #[test]
    fn no_desync_when_hashes_match() {
        let mut det = FrameDivergenceDetector::new(32);
        let hash = [0x11u8; 32];
        det.record_local(10, hash);
        let ev = det.observe_peer(1, 10, hash);
        assert!(ev.is_none(), "matching hashes must not trigger desync");
    }

    #[test]
    fn desync_detected_on_mismatch() {
        let mut det = FrameDivergenceDetector::new(32);
        det.record_local(20, [0xAAu8; 32]);
        let ev = det.observe_peer(2, 20, [0xBBu8; 32]);
        assert!(ev.is_some(), "hash mismatch must trigger desync event");
        let events = det.drain_desyncs();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].frame_n, 20);
    }

    #[test]
    fn bft_checkpoint_accepted_and_floor_advances() {
        let mut det = FrameDivergenceDetector::new(32);
        assert_eq!(det.bft_floor(), 0);
        let result = det.accept_bft_checkpoint(1000, [0xCC; 32], &genesis_qc());
        assert!(result.is_ok());
        assert_eq!(det.bft_floor(), 1000);
        assert_eq!(det.bft_floor_hash(), [0xCC; 32]);
    }

    #[test]
    fn bft_checkpoint_rejected_for_older_frame() {
        let mut det = FrameDivergenceDetector::new(32);
        det.accept_bft_checkpoint(1000, [0x01; 32], &genesis_qc()).unwrap();
        let result = det.accept_bft_checkpoint(500, [0x02; 32], &genesis_qc());
        assert!(result.is_err(), "older checkpoint must be rejected");
    }

    #[test]
    fn drain_desyncs_clears_queue() {
        let mut det = FrameDivergenceDetector::new(32);
        det.record_local(10, [0x01; 32]);
        det.observe_peer(1, 10, [0x02; 32]);
        det.record_local(20, [0x03; 32]);
        det.observe_peer(2, 20, [0x04; 32]);
        let events = det.drain_desyncs();
        assert_eq!(events.len(), 2);
        // After drain, queue is empty
        assert!(det.drain_desyncs().is_empty());
    }

    // ── Test: Byzantine peer forged QC is rejected ────────────────────────────

    /// A Byzantine peer sends a QC with a non-empty forged signature.
    /// Without a keyring it is fail-closed → rejected.
    /// With a keyring the forged signature fails Ed25519 → rejected.
    ///
    /// KG: sprint4D-ed25519-real-verify-2026-04-15
    #[test]
    fn byzantine_peer_forged_qc_rejected() {
        use crate::bft::crypto::{Signature, ValidatorKeyring, ValidatorSet};
        use crate::crypto_real::Identity;

        // No keyring — any non-genesis QC is fail-closed rejected.
        let mut det = FrameDivergenceDetector::new(32);

        let forged_qc = QuorumCert {
            block_hash: 0xBAD_F00D,
            view: 1,
            phase: Phase::Commit,
            signatures: vec![Signature { signer: 99, sig_bytes: vec![0u8; 64] }],
        };
        let result = det.accept_bft_checkpoint(500, [0x01; 32], &forged_qc);
        assert!(result.is_err(), "forged QC without keyring must be rejected (fail-closed)");

        // With a keyring but the forged sig_bytes don't match real Ed25519 → rejected.
        let mut kr = ValidatorKeyring::new();
        let identity = {
            let mut seed = [0u8; 32];
            seed[0] = 99;
            Identity::from_seed(&seed)
        };
        kr.register_identity(99, identity);
        let vs = ValidatorSet::new(vec![99]);

        let mut det2 = FrameDivergenceDetector::new(32).with_keyring(kr, vs);
        let result2 = det2.accept_bft_checkpoint(500, [0x01; 32], &forged_qc);
        assert!(result2.is_err(), "forged sig_bytes must fail Ed25519 verify → rejected");
    }

    // ── Test: real multi-sig QC accepted via with_keyring ────────────────────

    /// Build a real 3-of-4 QC and confirm accept_bft_checkpoint passes.
    ///
    /// KG: sprint4D-ed25519-real-verify-2026-04-15
    #[test]
    fn real_multisig_qc_accepted_with_keyring() {
        use crate::bft::crypto::{phase_tag, sign_vote, ValidatorKeyring, ValidatorSet};
        use crate::crypto_real::Identity;

        fn det_identity(nid: u32) -> Identity {
            let mut seed = [0u8; 32];
            seed[..4].copy_from_slice(&nid.to_be_bytes());
            Identity::from_seed(&seed)
        }

        let node_ids: Vec<u32> = vec![1, 2, 3, 4];
        let vs = ValidatorSet::new(node_ids.clone());
        let mut kr = ValidatorKeyring::new();
        for &nid in &node_ids {
            kr.register_identity(nid, det_identity(nid));
        }

        let block_hash: u64 = 0xFEED_FACE_CAFE_BABE;
        // 3 valid Commit-phase-bound signatures — satisfies quorum=3 for n=4.
        // Checkpoint QCs are Commit QCs, so honest test sigs bind the Commit tag.
        let signatures: Vec<_> = [1u32, 2, 3]
            .iter()
            .map(|&nid| sign_vote(nid, block_hash, phase_tag(Phase::Commit), &det_identity(nid)))
            .collect();
        let valid_qc = QuorumCert { block_hash, view: 1, phase: Phase::Commit, signatures };

        let mut det = FrameDivergenceDetector::new(32).with_keyring(kr, vs);
        let result = det.accept_bft_checkpoint(999, [0x42; 32], &valid_qc);
        assert!(result.is_ok(), "real 3-of-4 QC must be accepted: {:?}", result.err());
        assert_eq!(det.bft_floor(), 999);
    }
}
