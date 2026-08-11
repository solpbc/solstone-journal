// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One owner for segment-layout relations.
//!
//! # 🔴 The default stream has no directory
//!
//! `_default` is the owner-facing name for a segment directly below a day. Its
//! omitted disk component cannot be derived from an interpolated path, so this
//! explicit branch owns disk, parent, and index relations together. Keeping it
//! here prevents a reader from fixing one projection while leaving another
//! pointed at a non-existent `_default` directory.

use std::path::{Path, PathBuf};

use solstone_core_journal_io::DEFAULT_STREAM;
use solstone_core_segment::{SegmentDir, SegmentError};

#[derive(Clone, Debug)]
pub(crate) struct SegmentLocation {
    pub(crate) day: String,
    pub(crate) stream: String,
    pub(crate) segment: String,
    pub(crate) path: PathBuf,
    pub(crate) disk_rel: String,
    pub(crate) parent_rel: String,
    pub(crate) index_rel: String,
}

impl SegmentLocation {
    pub(crate) fn resolve(
        journal: &Path,
        day: &str,
        stream: &str,
        segment: &str,
    ) -> Result<Self, SegmentError> {
        let handle = SegmentDir::resolve(journal, day, segment, stream)?;
        let (disk_rel, parent_rel, index_rel) = if stream == DEFAULT_STREAM {
            (
                format!("chronicle/{day}/{segment}"),
                format!("chronicle/{day}"),
                format!("{day}/{segment}"),
            )
        } else {
            (
                format!("chronicle/{day}/{stream}/{segment}"),
                format!("chronicle/{day}/{stream}"),
                format!("{day}/{stream}/{segment}"),
            )
        };
        Ok(Self {
            day: day.to_owned(),
            stream: stream.to_owned(),
            segment: segment.to_owned(),
            path: handle.path().to_path_buf(),
            disk_rel,
            parent_rel,
            index_rel,
        })
    }

    pub(crate) fn token(&self) -> String {
        format!("{}/{}/{}", self.day, self.stream, self.segment)
    }
}
