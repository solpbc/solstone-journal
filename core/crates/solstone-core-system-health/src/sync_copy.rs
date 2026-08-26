// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Terminal-safe presentation of descriptor-bound sync scan results.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use solstone_core_journal_io::{FlatDirectoryError, JournalEntryKind};
use solstone_core_system::lifecycle::{
    ADMISSION_WAIT_TRANSIENT_COPY, HeartbeatClassification, SyncCheckResult,
    SyncIncompleteSnapshotReason, SyncRescan, SyncScanFailure, SyncUnsafeReason,
    is_admission_wait_marker_filename_candidate, rescan_sync_read_only,
};
use solstone_core_system::process::{InstanceVerdict, ProcessInstanceSource};

use crate::sanitize_os_bytes_for_terminal;

/// The presentation-level conclusion of a read-only sync rescan.
#[derive(Debug)]
pub enum SyncRescanDiagnosis {
    /// No scan failure or live foreign writer was found. `None` means the
    /// `health/sync` directory was absent.
    Clean(Option<SyncCheckResult>),
    /// A live admitting process retained an admission-wait marker.
    Waiting(String),
    /// A live heartbeat exists without proof that an admitting process is
    /// currently making the bounded start attempt.
    HeartbeatNeedsAttention(String),
    /// An admission-wait marker cannot be safely trusted.
    AdmissionWaitNeedsAttention(String),
    /// A safe complete scan could not be obtained.
    Unsafe(String),
}

pub const HEARTBEAT_WITHOUT_WAIT_MARKER_COPY: &str = "Installation: needs attention\na recent heartbeat from another run is present.\nwait a moment, then try again.";
pub const ADMISSION_WAIT_UNVERIFIABLE_COPY: &str = "Installation: needs attention\nstartup status couldn't be verified.\nwait a moment, then try again.";

/// Format the passive waiting state from the lifecycle-owned transient copy.
#[must_use]
pub fn format_admission_waiting_copy() -> String {
    format!("Installation: waiting\n{ADMISSION_WAIT_TRANSIENT_COPY}")
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
    now: f64,
    process_source: &dyn ProcessInstanceSource,
) -> SyncRescanDiagnosis {
    match rescan_sync_read_only(journal, self_filename, None, now) {
        Ok(SyncRescan::Absent) => SyncRescanDiagnosis::Clean(None),
        Ok(SyncRescan::Complete(result)) => diagnose_complete_rescan(result, process_source),
        Err(failure) if admission_wait_marker_scan_failure(&failure) => {
            SyncRescanDiagnosis::AdmissionWaitNeedsAttention(
                ADMISSION_WAIT_UNVERIFIABLE_COPY.to_owned(),
            )
        }
        Err(failure) => SyncRescanDiagnosis::Unsafe(format_sync_scan_failure_copy(&failure)),
    }
}

fn admission_wait_marker_scan_failure(failure: &SyncScanFailure) -> bool {
    let name = match failure {
        SyncScanFailure::UnsafeEntry { name, .. }
        | SyncScanFailure::IncompleteSnapshot { name, .. } => name,
        SyncScanFailure::DirectoryBinding { .. } | SyncScanFailure::CountCapExceeded { .. } => {
            return false;
        }
    };
    is_admission_wait_marker_filename_candidate(name)
}

