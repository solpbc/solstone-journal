// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Non-healing, read-only access to validated private agent-memory originals.

use std::error::Error;
use std::ffi::OsStr;
use std::io;
#[cfg(unix)]
use std::os::fd::AsFd;
#[cfg(windows)]
use std::os::windows::io::AsHandle;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use solstone_core_format::agent_memory::{
    ChainPredecessor, Coordinate, MAX_METADATA_BYTES, MAX_NOTE_BYTES, Origin, ReadyDocument,
    SourceKey, validate_complete_original, validate_coordinate,
};
use solstone_core_journal_io::errors::FlatDirectoryError;
use solstone_core_journal_io::journal_root::JournalRoot;
use solstone_core_journal_io::{
    BoundParentLock, ExactLookupError, FlatDirectoryError as DirectoryError,
    open_existing_parent_lock_bound, read_relative_file_bounded, resolve_segment_exact,
    resolve_stream_exact,
};
use solstone_core_segment::{
    OwnerDeletionState, SegmentDir, owner_deletion_state, read_agent_memory_chain,
};

/// Result of reading one original without healing or changing journal state.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum OriginalRead {
    Absent,
    Unready,
    Deleted,
    Staged,
    Corrupt,
    Unavailable {
        detail: String,
    },
    Ready {
        bytes: Vec<u8>,
        origin: Origin,
        ready: ReadyDocument,
    },
}

/// Read one source-bound original. A missing path is an observation only and
/// never creates a source namespace, chronicle directory, lock, or marker.
pub fn read_original(
    journal: &Path,
    source_key: &SourceKey,
    coordinate: &Coordinate,
) -> OriginalRead {
    read_original_until(
        journal,
        source_key,
        coordinate,
        Instant::now() + Duration::from_secs(5),
    )
}

