// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-relative and chronicle path helpers.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::errors::{PathError, PathEscapeError, SegmentIdentityError};

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

/// Layout of one discovered chronicle segment.
///
/// `Direct` is a child of the day directory. `Named` is a child of an exact
/// stream-directory basename, including a directory literally named `_default`.
/// The `_default` filter spelling selects `Direct` only. A `Named` directory
/// whose UTF-8 name is `_default` has no [`RecordIdentity`]: that spelling is
/// reserved for [`Direct`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamLocation {
    /// Segment directory sits directly under the chronicle day.
    Direct,
    /// Exact native basename of the stream directory.
    Named(OsString),
}

impl StreamLocation {
    /// True when the segment is a direct day child.
    #[must_use]
    pub fn is_direct(&self) -> bool {
        matches!(self, Self::Direct)
    }

    /// Exact stream-directory name, or `None` for the direct layout.
    #[must_use]
    pub fn directory(&self) -> Option<&OsStr> {
        match self {
            Self::Direct => None,
            Self::Named(name) => Some(name),
        }
    }

    /// Match a UTF-8 filter. `"_default"` selects [`Direct`] only.
    #[must_use]
    pub fn matches(&self, filter: &str) -> bool {
        if filter == DEFAULT_STREAM {
            self.is_direct()
        } else {
            self.directory().and_then(OsStr::to_str) == Some(filter)
        }
    }
}

/// Explicit on-disk chronicle-segment layout.
///
/// Distinguishes a day-child segment ([`Direct`](Self::Direct)) from a child of
/// a stream directory ([`Named`](Self::Named)), including a directory literally
/// named `_default`. This is lossless; [`RecordIdentity`] is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SegmentLayout {
    /// Segment directory sits directly under the chronicle day.
    Direct,
    /// Segment directory sits under a stream-directory basename.
    Named,
}

/// UTF-8 view of a segment for durable or wire records that already emit
/// `(stream, key)` with `_default` as the direct-layout spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordIdentity<'a> {
    /// `_default` for [`StreamLocation::Direct`], otherwise the UTF-8 stream directory.
    pub stream: &'a str,
    /// Parsed `HHMMSS_LEN` key (time metadata / existing record spelling).
    pub key: &'a str,
    /// Exact UTF-8 segment-directory basename.
    pub name: &'a str,
}

/// Lossless UTF-8 view of a discovered segment's on-disk layout.
///
/// [`Direct`](SegmentLayout::Direct) still spells [`stream`](Self::stream) as
/// [`DEFAULT_STREAM`]. A [`Named`](SegmentLayout::Named) directory whose UTF-8
/// name is `_default` is representable here; [`RecordIdentity`] refuses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentLocatorIdentity<'a> {
    /// Explicit on-disk layout. Never inferred from the stream spelling.
    pub layout: SegmentLayout,
    /// `_default` for [`SegmentLayout::Direct`], otherwise the UTF-8 stream directory.
    pub stream: &'a str,
    /// Parsed `HHMMSS_LEN` key (time metadata / existing record spelling).
    pub key: &'a str,
    /// Exact UTF-8 segment-directory basename.
    pub name: &'a str,
}

/// One discovered chronicle segment.
///
/// Fields are private so path, stream location, exact basename, and parsed key
/// stay correlated. The parsed key is time metadata, not the directory name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    stream: StreamLocation,
    name: OsString,
    key: String,
    path: PathBuf,
}

impl Segment {
    /// Stream layout of this segment.
    #[must_use]
    pub fn stream(&self) -> &StreamLocation {
        &self.stream
    }

    /// Exact native segment-directory basename.
    #[must_use]
    pub fn name(&self) -> &OsStr {
        &self.name
    }

