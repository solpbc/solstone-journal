// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata},
    io::Read,
    path::Path,
};

pub(crate) const CUSTODY_CANCELLED: &str = "custody verification cancelled";

/// Publication before receipt persistence is not a completed network input.
/// Match the accepted custody receipt before giving these bytes to a processor.
pub(crate) fn accepted_input(root: &Path, input: &Path) -> Result<bool, String> {
    Ok(AcceptedInput::capture(root, input, &|| false)?.is_some())
}

/// The receipt still names this input. This does not read or hash the recording.
pub(crate) fn receipt_still_names(root: &Path, input: &Path) -> Result<bool, String> {
    Ok(matches!(
        accepted_receipt_files(root, input)?,
        ReceiptFiles::Present(_)
    ))
}

enum ReceiptFiles {
    Absent,
    Present(Vec<(u64, String)>),
}

fn accepted_receipt_files(root: &Path, input: &Path) -> Result<ReceiptFiles, String> {
    let segment = input.parent().ok_or("input segment missing")?;
    let relative = segment
        .strip_prefix(root.join("chronicle"))
        .map_err(|e| e.to_string())?;
    let parts = relative
        .iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>();
    if parts.len() != 3 {
        return Ok(ReceiptFiles::Absent);
    }
    let report =
        solstone_core_callosum::read_device_ingest_events(segment).map_err(|e| e.to_string())?;
    if report.unparseable != 0 {
        return Err(format!("unreadable custody receipt: {}", segment.display()));
    }
    let name = input
        .file_name()
        .ok_or("input filename missing")?
        .to_string_lossy();
    let files = report
        .records
        .iter()
        .filter(|r| {
            r.outcome == "accepted"
                && r.day == parts[0]
                && r.stream == parts[1]
                && r.segment == parts[2]
        })
        .flat_map(|r| &r.files)
        .filter(|f| f.written == name)
        .map(|f| (f.size, f.sha256.clone()))
        .collect::<Vec<_>>();
    if files.is_empty() {
        Ok(ReceiptFiles::Absent)
    } else {
        Ok(ReceiptFiles::Present(files))
    }
}

/// Retain the admitted inode and compare cheap file facts between work units.
/// Exact bytes are checked once here and again by the processor at publication.
pub(crate) struct AcceptedInput {
    _file: File,
    stamp: FileStamp,
}

#[derive(PartialEq, Eq)]
struct FileStamp {
    size: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

fn stamp(metadata: &Metadata) -> Result<FileStamp, String> {
    if !metadata.is_file() {
        return Err("input is not a regular file".into());
    }
    Ok(FileStamp {
        size: metadata.len(),
        modified: metadata.modified().map_err(|e| e.to_string())?,
        #[cfg(unix)]
        identity: {
            use std::os::unix::fs::MetadataExt;
            (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            )
        },
    })
}

impl AcceptedInput {
    pub(crate) fn still_current(&self, input: &Path) -> bool {
        if !fs::symlink_metadata(input)
            .ok()
            .filter(|m| m.file_type().is_file())
            .and_then(|m| stamp(&m).ok())
            .is_some_and(|current| current == self.stamp)
        {
            return false;
        }
        #[cfg(windows)]
        {
            let Ok(current) = File::open(input) else {
                return false;
            };
            let (Ok(current), Ok(admitted)) = (
                solstone_core_journal_io::windows_file_identity(&current),
                solstone_core_journal_io::windows_file_identity(&self._file),
            ) else {
                return false;
            };
            if current != admitted {
                return false;
            }
        }
        true
    }

    pub(crate) fn capture(
        root: &Path,
        input: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Self>, String> {
        let ReceiptFiles::Present(files) = accepted_receipt_files(root, input)? else {
            return Ok(None);
        };
        let named = fs::symlink_metadata(input).map_err(|e| e.to_string())?;
        if !named.file_type().is_file() {
            return Err("input is not a regular file".into());
        }
        #[cfg(unix)]
        let mut file = File::open(input).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let mut file = {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_SHARE_READ | FILE_SHARE_DELETE; accepted immutable bytes
            // cannot be changed by a writer with a deferred last-write time.
            fs::OpenOptions::new()
                .read(true)
                .share_mode(0x00000001 | 0x00000004)
                .open(input)
                .map_err(|e| e.to_string())?
        };
        let admitted = stamp(&file.metadata().map_err(|e| e.to_string())?)?;
        if stamp(&named)? != admitted {
            return Err("input changed while opening".into());
        }
        let size = admitted.size;
        // The processing owner records execution-budget refusals durably. Custody
        // still verifies the entire accepted receipt with a constant-size buffer.
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("input is not a regular file".into());
        }
        let mut hash = Sha256::new();
        let mut bytes = [0u8; 65536];
        loop {
            if cancelled() {
                return Err(CUSTODY_CANCELLED.into());
            }
            let n = file.read(&mut bytes).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hash.update(&bytes[..n]);
        }
        let hash = format!("{:x}", hash.finalize());
        let accepted = Self {
            _file: file,
            stamp: admitted,
        };
        if cancelled() || !accepted.still_current(input) {
            return Err("input changed during custody verification".into());
        }
        Ok(files
            .iter()
            .any(|file| file.0 == size && file.1 == hash)
            .then_some(accepted))
    }
}
