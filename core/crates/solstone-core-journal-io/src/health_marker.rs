// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Versioned per-day stream and daily health markers.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HealthMarker {
    pub version: u64,
    pub generation: u64,
    pub fingerprint: Option<String>,
}

impl<'de> Deserialize<'de> for HealthMarker {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(HealthMarkerVisitor)
    }
}

struct HealthMarkerVisitor;

impl<'de> Visitor<'de> for HealthMarkerVisitor {
    type Value = HealthMarker;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a health marker envelope")
    }

    fn visit_map<A>(self, mut map: A) -> Result<HealthMarker, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut version = None;
        let mut generation = None;
        let mut fingerprint = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "version" => {
                    if version.is_some() {
                        return Err(de::Error::duplicate_field("version"));
                    }
                    version = Some(map.next_value()?);
                }
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "fingerprint" => {
                    if fingerprint.is_some() {
                        return Err(de::Error::duplicate_field("fingerprint"));
                    }
                    fingerprint = Some(map.next_value()?);
                }
                _ => {
                    let _: de::IgnoredAny = map.next_value()?;
                }
            }
        }
        Ok(HealthMarker {
            version: version.ok_or_else(|| de::Error::missing_field("version"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            fingerprint: fingerprint.ok_or_else(|| de::Error::missing_field("fingerprint"))?,
        })
    }
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
    /// Raw input changed after this generation was admitted. The stream
    /// generation was advanced while holding the shared stream lock.
    InputChanged(u64),
}

