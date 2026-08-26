// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Terminal-safe presentation of descriptor-bound sync scan results.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use solstone_core_journal_io::{FlatDirectoryError, JournalEntryKind};
use solstone_core_system::lifecycle::{
    SyncCheckResult, SyncIncompleteSnapshotReason, SyncRescan, SyncScanFailure, SyncUnsafeReason,
    format_conflict_message, rescan_sync_read_only,
};

use crate::sanitize_os_bytes_for_terminal;

/// The presentation-level conclusion of a read-only sync rescan.
#[derive(Debug)]
pub enum SyncRescanDiagnosis {
    /// No scan failure or live foreign writer was found. `None` means the
    /// `health/sync` directory was absent.
    Clean(Option<SyncCheckResult>),
    /// A complete scan found a live foreign writer.
    Conflict(String),
    /// A safe complete scan could not be obtained.
    Unsafe(String),
}

/// Format the locked terminal copy for a descriptor-bound sync scan failure.
#[must_use]
pub fn format_sync_scan_failure_copy(failure: &SyncScanFailure) -> String {
    match failure {
        SyncScanFailure::UnsafeEntry {
            folder,
            name,
            kind,
            reason,
            source,
        } => format_unsafe_named_entry(
            folder,
            name,
            entry_kind_description(*kind),
            unsafe_reason(reason, source.as_deref()),
        ),
        SyncScanFailure::IncompleteSnapshot {
            folder,
            name,
            reason,
        } => format_unsafe_named_entry(
            folder,
            name,
            "changed or disappeared during inspection",
            incomplete_snapshot_reason(reason),
        ),
        SyncScanFailure::DirectoryBinding { path, reason, .. } => format!(
            "Installation: needs attention\n\
             part of your journal can't be checked safely.\n\
             check the path named under details. it needs to be an ordinary folder inside your journal that you can open before you try again.\n\n\
             details:\n\
             path: {}\n\
             reason: {}",
            sanitize_path(path),
            flat_directory_reason(reason.as_ref()),
        ),
        SyncScanFailure::CountCapExceeded { folder, .. } => format!(
            "Installation: needs attention\n\
             your journal has too many items to check safely.\n\
             review the items in your journal's `health/sync` folder. move only items you know do not belong there until 256 or fewer remain, then try again.\n\n\
             details:\n\
             folder: {}\n\
             items found: more than 256\n\
             maximum checked: 256",
            sanitize_path(folder),
        ),
    }
}

/// Complete one read-only sync rescan and classify it for presentation callers.
#[must_use]
pub fn describe_sync_rescan(
    journal: &Path,
    self_filename: &str,
    machine_id: &str,
    now: f64,
) -> SyncRescanDiagnosis {
    match rescan_sync_read_only(journal, self_filename, machine_id, None, now) {
        Ok(SyncRescan::Absent) => SyncRescanDiagnosis::Clean(None),
        Ok(SyncRescan::Complete(result)) if result.is_boot_conflict() => {
            SyncRescanDiagnosis::Conflict(format_conflict_message(&result))
        }
        Ok(SyncRescan::Complete(result)) => SyncRescanDiagnosis::Clean(Some(result)),
        Err(failure) => SyncRescanDiagnosis::Unsafe(format_sync_scan_failure_copy(&failure)),
    }
}

fn format_unsafe_named_entry(
    folder: &Path,
    name: &OsString,
    entry_type: &str,
    reason: String,
) -> String {
    let path = joined_path(folder, name);
    let mut output = format!(
        "Installation: needs attention\n\
         your journal contains an item that can't be checked safely.\n\
         move the item named under details out of your journal's `health/sync` folder, then try again.\n\n\
         details:\n\
         path: {}\n\
         type: {entry_type}\n\
         reason: {reason}",
        sanitize_path(&path),
    );
    if name.to_str().is_none() {
        let _ = write!(
            output,
            "\nname bytes: {}",
            sanitize_os_bytes_for_terminal(name.as_encoded_bytes())
        );
    }
    output
}

