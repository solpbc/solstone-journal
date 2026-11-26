// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows create-only archive publication.
//!
//! Archive bytes are encoded directly into journal-io's single exclusively
//! created stage handle beneath the admitted parent, flushed, revalidated, and
//! published create-only via `MoveFileExW(..., 0)`. Writing directly into the
//! publication stage avoids a second scratch copy on the destination volume.
//! On any encoding, target, or publication failure, the held stage handle is
//! marked for deletion before returning and the destination name is never
//! created or overwritten.

use std::fmt::{Display, Formatter};

use solstone_core_journal_io::{AtomicWriteOptions, ExclusiveWriteError, write_file_exclusive};

use crate::encode::{
    EncodeArchiveError, EncodeArchiveRequest, EncodeOptions, encode_archive_with_options,
};
use crate::windows_target::{ArchiveOutputTarget, ExplicitTargetError};

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
/// single-stage create-only publication protocol.
pub fn publish_archive(
    target: &ArchiveOutputTarget,
    request: &EncodeArchiveRequest<'_>,
) -> Result<(), ArchivePublicationError> {
    publish_archive_with_options(target, request, EncodeOptions::default())
}

/// Encode with options and atomically create the final archive through the
/// Windows single-stage create-only publication protocol.
pub fn publish_archive_with_options(
    target: &ArchiveOutputTarget,
    request: &EncodeArchiveRequest<'_>,
    options: EncodeOptions<'_>,
) -> Result<(), ArchivePublicationError> {
    target
        .revalidate()
        .map_err(ArchivePublicationError::Target)?;
    match write_file_exclusive(
        target.final_path(),
        AtomicWriteOptions { mode: Some(0o600) },
        |stage| {
            encode_archive_with_options(request, stage, options)
                .map_err(ArchivePublicationError::Encode)
        },
        || target.revalidate().map_err(ArchivePublicationError::Target),
    ) {
        Ok(()) => Ok(()),
        Err(ExclusiveWriteError::Write(error)) => Err(error),
        Err(ExclusiveWriteError::WriteCleanup { cleanup, .. }) => {
            Err(ArchivePublicationError::Cleanup(cleanup))
        }
        Err(ExclusiveWriteError::Publish(error)) => Err(ArchivePublicationError::Publish(error)),
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::io::Read;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::windows_target::{ExplicitArchiveOutputRequest, acquire_explicit_output_target};
    use crate::{ArchiveError, ArchiveSource};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "solstone-win-publish-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create temp dir");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn single_stage_publish_succeeds_preserves_existing_and_cleans_stage_on_failure() {
        let temp = TempDir::new("single-stage");
        let journal = temp.0.join("journal");
        let out_dir = temp.0.join("out");
        fs::create_dir_all(journal.join("imports/item")).unwrap();
        fs::create_dir(&out_dir).unwrap();
        fs::write(journal.join("imports/item/data.bin"), b"hello-archive").unwrap();

        let source = ArchiveSource::open(&journal).unwrap();
        let request = EncodeArchiveRequest {
            source: &source,
            solstone_version: "2.0.91",
            exported_at: "2026-11-26T00:00:00Z",
            day_window: None,
        };
        let final_path = out_dir.join("out.zip");
        let target = acquire_explicit_output_target(&ExplicitArchiveOutputRequest::new(
            final_path.clone(),
            out_dir.clone(),
        ))
        .unwrap();
        publish_archive(&target, &request).unwrap();
        assert_eq!(fs::read_dir(&out_dir).unwrap().count(), 1);
        let mut archive = zip::ZipArchive::new(fs::File::open(&final_path).unwrap()).unwrap();
        let mut content = Vec::new();
        archive
            .by_name("imports/item/data.bin")
            .unwrap()
            .read_to_end(&mut content)
            .unwrap();
        assert_eq!(content, b"hello-archive");
        drop(archive);

        // Mutating the source after inventory must abort encode, leave no new final archive,
        // and clean up the single stage file.
        let aborted_path = out_dir.join("aborted.zip");
        let aborted_target = acquire_explicit_output_target(&ExplicitArchiveOutputRequest::new(
            aborted_path.clone(),
            out_dir.clone(),
        ))
        .unwrap();
        fs::write(journal.join("imports/item/data.bin"), b"changed").unwrap();
        let err = publish_archive(&aborted_target, &request).unwrap_err();
        assert!(matches!(
            err,
            ArchivePublicationError::Encode(EncodeArchiveError::Source(
                ArchiveError::SourceChanged { .. }
            ))
        ));
        assert!(!aborted_path.exists());
        assert_eq!(fs::read_dir(&out_dir).unwrap().count(), 1);

        // Pre-existing destination must be preserved untouched.
        let existing_path = out_dir.join("existing.zip");
        fs::write(&existing_path, b"keep-existing").unwrap();
        let collision = acquire_explicit_output_target(&ExplicitArchiveOutputRequest::new(
            existing_path.clone(),
            out_dir.clone(),
        ));
        assert!(matches!(
            collision,
            Err(ExplicitTargetError::Collision { .. })
        ));
        assert_eq!(fs::read(&existing_path).unwrap(), b"keep-existing");
    }
}