    /// Parsed `HHMMSS_LEN` time metadata.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Discovered filesystem path. Every path decision goes through this.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Fallible UTF-8 identity for records that spell `_default` and `key`.
    ///
    /// `Direct` spells `stream` as [`DEFAULT_STREAM`]. A `Named` directory
    /// whose UTF-8 name is `_default` is [`SegmentIdentityError::AmbiguousNamedDefault`].
    /// Non-UTF-8 stream directories or basenames are [`SegmentIdentityError::NotUtf8`].
    pub fn record_identity(&self) -> Result<RecordIdentity<'_>, SegmentIdentityError> {
        let stream = match &self.stream {
            StreamLocation::Direct => DEFAULT_STREAM,
            StreamLocation::Named(name) => {
                let Some(name) = name.to_str() else {
                    return Err(SegmentIdentityError::NotUtf8 {
                        path: self.path.clone(),
                    });
                };
                if name == DEFAULT_STREAM {
                    return Err(SegmentIdentityError::AmbiguousNamedDefault {
                        path: self.path.clone(),
                    });
                }
                name
            }
        };
        let Some(name) = self.name.to_str() else {
            return Err(SegmentIdentityError::NotUtf8 {
                path: self.path.clone(),
            });
        };
        Ok(RecordIdentity {
            stream,
            key: &self.key,
            name,
        })
    }

    /// Fallible lossless UTF-8 identity with an explicit [`SegmentLayout`].
    ///
    /// `Direct` spells `stream` as [`DEFAULT_STREAM`]. A `Named` directory whose
    /// UTF-8 name is `_default` is representable. Non-UTF-8 stream directories
    /// or basenames are [`SegmentIdentityError::NotUtf8`].
    pub fn locator_identity(&self) -> Result<SegmentLocatorIdentity<'_>, SegmentIdentityError> {
        let (layout, stream) = match &self.stream {
            StreamLocation::Direct => (SegmentLayout::Direct, DEFAULT_STREAM),
            StreamLocation::Named(name) => {
                let Some(name) = name.to_str() else {
                    return Err(SegmentIdentityError::NotUtf8 {
                        path: self.path.clone(),
                    });
                };
                (SegmentLayout::Named, name)
            }
        };
        let Some(name) = self.name.to_str() else {
            return Err(SegmentIdentityError::NotUtf8 {
                path: self.path.clone(),
            });
        };
        Ok(SegmentLocatorIdentity {
            layout,
            stream,
            key: &self.key,
            name,
        })
    }
}

/// Produce a representable [`RecordIdentity`] for every selected segment.
///
/// Refuses on non-UTF-8 names and on the named-`_default` ambiguity.
pub fn utf8_identities<'a, I>(segments: I) -> Result<Vec<RecordIdentity<'a>>, SegmentIdentityError>
where
    I: IntoIterator<Item = &'a Segment>,
{
    segments.into_iter().map(Segment::record_identity).collect()
}

/// Require `(stream, key)` pairs in `identities` to be unique.
pub fn check_unique_record_keys(
    identities: &[RecordIdentity<'_>],
) -> Result<(), SegmentIdentityError> {
    let mut seen = HashMap::<(&str, &str), ()>::new();
    for identity in identities {
        if seen.insert((identity.stream, identity.key), ()).is_some() {
            return Err(SegmentIdentityError::DuplicateKey {
                stream: identity.stream.to_owned(),
                key: identity.key.to_owned(),
            });
        }
    }
    Ok(())
}

/// UTF-8 representability plus `(stream, key)` uniqueness.
pub fn check_record_identities<'a, I>(
    segments: I,
) -> Result<Vec<RecordIdentity<'a>>, SegmentIdentityError>
where
    I: IntoIterator<Item = &'a Segment>,
{
    let identities = utf8_identities(segments)?;
    check_unique_record_keys(&identities)?;
    Ok(identities)
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
    let normalized = normalize_rel(rel);
    if normalized.trim().is_empty() {
        return Err(invalid(rel, "journal path must not be empty"));
    }
    if Path::new(&normalized).is_absolute() {
        return Err(invalid(rel, "journal path must be relative"));
    }
    if normalized.contains('\\') {
        return Err(invalid(rel, "journal path must use forward slashes"));
    }
    if normalized
        .split('/')
        .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(invalid(rel, "journal path contains invalid component"));
    }
    let relative = Path::new(&normalized);
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

