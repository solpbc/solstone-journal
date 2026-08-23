// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Crash-safe whole-file writers.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use nix::errno::Errno;
use nix::fcntl::{AT_FDCWD, AtFlags, OFlag, openat, renameat};
use nix::sys::stat::{Mode, SFlag, fstat, fstatat};
use nix::unistd::{UnlinkatFlags, unlinkat};

use crate::errors::AtomicWriteError;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Successful publication states returned by [`atomic_replace_detailed`].
#[derive(Debug)]
pub enum DetailedAtomicOutcome {
    /// The caller-visible path was reverified and its directory was synced.
    Published,
    /// Bytes were published, but syncing the bound directory failed.
    PublishedDurabilityUncertain { source: io::Error },
    /// Bytes were published in the inspected directory, but its pathname changed.
    PublishedParentPathRaced { sync_error: Option<io::Error> },
    /// Bytes were published, but the final pathname observation itself failed.
    PublishedParentPathUnverified {
        observation: io::Error,
        sync_error: Option<io::Error>,
    },
}

/// A failure before publication. The prior destination is preserved.
#[derive(Debug)]
pub struct DetailedAtomicError {
    pub path: PathBuf,
    pub operation: &'static str,
    pub source: io::Error,
    /// A stage is named only when cleanup also failed.
    pub orphan_stage: Option<OsString>,
    pub cleanup_error: Option<io::Error>,
}

impl std::fmt::Display for DetailedAtomicError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}: {}: {}",
            self.path.display(),
            self.operation,
            self.source
        )?;
        if let (Some(stage), Some(cleanup)) = (&self.orphan_stage, &self.cleanup_error) {
            write!(formatter, "; could not remove stage {stage:?}: {cleanup}")?;
        }
        Ok(())
    }
}

impl std::error::Error for DetailedAtomicError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Options shared by the byte-oriented whole-file writers.
#[derive(Debug, Clone, Copy, Default)]
pub struct AtomicWriteOptions {
    /// Final file mode, applied before the rename or hard-link publication.
    pub mode: Option<u32>,
}

/// JSON formatting and publication options.
#[derive(Debug, Clone, Copy)]
pub struct JsonWriteOptions {
    /// Final file mode, applied before publication.
    pub mode: Option<u32>,
    /// Pretty-print indentation width. `None` emits compact JSON.
    pub indent: Option<usize>,
    /// Whether object keys are recursively sorted before serialization.
    pub sort_keys: bool,
}

impl Default for JsonWriteOptions {
    fn default() -> Self {
        Self {
            mode: None,
            indent: Some(2),
            sort_keys: false,
        }
    }
}

/// Atomically replace a regular destination beneath an existing real parent.
///
/// The entire operation is bound to the inspected directory descriptor. This
/// function never creates a parent and never follows a parent or destination
/// symlink. Callers must serialize all writers for `path` with the stable lock.
#[cfg(unix)]
pub fn atomic_replace_detailed(
    path: &Path,
    contents: &[u8],
    mode: u32,
) -> Result<DetailedAtomicOutcome, DetailedAtomicError> {
    if mode > 0o777 {
        return Err(detailed_error(
            path,
            "validate mode",
            io::Error::new(io::ErrorKind::InvalidInput, "mode exceeds 0o777"),
        ));
    }
    let parent = path
        .parent()
        .filter(|item| !item.as_os_str().is_empty())
        .ok_or_else(|| {
            detailed_error(
                path,
                "validate destination",
                io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"),
            )
        })?;
    let name = normal_name(path.file_name()).ok_or_else(|| {
        detailed_error(
            path,
            "validate destination",
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination has no normal name",
            ),
        )
    })?;
    let inspected =
        stat_parent(parent).map_err(|source| detailed_error(path, "inspect parent", source))?;
    let directory = openat(
        AT_FDCWD,
        parent,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|source| detailed_errno(path, "open parent", source))?;
    let opened = fstat(&directory)
        .map_err(|source| detailed_errno(path, "inspect opened parent", source))?;
    if !same_identity(inspected, opened) {
        return Err(detailed_error(
            path,
            "bind parent",
            io::Error::other("parent pathname changed"),
        ));
    }
    inspect_destination(&directory, name, path)?;

    let stage = allocate_bound_stage(&directory, name, path)?;
    let stage_name = stage.0;
    let mut stage_file = stage.1;
    let operation = (|| -> io::Result<()> {
        stage_file.write_all(contents)?;
        stage_file.set_permissions(fs::Permissions::from_mode(mode))?;
        sync_file(&stage_file)?;
        Ok(())
    })();
    drop(stage_file);
    if let Err(source) = operation {
        return Err(cleanup_stage_error(
            &directory,
            path,
            stage_name,
            "prepare stage",
            source,
        ));
    }

    let before_rename = match stat_parent(parent) {
        Ok(status) if same_identity(inspected, status) => Ok(()),
        Ok(_) => Err(io::Error::other(
            "parent pathname changed before publication",
        )),
        Err(source) => Err(source),
    };
    if let Err(source) = before_rename {
        return Err(cleanup_stage_error(
            &directory,
            path,
            stage_name,
            "reverify parent before publication",
            source,
        ));
    }

    renameat(&directory, stage_name.as_os_str(), &directory, name).map_err(|source| {
        cleanup_stage_error(
            &directory,
            path,
            stage_name.clone(),
            "publish stage",
            errno_io(source),
        )
    })?;

    let final_observation = stat_parent(parent);
    let sync_error = directory.sync_all().err();
    match final_observation {
        Ok(status) if same_identity(inspected, status) => match sync_error {
            None => Ok(DetailedAtomicOutcome::Published),
            Some(source) => Ok(DetailedAtomicOutcome::PublishedDurabilityUncertain { source }),
        },
        Ok(_) => Ok(DetailedAtomicOutcome::PublishedParentPathRaced { sync_error }),
        Err(observation) => Ok(DetailedAtomicOutcome::PublishedParentPathUnverified {
            observation,
            sync_error,
        }),
    }
}

