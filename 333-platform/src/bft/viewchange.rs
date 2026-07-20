// KG: SPAN_333_BFT_ViewChange
//! View Change — timeout-based leader rotation
//!
//! When the current leader fails (no proposal within timeout):
//! 1. Validator sends ViewChange message with its high_qc
//! 2. When 2f+1 ViewChange messages collected for same new_view → new leader takes over
//! 3. New leader starts with the highest QC from the ViewChange messages
//!
//! This ensures liveness: even if a leader crashes, the protocol makes progress.

use super::crypto::{sign_with_identity, NodeId, QuorumCert, ValidatorSet};
use crate::crypto_real::Identity;
use super::types::*;
use std::collections::HashMap;
use std::time::Duration;

/// Milliseconds from a platform-appropriate clock.
///
/// `std::time::Instant::now()` panics on wasm32-unknown-unknown
/// (`std::sys::time::unsupported` → "time not implemented on this platform"),
/// which made `ViewChangeTracker::new` — and therefore the whole
/// `Platform333` constructor — abort in the browser. Mirrors `hlc::now_ms`.
///
/// Wall-clock, not monotonic: an NTP step can make a view time out early or
/// late. That costs a spurious view change (liveness hiccup), never safety —
/// the safety rule is `locked_qc`, not this timer.
///
/// # KG: lesson-333-viewchange-instant-panics-on-wasm-2026-07-15
fn now_ms() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

/// View change timeout configuration
#[derive(Debug, Clone)]
pub struct ViewChangeConfig {
    /// Base timeout for a view (doubled on each consecutive timeout)
    pub base_timeout: Duration,
    /// Maximum timeout cap
    pub max_timeout: Duration,
}

impl Default for ViewChangeConfig {
    fn default() -> Self {
        Self {
            base_timeout: Duration::from_secs(3),
            max_timeout: Duration::from_secs(30),
        }
    }
}

/// View change state tracker
#[derive(Debug)]
pub struct ViewChangeTracker {
    config: ViewChangeConfig,
    /// When the current view started (ms from `now_ms`)
    view_start_ms: u64,
    /// Current timeout duration (exponential backoff)
    current_timeout: Duration,
    /// Consecutive timeouts (for exponential backoff)
    consecutive_timeouts: u32,
    /// Collected ViewChange messages per view: new_view → (sender, high_qc)
    pending: HashMap<u64, Vec<(NodeId, QuorumCert)>>,
}

impl ViewChangeTracker {
    pub fn new(config: ViewChangeConfig) -> Self {
        Self {
            current_timeout: config.base_timeout,
            config,
            view_start_ms: now_ms(),
            consecutive_timeouts: 0,
            pending: HashMap::new(),
        }
    }

    /// Reset timer — call when entering a new view normally (no timeout)
    pub fn reset(&mut self) {
        self.view_start_ms = now_ms();
        self.consecutive_timeouts = 0;
        self.current_timeout = self.config.base_timeout;
        self.pending.clear();
    }

    /// Check if current view has timed out
    pub fn is_timed_out(&self) -> bool {
        let elapsed_ms = now_ms().saturating_sub(self.view_start_ms);
        elapsed_ms >= self.current_timeout.as_millis() as u64
    }

    /// Called when a timeout occurs — prepares ViewChange message
    pub fn on_timeout(&mut self, current_view: u64, node_id: NodeId, high_qc: QuorumCert, identity: &Identity) -> HotStuffMsg {
        self.consecutive_timeouts += 1;
        // Exponential backoff: 3s → 6s → 12s → 24s → 30s (capped)
        self.current_timeout = self.config.max_timeout.min(
            self.config.base_timeout * 2u32.saturating_pow(self.consecutive_timeouts),
        );
        self.view_start_ms = now_ms();

        HotStuffMsg::ViewChange {
            new_view: current_view + 1,
            sender: node_id,
            high_qc,
            signature: sign_with_identity(node_id, current_view + 1, identity),
        }
    }

    /// Collect a ViewChange message. Returns Some(new_view, highest_qc) when quorum reached.
    pub fn collect_view_change(
        &mut self,
        new_view: u64,
        sender: NodeId,
        high_qc: QuorumCert,
        validators: &ValidatorSet,
    ) -> Option<(u64, QuorumCert)> {
        let entries = self.pending.entry(new_view).or_default();

        // Deduplicate by sender
        if entries.iter().any(|(s, _)| *s == sender) {
            return None;
        }

        entries.push((sender, high_qc));

        // Check quorum
        if entries.len() >= validators.quorum_size() {
            // Find highest QC among all ViewChange messages
            let highest = entries
                .iter()
                .max_by_key(|(_, qc)| qc.view)
                .map(|(_, qc)| qc.clone())
                .unwrap();

            Some((new_view, highest))
        } else {
            None
        }
    }

