// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use solstone_core_journal_io::{DEFAULT_STREAM, contained_path, day_path, segment_path};

use crate::SegmentError;

/// A resolved journal segment directory with no creation side effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentDir {
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
        let path = if stream == DEFAULT_STREAM {
            let day_dir = day_path(journal, Some(day), false)?;
            contained_path(&day_dir, segment)?
        } else {
            segment_path(journal, day, segment, stream, false)?
        };
        Ok(Self {
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
}
