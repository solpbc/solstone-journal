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

use nix::fcntl::{AtFlags, OFlag, openat};
use nix::sys::stat::Mode;
use nix::unistd::{UnlinkatFlags, fsync, linkat, unlinkat};

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
    if let Err(error) = linkat(
        &target.parent,
        temp_name.as_os_str(),
        &target.parent,
        target.final_name.as_os_str(),
        AtFlags::empty(),
    ) {
        cleanup_temp(target, &temp_name)?;
        return Err(ArchivePublicationError::Publish(Error::from_raw_os_error(
            error as i32,
        )));
    }
    fsync(&target.parent).map_err(|error| {
        ArchivePublicationError::SyncDirectory(Error::from_raw_os_error(error as i32))
    })?;
    cleanup_temp(target, &temp_name)?;
    Ok(())
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
