// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Crash-safe whole-file writers.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::errors::AtomicWriteError;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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

/// Atomically replace `path` with fully durable `contents`.
///
/// The replacement bytes are synced before rename. The containing directory is
/// synced afterwards on a best-effort basis; a directory-sync failure is logged
/// and does not turn an otherwise published replacement into an error.
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

fn create_temporary(
    parent: &Path,
    destination: &Path,
) -> Result<(PathBuf, File), AtomicWriteError> {
    let stem = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    for _ in 0..100 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let candidate = parent.join(format!(
            ".tmp_{stem}_{}_{}_{}.tmp",
            std::process::id(),
            nanos,
            sequence
        ));
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

#[cfg(test)]
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

#[cfg(not(test))]
fn pause_at(_step: &str) {}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};

    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    use super::*;
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

    #[test]
    fn atomic_pause_helper() {
        let Ok(target) = std::env::var("JOURNAL_IO_HELPER_TARGET") else {
            return;
        };
        atomic_replace(target, b"new-content", AtomicWriteOptions::default()).unwrap();
    }

    #[test]
    fn atomic_replace_survives_kill_at_every_boundary() {
        let temporary = TempDir::new();
        for step in [
            "temp-create",
            "write",
            "fsync-file",
            "chmod",
            "close",
            "rename",
            "fsync-parent-dir",
        ] {
            let target = temporary.path().join(format!("{step}.txt"));
            let marker = temporary.path().join(format!("{step}.ready"));
            fs::write(&target, b"old-content").unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "atomic::tests::atomic_pause_helper",
                    "--nocapture",
                ])
                .env("JOURNAL_IO_HELPER_TARGET", &target)
                .env("JOURNAL_IO_TEST_PAUSE_AT", step)
                .env("JOURNAL_IO_TEST_MARKER", &marker)
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while !marker.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            assert!(marker.exists(), "helper did not reach {step}");
            kill(Pid::from_raw(child.id() as i32), Signal::SIGKILL).unwrap();
            child.wait().unwrap();
            let contents = fs::read(&target).unwrap();
            assert!(
                contents == b"old-content" || contents == b"new-content",
                "{step}"
            );
        }
    }
}