/// A marker read, lock, or publication failure.
#[derive(Debug)]
pub enum HealthMarkerError {
    Io { path: PathBuf, source: io::Error },
    Lock(LockError),
    Atomic(AtomicWriteError),
    Malformed { path: PathBuf },
    Fingerprint { message: String },
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
            Self::Fingerprint { message } => {
                write!(
                    formatter,
                    "health marker fingerprint read failed: {message}"
                )
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
            Self::Malformed { .. } | Self::Fingerprint { .. } => None,
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
    let (generation, fingerprint) = state
        .generation_and_fingerprint()
        .ok_or_else(|| atomic_error(&path, "refusing to replace malformed stream marker"))?;
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
    observed_fingerprint: &str,
    read_current_fingerprint: impl FnOnce() -> Result<String, String>,
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
    let current_fingerprint =
        read_current_fingerprint().map_err(|message| HealthMarkerError::Fingerprint { message })?;
    if current_fingerprint != observed_fingerprint {
        let next = generation.checked_add(1).ok_or_else(|| {
            HealthMarkerError::Atomic(atomic_error(
                &stream_path,
                "stream marker generation overflow",
            ))
        })?;
        write_marker(
            &stream_path,
            &HealthMarker {
                version: MARKER_VERSION,
                generation: next,
                fingerprint,
            },
        )?;
        return Ok(PublishOutcome::InputChanged(next));
    }
    match read_health_marker_path(&daily_path)? {
        HealthMarkerState::MalformedNonEmpty { .. } => {
            return Err(HealthMarkerError::Malformed { path: daily_path });
        }
        HealthMarkerState::Versioned { marker, .. } if marker.generation == generation => {
            if marker.fingerprint.as_deref() != Some(observed_fingerprint) {
                // The raw input may have changed after the prior daily marker
                // was published but before this invocation was admitted, with
                // the corresponding dirty-writer generation bump lost.  The
                // daily marker gives us the missing before-state.  Invalidate
                // it under the same lock instead of leaving an equal-generation
                // pair that readers would mistake for complete.
                let next = generation.checked_add(1).ok_or_else(|| {
                    HealthMarkerError::Atomic(atomic_error(
                        &stream_path,
                        "stream marker generation overflow",
                    ))
                })?;
                write_marker(
                    &stream_path,
                    &HealthMarker {
                        version: MARKER_VERSION,
                        generation: next,
                        fingerprint,
                    },
                )?;
                return Ok(PublishOutcome::InputChanged(next));
            }
            return Ok(PublishOutcome::AlreadyCurrent(generation));
        }
        HealthMarkerState::Versioned { marker, .. } if marker.generation > generation => {
            return Err(HealthMarkerError::Malformed { path: daily_path });
        }
        HealthMarkerState::Absent
        | HealthMarkerState::LegacyEmpty { .. }
        | HealthMarkerState::Versioned { .. } => {}
    }
    write_marker(
        &daily_path,
        &HealthMarker {
            version: MARKER_VERSION,
            generation,
            fingerprint: Some(observed_fingerprint.to_owned()),
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
    let (stream_generation, stream_fingerprint) = stream
        .generation_and_fingerprint()
        .expect("malformed marker checked");
    let (daily_generation, daily_fingerprint) = daily
        .generation_and_fingerprint()
        .expect("malformed marker checked");
    let fingerprint_mismatch = matches!(
        (&stream_fingerprint, &daily_fingerprint),
        (Some(stream), Some(daily)) if stream != daily
    );
    Ok(
        if stream_generation == daily_generation && !fingerprint_mismatch {
            DayMarkerPairStatus::Complete
        } else {
            DayMarkerPairStatus::Dirty
        },
    )
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

    fn stable_fingerprint() -> Result<String, String> {
        Ok("raw-fingerprint".to_owned())
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
    fn malformed_stream_marker_refuses_bump_without_changing_bytes() {
        let temporary = TempDir::new();
        let stream = marker_path(temporary.path(), HealthMarkerKind::Stream);
        fs::create_dir_all(stream.parent().unwrap()).unwrap();
        let malformed = b"not-json";
        fs::write(&stream, malformed).unwrap();

        assert!(bump_stream_marker(temporary.path(), DAY).is_err());
        assert_eq!(fs::read(stream).unwrap(), malformed);
    }

    #[test]
    fn malformed_or_future_daily_marker_refuses_publication_without_changing_bytes() {
        for bytes in [
            b"not-json".as_slice(),
            br#"{"version":1,"generation":2,"fingerprint":null}"#.as_slice(),
        ] {
            let temporary = TempDir::new();
            assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 1);
            let daily = marker_path(temporary.path(), HealthMarkerKind::Daily);
            fs::write(&daily, bytes).unwrap();

            assert!(
                publish_daily_marker_if_current(
                    temporary.path(),
                    DAY,
                    1,
                    "raw-fingerprint",
                    stable_fingerprint,
                )
                .is_err()
            );
            assert_eq!(fs::read(daily).unwrap(), bytes);
        }
    }

    #[test]
    fn marker_without_fingerprint_is_malformed() {
        let temporary = TempDir::new();
        let stream = marker_path(temporary.path(), HealthMarkerKind::Stream);
        fs::create_dir_all(stream.parent().unwrap()).unwrap();
        fs::write(&stream, br#"{"version":1,"generation":1}"#).unwrap();

        assert!(matches!(
            read_health_marker(temporary.path(), DAY, HealthMarkerKind::Stream).unwrap(),
            HealthMarkerState::MalformedNonEmpty { .. }
        ));
    }

    #[test]
    fn publish_reports_superseded_generation() {
        let temporary = TempDir::new();
        assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 1);
        assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 2);
        assert_eq!(
            publish_daily_marker_if_current(
                temporary.path(),
                DAY,
                1,
                "raw-fingerprint",
                stable_fingerprint,
            )
            .unwrap(),
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
            publish_daily_marker_if_current(
                &first_root,
                DAY,
                1,
                "raw-fingerprint",
                stable_fingerprint,
            )
            .unwrap()
        });
        let second_root = Arc::clone(&root);
        let second_start = Arc::clone(&start);
        let second = std::thread::spawn(move || {
            second_start.wait();
            publish_daily_marker_if_current(
                &second_root,
                DAY,
                1,
                "raw-fingerprint",
                stable_fingerprint,
            )
            .unwrap()
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
    fn real_stream_lock_serializes_an_adversarial_writer_and_finalizer() {
        let temporary = TempDir::new();
        assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 1);
        let stream = marker_path(temporary.path(), HealthMarkerKind::Stream);
        let held = hold_lock(&stream, LockOptions::default()).unwrap();
        let root = Arc::new(temporary.path().to_path_buf());
        let ready = Arc::new(Barrier::new(3));
        let writer_root = Arc::clone(&root);
        let writer_ready = Arc::clone(&ready);
        let writer = std::thread::spawn(move || {
            writer_ready.wait();
            bump_stream_marker(&writer_root, DAY).unwrap()
        });
        let finalizer_root = Arc::clone(&root);
        let finalizer_ready = Arc::clone(&ready);
        let finalizer = std::thread::spawn(move || {
            finalizer_ready.wait();
            publish_daily_marker_if_current(
                &finalizer_root,
                DAY,
                1,
                "raw-fingerprint",
                stable_fingerprint,
            )
            .unwrap()
        });
        ready.wait();
        drop(held);
        assert_eq!(writer.join().unwrap(), 2);
        match finalizer.join().unwrap() {
            PublishOutcome::Published(1) => assert_eq!(
                day_marker_pair_status(temporary.path(), DAY).unwrap(),
                DayMarkerPairStatus::Dirty
            ),
            PublishOutcome::Superseded(2) => assert!(matches!(
                read_health_marker(temporary.path(), DAY, HealthMarkerKind::Daily).unwrap(),
                HealthMarkerState::Absent
            )),
            outcome => panic!("unexpected finalizer outcome: {outcome:?}"),
        }
    }

    #[test]
    fn raw_fingerprint_is_compared_while_holding_the_real_stream_lock() {
        let temporary = TempDir::new();
        assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 1);
        let raw = temporary.path().join("raw-input");
        fs::write(&raw, "before").unwrap();
        let root = Arc::new(temporary.path().to_path_buf());
        let raw_for_finalizer = raw.clone();
        let entered_comparison = Arc::new(Barrier::new(2));
        let release_comparison = Arc::new(Barrier::new(2));
        let finalizer_entered = Arc::clone(&entered_comparison);
        let finalizer_release = Arc::clone(&release_comparison);
        let finalizer = std::thread::spawn(move || {
            publish_daily_marker_if_current(&root, DAY, 1, "before", move || {
                // This callback is invoked only after the shared stream
                // lock has been acquired. Let a raw writer race here.
                finalizer_entered.wait();
                finalizer_release.wait();
                fs::read_to_string(&raw_for_finalizer).map_err(|error| error.to_string())
            })
            .unwrap()
        });
        entered_comparison.wait();
        fs::write(&raw, "after").unwrap();
        release_comparison.wait();

        assert_eq!(finalizer.join().unwrap(), PublishOutcome::InputChanged(2));
        assert!(matches!(
            read_health_marker(temporary.path(), DAY, HealthMarkerKind::Stream).unwrap(),
            HealthMarkerState::Versioned {
                marker: HealthMarker { generation: 2, .. },
                ..
            }
        ));
        assert!(matches!(
            read_health_marker(temporary.path(), DAY, HealthMarkerKind::Daily).unwrap(),
            HealthMarkerState::Absent
        ));
    }

    #[test]
    fn stale_same_generation_daily_fingerprint_advances_the_dirty_generation() {
        let temporary = TempDir::new();
        assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 1);
        assert_eq!(
            publish_daily_marker_if_current(temporary.path(), DAY, 1, "before", || Ok(
                "before".to_owned()
            ),)
            .unwrap(),
            PublishOutcome::Published(1)
        );

        assert_eq!(
            publish_daily_marker_if_current(temporary.path(), DAY, 1, "after", || Ok(
                "after".to_owned()
            ),)
            .unwrap(),
            PublishOutcome::InputChanged(2)
        );
        assert_eq!(
            day_marker_pair_status(temporary.path(), DAY).unwrap(),
            DayMarkerPairStatus::Dirty
        );
    }

    #[test]
    fn same_generation_comparable_fingerprint_mismatch_is_dirty() {
        let temporary = TempDir::new();
        let stream = marker_path(temporary.path(), HealthMarkerKind::Stream);
        let daily = marker_path(temporary.path(), HealthMarkerKind::Daily);
        fs::create_dir_all(stream.parent().unwrap()).unwrap();
        write_marker(
            &stream,
            &HealthMarker {
                version: MARKER_VERSION,
                generation: 7,
                fingerprint: Some("new".to_owned()),
            },
        )
        .unwrap();
        write_marker(
            &daily,
            &HealthMarker {
                version: MARKER_VERSION,
                generation: 7,
                fingerprint: Some("old".to_owned()),
            },
        )
        .unwrap();

        assert_eq!(
            day_marker_pair_status(temporary.path(), DAY).unwrap(),
            DayMarkerPairStatus::Dirty
        );
    }

    #[test]
    fn bumped_stream_marker_writes_exact_compact_json() {
        let temporary = TempDir::new();
        assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 1);
        assert_eq!(
            fs::read(marker_path(temporary.path(), HealthMarkerKind::Stream)).unwrap(),
            br#"{"version":1,"generation":1,"fingerprint":null}"#
        );
    }