fn joined_path(folder: &Path, name: &OsString) -> PathBuf {
    let mut path = folder.to_path_buf();
    path.push(name);
    path
}

fn sanitize_path(path: &Path) -> String {
    sanitize_os_bytes_for_terminal(path.as_os_str().as_encoded_bytes())
}

fn sanitize_display(value: &impl std::fmt::Display) -> String {
    sanitize_os_bytes_for_terminal(value.to_string().as_bytes())
}

fn unsafe_reason(reason: &SyncUnsafeReason, source: Option<&FlatDirectoryError>) -> String {
    let reason = sanitize_display(reason);
    match source {
        Some(source) => format!("{reason}: {}", flat_directory_reason(source)),
        None => reason,
    }
}

fn incomplete_snapshot_reason(reason: &SyncIncompleteSnapshotReason) -> String {
    match reason {
        SyncIncompleteSnapshotReason::DisappearedDuringObservation => {
            "entry disappeared during observation".to_owned()
        }
        SyncIncompleteSnapshotReason::ReplacedDuringObservation { source } => format!(
            "entry changed during observation: {}",
            flat_directory_reason(source.as_ref())
        ),
    }
}

fn flat_directory_reason(error: &FlatDirectoryError) -> String {
    match error {
        FlatDirectoryError::InvalidRelativePath { path, reason } => format!(
            "invalid flat-directory path {}: {}",
            sanitize_path(path),
            sanitize_display(reason)
        ),
        FlatDirectoryError::InvalidName { name, reason } => format!(
            "invalid flat-directory entry {}: {}",
            sanitize_os_bytes_for_terminal(name.as_encoded_bytes()),
            sanitize_display(reason)
        ),
        FlatDirectoryError::NotDirectory { path } => format!(
            "flat-directory descendant is not a directory: {}",
            sanitize_path(path)
        ),
        FlatDirectoryError::SymlinkRefused { path } => format!(
            "flat-directory descendant is a symlink: {}",
            sanitize_path(path)
        ),
        FlatDirectoryError::NotRegular { path } => format!(
            "flat-directory entry is not a regular file: {}",
            sanitize_path(path)
        ),
        FlatDirectoryError::SizeLimitExceeded {
            path,
            kind,
            size,
            limit,
        } => format!(
            "flat-directory entry exceeds observed-read limit: {} is {kind:?}, {size} bytes exceeds {limit}",
            sanitize_path(path)
        ),
        FlatDirectoryError::IdentityChanged { path } => {
            format!("flat-directory identity changed: {}", sanitize_path(path))
        }
        FlatDirectoryError::EnumerationChanged { path } => format!(
            "flat-directory entry vanished while listing: {}",
            sanitize_path(path)
        ),
        FlatDirectoryError::Io {
            operation,
            path,
            source,
        } => format!(
            "{operation} failed for {}: {}",
            sanitize_path(path),
            sanitize_display(source)
        ),
    }
}

