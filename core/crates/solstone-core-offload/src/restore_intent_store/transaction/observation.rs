// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::{Hold, Owner};
use solstone_core_journal_io::activity_obligations::{
    CarrierObligations, RecoveryObservationError, RecoveryObservationFailure, observe_with_carrier,
};

pub(super) fn observe_restore_obligations<'a>(
    owner: &'a Owner<'_, '_>,
) -> Result<CarrierObligations<'a>, Hold> {
    observe_bounded(
        || owner.native_check().and_then(|()| owner.admission.check()),
        || observe_with_carrier(owner.root(), owner.carrier.observation()),
        || std::thread::sleep(std::time::Duration::from_millis(10)),
    )
}

fn observe_bounded<T>(
    mut checkpoint: impl FnMut() -> Result<(), Hold>,
    mut observe: impl FnMut() -> Result<T, RecoveryObservationError>,
    mut pause: impl FnMut(),
) -> Result<T, Hold> {
    let mut failures = Vec::new();
    loop {
        checkpoint().map_err(|cause| with_failures(&failures, cause))?;
        match observe() {
            Ok(complete) => {
                checkpoint().map_err(|cause| with_failures(&failures, cause))?;
                return Ok(complete);
            }
            Err(error) => {
                // Only an additive directory frontier may trigger a whole new
                // census. Changed identities/records, unknown stages and IO
                // refusals retain their original meanings and never retry.
                let retry = error.failure == RecoveryObservationFailure::DirectoryAdded;
                failures.push(error);
                if !retry || failures.len() == 3 {
                    return Err(Hold::Admission(format!(
                        "recovery observations {failures:?}"
                    )));
                }
                pause();
            }
        }
    }
}

fn with_failures(failures: &[RecoveryObservationError], cause: Hold) -> Hold {
    if failures.is_empty() {
        cause
    } else {
        Hold::Admission(format!(
            "recovery observations {failures:?}; checkpoint {cause:?}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::path::PathBuf;

    fn changed(failure: RecoveryObservationFailure, name: &str) -> RecoveryObservationError {
        RecoveryObservationError {
            failure,
            relative: PathBuf::from(name),
            source: None,
        }
    }

    #[test]
    fn retries_only_additive_directories_and_requires_a_new_complete_observation() {
        let mut calls = 0;
        let mut pauses = 0;
        let value = observe_bounded(
            || Ok(()),
            || {
                calls += 1;
                if calls < 3 {
                    Err(changed(
                        RecoveryObservationFailure::DirectoryAdded,
                        "chronicle/new",
                    ))
                } else {
                    Ok("fresh complete census")
                }
            },
            || pauses += 1,
        )
        .unwrap();
        assert_eq!(value, "fresh complete census");
        assert_eq!((calls, pauses), (3, 2));
        for failure in [
            RecoveryObservationFailure::Changed,
            RecoveryObservationFailure::Io,
            RecoveryObservationFailure::NotDirectory,
        ] {
            let mut calls = 0;
            let result = observe_bounded::<()>(
                || Ok(()),
                || {
                    calls += 1;
                    Err(changed(failure, "original"))
                },
                || panic!("permanent refusal cannot pause or retry"),
            );
            assert!(result.is_err());
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn exhausted_observations_and_original_checkpoint_failure_remain_in_the_hold() {
        let mut calls = 0;
        let result = observe_bounded::<()>(
            || Ok(()),
            || {
                calls += 1;
                Err(changed(
                    RecoveryObservationFailure::DirectoryAdded,
                    &format!("attempt{calls}"),
                ))
            },
            || {},
        );
        let detail = format!("{:?}", result.unwrap_err());
        assert_eq!(calls, 3);
        for name in ["attempt1", "attempt2", "attempt3"] {
            assert!(detail.contains(name));
        }
        for cause in ["cancelled", "deadline_exceeded", "root_changed"] {
            let paused = Cell::new(false);
            let result = observe_bounded::<()>(
                || {
                    if paused.get() {
                        Err(Hold::Admission(cause.into()))
                    } else {
                        Ok(())
                    }
                },
                || Err(changed(RecoveryObservationFailure::DirectoryAdded, "first")),
                || paused.set(true),
            );
            let detail = format!("{:?}", result.unwrap_err());
            assert!(detail.contains(cause));
            assert!(detail.contains("first"));
        }
    }
}
