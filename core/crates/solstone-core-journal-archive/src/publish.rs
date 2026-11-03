// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Descriptor-relative, create-only archive publication.

#![allow(
    clippy::disallowed_methods,
    reason = "this isolated module is the archive crate's publication authority"
)]

use std::ffi::{OsStr, OsString};
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io::Error;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(not(target_os = "android"))]
use nix::fcntl::AtFlags;
use nix::fcntl::{OFlag, openat};
use nix::sys::stat::Mode;
#[cfg(not(target_os = "android"))]
use nix::unistd::linkat;
use nix::unistd::{UnlinkatFlags, fsync, unlinkat};

use crate::target::{ArchiveOutputTarget, ExplicitTargetError};
use crate::{EncodeArchiveError, EncodeArchiveRequest, encode_archive};

const TEMP_FILE_FLAGS: OFlag = OFlag::O_WRONLY
    .union(OFlag::O_CREAT)
    .union(OFlag::O_EXCL)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Failure while encoding and create-only publishing an archive.
#[derive(Debug)]
pub enum ArchivePublicationError {
    Target(ExplicitTargetError),
    CreateTemp(Error),
    Encode(EncodeArchiveError),
    SyncTemp(Error),
    Publish(Error),
    SyncDirectory(Error),
    Cleanup(Error),
}

impl Display for ArchivePublicationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Target(error) => error.fmt(formatter),
            Self::CreateTemp(error) => write!(formatter, "create archive temporary file: {error}"),
            Self::Encode(error) => error.fmt(formatter),
            Self::SyncTemp(error) => write!(formatter, "sync archive temporary file: {error}"),
            Self::Publish(error) => write!(formatter, "publish archive: {error}"),
            Self::SyncDirectory(error) => write!(formatter, "sync archive directory: {error}"),
            Self::Cleanup(error) => write!(formatter, "clean archive temporary file: {error}"),
        }
    }
}

impl std::error::Error for ArchivePublicationError {}

/// Encode and atomically create the final archive through the retained parent.
pub fn publish_archive(
    target: &ArchiveOutputTarget,
    request: &EncodeArchiveRequest<'_>,
) -> Result<(), ArchivePublicationError> {
    publish_archive_with(target, request, publish_temp)
}

fn publish_archive_with(
    target: &ArchiveOutputTarget,
    request: &EncodeArchiveRequest<'_>,
    publish: impl FnOnce(&ArchiveOutputTarget, &OsStr) -> Result<bool, Error>,
) -> Result<(), ArchivePublicationError> {
    target
        .revalidate()
        .map_err(ArchivePublicationError::Target)?;
    let (temp_name, temp_fd) = create_temp(target)?;
    let mut file = File::from(temp_fd);
    if let Err(error) = encode_archive(request, &mut file) {
        drop(file);
        cleanup_temp(target, &temp_name)?;
        return Err(ArchivePublicationError::Encode(error));
    }
    if let Err(error) = file.sync_all() {
        drop(file);
        cleanup_temp(target, &temp_name)?;
        return Err(ArchivePublicationError::SyncTemp(error));
    }
    drop(file);
    if let Err(error) = target.revalidate() {
        cleanup_temp(target, &temp_name)?;
        return Err(ArchivePublicationError::Target(error));
    }
    let temp_consumed = match publish(target, &temp_name) {
        Ok(consumed) => consumed,
        Err(error) => {
            cleanup_temp(target, &temp_name)?;
            return Err(ArchivePublicationError::Publish(error));
        }
    };
    fsync(&target.parent).map_err(|error| {
        ArchivePublicationError::SyncDirectory(Error::from_raw_os_error(error as i32))
    })?;
    if !temp_consumed {
        cleanup_temp(target, &temp_name)?;
    }
    Ok(())
}

#[cfg(not(target_os = "android"))]
fn publish_temp(target: &ArchiveOutputTarget, temp_name: &OsStr) -> Result<bool, Error> {
    linkat(
        &target.parent,
        temp_name,
        &target.parent,
        target.final_name.as_os_str(),
        AtFlags::empty(),
    )
    .map_err(Error::from)?;
    Ok(false)
}

#[cfg(target_os = "android")]
fn publish_temp(target: &ArchiveOutputTarget, temp_name: &OsStr) -> Result<bool, Error> {
    publish_temp_by_rename(target, temp_name)
}