fn entry_kind_description(kind: JournalEntryKind) -> &'static str {
    match kind {
        JournalEntryKind::RegularFile => "regular file",
        JournalEntryKind::Directory => "directory",
        JournalEntryKind::Symlink => "symbolic link",
        JournalEntryKind::Fifo => "named pipe",
        JournalEntryKind::Socket => "Unix socket",
        JournalEntryKind::CharacterDevice => "character device",
        JournalEntryKind::BlockDevice => "block device",
        JournalEntryKind::Other => "other filesystem object",
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    use solstone_core_journal_io::{FlatDirectoryError, JournalEntryKind};
    use solstone_core_system::lifecycle::{
        SyncDirectoryOperation, SyncIncompleteSnapshotReason, SyncScanFailure, SyncUnsafeReason,
    };

    use super::format_sync_scan_failure_copy;

    #[test]
    fn unsafe_entry_copy_is_exact_and_preserves_non_utf8_name_bytes() {
        let failure = SyncScanFailure::UnsafeEntry {
            folder: PathBuf::from("journal/health/sync"),
            name: OsString::from_vec(b"bad\xffname".to_vec()),
            kind: JournalEntryKind::Directory,
            reason: SyncUnsafeReason::NonRegular {
                kind: JournalEntryKind::Directory,
            },
            source: None,
        };

        assert_eq!(
            format_sync_scan_failure_copy(&failure),
            "Installation: needs attention\nyour journal contains an item that can't be checked safely.\nmove the item named under details out of your journal's `health/sync` folder, then try again.\n\ndetails:\npath: journal/health/sync/bad\\xffname\ntype: directory\nreason: not a regular file (Directory)\nname bytes: bad\\xffname"
        );
    }

    #[test]
    fn directory_binding_copy_is_exact() {
        let failure = SyncScanFailure::DirectoryBinding {
            path: PathBuf::from("journal/health/sync"),
            operation: SyncDirectoryOperation::BindSync,
            reason: Box::new(FlatDirectoryError::NotDirectory {
                path: PathBuf::from("journal/health/sync"),
            }),
        };

        assert_eq!(
            format_sync_scan_failure_copy(&failure),
            "Installation: needs attention\npart of your journal can't be checked safely.\ncheck the path named under details. it needs to be an ordinary folder inside your journal that you can open before you try again.\n\ndetails:\npath: journal/health/sync\nreason: flat-directory descendant is not a directory: journal/health/sync"
        );
    }

    #[test]
    fn flat_directory_reason_preserves_non_utf8_path_bytes() {
        let raw_path = PathBuf::from(OsString::from_vec(b"journal/health/\xffsync".to_vec()));
        let failure = SyncScanFailure::DirectoryBinding {
            path: raw_path.clone(),
            operation: SyncDirectoryOperation::BindSync,
            reason: Box::new(FlatDirectoryError::NotDirectory { path: raw_path }),
        };

        let copy = format_sync_scan_failure_copy(&failure);
        assert!(copy.contains("path: journal/health/\\xffsync"));
        assert!(copy.contains(
            "reason: flat-directory descendant is not a directory: journal/health/\\xffsync"
        ));
        assert!(!copy.contains('\u{fffd}'));
    }

    #[test]
    fn unsafe_entry_source_preserves_non_utf8_path_bytes() {
        let raw_path = PathBuf::from(OsString::from_vec(b"journal/health/\xffsync/peer".to_vec()));
        let failure = SyncScanFailure::UnsafeEntry {
            folder: PathBuf::from("journal/health/sync"),
            name: OsString::from("peer"),
            kind: JournalEntryKind::RegularFile,
            reason: SyncUnsafeReason::Unreadable {
                operation:
                    solstone_core_system::lifecycle::SyncReadOperation::ReadObservedFileBounded,
            },
            source: Some(Box::new(FlatDirectoryError::NotRegular { path: raw_path })),
        };

        let copy = format_sync_scan_failure_copy(&failure);
        assert!(copy.contains("journal/health/\\xffsync/peer"));
        assert!(!copy.contains('\u{fffd}'));
    }

    #[test]
    fn count_cap_copy_uses_locked_literals() {
        let failure = SyncScanFailure::CountCapExceeded {
            folder: PathBuf::from("journal/health/sync"),
            found_more_than: 999,
            maximum: 999,
        };

        assert_eq!(
            format_sync_scan_failure_copy(&failure),
            "Installation: needs attention\nyour journal has too many items to check safely.\nreview the items in your journal's `health/sync` folder. move only items you know do not belong there until 256 or fewer remain, then try again.\n\ndetails:\nfolder: journal/health/sync\nitems found: more than 256\nmaximum checked: 256"
        );
    }

    #[test]
    fn incomplete_snapshot_uses_unsafe_named_entry_copy() {
        let failure = SyncScanFailure::IncompleteSnapshot {
            folder: PathBuf::from("journal/health/sync"),
            name: OsString::from("peer.check"),
            reason: SyncIncompleteSnapshotReason::DisappearedDuringObservation,
        };

        assert!(format_sync_scan_failure_copy(&failure).contains(
            "type: changed or disappeared during inspection\nreason: entry disappeared during observation"
        ));
    }
}
