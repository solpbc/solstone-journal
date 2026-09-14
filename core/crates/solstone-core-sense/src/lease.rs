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

    #[test]
    fn sense_day_lease_released_on_holder_death_without_unlink() {
        if let Ok(day) = std::env::var("SENSE_DAY_LEASE_CHILD_TEST_DAY") {
            let journal = PathBuf::from(
                std::env::var("SENSE_DAY_LEASE_CHILD_TEST_JOURNAL").expect("journal"),
            );
            let _lease = acquire_sense_day_lease(&journal, &day)
                .expect("acquire lease")
                .expect("lease is Some");
            let ready_file = PathBuf::from(
                std::env::var("SENSE_DAY_LEASE_CHILD_READY_FILE").expect("ready file"),
            );
            std::fs::write(&ready_file, b"ready").expect("write ready");
            std::thread::sleep(Duration::from_secs(60));
            return;
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let journal = temp.path();
        let day = "20260914";
        let ready_file = journal.join("child_ready");

        let current_exe = std::env::current_exe().expect("current exe");
        let mut child = std::process::Command::new(current_exe)
            .arg("--exact")
            .arg("lease::tests::sense_day_lease_released_on_holder_death_without_unlink")
            .arg("--nocapture")
            .env("SENSE_DAY_LEASE_CHILD_TEST_DAY", day)
            .env("SENSE_DAY_LEASE_CHILD_TEST_JOURNAL", journal)
            .env("SENSE_DAY_LEASE_CHILD_READY_FILE", &ready_file)
            .spawn()
            .expect("spawn child");

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !ready_file.exists() {
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                panic!("child did not acquire lease in time");
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let contended = acquire_sense_day_lease(journal, day).expect("acquire while child alive");
        assert!(
            contended.is_none(),
            "lease must be contended while child holds it"
        );

        child.kill().expect("kill child");
        let _ = child.wait();

        let lease_path = sense_day_lease_path(journal, day);
        assert!(
            lease_path.is_file(),
            "lease file must not be unlinked on holder death"
        );

        let acquired = acquire_sense_day_lease(journal, day)
            .expect("acquire after child death")
            .expect("lease acquired");
        assert!(acquired.path().is_file());
    }
}
