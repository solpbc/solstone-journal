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
    pub subjects: Vec<TimelineLockSubject>,
    pub options: LockOptions,
}

#[derive(Debug)]
pub struct TimelineLockSet {
    subjects: Vec<FileLock>,
    population: FileLock,
    options: LockOptions,
}

impl TimelineLockSet {
    pub(crate) fn require_subject(
        &self,
        journal: &Path,
        subject: &str,
    ) -> Result<(), TimelineError> {
        let root = journal.join("health/timeline/locks");
        let expected = subject_lock_path(&root, &crate::state::parse_subject(subject)?);
        if self.population.path() == root.join("population")
            && self.subjects.iter().any(|lock| lock.path() == expected)
        {
            return Ok(());
        }
        Err(TimelineError::LockContention {
            detail: format!(
                "publication requires the held lock for {subject} in {}",
                journal.display()
            ),
        })
    }

    /// Run slow work while retaining subject ownership, then reacquire publication access.
    pub fn without_population<T>(
        self,
        work: impl FnOnce() -> T,
    ) -> Result<(Self, T), TimelineError> {
        let Self {
            subjects,
            population,
            options,
        } = self;
        let path = population.path().to_path_buf();
        drop(population);
        let result = work();
        let population = acquire(path, options)?;
        Ok((
            Self {
                subjects,
                population,
                options,
            },
            result,
        ))
    }

    pub fn protected_paths(&self) -> Vec<&Path> {
        self.subjects
            .iter()
            .map(FileLock::path)
            .chain(std::iter::once(self.population.path()))
            .collect()
    }
}

pub fn acquire_timeline_locks(
    journal: &Path,
    request: TimelineLockRequest,
) -> Result<TimelineLockSet, TimelineError> {
    let root = journal.join("health/timeline/locks");
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
    let population = acquire(root.join("population"), request.options)?;
    Ok(TimelineLockSet {
        subjects,
        population,
        options: request.options,
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
    fn acquisition_orders_subject_requests() {
        let journal = tempfile::tempdir().unwrap();
        let locks = acquire_timeline_locks(
            journal.path(),
            TimelineLockRequest {
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

        assert!(paths[..3].windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(paths[3], PathBuf::from("health/timeline/locks/population"));
        assert_eq!(paths.len(), 4);
    }

    #[test]
    fn slow_work_retains_subject_and_reacquires_population_even_on_work_error() {
        let journal = tempfile::tempdir().unwrap();
        let request = |subject| TimelineLockRequest {
            subjects: vec![subject],
            options: LockOptions {
                timeout: Duration::ZERO,
                ..LockOptions::default()
            },
        };
        let locks =
            acquire_timeline_locks(journal.path(), request(TimelineLockSubject::Master)).unwrap();
        let (locks, result) = locks
            .without_population(|| {
                let unrelated = acquire_timeline_locks(
                    journal.path(),
                    request(TimelineLockSubject::Segment(binding())),
                )
                .unwrap();
                drop(unrelated);
                assert!(
                    acquire_timeline_locks(journal.path(), request(TimelineLockSubject::Master),)
                        .is_err()
                );
                Err::<(), _>("model failed")
            })
            .unwrap();
        assert_eq!(result, Err("model failed"));
        locks.require_subject(journal.path(), "master").unwrap();
        assert!(
            acquire_timeline_locks(
                journal.path(),
                request(TimelineLockSubject::Segment(binding())),
            )
            .is_err()
        );
        drop(locks);
        acquire_timeline_locks(journal.path(), request(TimelineLockSubject::Master)).unwrap();
    }
}