#[cfg(not(unix))]
pub fn atomic_replace_detailed(
    path: &Path,
    _contents: &[u8],
    _mode: u32,
) -> Result<DetailedAtomicOutcome, DetailedAtomicError> {
    Err(detailed_error(
        path,
        "publish",
        io::Error::new(
            io::ErrorKind::Unsupported,
            "detailed publication requires Unix",
        ),
    ))
}

/// Atomically replace `path` with durably prepared `contents`.
///
/// The replacement bytes are synced before rename. On Apple targets, that sync
/// also performs `F_FULLFSYNC`; a full-flush failure aborts publication and the
/// temporary file is removed. The containing directory is synced afterwards on
/// a best-effort basis; a directory-sync failure is logged and does not turn an
/// otherwise published replacement into an error.
pub fn atomic_replace(
    path: impl AsRef<Path>,
    contents: &[u8],
    options: AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    let path = path.as_ref();
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;

    let (temporary_path, mut temporary_file) = create_temporary(parent, path)?;
    pause_at("temp-create");
    let operation = (|| {
        temporary_file
            .write_all(contents)
            .map_err(|source| io_error(path, source))?;
        pause_at("write");
        sync_file(&temporary_file).map_err(|source| io_error(path, source))?;
        pause_at("fsync-file");
        if let Some(mode) = options.mode {
            apply_mode(&temporary_file, mode).map_err(|source| io_error(path, source))?;
        }
        pause_at("chmod");
        drop(temporary_file);
        pause_at("close");
        fs::rename(&temporary_path, path).map_err(|source| io_error(path, source))?;
        pause_at("rename");
        fsync_dir(parent);
        pause_at("fsync-parent-dir");
        Ok(())
    })();

    if operation.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    operation
}

/// Publish `contents` only when `path` does not yet exist.
///
/// Contents are written and synced to an unlinked temporary inode first, then
/// published with `link(2)`. Consequently the destination name is never visible
/// with partial content.
pub fn write_bytes_exclusive(
    path: impl AsRef<Path>,
    contents: &[u8],
    options: AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    let path = path.as_ref();
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;

    let (temporary_path, mut temporary_file) = create_temporary(parent, path)?;
    let operation = (|| {
        temporary_file
            .write_all(contents)
            .map_err(|source| io_error(path, source))?;
        sync_file(&temporary_file).map_err(|source| io_error(path, source))?;
        if let Some(mode) = options.mode {
            apply_mode(&temporary_file, mode).map_err(|source| io_error(path, source))?;
        }
        drop(temporary_file);
        fs::hard_link(&temporary_path, path).map_err(|source| io_error(path, source))?;
        fs::remove_file(&temporary_path).map_err(|source| io_error(path, source))?;
        fsync_dir(parent);
        Ok(())
    })();

    if operation.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    operation
}

/// Create and durably fill a new file from a bounded-memory reader.
///
/// The destination is create-only, receives the requested final mode before
/// publication is reported, and is synced before this function returns. A
/// failed copy removes the incomplete destination.
pub fn write_reader_exclusive(
    path: &Path,
    reader: &mut impl Read,
    options: AtomicWriteOptions,
) -> Result<u64, AtomicWriteError> {
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;
    let (temporary_path, mut temporary_file) = create_temporary(parent, path)?;
    let operation = (|| {
        let bytes =
            io::copy(reader, &mut temporary_file).map_err(|source| io_error(path, source))?;
        sync_file(&temporary_file).map_err(|source| io_error(path, source))?;
        if let Some(mode) = options.mode {
            apply_mode(&temporary_file, mode).map_err(|source| io_error(path, source))?;
        }
        drop(temporary_file);
        fs::hard_link(&temporary_path, path).map_err(|source| io_error(path, source))?;
        fs::remove_file(&temporary_path).map_err(|source| io_error(path, source))?;
        fsync_dir(parent);
        Ok(bytes)
    })();
    if operation.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    operation
}