    #[test]
    fn marker_without_version_or_generation_is_malformed() {
        for bytes in [
            br#"{"generation":1,"fingerprint":null}"#.as_slice(),
            br#"{"version":1,"fingerprint":null}"#.as_slice(),
        ] {
            let temporary = TempDir::new();
            let stream = marker_path(temporary.path(), HealthMarkerKind::Stream);
            fs::create_dir_all(stream.parent().unwrap()).unwrap();
            fs::write(&stream, bytes).unwrap();
            assert!(matches!(
                read_health_marker(temporary.path(), DAY, HealthMarkerKind::Stream).unwrap(),
                HealthMarkerState::MalformedNonEmpty { .. }
            ));
        }
    }

    #[test]
    fn duplicate_generation_field_is_malformed() {
        let bytes = br#"{"version":1,"generation":1,"generation":2,"fingerprint":null}"#;
        assert!(serde_json::from_slice::<HealthMarker>(bytes).is_err());

        let temporary = TempDir::new();
        let stream = marker_path(temporary.path(), HealthMarkerKind::Stream);
        fs::create_dir_all(stream.parent().unwrap()).unwrap();
        fs::write(&stream, bytes).unwrap();
        assert!(matches!(
            read_health_marker(temporary.path(), DAY, HealthMarkerKind::Stream).unwrap(),
            HealthMarkerState::MalformedNonEmpty { .. }
        ));
    }

