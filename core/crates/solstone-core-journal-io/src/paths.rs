// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-relative and chronicle path helpers.

use std::collections::HashMap;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use chrono::Local;

use crate::errors::{PathError, PathEscapeError};

const CHRONICLE_DIR: &str = "chronicle";
/// Default stream directory below a chronicle day.
pub const DEFAULT_STREAM: &str = "_default";

/// A day key or an already-resolved day directory.
#[derive(Debug, Clone, Copy)]
pub enum PathOrDay<'a> {
    /// A `YYYYMMDD` chronicle day.
    Day(&'a str),
    /// A day directory resolved by the caller.
    Directory(&'a Path),
}

/// One discovered chronicle segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Stream owning the segment (`_default` for direct day children).
    pub stream: String,
    /// Extracted `HHMMSS_LEN` segment key.
    pub key: String,
    /// Full path to the segment directory.
    pub path: PathBuf,
}

/// Kind of one entry returned by [`list_dir_entries`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirEntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// Any other filesystem entry, including a symlink.
    Other,
}

/// One deterministic entry from a journal directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Entry basename.
    pub name: std::ffi::OsString,
    /// Full path to the entry.
    pub path: PathBuf,
    /// Entry kind without following symlinks.
    pub kind: DirEntryKind,
}

/// Re-export the existing core journal-root resolver.
pub use solstone_core_journal::resolve_journal_path as resolve_configured_journal;

/// Resolve a Python-compatible journal-relative path against `journal`.
pub fn resolve_journal_path(journal: &Path, rel: &str) -> Result<PathBuf, PathError> {
    if rel.trim().is_empty() {
        return Err(invalid(rel, "journal path must not be empty"));
    }
    if Path::new(rel).is_absolute() {
        return Err(invalid(rel, "journal path must be relative"));
    }
    if rel.contains('\\') {
        return Err(invalid(rel, "journal path must use forward slashes"));
    }
    if rel
        .split('/')
        .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(invalid(rel, "journal path contains invalid component"));
    }
    let relative = Path::new(rel);
    let mut components = relative.components();
    let resolved = match components
        .next()
        .and_then(|component| component.as_os_str().to_str())
    {
        Some(day) if is_day_key(day) => journal.join(CHRONICLE_DIR).join(relative),
        _ => journal.join(relative),
    };
    Ok(resolved)
}

/// Return whether `path` exists, including a dangling symlink.
pub fn path_lexists(path: &Path) -> Result<bool, PathError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(path_io(path, source)),
    }
}

/// Create a directory and any missing parents without changing existing contents.
pub fn ensure_directory(path: &Path) -> Result<(), PathError> {
    fs::create_dir_all(path).map_err(|source| path_io(path, source))
}

/// Create a directory tree and set the final directory's Unix permission mode.
///
/// Existing contents are preserved. On Unix, the final directory is always
/// normalized to `mode`; on other targets, creation remains available and the
/// mode argument is intentionally ignored.
pub fn create_directory_with_mode(path: &Path, mode: u32) -> Result<(), PathError> {
    fs::create_dir_all(path).map_err(|source| path_io(path, source))?;
    let metadata = fs::symlink_metadata(path).map_err(|source| path_io(path, source))?;
    if !metadata.file_type().is_dir() {
        return Err(path_io(
            path,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "private directory path must not be a symlink",
            ),
        ));
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|source| path_io(path, source))?;
    #[cfg(not(unix))]
    let _ = mode;
    Ok(())
}

/// List direct directory entries by name without creating anything.
///
/// A missing path or a path that is not a directory produces no entries. This
/// mirrors readers that treat absent durable-store subdirectories as empty.
pub fn list_dir_entries(dir: &Path) -> Result<Vec<DirEntry>, PathError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new());
        }
        Err(source) => return Err(path_io(dir, source)),
    };

    let mut listed = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| path_io(dir, source))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|source| path_io(&path, source))?;
        listed.push(DirEntry {
            name: entry.file_name(),
            path,
            kind: if kind.is_file() {
                DirEntryKind::File
            } else if kind.is_dir() {
                DirEntryKind::Directory
            } else {
                DirEntryKind::Other
            },
        });
    }
    listed.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(listed)
}

