// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Versioned per-day stream and daily health markers.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{
    AtomicWriteError, AtomicWriteOptions, LockError, LockOptions, atomic_replace, hold_lock,
};

const MARKER_VERSION: u64 = 1;

/// The two marker files participating in a day's completion comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthMarkerKind {
    Stream,
    Daily,
}

impl HealthMarkerKind {
    fn filename(self) -> &'static str {
        match self {
            Self::Stream => "stream.updated",
            Self::Daily => "daily.updated",
        }
    }
}

/// Durable marker payload published with every new stream generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthMarker {
    pub version: u64,
    pub generation: u64,
    #[serde(default)]
    pub fingerprint: Option<String>,
}

/// The parsed state of one on-disk marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthMarkerState {
    Absent,
    LegacyEmpty {
        modified: SystemTime,
    },
    MalformedNonEmpty {
        modified: SystemTime,
    },
    Versioned {
        marker: HealthMarker,
        modified: SystemTime,
    },
}

impl HealthMarkerState {
    fn modified(&self) -> Option<SystemTime> {
        match self {
            Self::Absent => None,
            Self::LegacyEmpty { modified }
            | Self::MalformedNonEmpty { modified }
            | Self::Versioned { modified, .. } => Some(*modified),
        }
    }

    fn generation_and_fingerprint(&self) -> Option<(u64, Option<String>)> {
        match self {
            Self::Absent | Self::LegacyEmpty { .. } => Some((0, None)),
            Self::Versioned { marker, .. } => Some((marker.generation, marker.fingerprint.clone())),
            Self::MalformedNonEmpty { .. } => None,
        }
    }
}

/// Shared complete/dirty decision for a stream/daily marker pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayMarkerPairStatus {
    Complete,
    Dirty,
}

impl DayMarkerPairStatus {
    pub const fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }
}

/// Result of compare-and-publishing a daily marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    Published(u64),
    AlreadyCurrent(u64),
    Superseded(u64),
}

/// A marker read, lock, or publication failure.
#[derive(Debug)]
pub enum HealthMarkerError {
    Io { path: PathBuf, source: io::Error },
    Lock(LockError),
    Atomic(AtomicWriteError),
    Malformed { path: PathBuf },
}

impl std::fmt::Display for HealthMarkerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "health marker I/O failed at {}: {source}",
                    path.display()
                )
            }
            Self::Lock(error) => write!(formatter, "health marker lock failed: {error}"),
            Self::Atomic(error) => write!(formatter, "health marker publication failed: {error}"),
            Self::Malformed { path } => {
                write!(formatter, "health marker is malformed: {}", path.display())
            }
        }
    }
}

impl std::error::Error for HealthMarkerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Lock(error) => Some(error),
            Self::Atomic(error) => Some(error),
            Self::Malformed { .. } => None,
        }
    }
}

impl From<LockError> for HealthMarkerError {
    fn from(error: LockError) -> Self {
        Self::Lock(error)
    }
}

impl From<AtomicWriteError> for HealthMarkerError {
    fn from(error: AtomicWriteError) -> Self {
        Self::Atomic(error)
    }
}

/// Return the canonical path for one day's health marker.
pub fn health_marker_path(journal: &Path, day: &str, kind: HealthMarkerKind) -> PathBuf {
    journal
        .join("chronicle")
        .join(day)
        .join("health")
        .join(kind.filename())
}

/// Parse a marker while retaining legacy empty-file compatibility.
pub fn read_health_marker(
    journal: &Path,
    day: &str,
    kind: HealthMarkerKind,
) -> Result<HealthMarkerState, HealthMarkerError> {
    let path = health_marker_path(journal, day, kind);
    read_health_marker_path(&path)
}

/// Advance the stream marker under its stable sidecar lock.
pub fn bump_stream_marker(journal: &Path, day: &str) -> Result<u64, AtomicWriteError> {
    let path = health_marker_path(journal, day, HealthMarkerKind::Stream);
    let _lock =
        hold_lock(&path, LockOptions::default()).map_err(|error| atomic_error(&path, error))?;
    let state = read_health_marker_path(&path).map_err(|error| atomic_error(&path, error))?;
    let (generation, fingerprint) = state.generation_and_fingerprint().unwrap_or((0, None));
    let next = generation
        .checked_add(1)
        .ok_or_else(|| atomic_error(&path, "stream marker generation overflow"))?;
    write_marker(
        &path,
        &HealthMarker {
            version: MARKER_VERSION,
            generation: next,
            fingerprint,
        },
    )?;
    Ok(next)
}

