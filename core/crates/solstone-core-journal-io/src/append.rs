// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable append-only text and JSONL writers.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde::Serialize;

use crate::LockOptions;

#[cfg(unix)]
use crate::atomic::{fsync_dir, sync_file};
use crate::errors::AppendError;

#[cfg(windows)]
fn sync_file(file: &fs::File) -> io::Result<()> {
    // `File::sync_all` is `FlushFileBuffers` on Windows.  The existing Unix append contract
    // only requires the record-file sync to succeed; parent-directory sync after a create is
    // best effort there, and Windows has no corresponding directory-handle operation here.
    file.sync_all()
}

/// Append one newline-terminated text record through a single raw write.
///
/// A successful return means exactly one complete record was appended and
/// synced. A returned error can still leave a partial write on disk when the
/// underlying single write reports a short byte count.
#[cfg(any(unix, windows))]
pub fn append_text(path: impl AsRef<Path>, text: &str) -> Result<(), AppendError> {
    append_text_checked(path, text, &|| Ok(()))
}

/// Append one record, checking before the parent is created and again immediately
/// before the record is written.
#[cfg(any(unix, windows))]
pub fn append_text_checked(
    path: impl AsRef<Path>,
    text: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), AppendError> {
    let mut contents = Vec::with_capacity(text.len() + 1);
    contents.extend_from_slice(text.as_bytes());
    contents.push(b'\n');
    append_record(
        path.as_ref(),
        &contents,
        checkpoint,
        None,
        CutTail::StartNewLine,
    )
}

#[cfg(unix)]
use crate::FlatDirectory as CheckedAppendDirectory;
#[cfg(windows)]
use crate::WindowsFlatDirectory as CheckedAppendDirectory;

/// Append through the retained parent, refusing links and renewing caller and
/// named-file authority immediately before the write. An error after a write
/// can leave appended bytes; success acknowledges the complete record/file sync.
#[cfg(any(unix, windows))]
pub fn append_text_bound_checked(
    parent: &CheckedAppendDirectory,
    name: &std::ffi::OsStr,
    text: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), AppendError> {
    let path = parent.diagnostic_path().join(name);
    if name
        .to_str()
        .is_none_or(|name| crate::check_portable_component(name).is_err())
    {
        return Err(io_error(
            &path,
            io::Error::new(io::ErrorKind::InvalidInput, "invalid append filename"),
        ));
    }
    let check = || {
        checkpoint().map_err(|detail| {
            io_error(&path, io::Error::new(io::ErrorKind::Interrupted, detail))
        })?;
        #[cfg(unix)]
        let result = parent.revalidate();
        #[cfg(windows)]
        let result = parent.revalidate_bound();
        result.map_err(|error| io_error(&path, io::Error::other(error.to_string())))
    };
    check()?;
    let file = open_bound_append(parent, name).map_err(|error| io_error(&path, error))?;
    bound_append_open_barrier();
    check()?;
    check_bound_append_file(parent, name, &file).map_err(|error| io_error(&path, error))?;
    let mut contents = Vec::with_capacity(text.len() + 1);
    contents.extend_from_slice(text.as_bytes());
    contents.push(b'\n');
    write_and_sync(&path, file, &contents)?;
    #[cfg(unix)]
    nix::unistd::fsync(parent).map_err(|error| io_error(&path, error.into()))?;
    check()
}

#[cfg(unix)]
fn open_bound_append(
    parent: &CheckedAppendDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<fs::File> {
    use nix::fcntl::{OFlag, openat};
    use nix::sys::stat::Mode;
    let file = openat(
        parent,
        name,
        OFlag::O_WRONLY
            | OFlag::O_APPEND
            | OFlag::O_CREAT
            | OFlag::O_NOFOLLOW
            | OFlag::O_CLOEXEC
            | OFlag::O_NONBLOCK,
        Mode::from_bits_truncate(0o666),
    )
    .map(fs::File::from)
    .map_err(io::Error::from)?;
    check_bound_append_file(parent, name, &file)?;
    Ok(file)
}

#[cfg(unix)]
fn check_bound_append_file(
    parent: &CheckedAppendDirectory,
    name: &std::ffi::OsStr,
    file: &fs::File,
) -> io::Result<()> {
    use nix::fcntl::AtFlags;
    use nix::sys::stat::{SFlag, fstat, fstatat};
    let opened = fstat(file).map_err(io::Error::from)?;
    let named = fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
    if opened.st_nlink != 1
        || named.st_nlink != 1
        || SFlag::from_bits_truncate(opened.st_mode) != SFlag::S_IFREG
        || SFlag::from_bits_truncate(named.st_mode) != SFlag::S_IFREG
        || opened.st_dev != named.st_dev
        || opened.st_ino != named.st_ino
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "append destination changed",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn open_bound_append(
    parent: &CheckedAppendDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<fs::File> {
    use std::io::{Seek, SeekFrom};
    let mut file = crate::windows_sync_dir::create_or_open_windows_append_file_bound(
        parent,
        name,
        parent.diagnostic_path(),
    )
    .map_err(|error| io::Error::other(error.to_string()))?;
    file.seek(SeekFrom::End(0))?;
    Ok(file)
}

#[cfg(windows)]
fn check_bound_append_file(
    parent: &CheckedAppendDirectory,
    name: &std::ffi::OsStr,
    file: &fs::File,
) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    let opened = crate::windows_identity::file_identity(file.as_raw_handle())?;
    let named = crate::windows_sync_dir::open_windows_regular_file_from_bound_parent(
        parent,
        name,
        parent.diagnostic_path(),
    )
    .map_err(|error| io::Error::other(error.to_string()))?
    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "append destination disappeared"))?;
    if opened != crate::windows_identity::file_identity(named.as_raw_handle())? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "append destination changed",
        ));
    }
    Ok(())
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static BOUND_APPEND_OPEN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_bound_append_open_barrier<T>(
    callback: impl FnOnce() + 'static,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    BOUND_APPEND_OPEN_HOOK.with(|hook| {
        assert!(hook.borrow().is_none(), "bound append hook already active");
        *hook.borrow_mut() = Some(Box::new(callback));
    });
    let result = operation();
    let pending = BOUND_APPEND_OPEN_HOOK.with(|hook| hook.borrow_mut().take());
    (result, pending.is_none())
}

