// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Create-only directory-set publication through a same-parent staging directory.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, DirBuilder};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_SHARING_VIOLATION,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

#[cfg(unix)]
use crate::atomic::fsync_dir;
use crate::atomic::{STAGED_CANDIDATE_MARKER, publication_candidate_name};

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Options for a private staging and destination directory.
///
/// On Windows, [`Self::directory_mode`] is a no-op: there is no documented
/// `DirBuilder` mode-bit primitive, and this type does not claim ACL
/// equivalence with Unix directory mode bits.
#[derive(Debug, Clone, Copy, Default)]
pub struct StagedDirOptions {
    /// Directory mode applied atomically when the staging directory is created.
    ///
    /// On Unix, passed to [`DirBuilder::mode`] when `Some`. On Windows this
    /// field is ignored. There is no documented equivalent of Unix `mkdir`
    /// mode bits on `DirBuilder`, and this option does not claim ACL
    /// equivalence.
    pub directory_mode: Option<u32>,
}

/// Failure while preparing or publishing a staged directory set.
#[derive(Debug)]
pub enum StagedWriteError {
    /// A filesystem or staging-directory sync operation failed.
    Io { path: PathBuf, source: io::Error },
    /// The caller's population closure failed before publication.
    Populate {
        path: PathBuf,
        source: Box<dyn Error + Send + Sync>,
    },
}

impl fmt::Display for StagedWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Populate { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl Error for StagedWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Populate { source, .. } => Some(source.as_ref()),
        }
    }
}

/// Populate and atomically publish a new directory at `destination`.
///
/// A destination observed before staging begins, including a dangling symlink,
/// fails with `AlreadyExists`. Before rename, ordinary failures attempt to
/// remove the private staging directory. Cleanup is best effort: a process
/// killed before rename or a removal blocked by sharing or permissions can
/// leave that directory orphaned, while the destination remains absent. After
/// rename, the destination contains the complete populated set.
///
/// On Unix, the staging directory is hard-synced before rename and the parent
/// directory is synced best effort afterward.
///
/// On Windows, both directory-sync operations are omitted: there is no
/// documented directory-entry metadata flush on a directory handle opened for
/// listing and attributes (`FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES |
/// SYNCHRONIZE`, plus `FILE_TRAVERSE` on the publication-path capability) —
/// neither includes `GENERIC_WRITE`/`FILE_WRITE_DATA`, which `FlushFileBuffers`
/// requires. This primitive makes no directory-metadata durability claim on
/// Windows. It also does not claim ACL equivalence with Unix
/// [`StagedDirOptions::directory_mode`].
///
/// After the absence check, a late name at `fs::rename` is replace-or-refuse
/// on Unix (an empty directory is replaced by the complete staged set; a
/// regular file or non-empty directory is left untouched and publish fails).
/// On Windows the exact result is platform- and filesystem-conditioned: a
/// successful rename exposes the complete staged set, while a refused rename
/// preserves the competing destination.
///
/// This primitive does not hold a lock across its absence-check-and-rename
/// sequence. Concurrent, uncoordinated callers targeting the same destination
/// can race; callers needing strict mutual exclusion across multiple publishers
/// must coordinate externally, for example with [`crate::hold_lock`]. This
/// wave's link-bundle consumer publishes to a unique path per pairing session
/// and does not hit this window in practice.
pub fn publish_staged_dir<F, E>(
    destination: &Path,
    options: StagedDirOptions,
    populate: F,
) -> Result<(), StagedWriteError>
where
    F: FnOnce(&Path) -> Result<(), E>,
    E: Error + Send + Sync + 'static,
{
    let parent = parent_dir(destination);
    fs::create_dir_all(parent).map_err(|source| io_error(destination, source))?;
    if lexists(destination).map_err(|source| io_error(destination, source))? {
        return Err(io_error(
            destination,
            io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists"),
        ));
    }

    pause_at("before-staging-dir-create");
    let staging = StagingDir::create(parent, destination, options)?;
    pause_at("after-staging-dir-create");
    populate(staging.path()).map_err(|source| StagedWriteError::Populate {
        path: destination.to_path_buf(),
        source: Box::new(source),
    })?;
    pause_at("after-populate");
    #[cfg(unix)]
    {
        File::open(staging.path())
            .and_then(|directory| directory.sync_all())
            .map_err(|source| io_error(destination, source))?;
    }
    pause_at("after-staging-sync");
    fs::rename(staging.path(), destination).map_err(|source| io_error(destination, source))?;
    staging.disarm();
    pause_at("after-rename");
    #[cfg(unix)]
    {
        fsync_dir(parent);
    }
    Ok(())
}

