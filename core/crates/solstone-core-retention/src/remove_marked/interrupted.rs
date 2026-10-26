// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Removals interrupted by a crash or restart. A removal stage records its
//! original's directory, name and digest before the original moves into it, so
//! the next exclusive lease holder can resolve it: an original that never moved
//! or was already deleted leaves only its record; a moved original goes back
//! under its own name; one whose segment the owner deleted is deleted as
//! approved. Marks are untouched, so a returned original is approved again.

use std::path::PathBuf;
use std::sync::Arc;

use solstone_core_journal_io::activity::ActivityLease;
use solstone_core_journal_io::media::InterruptedRemovalResolution;

#[derive(Debug)]
pub struct InterruptedRemoval {
    /// The stage, relative to the journal root.
    pub stage: PathBuf,
    /// How it was resolved, or why it stays an unresolved obligation.
    pub outcome: Result<InterruptedRemovalResolution, std::io::ErrorKind>,
}

/// Resolve every interrupted removal. Call only with a freshly acquired lease,
/// before creating anything. Desktop storage that is not exclusive-local still
/// returns an empty list. Android and iOS refuse when the private container
/// is not qualified, and never report that refusal as an empty success.
/// Other stages are untouched.
pub fn recover_interrupted_removals(
    activity: &Arc<ActivityLease>,
) -> Result<Vec<InterruptedRemoval>, std::io::Error> {
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    ))]
    {
        local::recover(activity)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        let _ = activity;
        Err(unsupported_storage())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn unsupported_storage() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "the activity lease does not exclude every writer here",
    )
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
mod local {
    use std::path::Path;

    use solstone_core_journal_io::activity_obligations::{NativeObligation, observe_obligations};
    use solstone_core_journal_io::journal_root::JournalEntryKind;
    use solstone_core_journal_io::media::{
        MediaDirectory, exclusive_local_activity, resolve_interrupted_removal,
    };
    use solstone_core_segment::{OwnerDeletionState, owner_deletion_state};

    use super::*;

    pub(super) fn recover(
        activity: &Arc<ActivityLease>,
    ) -> Result<Vec<InterruptedRemoval>, std::io::Error> {
        if let Some(miss) = storage_miss(exclusive_local_activity(activity)) {
            return miss;
        }
        let census = match observe_obligations(activity.journal_root()) {
            Ok(census) => census,
            Err(error) => return census_miss(error),
        };
        Ok(census
            .entries()
            .iter()
            .filter_map(|obligation| match obligation {
                NativeObligation::Stage { relative, entry }
                    if entry.kind == JournalEntryKind::Directory =>
                {
                    Some((relative.clone(), entry.name.clone(), entry.identity))
                }
                _ => None,
            })
            .filter_map(|(relative, name, identity)| {
                let parent = relative.parent().unwrap_or(Path::new(""));
                let outcome = MediaDirectory::open(Arc::clone(activity.journal_root()), parent)
                    .and_then(|parent| {
                        let base = parent.diagnostic_path().to_owned();
                        resolve_interrupted_removal(activity, &parent, &name, identity, |source| {
                            // Never put an original back into a segment the
                            // owner deleted; an ambiguous marker refuses.
                            match owner_deletion_state(&base.join(source))
                                .map_err(std::io::Error::other)?
                            {
                                OwnerDeletionState::Live => Ok(false),
                                OwnerDeletionState::Deleted | OwnerDeletionState::Staged => {
                                    Ok(true)
                                }
                                OwnerDeletionState::Occupied => Err(std::io::Error::other(
                                    "the owner-deletion marker is ambiguous",
                                )),
                            }
                        })
                    });
                match outcome {
                    Ok(None) => None,
                    Ok(Some(resolution)) => Some(InterruptedRemoval {
                        stage: relative,
                        outcome: Ok(resolution),
                    }),
                    Err(error) => Some(InterruptedRemoval {
                        stage: relative,
                        outcome: Err(error.kind()),
                    }),
                }
            })
            .collect())
    }

    fn storage_miss<T>(qualified: std::io::Result<bool>) -> Option<Result<Vec<T>, std::io::Error>> {
        match qualified {
            Ok(true) => None,
            Ok(false) => Some(super::platform_miss(None)),
            Err(error) => Some(super::platform_miss(Some(error))),
        }
    }

    fn census_miss<T>(error: impl std::fmt::Display) -> Result<Vec<T>, std::io::Error> {
        super::platform_miss(Some(std::io::Error::other(error.to_string())))
    }
}

pub(super) fn platform_miss<T>(error: Option<std::io::Error>) -> Result<Vec<T>, std::io::Error> {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        Err(error.unwrap_or_else(unsupported_storage))
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = error;
        Ok(Vec::new())
    }
}

#[cfg(all(
    test,
    feature = "full-tests",
    any(target_os = "linux", target_os = "macos")
))]
#[path = "../../tests/support/interrupted_removal.rs"]
mod tests;
