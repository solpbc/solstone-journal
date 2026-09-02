// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use solstone_core_brain::fingerprint_sha256;
use solstone_core_journal_io::{FileLock, LockOptions, hold_lock};

use crate::{SegmentBindingV1, TimelineError, bounded_diagnostic_detail};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimelineLockSubject {
    Segment(SegmentBindingV1),
    Day(String),
    Master,
}

#[derive(Debug, Clone, Default)]
pub struct TimelineLockRequest {
    pub days: Vec<String>,
    pub subjects: Vec<TimelineLockSubject>,
    pub options: LockOptions,
}

#[derive(Debug)]
pub struct TimelineLockSet {
    population: FileLock,
    days: Vec<FileLock>,
    subjects: Vec<FileLock>,
}

impl TimelineLockSet {
    pub fn protected_paths(&self) -> Vec<&Path> {
        std::iter::once(self.population.path())
            .chain(self.days.iter().map(FileLock::path))
            .chain(self.subjects.iter().map(FileLock::path))
            .collect()
    }
}

pub fn acquire_timeline_locks(
    journal: &Path,
    mut request: TimelineLockRequest,
) -> Result<TimelineLockSet, TimelineError> {
    request.days.sort();
    request.days.dedup();
    let root = journal.join("health/timeline/locks");
    let population = acquire(root.join("population"), request.options)?;
    let days = request
        .days
        .iter()
        .map(|day| {
            acquire(
                root.join("days").join(format!("{day}.order")),
                request.options,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut subject_paths = request
        .subjects
        .iter()
        .map(|subject| subject_lock_path(&root, subject))
        .collect::<Vec<_>>();
    subject_paths.sort();
    subject_paths.dedup();
    let subjects = subject_paths
        .into_iter()
        .map(|path| acquire(path, request.options))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TimelineLockSet {
        population,
        days,
        subjects,
    })
}

pub fn segment_attempt_lock_name(binding: &SegmentBindingV1) -> String {
    let identity = format!("{}\0{}\0{}", binding.day, binding.stream, binding.segment);
    format!("{}.attempt", fingerprint_sha256(&identity))
}

fn subject_lock_path(root: &Path, subject: &TimelineLockSubject) -> PathBuf {
    let subjects = root.join("subjects");
    match subject {
        TimelineLockSubject::Segment(binding) => subjects
            .join("segment")
            .join(segment_attempt_lock_name(binding)),
        TimelineLockSubject::Day(day) => subjects.join("day").join(format!("{day}.attempt")),
        TimelineLockSubject::Master => subjects.join("master.attempt"),
    }
}

fn acquire(path: PathBuf, options: LockOptions) -> Result<FileLock, TimelineError> {
    hold_lock(path, options).map_err(|error| TimelineError::LockContention {
        detail: bounded_diagnostic_detail(&error.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn binding() -> SegmentBindingV1 {
        SegmentBindingV1 {
            day: "20260401".to_owned(),
            stream: "audio".to_owned(),
            segment: "080000_300".to_owned(),
        }
    }

    #[test]
    fn acquisition_orders_unsorted_day_and_subject_requests() {
        let journal = tempfile::tempdir().unwrap();
        let locks = acquire_timeline_locks(
            journal.path(),
            TimelineLockRequest {
                days: vec!["20260402".to_owned(), "20260401".to_owned()],
                subjects: vec![
                    TimelineLockSubject::Master,
                    TimelineLockSubject::Day("20260401".to_owned()),
                    TimelineLockSubject::Segment(binding()),
                ],
                options: LockOptions {
                    timeout: Duration::ZERO,
                    ..LockOptions::default()
                },
            },
        )
        .unwrap();
        let paths = locks
            .protected_paths()
            .into_iter()
            .map(|path| path.strip_prefix(journal.path()).unwrap().to_path_buf())
            .collect::<Vec<_>>();

        assert_eq!(paths[0], Path::new("health/timeline/locks/population"));
        assert_eq!(
            &paths[1..3],
            [
                PathBuf::from("health/timeline/locks/days/20260401.order"),
                PathBuf::from("health/timeline/locks/days/20260402.order"),
            ]
        );
        assert!(paths[3..].windows(2).all(|pair| pair[0] <= pair[1]));
    }
}
