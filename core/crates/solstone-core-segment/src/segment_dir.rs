// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use solstone_core_journal_io::{
    DEFAULT_STREAM, PathError, PathEscapeError, PathOrDay, Segment, contained_path, day_dirs,
    day_path, iter_segments, iter_stream_segments,
};

use crate::SegmentError;

const TOMBSTONE_NAME: &str = "tombstone.json";
const STAGED_PREFIX: &str = ".removing_";

/// Non-mutating state of an owner-deleted segment path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerDeletionState {
    Live,
    Deleted,
    Staged,
    Occupied,
}

/// A resolved journal segment directory with no creation side effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentDir {
    pub(crate) journal: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) day: String,
    pub(crate) segment: String,
    pub(crate) stream: String,
}

impl SegmentDir {
    /// Resolve the Python-compatible on-disk location for a segment.
    pub fn resolve(
        journal: &Path,
        day: &str,
        segment: &str,
        stream: &str,
    ) -> Result<Self, SegmentError> {
        validate_component(segment, "segment")?;
        validate_component(stream, "stream")?;
        let _ = day_path(journal, day, false)?;
        let rel = if stream == DEFAULT_STREAM {
            format!("chronicle/{day}/{segment}")
        } else {
            format!("chronicle/{day}/{stream}/{segment}")
        };
        let chronicle = contained_path(journal, "chronicle")?;
        let path = contained_path(journal, &rel)?;
        if !path.starts_with(&chronicle) {
            return Err(SegmentError::Path(PathError::Escape(PathEscapeError {
                path,
                rel,
            })));
        }
        Ok(Self {
            journal: journal.to_path_buf(),
            path,
            day: day.to_owned(),
            segment: segment.to_owned(),
            stream: stream.to_owned(),
        })
    }

    /// Return the contained path resolved by this segment handle.
    ///
    /// Delete-owning crates may use this after resolving a `(day, stream,
    /// segment)` name triple; callers must not substitute walked directory paths.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Whether the owner has deleted the segment at `segment_dir`.
///
/// True when that path holds `tombstone.json`, its parent holds the staged
/// removal name, or either marker name is occupied by an unrecognized path.
/// The staged name is `STAGED_PREFIX` (`.removing_`) plus the segment's final
/// component. This function does not take the removal lock.
///
/// A tombstone or staged removal means deleted. An occupied marker path that
/// is not a recognized tombstone or staged directory still occupies the key.
///
/// A key that any marker of its stream names as `prev` stays deleted. Rewriting
/// that key at the head would give it a second chain position. Links are by
/// key, so the chain would loop. `advance_unbound_stream` already forbids
/// giving a segment a second position. A later release may treat a tombstone as
/// claimable only for the same `(activity_id, index)` it released. Both the
/// writer and any later key probe go through that identity rule. A key a
/// successor still names stays deleted, and the writer steps to the next key.
/// This function does not implement that release. A later release has to honour
/// this identity rule.
///
/// A caller that probes keys treats `Err` as stop, never as a free key.
pub fn owner_deleted(segment_dir: &Path) -> Result<bool, SegmentError> {
    Ok(owner_deletion_state(segment_dir)? != OwnerDeletionState::Live)
}

/// Distinguish a live segment, a tombstoned segment, a staged removal, and an
/// occupied marker without acquiring the retention lock or creating state.
pub fn owner_deletion_state(segment_dir: &Path) -> Result<OwnerDeletionState, SegmentError> {
    let parent = segment_dir.parent().ok_or(SegmentError::StreamInput(
        "segment directory must have a parent",
    ))?;
    let file_name = segment_dir.file_name().ok_or(SegmentError::StreamInput(
        "segment directory must have a final component",
    ))?;

    match std::fs::symlink_metadata(segment_dir) {
        Ok(metadata) => {
            if metadata.is_dir() {
                let tombstone = segment_dir.join(TOMBSTONE_NAME);
                match std::fs::symlink_metadata(&tombstone) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Ok(OwnerDeletionState::Occupied);
                    }
                    Ok(_) => return Ok(OwnerDeletionState::Deleted),
                    Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(SegmentError::Io {
                            path: tombstone,
                            source,
                        });
                    }
                }
            }
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(SegmentError::Io {
                path: segment_dir.to_path_buf(),
                source,
            });
        }
    }

    let mut staged_name = std::ffi::OsString::from(STAGED_PREFIX);
    staged_name.push(file_name);
    let staged_path = parent.join(staged_name);
    match std::fs::symlink_metadata(&staged_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(OwnerDeletionState::Occupied),
        Ok(metadata) if metadata.is_dir() => Ok(OwnerDeletionState::Staged),
        Ok(_) => Ok(OwnerDeletionState::Occupied),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(OwnerDeletionState::Live)
        }
        Err(source) => Err(SegmentError::Io {
            path: staged_path,
            source,
        }),
    }
}

fn validate_component(value: &str, kind: &'static str) -> Result<(), SegmentError> {
    if !is_safe_stream_component(value) {
        return Err(SegmentError::StreamInput(match kind {
            "segment" => "segment must be a plain path component",
            _ => "stream must be a plain path component",
        }));
    }
    Ok(())
}

/// True when a stream name is safe to use as one journal path component.
pub fn is_safe_stream_component(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('/')
        && !value.contains('\\')
        && !matches!(value, "." | "..")
        && !value.starts_with('.')
        && !value.chars().any(|ch| ch.is_ascii_uppercase())
}

