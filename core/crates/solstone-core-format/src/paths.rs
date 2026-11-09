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
    /// The trash folder, which nothing outside the trash reads.
    Trash,
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
            JournalPathError::Trash => "rel must not name the trash",
        })
    }
}

impl std::error::Error for JournalPathError {}

pub fn resolve_journal_path(journal: &Path, rel: &str) -> Result<PathBuf, JournalPathError> {
    let rel = normalize_rel(rel);
    validate_rel(&rel)?;
    let first = rel.split('/').next().unwrap_or("");
    if solstone_core_journal::is_hidden_root_name(std::ffi::OsStr::new(first)) {
        return Err(JournalPathError::Trash);
    }
    let mut resolved = if is_date_key(first) {
        journal.join(CHRONICLE_DIR)
    } else {
        journal.to_path_buf()
    };
    for part in rel.split('/') {
        resolved.push(part);
    }
    Ok(resolved)
}

/// Drop trailing slashes, repeated separators, and `.` the way pathlib parts do.
fn normalize_rel(rel: &str) -> String {
    if rel.contains('\\') || rel.starts_with('/') || Path::new(rel).has_root() {
        return rel.to_owned();
    }
    let parts: Vec<&str> = rel
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty() {
        return rel.to_owned();
    }
    parts.join("/")
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
    if rel.starts_with('/') || Path::new(rel).has_root() {
        return Err(JournalPathError::Absolute);
    }
    if rel.contains('\\') {
        return Err(JournalPathError::Backslash);
    }
    if rel.split('/').any(|part| {
        part.is_empty()
            || part == "."
            || part == ".."
            || !matches!(
                Path::new(part).components().collect::<Vec<_>>().as_slice(),
                [std::path::Component::Normal(_)]
            )
    }) {
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

    #[test]
    fn normalises_trailing_slashes_dots_and_repeated_separators() {
        let journal = Path::new("/tmp/journal");
        assert_eq!(
            resolve_journal_path(journal, "chronicle/").unwrap(),
            PathBuf::from("/tmp/journal/chronicle")
        );
        assert_eq!(
            resolve_journal_path(journal, "./facets/x").unwrap(),
            PathBuf::from("/tmp/journal/facets/x")
        );
        assert_eq!(
            resolve_journal_path(journal, "facets//x").unwrap(),
            PathBuf::from("/tmp/journal/facets/x")
        );
        assert_eq!(
            resolve_journal_path(journal, "20240101/").unwrap(),
            PathBuf::from("/tmp/journal/chronicle/20240101")
        );
    }

    #[test]
    fn resolved_paths_use_native_separators_without_changing_the_journal() {
        let journal = Path::new(if cfg!(windows) {
            r"C:\journal"
        } else {
            "/journal"
        });
        let expected = journal
            .join("chronicle")
            .join("20240101")
            .join("talents")
            .join("flow.md");
        let actual = resolve_journal_path(journal, "20240101/talents/flow.md").unwrap();
        assert_eq!(actual.as_os_str(), expected.as_os_str());
    }

    #[cfg(windows)]
    #[test]
    fn drive_relative_components_never_replace_the_journal_root() {
        let journal = Path::new(r"C:\journal");
        for rel in ["D:escape", "chronicle/D:escape", "D:/escape"] {
            assert!(resolve_journal_path(journal, rel).is_err(), "{rel}");
        }
    }

    #[test]
    fn still_rejects_empty_dot_parent_absolute_and_backslash() {
        let journal = Path::new("/tmp/journal");
        assert_eq!(
            resolve_journal_path(journal, "").unwrap_err(),
            JournalPathError::Empty
        );
        assert_eq!(
            resolve_journal_path(journal, ".").unwrap_err(),
            JournalPathError::InvalidComponent
        );
        assert_eq!(
            resolve_journal_path(journal, "./").unwrap_err(),
            JournalPathError::InvalidComponent
        );
        assert_eq!(
            resolve_journal_path(journal, "//").unwrap_err(),
            JournalPathError::Absolute
        );
        assert_eq!(
            resolve_journal_path(journal, "..").unwrap_err(),
            JournalPathError::InvalidComponent
        );
        assert_eq!(
            resolve_journal_path(journal, "/abs").unwrap_err(),
            JournalPathError::Absolute
        );
        assert_eq!(
            resolve_journal_path(journal, "a\\b").unwrap_err(),
            JournalPathError::Backslash
        );
    }
}

#[cfg(test)]
mod trash_resolution_tests {
    use super::{JournalPathError, resolve_journal_path};
    use std::path::Path;

    #[test]
    fn a_path_into_the_trash_resolves_to_nothing() {
        let journal = Path::new("/journal");
        assert_eq!(
            resolve_journal_path(journal, ".trash/x/files").unwrap_err(),
            JournalPathError::Trash
        );
        assert!(resolve_journal_path(journal, "20260731/x").is_ok());
    }
}
