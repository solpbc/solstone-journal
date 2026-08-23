// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strict segment create and exact lookup against the journal root.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::errors::{PathError, PathEscapeError};
use crate::name_admission::{
    ConflictKind, NameAdmissionError, NameAdmissionReason, NameReuse, NoFollowEntryKind,
    StreamName, check_lookup_component, check_portable_component, classify_no_follow, escape_name,
    escape_path, scan_directory_conflicts,
};
use crate::paths::{DEFAULT_STREAM, SegmentLayout, day_path, realpath_non_strict};

const CHRONICLE_DIR: &str = "chronicle";

/// Failure while creating a strictly admitted segment directory.
#[derive(Debug)]
pub enum StrictCreateError {
    /// Admission or collision refused the create.
    Admission(NameAdmissionError),
    /// A directory could not be created after admission succeeded.
    CreateIo { path: PathBuf, source: io::Error },
}

impl StrictCreateError {
    /// Owner-facing invalid-name frame. Tests assert this string.
    #[must_use]
    pub fn invalid_template(name: &str, reason: NameAdmissionReason) -> String {
        format!(
            "Couldn't create '{}': {reason}. Choose a different name. No new journal item was created.",
            escape_name(name)
        )
    }

    /// Owner-facing collision frame. Tests assert this string.
    #[must_use]
    pub fn collision_template(name: &str, existing: &str) -> String {
        format!(
            "Couldn't create '{}' because it conflicts with '{}' when letter case is ignored. Choose a different name. No new journal item was created.",
            escape_name(name),
            escape_name(existing)
        )
    }
}

impl fmt::Display for StrictCreateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admission(NameAdmissionError::Invalid { candidate, reason }) => {
                formatter.write_str(&Self::invalid_template(candidate, *reason))
            }
            Self::Admission(NameAdmissionError::Collision {
                candidate,
                conflicts,
            }) => {
                let existing = conflicts
                    .first()
                    .map(|entry| entry.name.as_str())
                    .unwrap_or_default();
                formatter.write_str(&Self::collision_template(candidate, existing))
            }
            Self::Admission(error) => error.fmt(formatter),
            Self::CreateIo { path, source } => {
                write!(formatter, "{}: {source}", path.display())
            }
        }
    }
}

impl Error for StrictCreateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Admission(error) => Some(error),
            Self::CreateIo { source, .. } => Some(source),
        }
    }
}

/// Failure while resolving an existing stream or segment directory.
#[derive(Debug)]
pub enum ExactLookupError {
    /// A path component failed the lookup syntax check.
    InvalidComponent {
        /// Rejected component.
        candidate: String,
        /// Empty, DotComponent, RootOrPrefix, Separator (`/` only), or NUL Control.
        reason: NameAdmissionReason,
    },
    /// A filesystem operation failed.
    Io { path: PathBuf, source: io::Error },
    /// A path escaped the journal root.
    Containment(PathEscapeError),
    /// The requested slot exists but is not a directory.
    WrongKind {
        path: PathBuf,
        kind: NoFollowEntryKind,
    },
    /// Direct layout was requested with a stream other than `_default`.
    LayoutMismatch {
        /// Offending stream spelling.
        stream: String,
    },
}

impl fmt::Display for ExactLookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidComponent { candidate, reason } => {
                write!(
                    formatter,
                    "invalid path component '{}': {reason}",
                    escape_name(candidate)
                )
            }
            Self::Io { path, source } => write!(formatter, "{}: {source}", escape_path(path)),
            Self::Containment(error) => error.fmt(formatter),
            Self::WrongKind { path, kind } => write!(
                formatter,
                "{} is not a directory ({kind:?})",
                escape_path(path)
            ),
            Self::LayoutMismatch { stream } => write!(
                formatter,
                "direct layout requires stream {DEFAULT_STREAM:?}, got {stream:?}"
            ),
        }
    }
}

impl Error for ExactLookupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Containment(error) => Some(error),
            Self::InvalidComponent { .. }
            | Self::WrongKind { .. }
            | Self::LayoutMismatch { .. } => None,
        }
    }
}

struct AdmittedPaths {
    stream_dir: PathBuf,
    segment_dir: PathBuf,
    reuse_stream: bool,
}