/// A private staging directory inside its target parent that can be populated
/// once and published atomically without replacing any existing entry.
///
/// If dropped without a successful [`Self::publish_exclusive`] call, the
/// staging directory and its contents are removed best effort.
pub struct ExclusiveStagedDir {
    parent: PathBuf,
    staging: StagingDir,
    options: StagedDirOptions,
}

impl ExclusiveStagedDir {
    /// Create a hidden staging directory inside `parent`.
    pub fn create_in(
        parent: &Path,
        name_hint: &OsStr,
        options: StagedDirOptions,
    ) -> Result<Self, StagedWriteError> {
        fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
        let hint_path = parent.join(name_hint);
        let staging = StagingDir::create(parent, &hint_path, options)?;
        #[cfg(unix)]
        if let Some(mode) = options.directory_mode {
            fs::set_permissions(staging.path(), fs::Permissions::from_mode(mode))
                .map_err(|source| io_error(&hint_path, source))?;
        }
        Ok(Self {
            parent: parent.to_path_buf(),
            staging,
            options,
        })
    }

    /// Path of the private staging directory before publication.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.staging.path()
    }

    /// Atomically publish this staging directory to `destination` only when
    /// `destination` does not already exist (including an empty directory,
    /// regular file, or symlink).
    ///
    /// On `AlreadyExists`, the staging directory remains intact and armed so
    /// the caller may probe another destination name or clean up on drop.
    pub fn publish_exclusive(&mut self, destination: &Path) -> Result<(), StagedWriteError> {
        if !self.staging.armed {
            return Err(io_error(
                destination,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "staging directory already published",
                ),
            ));
        }
        let parent = parent_dir(destination);
        if parent != self.parent {
            return Err(io_error(
                destination,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "destination parent does not match staging directory parent",
                ),
            ));
        }
        let dest_name = destination.file_name().ok_or_else(|| {
            io_error(
                destination,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "destination has no final path component",
                ),
            )
        })?;
        if lexists(destination).map_err(|source| io_error(destination, source))? {
            return Err(io_error(
                destination,
                io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists"),
            ));
        }
        #[cfg(unix)]
        {
            if let Some(mode) = self.options.directory_mode {
                fs::set_permissions(self.staging.path(), fs::Permissions::from_mode(mode))
                    .map_err(|source| io_error(destination, source))?;
            }
            File::open(self.staging.path())
                .and_then(|directory| directory.sync_all())
                .map_err(|source| io_error(destination, source))?;
            let stage_name = self.staging.path().file_name().ok_or_else(|| {
                io_error(
                    destination,
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "staging directory has no file name",
                    ),
                )
            })?;
            crate::media::publish_directory_noreplace(parent, stage_name, dest_name)
                .map_err(|source| io_error(destination, source))?;
        }
        #[cfg(windows)]
        {
            let _ = (parent, dest_name, self.options);
            move_directory_noreplace_windows(self.staging.path(), destination)
                .map_err(|source| io_error(destination, source))?;
        }
        self.staging.armed = false;
        #[cfg(unix)]
        {
            fsync_dir(&self.parent);
        }
        Ok(())
    }
}