fn bound_append_open_barrier() {
    #[cfg(any(test, feature = "test-hooks"))]
    BOUND_APPEND_OPEN_HOOK.with(|hook| {
        let callback = hook.borrow_mut().take();
        if let Some(callback) = callback {
            callback();
        }
    });
}

/// How a canonical JSONL append creates its file.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonlAppendOptions {
    /// Unix mode for a file this append creates. Windows has no equivalent.
    pub mode: Option<u32>,
}

/// Serialize and append one newline-terminated JSON record through a single raw write.
///
/// The append holds the path's canonical lock ([`crate::hold_lock`]), or reuses
/// it when this thread already holds it, so it never interleaves with another
/// writer or with a healing reader's rewrite. The record always starts on a
/// line of its own: a last line without a newline keeps its place when it is
/// valid JSON, and is otherwise moved into a dated `.wedged` side file beside
/// the log (a stopped write's remains, quietly healed). Every complete line
/// stays as it is. For operational logs, never an owner transcript.
///
/// A successful return means exactly one complete record was appended and
/// synced. A returned error can still leave a partial write on disk when the
/// underlying single write reports a short byte count.
pub fn append_jsonl<T: Serialize>(path: impl AsRef<Path>, record: &T) -> Result<(), AppendError> {
    append_jsonl_with_options(path, record, JsonlAppendOptions::default())
}

/// [`append_jsonl`], creating a missing file with `options`.
pub fn append_jsonl_with_options<T: Serialize>(
    path: impl AsRef<Path>,
    record: &T,
    options: JsonlAppendOptions,
) -> Result<(), AppendError> {
    let path = path.as_ref();
    let mut contents = serde_json::to_vec(record)
        .map_err(|source| io_error(path, io::Error::new(io::ErrorKind::InvalidData, source)))?;
    contents.push(b'\n');
    append_record(
        path,
        &contents,
        &|| Ok(()),
        options.mode,
        CutTail::SetAsideIfDamaged,
    )
}

/// Append complete JSONL lines to a file that must already exist, never creating it.
///
/// Returns `Ok(false)` when the file is missing. For a target no lock sidecar
/// may sit beside and no durable reader rewrites: writers coordinate through
/// this process's path gate ([`crate::hold_path_gate`]) and one write. A last
/// line cut short gets a newline first; `lines` gets a final newline if it
/// lacks one.
pub fn append_lines_to_existing(path: impl AsRef<Path>, lines: &[u8]) -> Result<bool, AppendError> {
    let path = path.as_ref();
    if lines.is_empty() {
        return Ok(path.exists());
    }
    let mut contents = lines.to_vec();
    if contents.last() != Some(&b'\n') {
        contents.push(b'\n');
    }
    let _gate = crate::hold_path_gate(path);
    let file = match OpenOptions::new().read(true).append(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error(path, error)),
    };
    // A side file would be a leaf a namespace census counts: keep the line.
    write_after_line_boundary_and_sync(path, file, &contents, CutTail::StartNewLine)?;
    Ok(true)
}

