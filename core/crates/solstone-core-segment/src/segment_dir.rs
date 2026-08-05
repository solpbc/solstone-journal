// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use solstone_core_journal_io::{DEFAULT_STREAM, contained_path, day_path};

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
        let day_dir = day_path(journal, Some(day), false)?;
        let path = if stream == DEFAULT_STREAM {
            contained_path(&day_dir, segment)?
        } else {
            contained_path(&day_dir, &format!("{stream}/{segment}"))?
        };
        Ok(Self {
            journal: journal.to_path_buf(),
            path,
            day: day.to_owned(),
            segment: segment.to_owned(),
            stream: stream.to_owned(),
        })
    }
}

fn validate_component(value: &str, kind: &'static str) -> Result<(), SegmentError> {
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || matches!(value, "." | "..")
    {
        return Err(SegmentError::StreamInput(match kind {
            "segment" => "segment must be a plain path component",
            _ => "stream must be a plain path component",
        }));
    }
    Ok(())
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
}