/// Every `YYYYMMDD` chronicle day directory present in the journal, sorted.
///
/// Segment enumeration belongs to the crate that owns segments. A caller that
/// merely lists is otherwise pushed into depending on `journal-io` directly,
/// which routes it around the single write door *and* around the reviewed
/// write-owner allowlist that keeps that door narrow. Listing is a read, so
/// nothing here writes -- but the dependency edge is the thing being kept
/// narrow, not the operation.
pub fn list_days(journal: &Path) -> Result<Vec<(String, PathBuf)>, SegmentError> {
    let mut days: Vec<(String, PathBuf)> = day_dirs(journal)?.into_iter().collect();
    days.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(days)
}

/// Discovered segments under one chronicle day.
///
/// Each row is the exact on-disk location (`StreamLocation` plus basename),
/// not a `(stream, key)` pair. Ask `record_identity()` when a UTF-8 spelling
/// is required.
pub fn list_segments(journal: &Path, day: &str) -> Result<Vec<Segment>, SegmentError> {
    Ok(iter_segments(journal, PathOrDay::Day(day))?)
}

/// Discovered segments of one stream under one chronicle day, reading no
/// other stream's directory.
pub fn list_stream_segments(
    journal: &Path,
    day: &str,
    stream: &str,
) -> Result<Vec<Segment>, SegmentError> {
    Ok(iter_stream_segments(journal, day, stream)?)
}

/// Discovered segments under an already-resolved day directory.
pub fn list_segments_in(journal: &Path, day_dir: &Path) -> Result<Vec<Segment>, SegmentError> {
    Ok(iter_segments(journal, PathOrDay::Directory(day_dir))?)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[cfg(unix)]
    use solstone_core_journal_io::PathError;

    use crate::test_support::TempDir;

    use super::*;

    #[test]
    fn resolves_default_stream_directly_under_day() {
        let temporary = TempDir::new();
        let root = temporary.path();
        fs::create_dir_all(root.join("chronicle/20260804")).unwrap();
        let resolved = SegmentDir::resolve(root, "20260804", "120000_60", DEFAULT_STREAM).unwrap();
        assert_eq!(resolved.path, root.join("chronicle/20260804/120000_60"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_named_stream_symlink_escape() {
        let temporary = TempDir::new();
        let root = temporary.path();
        let day_dir = root.join("chronicle/20260804");
        let outside = root.join("outside");
        fs::create_dir_all(&day_dir).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&outside, day_dir.join("workstation")).unwrap();

        assert!(matches!(
            SegmentDir::resolve(root, "20260804", "120000_60", "workstation"),
            Err(SegmentError::Path(PathError::Escape(_)))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_day_symlink_escape() {
        let temporary = TempDir::new();
        let root = temporary.path();
        let chronicle = root.join("chronicle");
        let outside = root.join("outside");
        fs::create_dir(&chronicle).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&outside, chronicle.join("20260804")).unwrap();

        assert!(matches!(
            SegmentDir::resolve(root, "20260804", "120000_60", DEFAULT_STREAM),
            Err(SegmentError::Path(PathError::Escape(_)))
        ));
    }

    #[test]
    fn owner_deleted_predicate_states() {
        let temporary = TempDir::new();
        let root = temporary.path();

        // 1. Tombstone directory -> true
        let tombstoned_dir = root.join("tombstoned_seg");
        fs::create_dir_all(&tombstoned_dir).unwrap();
        fs::write(tombstoned_dir.join("tombstone.json"), b"{}").unwrap();
        assert!(owner_deleted(&tombstoned_dir).unwrap());

        // 2. Parent holds .removing_<key> and <key> is absent -> true
        let parent = root.join("day_dir");
        fs::create_dir_all(&parent).unwrap();
        let removing_seg = parent.join("staged_key");
        fs::create_dir_all(parent.join(".removing_staged_key")).unwrap();
        assert!(owner_deleted(&removing_seg).unwrap());

        // 3. Live directory -> false
        let live_dir = root.join("live_seg");
        fs::create_dir_all(&live_dir).unwrap();
        assert!(!owner_deleted(&live_dir).unwrap());

        // 4. Nothing at the path -> false
        let missing_path = root.join("missing_seg");
        assert!(!owner_deleted(&missing_path).unwrap());

        // 5. A path whose parent is a file -> Err
        let file_parent = root.join("parent_file");
        fs::write(&file_parent, b"not a dir").unwrap();
        let bad_child = file_parent.join("child");
        assert!(owner_deleted(&bad_child).is_err());

        // 6. A non-directory staged marker still occupies the key.
        let occupied_seg = parent.join("occupied_key");
        fs::write(parent.join(".removing_occupied_key"), b"occupied").unwrap();
        assert!(owner_deleted(&occupied_seg).unwrap());
        assert_eq!(
            owner_deletion_state(&occupied_seg).unwrap(),
            OwnerDeletionState::Occupied
        );
    }

    #[cfg(all(test, feature = "full-tests", unix))]
    #[test]
    fn tombstone_symlink_occupies_key_without_mutation() {
        use std::os::unix::fs::MetadataExt;

        let temporary = TempDir::new();
        let segment = temporary.path().join("segment");
        fs::create_dir_all(&segment).unwrap();
        let target = temporary.path().join("target");
        fs::write(&target, b"target").unwrap();
        let tombstone = segment.join("tombstone.json");
        symlink(&target, &tombstone).unwrap();
        let inode = fs::symlink_metadata(&tombstone).unwrap().ino();

        assert!(owner_deleted(&segment).unwrap());
        assert_eq!(
            owner_deletion_state(&segment).unwrap(),
            OwnerDeletionState::Occupied
        );
        let metadata = fs::symlink_metadata(&tombstone).unwrap();
        assert!(metadata.file_type().is_symlink());
        assert_eq!(metadata.ino(), inode);
    }
}