    /// Current timeout duration (for monitoring)
    pub fn timeout_duration(&self) -> Duration {
        self.current_timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_id() -> Identity {
        let seed = [1u8; 32];
        Identity::from_seed(&seed)
    }

    fn make_validators() -> ValidatorSet {
        ValidatorSet::new(vec![1, 2, 3, 4, 5, 6, 7])
    }

    #[test]
    fn timeout_detection() {
        let config = ViewChangeConfig {
            base_timeout: Duration::from_millis(50),
            max_timeout: Duration::from_secs(1),
        };
        let tracker = ViewChangeTracker::new(config);

        assert!(!tracker.is_timed_out());

        // Wait for timeout
        std::thread::sleep(Duration::from_millis(60));
        assert!(tracker.is_timed_out());
    }

    #[test]
    fn exponential_backoff() {
        let config = ViewChangeConfig {
            base_timeout: Duration::from_secs(3),
            max_timeout: Duration::from_secs(30),
        };
        let mut tracker = ViewChangeTracker::new(config);
        let qc = QuorumCert::genesis();

        // First timeout: 3s → next will be 6s
        tracker.on_timeout(0, 1, qc.clone(), &test_id());
        assert_eq!(tracker.timeout_duration(), Duration::from_secs(6));

        // Second timeout: 6s → next 12s
        tracker.on_timeout(1, 1, qc.clone(), &test_id());
        assert_eq!(tracker.timeout_duration(), Duration::from_secs(12));

        // Third: 12 → 24
        tracker.on_timeout(2, 1, qc.clone(), &test_id());
        assert_eq!(tracker.timeout_duration(), Duration::from_secs(24));

        // Fourth: capped at 30
        tracker.on_timeout(3, 1, qc, &test_id());
        assert_eq!(tracker.timeout_duration(), Duration::from_secs(30));
    }

    #[test]
    fn reset_clears_backoff() {
        let config = ViewChangeConfig {
            base_timeout: Duration::from_secs(3),
            max_timeout: Duration::from_secs(30),
        };
        let mut tracker = ViewChangeTracker::new(config);

        // Trigger timeouts
        let qc = QuorumCert::genesis();
        tracker.on_timeout(0, 1, qc.clone(), &test_id());
        tracker.on_timeout(1, 1, qc, &test_id());
        assert_eq!(tracker.timeout_duration(), Duration::from_secs(12));

        // Reset
        tracker.reset();
        assert_eq!(tracker.timeout_duration(), Duration::from_secs(3));
        assert!(!tracker.is_timed_out());
    }

    #[test]
    fn view_change_quorum() {
        let validators = make_validators();
        let mut tracker = ViewChangeTracker::new(ViewChangeConfig::default());

        let qc_low = QuorumCert { block_hash: 1, view: 1, phase: Phase::Prepare, signatures: vec![] };
        let qc_high = QuorumCert { block_hash: 2, view: 5, phase: Phase::Prepare, signatures: vec![] };

        // Need 5 of 7 for quorum
        assert!(tracker.collect_view_change(10, 1, qc_low.clone(), &validators).is_none());
        assert!(tracker.collect_view_change(10, 2, qc_low.clone(), &validators).is_none());
        assert!(tracker.collect_view_change(10, 3, qc_high.clone(), &validators).is_none());
        assert!(tracker.collect_view_change(10, 4, qc_low.clone(), &validators).is_none());

        // 5th → quorum!
        let result = tracker.collect_view_change(10, 5, qc_low, &validators);
        assert!(result.is_some());

        let (new_view, highest) = result.unwrap();
        assert_eq!(new_view, 10);
        assert_eq!(highest.view, 5, "should pick highest QC (view 5, not view 1)");
    }

    #[test]
    fn duplicate_sender_ignored() {
        let validators = make_validators();
        let mut tracker = ViewChangeTracker::new(ViewChangeConfig::default());
        let qc = QuorumCert::genesis();

        // Same sender twice — should be deduplicated
        tracker.collect_view_change(10, 1, qc.clone(), &validators);
        tracker.collect_view_change(10, 1, qc.clone(), &validators); // duplicate
        tracker.collect_view_change(10, 2, qc.clone(), &validators);
        tracker.collect_view_change(10, 3, qc.clone(), &validators);
        tracker.collect_view_change(10, 4, qc.clone(), &validators);

        // This is the 5th unique sender — quorum
        let result = tracker.collect_view_change(10, 5, qc, &validators);
        assert!(result.is_some());
    }
}
