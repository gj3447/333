// KG: SPAN_333_BFT_StateMachine
//! Leader election for HotStuff.
//! Simple round-robin by view number.

use super::crypto::{NodeId, ValidatorSet};

/// Determine the leader for a given view
pub fn leader_for_view(view: u64, validators: &ValidatorSet) -> NodeId {
    let idx = (view as usize) % validators.n();
    validators.validators[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_robin() {
        let vs = ValidatorSet::new(vec![10, 20, 30, 40, 50, 60, 70]);
        assert_eq!(leader_for_view(0, &vs), 10);
        assert_eq!(leader_for_view(1, &vs), 20);
        assert_eq!(leader_for_view(6, &vs), 70);
        assert_eq!(leader_for_view(7, &vs), 10); // wraps
    }
}