/// Cut a file back to the length a caller's checkpoint recorded, then append
/// one record there, under the path's canonical lock.
///
/// For a writer that checkpoints how much of its own log it has written and
/// must drop what a stopped run appended after that point. A file shorter than
/// the checkpoint is not padded: the record goes at its end, after a newline if
/// its last line was cut short. Returns the file's new length.
pub fn append_jsonl_at_checkpoint<T: Serialize>(
    path: impl AsRef<Path>,
    checkpoint_len: u64,
    record: &T,
) -> Result<u64, AppendError> {
    let path = path.as_ref();
    let mut contents = serde_json::to_vec(record)
        .map_err(|source| io_error(path, io::Error::new(io::ErrorKind::InvalidData, source)))?;
    contents.push(b'\n');
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;
    let _lock =
        crate::hold_or_reuse_lock(path, append_lock_options()).map_err(|e| lock_error(path, e))?;
    // Not append mode: Windows refuses to cut a file opened that way.
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|source| io_error(path, source))?;
    let length = file
        .metadata()
        .map_err(|source| io_error(path, source))?
        .len();
    if length > checkpoint_len {
        file.set_len(checkpoint_len)
            .map_err(|source| io_error(path, source))?;
    }
    let end = length.min(checkpoint_len);
    if end > 0 && last_byte_at(&mut file, end).map_err(|source| io_error(path, source))? != b'\n' {
        contents.insert(0, b'\n');
    }
    file.seek(SeekFrom::Start(end))
        .map_err(|source| io_error(path, source))?;
    file.write_all(&contents)
        .map_err(|source| io_error(path, source))?;
    file.sync_all().map_err(|source| io_error(path, source))?;
    Ok(end + contents.len() as u64)
}

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// Serialize and append one newline-terminated JSON record through a single raw write,
/// refusing symlink traversal and refusing to create missing parent directories.
#[cfg(any(unix, windows))]
pub fn append_jsonl_no_follow<T: Serialize>(
    path: impl AsRef<Path>,
    record: &T,
) -> Result<(), AppendError> {
    append_jsonl_no_follow_checked(path, record, &|| Ok(()))
}

#[cfg(any(unix, windows))]
pub fn append_jsonl_no_follow_checked<T: Serialize>(
    path: impl AsRef<Path>,
    record: &T,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), AppendError> {
    let path = path.as_ref();
    let mut contents = serde_json::to_vec(record)
        .map_err(|source| io_error(path, io::Error::new(io::ErrorKind::InvalidData, source)))?;
    contents.push(b'\n');
    append_record_no_follow(path, &contents, checkpoint)
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> io::Result<fs::File> {
    OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(windows)]
fn open_no_follow(path: &Path) -> io::Result<fs::File> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_READ_DATA, OPEN_ALWAYS, SYNCHRONIZE,
    };
    // Read access lets the append look at the last byte before it writes.
    let file = crate::locking::open_windows_path(
        path,
        FILE_APPEND_DATA | FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        OPEN_ALWAYS,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
    )?;
    let attributes = crate::locking::attribute_tag_windows(&file)?;
    if crate::locking::is_reparse_point_windows(attributes) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to append through a Windows reparse point",
        ));
    }
    Ok(file)
}

/// The options every canonical append takes the path's lock with: a private
/// sidecar and the ordinary bounded wait.
fn append_lock_options() -> LockOptions {
    LockOptions {
        mode: Some(0o600),
        ..LockOptions::default()
    }
}

fn lock_error(path: &Path, error: crate::LockError) -> AppendError {
    let source = match error {
        crate::LockError::Io { source, .. } => source,
        timeout @ crate::LockError::Timeout(_) => {
            io::Error::new(io::ErrorKind::TimedOut, timeout.to_string())
        }
    };
    io_error(path, source)
}

/// The byte just before `end`, which must be at least one.
fn last_byte_at(file: &mut fs::File, end: u64) -> io::Result<u8> {
    file.seek(SeekFrom::Start(end - 1))?;
    let mut last = [0_u8];
    file.read_exact(&mut last)?;
    Ok(last[0])
}

/// What an append does with a last line that has no newline after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CutTail {
    /// End it with a newline and keep it where it is.
    StartNewLine,
    /// Keep it if it is one valid JSON value; otherwise a write stopped part
    /// way, so move it into a `<stem>.wedged-<stamp>-line-<n><ext>` side file
    /// beside the log (a quiet heal, counted by
    /// [`crate::operational_heals_since`]) and append where it began.
    SetAsideIfDamaged,
}

/// Append `contents` in one write so it begins on a line of its own. `file` is
/// open for reading and appending; the caller holds whatever excludes other
/// writers. Every complete line already in the file stays as it is.
fn write_after_line_boundary_and_sync(
    path: &Path,
    mut file: fs::File,
    contents: &[u8],
    cut_tail: CutTail,
) -> Result<(), AppendError> {
    let io = |source| io_error(path, source);
    let length = file.metadata().map_err(io)?.len();
    if length == 0 || last_byte_at(&mut file, length).map_err(io)? == b'\n' {
        return write_and_sync(path, file, contents);
    }
    if cut_tail == CutTail::SetAsideIfDamaged {
        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(0)).map_err(io)?;
        file.read_to_end(&mut bytes).map_err(io)?;
        let tail_start = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |newline| newline + 1);
        let tail = crate::jsonl_integrity::JsonlLine {
            number: bytes[..tail_start]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count()
                + 1,
            bytes: &bytes[tail_start..],
            terminated: false,
        };
        if crate::jsonl_integrity::classify_line(&tail).is_some() {
            crate::jsonl_integrity::write_new_aside(
                path,
                &format!("-line-{}", tail.number),
                tail.bytes,
            )
            .map_err(io)?;
            let cutter = OpenOptions::new().write(true).open(path).map_err(io)?;
            cutter.set_len(tail_start as u64).map_err(io)?;
            sync_file(&cutter).map_err(io)?;
            return write_and_sync(path, file, contents);
        }
    }
    if contents.first() == Some(&b'\n') {
        return write_and_sync(path, file, contents);
    }
    let mut bounded = Vec::with_capacity(contents.len() + 1);
    bounded.push(b'\n');
    bounded.extend_from_slice(contents);
    write_and_sync(path, file, &bounded)
}