#[cfg(any(
    target_os = "android",
    all(test, feature = "full-tests", target_os = "linux")
))]
fn publish_temp_by_rename(target: &ArchiveOutputTarget, temp_name: &OsStr) -> Result<bool, Error> {
    // This Journal IO primitive validates single names and uses native no-replace rename.
    // Its retained-parent contract also applies to files; no overwrite fallback is used.
    solstone_core_journal_io::media::publish_directory_noreplace_bound(
        &target.parent,
        temp_name,
        &target.parent,
        target.final_name.as_os_str(),
    )?;
    Ok(true)
}

fn create_temp(
    target: &ArchiveOutputTarget,
) -> Result<(OsString, OwnedFd), ArchivePublicationError> {
    for _ in 0..64 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let name = OsString::from(format!(".solstone-archive-{stamp}-{sequence}.tmp"));
        match openat(
            &target.parent,
            name.as_os_str(),
            TEMP_FILE_FLAGS,
            Mode::from_bits_truncate(0o600),
        ) {
            Ok(fd) => return Ok((name, fd)),
            Err(nix::errno::Errno::EEXIST) => continue,
            Err(error) => {
                return Err(ArchivePublicationError::CreateTemp(
                    Error::from_raw_os_error(error as i32),
                ));
            }
        }
    }
    Err(ArchivePublicationError::CreateTemp(Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique archive temporary name",
    )))
}

fn cleanup_temp(target: &ArchiveOutputTarget, name: &OsStr) -> Result<(), ArchivePublicationError> {
    unlinkat(&target.parent, name, UnlinkatFlags::NoRemoveDir).map_err(|error| {
        ArchivePublicationError::Cleanup(Error::from_raw_os_error(error as i32))
    })?;
    fsync(&target.parent)
        .map_err(|error| ArchivePublicationError::Cleanup(Error::from_raw_os_error(error as i32)))
}

#[cfg(all(test, feature = "full-tests", target_os = "linux"))]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Read;
    use std::path::PathBuf;

    use nix::errno::Errno;
    use solstone_core_journal_io::JournalRoot;

    struct TemporaryDirectory(PathBuf);

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn android_rename_publishes_readable_archive_and_preserves_a_late_collision() {
        let path = std::env::temp_dir().join(format!(
            "archive-rename-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let temporary = TemporaryDirectory(path);
        let source_path = temporary.0.join("source");
        let output_path = temporary.0.join("output");
        fs::create_dir_all(source_path.join("imports/example")).unwrap();
        fs::create_dir(&output_path).unwrap();
        let original = source_path.join("imports/example/source.bin");
        fs::write(&original, b"preserved original").unwrap();
        let source = crate::ArchiveSource::open(&source_path).unwrap();
        let request = EncodeArchiveRequest {
            source: &source,
            solstone_version: "2.0.66",
            exported_at: "2026-11-03T07:00:00Z",
            day_window: None,
        };
        let target = crate::acquire_bound_output_target(
            JournalRoot::open_no_follow(&output_path).unwrap(),
            OsStr::new("archive.zip"),
        )
        .unwrap();
        publish_archive_with(&target, &request, publish_temp_by_rename).unwrap();
        // Success includes parent synchronization, with no unlink of the consumed name.
        assert_eq!(fs::read_dir(&output_path).unwrap().count(), 1);
        let mut zip =
            zip::ZipArchive::new(File::open(output_path.join("archive.zip")).unwrap()).unwrap();
        let mut bytes = Vec::new();
        zip.by_name("imports/example/source.bin")
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"preserved original");

        let collision = output_path.join("collision.zip");
        let target = crate::acquire_bound_output_target(
            JournalRoot::open_no_follow(&output_path).unwrap(),
            OsStr::new("collision.zip"),
        )
        .unwrap();
        let result = publish_archive_with(&target, &request, |target, temp_name| {
            // Race the final target check so the actual syscall must refuse replacement.
            fs::write(&collision, b"keep existing destination").unwrap();
            publish_temp_by_rename(target, temp_name)
        });
        assert!(matches!(
            result,
            Err(ArchivePublicationError::Publish(ref error))
                if error.raw_os_error() == Some(Errno::EEXIST as i32)
        ));
        assert_eq!(fs::read(&collision).unwrap(), b"keep existing destination");
        assert_eq!(fs::read(&original).unwrap(), b"preserved original");
        assert_eq!(fs::read_dir(&output_path).unwrap().count(), 2);
    }
}