/// The same original read under a caller's existing absolute deadline.
pub fn read_original_until(
    journal: &Path,
    source_key: &SourceKey,
    coordinate: &Coordinate,
    deadline: Instant,
) -> OriginalRead {
    if source_key.validate().is_err() || validate_coordinate(coordinate, source_key).is_err() {
        return OriginalRead::Corrupt;
    }
    if matches!(std::fs::symlink_metadata(journal), Err(error) if error.kind() == io::ErrorKind::NotFound)
    {
        return OriginalRead::Absent;
    }

    let root = match JournalRoot::open(journal) {
        Ok(root) => root,
        Err(solstone_core_journal_io::JournalRootError::Io { source, .. })
            if source.kind() == io::ErrorKind::NotFound =>
        {
            return OriginalRead::Absent;
        }
        Err(error) => return unavailable(error),
    };
    let namespace = match std::fs::canonicalize(root.canonical_path()) {
        Ok(path) => path,
        Err(error) => return unavailable(error),
    };
    if let Err(error) = root.revalidate_canonical_binding() {
        return unavailable(error);
    }

    match resolve_stream_exact(&namespace, &coordinate.day, &coordinate.stream) {
        Ok(Some(_)) => {}
        Ok(None) => return OriginalRead::Absent,
        Err(error) => return lookup_unavailable_or_absent(error),
    }

    let live_guard = match read_live_guard(&root, coordinate, deadline) {
        Ok(guard) => guard,
        Err(detail) => return OriginalRead::Unavailable { detail },
    };

    let segment_path = match resolve_segment_exact(
        &namespace,
        &coordinate.day,
        &coordinate.stream,
        &coordinate.segment,
    ) {
        Ok(Some(path)) => path,
        Ok(None) => {
            let expected = match SegmentDir::resolve(
                journal,
                &coordinate.day,
                &coordinate.segment,
                &coordinate.stream,
            ) {
                Ok(segment) => segment,
                Err(error) => return unavailable(error),
            };
            return match owner_deletion_state(expected.path()) {
                Ok(OwnerDeletionState::Deleted) => OriginalRead::Deleted,
                Ok(OwnerDeletionState::Staged) => OriginalRead::Staged,
                Ok(OwnerDeletionState::Live) => OriginalRead::Absent,
                Ok(OwnerDeletionState::Occupied) => OriginalRead::Unavailable {
                    detail: "deletion marker refused".into(),
                },
                Err(error) => unavailable(error),
            };
        }
        Err(error) => return lookup_unavailable_or_absent(error),
    };

    let segment = match SegmentDir::resolve(
        journal,
        &coordinate.day,
        &coordinate.segment,
        &coordinate.stream,
    ) {
        Ok(segment) if segment.path() == segment_path => segment,
        Ok(_) => {
            return OriginalRead::Unavailable {
                detail: "segment path changed".into(),
            };
        }
        Err(error) => return unavailable(error),
    };
    match owner_deletion_state(segment.path()) {
        Ok(OwnerDeletionState::Deleted) => return OriginalRead::Deleted,
        Ok(OwnerDeletionState::Staged) => return OriginalRead::Staged,
        Ok(OwnerDeletionState::Live) => {}
        Ok(OwnerDeletionState::Occupied) => {
            return OriginalRead::Unavailable {
                detail: "deletion marker refused".into(),
            };
        }
        Err(error) => return unavailable(error),
    }
    if live_guard.is_none() {
        return OriginalRead::Unavailable {
            detail: "original live-name lock is absent".into(),
        };
    }

    let relative_segment = PathBuf::from("chronicle")
        .join(&coordinate.day)
        .join(&coordinate.stream)
        .join(&coordinate.segment);
    let ready = match read_json_bounded::<ReadyDocument>(
        &root,
        &relative_segment.join("ready.json"),
        MAX_METADATA_BYTES,
    ) {
        Ok(Some(document)) => document,
        Ok(None) => return OriginalRead::Unready,
        Err(ReadFailure::Corrupt) => return OriginalRead::Corrupt,
        Err(ReadFailure::Unavailable(detail)) => return OriginalRead::Unavailable { detail },
    };

    let bytes =
        match read_relative_file_bounded(&root, &relative_segment.join("note.txt"), MAX_NOTE_BYTES)
        {
            Ok(Some(observed)) => observed.bytes,
            Ok(None) => return OriginalRead::Corrupt,
            Err(FlatDirectoryError::SizeLimitExceeded { .. }) => return OriginalRead::Corrupt,
            Err(error) => {
                return OriginalRead::Unavailable {
                    detail: error.to_string(),
                };
            }
        };
    if bytes.is_empty() || std::str::from_utf8(&bytes).is_err() {
        return OriginalRead::Corrupt;
    }
    let origin = match read_json_bounded::<Origin>(
        &root,
        &relative_segment.join("origin.json"),
        MAX_METADATA_BYTES,
    ) {
        Ok(Some(origin)) => origin,
        Ok(None) => return OriginalRead::Corrupt,
        Err(ReadFailure::Corrupt) => return OriginalRead::Corrupt,
        Err(ReadFailure::Unavailable(detail)) => return OriginalRead::Unavailable { detail },
    };
    let chain = match read_agent_memory_chain(&segment) {
        Ok(Some(chain)) => ChainPredecessor {
            prev_day: chain.prev_day,
            prev_segment: chain.prev_segment,
            seq: chain.seq,
        },
        Ok(None) => return OriginalRead::Corrupt,
        Err(error) if segment_error_is_corrupt(&error) => return OriginalRead::Corrupt,
        Err(error) => {
            return OriginalRead::Unavailable {
                detail: error.to_string(),
            };
        }
    };

    if validate_complete_original(&ready, source_key, coordinate, &origin, &chain, &bytes).is_err()
    {
        return OriginalRead::Corrupt;
    }
    if Instant::now() >= deadline {
        return OriginalRead::Unavailable {
            detail: "original read deadline exhausted".into(),
        };
    }
    if let Err(error) = root.revalidate_canonical_binding() {
        return unavailable(error);
    }
    OriginalRead::Ready {
        bytes,
        origin,
        ready,
    }
}

#[cfg(unix)]
type ReadDirectory = solstone_core_journal_io::FlatDirectory;
#[cfg(windows)]
type ReadDirectory = solstone_core_journal_io::WindowsFlatDirectory;

#[cfg(unix)]
fn open_child(
    parent: &impl AsFd,
    name: &str,
    diagnostic: &Path,
) -> Result<Option<ReadDirectory>, DirectoryError> {
    solstone_core_journal_io::open_flat_directory_bound(parent, OsStr::new(name), diagnostic)
}
#[cfg(windows)]
fn open_child(
    parent: &impl AsHandle,
    name: &str,
    diagnostic: &Path,
) -> Result<Option<ReadDirectory>, DirectoryError> {
    solstone_core_journal_io::open_windows_flat_directory_bound(
        parent,
        OsStr::new(name),
        diagnostic,
    )
}