/// Publish an already-created temporary file by syncing and atomically renaming it.
pub fn install_file(
    temporary_path: impl AsRef<Path>,
    path: impl AsRef<Path>,
    options: AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    let temporary_path = temporary_path.as_ref();
    let path = path.as_ref();
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;
    let temporary_file = File::open(temporary_path).map_err(|source| io_error(path, source))?;
    let operation = (|| {
        sync_file(&temporary_file).map_err(|source| io_error(path, source))?;
        if let Some(mode) = options.mode {
            apply_mode(&temporary_file, mode).map_err(|source| io_error(path, source))?;
        }
        drop(temporary_file);
        fs::rename(temporary_path, path).map_err(|source| io_error(path, source))?;
        fsync_dir(parent);
        Ok(())
    })();
    if operation.is_err() {
        let _ = fs::remove_file(temporary_path);
    }
    operation
}

/// Serialize and atomically replace a JSON file.
pub fn write_json<T: Serialize>(
    path: impl AsRef<Path>,
    value: &T,
    options: JsonWriteOptions,
) -> Result<(), AtomicWriteError> {
    let mut value =
        serde_json::to_value(value).map_err(|source| serialization_error(path.as_ref(), source))?;
    if options.sort_keys {
        sort_json_keys(&mut value);
    }
    let mut contents = serialize_json(&value, options.indent)
        .map_err(|source| serialization_error(path.as_ref(), source))?;
    contents.push(b'\n');
    atomic_replace(path, &contents, AtomicWriteOptions { mode: options.mode })
}

/// Atomically replace a UTF-8 text file.
pub fn write_text(
    path: impl AsRef<Path>,
    text: &str,
    options: AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    atomic_replace(path, text.as_bytes(), options)
}

/// Atomically replace a JSONL file with one record per line.
pub fn write_jsonl<T: Serialize>(
    path: impl AsRef<Path>,
    records: impl IntoIterator<Item = T>,
    options: AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    let path = path.as_ref();
    let mut contents = Vec::new();
    for record in records {
        serde_json::to_writer(&mut contents, &record)
            .map_err(|source| serialization_error(path, source))?;
        contents.push(b'\n');
    }
    atomic_replace(path, &contents, options)
}

/// Sync a file before its name is published.
///
/// All sync failures are hard errors. On Apple targets this performs both
/// `sync_all()` and `F_FULLFSYNC`; an `F_FULLFSYNC` failure propagates to the
/// caller before rename or hard-link publication. This is distinct from the
/// best-effort parent-directory sync performed after publication.
pub(crate) fn sync_file(file: &File) -> io::Result<()> {
    file.sync_all()?;
    #[cfg(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "tvos",
        target_os = "visionos",
        target_os = "watchos"
    ))]
    {
        nix::fcntl::fcntl(file, nix::fcntl::FcntlArg::F_FULLFSYNC)
            .map_err(|error| io::Error::from_raw_os_error(error as i32))?;
    }
    Ok(())
}

pub(crate) fn fsync_dir(path: &Path) {
    if let Err(error) = File::open(path).and_then(|directory| directory.sync_all()) {
        log::warn!(
            "parent-directory fsync degraded for {}: {error}",
            path.display()
        );
    }
}

fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(unix)]
fn normal_name(value: Option<&OsStr>) -> Option<&OsStr> {
    let value = value?;
    let mut components = Path::new(value).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(component)), None) if component == value => Some(value),
        _ => None,
    }
}

#[cfg(unix)]
fn stat_parent(parent: &Path) -> io::Result<nix::sys::stat::FileStat> {
    let status = fstatat(AT_FDCWD, parent, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(errno_io)?;
    if SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT == SFlag::S_IFDIR {
        Ok(status)
    } else {
        Err(io::Error::other("parent is not a real directory"))
    }
}

#[cfg(unix)]
fn same_identity(left: nix::sys::stat::FileStat, right: nix::sys::stat::FileStat) -> bool {
    left.st_dev == right.st_dev && left.st_ino == right.st_ino
}

#[cfg(unix)]
fn inspect_destination(
    directory: &impl AsFd,
    name: &OsStr,
    path: &Path,
) -> Result<(), DetailedAtomicError> {
    match fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(status)
            if SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT == SFlag::S_IFREG =>
        {
            Ok(())
        }
        Ok(_) => Err(detailed_error(
            path,
            "inspect destination",
            io::Error::other("destination is not a regular file"),
        )),
        Err(Errno::ENOENT) => Ok(()),
        Err(source) => Err(detailed_errno(path, "inspect destination", source)),
    }
}

