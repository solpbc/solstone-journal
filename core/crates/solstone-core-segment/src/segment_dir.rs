// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use solstone_core_journal_io::media::{MediaDirectory, MediaEntry};

use solstone_core_journal_io::{
    DEFAULT_STREAM, JournalEntryKind, PathError, PathEscapeError, PathOrDay, Segment,
    contained_path, day_dirs, day_path, iter_segments, iter_stream_segments,
};

use crate::SegmentError;

const TOMBSTONE_NAME: &str = "tombstone.json";
pub(crate) const STAGED_PREFIX: &str = ".removing_";

/// Non-mutating state of an owner-deleted segment path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerDeletionState {
    Live,
    Deleted,
    Staged,
    Occupied,
}

fn classify_owner_markers(
    tombstone: Option<JournalEntryKind>,
    staged: Option<JournalEntryKind>,
) -> OwnerDeletionState {
    match (tombstone, staged) {
        (Some(JournalEntryKind::Symlink), _) => OwnerDeletionState::Occupied,
        (Some(_), _) => OwnerDeletionState::Deleted,
        (None, Some(JournalEntryKind::Directory)) => OwnerDeletionState::Staged,
        (None, Some(_)) => OwnerDeletionState::Occupied,
        (None, None) => OwnerDeletionState::Live,
    }
}

/// Owner-deletion observations through an existing retained parent and child.
/// This does not acquire claims, exclude writers, or authorize publication.
pub struct RetainedOwnerDeletion {
    parent: Arc<MediaDirectory>,
    basename: OsString,
    staged_name: OsString,
    directory: Option<Arc<MediaDirectory>>,
    maximum_entries: usize,
}

impl RetainedOwnerDeletion {
    /// Admit the existing child through its actual parent. A supplied live
    /// directory must match the full identity of the parent-derived child.
    /// Without one, only initial child-open absence is admitted as absence.
    pub fn open(
        parent: Arc<MediaDirectory>,
        basename: &OsStr,
        expected_live: Option<&MediaDirectory>,
        maximum_entries: usize,
    ) -> Result<Self, SegmentError> {
        validate_component(
            basename.to_str().ok_or(SegmentError::StreamInput(
                "segment must be a UTF-8 path component",
            ))?,
            "segment",
        )?;
        let child_path = parent.diagnostic_path().join(basename);
        parent.revalidate().map_err(|source| SegmentError::Io {
            path: parent.diagnostic_path().to_path_buf(),
            source,
        })?;
        if let Some(expected) = expected_live {
            expected.revalidate().map_err(|source| SegmentError::Io {
                path: expected.diagnostic_path().to_path_buf(),
                source,
            })?;
        }
        let directory = parent
            .open_child(basename)
            .map_err(|source| SegmentError::Io {
                path: child_path.clone(),
                source,
            })?;
        if let Some(expected) = expected_live {
            if directory.as_ref().map(|child| child.identity()) != Some(expected.identity()) {
                return Err(SegmentError::Io {
                    path: child_path,
                    source: io::Error::other("retained segment identity does not match"),
                });
            }
            expected.revalidate().map_err(|source| SegmentError::Io {
                path: expected.diagnostic_path().to_path_buf(),
                source,
            })?;
        }
        let mut staged_name = OsString::from(STAGED_PREFIX);
        staged_name.push(basename);
        let admitted = Self {
            parent,
            basename: basename.to_os_string(),
            staged_name,
            directory,
            maximum_entries,
        };
        admitted.revalidate()?;
        Ok(admitted)
    }

    /// The actual parent-derived child, if initially present. Never creates one.
    pub fn directory(&self) -> Option<&Arc<MediaDirectory>> {
        self.directory.as_ref()
    }

