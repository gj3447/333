// KG: SPAN_333_L5_CRDT_Extensions, finding_333_synth_crd_sota_d8, finding_333_synth_crd_prt_d10
// Delta-CRDT wrapper — bandwidth optimization via incremental deltas.
//
// Sources:
//   - D8 SOTA: Delta-state CRDTs (Almeida et al. 2016). Bandwidth 50-1000x
//     reduction vs full-state gossip on typical collab-editing workloads.
//   - D10 Port-333: Sealed delta wrapper → extends Crdt without breaking
//     333-signed-state composition.
//
// Design: a `Delta<T>` is a T-typed payload that represents a causal step.
// The trait `DeltaCrdt` lets a CRDT report "what just changed" instead of
// shipping its whole state. Transport layers ship `Delta`s; receivers
// apply via `merge_delta`.

use crate::traits::Crdt;

/// A delta is itself a value of the CRDT's join-semilattice — so applying a
/// delta = joining it in. Same type alias the literature uses.
pub type Delta<T> = T;

/// CRDTs that can emit and consume deltas. Blanket-impl'd for any `Crdt + Clone`
/// because a full state is a valid (non-minimal) delta. Implementors override
/// `take_delta` when a smaller representative exists.
pub trait DeltaCrdt: Crdt + Clone {
    /// Emit a delta representing uncommitted changes since `baseline`. Default:
    /// the whole state (correct but maximal). Downstream CRDTs (LwwMap, ORSet,
    /// PNCounter) override to return only the changed entries.
    fn take_delta(&self, _baseline: Option<&Self>) -> Delta<Self> {
        self.clone()
    }

    /// Apply a received delta. Default: same as `merge`. Kept as a separate
    /// method so implementors can add delta-specific validation (e.g. reject
    /// deltas whose context claims writes we don't know about).
    fn merge_delta(&mut self, delta: &Delta<Self>) {
        self.merge(delta);
    }
}

// Blanket impl: any Crdt+Clone gets default full-state deltas.
impl<T: Crdt + Clone> DeltaCrdt for T {}

/// Sealed wrapper that pairs a CRDT with a delta buffer. Call `stage` to push
/// deltas that haven't been flushed; `drain` returns the buffered deltas for
/// transport; `absorb` folds an incoming delta into the underlying state and
/// the buffer both (so relays downstream see the new delta too).
#[derive(Debug, Clone)]
pub struct DeltaWrap<T: DeltaCrdt> {
    state: T,
    buffer: Vec<Delta<T>>,
}

impl<T: DeltaCrdt + Default> Default for DeltaWrap<T> {
    fn default() -> Self {
        Self { state: T::default(), buffer: Vec::new() }
    }
}

impl<T: DeltaCrdt> DeltaWrap<T> {
    pub fn new(state: T) -> Self {
        Self { state, buffer: Vec::new() }
    }

    pub fn state(&self) -> &T {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut T {
        &mut self.state
    }

    /// Stage a local mutation: caller mutates `state_mut`, then calls `stage`
    /// to capture the new delta (as the current state snapshot).
    pub fn stage(&mut self) {
        self.buffer.push(self.state.clone());
    }

    pub fn drain(&mut self) -> Vec<Delta<T>> {
        std::mem::take(&mut self.buffer)
    }

    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Absorb a remote delta. Merges into state AND re-emits into the buffer
    /// so gossip relays propagate the causal step.
    pub fn absorb(&mut self, delta: &Delta<T>) {
        self.state.merge_delta(delta);
        self.buffer.push(delta.clone());
    }
}

impl<T: DeltaCrdt> Crdt for DeltaWrap<T> {
    fn merge(&mut self, other: &Self) {
        self.state.merge(&other.state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::counter::PNCounter;

    #[test]
    fn delta_default_is_full_state() {
        let mut c = PNCounter::new();
        c.increment(&"A".into(), 10);
        let d = c.take_delta(None);
        let mut target = PNCounter::new();
        target.merge_delta(&d);
        assert_eq!(target.value(), 10);
    }

    #[test]
    fn wrap_stage_drain_roundtrip() {
        let mut w: DeltaWrap<PNCounter> = DeltaWrap::default();
        w.state_mut().increment(&"A".into(), 5);
        w.stage();
        w.state_mut().increment(&"A".into(), 3);
        w.stage();
        let drained = w.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(w.drain().len(), 0); // drained twice: empty
    }

    #[test]
    fn wrap_absorb_merges_and_buffers() {
        let mut a: DeltaWrap<PNCounter> = DeltaWrap::default();
        let mut remote = PNCounter::new();
        remote.increment(&"B".into(), 7);
        a.absorb(&remote);
        assert_eq!(a.state().value(), 7);
        assert_eq!(a.buffered(), 1);
    }

    #[test]
    fn wrap_merge_pure_state_no_buffer_growth() {
        let mut a: DeltaWrap<PNCounter> = DeltaWrap::default();
        let mut b: DeltaWrap<PNCounter> = DeltaWrap::default();
        b.state_mut().increment(&"B".into(), 3);
        a.merge(&b);
        assert_eq!(a.state().value(), 3);
        assert_eq!(a.buffered(), 0);
    }
}