/// Populate and atomically publish a new directory at `destination` without
/// replacing any existing entry — including an empty directory, regular file,
/// or symlink.
///
/// Unlike [`publish_staged_dir`], concurrent callers targeting the same
/// `destination` cannot silently replace an empty directory occupant: Unix
/// publishes with native no-replace rename (`renameat2(RENAME_NOREPLACE)` /
/// `renameatx_np(RENAME_EXCL)`) and Windows publishes with
/// `MoveFileExW(..., 0)` (retrying briefly on transient sharing/access errors
/// while `destination` remains absent). An existing occupant fails with
/// `io::ErrorKind::AlreadyExists` and leaves the occupant untouched while
/// cleaning up the staging directory.
pub fn publish_staged_dir_exclusive<F, E>(
    destination: &Path,
    options: StagedDirOptions,
    populate: F,
) -> Result<(), StagedWriteError>
where
    F: FnOnce(&Path) -> Result<(), E>,
    E: Error + Send + Sync + 'static,
{
    let parent = parent_dir(destination);
    let dest_name = destination.file_name().ok_or_else(|| {
        io_error(
            destination,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination has no final path component",
            ),
        )
    })?;
    fs::create_dir_all(parent).map_err(|source| io_error(destination, source))?;
    if lexists(destination).map_err(|source| io_error(destination, source))? {
        return Err(io_error(
            destination,
            io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists"),
        ));
    }
    let mut staged = ExclusiveStagedDir::create_in(parent, dest_name, options)?;
    populate(staged.path()).map_err(|source| StagedWriteError::Populate {
        path: destination.to_path_buf(),
        source: Box::new(source),
    })?;
    staged.publish_exclusive(destination)
}

#[cfg(windows)]
fn move_directory_noreplace_windows(source: &Path, destination: &Path) -> io::Result<()> {
    let source_wide = encode_wide_nul(source)?;
    let destination_wide = encode_wide_nul(destination)?;
    const BACKOFF_MS: &[u64] = &[5, 10, 20, 40, 80, 120];
    let mut attempt = 0;
    loop {
        // SAFETY: both NUL-terminated UTF-16 buffers live through the synchronous Win32 call.
        #[allow(unsafe_code)]
        let ok = unsafe { MoveFileExW(source_wide.as_ptr(), destination_wide.as_ptr(), 0) };
        if ok != 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        let code = error.raw_os_error().map(|raw| raw as u32);
        if matches!(code, Some(ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS)) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, error));
        }
        if matches!(code, Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION)) {
            if lexists(destination)? {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, error));
            }
            if let Some(&delay_ms) = BACKOFF_MS.get(attempt) {
                attempt += 1;
                thread::sleep(Duration::from_millis(delay_ms));
                continue;
            }
        }
        return Err(error);
    }
}

#[cfg(windows)]
fn encode_wide_nul(path: &Path) -> io::Result<Vec<u16>> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains an interior NUL",
        ));
    }
    wide.push(0);
    Ok(wide)
}

struct StagingDir {
    path: PathBuf,
    armed: bool,
}

