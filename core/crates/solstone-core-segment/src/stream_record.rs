// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, DEFAULT_STREAM, FileLock, JsonWriteOptions, LockOptions,
    MalformedPolicy, ReadError, hold_lock, read_json, write_bytes_exclusive, write_json,
};

use crate::device::validate_did;
use crate::projection::name_with_ordinal;
use crate::{Kind, SegmentDir, SegmentError, project_stream_name};

const REGISTRY_LOCK_NAME: &str = ".registry";

/// A persistent stream state record, accepting Python's legacy `type` field.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StreamRecord {
    pub name: String,
    #[serde(alias = "type")]
    pub kind: String,
    pub host: Option<String>,
    pub platform: Option<String>,
    pub created_at: u64,
    pub last_day: Option<String>,
    pub last_segment: Option<String>,
    pub seq: u64,
    #[serde(default)]
    pub did: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StreamHints {
    pub kind: Option<Kind>,
    pub host: Option<String>,
    pub platform: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamAdvance {
    pub prev_day: Option<String>,
    pub prev_segment: Option<String>,
    pub seq: u64,
}

/// A selected stream and its segment advancement result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedStream {
    pub stream: String,
    pub segment: SegmentDir,
    pub advance: StreamAdvance,
}

#[derive(Serialize)]
struct StreamMarker {
    stream: String,
    prev_day: Option<String>,
    prev_segment: Option<String>,
    seq: u64,
}

#[derive(Clone, Copy)]
struct StreamBinding<'a> {
    did: &'a str,
    source: &'a str,
}

/// Resolve a device-owned stream by authenticated identity, then advance it.
pub fn resolve_stream(
    journal: &Path,
    day: &str,
    segment: &str,
    label: &str,
    did: &str,
    source: &str,
    hints: StreamHints,
) -> Result<ResolvedStream, SegmentError> {
    validate_did(did)?;
    let _ = SegmentDir::resolve(journal, day, segment, DEFAULT_STREAM)?;
    let binding = StreamBinding { did, source };
    loop {
        let name = {
            let registry_target = journal.join("streams").join(REGISTRY_LOCK_NAME);
            let registry_lock = hold_lock(registry_target, LockOptions::default())?;
            allocate(&registry_lock, journal, label, binding, &hints)?
        };
        let segment_dir = SegmentDir::resolve(journal, day, segment, &name)?;
        match advance_stream(&name, day, segment, &segment_dir, hints.clone(), binding) {
            Ok(advance) => {
                return Ok(ResolvedStream {
                    stream: name,
                    segment: segment_dir,
                    advance,
                });
            }
            Err(SegmentError::StreamBindingConflict { .. }) => {
                // A non-native writer may have replaced a reservation after the
                // registry lock was released. Re-enter allocation rather than
                // adopting its record.
            }
            Err(error) => return Err(error),
        }
    }
}

/// Allocate under the caller-held registry guard. Registration is infrequent,
/// so one small flat-directory scan is preferable to a second index or cache.
fn allocate(
    _registry_lock: &FileLock,
    journal: &Path,
    label: &str,
    binding: StreamBinding<'_>,
    hints: &StreamHints,
) -> Result<String, SegmentError> {
    let records = read_registry_records(journal)?;
    if let Some((name, _)) = records
        .iter()
        .find(|(_, record)| binding_matches(record, binding))
    {
        return Ok(name.clone());
    }

    let base = project_stream_name(label, binding.source);
    for ordinal in 1.. {
        let name = name_with_ordinal(&base, ordinal);
        if records.contains_key(&name) {
            continue;
        }
        let path = stream_record_path(journal, &name);
        let record = reservation_record(name.clone(), binding, hints)?;
        let bytes = serde_json::to_vec(&record).map_err(|source| SegmentError::Serialization {
            path: path.clone(),
            source,
        })?;
        match write_bytes_exclusive(&path, &bytes, AtomicWriteOptions::default()) {
            Ok(()) => return Ok(name),
            Err(AtomicWriteError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        }
    }
    unreachable!("u64 ordinal iterator is inexhaustible")
}

/// Advance one reserved stream, then atomically write its matching segment marker.
///
/// This is deliberately two durable writes, not a cross-file transaction. The
/// state is written before the marker because stream-state rebuild tooling
/// treats markers as ground truth and can recover by skipping an orphaned
/// advance after a marker failure. That rebuild tooling is outside this crate.
fn advance_stream(
    name: &str,
    day: &str,
    segment: &str,
    segment_dir: &SegmentDir,
    hints: StreamHints,
    binding: StreamBinding<'_>,
) -> Result<StreamAdvance, SegmentError> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(SegmentError::StreamInput(
            "stream name must be a plain path component",
        ));
    }
    if day != segment_dir.day || segment != segment_dir.segment || name != segment_dir.stream {
        return Err(SegmentError::StreamInput(
            "stream advance does not match segment directory",
        ));
    }
    let state_path = stream_record_path(&segment_dir.journal, name);
    let _lock = hold_lock(&state_path, LockOptions::default())?;
    let record = read_stream_record(&state_path)?;
    if let Some(record) = record.as_ref()
        && !binding_matches(record, binding)
    {
        return Err(SegmentError::StreamBindingConflict {
            name: name.to_owned(),
        });
    }
    let (record, advance) = update_record(record, name, day, segment, hints, binding)?;
    write_stream_record(&state_path, &record)?;
    let marker = StreamMarker {
        stream: name.to_owned(),
        prev_day: advance.prev_day.clone(),
        prev_segment: advance.prev_segment.clone(),
        seq: advance.seq,
    };
    write_json(
        segment_dir.path.join("stream.json"),
        &marker,
        JsonWriteOptions::default(),
    )?;
    Ok(advance)
}

