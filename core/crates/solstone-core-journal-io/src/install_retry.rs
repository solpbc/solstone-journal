// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host-neutral retry table for Windows install_file publication.

#![cfg_attr(not(windows), allow(dead_code))]

pub(crate) const INSTALL_MAX_ATTEMPTS: u8 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallMoveFailure {
    SharingViolation,
    LockViolation,
    MoveOriginAccessDenied,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallSourceClass {
    Retained,
    Absent,
    Different,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallDestinationClass {
    Absent,
    Admitted,
    IsSource,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallReclass {
    Retryable,
    Landed,
    CleanupSource,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallRetryDecision {
    Retry { wait: bool },
    Landed,
    StopCleanup,
    StopUncertain,
}

pub(crate) fn classify_install_names(
    source: Option<InstallSourceClass>,
    dest: Option<InstallDestinationClass>,
    capabilities_ok: bool,
) -> InstallReclass {
    if !capabilities_ok || source.is_none() || dest.is_none() {
        return InstallReclass::Uncertain;
    }
    match (source.unwrap(), dest.unwrap()) {
        (InstallSourceClass::Retained, InstallDestinationClass::Absent)
        | (InstallSourceClass::Retained, InstallDestinationClass::Admitted) => {
            InstallReclass::Retryable
        }
        (InstallSourceClass::Retained, InstallDestinationClass::Other) => {
            InstallReclass::CleanupSource
        }
        (InstallSourceClass::Retained, InstallDestinationClass::IsSource) => {
            InstallReclass::Uncertain
        }
        (InstallSourceClass::Absent, InstallDestinationClass::IsSource)
        | (InstallSourceClass::Different, InstallDestinationClass::IsSource) => {
            InstallReclass::Landed
        }
        (InstallSourceClass::Absent, _) | (InstallSourceClass::Different, _) => {
            InstallReclass::Uncertain
        }
    }
}

pub(crate) fn decide_install_retry(
    failure: InstallMoveFailure,
    reclass: InstallReclass,
    attempt: u8,
) -> InstallRetryDecision {
    match reclass {
        InstallReclass::Landed => InstallRetryDecision::Landed,
        InstallReclass::Uncertain => InstallRetryDecision::StopUncertain,
        InstallReclass::CleanupSource => InstallRetryDecision::StopCleanup,
        InstallReclass::Retryable => {
            let retryable_failure = matches!(
                failure,
                InstallMoveFailure::SharingViolation
                    | InstallMoveFailure::LockViolation
                    | InstallMoveFailure::MoveOriginAccessDenied
            );
            if retryable_failure && (1..INSTALL_MAX_ATTEMPTS).contains(&attempt) {
                InstallRetryDecision::Retry { wait: true }
            } else {
                InstallRetryDecision::StopCleanup
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        INSTALL_MAX_ATTEMPTS, InstallDestinationClass, InstallMoveFailure, InstallReclass,
        InstallRetryDecision, InstallSourceClass, classify_install_names, decide_install_retry,
    };

    const FAILURES: [InstallMoveFailure; 4] = [
        InstallMoveFailure::SharingViolation,
        InstallMoveFailure::LockViolation,
        InstallMoveFailure::MoveOriginAccessDenied,
        InstallMoveFailure::Other,
    ];

    const RECLASSES: [InstallReclass; 4] = [
        InstallReclass::Retryable,
        InstallReclass::Landed,
        InstallReclass::CleanupSource,
        InstallReclass::Uncertain,
    ];

    const SOURCES: [InstallSourceClass; 3] = [
        InstallSourceClass::Retained,
        InstallSourceClass::Absent,
        InstallSourceClass::Different,
    ];

    const DESTS: [InstallDestinationClass; 4] = [
        InstallDestinationClass::Absent,
        InstallDestinationClass::Admitted,
        InstallDestinationClass::IsSource,
        InstallDestinationClass::Other,
    ];

    fn expected_reclass(
        source: InstallSourceClass,
        dest: InstallDestinationClass,
    ) -> InstallReclass {
        match (source, dest) {
            (InstallSourceClass::Retained, InstallDestinationClass::Absent)
            | (InstallSourceClass::Retained, InstallDestinationClass::Admitted) => {
                InstallReclass::Retryable
            }
            (InstallSourceClass::Retained, InstallDestinationClass::Other) => {
                InstallReclass::CleanupSource
            }
            (InstallSourceClass::Retained, InstallDestinationClass::IsSource) => {
                InstallReclass::Uncertain
            }
            (InstallSourceClass::Absent, InstallDestinationClass::IsSource)
            | (InstallSourceClass::Different, InstallDestinationClass::IsSource) => {
                InstallReclass::Landed
            }
            (InstallSourceClass::Absent, _) | (InstallSourceClass::Different, _) => {
                InstallReclass::Uncertain
            }
        }
    }

    fn expected_decision(
        failure: InstallMoveFailure,
        reclass: InstallReclass,
        attempt: u8,
    ) -> InstallRetryDecision {
        match reclass {
            InstallReclass::Landed => InstallRetryDecision::Landed,
            InstallReclass::Uncertain => InstallRetryDecision::StopUncertain,
            InstallReclass::CleanupSource => InstallRetryDecision::StopCleanup,
            InstallReclass::Retryable => {
                let retryable = matches!(
                    failure,
                    InstallMoveFailure::SharingViolation
                        | InstallMoveFailure::LockViolation
                        | InstallMoveFailure::MoveOriginAccessDenied
                );
                if retryable && (1..INSTALL_MAX_ATTEMPTS).contains(&attempt) {
                    InstallRetryDecision::Retry { wait: true }
                } else {
                    InstallRetryDecision::StopCleanup
                }
            }
        }
    }

    #[test]
    fn classify_names_covers_every_source_dest_and_capability() {
        for source in SOURCES {
            for dest in DESTS {
                assert_eq!(
                    classify_install_names(Some(source), Some(dest), true),
                    expected_reclass(source, dest),
                    "{source:?} {dest:?}"
                );
                assert_eq!(
                    classify_install_names(Some(source), Some(dest), false),
                    InstallReclass::Uncertain,
                    "capabilities_ok=false {source:?} {dest:?}"
                );
            }
        }
        for dest in DESTS {
            assert_eq!(
                classify_install_names(None, Some(dest), true),
                InstallReclass::Uncertain,
                "source observation failed {dest:?}"
            );
        }
        for source in SOURCES {
            assert_eq!(
                classify_install_names(Some(source), None, true),
                InstallReclass::Uncertain,
                "dest observation failed {source:?}"
            );
        }
        assert_eq!(
            classify_install_names(None, None, true),
            InstallReclass::Uncertain
        );
        assert_eq!(
            classify_install_names(None, None, false),
            InstallReclass::Uncertain
        );
    }

    #[test]
    fn retry_table_covers_every_failure_reclass_and_attempt() {
        for failure in FAILURES {
            for reclass in RECLASSES {
                for attempt in 1..=INSTALL_MAX_ATTEMPTS {
                    let got = decide_install_retry(failure, reclass, attempt);
                    let expected = expected_decision(failure, reclass, attempt);
                    assert_eq!(got, expected, "{failure:?} {reclass:?} attempt {attempt}");
                    assert!(
                        !matches!(got, InstallRetryDecision::Retry { wait: false }),
                        "{failure:?} {reclass:?} attempt {attempt} produced a no-wait retry"
                    );
                }
            }
        }
    }

    #[test]
    fn retryable_failures_wait_three_times_then_stop_cleanup() {
        for failure in [
            InstallMoveFailure::SharingViolation,
            InstallMoveFailure::LockViolation,
            InstallMoveFailure::MoveOriginAccessDenied,
        ] {
            let mut waits = 0u8;
            let mut stopped = None;
            for attempt in 1..=INSTALL_MAX_ATTEMPTS {
                match decide_install_retry(failure, InstallReclass::Retryable, attempt) {
                    InstallRetryDecision::Retry { wait: true } => waits += 1,
                    InstallRetryDecision::Retry { wait: false } => {
                        panic!("{failure:?} produced a no-wait retry")
                    }
                    other => {
                        stopped = Some((attempt, other));
                        break;
                    }
                }
            }
            assert_eq!(waits, 3, "{failure:?}");
            assert_eq!(
                stopped,
                Some((INSTALL_MAX_ATTEMPTS, InstallRetryDecision::StopCleanup)),
                "{failure:?}"
            );
        }
    }

    #[test]
    fn other_failure_never_retries() {
        for reclass in RECLASSES {
            for attempt in 1..=INSTALL_MAX_ATTEMPTS {
                let got = decide_install_retry(InstallMoveFailure::Other, reclass, attempt);
                assert!(
                    !matches!(got, InstallRetryDecision::Retry { .. }),
                    "{reclass:?} attempt {attempt}"
                );
            }
        }
        for reclass in [
            InstallReclass::CleanupSource,
            InstallReclass::Uncertain,
            InstallReclass::Landed,
        ] {
            for failure in FAILURES {
                assert!(
                    !matches!(
                        decide_install_retry(failure, reclass, 1),
                        InstallRetryDecision::Retry { .. }
                    ),
                    "{failure:?} {reclass:?}"
                );
            }
        }
    }

    #[test]
    fn landed_and_uncertain_ignore_failure_and_attempt() {
        for failure in FAILURES {
            for attempt in 1..=INSTALL_MAX_ATTEMPTS {
                assert_eq!(
                    decide_install_retry(failure, InstallReclass::Landed, attempt),
                    InstallRetryDecision::Landed,
                    "{failure:?} attempt {attempt}"
                );
                assert_eq!(
                    decide_install_retry(failure, InstallReclass::Uncertain, attempt),
                    InstallRetryDecision::StopUncertain,
                    "{failure:?} attempt {attempt}"
                );
                assert_eq!(
                    decide_install_retry(failure, InstallReclass::CleanupSource, attempt),
                    InstallRetryDecision::StopCleanup,
                    "{failure:?} attempt {attempt}"
                );
            }
        }
    }
}