/// Resolve `rel` below `root`, rejecting a symlink-aware escape.
pub fn contained_path(root: &Path, rel: &str) -> Result<PathBuf, PathError> {
    let lexical = resolve_journal_path(root, rel)?;
    let root = realpath_non_strict(root)?;
    let candidate = realpath_non_strict(&lexical)?;
    if candidate.starts_with(&root) {
        Ok(candidate)
    } else {
        Err(PathError::Escape(PathEscapeError {
            path: candidate,
            rel: rel.to_owned(),
        }))
    }
}

/// Return the requested day directory, creating it by default.
pub fn day_path(journal: &Path, day: Option<&str>, create: bool) -> Result<PathBuf, PathError> {
    let day = day
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| Local::now().format("%Y%m%d").to_string());
    if !is_day_key(&day) {
        return Err(invalid(&day, "day must be YYYYMMDD"));
    }
    let path = journal.join(CHRONICLE_DIR).join(day);
    if create {
        fs::create_dir_all(&path).map_err(|source| path_io(&path, source))?;
    }
    Ok(path)
}

/// Map every direct `YYYYMMDD` chronicle directory to its path.
pub fn day_dirs(journal: &Path) -> Result<HashMap<String, PathBuf>, PathError> {
    let chronicle = journal.join(CHRONICLE_DIR);
    if !chronicle.is_dir() {
        return Ok(HashMap::new());
    }
    let mut days = HashMap::new();
    for entry in fs::read_dir(&chronicle).map_err(|source| path_io(&chronicle, source))? {
        let entry = entry.map_err(|source| path_io(&chronicle, source))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if is_day_key(name) && entry.path().is_dir() {
            days.insert(name.to_owned(), entry.path());
        }
    }
    Ok(days)
}

/// Return the segment directory for `segment`, optionally creating it.
pub fn segment_path(
    journal: &Path,
    day: &str,
    segment: &str,
    stream: &str,
    create: bool,
) -> Result<PathBuf, PathError> {
    let day_dir = day_path(journal, Some(day), create)?;
    let path = day_dir.join(stream).join(segment);
    if create {
        let contained = contained_path(&day_dir, &format!("{stream}/{segment}"))?;
        fs::create_dir_all(&contained).map_err(|source| path_io(&contained, source))?;
        return Ok(contained);
    }
    Ok(path)
}

/// Iterate direct segment directories in Python-compatible name order.
pub fn iter_segments(journal: &Path, day: PathOrDay<'_>) -> Result<Vec<Segment>, PathError> {
    let day_dir = match day {
        PathOrDay::Day(day) => day_path(journal, Some(day), false)?,
        PathOrDay::Directory(path) => path.to_path_buf(),
    };
    if !day_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut segments = Vec::new();
    for entry in fs::read_dir(&day_dir).map_err(|source| path_io(&day_dir, source))? {
        let entry = entry.map_err(|source| path_io(&day_dir, source))?;
        if !entry.path().is_dir() {
            continue;
        }
        let entry_name = entry.file_name().to_string_lossy().into_owned();
        if let Some(key) = segment_key(&entry_name) {
            segments.push(Segment {
                stream: DEFAULT_STREAM.to_owned(),
                key: key.to_owned(),
                path: entry.path(),
            });
            continue;
        }
        if entry_name == "health" {
            continue;
        }
        for segment_entry in
            fs::read_dir(entry.path()).map_err(|source| path_io(&entry.path(), source))?
        {
            let segment_entry = segment_entry.map_err(|source| path_io(&entry.path(), source))?;
            let name = segment_entry.file_name().to_string_lossy().into_owned();
            if segment_entry.path().is_dir()
                && let Some(key) = segment_key(&name)
            {
                segments.push(Segment {
                    stream: entry_name.clone(),
                    key: key.to_owned(),
                    path: segment_entry.path(),
                });
            }
        }
    }
    segments.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(segments)
}

fn realpath_non_strict(path: &Path) -> Result<PathBuf, PathError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| path_io(path, source))?
            .join(path)
    };
    let mut suffix = Vec::new();
    let mut existing = absolute.as_path();
    while !path_lexists(existing)? {
        let Some(name) = existing.file_name() else {
            break;
        };
        suffix.push(name.to_os_string());
        let Some(parent) = existing.parent() else {
            break;
        };
        existing = parent;
    }
    let mut resolved = fs::canonicalize(existing).map_err(|source| path_io(existing, source))?;
    for component in suffix.iter().rev() {
        resolved.push(component);
    }
    Ok(normalize_lexical(resolved))
}