fn read_registry_records(journal: &Path) -> Result<BTreeMap<String, StreamRecord>, SegmentError> {
    let directory = journal.join("streams");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(source) => {
            return Err(SegmentError::Io {
                path: directory,
                source,
            });
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| SegmentError::Io {
            path: directory.clone(),
            source,
        })?;
        let path = entry.path();
        if entry
            .file_type()
            .map_err(|source| SegmentError::Io {
                path: path.clone(),
                source,
            })?
            .is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            paths.push(path);
        }
    }
    paths.sort();
    let mut records = BTreeMap::new();
    for path in paths {
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let record =
            read_stream_record(&path)?.ok_or_else(|| SegmentError::MalformedStreamRecord {
                path: path.clone(),
                source: ReadError::Malformed(solstone_core_journal_io::MalformedDataError {
                    path: path.clone(),
                    line: None,
                    source: serde_json::Error::io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "empty stream record",
                    )),
                }),
            })?;
        records.insert(name.to_owned(), record);
    }
    Ok(records)
}

fn stream_record_path(journal: &Path, name: &str) -> PathBuf {
    journal.join("streams").join(format!("{name}.json"))
}

fn read_stream_record(path: &Path) -> Result<Option<StreamRecord>, SegmentError> {
    match read_json(path, None, MalformedPolicy::Raise) {
        Ok(record) => Ok(record),
        Err(error @ ReadError::Malformed(_)) => Err(SegmentError::MalformedStreamRecord {
            path: path.to_path_buf(),
            source: error,
        }),
        Err(error) => Err(SegmentError::Read(error)),
    }
}

fn write_stream_record(path: &Path, record: &StreamRecord) -> Result<(), SegmentError> {
    write_json(path, record, JsonWriteOptions::default())?;
    Ok(())
}

fn reservation_record(
    name: String,
    binding: StreamBinding<'_>,
    hints: &StreamHints,
) -> Result<StreamRecord, SegmentError> {
    Ok(StreamRecord {
        name,
        kind: hints
            .kind
            .as_ref()
            .map(Kind::compat_label)
            .unwrap_or("unknown")
            .to_owned(),
        host: hints.host.clone(),
        platform: hints.platform.clone(),
        created_at: now_unix_seconds()?,
        last_day: None,
        last_segment: None,
        seq: 0,
        did: Some(binding.did.to_owned()),
        source: Some(binding.source.to_owned()),
    })
}

