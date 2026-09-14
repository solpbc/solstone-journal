// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Per-day concurrency lease for Sense processing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use solstone_core_journal_io::{FileLease, LeaseError, LeaseOptions, acquire_file_lease};

/// Path to the per-day advisory lease file for Sense processing.
pub fn sense_day_lease_path(journal: &Path, day: &str) -> PathBuf {
    journal
        .join("chronicle")
        .join(day)
        .join("health")
        .join("sense-day.lease")
}

/// Acquire an exclusive advisory lease for processing `day`.
///
/// Returns `Ok(Some(FileLease))` if acquired, `Ok(None)` if contended,
/// or `Err(LeaseError)` on I/O failure.
pub fn acquire_sense_day_lease(journal: &Path, day: &str) -> Result<Option<FileLease>, LeaseError> {
    let lease_path = sense_day_lease_path(journal, day);
    acquire_file_lease(
        &lease_path,
        LeaseOptions {
            attempts: 1,
            retry_max: Duration::ZERO,
            ..LeaseOptions::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sense_day_lease_contention_returns_none_without_unlinking() {
        let temp = tempfile::tempdir().expect("tempdir");
        let journal = temp.path();
        let day = "20260914";

        let lease1 = acquire_sense_day_lease(journal, day)
            .expect("first acquire succeeds")
            .expect("lease is Some");

        let lease_path = sense_day_lease_path(journal, day);
        assert!(lease_path.is_file(), "lease file must exist on disk");

        let lease2 = acquire_sense_day_lease(journal, day).expect("second acquire checked");
        assert!(lease2.is_none(), "second acquire must encounter contention");

        drop(lease1);
        assert!(
            lease_path.is_file(),
            "lease file must remain on disk after drop (never unlinked)"
        );

        let lease3 = acquire_sense_day_lease(journal, day)
            .expect("reacquire succeeds")
            .expect("lease is Some");
        assert!(lease3.path().is_file());
    }
}