    /// Read fresh marker state with complete bounded observations. Any later
    /// identity, chain, coverage or relevant entry change is an error.
    pub fn state(&self) -> Result<OwnerDeletionState, SegmentError> {
        self.revalidate()?;
        let before = self.parent_markers()?;
        match (&self.directory, &before.0) {
            (Some(child), Some(entry))
                if entry.kind == JournalEntryKind::Directory
                    && entry.identity == child.identity() => {}
            (None, None) => {}
            _ => return Err(self.changed()),
        }
        let tombstone = match &self.directory {
            Some(child) => self
                .entries(child)?
                .into_iter()
                .find(|entry| entry.name == TOMBSTONE_NAME)
                .map(|entry| entry.kind),
            None => None,
        };
        // Retain MediaEntry equality, including its private native stamp.
        if self.parent_markers()? != before {
            return Err(self.changed());
        }
        self.revalidate()?;
        Ok(classify_owner_markers(
            tombstone,
            before.1.map(|entry| entry.kind),
        ))
    }

    fn parent_markers(&self) -> Result<(Option<MediaEntry>, Option<MediaEntry>), SegmentError> {
        let mut segment = None;
        let mut staged = None;
        for entry in self.entries(&self.parent)? {
            if entry.name == self.basename {
                segment = Some(entry);
            } else if entry.name == self.staged_name {
                staged = Some(entry);
            }
        }
        Ok((segment, staged))
    }

    fn entries(&self, directory: &MediaDirectory) -> Result<Vec<MediaEntry>, SegmentError> {
        directory
            .entries(self.maximum_entries)
            .and_then(|entries| {
                entries.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "owner-deletion entry limit exceeded",
                    )
                })
            })
            .map_err(|source| SegmentError::Io {
                path: directory.diagnostic_path().to_path_buf(),
                source,
            })
    }

    fn revalidate(&self) -> Result<(), SegmentError> {
        if let Some(child) = &self.directory {
            child.revalidate().map_err(|source| SegmentError::Io {
                path: child.diagnostic_path().to_path_buf(),
                source,
            })?;
        }
        self.parent.revalidate().map_err(|source| SegmentError::Io {
            path: self.parent.diagnostic_path().to_path_buf(),
            source,
        })
    }

    fn changed(&self) -> SegmentError {
        SegmentError::Io {
            path: self.parent.diagnostic_path().join(&self.basename),
            source: io::Error::other("retained owner-deletion observation changed"),
        }
    }
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
/// A release tombstone is never written into and never claimed, by anyone. The
/// Strava probe steps past it to the next candidate. It records no owner intent
/// to keep the tile away. The probe stops at any other tombstone or staged removal.
/// Reading the reason is fail-closed: a `tombstone.json` that cannot be read fails
/// the run; one that does not parse, has no reason, or names any reason but the
/// exact release string stops the probe; only an exact match steps past.
/// Writers that place by a fixed key (chat, calendar, notes) do not probe, so a
/// release tombstone keeps their segment deleted. A general delete that treats an
/// import as a set needs its own rule. This contract does not claim one. This
/// function does not implement that step-past.
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
                        return Ok(classify_owner_markers(
                            Some(JournalEntryKind::Symlink),
                            None,
                        ));
                    }
                    Ok(_) => {
                        return Ok(classify_owner_markers(Some(JournalEntryKind::Other), None));
                    }
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
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            check_missing_segment_ancestors(segment_dir).map_err(|source| SegmentError::Io {
                path: segment_dir.to_path_buf(),
                source,
            })?;
        }
        Err(source) => {
            return Err(SegmentError::Io {
                path: segment_dir.to_path_buf(),
                source,
            });
        }
    }

    let mut staged_name = OsString::from(STAGED_PREFIX);
    staged_name.push(file_name);
    let staged_path = parent.join(staged_name);
    match std::fs::symlink_metadata(&staged_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(classify_owner_markers(
            None,
            Some(JournalEntryKind::Symlink),
        )),
        Ok(metadata) if metadata.is_dir() => Ok(classify_owner_markers(
            None,
            Some(JournalEntryKind::Directory),
        )),
        Ok(_) => Ok(classify_owner_markers(None, Some(JournalEntryKind::Other))),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(classify_owner_markers(None, None))
        }
        Err(source) => Err(SegmentError::Io {
            path: staged_path,
            source,
        }),
    }
}