fn diagnose_complete_rescan(
    result: SyncCheckResult,
    process_source: &dyn ProcessInstanceSource,
) -> SyncRescanDiagnosis {
    let mut has_waiting_marker = false;
    let mut has_unverifiable_marker = false;
    for peer in &result.peer_observations {
        match &peer.classification {
            HeartbeatClassification::AdmissionWaitMarker(marker) => {
                match process_source.observe(&marker.process) {
                    InstanceVerdict::SameLive { .. } => has_waiting_marker = true,
                    InstanceVerdict::Unverifiable => has_unverifiable_marker = true,
                    InstanceVerdict::NotSameOrExited => {}
                }
            }
            HeartbeatClassification::AdmissionWaitMarkerIdentityMismatch(_)
            | HeartbeatClassification::AdmissionWaitMarkerMalformed => {
                has_unverifiable_marker = true;
            }
            _ => {}
        }
    }
    if has_waiting_marker {
        return SyncRescanDiagnosis::Waiting(format_admission_waiting_copy());
    }
    if has_unverifiable_marker {
        return SyncRescanDiagnosis::AdmissionWaitNeedsAttention(
            ADMISSION_WAIT_UNVERIFIABLE_COPY.to_owned(),
        );
    }
    if result.is_boot_conflict() {
        return SyncRescanDiagnosis::HeartbeatNeedsAttention(
            HEARTBEAT_WITHOUT_WAIT_MARKER_COPY.to_owned(),
        );
    }
    SyncRescanDiagnosis::Clean(Some(result))
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
        AdmissionWaitMarker, AdmissionWaitReason, Heartbeat, HeartbeatClassification, RunId,
        SyncCheckResult, SyncDirectoryOperation, SyncIncompleteSnapshotReason, SyncPeerObservation,
        SyncScanFailure, SyncSnapshot, SyncUnsafeReason, WriterId,
    };
    use solstone_core_system::process::{
        ExecutionState, InspectResult, InstanceCensus, ProcessBirth, ProcessInstance,
        ProcessInstanceSource,
    };

    use super::{
        ADMISSION_WAIT_UNVERIFIABLE_COPY, HEARTBEAT_WITHOUT_WAIT_MARKER_COPY, SyncRescanDiagnosis,
        diagnose_complete_rescan, format_admission_waiting_copy, format_sync_scan_failure_copy,
    };

    struct FakeProcessSource {
        result: InspectResult,
    }

    impl ProcessInstanceSource for FakeProcessSource {
        fn inspect(&self, _pid: u32) -> InspectResult {
            self.result
        }

        fn census(&self) -> InstanceCensus {
            InstanceCensus::Incomplete(Vec::new())
        }
    }

    fn writer_id() -> WriterId {
        WriterId::parse("0123456789abcdef0123456789abcdef").expect("writer ID")
    }

    fn run_id() -> RunId {
        RunId::parse("fedcba9876543210fedcba9876543210").expect("run ID")
    }

    fn marker() -> AdmissionWaitMarker {
        AdmissionWaitMarker::new(
            writer_id(),
            run_id(),
            ProcessInstance {
                pid: 7,
                birth: ProcessBirth::linux(10, 100, 100),
            },
            AdmissionWaitReason::FreshNonSelfHeartbeat,
        )
    }

    fn complete(classification: HeartbeatClassification, is_live: bool) -> SyncCheckResult {
        let peer = SyncPeerObservation {
            source_filename: OsString::from("peer.check"),
            classification,
            heartbeat: None,
            is_live,
        };
        SyncCheckResult {
            snapshot: SyncSnapshot::default(),
            peer_observations: vec![peer.clone()],
            live_peer_observations: if is_live { vec![peer] } else { Vec::new() },
        }
    }

    #[test]
    fn admission_wait_copy_blocks_are_exact() {
        assert_eq!(
            HEARTBEAT_WITHOUT_WAIT_MARKER_COPY,
            "Installation: needs attention\na recent heartbeat from another run is present.\nwait a moment, then try again."
        );
        assert_eq!(
            ADMISSION_WAIT_UNVERIFIABLE_COPY,
            "Installation: needs attention\nstartup status couldn't be verified.\nwait a moment, then try again."
        );
        assert_eq!(
            format_admission_waiting_copy(),
            "Installation: waiting\na recent heartbeat from another run is present.\nsolstone is waiting to protect your journal. it should clear on its own shortly."
        );
    }

    #[test]
    fn unreadable_marker_scan_failure_is_unverifiable_not_generic_unsafe() {
        let failure = SyncScanFailure::IncompleteSnapshot {
            folder: PathBuf::from("journal/health/sync"),
            name: OsString::from("solstone-wait-v2-invalid.check"),
            reason: SyncIncompleteSnapshotReason::DisappearedDuringObservation,
        };
        assert!(super::admission_wait_marker_scan_failure(&failure));
    }

    #[test]
    fn live_marker_dominates_other_sync_evidence() {
        let marker = marker();
        let source = FakeProcessSource {
            result: InspectResult::Present {
                instance: marker.process,
                execution: ExecutionState::Running,
                ppid: Some(1),
                pgid: Some(7),
            },
        };
        assert!(matches!(
            diagnose_complete_rescan(
                complete(HeartbeatClassification::AdmissionWaitMarker(marker), false),
                &source,
            ),
            SyncRescanDiagnosis::Waiting(_)
        ));
    }

    #[test]
    fn unverifiable_marker_and_uncorroborated_heartbeat_need_attention() {
        let marker = marker();
        let source = FakeProcessSource {
            result: InspectResult::Unverifiable,
        };
        assert!(matches!(
            diagnose_complete_rescan(
                complete(HeartbeatClassification::AdmissionWaitMarker(marker), false),
                &source,
            ),
            SyncRescanDiagnosis::AdmissionWaitNeedsAttention(_)
        ));

        let heartbeat = Heartbeat {
            schema: 1,
            machine_id: "legacy".to_owned(),
            hostname: "peer".to_owned(),
            pid: 7,
            wall_time: "now".to_owned(),
            solstone_version: "test".to_owned(),
            interval_seconds: 15,
            journal_path: "/journal".to_owned(),
        };
        assert!(matches!(
            diagnose_complete_rescan(
                complete(HeartbeatClassification::SchemaV1(heartbeat), true),
                &source,
            ),
            SyncRescanDiagnosis::HeartbeatNeedsAttention(_)
        ));
    }

    #[test]
    fn stale_marker_is_passively_ignored_and_malformed_marker_needs_attention() {
        let source = FakeProcessSource {
            result: InspectResult::Absent,
        };
        assert!(matches!(
            diagnose_complete_rescan(
                complete(
                    HeartbeatClassification::AdmissionWaitMarker(marker()),
                    false
                ),
                &source,
            ),
            SyncRescanDiagnosis::Clean(_)
        ));
        assert!(matches!(
            diagnose_complete_rescan(
                complete(HeartbeatClassification::AdmissionWaitMarkerMalformed, false),
                &source,
            ),
            SyncRescanDiagnosis::AdmissionWaitNeedsAttention(_)
        ));
    }

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