fn update_record(
    record: Option<StreamRecord>,
    name: &str,
    day: &str,
    segment: &str,
    hints: StreamHints,
    binding: StreamBinding<'_>,
) -> Result<(StreamRecord, StreamAdvance), SegmentError> {
    match record {
        None => {
            let mut record = reservation_record(name.to_owned(), binding, &hints)?;
            record.last_day = Some(day.to_owned());
            record.last_segment = Some(segment.to_owned());
            record.seq = 1;
            Ok((
                record,
                StreamAdvance {
                    prev_day: None,
                    prev_segment: None,
                    seq: 1,
                },
            ))
        }
        Some(mut record) => {
            let prev_day = record.last_day.clone();
            let prev_segment = record.last_segment.clone();
            let seq = record
                .seq
                .checked_add(1)
                .ok_or(SegmentError::StreamInput("stream sequence overflow"))?;
            record.last_day = Some(day.to_owned());
            record.last_segment = Some(segment.to_owned());
            record.seq = seq;
            if let Some(kind) = hints.kind {
                record.kind = kind.compat_label().to_owned();
            }
            if let Some(host) = hints.host {
                record.host = Some(host);
            }
            if let Some(platform) = hints.platform {
                record.platform = Some(platform);
            }
            Ok((
                record,
                StreamAdvance {
                    prev_day,
                    prev_segment,
                    seq,
                },
            ))
        }
    }
}

fn binding_matches(record: &StreamRecord, binding: StreamBinding<'_>) -> bool {
    record.did.as_deref() == Some(binding.did) && record.source.as_deref() == Some(binding.source)
}