// Windows can report a missing path when an ancestor is a file. This only
// classifies absence; it does not admit a path or exclude namespace changes.
fn check_missing_segment_ancestors(segment_dir: &Path) -> io::Result<()> {
    for parent in segment_dir.ancestors().skip(1) {
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        match std::fs::metadata(parent) {
            Ok(metadata) if metadata.is_dir() => return Ok(()),
            Ok(_) => return Err(io::ErrorKind::NotADirectory.into()),
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(source),
        }
    }
    Err(io::ErrorKind::Other.into())
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
    fn owner_marker_classification() {
        use JournalEntryKind::{Directory, Other, RegularFile, Symlink};
        use OwnerDeletionState::{Deleted, Live, Occupied, Staged};

        let cases = [
            (None, None, Live),
            (None, Some(Directory), Staged),
            (None, Some(RegularFile), Occupied),
            (None, Some(Symlink), Occupied),
            (Some(RegularFile), None, Deleted),
            (Some(Directory), Some(Symlink), Deleted),
            (Some(Other), Some(Directory), Deleted),
            (Some(Symlink), Some(Directory), Occupied),
        ];
        for (tombstone, staged, expected) in cases {
            assert_eq!(classify_owner_markers(tombstone, staged), expected);
        }
    }

    #[test]
    fn resolves_default_stream_directly_under_day() {
        let temporary = TempDir::new();
        let root = temporary.path();
        let day = root.join("chronicle").join("20260804");
        fs::create_dir_all(&day).unwrap();
        let canonical_day = fs::canonicalize(&day).unwrap();
        let resolved = SegmentDir::resolve(root, "20260804", "120000_60", DEFAULT_STREAM).unwrap();
        assert_eq!(resolved.path, canonical_day.join("120000_60"));
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
        let assert_obstruction = |requested: &Path| match owner_deleted(requested).unwrap_err() {
            SegmentError::Io { path, source } => {
                assert_eq!(path, requested);
                assert_eq!(source.kind(), io::ErrorKind::NotADirectory);
            }
            other => panic!("unexpected obstruction error: {other:?}"),
        };
        assert_obstruction(&bad_child);
        assert_obstruction(&file_parent.join("missing_parent").join("child"));
        assert!(!owner_deleted(&root.join("missing_parent").join("child")).unwrap());

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

    #[cfg(feature = "full-tests")]
    mod retained_owner_deletion {
        use solstone_core_journal_io::JournalRoot;
        use solstone_core_journal_io::media::{MediaPrimitive, run_with_media_hook};

        use super::*;

        struct Fixture {
            temporary: TempDir,
            parent: Arc<MediaDirectory>,
            parent_path: PathBuf,
            segment: PathBuf,
        }

        impl Fixture {
            fn new(present: bool) -> Self {
                let temporary = TempDir::new();
                let journal = temporary.path().join("journal");
                let parent_path = journal.join("chronicle/20261010");
                fs::create_dir_all(&parent_path).unwrap();
                let segment = parent_path.join("120000_60");
                if present {
                    fs::create_dir(&segment).unwrap();
                }
                let root = Arc::new(JournalRoot::open(&journal).unwrap());
                let parent = MediaDirectory::open(root, Path::new("chronicle/20261010")).unwrap();
                Self {
                    temporary,
                    parent,
                    parent_path,
                    segment,
                }
            }

            fn open(&self, maximum: usize) -> RetainedOwnerDeletion {
                RetainedOwnerDeletion::open(
                    self.parent.clone(),
                    OsStr::new("120000_60"),
                    None,
                    maximum,
                )
                .unwrap()
            }

            fn staged(&self) -> PathBuf {
                self.parent_path.join(".removing_120000_60")
            }
        }

        #[test]
        fn live_absent_markers_and_fresh_state() {
            for present in [false, true] {
                let fixture = Fixture::new(present);
                let observed = fixture.open(16);
                assert_eq!(observed.directory().is_some(), present);
                assert_eq!(observed.state().unwrap(), OwnerDeletionState::Live);
                fs::create_dir(fixture.staged()).unwrap();
                assert_eq!(observed.state().unwrap(), OwnerDeletionState::Staged);
                fs::remove_dir(fixture.staged()).unwrap();
                fs::write(fixture.staged(), b"occupied").unwrap();
                assert_eq!(observed.state().unwrap(), OwnerDeletionState::Occupied);
                if present {
                    fs::write(fixture.segment.join(TOMBSTONE_NAME), b"malformed JSON").unwrap();
                    assert_eq!(observed.state().unwrap(), OwnerDeletionState::Deleted);
                    fs::remove_file(fixture.segment.join(TOMBSTONE_NAME)).unwrap();
                    fs::create_dir(fixture.segment.join(TOMBSTONE_NAME)).unwrap();
                    assert_eq!(observed.state().unwrap(), OwnerDeletionState::Deleted);
                }
            }
        }

        #[test]
        fn supplied_child_identity_and_missing_refusal() {
            let fixture = Fixture::new(true);
            let live = fixture
                .parent
                .open_child(OsStr::new("120000_60"))
                .unwrap()
                .unwrap();
            let observed = RetainedOwnerDeletion::open(
                fixture.parent.clone(),
                OsStr::new("120000_60"),
                Some(&live),
                16,
            )
            .unwrap();
            assert_eq!(observed.directory().unwrap().identity(), live.identity());
            assert_eq!(observed.state().unwrap(), OwnerDeletionState::Live);
            fs::create_dir(fixture.parent_path.join("other")).unwrap();
            let other = fixture
                .parent
                .open_child(OsStr::new("other"))
                .unwrap()
                .unwrap();
            assert!(
                RetainedOwnerDeletion::open(
                    fixture.parent.clone(),
                    OsStr::new("120000_60"),
                    Some(&other),
                    16
                )
                .is_err()
            );
            assert!(
                RetainedOwnerDeletion::open(
                    fixture.parent.clone(),
                    OsStr::new("missing"),
                    Some(&live),
                    16
                )
                .is_err()
            );
        }

        #[test]
        fn coverage_and_name_refusals() {
            let fixture = Fixture::new(true);
            assert_eq!(fixture.open(1).state().unwrap(), OwnerDeletionState::Live);
            assert!(fixture.open(0).state().is_err());
            fs::write(fixture.segment.join("one"), b"one").unwrap();
            assert_eq!(fixture.open(1).state().unwrap(), OwnerDeletionState::Live);
            fs::write(fixture.segment.join("two"), b"two").unwrap();
            assert!(fixture.open(1).state().is_err());
            fs::write(fixture.parent_path.join("extra"), b"extra").unwrap();
            assert!(fixture.open(1).state().is_err());
            for name in ["", ".", "..", ".hidden", "Upper", "a/b", "a\\b"] {
                assert!(
                    RetainedOwnerDeletion::open(fixture.parent.clone(), OsStr::new(name), None, 16)
                        .is_err()
                );
            }
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                assert!(
                    RetainedOwnerDeletion::open(
                        fixture.parent.clone(),
                        OsStr::from_bytes(b"\xff"),
                        None,
                        16
                    )
                    .is_err()
                );
            }
        }

        #[test]
        fn initial_absence_then_appearance_and_late_loss_refuse() {
            let fixture = Fixture::new(false);
            let observed = fixture.open(16);
            fs::create_dir(&fixture.segment).unwrap();
            assert!(observed.state().is_err());
            let observed = fixture.open(16);
            #[cfg(windows)]
            {
                // Windows refuses removal while the retained child is open.
                assert!(fs::remove_dir(&fixture.segment).is_err());
                assert_eq!(observed.state().unwrap(), OwnerDeletionState::Live);
                drop(observed);
                fs::remove_dir(&fixture.segment).unwrap();
            }
            #[cfg(not(windows))]
            {
                fs::remove_dir(&fixture.segment).unwrap();
                assert!(observed.state().is_err());
                drop(observed);
            }
            fs::write(&fixture.segment, b"not directory").unwrap();
            assert!(
                RetainedOwnerDeletion::open(
                    fixture.parent.clone(),
                    OsStr::new("120000_60"),
                    None,
                    16
                )
                .is_err()
            );
            assert_eq!(
                owner_deletion_state(&fixture.segment).unwrap(),
                OwnerDeletionState::Live
            );
        }

        #[cfg(unix)]
        #[test]
        fn symlinks_are_occupied_or_refused_without_touching_targets() {
            let fixture = Fixture::new(true);
            let target = fixture.temporary.path().join("target");
            fs::write(&target, b"untouched").unwrap();
            let observed = fixture.open(16);
            symlink(&target, fixture.segment.join(TOMBSTONE_NAME)).unwrap();
            assert_eq!(observed.state().unwrap(), OwnerDeletionState::Occupied);
            fs::remove_file(fixture.segment.join(TOMBSTONE_NAME)).unwrap();
            symlink(&target, fixture.staged()).unwrap();
            assert_eq!(observed.state().unwrap(), OwnerDeletionState::Occupied);
            fs::remove_file(fixture.staged()).unwrap();
            fs::remove_dir(&fixture.segment).unwrap();
            symlink(&target, &fixture.segment).unwrap();
            assert!(
                RetainedOwnerDeletion::open(
                    fixture.parent.clone(),
                    OsStr::new("120000_60"),
                    None,
                    16
                )
                .is_err()
            );
            assert_eq!(
                owner_deletion_state(&fixture.segment).unwrap(),
                OwnerDeletionState::Live
            );
            assert!(
                fs::symlink_metadata(&fixture.segment)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(fs::read(&target).unwrap(), b"untouched");
        }

        #[cfg(unix)]
        #[test]
        fn root_ancestor_parent_and_segment_rebinding_refuse() {
            for relative in [
                "journal",
                "journal/chronicle",
                "journal/chronicle/20261010",
                "journal/chronicle/20261010/120000_60",
            ] {
                let fixture = Fixture::new(true);
                fs::write(fixture.segment.join("original"), b"original").unwrap();
                let observed = fixture.open(16);
                let cut = fixture.temporary.path().join(relative);
                let parked = fixture.temporary.path().join("parked");
                let suffix = fixture.segment.strip_prefix(&cut).unwrap();
                let original = parked.join(suffix).join("original");
                fs::rename(&cut, &parked).unwrap();
                fs::create_dir_all(&fixture.segment).unwrap();
                fs::write(fixture.segment.join("replacement"), b"replacement").unwrap();
                assert!(observed.state().is_err(), "{relative}");
                assert_eq!(fs::read(original).unwrap(), b"original");
                assert_eq!(
                    fs::read(fixture.segment.join("replacement")).unwrap(),
                    b"replacement"
                );
            }
        }

        #[cfg(unix)]
        #[test]
        fn child_admission_substitution_is_not_initial_absence() {
            for primitive in [
                MediaPrimitive::AfterChildOpen,
                MediaPrimitive::AfterChildObservation,
            ] {
                let fixture = Fixture::new(true);
                let segment = fixture.segment.clone();
                let parked = fixture.parent_path.join("parked");
                let (result, fired) = run_with_media_hook(
                    primitive,
                    move || {
                        fs::rename(&segment, &parked)?;
                        fs::create_dir(&segment)
                    },
                    || {
                        RetainedOwnerDeletion::open(
                            fixture.parent.clone(),
                            OsStr::new("120000_60"),
                            None,
                            16,
                        )
                    },
                );
                assert!(fired, "{primitive:?}");
                assert!(result.is_err(), "{primitive:?}");
                assert!(fixture.parent_path.join("parked").is_dir());
                assert!(fixture.segment.is_dir());
            }
        }

        #[test]
        fn first_parent_guard_refuses_staged_insert_remove_replace() {
            for action in ["insert", "remove", "replace"] {
                let fixture = Fixture::new(true);
                let staged = fixture.staged();
                if action != "insert" {
                    fs::create_dir(&staged).unwrap();
                }
                let observed = fixture.open(16);
                let (result, fired) = run_with_media_hook(
                    MediaPrimitive::AfterFirstEnumeration,
                    move || match action {
                        "insert" => fs::create_dir(staged),
                        "remove" => fs::remove_dir(staged),
                        _ => {
                            fs::remove_dir(&staged)?;
                            fs::write(staged, b"replacement")
                        }
                    },
                    || observed.state(),
                );
                assert!(fired);
                assert!(result.is_err(), "{action}");
            }
        }

        #[test]
        fn observation_errors_preserve_their_kind() {
            for kind in [
                io::ErrorKind::PermissionDenied,
                io::ErrorKind::UnexpectedEof,
            ] {
                let fixture = Fixture::new(true);
                let observed = fixture.open(16);
                let (result, fired) = run_with_media_hook(
                    MediaPrimitive::AfterFirstEnumeration,
                    move || Err(io::Error::new(kind, "synthetic observation failure")),
                    || observed.state(),
                );
                assert!(fired);
                assert!(
                    matches!(result, Err(SegmentError::Io { source, .. }) if source.kind() == kind)
                );
            }
        }
    }
}

/// What a segment's place holds when the same segment arrives again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Arrival {
    /// Nothing is there: write it as new.
    Free,
    /// The owner removed it before and only its tombstone stood there. The
    /// tombstone is gone now and the folder is empty: write the arrival into it.
    Reclaimed,
    /// A live segment is there.
    Live,
    /// A removal of it is part-way through, or its place holds something a
    /// segment write must not take. Refuse the arrival for now.
    Removing,
}