pub(crate) const ATOMIC_CANDIDATE_MARKER: &str = "tmp";
pub(crate) const STAGED_CANDIDATE_MARKER: &str = "stage";
pub(crate) const CANDIDATE_SUFFIX: &str = ".tmp";

pub(crate) fn publication_candidate_name(
    destination_name: &OsStr,
    marker: &str,
    entropy: &[u128],
) -> OsString {
    let mut name = OsString::new();
    if destination_name.as_encoded_bytes().first() == Some(&b'.') {
        name.push("_");
    } else {
        name.push(".");
    }
    name.push(marker);
    for value in entropy {
        name.push("_");
        name.push(value.to_string());
    }
    name.push(CANDIDATE_SUFFIX);
    name
}

pub(crate) fn candidate_for_attempt(
    attempt: usize,
    forced: &[OsString],
    build: impl FnOnce() -> OsString,
) -> OsString {
    match forced.get(attempt) {
        Some(name) => name.clone(),
        None => build(),
    }
}

#[cfg(unix)]
fn allocate_bound_stage(
    directory: &impl AsFd,
    destination: &OsStr,
    path: &Path,
) -> Result<(OsString, File), DetailedAtomicError> {
    allocate_bound_stage_inner(directory, destination, path, &[])
}

#[cfg(all(unix, test))]
fn allocate_bound_stage_forced(
    directory: &impl AsFd,
    destination: &OsStr,
    path: &Path,
    forced: &[OsString],
) -> Result<(OsString, File), DetailedAtomicError> {
    allocate_bound_stage_inner(directory, destination, path, forced)
}

#[cfg(unix)]
fn allocate_bound_stage_inner(
    directory: &impl AsFd,
    destination: &OsStr,
    path: &Path,
    forced: &[OsString],
) -> Result<(OsString, File), DetailedAtomicError> {
    for attempt in 0..100 {
        let candidate = candidate_for_attempt(attempt, forced, || {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            publication_candidate_name(
                destination,
                ATOMIC_CANDIDATE_MARKER,
                &[u128::from(std::process::id()), u128::from(sequence)],
            )
        });
        match openat(
            directory,
            candidate.as_os_str(),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::from_bits_truncate(0o600),
        ) {
            Ok(fd) => return Ok((candidate, File::from(fd))),
            Err(Errno::EEXIST) => continue,
            Err(source) => return Err(detailed_errno(path, "create stage", source)),
        }
    }
    Err(detailed_error(
        path,
        "create stage",
        io::Error::new(io::ErrorKind::AlreadyExists, "could not allocate stage"),
    ))
}

#[cfg(unix)]
fn cleanup_stage_error(
    directory: &impl AsFd,
    path: &Path,
    stage: OsString,
    operation: &'static str,
    source: io::Error,
) -> DetailedAtomicError {
    let cleanup = unlinkat(directory, stage.as_os_str(), UnlinkatFlags::NoRemoveDir)
        .err()
        .map(errno_io);
    DetailedAtomicError {
        path: path.to_path_buf(),
        operation,
        source,
        orphan_stage: cleanup.as_ref().map(|_| stage),
        cleanup_error: cleanup,
    }
}

fn detailed_error(path: &Path, operation: &'static str, source: io::Error) -> DetailedAtomicError {
    DetailedAtomicError {
        path: path.to_path_buf(),
        operation,
        source,
        orphan_stage: None,
        cleanup_error: None,
    }
}

#[cfg(unix)]
fn detailed_errno(path: &Path, operation: &'static str, source: Errno) -> DetailedAtomicError {
    detailed_error(path, operation, errno_io(source))
}

#[cfg(unix)]
fn errno_io(source: Errno) -> io::Error {
    io::Error::from_raw_os_error(source as i32)
}

fn create_temporary(
    parent: &Path,
    destination: &Path,
) -> Result<(PathBuf, File), AtomicWriteError> {
    create_temporary_inner(parent, destination, &[])
}

#[cfg(test)]
fn create_temporary_forced(
    parent: &Path,
    destination: &Path,
    forced: &[OsString],
) -> Result<(PathBuf, File), AtomicWriteError> {
    create_temporary_inner(parent, destination, forced)
}

