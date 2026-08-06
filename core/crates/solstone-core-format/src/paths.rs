// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::path::{Path, PathBuf};

use crate::segment::is_date_key;

pub const CHRONICLE_DIR: &str = "chronicle";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalPathError {
    Empty,
    Absolute,
    Backslash,
    InvalidComponent,
}

impl fmt::Display for JournalPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            JournalPathError::Empty => "rel must be non-empty",
            JournalPathError::Absolute => "rel must be journal-relative",
            JournalPathError::Backslash => "rel must use POSIX separators",
            JournalPathError::InvalidComponent => {
                "rel must not contain empty, '.', or '..' components"
            }
        })
    }
}

impl std::error::Error for JournalPathError {}

pub fn resolve_journal_path(journal: &Path, rel: &str) -> Result<PathBuf, JournalPathError> {
    validate_rel(rel)?;
    let first = rel.split('/').next().unwrap_or("");
    if is_date_key(first) {
        Ok(journal.join(CHRONICLE_DIR).join(rel))
    } else {
        Ok(journal.join(rel))
    }
}

pub fn relative_to_journal(journal: &Path, abs_path: &Path) -> Option<String> {
    let chronicle_root = journal.join(CHRONICLE_DIR);
    if let Ok(rel) = abs_path.strip_prefix(&chronicle_root) {
        return path_to_posix(rel);
    }
    abs_path.strip_prefix(journal).ok().and_then(path_to_posix)
}

fn validate_rel(rel: &str) -> Result<(), JournalPathError> {
    if rel.is_empty() {
        return Err(JournalPathError::Empty);
    }
    if Path::new(rel).is_absolute() {
        return Err(JournalPathError::Absolute);
    }
    if rel.contains('\\') {
        return Err(JournalPathError::Backslash);
    }
    if rel
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(JournalPathError::InvalidComponent);
    }
    Ok(())
}

fn path_to_posix(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for part in path.components() {
        parts.push(part.as_os_str().to_str()?);
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_date_prefixed_rels_under_chronicle() {
        let journal = Path::new("/tmp/journal");
        assert_eq!(
            resolve_journal_path(journal, "20240101/talents/flow.md").unwrap(),
            PathBuf::from("/tmp/journal/chronicle/20240101/talents/flow.md")
        );
        assert_eq!(
            resolve_journal_path(journal, "facets/work/news/20240101.md").unwrap(),
            PathBuf::from("/tmp/journal/facets/work/news/20240101.md")
        );
    }
}