fn write_and_sync(path: &Path, mut file: fs::File, contents: &[u8]) -> Result<(), AppendError> {
    let written = file
        .write(contents)
        .map_err(|source| io_error(path, source))?;
    if written != contents.len() {
        return Err(io_error(
            path,
            io::Error::new(
                io::ErrorKind::WriteZero,
                "append record was only partially written",
            ),
        ));
    }
    sync_file(&file).map_err(|source| io_error(path, source))?;
    Ok(())
}

fn append_record_no_follow(
    path: &Path,
    contents: &[u8],
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), AppendError> {
    let check = || {
        checkpoint()
            .map_err(|detail| io_error(path, io::Error::new(io::ErrorKind::Interrupted, detail)))
    };
    check()?;
    // These targets include segment directories, where no lock sidecar may go,
    // and nothing rewrites them: this process's writers take turns instead.
    let _gate = crate::hold_path_gate(path);
    // A newly created file's directory entry is made durable too, as `append_record`
    // does. `symlink_metadata` does not follow the leaf, so a link is not "new".
    #[cfg(unix)]
    let is_new = matches!(
        fs::symlink_metadata(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound
    );
    let file = open_no_follow(path).map_err(|source| io_error(path, source))?;
    check()?;
    // A side file in a segment directory would read as content: keep the line.
    write_after_line_boundary_and_sync(path, file, contents, CutTail::StartNewLine)?;
    #[cfg(unix)]
    if is_new {
        fsync_dir(parent_dir(path));
    }
    Ok(())
}

fn append_record(
    path: &Path,
    contents: &[u8],
    checkpoint: &dyn Fn() -> Result<(), String>,
    mode: Option<u32>,
    cut_tail: CutTail,
) -> Result<(), AppendError> {
    let check = || {
        checkpoint()
            .map_err(|detail| io_error(path, io::Error::new(io::ErrorKind::Interrupted, detail)))
    };
    check()?;
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;
    let _lock = crate::hold_or_reuse_lock_checked(path, append_lock_options(), checkpoint)
        .map_err(|error| lock_error(path, error))?;
    #[cfg(unix)]
    let is_new = !path.exists();
    check()?;
    let mut options = OpenOptions::new();
    options.read(true).append(true).create(true);
    #[cfg(unix)]
    if let Some(mode) = mode {
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let file = options
        .open(path)
        .map_err(|source| io_error(path, source))?;
    check()?;
    write_after_line_boundary_and_sync(path, file, contents, cut_tail)?;
    #[cfg(unix)]
    if is_new {
        fsync_dir(parent);
    }
    Ok(())
}

fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn io_error(path: &Path, source: io::Error) -> AppendError {
    AppendError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(unix)]
pub use bound_append::{
    BoundAppendConfirmation, BoundAppendDurability, BoundAppendEffect, BoundAppendReceipt,
    append_jsonl_bound,
};

#[cfg(unix)]
mod bound_append {
    use std::ffi::OsStr;
    use std::fs::File;
    use std::io::{self, Read, Seek, SeekFrom, Write};
    use std::path::Path;

    use nix::fcntl::{AtFlags, OFlag, openat};
    use nix::sys::stat::{FileStat, Mode, fstat, fstatat};
    use serde::Serialize;

    use crate::flat_directory::entry_from_stat;
    use crate::journal_root::JournalEntryKind;
    use crate::observation::{FileObservation, same_entry_metadata};
    use crate::{FlatDirectory, read_observed_file_bounded};

    const INPUT_LIMIT: usize = 32 * 1024 * 1024;
    const RECORD_LIMIT: usize = 1024 * 1024;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum BoundAppendEffect {
        NotAttempted,
        /// The open may have created a leaf; a failed write may have changed bytes.
        Attempted {
            written: Option<usize>,
        },
        Appended {
            bytes: usize,
        },
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum BoundAppendDurability {
        Unproven,
        Synced,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum BoundAppendConfirmation {
        Unverified,
        Confirmed,
    }

    /// Physical facts beneath the retained parent, without canonical-root authority.
    #[must_use]
    #[derive(Debug)]
    pub struct BoundAppendReceipt {
        effect: BoundAppendEffect,
        file_durability: BoundAppendDurability,
        parent_durability: BoundAppendDurability,
        confirmation: BoundAppendConfirmation,
        failures: Vec<io::Error>,
    }

    impl BoundAppendReceipt {
        pub fn effect(&self) -> BoundAppendEffect {
            self.effect
        }
        pub fn file_durability(&self) -> BoundAppendDurability {
            self.file_durability
        }
        pub fn parent_durability(&self) -> BoundAppendDurability {
            self.parent_durability
        }
        pub fn confirmation(&self) -> BoundAppendConfirmation {
            self.confirmation
        }
        pub fn failures(&self) -> &[io::Error] {
            &self.failures
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(super) enum Point {
        BeforeOpen,
        BeforeWrite,
        AfterWrite,
        FileSync,
        ParentSync,
        AfterConfirmationRead,
    }

    pub(super) trait Hooks {
        fn checkpoint(&mut self, _point: Point) {}
        fn write(&mut self, file: &mut File, contents: &[u8]) -> io::Result<usize> {
            file.write(contents)
        }
        fn sync(&mut self, file: &impl std::os::fd::AsFd, _point: Point) -> io::Result<()> {
            nix::unistd::fsync(file).map_err(io::Error::from)
        }
    }
    struct NoHooks;
    impl Hooks for NoHooks {}

    fn invalid() -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "bound append observation changed",
        )
    }
    fn observed_error(error: impl std::fmt::Display) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, error.to_string())
    }

    struct RecordWriter(Vec<u8>);
    impl Write for RecordWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self
                .0
                .len()
                .checked_add(bytes.len())
                .is_none_or(|len| len >= RECORD_LIMIT)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "bound append record exceeds limit",
                ));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn same_stat(left: &FileStat, right: &FileStat) -> bool {
        left.st_dev == right.st_dev
            && left.st_ino == right.st_ino
            && left.st_mode == right.st_mode
            && left.st_nlink == right.st_nlink
            && left.st_size == right.st_size
            && left.st_mtime == right.st_mtime
            && left.st_mtime_nsec == right.st_mtime_nsec
            && left.st_ctime == right.st_ctime
            && left.st_ctime_nsec == right.st_ctime_nsec
    }

    fn named_stat(parent: &FlatDirectory, name: &OsStr) -> io::Result<FileStat> {
        fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(io::Error::from)
    }

    fn opened_observation(
        file: &mut File,
        name: &OsStr,
    ) -> io::Result<(FileObservation, FileStat)> {
        let before = fstat(&*file).map_err(io::Error::from)?;
        if before.st_nlink != 1 || before.st_size < 0 {
            return Err(invalid());
        }
        let entry =
            entry_from_stat(name.to_owned(), &before, Path::new(name)).map_err(observed_error)?;
        if entry.kind != JournalEntryKind::RegularFile || entry.size > INPUT_LIMIT as u64 {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(0))?;
        Read::by_ref(file)
            .take((INPUT_LIMIT as u64) + 1)
            .read_to_end(&mut bytes)?;
        let after = fstat(&*file).map_err(io::Error::from)?;
        let final_entry =
            entry_from_stat(name.to_owned(), &after, Path::new(name)).map_err(observed_error)?;
        if after.st_nlink != 1
            || !same_stat(&before, &after)
            || !same_entry_metadata(&entry, &final_entry)
            || u64::try_from(bytes.len()).ok() != Some(entry.size)
        {
            return Err(invalid());
        }
        Ok((FileObservation { entry, bytes }, after))
    }

    /// Append once through the retained parent and preserve independent effect facts.
    /// The caller retains and revalidates canonical ancestry and its owning exclusions.
    pub fn append_jsonl_bound<T: Serialize>(
        parent: &FlatDirectory,
        name: &OsStr,
        record: &T,
    ) -> BoundAppendReceipt {
        append_with_hooks(parent, name, record, &mut NoHooks)
    }

    pub(super) fn append_with_hooks<T: Serialize>(
        parent: &FlatDirectory,
        name: &OsStr,
        record: &T,
        hooks: &mut impl Hooks,
    ) -> BoundAppendReceipt {
        let mut receipt = BoundAppendReceipt {
            effect: BoundAppendEffect::NotAttempted,
            file_durability: BoundAppendDurability::Unproven,
            parent_durability: BoundAppendDurability::Unproven,
            confirmation: BoundAppendConfirmation::Unverified,
            failures: Vec::new(),
        };
        let preparation = (|| -> io::Result<(Vec<u8>, Option<FileObservation>)> {
            let mut encoded = RecordWriter(Vec::new());
            serde_json::to_writer(&mut encoded, record).map_err(observed_error)?;
            encoded.0.push(b'\n');
            parent.revalidate().map_err(observed_error)?;
            let baseline =
                read_observed_file_bounded(parent, name, INPUT_LIMIT).map_err(observed_error)?;
            if baseline.as_ref().is_some_and(|observed| {
                !observed.bytes.is_empty() && observed.bytes.last() != Some(&b'\n')
            }) {
                return Err(invalid());
            }
            Ok((encoded.0, baseline))
        })();
        let (contents, baseline) = match preparation {
            Ok(value) => value,
            Err(error) => {
                receipt.failures.push(error);
                return receipt;
            }
        };
        let opened = (|| -> io::Result<(File, Vec<u8>)> {
            hooks.checkpoint(Point::BeforeOpen);
            parent.revalidate().map_err(observed_error)?;
            receipt.effect = BoundAppendEffect::Attempted { written: None };
            let fd = openat(
                parent,
                name,
                OFlag::O_APPEND
                    | OFlag::O_RDWR
                    | OFlag::O_CREAT
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_CLOEXEC
                    | OFlag::O_NONBLOCK,
                Mode::from_bits_truncate(0o600),
            )
            .map_err(io::Error::from)?;
            let mut file = File::from(fd);
            hooks.checkpoint(Point::BeforeWrite);
            let (opened, opened_stat) = opened_observation(&mut file, name)?;
            if baseline
                .as_ref()
                .is_some_and(|baseline| baseline != &opened)
                || (baseline.is_none() && !opened.bytes.is_empty())
                || !same_stat(&opened_stat, &named_stat(parent, name)?)
            {
                return Err(invalid());
            }
            let mut expected = opened.bytes;
            expected
                .len()
                .checked_add(contents.len())
                .ok_or_else(invalid)?;
            expected.extend_from_slice(&contents);
            parent.revalidate().map_err(observed_error)?;
            if !same_stat(&opened_stat, &named_stat(parent, name)?) {
                return Err(invalid());
            }
            Ok((file, expected))
        })();
        let (mut file, expected) = match opened {
            Ok(value) => value,
            Err(error) => {
                receipt.failures.push(error);
                return receipt;
            }
        };
        match hooks.write(&mut file, &contents) {
            Ok(written) if written == contents.len() => {
                receipt.effect = BoundAppendEffect::Appended { bytes: written }
            }
            Ok(written) => {
                receipt.effect = BoundAppendEffect::Attempted {
                    written: Some(written),
                };
                receipt.failures.push(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "bound append record was only partially written",
                ));
            }
            Err(error) => receipt.failures.push(error),
        }
        hooks.checkpoint(Point::AfterWrite);
        match hooks.sync(&file, Point::FileSync) {
            Ok(()) => receipt.file_durability = BoundAppendDurability::Synced,
            Err(error) => receipt.failures.push(error),
        }
        match hooks.sync(parent, Point::ParentSync) {
            Ok(()) => receipt.parent_durability = BoundAppendDurability::Synced,
            Err(error) => receipt.failures.push(error),
        }
        let confirmation = (|| -> io::Result<()> {
            let before = fstat(&file).map_err(io::Error::from)?;
            let observed = read_observed_file_bounded(parent, name, expected.len())
                .map_err(observed_error)?
                .ok_or_else(invalid)?;
            let retained = fstat(&file).map_err(io::Error::from)?;
            let retained_entry = entry_from_stat(name.to_owned(), &retained, Path::new(name))
                .map_err(observed_error)?;
            if retained.st_nlink != 1
                || !same_stat(&before, &retained)
                || observed.bytes != expected
                || !same_entry_metadata(&observed.entry, &retained_entry)
            {
                return Err(invalid());
            }
            hooks.checkpoint(Point::AfterConfirmationRead);
            if !same_stat(&retained, &named_stat(parent, name)?) {
                return Err(invalid());
            }
            parent.revalidate().map_err(observed_error)?;
            Ok(())
        })();
        match confirmation {
            Ok(()) if matches!(receipt.effect, BoundAppendEffect::Appended { .. }) => {
                receipt.confirmation = BoundAppendConfirmation::Confirmed
            }
            Ok(()) => {}
            Err(error) => receipt.failures.push(error),
        }
        receipt
    }
}

