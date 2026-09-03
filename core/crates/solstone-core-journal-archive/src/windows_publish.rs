// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows create-only archive publication.
//!
//! Archive bytes are encoded into an exclusively-created scratch file, then
//! copied through journal-io's retained Windows create-only protocol. The
//! output name is never opened for replacement, and a target revalidation
//! precedes publication.

use std::fmt::{Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_journal_io::{AtomicWriteOptions, write_reader_exclusive};

use crate::encode::{EncodeArchiveError, EncodeArchiveRequest, encode_archive};
use crate::windows_target::{ArchiveOutputTarget, ExplicitTargetError};

static SCRATCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Failure while encoding and create-only publishing an archive.
#[derive(Debug)]
pub enum ArchivePublicationError {
    Target(ExplicitTargetError),
    CreateTemp(std::io::Error),
    Encode(EncodeArchiveError),
    SyncTemp(std::io::Error),
    Publish(solstone_core_journal_io::AtomicWriteError),
    Cleanup(std::io::Error),
}

impl Display for ArchivePublicationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Target(error) => error.fmt(formatter),
            Self::CreateTemp(error) => write!(formatter, "create archive temporary file: {error}"),
            Self::Encode(error) => error.fmt(formatter),
            Self::SyncTemp(error) => write!(formatter, "sync archive temporary file: {error}"),
            Self::Publish(error) => write!(formatter, "publish archive: {error}"),
            Self::Cleanup(error) => write!(formatter, "clean archive temporary file: {error}"),
        }
    }
}

impl std::error::Error for ArchivePublicationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Target(error) => Some(error),
            Self::CreateTemp(error) | Self::SyncTemp(error) | Self::Cleanup(error) => Some(error),
            Self::Encode(error) => Some(error),
            Self::Publish(error) => Some(error),
        }
    }
}

/// Encode and atomically create the final archive through the Windows
/// create-only publication protocol.
pub fn publish_archive(
    target: &ArchiveOutputTarget,
    request: &EncodeArchiveRequest<'_>,
) -> Result<(), ArchivePublicationError> {
    target
        .revalidate()
        .map_err(ArchivePublicationError::Target)?;
    let (scratch_path, mut scratch) =
        create_scratch_file().map_err(ArchivePublicationError::CreateTemp)?;
    let result = (|| {
        encode_archive(request, &mut scratch).map_err(ArchivePublicationError::Encode)?;
        scratch
            .sync_all()
            .map_err(ArchivePublicationError::SyncTemp)?;
        scratch
            .seek(SeekFrom::Start(0))
            .map_err(ArchivePublicationError::SyncTemp)?;
        target
            .revalidate()
            .map_err(ArchivePublicationError::Target)?;
        write_reader_exclusive(
            target.final_path(),
            &mut scratch,
            AtomicWriteOptions { mode: Some(0o600) },
        )
        .map_err(ArchivePublicationError::Publish)
    })();
    // Keep the exclusively-created handle open while removing its name. That
    // prevents a same-user replacement race from turning cleanup into deletion
    // of another file after publication or a failed encode.
    let cleanup = fs::remove_file(&scratch_path);
    drop(scratch);
    match (result, cleanup) {
        (Ok(()), Ok(())) | (Err(_), Ok(())) => result,
        (Ok(()), Err(error)) | (Err(_), Err(error)) => Err(ArchivePublicationError::Cleanup(error)),
    }
}

fn create_scratch_file() -> std::io::Result<(PathBuf, File)> {
    let root = std::env::temp_dir();
    for _ in 0..128 {
        let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = root.join(format!(
            "solstone-journal-archive-{}-{stamp}-{sequence}.tmp",
            std::process::id()
        ));
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate an exclusive archive scratch file",
    ))
}