fn now_unix_seconds() -> Result<u64, SegmentError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| SegmentError::Io {
            path: PathBuf::from("stream created_at"),
            source: std::io::Error::other(error),
        })
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::thread;
    use std::time::Duration;

    use serde_json::Value;
    use solstone_core_journal_io::{hold_lock, write_json};

    use crate::test_support::TempDir;

    use super::*;

    const DID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const DID_C: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const DID_D: &str = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    const DID_E: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

    fn hints() -> StreamHints {
        StreamHints {
            kind: Some(Kind::Observed),
            host: None,
            platform: None,
        }
    }

    fn record(
        name: &str,
        did: Option<&str>,
        source: Option<&str>,
        seq: u64,
        created_at: u64,
    ) -> StreamRecord {
        StreamRecord {
            name: name.to_owned(),
            kind: "observer".to_owned(),
            host: None,
            platform: None,
            created_at,
            last_day: None,
            last_segment: None,
            seq,
            did: did.map(str::to_owned),
            source: source.map(str::to_owned),
        }
    }

    fn write_record(root: &Path, record: &StreamRecord) {
        write_json(
            stream_record_path(root, &record.name),
            record,
            JsonWriteOptions::default(),
        )
        .unwrap();
    }

    #[test]
    fn malformed_state_refuses_without_changing_bytes() {
        let temporary = TempDir::new();
        let state = temporary.path().join("streams/workstation.json");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        fs::write(&state, b"{not json\n").unwrap();
        let before = fs::read(&state).unwrap();
        let segment =
            SegmentDir::resolve(temporary.path(), "20260804", "120000_60", "workstation").unwrap();

        let result = advance_stream(
            "workstation",
            "20260804",
            "120000_60",
            &segment,
            hints(),
            StreamBinding {
                did: DID_A,
                source: "",
            },
        );
        assert!(matches!(
            result,
            Err(SegmentError::MalformedStreamRecord { .. })
        ));
        assert_eq!(fs::read(state).unwrap(), before);
    }

    #[test]
    fn bool_sequence_is_rejected() {
        let temporary = TempDir::new();
        let state = temporary.path().join("streams/workstation.json");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        fs::write(&state, br#"{"name":"workstation","type":"observer","host":null,"platform":null,"created_at":1,"last_day":null,"last_segment":null,"seq":true}"#).unwrap();
        let segment =
            SegmentDir::resolve(temporary.path(), "20260804", "120000_60", "workstation").unwrap();
        assert!(matches!(
            advance_stream(
                "workstation",
                "20260804",
                "120000_60",
                &segment,
                hints(),
                StreamBinding {
                    did: DID_A,
                    source: ""
                },
            ),
            Err(SegmentError::MalformedStreamRecord { .. })
        ));
    }

    #[test]
    fn stream_advance_failure_propagates() {
        let temporary = TempDir::new();
        fs::write(temporary.path().join("streams"), b"not a directory").unwrap();
        assert!(
            resolve_stream(
                temporary.path(),
                "20260804",
                "120000_60",
                "workstation",
                DID_A,
                "",
                hints(),
            )
            .is_err()
        );
    }

    #[test]
    fn marker_write_failure_is_returned_after_state_advance() {
        let temporary = TempDir::new();
        let segment =
            SegmentDir::resolve(temporary.path(), "20260804", "120000_60", "workstation").unwrap();
        let marker_parent = temporary.path().join("chronicle/20260804/workstation");
        fs::create_dir_all(marker_parent.parent().unwrap()).unwrap();
        fs::write(&marker_parent, b"not a directory").unwrap();

        assert!(
            advance_stream(
                "workstation",
                "20260804",
                "120000_60",
                &segment,
                hints(),
                StreamBinding {
                    did: DID_A,
                    source: ""
                },
            )
            .is_err()
        );

        let state = temporary.path().join("streams/workstation.json");
        let record: StreamRecord = serde_json::from_slice(&fs::read(state).unwrap()).unwrap();
        assert_eq!(record.seq, 1);
    }

    #[test]
    fn records_from_other_or_legacy_producers_are_not_adopted() {
        let temporary = TempDir::new();
        write_record(temporary.path(), &record("iphone", None, None, 41, 7));
        let resolved = resolve_stream(
            temporary.path(),
            "20260804",
            "120000_60",
            "iPhone",
            DID_A,
            "",
            hints(),
        )
        .unwrap();
        assert_eq!(resolved.stream, "iphone_2");
        let created: StreamRecord = serde_json::from_slice(
            &fs::read(temporary.path().join("streams/iphone_2.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(created.seq, 1);
        assert_ne!(created.created_at, 7);

        write_record(
            temporary.path(),
            &record("my_phone", Some(DID_A), Some(""), 1, 9),
        );
        let other = resolve_stream(
            temporary.path(),
            "20260804",
            "120100_60",
            "my.phone",
            DID_B,
            "",
            hints(),
        )
        .unwrap();
        assert_eq!(other.stream, "my_phone_2");
        let other_record: StreamRecord = serde_json::from_slice(
            &fs::read(temporary.path().join("streams/my_phone_2.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(other_record.seq, 1);
        assert_ne!(other_record.created_at, 9);
    }

    #[test]
    fn identical_projections_allocate_distinct_directories() {
        let temporary = TempDir::new();
        let first = resolve_stream(
            temporary.path(),
            "20260804",
            "120000_60",
            "my.phone",
            DID_A,
            "",
            hints(),
        )
        .unwrap();
        let second = resolve_stream(
            temporary.path(),
            "20260804",
            "120100_60",
            "my_phone",
            DID_B,
            "",
            hints(),
        )
        .unwrap();
        assert_eq!(
            (first.stream.as_str(), second.stream.as_str()),
            ("my_phone", "my_phone_2")
        );

        let phone_one = resolve_stream(
            temporary.path(),
            "20260804",
            "120200_60",
            "iPhone",
            DID_C,
            "",
            hints(),
        )
        .unwrap();
        let phone_two = resolve_stream(
            temporary.path(),
            "20260804",
            "120300_60",
            "iPhone",
            DID_D,
            "",
            hints(),
        )
        .unwrap();
        let parenthesized = resolve_stream(
            temporary.path(),
            "20260804",
            "120400_60",
            "iPhone (2)",
            DID_E,
            "",
            hints(),
        )
        .unwrap();
        assert_eq!(phone_one.stream, "iphone");
        assert_eq!(phone_two.stream, "iphone_2");
        assert_eq!(parenthesized.stream, "iphone_2_2");
    }

    #[test]
    fn advance_reallocates_past_a_legacy_record_that_arrives_after_reservation() {
        let temporary = TempDir::new();
        let state = stream_record_path(temporary.path(), "iphone");
        let held = hold_lock(&state, LockOptions::default()).unwrap();
        let root = temporary.path().to_path_buf();
        let worker = thread::spawn(move || {
            resolve_stream(&root, "20260804", "120000_60", "iPhone", DID_A, "", hints()).unwrap()
        });
        for _ in 0..100 {
            if state.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(state.exists(), "resolver did not create its reservation");
        write_json(
            &state,
            &record("iphone", None, None, 9, 11),
            JsonWriteOptions::default(),
        )
        .unwrap();
        drop(held);
        assert_eq!(worker.join().unwrap().stream, "iphone_2");
    }

    #[test]
    fn registration_and_resolution_agree_and_seq_zero_advances_to_one() {
        let temporary = TempDir::new();
        let registry = hold_lock(
            temporary.path().join("streams").join(REGISTRY_LOCK_NAME),
            LockOptions::default(),
        )
        .unwrap();
        let binding = StreamBinding {
            did: DID_A,
            source: "",
        };
        let registered =
            allocate(&registry, temporary.path(), "iPhone", binding, &hints()).unwrap();
        assert_eq!(registered, "iphone");
        drop(registry);
        let resolved = resolve_stream(
            temporary.path(),
            "20260804",
            "120000_60",
            "iPhone",
            DID_A,
            "",
            hints(),
        )
        .unwrap();
        assert_eq!(resolved.stream, registered);
        assert_eq!(resolved.advance.seq, 1);
    }

    #[test]
    fn allocator_reads_after_the_caller_held_guard() {
        let temporary = TempDir::new();
        let registry = hold_lock(
            temporary.path().join("streams").join(REGISTRY_LOCK_NAME),
            LockOptions::default(),
        )
        .unwrap();
        write_record(
            temporary.path(),
            &record("iphone", Some(DID_B), Some(""), 1, 1),
        );
        let allocated = allocate(
            &registry,
            temporary.path(),
            "iPhone",
            StreamBinding {
                did: DID_A,
                source: "",
            },
            &hints(),
        )
        .unwrap();
        assert_eq!(allocated, "iphone_2");
    }

    #[test]
    fn allocator_skips_a_foreign_reservation_collision() {
        let temporary = TempDir::new();
        let registry = hold_lock(
            temporary.path().join("streams").join(REGISTRY_LOCK_NAME),
            LockOptions::default(),
        )
        .unwrap();
        write_record(
            temporary.path(),
            &record("iphone", Some(DID_B), Some(""), 1, 1),
        );

        let allocated = allocate(
            &registry,
            temporary.path(),
            "iPhone",
            StreamBinding {
                did: DID_A,
                source: "",
            },
            &hints(),
        )
        .unwrap();

        assert_eq!(allocated, "iphone_2");
    }

    #[test]
    fn colliding_sources_and_repeat_bindings_remain_distinct_and_idempotent() {
        let temporary = TempDir::new();
        let first = resolve_stream(
            temporary.path(),
            "20260804",
            "120000_60",
            "iPhone",
            DID_A,
            "WATCH",
            hints(),
        )
        .unwrap();
        let second = resolve_stream(
            temporary.path(),
            "20260804",
            "120100_60",
            "iPhone",
            DID_A,
            "watch",
            hints(),
        )
        .unwrap();
        let repeated = resolve_stream(
            temporary.path(),
            "20260804",
            "120200_60",
            "iPhone",
            DID_A,
            "WATCH",
            hints(),
        )
        .unwrap();
        let primary = resolve_stream(
            temporary.path(),
            "20260804",
            "120300_60",
            "iPhone",
            DID_A,
            "",
            hints(),
        )
        .unwrap();
        assert_eq!(first.stream, "iphone_watch");
        assert_eq!(second.stream, "iphone_watch_2");
        assert_eq!(repeated.stream, first.stream);
        assert_ne!(primary.stream, first.stream);
        assert_ne!(primary.stream, second.stream);
        let first_record: StreamRecord = serde_json::from_slice(
            &fs::read(temporary.path().join("streams/iphone_watch.json")).unwrap(),
        )
        .unwrap();
        let second_record: StreamRecord = serde_json::from_slice(
            &fs::read(temporary.path().join("streams/iphone_watch_2.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(first_record.source.as_deref(), Some("WATCH"));
        assert_eq!(second_record.source.as_deref(), Some("watch"));
        let names: BTreeSet<String> = fs::read_dir(temporary.path().join("streams"))
            .unwrap()
            .map(Result::unwrap)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .map(|path| path.file_stem().unwrap().to_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            names,
            BTreeSet::from([
                "iphone".to_owned(),
                "iphone_watch".to_owned(),
                "iphone_watch_2".to_owned(),
            ])
        );
    }

    #[test]
    fn legacy_fixture_names_remain_accepted_and_invalid_components_are_refused() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../fixtures/stream-name-projection-vectors.json"
        ))
        .unwrap();
        let temporary = TempDir::new();
        for vector in fixture["legacy"].as_array().unwrap() {
            let name = vector["expect"].as_str().unwrap();
            assert!(SegmentDir::resolve(temporary.path(), "20260804", "120000_60", name).is_ok());
        }
        for invalid in [
            "nested/name",
            "nested\\name",
            "..",
            ".registry",
            "Uppercase",
            "",
        ] {
            assert!(
                SegmentDir::resolve(temporary.path(), "20260804", "120000_60", invalid).is_err()
            );
        }
    }
}