/// Drop trailing slashes, repeated separators, and `.` the way pathlib parts do.
fn normalize_rel(rel: &str) -> String {
    if rel.contains('\\') || Path::new(rel).is_absolute() {
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

/// Create a single directory component beneath an already-bound parent.
///
/// `EEXIST` succeeds when the name is already a real directory. A symlink or
/// other kind at that name is an error. This never opens a parent via `AT_FDCWD`.
#[cfg(unix)]
pub(crate) fn create_directory_bound(
    parent: &impl std::os::fd::AsFd,
    name: &OsStr,
    mode: u32,
) -> Result<(), PathError> {
    use nix::errno::Errno;
    use nix::fcntl::AtFlags;
    use nix::sys::stat::{Mode, SFlag, fstatat, mkdirat};

    let path = Path::new(name);
    if mode > 0o777 {
        return Err(path_io(
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "mode exceeds 0o777"),
        ));
    }
    match mkdirat(
        parent,
        name,
        Mode::from_bits_truncate(mode as nix::libc::mode_t),
    ) {
        Ok(()) => Ok(()),
        Err(Errno::EEXIST) => {
            let status = fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW)
                .map_err(|source| path_io(path, io::Error::from_raw_os_error(source as i32)))?;
            if SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT == SFlag::S_IFDIR {
                Ok(())
            } else {
                Err(path_io(
                    path,
                    io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "bound directory name exists and is not a directory",
                    ),
                ))
            }
        }
        Err(source) => Err(path_io(path, io::Error::from_raw_os_error(source as i32))),
    }
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
    Ok(list_dir_entries_bounded(dir, usize::MAX)?
        .expect("a directory cannot contain more than usize::MAX entries"))
}

/// List direct directory entries without retaining more than `maximum`.
///
/// `Ok(None)` means the directory contains more entries than the caller's
/// bound. Missing and non-directory paths remain `Ok(Some(Vec::new()))`.
pub fn list_dir_entries_bounded(
    dir: &Path,
    maximum: usize,
) -> Result<Option<Vec<DirEntry>>, PathError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Some(Vec::new()));
        }
        Err(source) => return Err(path_io(dir, source)),
    };

    let mut listed = Vec::new();
    for entry in entries {
        if listed.len() >= maximum {
            return Ok(None);
        }
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
    Ok(Some(listed))
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
        let entry_name = entry.file_name();
        if let Some(key) = segment_key_os(&entry_name).map(str::to_owned) {
            segments.push(Segment {
                stream: StreamLocation::Direct,
                name: entry_name,
                key,
                path: entry.path(),
            });
            continue;
        }
        if entry_name == OsStr::new("health") {
            continue;
        }
        for segment_entry in
            fs::read_dir(entry.path()).map_err(|source| path_io(&entry.path(), source))?
        {
            let segment_entry = segment_entry.map_err(|source| path_io(&entry.path(), source))?;
            let name = segment_entry.file_name();
            if segment_entry.path().is_dir()
                && let Some(key) = segment_key_os(&name).map(str::to_owned)
            {
                segments.push(Segment {
                    stream: StreamLocation::Named(entry_name.clone()),
                    name,
                    key,
                    path: segment_entry.path(),
                });
            }
        }
    }
    segments.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(segments)
}

/// Resolve a path through its longest existing prefix without creating it.
///
/// Existing components are canonicalized, including symlinks. Nonexistent
/// trailing components are retained lexically, so callers can safely compare
/// prospective paths against resolved roots.
pub fn realpath_non_strict(path: &Path) -> Result<PathBuf, PathError> {
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

/// True when `value` is a canonical chronicle day: exactly eight ASCII digits.
pub fn is_day_key(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn segment_key_os(name: &OsStr) -> Option<&str> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        segment_key_from_bytes(name.as_bytes())
    }
    #[cfg(not(unix))]
    {
        segment_key_from_bytes(name.to_str()?.as_bytes())
    }
}