/// Publish the daily marker only when the observed stream generation is current.
pub fn publish_daily_marker_if_current(
    journal: &Path,
    day: &str,
    observed_generation: u64,
) -> Result<PublishOutcome, HealthMarkerError> {
    let stream_path = health_marker_path(journal, day, HealthMarkerKind::Stream);
    let daily_path = health_marker_path(journal, day, HealthMarkerKind::Daily);
    let _lock = hold_lock(&stream_path, LockOptions::default())?;
    let stream = read_health_marker_path(&stream_path)?;
    let Some((generation, fingerprint)) = stream.generation_and_fingerprint() else {
        return Err(HealthMarkerError::Malformed { path: stream_path });
    };
    if generation != observed_generation {
        return Ok(PublishOutcome::Superseded(generation));
    }
    if matches!(
        read_health_marker_path(&daily_path)?,
        HealthMarkerState::Versioned {
            marker: HealthMarker {
                generation: daily_generation,
                ..
            },
            ..
        } if daily_generation == generation
    ) {
        return Ok(PublishOutcome::AlreadyCurrent(generation));
    }
    write_marker(
        &daily_path,
        &HealthMarker {
            version: MARKER_VERSION,
            generation,
            fingerprint,
        },
    )?;
    Ok(PublishOutcome::Published(generation))
}

/// Return the completion status shared by health and catchup readers.
pub fn day_marker_pair_status(
    journal: &Path,
    day: &str,
) -> Result<DayMarkerPairStatus, HealthMarkerError> {
    let stream = read_health_marker(journal, day, HealthMarkerKind::Stream)?;
    let daily = read_health_marker(journal, day, HealthMarkerKind::Daily)?;
    if matches!(stream, HealthMarkerState::MalformedNonEmpty { .. })
        || matches!(daily, HealthMarkerState::MalformedNonEmpty { .. })
    {
        return Ok(DayMarkerPairStatus::Dirty);
    }
    if matches!(stream, HealthMarkerState::Absent) {
        return Ok(DayMarkerPairStatus::Complete);
    }
    if matches!(daily, HealthMarkerState::Absent) {
        return Ok(DayMarkerPairStatus::Dirty);
    }
    if matches!(stream, HealthMarkerState::LegacyEmpty { .. })
        || matches!(daily, HealthMarkerState::LegacyEmpty { .. })
    {
        let stream_modified = stream.modified().expect("stream presence checked");
        let daily_modified = daily.modified().expect("daily presence checked");
        return Ok(if stream_modified <= daily_modified {
            DayMarkerPairStatus::Complete
        } else {
            DayMarkerPairStatus::Dirty
        });
    }
    let (stream_generation, _) = stream
        .generation_and_fingerprint()
        .expect("malformed marker checked");
    let (daily_generation, _) = daily
        .generation_and_fingerprint()
        .expect("malformed marker checked");
    Ok(if stream_generation == daily_generation {
        DayMarkerPairStatus::Complete
    } else {
        DayMarkerPairStatus::Dirty
    })
}