#[cfg(all(test, unix, feature = "full-tests"))]
#[path = "../tests/support/append_bound.rs"]
mod bound_tests;

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn appends_one_complete_newline_terminated_record_per_call() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        append_text(&path, "first").unwrap();
        let first_len = fs::metadata(&path).unwrap().len();
        append_jsonl(&path, &serde_json::json!({"second": true})).unwrap();
        let contents = fs::read_to_string(&path).unwrap();

        assert_eq!(first_len, "first\n".len() as u64);
        assert_eq!(
            contents.lines().collect::<Vec<_>>(),
            vec!["first", r#"{"second":true}"#]
        );
        assert!(contents.ends_with('\n'));
    }

    #[test]
    fn a_jsonl_append_sets_a_cut_last_line_aside_and_keeps_every_complete_line() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        fs::write(&path, b"{\"kept\":1}\n{\"cut\":").unwrap();
        append_jsonl(&path, &serde_json::json!({"next": 2})).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"kept\":1}\n{\"next\":2}\n");
        let asides = fs::read_dir(temporary.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.contains(".wedged-"))
            .collect::<Vec<_>>();
        assert_eq!(asides.len(), 1);
        assert!(asides[0].starts_with("records.wedged-") && asides[0].ends_with("-line-2.jsonl"));
        assert_eq!(
            fs::read(temporary.path().join(&asides[0])).unwrap(),
            b"{\"cut\":"
        );
        let heals = crate::operational_heals_since(temporary.path(), std::time::UNIX_EPOCH);
        assert!(
            heals.unwrap().is_empty(),
            "an unregistered file is not counted"
        );
    }

    #[test]
    fn a_valid_last_line_without_a_newline_keeps_its_place() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        fs::write(&path, b"{\"kept\":1}").unwrap();
        append_jsonl(&path, &serde_json::json!({"next": 2})).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"kept\":1}\n{\"next\":2}\n");
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 2);
    }

    #[test]
    fn a_text_or_segment_append_after_a_cut_line_starts_a_new_line_in_place() {
        let temporary = TempDir::new();
        let path = temporary.path().join("events.jsonl");
        fs::write(&path, b"{\"kept\":1}\n{\"cut\":").unwrap();
        append_jsonl_no_follow(&path, &serde_json::json!({"next": 2})).unwrap();
        append_text(&path, "\nalready-led").unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            b"{\"kept\":1}\n{\"cut\":\n{\"next\":2}\n\nalready-led\n"
        );
        assert!(fs::read_dir(temporary.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".wedged-")
        }));
    }

    #[test]
    fn append_reuses_the_lock_its_caller_already_holds() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        let _outer = crate::hold_lock(
            &path,
            LockOptions {
                timeout: std::time::Duration::from_millis(1),
                ..LockOptions::default()
            },
        )
        .unwrap();
        append_jsonl(&path, &serde_json::json!({"inside": true})).unwrap();
        append_text_checked(&path, "also", &|| Ok(())).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"inside\":true}\nalso\n");
    }

    #[test]
    fn concurrent_appenders_each_land_one_whole_line() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        std::thread::scope(|scope| {
            for writer in 0..8 {
                let path = &path;
                scope.spawn(move || {
                    for row in 0..25 {
                        append_jsonl(path, &serde_json::json!({"writer": writer, "row": row}))
                            .unwrap();
                    }
                });
            }
        });
        let bytes = fs::read(&path).unwrap();
        let (valid, damaged) = crate::inspect_jsonl_bytes(&bytes);
        assert_eq!((valid, damaged.len()), (200, 0));
    }

    #[cfg(unix)]
    #[test]
    fn append_with_a_mode_creates_a_private_file() {
        use std::os::unix::fs::PermissionsExt;
        let temporary = TempDir::new();
        let path = temporary.path().join("private.jsonl");
        append_jsonl_with_options(&path, &1, JsonlAppendOptions { mode: Some(0o600) }).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn appending_to_an_existing_file_never_creates_one() {
        let temporary = TempDir::new();
        let path = temporary.path().join("123_active.jsonl");
        assert!(!append_lines_to_existing(&path, b"{\"late\":true}\n").unwrap());
        assert!(!path.exists());
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 0);
        fs::write(&path, b"{\"start\":1}").unwrap();
        assert!(append_lines_to_existing(&path, b"{\"a\":1}\n{\"b\":2}").unwrap());
        assert_eq!(
            fs::read(&path).unwrap(),
            b"{\"start\":1}\n{\"a\":1}\n{\"b\":2}\n"
        );
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_checkpointed_append_drops_what_a_stopped_run_wrote_after_the_checkpoint() {
        let temporary = TempDir::new();
        let path = temporary.path().join("body-touched.jsonl");
        let first = append_jsonl_at_checkpoint(&path, 0, &serde_json::json!({"day": 1})).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"day\":2}\n")
            .unwrap();
        let second =
            append_jsonl_at_checkpoint(&path, first, &serde_json::json!({"day": 2})).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"day\":1}\n{\"day\":2}\n");
        assert_eq!(second, fs::metadata(&path).unwrap().len());
        // A file shorter than its checkpoint is never padded with zeros.
        fs::write(&path, b"{\"day\":1}\n{\"da").unwrap();
        let third = append_jsonl_at_checkpoint(&path, 400, &serde_json::json!({"day": 3})).unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            b"{\"day\":1}\n{\"da\n{\"day\":3}\n"
        );
        assert_eq!(third, fs::metadata(&path).unwrap().len());
    }

    #[test]
    fn checked_append_refuses_revoked_authority_after_destination_open() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        let result = append_text_checked(&path, "withheld", &|| {
            if path.exists() {
                Err("authority revoked after open".to_owned())
            } else {
                Ok(())
            }
        });
        assert!(matches!(result, Err(AppendError::Io { source, .. })
            if source.kind() == io::ErrorKind::Interrupted));
        assert_eq!(fs::read(&path).unwrap(), b"");
        append_text_checked(&path, "admitted", &|| Ok(())).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"admitted\n");
    }

    #[test]
    fn appends_jsonl_as_one_complete_newline_terminated_record_per_call() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        append_jsonl(&path, &serde_json::json!({"first": true})).unwrap();
        let first_len = fs::metadata(&path).unwrap().len();
        append_jsonl(&path, &serde_json::json!({"second": true})).unwrap();
        let contents = fs::read_to_string(&path).unwrap();

        assert_eq!(first_len, r#"{"first":true}"#.len() as u64 + 1);
        assert_eq!(
            contents.lines().collect::<Vec<_>>(),
            vec![r#"{"first":true}"#, r#"{"second":true}"#]
        );
        assert!(contents.ends_with('\n'));
    }

    #[test]
    fn appends_empty_text_as_one_complete_newline_terminated_record() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        append_text(&path, "").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"\n");
    }

    #[test]
    fn appends_nonempty_text_as_one_complete_newline_terminated_record() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        append_text(&path, "first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first\n");
    }

    #[test]
    fn appends_repeated_text_records_accumulate_in_call_order() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        append_text(&path, "a").unwrap();
        append_text(&path, "b").unwrap();
        append_text(&path, "c").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"a\nb\nc\n");
    }

    #[test]
    fn appends_text_into_a_missing_nested_parent_directory() {
        let temporary = TempDir::new();
        let path = temporary.path().join("nested/a/b/records.jsonl");
        append_text(&path, "first").unwrap();
        assert!(path.parent().unwrap().is_dir());
        assert_eq!(fs::read(&path).unwrap(), b"first\n");
    }

    #[test]
    fn append_jsonl_no_follow_appends_records_in_order() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        append_jsonl_no_follow(&path, &serde_json::json!({"first": true})).unwrap();
        append_jsonl_no_follow(&path, &serde_json::json!({"second": 2})).unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(
            contents.lines().collect::<Vec<_>>(),
            vec![r#"{"first":true}"#, r#"{"second":2}"#]
        );
        assert!(contents.ends_with('\n'));
    }

    #[cfg(unix)]
    #[test]
    fn append_jsonl_no_follow_refuses_symlinked_leaf_on_unix() {
        let temporary = TempDir::new();
        let target = temporary.path().join("target.jsonl");
        let link = temporary.path().join("link.jsonl");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let error =
            append_jsonl_no_follow(&link, &serde_json::json!({"blocked": true})).unwrap_err();
        assert!(matches!(error, AppendError::Io { .. }));
        assert!(!target.exists() || fs::read(&target).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn append_jsonl_no_follow_refuses_missing_parent_and_creates_nothing_on_unix() {
        let temporary = TempDir::new();
        let missing_parent = temporary.path().join("missing_dir");
        let path = missing_parent.join("records.jsonl");

        let error =
            append_jsonl_no_follow(&path, &serde_json::json!({"blocked": true})).unwrap_err();
        assert!(matches!(error, AppendError::Io { .. }));
        assert!(!missing_parent.exists());
        assert!(!path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn append_jsonl_no_follow_accumulates_records_in_order_on_windows() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        append_jsonl_no_follow(&path, &serde_json::json!({"first": 1})).unwrap();
        append_jsonl_no_follow(&path, &serde_json::json!({"second": 2})).unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(
            contents.lines().collect::<Vec<_>>(),
            vec![r#"{"first":1}"#, r#"{"second":2}"#]
        );
        assert!(contents.ends_with('\n'));
    }

    #[cfg(windows)]
    #[test]
    fn append_jsonl_no_follow_refuses_symlinked_leaf_on_windows() {
        let temporary = TempDir::new();
        let target = temporary.path().join("target.jsonl");
        fs::write(&target, b"").unwrap();
        let link = temporary.path().join("link.jsonl");
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            return;
        }

        let error =
            append_jsonl_no_follow(&link, &serde_json::json!({"blocked": true})).unwrap_err();
        assert!(matches!(error, AppendError::Io { .. }));
        assert!(fs::read(&target).unwrap().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn append_jsonl_no_follow_refuses_dangling_symlink_on_windows() {
        let temporary = TempDir::new();
        let target = temporary.path().join("nonexistent_target.jsonl");
        let link = temporary.path().join("link.jsonl");
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            return;
        }

        let error =
            append_jsonl_no_follow(&link, &serde_json::json!({"blocked": true})).unwrap_err();
        assert!(matches!(error, AppendError::Io { .. }));
        assert!(!target.exists());
    }

    #[cfg(windows)]
    #[test]
    fn append_jsonl_no_follow_refuses_missing_parent_and_creates_nothing_on_windows() {
        let temporary = TempDir::new();
        let missing_parent = temporary.path().join("missing_dir");
        let path = missing_parent.join("records.jsonl");

        let error =
            append_jsonl_no_follow(&path, &serde_json::json!({"blocked": true})).unwrap_err();
        assert!(matches!(error, AppendError::Io { .. }));
        assert!(!missing_parent.exists());
        assert!(!path.exists());
    }
}