impl StagingDir {
    fn create(
        parent: &Path,
        destination: &Path,
        options: StagedDirOptions,
    ) -> Result<Self, StagedWriteError> {
        let destination_name = destination.file_name().unwrap_or(OsStr::new(""));
        for _ in 0..100 {
            let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = parent.join(publication_candidate_name(
                destination_name,
                STAGED_CANDIDATE_MARKER,
                &[u128::from(std::process::id()), nanos + u128::from(sequence)],
            ));
            #[cfg(unix)]
            let builder = {
                let mut builder = DirBuilder::new();
                if let Some(mode) = options.directory_mode {
                    builder.mode(mode);
                }
                builder
            };
            #[cfg(windows)]
            let builder = {
                let _ = options;
                DirBuilder::new()
            };
            match builder.create(&path) {
                Ok(()) => return Ok(Self { path, armed: true }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(io_error(destination, source)),
            }
        }
        Err(io_error(
            destination,
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not create unique staging directory",
            ),
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        if self.armed && lexists(&self.path).unwrap_or(false) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn lexists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn io_error(path: &Path, source: io::Error) -> StagedWriteError {
    StagedWriteError::Io {
        path: path.to_path_buf(),
        source,
    }
}

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
    use std::collections::BTreeSet;
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::io;
    #[cfg(target_os = "linux")]
    use std::os::unix::ffi::OsStringExt;
    use std::path::Path;

    use super::*;
    use crate::atomic::{STAGED_CANDIDATE_MARKER, publication_candidate_name};
    use crate::test_support::TempDir;

    fn write_complete_set(staging: &Path) -> io::Result<()> {
        fs::write(staging.join("manifest.json"), b"{\"complete\":true}\n")?;
        fs::write(staging.join("payload.bin"), b"complete-payload")?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn os_from_bytes(bytes: &[u8]) -> OsString {
        OsString::from_vec(bytes.to_vec())
    }

    fn dir_names(dir: &Path) -> BTreeSet<OsString> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect()
    }

    fn is_staging_candidate(name: &OsStr) -> bool {
        let bytes = name.as_encoded_bytes();
        (bytes.starts_with(b".stage_") || bytes.starts_with(b"_stage_")) && bytes.ends_with(b".tmp")
    }

    #[test]
    fn population_failure_removes_staging_directory() {
        let temporary = TempDir::new();
        let destination = temporary.path().join("bundle");
        let sibling = temporary.path().join("keep.txt");
        fs::write(&sibling, b"sibling-bytes").unwrap();
        let foreign = temporary.path().join(publication_candidate_name(
            OsStr::new("other"),
            STAGED_CANDIDATE_MARKER,
            &[1],
        ));
        fs::create_dir(&foreign).unwrap();
        fs::write(foreign.join("marker"), b"foreign").unwrap();

        let error = publish_staged_dir(&destination, StagedDirOptions::default(), |_staging| {
            Err::<(), _>(io::Error::other("injected failure"))
        });

        assert!(matches!(error, Err(StagedWriteError::Populate { .. })));
        assert!(!destination.exists());
        assert_eq!(fs::read(&sibling).unwrap(), b"sibling-bytes");
        assert_eq!(fs::read(foreign.join("marker")).unwrap(), b"foreign");
        assert_eq!(
            dir_names(temporary.path()),
            BTreeSet::from([
                sibling.file_name().unwrap().to_os_string(),
                foreign.file_name().unwrap().to_os_string()
            ])
        );
    }

    #[test]
    fn publisher_is_create_only() {
        let temporary = TempDir::new();
        let destination = temporary.path().join("bundle");
        fs::create_dir(&destination).unwrap();
        let error = publish_staged_dir(
            &destination,
            StagedDirOptions::default(),
            write_complete_set,
        )
        .unwrap_err();
        assert!(
            matches!(error, StagedWriteError::Io { source, .. } if source.kind() == io::ErrorKind::AlreadyExists)
        );
    }

    #[test]
    fn publish_staged_dir_observes_absent_destination_and_preserves_siblings() {
        let temporary = TempDir::new();
        let destination = temporary.path().join("bundle");
        let sibling = temporary.path().join("keep.txt");
        fs::write(&sibling, b"sibling-bytes").unwrap();

        publish_staged_dir(&destination, StagedDirOptions::default(), |staging| {
            assert!(fs::metadata(&destination).is_err());
            assert_eq!(fs::read(&sibling).unwrap(), b"sibling-bytes");
            write_complete_set(staging)
        })
        .unwrap();

        assert_eq!(
            fs::read(destination.join("manifest.json")).unwrap(),
            b"{\"complete\":true}\n"
        );
        assert_eq!(
            fs::read(destination.join("payload.bin")).unwrap(),
            b"complete-payload"
        );
        assert_eq!(fs::read(&sibling).unwrap(), b"sibling-bytes");
        assert!(
            dir_names(temporary.path())
                .iter()
                .all(|name| !is_staging_candidate(name))
        );
        assert_eq!(
            dir_names(temporary.path()),
            BTreeSet::from([
                destination.file_name().unwrap().to_os_string(),
                sibling.file_name().unwrap().to_os_string()
            ])
        );
    }

    #[test]
    fn filesystem_accepts_255_byte_file_names() {
        let temporary = TempDir::new();
        let path = temporary.path().join("a".repeat(255));
        fs::write(&path, b"ok").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"ok");
    }

    #[test]
    fn publish_staged_dir_publishes_255_byte_basename() {
        let temporary = TempDir::new();
        let destination = temporary.path().join("a".repeat(255));
        let candidate = publication_candidate_name(
            destination.file_name().unwrap(),
            STAGED_CANDIDATE_MARKER,
            &[u128::from(u32::MAX), u128::MAX, u128::from(u64::MAX)],
        );
        assert!(
            candidate.as_encoded_bytes().len() < 100,
            "candidate {} bytes",
            candidate.as_encoded_bytes().len()
        );
        publish_staged_dir(
            &destination,
            StagedDirOptions::default(),
            write_complete_set,
        )
        .unwrap();
        assert_eq!(
            fs::read(destination.join("payload.bin")).unwrap(),
            b"complete-payload"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn publish_staged_dir_preserves_distinct_invalid_utf8_basenames() {
        let temporary = TempDir::new();
        let left = temporary.path().join(os_from_bytes(b"bundle-\xff-a"));
        let right = temporary.path().join(os_from_bytes(b"bundle-\xfe-a"));
        publish_staged_dir(&left, StagedDirOptions::default(), |staging| {
            fs::write(staging.join("payload.bin"), b"alpha")
        })
        .unwrap();
        publish_staged_dir(&right, StagedDirOptions::default(), |staging| {
            fs::write(staging.join("payload.bin"), b"beta")
        })
        .unwrap();
        assert_eq!(fs::read(left.join("payload.bin")).unwrap(), b"alpha");
        assert_eq!(fs::read(right.join("payload.bin")).unwrap(), b"beta");
    }

    #[test]
    fn publish_staged_dir_creates_missing_nested_parent_directory() {
        let temporary = TempDir::new();
        let destination = temporary.path().join("nested/a/b/bundle");
        assert!(!temporary.path().join("nested").exists());

        publish_staged_dir(
            &destination,
            StagedDirOptions::default(),
            write_complete_set,
        )
        .unwrap();

        assert!(destination.parent().unwrap().is_dir());
        assert_eq!(
            fs::read(destination.join("manifest.json")).unwrap(),
            b"{\"complete\":true}\n"
        );
        assert_eq!(
            fs::read(destination.join("payload.bin")).unwrap(),
            b"complete-payload"
        );
    }

    #[cfg(unix)]
    #[test]
    fn publish_staged_dir_replaces_late_empty_directory_competitor() {
        let temporary = TempDir::new();
        let destination = temporary.path().join("bundle");
        let sibling = temporary.path().join("keep.txt");
        fs::write(&sibling, b"sibling-bytes").unwrap();

        publish_staged_dir(&destination, StagedDirOptions::default(), |staging| {
            fs::create_dir(&destination)?;
            write_complete_set(staging)
        })
        .unwrap();

        assert_eq!(
            fs::read(destination.join("manifest.json")).unwrap(),
            b"{\"complete\":true}\n"
        );
        assert_eq!(
            fs::read(destination.join("payload.bin")).unwrap(),
            b"complete-payload"
        );
        assert_eq!(fs::read(&sibling).unwrap(), b"sibling-bytes");
        assert!(
            dir_names(temporary.path())
                .iter()
                .all(|name| !is_staging_candidate(name))
        );
        assert_eq!(
            dir_names(temporary.path()),
            BTreeSet::from([
                destination.file_name().unwrap().to_os_string(),
                sibling.file_name().unwrap().to_os_string()
            ])
        );
    }

    #[cfg(unix)]
    #[test]
    fn publish_staged_dir_leaves_late_regular_file_competitor_untouched() {
        let temporary = TempDir::new();
        let destination = temporary.path().join("bundle");
        let sibling = temporary.path().join("keep.txt");
        fs::write(&sibling, b"sibling-bytes").unwrap();

        let error = publish_staged_dir(&destination, StagedDirOptions::default(), |staging| {
            fs::write(&destination, b"late-file-competitor")?;
            write_complete_set(staging)
        })
        .unwrap_err();

        assert!(matches!(error, StagedWriteError::Io { .. }));
        assert_eq!(fs::read(&destination).unwrap(), b"late-file-competitor");
        assert_eq!(fs::read(&sibling).unwrap(), b"sibling-bytes");
        assert_eq!(
            dir_names(temporary.path()),
            BTreeSet::from([
                destination.file_name().unwrap().to_os_string(),
                sibling.file_name().unwrap().to_os_string()
            ])
        );
    }

    #[cfg(unix)]
    #[test]
    fn publish_staged_dir_leaves_late_non_empty_directory_competitor_untouched() {
        let temporary = TempDir::new();
        let destination = temporary.path().join("bundle");
        let sibling = temporary.path().join("keep.txt");
        fs::write(&sibling, b"sibling-bytes").unwrap();

        let error = publish_staged_dir(&destination, StagedDirOptions::default(), |staging| {
            fs::create_dir(&destination)?;
            fs::write(destination.join("existing.txt"), b"late-dir-competitor")?;
            write_complete_set(staging)
        })
        .unwrap_err();

        assert!(matches!(error, StagedWriteError::Io { .. }));
        assert_eq!(
            fs::read(destination.join("existing.txt")).unwrap(),
            b"late-dir-competitor"
        );
        assert_eq!(fs::read(&sibling).unwrap(), b"sibling-bytes");
        assert_eq!(
            dir_names(temporary.path()),
            BTreeSet::from([
                destination.file_name().unwrap().to_os_string(),
                sibling.file_name().unwrap().to_os_string()
            ])
        );
    }

    #[test]
    fn publish_staged_dir_exclusive_leaves_late_empty_directory_competitor_untouched() {
        let temporary = TempDir::new();
        let destination = temporary.path().join("bundle");
        let sibling = temporary.path().join("keep.txt");
        fs::write(&sibling, b"sibling-bytes").unwrap();

        let error =
            publish_staged_dir_exclusive(&destination, StagedDirOptions::default(), |staging| {
                fs::create_dir(&destination)?;
                write_complete_set(staging)
            })
            .unwrap_err();

        match error {
            StagedWriteError::Io { source, .. } => {
                assert_eq!(source.kind(), io::ErrorKind::AlreadyExists);
            }
            other => panic!("expected AlreadyExists IO error, got {other:?}"),
        }
        assert!(destination.is_dir());
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
        assert_eq!(fs::read(&sibling).unwrap(), b"sibling-bytes");
        assert_eq!(
            dir_names(temporary.path()),
            BTreeSet::from([
                destination.file_name().unwrap().to_os_string(),
                sibling.file_name().unwrap().to_os_string()
            ])
        );
    }

    #[test]
    fn publish_staged_dir_exclusive_publishes_populated_directory_atomically() {
        let temporary = TempDir::new();
        let destination = temporary.path().join("bundle");

        publish_staged_dir_exclusive(
            &destination,
            StagedDirOptions {
                directory_mode: Some(0o700),
            },
            write_complete_set,
        )
        .unwrap();

        assert_eq!(
            fs::read(destination.join("payload.bin")).unwrap(),
            b"complete-payload"
        );
        assert_eq!(
            dir_names(temporary.path()),
            BTreeSet::from([destination.file_name().unwrap().to_os_string()])
        );
        #[cfg(unix)]
        {
            let mode = fs::symlink_metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700);
        }
    }
}
