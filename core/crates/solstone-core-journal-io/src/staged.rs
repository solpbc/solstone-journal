// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Create-only directory-set publication through a same-parent staging directory.

use std::error::Error;
use std::fmt;
use std::fs::{self, DirBuilder, File};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::atomic::fsync_dir;

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Options for a private staging and destination directory.
#[derive(Debug, Clone, Copy, Default)]
pub struct StagedDirOptions {
    /// Directory mode applied atomically when the staging directory is created.
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
    File::open(staging.path())
        .and_then(|directory| directory.sync_all())
        .map_err(|source| io_error(destination, source))?;
    pause_at("after-staging-sync");
    fs::rename(staging.path(), destination).map_err(|source| io_error(destination, source))?;
    staging.disarm();
    pause_at("after-rename");
    fsync_dir(parent);
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
        let stem = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("staged");
        for _ in 0..100 {
            let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = parent.join(format!(
                ".{stem}.staging.{}_{}.tmp",
                std::process::id(),
                nanos + u128::from(sequence)
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
    use std::fs;
    use std::io;

    use super::*;
    use crate::test_support::TempDir;

    fn write_complete_set(staging: &Path) -> io::Result<()> {
        fs::write(staging.join("manifest.json"), b"{\"complete\":true}\n")?;
        fs::write(staging.join("payload.bin"), b"complete-payload")?;
        Ok(())
    }

    #[test]
    fn population_failure_removes_staging_directory() {
        let temporary = TempDir::new();
        let destination = temporary.path().join("bundle");
        let error = publish_staged_dir(&destination, StagedDirOptions::default(), |_staging| {
            Err::<(), _>(io::Error::other("injected failure"))
        });

        assert!(matches!(error, Err(StagedWriteError::Populate { .. })));
        assert!(!destination.exists());
        assert!(fs::read_dir(temporary.path()).unwrap().next().is_none());
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
}
