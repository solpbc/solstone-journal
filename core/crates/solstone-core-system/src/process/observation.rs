// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Closed process-observation projection shared by supervisor services.

use std::io;
use std::time::Instant;

/// A status read may only publish liveness when its complete process tuple is coherent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessObservation {
    Live {
        reference: String,
        pid: u32,
        uptime_seconds: u64,
    },
    ConfirmedAbsent,
    Indeterminate,
}

/// The fully gathered facts for one current process.
pub struct ProcessObservationTuple<T> {
    pub reference: String,
    pub pid: u32,
    pub started_at: Instant,
    pub poll: io::Result<Option<T>>,
}

/// Classify one status sample without inspecting or mutating an OS process.
pub fn classify_process_observation<T>(
    current_process_count: usize,
    has_residue: bool,
    tuple: Option<ProcessObservationTuple<T>>,
    now: Instant,
) -> ProcessObservation {
    match current_process_count {
        0 => {
            if has_residue {
                ProcessObservation::Indeterminate
            } else {
                ProcessObservation::ConfirmedAbsent
            }
        }
        1 => match tuple {
            Some(tuple) => match tuple.poll {
                Ok(None) => ProcessObservation::Live {
                    reference: tuple.reference,
                    pid: tuple.pid,
                    uptime_seconds: now.saturating_duration_since(tuple.started_at).as_secs(),
                },
                Ok(Some(_)) => ProcessObservation::ConfirmedAbsent,
                Err(_) => ProcessObservation::Indeterminate,
            },
            None => ProcessObservation::Indeterminate,
        },
        _ => ProcessObservation::Indeterminate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tuple(poll: io::Result<Option<()>>) -> ProcessObservationTuple<()> {
        let started_at = Instant::now();
        ProcessObservationTuple {
            reference: "ref".into(),
            pid: 7,
            started_at,
            poll,
        }
    }

    #[test]
    fn classifies_the_closed_process_observation_matrix() {
        let now = Instant::now();
        assert_eq!(
            classify_process_observation::<()>(0, false, None, now),
            ProcessObservation::ConfirmedAbsent
        );
        assert_eq!(
            classify_process_observation::<()>(0, true, None, now),
            ProcessObservation::Indeterminate
        );
        assert_eq!(
            classify_process_observation(1, false, Some(tuple(Ok(Some(())))), now),
            ProcessObservation::ConfirmedAbsent
        );
        assert_eq!(
            classify_process_observation(1, false, Some(tuple(Err(io::Error::other("poll")))), now),
            ProcessObservation::Indeterminate
        );
        assert_eq!(
            classify_process_observation::<()>(1, false, None, now),
            ProcessObservation::Indeterminate
        );
        assert_eq!(
            classify_process_observation::<()>(2, false, None, now),
            ProcessObservation::Indeterminate
        );
    }

    #[test]
    fn live_uptime_uses_the_supplied_monotonic_sample() {
        let started_at = Instant::now();
        let now = started_at + Duration::from_secs(9);
        let observation = classify_process_observation(
            1,
            false,
            Some(ProcessObservationTuple {
                reference: "ref".into(),
                pid: 7,
                started_at,
                poll: Ok(None::<()>),
            }),
            now,
        );
        assert_eq!(
            observation,
            ProcessObservation::Live {
                reference: "ref".into(),
                pid: 7,
                uptime_seconds: 9,
            }
        );
    }
}
