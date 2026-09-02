// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host-neutral retry table for Windows create-only publication.

#![cfg_attr(not(windows), allow(dead_code))]

pub(crate) const CREATE_ONLY_MAX_ATTEMPTS: u8 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CreateOnlyMoveFailure {
    SharingViolation,
    LockViolation,
    AccessDenied,
    AlreadyExists,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CreateOnlyReclass {
    StillHeld,
    DestinationOccupied,
    CapabilityChanged,
    StageMissing,
    Indeterminate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CreateOnlyRetry {
    Retry { wait: bool },
    Stop,
}

pub(crate) fn decide_create_only_retry(
    failure: CreateOnlyMoveFailure,
    reclass: CreateOnlyReclass,
    attempt: u8,
) -> CreateOnlyRetry {
    let retryable_failure = matches!(
        failure,
        CreateOnlyMoveFailure::SharingViolation
            | CreateOnlyMoveFailure::LockViolation
            | CreateOnlyMoveFailure::AccessDenied
    );
    if retryable_failure
        && reclass == CreateOnlyReclass::StillHeld
        && (1..CREATE_ONLY_MAX_ATTEMPTS).contains(&attempt)
    {
        CreateOnlyRetry::Retry { wait: true }
    } else {
        CreateOnlyRetry::Stop
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CREATE_ONLY_MAX_ATTEMPTS, CreateOnlyMoveFailure, CreateOnlyReclass, CreateOnlyRetry,
        decide_create_only_retry,
    };

    const FAILURES: [CreateOnlyMoveFailure; 5] = [
        CreateOnlyMoveFailure::SharingViolation,
        CreateOnlyMoveFailure::LockViolation,
        CreateOnlyMoveFailure::AccessDenied,
        CreateOnlyMoveFailure::AlreadyExists,
        CreateOnlyMoveFailure::Other,
    ];

    const RECLASSES: [CreateOnlyReclass; 5] = [
        CreateOnlyReclass::StillHeld,
        CreateOnlyReclass::DestinationOccupied,
        CreateOnlyReclass::CapabilityChanged,
        CreateOnlyReclass::StageMissing,
        CreateOnlyReclass::Indeterminate,
    ];

    fn eligible(failure: CreateOnlyMoveFailure, reclass: CreateOnlyReclass, attempt: u8) -> bool {
        matches!(
            failure,
            CreateOnlyMoveFailure::SharingViolation
                | CreateOnlyMoveFailure::LockViolation
                | CreateOnlyMoveFailure::AccessDenied
        ) && reclass == CreateOnlyReclass::StillHeld
            && (1..CREATE_ONLY_MAX_ATTEMPTS).contains(&attempt)
    }

    #[test]
    fn retry_table_covers_every_failure_reclass_and_attempt() {
        for failure in FAILURES {
            for reclass in RECLASSES {
                for attempt in 1..=CREATE_ONLY_MAX_ATTEMPTS {
                    let got = decide_create_only_retry(failure, reclass, attempt);
                    let expected = if eligible(failure, reclass, attempt) {
                        CreateOnlyRetry::Retry { wait: true }
                    } else {
                        CreateOnlyRetry::Stop
                    };
                    assert_eq!(got, expected, "{failure:?} {reclass:?} attempt {attempt}");
                }
            }
        }
    }

    #[test]
    fn still_held_eligible_failures_wait_three_times_then_stop() {
        for failure in [
            CreateOnlyMoveFailure::SharingViolation,
            CreateOnlyMoveFailure::LockViolation,
            CreateOnlyMoveFailure::AccessDenied,
        ] {
            let mut waits = 0u8;
            let mut stopped_at = None;
            for attempt in 1..=CREATE_ONLY_MAX_ATTEMPTS {
                match decide_create_only_retry(failure, CreateOnlyReclass::StillHeld, attempt) {
                    CreateOnlyRetry::Retry { wait: true } => waits += 1,
                    CreateOnlyRetry::Retry { wait: false } => {
                        panic!("{failure:?} produced a no-wait retry")
                    }
                    CreateOnlyRetry::Stop => {
                        stopped_at = Some(attempt);
                        break;
                    }
                }
            }
            assert_eq!(waits, 3, "{failure:?}");
            assert_eq!(stopped_at, Some(CREATE_ONLY_MAX_ATTEMPTS), "{failure:?}");
        }
    }

    #[test]
    fn access_denied_retries_only_when_still_held() {
        for reclass in RECLASSES {
            for attempt in 1..=CREATE_ONLY_MAX_ATTEMPTS {
                let got =
                    decide_create_only_retry(CreateOnlyMoveFailure::AccessDenied, reclass, attempt);
                if reclass == CreateOnlyReclass::StillHeld && attempt < CREATE_ONLY_MAX_ATTEMPTS {
                    assert_eq!(got, CreateOnlyRetry::Retry { wait: true }, "{reclass:?}");
                } else {
                    assert_eq!(got, CreateOnlyRetry::Stop, "{reclass:?} attempt {attempt}");
                }
            }
        }
    }

    #[test]
    fn ineligible_failures_stop_immediately_with_zero_waits() {
        for failure in [
            CreateOnlyMoveFailure::AlreadyExists,
            CreateOnlyMoveFailure::Other,
        ] {
            for reclass in RECLASSES {
                let mut waits = 0u8;
                let first = decide_create_only_retry(failure, reclass, 1);
                assert_eq!(first, CreateOnlyRetry::Stop, "{failure:?} {reclass:?}");
                if matches!(first, CreateOnlyRetry::Retry { wait: true }) {
                    waits += 1;
                }
                assert_eq!(waits, 0, "{failure:?} {reclass:?}");
            }
        }
        for failure in [
            CreateOnlyMoveFailure::SharingViolation,
            CreateOnlyMoveFailure::LockViolation,
            CreateOnlyMoveFailure::AccessDenied,
        ] {
            for reclass in RECLASSES {
                if reclass == CreateOnlyReclass::StillHeld {
                    continue;
                }
                assert_eq!(
                    decide_create_only_retry(failure, reclass, 1),
                    CreateOnlyRetry::Stop,
                    "{failure:?} {reclass:?}"
                );
            }
        }
    }
}