/// Admit stream and segment names and scan for collisions without creating.
pub fn preflight_segment_admission(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Result<(), NameAdmissionError> {
    admit_segment(journal_root, day, stream, segment).map(|_| ())
}

/// Admit names, then create day/stream/segment directories as needed.
pub fn create_segment_strict(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Result<PathBuf, StrictCreateError> {
    let admitted =
        admit_segment(journal_root, day, stream, segment).map_err(StrictCreateError::Admission)?;
    day_path(journal_root, Some(day), true).map_err(|error| match error {
        PathError::Io { path, source } => StrictCreateError::CreateIo { path, source },
        other => StrictCreateError::Admission(from_path_error(other)),
    })?;
    if !admitted.reuse_stream {
        fs::create_dir_all(&admitted.stream_dir).map_err(|source| StrictCreateError::CreateIo {
            path: admitted.stream_dir.clone(),
            source,
        })?;
    }
    fs::create_dir_all(&admitted.segment_dir).map_err(|source| StrictCreateError::CreateIo {
        path: admitted.segment_dir.clone(),
        source,
    })?;
    Ok(admitted.segment_dir)
}

/// Resolve an existing named stream directory without following symlinks.
pub fn resolve_stream_exact(
    journal_root: &Path,
    day: &str,
    stream: &str,
) -> Result<Option<PathBuf>, ExactLookupError> {
    descend(journal_root, &[day, stream])
}

/// Resolve an existing named segment directory without following symlinks.
pub fn resolve_segment_exact(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Result<Option<PathBuf>, ExactLookupError> {
    descend(journal_root, &[day, stream, segment])
}

/// Resolve an existing segment directory for an explicit layout without following
/// symlinks.
pub fn resolve_segment_locator_exact(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    layout: SegmentLayout,
) -> Result<Option<PathBuf>, ExactLookupError> {
    match layout {
        SegmentLayout::Direct => {
            if stream != DEFAULT_STREAM {
                return Err(ExactLookupError::LayoutMismatch {
                    stream: stream.to_string(),
                });
            }
            descend(journal_root, &[day, segment])
        }
        SegmentLayout::Named => descend(journal_root, &[day, stream, segment]),
    }
}

fn admit_segment(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Result<AdmittedPaths, NameAdmissionError> {
    let day_dir = day_path(journal_root, Some(day), false).map_err(from_path_error)?;
    let stream = StreamName::parse(stream).map_err(|reason| NameAdmissionError::Invalid {
        candidate: stream.to_owned(),
        reason,
    })?;
    check_portable_component(segment).map_err(|reason| NameAdmissionError::Invalid {
        candidate: segment.to_owned(),
        reason,
    })?;
    ensure_contained(journal_root, &day_dir)?;
    let reuse_stream = match scan_directory_conflicts(&day_dir, stream.as_str())? {
        NameReuse::Create => false,
        NameReuse::Reuse => true,
    };
    let stream_dir = day_dir.join(stream.as_str());
    ensure_contained(journal_root, &stream_dir)?;
    if reuse_stream {
        scan_directory_conflicts(&stream_dir, segment)?;
    }
    let segment_dir = stream_dir.join(segment);
    ensure_contained(journal_root, &segment_dir)?;
    Ok(AdmittedPaths {
        stream_dir,
        segment_dir,
        reuse_stream,
    })
}

fn descend(journal_root: &Path, components: &[&str]) -> Result<Option<PathBuf>, ExactLookupError> {
    for component in components {
        check_lookup_component(component).map_err(|reason| ExactLookupError::InvalidComponent {
            candidate: (*component).to_owned(),
            reason,
        })?;
    }
    let mut current = journal_root.to_path_buf();
    let mut levels = Vec::with_capacity(components.len() + 1);
    levels.push(CHRONICLE_DIR);
    levels.extend(components.iter().copied());
    for component in levels {
        let child = current.join(component);
        let metadata = match fs::symlink_metadata(&child) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ExactLookupError::Io {
                    path: child,
                    source,
                });
            }
        };
        let kind = classify_no_follow(metadata.file_type());
        if kind == ConflictKind::Directory {
            ensure_contained_lookup(journal_root, &child)?;
            current = child;
            continue;
        }
        // A resolvable symlink whose target leaves the journal is an escape.
        // Dangling and contained symlinks, files, and FIFOs are WrongKind.
        // Genuine realpath I/O (ELOOP, EACCES, ...) must surface as Io, not
        // WrongKind. A dangling symlink lexists, so canonicalize of the link
        // itself fails with NotFound; that is still WrongKind.
        if kind == ConflictKind::Symlink {
            let root = realpath_non_strict(journal_root).map_err(lookup_from_path_error)?;
            match realpath_non_strict(&child) {
                Ok(resolved) if !resolved.starts_with(&root) => {
                    return Err(ExactLookupError::Containment(PathEscapeError {
                        path: resolved,
                        rel: child.strip_prefix(journal_root).map_or_else(
                            |_| child.display().to_string(),
                            |rel| rel.display().to_string(),
                        ),
                    }));
                }
                Ok(_) => {}
                Err(PathError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(lookup_from_path_error(error)),
            }
        }
        return Err(ExactLookupError::WrongKind {
            path: child,
            kind: kind.as_wrong_kind().unwrap_or(NoFollowEntryKind::Other),
        });
    }
    Ok(Some(current))
}

fn ensure_contained(journal_root: &Path, candidate: &Path) -> Result<(), NameAdmissionError> {
    let root = realpath_non_strict(journal_root).map_err(from_path_error)?;
    let resolved = realpath_non_strict(candidate).map_err(from_path_error)?;
    if resolved.starts_with(&root) {
        Ok(())
    } else {
        Err(NameAdmissionError::Containment(PathEscapeError {
            path: resolved,
            rel: candidate.strip_prefix(journal_root).map_or_else(
                |_| candidate.display().to_string(),
                |rel| rel.display().to_string(),
            ),
        }))
    }
}

fn ensure_contained_lookup(journal_root: &Path, candidate: &Path) -> Result<(), ExactLookupError> {
    let root = realpath_non_strict(journal_root).map_err(lookup_from_path_error)?;
    let resolved = realpath_non_strict(candidate).map_err(lookup_from_path_error)?;
    if resolved.starts_with(&root) {
        Ok(())
    } else {
        Err(ExactLookupError::Containment(PathEscapeError {
            path: resolved,
            rel: candidate.strip_prefix(journal_root).map_or_else(
                |_| candidate.display().to_string(),
                |rel| rel.display().to_string(),
            ),
        }))
    }
}

fn from_path_error(error: PathError) -> NameAdmissionError {
    match error {
        PathError::Escape(escape) => NameAdmissionError::Containment(escape),
        PathError::Io { path, source } => NameAdmissionError::Io { path, source },
        PathError::InvalidRelativePath { rel, .. } => NameAdmissionError::Invalid {
            candidate: rel,
            reason: NameAdmissionReason::RootOrPrefix,
        },
    }
}

fn lookup_from_path_error(error: PathError) -> ExactLookupError {
    match error {
        PathError::Escape(escape) => ExactLookupError::Containment(escape),
        PathError::Io { path, source } => ExactLookupError::Io { path, source },
        PathError::InvalidRelativePath { rel, .. } => ExactLookupError::InvalidComponent {
            candidate: rel,
            reason: NameAdmissionReason::RootOrPrefix,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ExactLookupError, StrictCreateError, create_segment_strict, preflight_segment_admission,
        resolve_segment_exact, resolve_segment_locator_exact, resolve_stream_exact,
    };
    use crate::errors::PathError;
    use crate::name_admission::{NameAdmissionError, NameAdmissionReason, NoFollowEntryKind};
    use crate::paths::{DEFAULT_STREAM, SegmentLayout, day_path, segment_path};
    use crate::test_support::TempDir;
    use std::fs;

    const DAY: &str = "20240103";
    const STREAM: &str = "import.apple_health";
    const SEGMENT: &str = "000000_300";

    #[test]
    fn preflight_ok_on_empty_journal() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        fs::create_dir(&journal).unwrap();
        preflight_segment_admission(&journal, DAY, STREAM, SEGMENT).unwrap();
    }

    #[test]
    fn create_segment_strict_creates_day_stream_segment() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        fs::create_dir(&journal).unwrap();
        let path = create_segment_strict(&journal, DAY, STREAM, SEGMENT).unwrap();
        assert_eq!(
            path,
            journal
                .join("chronicle")
                .join(DAY)
                .join(STREAM)
                .join(SEGMENT)
        );
        assert!(path.is_dir());
    }

    #[test]
    fn create_segment_strict_reuses_exact_stream_directory() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let stream = journal.join("chronicle").join(DAY).join(STREAM);
        fs::create_dir_all(&stream).unwrap();
        let marker = stream.join("kept");
        fs::write(&marker, b"keep").unwrap();
        create_segment_strict(&journal, DAY, STREAM, SEGMENT).unwrap();
        assert!(marker.is_file());
        assert!(stream.join(SEGMENT).is_dir());
    }

    #[test]
    fn create_segment_strict_aborts_case_variant_before_creating() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let day = journal.join("chronicle").join(DAY);
        fs::create_dir_all(day.join("Import.Apple_Health")).unwrap();
        let error = create_segment_strict(&journal, DAY, STREAM, SEGMENT).unwrap_err();
        match error {
            StrictCreateError::Admission(NameAdmissionError::Collision {
                ref candidate, ..
            }) => {
                assert_eq!(candidate, STREAM);
            }
            other => panic!("{other:?}"),
        }
        assert!(!day.join(STREAM).exists());
        assert_eq!(
            error.to_string(),
            StrictCreateError::collision_template(STREAM, "Import.Apple_Health")
        );
    }

    #[test]
    fn invalid_template_matches_contract() {
        assert_eq!(
            StrictCreateError::invalid_template("main", NameAdmissionReason::Empty),
            "Couldn't create 'main': the name is empty. Choose a different name. No new journal item was created."
        );
        assert_eq!(
            StrictCreateError::Admission(NameAdmissionError::Invalid {
                candidate: "a\nb".to_owned(),
                reason: NameAdmissionReason::Control,
            })
            .to_string(),
            "Couldn't create 'a\\nb': the name contains a control character. Choose a different name. No new journal item was created."
        );
    }

    #[test]
    fn file_at_day_is_admission_io() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let chronicle = journal.join("chronicle");
        fs::create_dir_all(&chronicle).unwrap();
        fs::write(chronicle.join(DAY), b"not-a-day").unwrap();
        match preflight_segment_admission(&journal, DAY, STREAM, SEGMENT) {
            Err(NameAdmissionError::Io { .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn resolve_stream_exact_none_when_absent() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        fs::create_dir(&journal).unwrap();
        assert_eq!(resolve_stream_exact(&journal, DAY, STREAM).unwrap(), None);
    }

    #[test]
    fn resolve_stream_exact_some_for_byte_exact_directory() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let stream = journal.join("chronicle").join(DAY).join(STREAM);
        fs::create_dir_all(&stream).unwrap();
        assert_eq!(
            resolve_stream_exact(&journal, DAY, STREAM)
                .unwrap()
                .as_deref(),
            Some(stream.as_path())
        );
    }

    #[test]
    fn resolve_stream_exact_wrong_kind_regular_file() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let day = journal.join("chronicle").join(DAY);
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join(STREAM), b"file").unwrap();
        match resolve_stream_exact(&journal, DAY, STREAM) {
            Err(ExactLookupError::WrongKind {
                kind: NoFollowEntryKind::RegularFile,
                ..
            }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn resolve_segment_exact_wrong_kind_regular_file() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let stream = journal.join("chronicle").join(DAY).join(STREAM);
        fs::create_dir_all(&stream).unwrap();
        fs::write(stream.join(SEGMENT), b"file").unwrap();
        match resolve_segment_exact(&journal, DAY, STREAM, SEGMENT) {
            Err(ExactLookupError::WrongKind {
                kind: NoFollowEntryKind::RegularFile,
                ..
            }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn resolve_stream_exact_reads_con_uppercase_colon_trailing_dot_and_unicode() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let day = journal.join("chronicle").join(DAY);
        for name in ["con", "Main", "a:b", "foo.", "café"] {
            let path = day.join(name);
            fs::create_dir_all(&path).unwrap();
            assert_eq!(
                resolve_stream_exact(&journal, DAY, name)
                    .unwrap()
                    .as_deref(),
                Some(path.as_path()),
                "{name}"
            );
        }
    }

    #[test]
    fn resolve_stream_exact_rejects_slash_nul_dot_and_absolute() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        fs::create_dir(&journal).unwrap();
        assert!(matches!(
            resolve_stream_exact(&journal, DAY, "a/b"),
            Err(ExactLookupError::InvalidComponent {
                reason: NameAdmissionReason::Separator,
                ..
            })
        ));
        assert!(matches!(
            resolve_stream_exact(&journal, DAY, "a\0b"),
            Err(ExactLookupError::InvalidComponent {
                reason: NameAdmissionReason::Control,
                ..
            })
        ));
        assert!(matches!(
            resolve_stream_exact(&journal, DAY, "."),
            Err(ExactLookupError::InvalidComponent {
                reason: NameAdmissionReason::DotComponent,
                ..
            })
        ));
        assert!(matches!(
            resolve_stream_exact(&journal, DAY, "/abs"),
            Err(ExactLookupError::InvalidComponent {
                reason: NameAdmissionReason::RootOrPrefix,
                ..
            })
        ));
        assert!(matches!(
            resolve_stream_exact(&journal, DAY, ""),
            Err(ExactLookupError::InvalidComponent {
                reason: NameAdmissionReason::Empty,
                ..
            })
        ));
    }

    #[test]
    fn segment_path_still_joins_named_and_default_streams() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        fs::create_dir(&journal).unwrap();
        let named = segment_path(&journal, DAY, SEGMENT, "main", true).unwrap();
        assert_eq!(
            named,
            journal
                .join("chronicle")
                .join(DAY)
                .join("main")
                .join(SEGMENT)
        );
        let default = segment_path(&journal, DAY, SEGMENT, DEFAULT_STREAM, true).unwrap();
        assert_eq!(
            default,
            journal
                .join("chronicle")
                .join(DAY)
                .join(DEFAULT_STREAM)
                .join(SEGMENT)
        );
        let missing = segment_path(&journal, "20240104", SEGMENT, "main", false).unwrap();
        assert_eq!(
            missing,
            journal
                .join("chronicle")
                .join("20240104")
                .join("main")
                .join(SEGMENT)
        );
        assert!(matches!(
            day_path(&journal, Some("bad"), false),
            Err(PathError::InvalidRelativePath { .. })
        ));
    }

    #[test]
    fn resolve_segment_locator_exact_selects_direct_and_named_default_without_aliasing() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let twin_day = journal.join("chronicle").join(DAY);
        let direct = twin_day.join("080000_300");
        let named_default = twin_day.join(DEFAULT_STREAM).join("090000_300");
        fs::create_dir_all(&direct).unwrap();
        fs::create_dir_all(&named_default).unwrap();

        assert_eq!(
            resolve_segment_locator_exact(
                &journal,
                DAY,
                DEFAULT_STREAM,
                "080000_300",
                SegmentLayout::Direct,
            )
            .unwrap()
            .as_deref(),
            Some(direct.as_path())
        );
        assert_eq!(
            resolve_segment_locator_exact(
                &journal,
                DAY,
                DEFAULT_STREAM,
                "090000_300",
                SegmentLayout::Named,
            )
            .unwrap()
            .as_deref(),
            Some(named_default.as_path())
        );
        assert_eq!(
            resolve_segment_locator_exact(
                &journal,
                DAY,
                DEFAULT_STREAM,
                "090000_300",
                SegmentLayout::Direct,
            )
            .unwrap(),
            None
        );
        assert_eq!(
            resolve_segment_locator_exact(
                &journal,
                DAY,
                DEFAULT_STREAM,
                "080000_300",
                SegmentLayout::Named,
            )
            .unwrap(),
            None
        );

        let only_direct_day = "20240104";
        let only_direct = journal
            .join("chronicle")
            .join(only_direct_day)
            .join("080000_300");
        fs::create_dir_all(&only_direct).unwrap();
        assert_eq!(
            resolve_segment_locator_exact(
                &journal,
                only_direct_day,
                DEFAULT_STREAM,
                "080000_300",
                SegmentLayout::Named,
            )
            .unwrap(),
            None
        );
        assert_eq!(
            resolve_segment_locator_exact(
                &journal,
                only_direct_day,
                DEFAULT_STREAM,
                "080000_300",
                SegmentLayout::Direct,
            )
            .unwrap()
            .as_deref(),
            Some(only_direct.as_path())
        );

        let only_named_day = "20240105";
        let only_named = journal
            .join("chronicle")
            .join(only_named_day)
            .join(DEFAULT_STREAM)
            .join("080000_300");
        fs::create_dir_all(&only_named).unwrap();
        assert_eq!(
            resolve_segment_locator_exact(
                &journal,
                only_named_day,
                DEFAULT_STREAM,
                "080000_300",
                SegmentLayout::Direct,
            )
            .unwrap(),
            None
        );
        assert_eq!(
            resolve_segment_locator_exact(
                &journal,
                only_named_day,
                DEFAULT_STREAM,
                "080000_300",
                SegmentLayout::Named,
            )
            .unwrap()
            .as_deref(),
            Some(only_named.as_path())
        );

        match resolve_segment_locator_exact(
            &journal,
            DAY,
            "somestream",
            SEGMENT,
            SegmentLayout::Direct,
        ) {
            Err(ExactLookupError::LayoutMismatch { stream }) => {
                assert_eq!(stream, "somestream");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn resolve_segment_locator_exact_reads_legacy_forbidden_utf8_names() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let names = ["con", "CON", "Main", "a:b", "foo.", "foo ", "café", "a\tb"];
        let direct_day = journal.join("chronicle").join(DAY);
        for name in names {
            let path = direct_day.join(name);
            fs::create_dir_all(&path).unwrap();
            assert_eq!(
                resolve_segment_locator_exact(
                    &journal,
                    DAY,
                    DEFAULT_STREAM,
                    name,
                    SegmentLayout::Direct,
                )
                .unwrap()
                .as_deref(),
                Some(path.as_path()),
                "direct {name:?}"
            );
        }

        let named_day = "20240104";
        let named_day_dir = journal.join("chronicle").join(named_day);
        for name in names {
            let stream_path = named_day_dir.join(name).join(SEGMENT);
            fs::create_dir_all(&stream_path).unwrap();
            assert_eq!(
                resolve_segment_locator_exact(
                    &journal,
                    named_day,
                    name,
                    SEGMENT,
                    SegmentLayout::Named,
                )
                .unwrap()
                .as_deref(),
                Some(stream_path.as_path()),
                "named stream {name:?}"
            );

            let segment_path = named_day_dir.join(STREAM).join(name);
            fs::create_dir_all(&segment_path).unwrap();
            assert_eq!(
                resolve_segment_locator_exact(
                    &journal,
                    named_day,
                    STREAM,
                    name,
                    SegmentLayout::Named,
                )
                .unwrap()
                .as_deref(),
                Some(segment_path.as_path()),
                "named segment {name:?}"
            );
        }
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::{
        ExactLookupError, create_segment_strict, resolve_segment_exact,
        resolve_segment_locator_exact, resolve_stream_exact,
    };
    use crate::name_admission::{NameAdmissionError, NameAdmissionReason, NoFollowEntryKind};
    use crate::paths::{DEFAULT_STREAM, SegmentLayout};
    use crate::test_support::TempDir;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    const DAY: &str = "20240103";
    const STREAM: &str = "import.apple_health";
    const SEGMENT: &str = "000000_300";

    #[test]
    fn resolve_stream_exact_backslash_is_not_a_separator() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let name = r"foo\bar";
        let path = journal.join("chronicle").join(DAY).join(name);
        fs::create_dir_all(&path).unwrap();
        assert_eq!(
            resolve_stream_exact(&journal, DAY, name)
                .unwrap()
                .as_deref(),
            Some(path.as_path())
        );
    }

    #[test]
    fn resolve_wrong_kind_symlink_and_dangling_and_fifo() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let day = journal.join("chronicle").join(DAY);
        fs::create_dir_all(&day).unwrap();

        let target = journal.join("inside-target");
        fs::create_dir(&target).unwrap();
        symlink(&target, day.join("linked")).unwrap();
        match resolve_stream_exact(&journal, DAY, "linked") {
            Err(ExactLookupError::WrongKind {
                kind: NoFollowEntryKind::Symlink,
                ..
            }) => {}
            other => panic!("{other:?}"),
        }

        symlink(day.join("missing-target"), day.join("dangling")).unwrap();
        match resolve_stream_exact(&journal, DAY, "dangling") {
            Err(ExactLookupError::WrongKind {
                kind: NoFollowEntryKind::Symlink,
                ..
            }) => {}
            other => panic!("{other:?}"),
        }

        mkfifo(&day.join("pipe"));
        match resolve_stream_exact(&journal, DAY, "pipe") {
            Err(ExactLookupError::WrongKind {
                kind: NoFollowEntryKind::Other,
                ..
            }) => {}
            other => panic!("{other:?}"),
        }

        let stream = day.join(STREAM);
        fs::create_dir(&stream).unwrap();
        symlink(&target, stream.join(SEGMENT)).unwrap();
        match resolve_segment_exact(&journal, DAY, STREAM, SEGMENT) {
            Err(ExactLookupError::WrongKind {
                kind: NoFollowEntryKind::Symlink,
                ..
            }) => {}
            other => panic!("{other:?}"),
        }
        fs::remove_file(stream.join(SEGMENT)).unwrap();
        symlink(stream.join("missing-target"), stream.join(SEGMENT)).unwrap();
        match resolve_segment_exact(&journal, DAY, STREAM, SEGMENT) {
            Err(ExactLookupError::WrongKind {
                kind: NoFollowEntryKind::Symlink,
                ..
            }) => {}
            other => panic!("{other:?}"),
        }
        fs::remove_file(stream.join(SEGMENT)).unwrap();
        mkfifo(&stream.join(SEGMENT));
        match resolve_segment_exact(&journal, DAY, STREAM, SEGMENT) {
            Err(ExactLookupError::WrongKind {
                kind: NoFollowEntryKind::Other,
                ..
            }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn symlink_realpath_io_is_not_wrong_kind() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let day = journal.join("chronicle").join(DAY);
        fs::create_dir_all(&day).unwrap();
        let looped = day.join("looped");
        symlink(&looped, &looped).unwrap();
        match resolve_stream_exact(&journal, DAY, "looped") {
            Err(ExactLookupError::Io { .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn resolve_segment_locator_exact_symlink_realpath_io_is_not_wrong_kind() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let day = journal.join("chronicle").join(DAY);
        fs::create_dir_all(&day).unwrap();
        let looped = day.join("looped");
        symlink(&looped, &looped).unwrap();
        match resolve_segment_locator_exact(&journal, DAY, "looped", SEGMENT, SegmentLayout::Named)
        {
            Err(ExactLookupError::Io { .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn resolve_segment_locator_exact_absent_or_refused_does_not_create() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        fs::create_dir(&journal).unwrap();
        let before = snapshot_tree(&journal);

        assert_eq!(
            resolve_segment_locator_exact(
                &journal,
                DAY,
                DEFAULT_STREAM,
                SEGMENT,
                SegmentLayout::Direct,
            )
            .unwrap(),
            None
        );
        assert_eq!(before, snapshot_tree(&journal));

        assert_eq!(
            resolve_segment_locator_exact(&journal, DAY, STREAM, SEGMENT, SegmentLayout::Named,)
                .unwrap(),
            None
        );
        assert_eq!(before, snapshot_tree(&journal));

        match resolve_segment_locator_exact(
            &journal,
            DAY,
            "somestream",
            SEGMENT,
            SegmentLayout::Direct,
        ) {
            Err(ExactLookupError::LayoutMismatch { stream }) => {
                assert_eq!(stream, "somestream");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(before, snapshot_tree(&journal));

        match resolve_segment_locator_exact(&journal, DAY, "a/b", SEGMENT, SegmentLayout::Named) {
            Err(ExactLookupError::InvalidComponent {
                reason: NameAdmissionReason::Separator,
                ..
            }) => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(before, snapshot_tree(&journal));

        match resolve_segment_locator_exact(
            &journal,
            DAY,
            DEFAULT_STREAM,
            ".",
            SegmentLayout::Direct,
        ) {
            Err(ExactLookupError::InvalidComponent {
                reason: NameAdmissionReason::DotComponent,
                ..
            }) => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(before, snapshot_tree(&journal));

        match resolve_segment_locator_exact(&journal, DAY, "", SEGMENT, SegmentLayout::Named) {
            Err(ExactLookupError::InvalidComponent {
                reason: NameAdmissionReason::Empty,
                ..
            }) => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(before, snapshot_tree(&journal));
    }

    #[test]
    fn resolve_segment_locator_exact_backslash_is_not_a_separator() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let name = r"foo\bar";
        let direct = journal.join("chronicle").join(DAY).join(name);
        fs::create_dir_all(&direct).unwrap();
        assert_eq!(
            resolve_segment_locator_exact(
                &journal,
                DAY,
                DEFAULT_STREAM,
                name,
                SegmentLayout::Direct,
            )
            .unwrap()
            .as_deref(),
            Some(direct.as_path())
        );

        let named_day = "20240104";
        let named = journal
            .join("chronicle")
            .join(named_day)
            .join(name)
            .join(SEGMENT);
        fs::create_dir_all(&named).unwrap();
        assert_eq!(
            resolve_segment_locator_exact(
                &journal,
                named_day,
                name,
                SEGMENT,
                SegmentLayout::Named,
            )
            .unwrap()
            .as_deref(),
            Some(named.as_path())
        );
    }

    fn snapshot_tree(root: &Path) -> Vec<(PathBuf, bool)> {
        fn walk(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, bool)>) {
            let mut entries: Vec<_> = fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap())
                .collect();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = entry.path();
                let is_dir = entry.file_type().unwrap().is_dir();
                out.push((path.strip_prefix(root).unwrap().to_path_buf(), is_dir));
                if is_dir {
                    walk(root, &path, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out
    }

    #[test]
    fn lookup_containment_rejects_symlink_escape_at_each_level() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let outside = temporary.path().join("outside");
        fs::create_dir_all(&journal).unwrap();
        fs::create_dir(&outside).unwrap();

        symlink(&outside, journal.join("chronicle")).unwrap();
        match resolve_stream_exact(&journal, DAY, STREAM) {
            Err(ExactLookupError::Containment(_)) => {}
            other => panic!("chronicle: {other:?}"),
        }
        fs::remove_file(journal.join("chronicle")).unwrap();

        let chronicle = journal.join("chronicle");
        fs::create_dir(&chronicle).unwrap();
        symlink(&outside, chronicle.join(DAY)).unwrap();
        match resolve_stream_exact(&journal, DAY, STREAM) {
            Err(ExactLookupError::Containment(_)) => {}
            other => panic!("day: {other:?}"),
        }
        fs::remove_file(chronicle.join(DAY)).unwrap();

        let day = chronicle.join(DAY);
        fs::create_dir(&day).unwrap();
        symlink(&outside, day.join(STREAM)).unwrap();
        match resolve_stream_exact(&journal, DAY, STREAM) {
            Err(ExactLookupError::Containment(_)) => {}
            other => panic!("stream: {other:?}"),
        }
        fs::remove_file(day.join(STREAM)).unwrap();

        let stream = day.join(STREAM);
        fs::create_dir(&stream).unwrap();
        symlink(&outside, stream.join(SEGMENT)).unwrap();
        match resolve_segment_exact(&journal, DAY, STREAM, SEGMENT) {
            Err(ExactLookupError::Containment(_)) => {}
            other => panic!("segment: {other:?}"),
        }
    }

    #[test]
    fn create_containment_uses_journal_root() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let outside = temporary.path().join("outside");
        fs::create_dir_all(&journal).unwrap();
        fs::create_dir(&outside).unwrap();
        let chronicle = journal.join("chronicle");
        fs::create_dir(&chronicle).unwrap();
        symlink(&outside, chronicle.join(DAY)).unwrap();
        match create_segment_strict(&journal, DAY, STREAM, SEGMENT) {
            Err(super::StrictCreateError::Admission(NameAdmissionError::Containment(_))) => {}
            other => panic!("{other:?}"),
        }
        assert!(!outside.join(STREAM).exists());
    }

    fn mkfifo(path: &Path) {
        nix::unistd::mkfifo(
            path,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .expect("mkfifo");
    }
}
