// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use solstone_core_journal_io::{
    DEFAULT_STREAM, PathError, PathEscapeError, PathOrDay, Segment, contained_path, day_dirs,
    day_path, iter_segments,
};

use crate::SegmentError;

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
        let _ = day_path(journal, Some(day), false)?;
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

/// Segment `(stream, key)` pairs under one chronicle day.
pub fn list_segments(journal: &Path, day: &str) -> Result<Vec<Segment>, SegmentError> {
    Ok(iter_segments(journal, PathOrDay::Day(day))?)
}

/// Segment `(stream, key)` pairs under an already-resolved day directory.
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
}