fn read_live_guard(
    root: &JournalRoot,
    coordinate: &Coordinate,
    deadline: Instant,
) -> Result<Option<BoundParentLock>, String> {
    let diagnostic = root.canonical_path();
    let Some(chronicle) =
        open_child(root, "chronicle", diagnostic).map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let diagnostic = diagnostic.join("chronicle");
    let Some(day) =
        open_child(&chronicle, &coordinate.day, &diagnostic).map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let diagnostic = diagnostic.join(&coordinate.day);
    let Some(stream) =
        open_child(&day, &coordinate.stream, &diagnostic).map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or("original read deadline exhausted")?;
    open_existing_parent_lock_bound(
        &stream,
        OsStr::new(&format!("{}.lock", coordinate.segment)),
        remaining,
        Duration::from_millis(10),
    )
    .map_err(|error| error.to_string())
}

/// Observe and hold this source's already-published mutation sidecar.
/// No source directory or sidecar is created by this read path.
pub fn read_source_guard(
    journal: &Path,
    source_key: &SourceKey,
    deadline: Instant,
) -> Result<Option<BoundParentLock>, String> {
    source_key.validate().map_err(|error| error.to_string())?;
    if matches!(std::fs::symlink_metadata(journal), Err(error) if error.kind() == io::ErrorKind::NotFound)
    {
        return Ok(None);
    }
    let root = match JournalRoot::open(journal) {
        Ok(root) => root,
        Err(solstone_core_journal_io::JournalRootError::Io { source, .. })
            if source.kind() == io::ErrorKind::NotFound =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.to_string()),
    };
    let Some(streams) =
        open_child(&root, "streams", root.canonical_path()).map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or("original read deadline exhausted")?;
    let guard = open_existing_parent_lock_bound(
        &streams,
        OsStr::new(&format!(".source-{}.mutation.lock", source_key.component())),
        remaining,
        Duration::from_millis(10),
    )
    .map_err(|error| error.to_string())?;
    root.revalidate_canonical_binding()
        .map_err(|error| error.to_string())?;
    Ok(guard)
}

enum ReadFailure {
    Corrupt,
    Unavailable(String),
}

fn read_json_bounded<T: DeserializeOwned>(
    root: &JournalRoot,
    relative: &Path,
    maximum: usize,
) -> Result<Option<T>, ReadFailure> {
    let observed =
        read_relative_file_bounded(root, relative, maximum).map_err(|error| match error {
            FlatDirectoryError::SizeLimitExceeded { .. } => ReadFailure::Corrupt,
            other => ReadFailure::Unavailable(other.to_string()),
        })?;
    let Some(observed) = observed else {
        return Ok(None);
    };
    serde_json::from_slice(&observed.bytes)
        .map(Some)
        .map_err(|_| ReadFailure::Corrupt)
}

fn lookup_unavailable_or_absent(error: ExactLookupError) -> OriginalRead {
    match error {
        ExactLookupError::Io { source, .. } if source.kind() == io::ErrorKind::NotFound => {
            OriginalRead::Absent
        }
        other => OriginalRead::Unavailable {
            detail: other.to_string(),
        },
    }
}

fn unavailable(error: impl Error) -> OriginalRead {
    OriginalRead::Unavailable {
        detail: error.to_string(),
    }
}