/// Make room for a segment that arrives again where the owner removed it.
///
/// What arrives is new data and comes back: the owner's earlier removal is not
/// a reason to refuse it. A tombstone left by that removal stays only to tell
/// readers the place is removed, so it gives way here, under the same segment
/// lock a removal and the trash take. The folder itself stays, because a
/// stream's chain may name it. A removal still in progress, or a tombstone
/// beside other files, is left alone and refused.
pub fn reclaim_for_arrival(segment_dir: &Path) -> Result<Arrival, SegmentError> {
    match owner_deletion_state(segment_dir)? {
        OwnerDeletionState::Live => {
            return Ok(if path_present(segment_dir)? {
                Arrival::Live
            } else {
                Arrival::Free
            });
        }
        OwnerDeletionState::Staged | OwnerDeletionState::Occupied => return Ok(Arrival::Removing),
        OwnerDeletionState::Deleted => {}
    }
    let _lock = solstone_core_journal_io::hold_lock(
        segment_dir,
        solstone_core_journal_io::LockOptions::default(),
    )
    .map_err(|_| SegmentError::StreamInput("the segment is busy; try again"))?;
    if owner_deletion_state(segment_dir)? != OwnerDeletionState::Deleted {
        return reclaim_for_arrival_unlocked(segment_dir);
    }
    let entries = std::fs::read_dir(segment_dir)
        .map_err(|source| SegmentError::Io {
            path: segment_dir.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| SegmentError::Io {
            path: segment_dir.to_path_buf(),
            source,
        })?;
    if entries.len() != 1
        || entries
            .iter()
            .any(|entry| entry.file_name() != TOMBSTONE_NAME)
    {
        return Ok(Arrival::Removing);
    }
    let tombstone = segment_dir.join(TOMBSTONE_NAME);
    #[expect(
        clippy::disallowed_methods,
        reason = "system internal: a removal's tombstone giving way to the same segment arriving again; the tombstone is a marker, not owner data"
    )]
    std::fs::remove_file(&tombstone).map_err(|source| SegmentError::Io {
        path: tombstone,
        source,
    })?;
    if let Ok(dir) = std::fs::File::open(segment_dir) {
        let _ = dir.sync_all();
    }
    Ok(Arrival::Reclaimed)
}

fn reclaim_for_arrival_unlocked(segment_dir: &Path) -> Result<Arrival, SegmentError> {
    Ok(match owner_deletion_state(segment_dir)? {
        OwnerDeletionState::Live if path_present(segment_dir)? => Arrival::Live,
        OwnerDeletionState::Live => Arrival::Free,
        _ => Arrival::Removing,
    })
}

fn path_present(path: &Path) -> Result<bool, SegmentError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(SegmentError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}
