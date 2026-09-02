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
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
/// Publication is create-only: an existing destination, including a dangling
/// symlink, fails with `AlreadyExists`. Before the rename, failures remove the
/// staging directory. A process killed before rename can leave that private
/// staging directory orphaned, but the destination remains absent; after rename
/// it contains the complete, synced set. Parent-directory sync is best effort.
///
/// On Windows, parent-directory sync after rename remains best effort: there is
/// no documented directory-entry metadata flush on a directory handle opened
/// for listing and attributes (`FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES |
/// SYNCHRONIZE`, plus `FILE_TRAVERSE` on the publication-path capability) —
/// neither includes `GENERIC_WRITE`/`FILE_WRITE_DATA`, which `FlushFileBuffers`
/// requires. This primitive does not claim that flush. It also does not claim
/// ACL equivalence with Unix [`StagedDirOptions::directory_mode`].
///
/// After the absence check, a late name at `fs::rename` is replace-or-refuse
/// on Unix (an empty directory is replaced by the complete staged set; a
/// regular file or non-empty directory is left untouched and publish fails).
/// On Windows that same collision is platform- and filesystem-conditioned,
/// not a guaranteed replace-or-refuse outcome.
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
            let mut builder = DirBuilder::new();
            #[cfg(unix)]
            if let Some(mode) = options.directory_mode {
                builder.mode(mode);
            }
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
}