fn normalize_lexical(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn is_day_key(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn segment_key(value: &str) -> Option<&str> {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index + 8 <= bytes.len() {
        let word_before = index == 0 || !is_word_byte(bytes[index - 1]);
        if !word_before || !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        if !bytes[index..index + 6]
            .iter()
            .all(|byte| byte.is_ascii_digit())
            || bytes[index + 6] != b'_'
        {
            index += 1;
            continue;
        }
        let mut end = index + 7;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > index + 7
            && (end == bytes.len() || bytes[end] == b'_' || !is_word_byte(bytes[end]))
        {
            return value.get(index..end);
        }
        index += 1;
    }
    None
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn invalid(rel: &str, message: &'static str) -> PathError {
    PathError::InvalidRelativePath {
        rel: rel.to_owned(),
        message,
    }
}

fn path_io(path: &Path, source: io::Error) -> PathError {
    PathError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn contained_path_rejects_a_symlink_escape() {
        let temporary = TempDir::new();
        let root = temporary.path().join("journal");
        let outside = temporary.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&outside, root.join("escape")).unwrap();

        assert!(matches!(
            contained_path(&root, "escape/file"),
            Err(PathError::Escape(_))
        ));
        assert_eq!(
            contained_path(&root, "safe/file").unwrap(),
            root.join("safe/file")
        );
    }

    #[test]
    fn list_dir_entries_is_sorted_and_missing_is_empty() {
        let temporary = TempDir::new();
        let directory = temporary.path().join("entries");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("z.json"), b"z").unwrap();
        fs::create_dir(directory.join("a")).unwrap();

        assert_eq!(
            list_dir_entries(&temporary.path().join("missing")).unwrap(),
            []
        );
        assert_eq!(
            list_dir_entries(&directory).unwrap(),
            vec![
                DirEntry {
                    name: "a".into(),
                    path: directory.join("a"),
                    kind: DirEntryKind::Directory,
                },
                DirEntry {
                    name: "z.json".into(),
                    path: directory.join("z.json"),
                    kind: DirEntryKind::File,
                },
            ]
        );
    }

    #[test]
    fn create_directory_with_mode_normalizes_final_directory_privacy() {
        let temporary = TempDir::new();
        let directory = temporary.path().join("private").join("imports");

        create_directory_with_mode(&directory, 0o700).unwrap();
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );

        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        create_directory_with_mode(&directory, 0o700).unwrap();
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let outside = temporary.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
        let linked = temporary.path().join("linked");
        symlink(&outside, &linked).unwrap();
        assert!(create_directory_with_mode(&linked, 0o700).is_err());
        assert_eq!(
            fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn day_and_segment_helpers_match_chronicle_layout() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let day = day_path(&journal, Some("20260102"), true).unwrap();
        assert!(day.is_dir());
        assert!(matches!(
            day_path(&journal, Some("2026-01-02"), false),
            Err(PathError::InvalidRelativePath { .. })
        ));
        let segment = segment_path(&journal, "20260102", "123456_300", "other", true).unwrap();
        fs::create_dir_all(day.join("080000_300")).unwrap();
        fs::create_dir_all(day.join("health/654321_300")).unwrap();
        fs::create_dir_all(day.join("other/093000_300_summary")).unwrap();
        fs::create_dir_all(day.join("other/not-a-segment")).unwrap();

        let days = day_dirs(&journal).unwrap();
        assert_eq!(days.get("20260102"), Some(&day));
        assert_eq!(
            iter_segments(&journal, PathOrDay::Day("20260102")).unwrap(),
            vec![
                Segment {
                    stream: DEFAULT_STREAM.to_owned(),
                    key: "080000_300".to_owned(),
                    path: day.join("080000_300")
                },
                Segment {
                    stream: "other".to_owned(),
                    key: "093000_300".to_owned(),
                    path: day.join("other/093000_300_summary")
                },
                Segment {
                    stream: "other".to_owned(),
                    key: "123456_300".to_owned(),
                    path: segment
                },
            ]
        );
    }
}
