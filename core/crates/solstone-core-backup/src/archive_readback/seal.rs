// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Positive same-owner settlement. A missing error is not confirmation.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerObservation {
    pub activity: u64,
    pub admission: u64,
    pub clock: usize,
    pub deadline: i64,
    pub journal: u64,
    pub repository: String,
    pub sources: String,
    pub target: String,
    pub call_handle: u64,
    pub owner_generation: u64,
    pub drain_ok: bool,
    pub cancelled: bool,
}

#[cfg(any(test, target_os = "android", target_os = "ios"))]
pub enum SettlementKind {
    Idle,
    Accepted,
    Other,
}

/// An idle creator never started an effect. An accepted creator settles only
/// when the finishing observation is the same owner and the drain succeeded.
#[cfg(any(test, target_os = "android", target_os = "ios"))]
pub fn mobile_settlement_confirms(
    kind: SettlementKind,
    pending: u64,
    bound: Option<&OwnerObservation>,
    finished: Option<&OwnerObservation>,
) -> bool {
    if pending != 0 {
        return false;
    }
    match kind {
        SettlementKind::Idle => bound.is_none() && finished.is_none(),
        SettlementKind::Accepted => match (bound, finished) {
            (Some(bound), Some(finished)) => same_owner(bound, finished),
            _ => false,
        },
        SettlementKind::Other => false,
    }
}

pub fn same_owner(bound: &OwnerObservation, finished: &OwnerObservation) -> bool {
    finished.drain_ok
        && !bound.cancelled
        && !finished.cancelled
        && bound.activity == finished.activity
        && bound.admission == finished.admission
        && bound.clock == finished.clock
        && bound.deadline == finished.deadline
        && bound.journal == finished.journal
        && bound.repository == finished.repository
        && bound.sources == finished.sources
        && bound.target == finished.target
        && bound.call_handle == finished.call_handle
        && bound.owner_generation == finished.owner_generation
}

#[cfg(test)]
pub fn sample_owner() -> OwnerObservation {
    OwnerObservation {
        activity: 1,
        admission: 2,
        clock: 3,
        deadline: 4,
        journal: 5,
        repository: "repo".to_owned(),
        sources: "clip.m4a:4:abcd".to_owned(),
        target: "issued".to_owned(),
        call_handle: 9,
        owner_generation: 8,
        drain_ok: true,
        cancelled: false,
    }
}

#[cfg(test)]
mod tests {
    use super::{SettlementKind, mobile_settlement_confirms, same_owner, sample_owner};

    #[test]
    fn matching_drain_is_the_same_owner() {
        let bound = sample_owner();
        let mut finished = bound.clone();
        finished.drain_ok = true;
        assert!(same_owner(&bound, &finished));
    }

    #[test]
    fn status_handle_clock_and_failed_drain_do_not_settle() {
        let bound = sample_owner();
        let mut other_handle = bound.clone();
        other_handle.call_handle = bound.call_handle + 1;
        assert!(!same_owner(&bound, &other_handle));

        let mut no_drain = bound.clone();
        no_drain.drain_ok = false;
        assert!(!same_owner(&bound, &no_drain));

        let mut cancelled = bound.clone();
        cancelled.cancelled = true;
        assert!(!same_owner(&bound, &cancelled));
        let mut bound_cancelled = bound.clone();
        bound_cancelled.cancelled = true;
        assert!(!same_owner(&bound_cancelled, &bound));

        let mut other_clock = bound.clone();
        other_clock.clock += 1;
        assert!(!same_owner(&bound, &other_clock));

        let mut other_admission = bound.clone();
        other_admission.admission += 1;
        assert!(!same_owner(&bound, &other_admission));

        let mut other_owner = bound.clone();
        other_owner.owner_generation += 1;
        assert!(!same_owner(&bound, &other_owner));

        let mut other_target = bound.clone();
        other_target.target.push_str("-replaced");
        assert!(!same_owner(&bound, &other_target));
    }

    #[test]
    fn idle_creators_settle_only_before_an_effect_starts() {
        assert!(mobile_settlement_confirms(
            SettlementKind::Idle,
            0,
            None,
            None
        ));
        let bound = sample_owner();
        assert!(!mobile_settlement_confirms(
            SettlementKind::Idle,
            0,
            Some(&bound),
            None
        ));
        assert!(!mobile_settlement_confirms(
            SettlementKind::Other,
            0,
            None,
            None
        ));
        assert!(!mobile_settlement_confirms(
            SettlementKind::Accepted,
            1,
            Some(&bound),
            Some(&bound)
        ));
    }

    #[test]
    fn accepted_settlement_requires_the_same_confirmed_owner() {
        let bound = sample_owner();
        let finished = bound.clone();
        assert!(mobile_settlement_confirms(
            SettlementKind::Accepted,
            0,
            Some(&bound),
            Some(&finished)
        ));
        assert!(!mobile_settlement_confirms(
            SettlementKind::Accepted,
            0,
            None,
            None
        ));
        let mut undrained = finished.clone();
        undrained.drain_ok = false;
        assert!(!mobile_settlement_confirms(
            SettlementKind::Accepted,
            0,
            Some(&bound),
            Some(&undrained)
        ));
        let mut replaced = finished.clone();
        replaced.call_handle += 1;
        assert!(!mobile_settlement_confirms(
            SettlementKind::Accepted,
            0,
            Some(&bound),
            Some(&replaced)
        ));
    }
}