fn segment_error_is_corrupt(error: &solstone_core_segment::SegmentError) -> bool {
    matches!(
        error,
        solstone_core_segment::SegmentError::StreamInput(_)
            | solstone_core_segment::SegmentError::MalformedStreamRecord { .. }
            | solstone_core_segment::SegmentError::Read(
                solstone_core_journal_io::ReadError::Malformed(_)
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_coordinate_is_corrupt() {
        let source = SourceKey::from_verified_id("test-source");
        let coordinate = Coordinate {
            day: "20260102".into(),
            stream: "agent-memory-wrong".into(),
            segment: "030405_1".into(),
        };
        assert_eq!(
            read_original(Path::new("/var/tmp/not-opened"), &source, &coordinate),
            OriginalRead::Corrupt
        );
    }

    #[cfg(feature = "full-tests")]
    fn scratch_root() -> PathBuf {
        if cfg!(windows) {
            std::env::temp_dir()
        } else {
            PathBuf::from("/var/tmp")
        }
    }

    #[cfg(feature = "full-tests")]
    fn temporary_journal(label: &str) -> PathBuf {
        let root = scratch_root().join(format!("memory-original-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temporary journal");
        root
    }

    #[cfg(feature = "full-tests")]
    fn ready_fixture(root: &Path) -> (SourceKey, Coordinate, PathBuf) {
        use solstone_core_format::agent_memory::{OperationRecord, digest, ready_document};

        let source = SourceKey::from_verified_id("test-source");
        let coordinate = Coordinate {
            day: "20260102".into(),
            stream: format!("agent-memory-{}", source.component()),
            segment: "030405_1".into(),
        };
        let segment = root
            .join("chronicle")
            .join(&coordinate.day)
            .join(&coordinate.stream)
            .join(&coordinate.segment);
        std::fs::create_dir_all(&segment).expect("create segment");
        let bytes = b"trusted note";
        let record: OperationRecord = serde_json::from_value(serde_json::json!({
            "operation_id": "operation-1",
            "digest": digest(bytes),
            "byte_count": bytes.len(),
            "created_at": "2026-01-02T03:04:05Z",
            "origin_kind": "agent_memory",
            "creation_label": "connection label",
            "coordinate": coordinate,
            "phase": "chained",
            "chain": {"prev_day": null, "prev_segment": null, "seq": 1}
        }))
        .expect("valid operation record");
        let ready = ready_document(&record, &source).unwrap();
        std::fs::write(segment.join("note.txt"), bytes).unwrap();
        std::fs::write(
            segment.join("origin.json"),
            serde_json::to_vec(&ready.origin).unwrap(),
        )
        .unwrap();
        std::fs::write(
            segment.join("ready.json"),
            serde_json::to_vec(&ready).unwrap(),
        )
        .unwrap();
        std::fs::write(
            segment.join("stream.json"),
            serde_json::json!({
                "stream": &coordinate.stream,
                "prev_day": null,
                "prev_segment": null,
                "seq": 1
            })
            .to_string(),
        )
        .unwrap();
        (source, coordinate, segment)
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn absent_parent_is_not_created() {
        let parent = scratch_root().join(format!("memory-original-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        let source = SourceKey::from_verified_id("test-source");
        let coordinate = Coordinate {
            day: "20260102".into(),
            stream: format!("agent-memory-{}", source.component()),
            segment: "030405_1".into(),
        };
        assert_eq!(
            read_original(&parent, &source, &coordinate),
            OriginalRead::Absent
        );
        assert!(!parent.exists());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn tombstone_and_staged_sibling_are_reported_without_mutation() {
        let tombstone_root = temporary_journal("tombstone");
        let (source, coordinate, segment) = ready_fixture(&tombstone_root);
        std::fs::write(segment.join("tombstone.json"), b"{}").unwrap();
        assert_eq!(
            read_original(&tombstone_root, &source, &coordinate),
            OriginalRead::Deleted
        );
        assert!(segment.join("tombstone.json").exists());
        let _ = std::fs::remove_dir_all(tombstone_root);

        let staged_root = temporary_journal("staged");
        let (source, coordinate, segment) = ready_fixture(&staged_root);
        let staged = segment.with_file_name(format!(".removing_{}", coordinate.segment));
        std::fs::rename(&segment, &staged).unwrap();
        assert_eq!(
            read_original(&staged_root, &source, &coordinate),
            OriginalRead::Staged
        );
        assert!(staged.exists());
        assert!(!segment.exists());
        let _ = std::fs::remove_dir_all(staged_root);
    }

    #[cfg(all(feature = "full-tests", unix))]
    #[test]
    fn symlink_refusal_leaves_names_unchanged() {
        use std::os::unix::fs::symlink;

        let root = temporary_journal("symlink");
        let (source, coordinate, segment) = ready_fixture(&root);
        let outside = root.join("outside-ready.json");
        std::fs::write(&outside, b"untouched").unwrap();
        std::fs::remove_file(segment.join("ready.json")).unwrap();
        symlink(&outside, segment.join("ready.json")).unwrap();
        assert!(matches!(
            read_original(&root, &source, &coordinate),
            OriginalRead::Unavailable { .. }
        ));
        assert!(
            std::fs::symlink_metadata(segment.join("ready.json"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(outside).unwrap(), b"untouched");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(all(feature = "full-tests", windows))]
    #[test]
    fn windows_path_proof_refuses_symlinked_ready_marker() {
        use std::os::windows::fs::symlink_file;

        let root = temporary_journal("windows-path-proof");
        let (source, coordinate, segment) = ready_fixture(&root);
        let outside = root.join("outside-ready.json");
        std::fs::write(&outside, b"untouched").unwrap();
        std::fs::remove_file(segment.join("ready.json")).unwrap();
        symlink_file(&outside, segment.join("ready.json")).unwrap();
        assert!(matches!(
            read_original(&root, &source, &coordinate),
            OriginalRead::Unavailable { .. }
        ));
        assert_eq!(std::fs::read(outside).unwrap(), b"untouched");
        let _ = std::fs::remove_dir_all(root);
    }
}
