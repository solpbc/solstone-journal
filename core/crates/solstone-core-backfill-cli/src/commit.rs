// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::fs;
use std::path::Path;

use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, bump_stream_marker};

use crate::classify::Eligible;

pub trait Writer {
    fn replace(&self, path: &Path, contents: &[u8]) -> Result<(), String>;
}

#[derive(Debug, Default)]
pub struct AtomicWriter;

impl Writer for AtomicWriter {
    fn replace(&self, path: &Path, contents: &[u8]) -> Result<(), String> {
        let options = write_options(path)?;
        atomic_replace(path, contents, options).map_err(|error| error.to_string())
    }
}

#[cfg(unix)]
fn write_options(path: &Path) -> Result<AtomicWriteOptions, String> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .map_err(|error| error.to_string())?
        .permissions()
        .mode();
    Ok(AtomicWriteOptions { mode: Some(mode) })
}

#[cfg(not(unix))]
fn write_options(_path: &Path) -> Result<AtomicWriteOptions, String> {
    Ok(AtomicWriteOptions::default())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommitError {
    Changed,
    Read(String),
    Write(String),
    Marker(String),
}

impl fmt::Display for CommitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Changed => formatter.write_str("file changed after classification"),
            Self::Read(error) => write!(formatter, "could not re-read file: {error}"),
            Self::Write(error) => write!(formatter, "write failed: {error}"),
            Self::Marker(error) => write!(formatter, "stream marker write failed: {error}"),
        }
    }
}

pub(crate) fn commit(
    journal: &Path,
    item: &Eligible,
    writer: &dyn Writer,
) -> Result<(), CommitError> {
    let current = fs::read(&item.path).map_err(|error| CommitError::Read(error.to_string()))?;
    if current != item.original {
        return Err(CommitError::Changed);
    }
    writer
        .replace(&item.path, &item.replacement)
        .map_err(CommitError::Write)?;
    bump_stream_marker(journal, &item.day)
        .map(|_| ())
        .map_err(|error| CommitError::Marker(error.to_string()))
}