fn read_health_marker_path(path: &Path) -> Result<HealthMarkerState, HealthMarkerError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(HealthMarkerState::Absent);
        }
        Err(source) => {
            return Err(HealthMarkerError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|source| HealthMarkerError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.is_empty() {
        return Ok(HealthMarkerState::LegacyEmpty { modified });
    }
    match serde_json::from_slice::<HealthMarker>(&bytes) {
        Ok(marker) if marker.version == MARKER_VERSION => {
            Ok(HealthMarkerState::Versioned { marker, modified })
        }
        Ok(_) | Err(_) => Ok(HealthMarkerState::MalformedNonEmpty { modified }),
    }
}

fn write_marker(path: &Path, marker: &HealthMarker) -> Result<(), AtomicWriteError> {
    let bytes = serde_json::to_vec(marker).map_err(|error| atomic_error(path, error))?;
    atomic_replace(path, &bytes, AtomicWriteOptions::default())
}

fn atomic_error(path: &Path, error: impl std::fmt::Display) -> AtomicWriteError {
    AtomicWriteError::Io {
        path: path.to_path_buf(),
        source: io::Error::other(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    use super::*;
    use crate::test_support::TempDir;

    const DAY: &str = "20260101";

    fn marker_path(root: &Path, kind: HealthMarkerKind) -> PathBuf {
        health_marker_path(root, DAY, kind)
    }

    #[test]
    fn bump_round_trips_versioned_stream_marker() {
        let temporary = TempDir::new();
        assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 1);
        assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 2);
        assert!(matches!(
            read_health_marker(temporary.path(), DAY, HealthMarkerKind::Stream).unwrap(),
            HealthMarkerState::Versioned {
                marker: HealthMarker { generation: 2, .. },
                ..
            }
        ));
    }

    #[test]
    fn legacy_empty_pair_uses_mtime_fallback() {
        let temporary = TempDir::new();
        let stream = marker_path(temporary.path(), HealthMarkerKind::Stream);
        let daily = marker_path(temporary.path(), HealthMarkerKind::Daily);
        fs::create_dir_all(stream.parent().unwrap()).unwrap();
        fs::write(&stream, b"").unwrap();
        fs::write(&daily, b"").unwrap();
        let old = SystemTime::now() - Duration::from_secs(1);
        fs::File::open(&daily)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
        assert_eq!(
            day_marker_pair_status(temporary.path(), DAY).unwrap(),
            DayMarkerPairStatus::Dirty
        );
        fs::File::open(&daily)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(SystemTime::now()))
            .unwrap();
        assert_eq!(
            day_marker_pair_status(temporary.path(), DAY).unwrap(),
            DayMarkerPairStatus::Complete
        );
    }

    #[test]
    fn malformed_nonempty_marker_is_dirty() {
        let temporary = TempDir::new();
        let stream = marker_path(temporary.path(), HealthMarkerKind::Stream);
        fs::create_dir_all(stream.parent().unwrap()).unwrap();
        fs::write(&stream, b"not-json").unwrap();
        assert!(matches!(
            read_health_marker(temporary.path(), DAY, HealthMarkerKind::Stream).unwrap(),
            HealthMarkerState::MalformedNonEmpty { .. }
        ));
        assert_eq!(
            day_marker_pair_status(temporary.path(), DAY).unwrap(),
            DayMarkerPairStatus::Dirty
        );
    }

    #[test]
    fn publish_reports_superseded_generation() {
        let temporary = TempDir::new();
        assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 1);
        assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 2);
        assert_eq!(
            publish_daily_marker_if_current(temporary.path(), DAY, 1).unwrap(),
            PublishOutcome::Superseded(2)
        );
    }

    #[test]
    fn concurrent_publishers_have_one_publication_winner() {
        let temporary = TempDir::new();
        assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 1);
        let root = Arc::new(temporary.path().to_path_buf());
        let start = Arc::new(Barrier::new(3));
        let first_root = Arc::clone(&root);
        let first_start = Arc::clone(&start);
        let first = std::thread::spawn(move || {
            first_start.wait();
            publish_daily_marker_if_current(&first_root, DAY, 1).unwrap()
        });
        let second_root = Arc::clone(&root);
        let second_start = Arc::clone(&start);
        let second = std::thread::spawn(move || {
            second_start.wait();
            publish_daily_marker_if_current(&second_root, DAY, 1).unwrap()
        });
        start.wait();
        let outcomes = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, PublishOutcome::Published(1)))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, PublishOutcome::AlreadyCurrent(1)))
                .count(),
            1
        );
    }

    #[test]
    fn real_stream_lock_serializes_a_bump_before_publish() {
        let temporary = TempDir::new();
        assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 1);
        let stream = marker_path(temporary.path(), HealthMarkerKind::Stream);
        let held = hold_lock(&stream, LockOptions::default()).unwrap();
        let root = temporary.path().to_path_buf();
        let ready = Arc::new(Barrier::new(2));
        let worker_ready = Arc::clone(&ready);
        let worker = std::thread::spawn(move || {
            worker_ready.wait();
            bump_stream_marker(&root, DAY).unwrap()
        });
        ready.wait();
        drop(held);
        assert_eq!(worker.join().unwrap(), 2);
        assert_eq!(
            publish_daily_marker_if_current(temporary.path(), DAY, 1).unwrap(),
            PublishOutcome::Superseded(2)
        );
    }
}