fn segment_key_from_bytes(bytes: &[u8]) -> Option<&str> {
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
            return std::str::from_utf8(&bytes[index..end]).ok();
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

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;
    use crate::test_support::TempDir;

    fn invalid_kind(journal: &Path, rel: &str) -> &'static str {
        match resolve_journal_path(journal, rel) {
            Err(PathError::InvalidRelativePath { message, .. }) => message,
            other => panic!("{rel:?}: {other:?}"),
        }
    }

    #[test]
    fn normalises_trailing_slashes_dots_and_repeated_separators() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        assert_eq!(
            resolve_journal_path(&journal, "chronicle/").unwrap(),
            journal.join("chronicle")
        );
        assert_eq!(
            resolve_journal_path(&journal, "./facets/x").unwrap(),
            journal.join("facets/x")
        );
        assert_eq!(
            resolve_journal_path(&journal, "facets//x").unwrap(),
            journal.join("facets/x")
        );
        assert_eq!(
            resolve_journal_path(&journal, "20240101/").unwrap(),
            journal.join("chronicle/20240101")
        );
        assert_eq!(
            contained_path(&journal, "chronicle/").unwrap(),
            journal.join("chronicle")
        );
    }

    #[test]
    fn is_day_key_is_eight_ascii_digits() {
        assert!(is_day_key("20260823"));
        assert!(!is_day_key("2026823"));
        assert!(!is_day_key("2026082a"));
        assert!(!is_day_key("2026-08-23"));
    }

    #[test]
    fn create_directory_bound_creates_or_accepts_an_existing_directory() {
        use nix::fcntl::{AT_FDCWD, OFlag, openat};
        use nix::sys::stat::Mode;

        let temporary = TempDir::new();
        let parent_fd = openat(
            AT_FDCWD,
            temporary.path(),
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::empty(),
        )
        .unwrap();
        create_directory_bound(&parent_fd, std::ffi::OsStr::new("nested"), 0o700).unwrap();
        assert!(temporary.path().join("nested").is_dir());
        create_directory_bound(&parent_fd, std::ffi::OsStr::new("nested"), 0o700).unwrap();
        fs::write(temporary.path().join("file"), b"x").unwrap();
        assert!(create_directory_bound(&parent_fd, std::ffi::OsStr::new("file"), 0o700).is_err());
    }

    #[test]
    fn still_rejects_empty_dot_parent_absolute_and_backslash() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        assert!(matches!(
            resolve_journal_path(&journal, ""),
            Err(PathError::InvalidRelativePath { .. })
        ));
        assert_eq!(invalid_kind(&journal, ""), invalid_kind(&journal, "   "));
        assert_eq!(invalid_kind(&journal, "."), invalid_kind(&journal, ".."));
        assert_eq!(invalid_kind(&journal, "./"), invalid_kind(&journal, ".."));
        assert_eq!(invalid_kind(&journal, "//"), invalid_kind(&journal, "/abs"));
        assert!(matches!(
            resolve_journal_path(&journal, "a\\b"),
            Err(PathError::InvalidRelativePath { .. })
        ));
        match resolve_journal_path(&journal, "chronicle/../x") {
            Err(PathError::InvalidRelativePath { rel, .. }) => {
                assert_eq!(rel, "chronicle/../x");
            }
            other => panic!("{other:?}"),
        }
    }

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
        assert_eq!(list_dir_entries_bounded(&directory, 1).unwrap(), None);
        assert_eq!(
            list_dir_entries_bounded(&directory, 2).unwrap(),
            Some(list_dir_entries(&directory).unwrap())
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
                    stream: StreamLocation::Direct,
                    name: "080000_300".into(),
                    key: "080000_300".to_owned(),
                    path: day.join("080000_300")
                },
                Segment {
                    stream: StreamLocation::Named("other".into()),
                    name: "093000_300_summary".into(),
                    key: "093000_300".to_owned(),
                    path: day.join("other/093000_300_summary")
                },
                Segment {
                    stream: StreamLocation::Named("other".into()),
                    name: "123456_300".into(),
                    key: "123456_300".to_owned(),
                    path: segment
                },
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn enumerates_distinct_non_utf8_stream_directories() {
        use std::os::unix::ffi::OsStrExt;

        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let day = journal.join("chronicle/20260101");
        fs::create_dir_all(day.join(OsStr::from_bytes(b"s\xff")).join("080000_300")).unwrap();
        fs::create_dir_all(day.join(OsStr::from_bytes(b"s\xfe")).join("090000_300")).unwrap();

        let segments = iter_segments(&journal, PathOrDay::Day("20260101")).unwrap();
        assert_eq!(segments.len(), 2);
        assert_ne!(
            segments[0].stream().directory(),
            segments[1].stream().directory()
        );
        assert!(segments.iter().all(|segment| !segment.stream().is_direct()));
        assert!(
            segments
                .iter()
                .all(|segment| segment.record_identity().is_err())
        );
        assert!(
            utf8_identities(&segments).is_err(),
            "{:?}",
            utf8_identities(&segments)
        );
        for segment in &segments {
            assert_eq!(
                segment.path(),
                day.join(segment.stream().directory().unwrap())
                    .join(segment.name())
            );
        }
    }

    #[test]
    fn named_default_directory_is_distinct_from_direct_layout() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let day = journal.join("chronicle/20260101");
        fs::create_dir_all(day.join("080000_300")).unwrap();
        fs::create_dir_all(day.join("_default/090000_300")).unwrap();

        let segments = iter_segments(&journal, PathOrDay::Day("20260101")).unwrap();
        assert_eq!(segments.len(), 2);
        let direct = segments
            .iter()
            .find(|segment| segment.name() == OsStr::new("080000_300"))
            .unwrap();
        let named = segments
            .iter()
            .find(|segment| segment.name() == OsStr::new("090000_300"))
            .unwrap();
        assert!(direct.stream().is_direct());
        assert_eq!(named.stream().directory(), Some(OsStr::new(DEFAULT_STREAM)));
        assert!(direct.stream().matches(DEFAULT_STREAM));
        assert!(!named.stream().matches(DEFAULT_STREAM));
        assert_ne!(direct.path(), named.path());
        let identity = direct.record_identity().unwrap();
        assert_eq!(identity.stream, DEFAULT_STREAM);
        assert_eq!(identity.key, "080000_300");
        assert_eq!(
            named.record_identity(),
            Err(SegmentIdentityError::AmbiguousNamedDefault {
                path: named.path().to_path_buf(),
            })
        );
        assert!(
            utf8_identities(&segments).is_err(),
            "{:?}",
            utf8_identities(&segments)
        );

        let lone = journal.join("chronicle/20260102");
        fs::create_dir_all(lone.join("_default/090000_300")).unwrap();
        let lone_segments = iter_segments(&journal, PathOrDay::Day("20260102")).unwrap();
        assert_eq!(lone_segments.len(), 1);
        assert_eq!(
            lone_segments[0].record_identity(),
            Err(SegmentIdentityError::AmbiguousNamedDefault {
                path: lone_segments[0].path().to_path_buf(),
            })
        );
        assert!(utf8_identities(&lone_segments).is_err());
    }

    #[test]
    fn locator_identity_is_lossless_and_leaves_record_identity_unchanged() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let day = journal.join("chronicle/20260101");
        fs::create_dir_all(day.join("080000_300")).unwrap();
        fs::create_dir_all(day.join("_default/090000_300")).unwrap();
        fs::create_dir_all(day.join("main/093000_300_summary")).unwrap();

        let segments = iter_segments(&journal, PathOrDay::Day("20260101")).unwrap();
        assert_eq!(segments.len(), 3);
        let direct = segments
            .iter()
            .find(|segment| segment.stream().is_direct())
            .unwrap();
        let named_default = segments
            .iter()
            .find(|segment| segment.stream().directory() == Some(OsStr::new(DEFAULT_STREAM)))
            .unwrap();
        let named_main = segments
            .iter()
            .find(|segment| segment.stream().directory() == Some(OsStr::new("main")))
            .unwrap();

        let direct_id = direct.locator_identity().unwrap();
        assert_eq!(direct_id.layout, SegmentLayout::Direct);
        assert_eq!(direct_id.stream, DEFAULT_STREAM);
        assert_eq!(direct_id.key, "080000_300");
        assert_eq!(direct_id.name, "080000_300");

        let named_default_id = named_default.locator_identity().unwrap();
        assert_eq!(named_default_id.layout, SegmentLayout::Named);
        assert_eq!(named_default_id.stream, DEFAULT_STREAM);
        assert_eq!(named_default_id.key, "090000_300");
        assert_eq!(named_default_id.name, "090000_300");

        let named_main_id = named_main.locator_identity().unwrap();
        assert_eq!(named_main_id.layout, SegmentLayout::Named);
        assert_eq!(named_main_id.stream, "main");
        assert_eq!(named_main_id.key, "093000_300");
        assert_eq!(named_main_id.name, "093000_300_summary");

        let record = direct.record_identity().unwrap();
        assert_eq!(record.stream, DEFAULT_STREAM);
        assert_eq!(record.key, "080000_300");
        assert_eq!(record.name, "080000_300");
        assert_eq!(
            named_default.record_identity(),
            Err(SegmentIdentityError::AmbiguousNamedDefault {
                path: named_default.path().to_path_buf(),
            })
        );
        let main_record = named_main.record_identity().unwrap();
        assert_eq!(main_record.stream, "main");
        assert_eq!(main_record.key, "093000_300");
        assert_eq!(main_record.name, "093000_300_summary");
    }

    #[test]
    fn segment_layout_serde_round_trips_lowercase_and_refuses_unknown_or_case_varied() {
        assert_eq!(
            serde_json::to_string(&SegmentLayout::Direct).unwrap(),
            "\"direct\""
        );
        assert_eq!(
            serde_json::to_string(&SegmentLayout::Named).unwrap(),
            "\"named\""
        );
        assert_eq!(
            serde_json::from_str::<SegmentLayout>("\"direct\"").unwrap(),
            SegmentLayout::Direct
        );
        assert_eq!(
            serde_json::from_str::<SegmentLayout>("\"named\"").unwrap(),
            SegmentLayout::Named
        );
        for invalid in ["\"foo\"", "\"Direct\"", "\"DIRECT\"", "\"Named\""] {
            assert!(
                serde_json::from_str::<SegmentLayout>(invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_named_default_shaped_bytes_are_not_utf8() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let path = PathBuf::from("chronicle/20260101").join(OsStr::from_bytes(b"_default\xff"));
        let segment = Segment {
            stream: StreamLocation::Named(OsString::from_vec(b"_default\xff".to_vec())),
            name: "090000_300".into(),
            key: "090000_300".to_owned(),
            path: path.clone(),
        };
        assert_eq!(
            segment.record_identity(),
            Err(SegmentIdentityError::NotUtf8 { path })
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn locator_identity_refuses_non_utf8_stream_or_basename() {
        use std::os::unix::ffi::OsStrExt;

        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let day = journal.join("chronicle/20260101");
        fs::create_dir_all(day.join(OsStr::from_bytes(b"s\xff")).join("080000_300")).unwrap();
        fs::create_dir_all(day.join(OsStr::from_bytes(b"090000_300\xff"))).unwrap();

        let segments = iter_segments(&journal, PathOrDay::Day("20260101")).unwrap();
        assert_eq!(segments.len(), 2);
        let named = segments
            .iter()
            .find(|segment| !segment.stream().is_direct())
            .unwrap();
        let direct = segments
            .iter()
            .find(|segment| segment.stream().is_direct())
            .unwrap();
        assert_eq!(
            named.locator_identity(),
            Err(SegmentIdentityError::NotUtf8 {
                path: named.path().to_path_buf(),
            })
        );
        assert_eq!(
            direct.locator_identity(),
            Err(SegmentIdentityError::NotUtf8 {
                path: direct.path().to_path_buf(),
            })
        );
    }

    #[test]
    fn same_key_siblings_keep_distinct_basenames() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let stream = journal.join("chronicle/20260101/other");
        fs::create_dir_all(stream.join("093000_300_a")).unwrap();
        fs::create_dir_all(stream.join("093000_300_b")).unwrap();

        let segments = iter_segments(&journal, PathOrDay::Day("20260101")).unwrap();
        assert_eq!(segments.len(), 2);
        assert!(segments.iter().all(|segment| segment.key() == "093000_300"));
        let names: Vec<_> = segments
            .iter()
            .map(|segment| segment.name().to_os_string())
            .collect();
        assert!(names.contains(&OsString::from("093000_300_a")));
        assert!(names.contains(&OsString::from("093000_300_b")));
        let identities = utf8_identities(&segments).unwrap();
        let identity_names: Vec<_> = identities.iter().map(|identity| identity.name).collect();
        assert!(identity_names.contains(&"093000_300_a"));
        assert!(identity_names.contains(&"093000_300_b"));
        assert!(check_unique_record_keys(&identities).is_err());
    }
}