fn create_temporary_inner(
    parent: &Path,
    destination: &Path,
    forced: &[OsString],
) -> Result<(PathBuf, File), AtomicWriteError> {
    let destination_name = destination.file_name().unwrap_or(OsStr::new(""));
    for attempt in 0..100 {
        let candidate = parent.join(candidate_for_attempt(attempt, forced, || {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            publication_candidate_name(
                destination_name,
                ATOMIC_CANDIDATE_MARKER,
                &[u128::from(std::process::id()), nanos, u128::from(sequence)],
            )
        }));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(io_error(destination, source)),
        }
    }
    Err(io_error(
        destination,
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate unique temporary file",
        ),
    ))
}

#[cfg(unix)]
fn apply_mode(file: &File, mode: u32) -> io::Result<()> {
    file.set_permissions(fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn apply_mode(_file: &File, _mode: u32) -> io::Result<()> {
    Ok(())
}

fn io_error(path: &Path, source: io::Error) -> AtomicWriteError {
    AtomicWriteError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn serialization_error(path: &Path, source: serde_json::Error) -> AtomicWriteError {
    io_error(path, io::Error::new(io::ErrorKind::InvalidData, source))
}

fn serialize_json(value: &serde_json::Value, indent: Option<usize>) -> serde_json::Result<Vec<u8>> {
    match indent {
        Some(width) => {
            let mut contents = Vec::new();
            let indent = vec![b' '; width];
            let formatter = serde_json::ser::PrettyFormatter::with_indent(&indent);
            let mut serializer = serde_json::Serializer::with_formatter(&mut contents, formatter);
            value.serialize(&mut serializer)?;
            Ok(contents)
        }
        None => serde_json::to_vec(value),
    }
}

fn sort_json_keys(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            let mut sorted = std::collections::BTreeMap::new();
            for (key, mut child) in std::mem::take(object) {
                sort_json_keys(&mut child);
                sorted.insert(key, child);
            }
            *object = sorted.into_iter().collect();
        }
        serde_json::Value::Array(values) => {
            for child in values {
                sort_json_keys(child);
            }
        }
        _ => {}
    }
}

// Gated on `test-hooks` as well as `test` so a dependent that enables the feature
// can inject a crash on this path. `staged.rs` already honours both; this half was
// `cfg(test)`-only, which left the hook unreachable outside this crate even for a
// dependent that asked for it by name (`solstone-core-sol-link`).
#[cfg(any(test, feature = "test-hooks"))]
fn pause_at(step: &str) {
    if std::env::var("JOURNAL_IO_TEST_PAUSE_AT").ok().as_deref() != Some(step) {
        return;
    }
    if let Ok(marker) = std::env::var("JOURNAL_IO_TEST_MARKER") {
        let _ = fs::write(marker, step);
    }
    loop {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn pause_at(_step: &str) {}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn detailed_replace_publishes_with_exact_mode_without_creating_parent() {
        let temporary = TempDir::new();
        let missing_parent = temporary.path().join("missing");
        let missing = missing_parent.join("unit.service");
        assert!(atomic_replace_detailed(&missing, b"new", 0o644).is_err());
        assert!(!missing_parent.exists());

        let target = temporary.path().join("unit.service");
        fs::write(&target, b"old").unwrap();
        let result = atomic_replace_detailed(&target, b"new", 0o640).unwrap();
        assert!(matches!(result, DetailedAtomicOutcome::Published));
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(fs::read_dir(temporary.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tmp_")
        }));
    }

    #[test]
    fn detailed_replace_refuses_unsafe_destination_and_mode() {
        let temporary = TempDir::new();
        let target = temporary.path().join("unit.service");
        let outside = temporary.path().join("outside");
        fs::write(&outside, b"outside").unwrap();
        std::os::unix::fs::symlink(&outside, &target).unwrap();
        assert!(atomic_replace_detailed(&target, b"new", 0o644).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"outside");
        assert!(atomic_replace_detailed(&target, b"new", 0o1000).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"outside");
    }

    #[test]
    fn reader_exclusive_is_create_only_and_copies_the_full_stream() {
        let temporary = TempDir::new();
        let path = temporary.path().join("reader.bin");
        let mut reader = io::Cursor::new(vec![b'x'; 131_073]);
        let copied =
            write_reader_exclusive(&path, &mut reader, AtomicWriteOptions { mode: Some(0o600) })
                .unwrap();
        assert_eq!(copied, 131_073);
        assert_eq!(fs::read(&path).unwrap(), vec![b'x'; 131_073]);
        assert!(
            write_reader_exclusive(
                &path,
                &mut io::Cursor::new(b"replacement"),
                AtomicWriteOptions::default(),
            )
            .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), vec![b'x'; 131_073]);
    }
    use crate::test_support::TempDir;

    #[test]
    fn write_bytes_exclusive_publishes_only_a_complete_temp_inode() {
        let temporary = TempDir::new();
        let target = temporary.path().join("record.bin");
        let payload = vec![b'x'; 1024 * 1024];

        write_bytes_exclusive(&target, &payload, AtomicWriteOptions::default()).unwrap();

        assert_eq!(fs::read(&target).unwrap(), payload);
        assert!(fs::read_dir(temporary.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tmp_")
        }));
        assert!(write_bytes_exclusive(&target, b"other", AtomicWriteOptions::default()).is_err());
        assert_eq!(fs::read(&target).unwrap().len(), 1024 * 1024);
    }

    use std::collections::BTreeSet;
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::OsStringExt;
    use std::path::Path;

    fn name_255() -> OsString {
        OsString::from("a".repeat(255))
    }

    fn os_from_bytes(bytes: &[u8]) -> OsString {
        OsString::from_vec(bytes.to_vec())
    }

    fn dir_names(dir: &Path) -> BTreeSet<OsString> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect()
    }

    fn parse_candidate_entropy(name: &OsStr, marker: &str) -> Vec<u128> {
        let text = name.to_str().expect("candidate is ASCII");
        let rest = text
            .strip_prefix('.')
            .or_else(|| text.strip_prefix('_'))
            .expect("sentinel");
        let rest = rest
            .strip_prefix(marker)
            .and_then(|rest| rest.strip_prefix('_'))
            .expect("marker");
        let rest = rest.strip_suffix(CANDIDATE_SUFFIX).expect("suffix");
        rest.split('_')
            .map(|part| part.parse().expect("entropy digit"))
            .collect()
    }

    fn assert_live_candidate(
        destination_name: &OsStr,
        candidate: &OsStr,
        marker: &str,
        fields: usize,
    ) {
        let entropy = parse_candidate_entropy(candidate, marker);
        assert_eq!(entropy.len(), fields);
        assert_eq!(entropy[0], u128::from(std::process::id()));
        assert_eq!(
            candidate,
            publication_candidate_name(destination_name, marker, &entropy)
        );
    }

    fn forced_names(destination_name: &OsStr, marker: &str, count: usize) -> Vec<OsString> {
        (0..count)
            .map(|index| publication_candidate_name(destination_name, marker, &[index as u128]))
            .collect()
    }

    fn assert_candidate_bounded(destination_name: &OsStr, marker: &str) {
        let candidate = publication_candidate_name(
            destination_name,
            marker,
            &[u128::from(u32::MAX), u128::MAX, u128::from(u64::MAX)],
        );
        assert!(
            candidate.as_encoded_bytes().len() < 100,
            "candidate {} bytes",
            candidate.as_encoded_bytes().len()
        );
    }

    #[test]
    fn filesystem_accepts_255_byte_file_names() {
        let temporary = TempDir::new();
        let path = temporary.path().join(name_255());
        fs::write(&path, b"ok").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"ok");
    }

    #[test]
    fn write_bytes_exclusive_publishes_255_byte_basename() {
        let temporary = TempDir::new();
        let path = temporary.path().join(name_255());
        assert_candidate_bounded(path.file_name().unwrap(), ATOMIC_CANDIDATE_MARKER);
        write_bytes_exclusive(&path, b"payload", AtomicWriteOptions::default()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"payload");
    }

    #[test]
    fn detailed_replace_publishes_255_byte_basename() {
        let temporary = TempDir::new();
        let path = temporary.path().join(name_255());
        assert_candidate_bounded(path.file_name().unwrap(), ATOMIC_CANDIDATE_MARKER);
        let result = atomic_replace_detailed(&path, b"payload", 0o644).unwrap();
        assert!(matches!(result, DetailedAtomicOutcome::Published));
        assert_eq!(fs::read(&path).unwrap(), b"payload");
    }

    #[test]
    fn write_bytes_exclusive_preserves_distinct_invalid_utf8_basenames() {
        let temporary = TempDir::new();
        let left = temporary.path().join(os_from_bytes(b"file-\xff-a"));
        let right = temporary.path().join(os_from_bytes(b"file-\xfe-a"));
        write_bytes_exclusive(&left, b"alpha", AtomicWriteOptions::default()).unwrap();
        write_bytes_exclusive(&right, b"beta", AtomicWriteOptions::default()).unwrap();
        assert_eq!(fs::read(&left).unwrap(), b"alpha");
        assert_eq!(fs::read(&right).unwrap(), b"beta");
    }

    #[test]
    fn write_reader_exclusive_preserves_distinct_invalid_utf8_basenames() {
        let temporary = TempDir::new();
        let left = temporary.path().join(os_from_bytes(b"reader-\xff-a"));
        let right = temporary.path().join(os_from_bytes(b"reader-\xfe-a"));
        let copied_left = write_reader_exclusive(
            &left,
            &mut io::Cursor::new(b"alpha"),
            AtomicWriteOptions::default(),
        )
        .unwrap();
        let copied_right = write_reader_exclusive(
            &right,
            &mut io::Cursor::new(b"beta"),
            AtomicWriteOptions::default(),
        )
        .unwrap();
        assert_eq!(copied_left, 5);
        assert_eq!(copied_right, 4);
        assert_eq!(fs::read(&left).unwrap(), b"alpha");
        assert_eq!(fs::read(&right).unwrap(), b"beta");
    }

    #[test]
    fn atomic_replace_preserves_distinct_invalid_utf8_basenames() {
        let temporary = TempDir::new();
        let left = temporary.path().join(os_from_bytes(b"replace-\xff-a"));
        let right = temporary.path().join(os_from_bytes(b"replace-\xfe-a"));
        atomic_replace(&left, b"alpha", AtomicWriteOptions::default()).unwrap();
        atomic_replace(&right, b"beta", AtomicWriteOptions::default()).unwrap();
        assert_eq!(fs::read(&left).unwrap(), b"alpha");
        assert_eq!(fs::read(&right).unwrap(), b"beta");
    }

    #[test]
    fn detailed_replace_preserves_distinct_invalid_utf8_basenames() {
        let temporary = TempDir::new();
        let left = temporary.path().join(os_from_bytes(b"detailed-\xff-a"));
        let right = temporary.path().join(os_from_bytes(b"detailed-\xfe-a"));
        assert!(matches!(
            atomic_replace_detailed(&left, b"alpha", 0o644).unwrap(),
            DetailedAtomicOutcome::Published
        ));
        assert!(matches!(
            atomic_replace_detailed(&right, b"beta", 0o644).unwrap(),
            DetailedAtomicOutcome::Published
        ));
        assert_eq!(fs::read(&left).unwrap(), b"alpha");
        assert_eq!(fs::read(&right).unwrap(), b"beta");
    }

    #[test]
    fn publication_candidate_dot_destination_uses_underscore_sentinel() {
        let name = publication_candidate_name(OsStr::new(".env"), ATOMIC_CANDIDATE_MARKER, &[1, 2]);
        assert_eq!(name.as_encoded_bytes().first(), Some(&b'_'));
        assert!(name.as_encoded_bytes().starts_with(b"_tmp_"));
    }

    #[test]
    fn publication_candidate_underscore_destination_uses_dot_sentinel() {
        let name = publication_candidate_name(OsStr::new("_keep"), ATOMIC_CANDIDATE_MARKER, &[1]);
        assert_eq!(name.as_encoded_bytes().first(), Some(&b'.'));
        assert!(name.as_encoded_bytes().starts_with(b".tmp_"));
    }

    #[test]
    fn publication_candidate_ordinary_ascii_uses_dot_sentinel() {
        let name =
            publication_candidate_name(OsStr::new("report.json"), ATOMIC_CANDIDATE_MARKER, &[1, 2]);
        assert_eq!(name.as_encoded_bytes().first(), Some(&b'.'));
        assert!(name.as_encoded_bytes().starts_with(b".tmp_"));
        assert!(name.as_encoded_bytes().ends_with(b".tmp"));
        let staged =
            publication_candidate_name(OsStr::new("bundle"), STAGED_CANDIDATE_MARKER, &[1, 2]);
        assert!(staged.as_encoded_bytes().starts_with(b".stage_"));
        assert!(staged.as_encoded_bytes().ends_with(b".tmp"));
        assert!(!staged.as_encoded_bytes().starts_with(b".tmp_"));
    }

    #[test]
    fn publication_candidate_interior_dot_does_not_flip_sentinel() {
        let name = publication_candidate_name(OsStr::new("foo.bar"), ATOMIC_CANDIDATE_MARKER, &[1]);
        assert_eq!(name.as_encoded_bytes().first(), Some(&b'.'));
    }

    #[test]
    fn publication_candidate_invalid_utf8_leading_byte_uses_dot_unless_ascii_dot() {
        let not_dot = publication_candidate_name(
            &os_from_bytes(b"\xffhidden"),
            ATOMIC_CANDIDATE_MARKER,
            &[1],
        );
        assert_eq!(not_dot.as_encoded_bytes().first(), Some(&b'.'));
        let leading_dot = publication_candidate_name(
            &os_from_bytes(b".\xffhidden"),
            ATOMIC_CANDIDATE_MARKER,
            &[1],
        );
        assert_eq!(leading_dot.as_encoded_bytes().first(), Some(&b'_'));
        assert!(
            !not_dot.as_encoded_bytes().contains(&0xff)
                && !leading_dot.as_encoded_bytes().contains(&0xff)
        );
        let replacement = [0xef, 0xbf, 0xbd];
        assert!(
            !not_dot
                .as_encoded_bytes()
                .windows(3)
                .any(|window| window == replacement)
        );
        assert!(
            !leading_dot
                .as_encoded_bytes()
                .windows(3)
                .any(|window| window == replacement)
        );
    }

    #[test]
    fn publication_candidate_leading_bytes_remain_distinct_after_ascii_case_fold() {
        // '.' (U+002E) and '_' (U+005F) have no canonical decomposition and no case mapping.
        for dest in [OsStr::new(".env"), OsStr::new("report.json")] {
            let candidate = publication_candidate_name(dest, ATOMIC_CANDIDATE_MARKER, &[1, 2]);
            let dest_lead = dest
                .as_encoded_bytes()
                .first()
                .copied()
                .unwrap_or(b'x')
                .to_ascii_lowercase();
            let cand_lead = candidate.as_encoded_bytes()[0].to_ascii_lowercase();
            assert_ne!(cand_lead, dest_lead);
        }
    }

    #[test]
    fn create_temporary_succeeds_after_ninety_nine_forced_collisions() {
        let temporary = TempDir::new();
        let destination = temporary.path().join("unit.service");
        let destination_name = destination.file_name().unwrap();
        let forced = forced_names(destination_name, ATOMIC_CANDIDATE_MARKER, 99);
        for (index, name) in forced.iter().enumerate() {
            fs::write(temporary.path().join(name), format!("collider-{index}")).unwrap();
        }
        let (path, file) =
            create_temporary_forced(temporary.path(), &destination, &forced).unwrap();
        drop(file);
        assert!(!forced.iter().any(|name| name == path.file_name().unwrap()));
        assert_live_candidate(
            destination_name,
            path.file_name().unwrap(),
            ATOMIC_CANDIDATE_MARKER,
            3,
        );
    }

    #[test]
    fn create_temporary_exhausts_after_one_hundred_forced_collisions() {
        let temporary = TempDir::new();
        let destination = temporary.path().join("unit.service");
        let destination_name = destination.file_name().unwrap();
        let forced = forced_names(destination_name, ATOMIC_CANDIDATE_MARKER, 100);
        for (index, name) in forced.iter().enumerate() {
            fs::write(temporary.path().join(name), format!("collider-{index}")).unwrap();
        }
        let error = create_temporary_forced(temporary.path(), &destination, &forced).unwrap_err();
        match error {
            AtomicWriteError::Io { path, source } => {
                assert_eq!(path, destination);
                assert_eq!(source.kind(), io::ErrorKind::AlreadyExists);
                assert_eq!(
                    source.to_string(),
                    "could not allocate unique temporary file"
                );
            }
        }
        assert!(!destination.exists());
        for (index, name) in forced.iter().enumerate() {
            assert_eq!(
                fs::read(temporary.path().join(name)).unwrap(),
                format!("collider-{index}").into_bytes()
            );
        }
        assert_eq!(dir_names(temporary.path()), forced.into_iter().collect());
    }

    #[test]
    fn allocate_bound_stage_succeeds_after_ninety_nine_forced_collisions() {
        let temporary = TempDir::new();
        let destination = OsStr::new("unit.service");
        let path = temporary.path().join(destination);
        let forced = forced_names(destination, ATOMIC_CANDIDATE_MARKER, 99);
        for (index, name) in forced.iter().enumerate() {
            fs::write(temporary.path().join(name), format!("collider-{index}")).unwrap();
        }
        let directory = File::open(temporary.path()).unwrap();
        let (name, file) =
            allocate_bound_stage_forced(&directory, destination, &path, &forced).unwrap();
        drop(file);
        assert!(!forced.iter().any(|forced_name| forced_name == &name));
        assert_live_candidate(destination, &name, ATOMIC_CANDIDATE_MARKER, 2);
    }

    #[test]
    fn allocate_bound_stage_exhausts_after_one_hundred_forced_collisions() {
        let temporary = TempDir::new();
        let destination = OsStr::new("unit.service");
        let path = temporary.path().join(destination);
        let forced = forced_names(destination, ATOMIC_CANDIDATE_MARKER, 100);
        for (index, name) in forced.iter().enumerate() {
            fs::write(temporary.path().join(name), format!("collider-{index}")).unwrap();
        }
        let directory = File::open(temporary.path()).unwrap();
        let error =
            allocate_bound_stage_forced(&directory, destination, &path, &forced).unwrap_err();
        assert_eq!(error.operation, "create stage");
        assert_eq!(error.source.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(error.source.to_string(), "could not allocate stage");
        assert!(!path.exists());
        for (index, name) in forced.iter().enumerate() {
            assert_eq!(
                fs::read(temporary.path().join(name)).unwrap(),
                format!("collider-{index}").into_bytes()
            );
        }
        assert_eq!(dir_names(temporary.path()), forced.into_iter().collect());
    }
}