    #[test]
    fn unknown_marker_field_is_ignored() {
        let bytes = br#"{"version":1,"generation":1,"fingerprint":null,"extra":"ignored"}"#;
        assert_eq!(
            serde_json::from_slice::<HealthMarker>(bytes).unwrap(),
            HealthMarker {
                version: 1,
                generation: 1,
                fingerprint: None,
            }
        );

        let temporary = TempDir::new();
        let stream = marker_path(temporary.path(), HealthMarkerKind::Stream);
        fs::create_dir_all(stream.parent().unwrap()).unwrap();
        fs::write(&stream, bytes).unwrap();
        assert!(matches!(
            read_health_marker(temporary.path(), DAY, HealthMarkerKind::Stream).unwrap(),
            HealthMarkerState::Versioned {
                marker: HealthMarker {
                    generation: 1,
                    fingerprint: None,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn unsupported_marker_version_is_malformed() {
        let bytes = br#"{"version":2,"generation":1,"fingerprint":null}"#;
        let marker = serde_json::from_slice::<HealthMarker>(bytes).unwrap();
        assert_eq!(marker.version, 2);

        let temporary = TempDir::new();
        let stream = marker_path(temporary.path(), HealthMarkerKind::Stream);
        fs::create_dir_all(stream.parent().unwrap()).unwrap();
        fs::write(&stream, bytes).unwrap();
        assert!(matches!(
            read_health_marker(temporary.path(), DAY, HealthMarkerKind::Stream).unwrap(),
            HealthMarkerState::MalformedNonEmpty { .. }
        ));
    }

    #[test]
    fn read_io_error_includes_the_marker_path() {
        let temporary = TempDir::new();
        let expected = marker_path(temporary.path(), HealthMarkerKind::Stream);
        fs::create_dir_all(&expected).unwrap();
        let error =
            read_health_marker(temporary.path(), DAY, HealthMarkerKind::Stream).unwrap_err();
        assert!(matches!(
            &error,
            HealthMarkerError::Io { path, .. } if path == &expected
        ));
        assert!(error.to_string().contains(&expected.display().to_string()));
    }

    #[test]
    fn overflow_refuses_bump_without_changing_bytes() {
        let temporary = TempDir::new();
        let stream = marker_path(temporary.path(), HealthMarkerKind::Stream);
        write_marker(
            &stream,
            &HealthMarker {
                version: MARKER_VERSION,
                generation: u64::MAX,
                fingerprint: None,
            },
        )
        .unwrap();
        let before = fs::read(&stream).unwrap();

        let error = bump_stream_marker(temporary.path(), DAY).unwrap_err();
        assert!(matches!(
            error,
            AtomicWriteError::Io { path, source }
                if path == stream && source.to_string() == "stream marker generation overflow"
        ));
        assert_eq!(fs::read(&stream).unwrap(), before);
    }

    #[test]
    fn overflow_refuses_publish_without_changing_bytes() {
        let temporary = TempDir::new();
        let stream = marker_path(temporary.path(), HealthMarkerKind::Stream);
        write_marker(
            &stream,
            &HealthMarker {
                version: MARKER_VERSION,
                generation: u64::MAX,
                fingerprint: None,
            },
        )
        .unwrap();
        let before = fs::read(&stream).unwrap();
        let error =
            publish_daily_marker_if_current(temporary.path(), DAY, u64::MAX, "before", || {
                Ok("after".to_owned())
            })
            .unwrap_err();
        assert!(matches!(
            error,
            HealthMarkerError::Atomic(AtomicWriteError::Io { path, source })
                if path == stream && source.to_string() == "stream marker generation overflow"
        ));
        assert_eq!(fs::read(&stream).unwrap(), before);

        let temporary = TempDir::new();
        let stream = marker_path(temporary.path(), HealthMarkerKind::Stream);
        let daily = marker_path(temporary.path(), HealthMarkerKind::Daily);
        write_marker(
            &stream,
            &HealthMarker {
                version: MARKER_VERSION,
                generation: u64::MAX,
                fingerprint: None,
            },
        )
        .unwrap();
        write_marker(
            &daily,
            &HealthMarker {
                version: MARKER_VERSION,
                generation: u64::MAX,
                fingerprint: Some("old".to_owned()),
            },
        )
        .unwrap();
        let before = fs::read(&stream).unwrap();
        let daily_before = fs::read(&daily).unwrap();
        let error =
            publish_daily_marker_if_current(temporary.path(), DAY, u64::MAX, "after", || {
                Ok("after".to_owned())
            })
            .unwrap_err();
        assert!(matches!(
            error,
            HealthMarkerError::Atomic(AtomicWriteError::Io { path, source })
                if path == stream && source.to_string() == "stream marker generation overflow"
        ));
        assert_eq!(fs::read(&stream).unwrap(), before);
        assert_eq!(fs::read(&daily).unwrap(), daily_before);
    }

    #[test]
    fn absent_stream_marker_is_complete() {
        let temporary = TempDir::new();
        assert_eq!(
            day_marker_pair_status(temporary.path(), DAY).unwrap(),
            DayMarkerPairStatus::Complete
        );
    }

    #[test]
    fn stream_without_daily_marker_is_dirty() {
        let temporary = TempDir::new();
        assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 1);
        assert_eq!(
            day_marker_pair_status(temporary.path(), DAY).unwrap(),
            DayMarkerPairStatus::Dirty
        );
    }

    #[test]
    fn equal_generation_pair_without_fingerprint_mismatch_is_complete() {
        let temporary = TempDir::new();
        assert_eq!(bump_stream_marker(temporary.path(), DAY).unwrap(), 1);
        assert_eq!(
            publish_daily_marker_if_current(
                temporary.path(),
                DAY,
                1,
                "raw-fingerprint",
                stable_fingerprint,
            )
            .unwrap(),
            PublishOutcome::Published(1)
        );
        assert_eq!(
            day_marker_pair_status(temporary.path(), DAY).unwrap(),
            DayMarkerPairStatus::Complete
        );
    }

    #[test]
    fn equal_generation_pair_with_matching_fingerprints_is_complete() {
        let temporary = TempDir::new();
        let marker = HealthMarker {
            version: MARKER_VERSION,
            generation: 1,
            fingerprint: Some("same-fingerprint".to_owned()),
        };
        write_marker(
            &marker_path(temporary.path(), HealthMarkerKind::Stream),
            &marker,
        )
        .unwrap();
        write_marker(
            &marker_path(temporary.path(), HealthMarkerKind::Daily),
            &marker,
        )
        .unwrap();

        assert_eq!(
            day_marker_pair_status(temporary.path(), DAY).unwrap(),
            DayMarkerPairStatus::Complete
        );
    }
}
