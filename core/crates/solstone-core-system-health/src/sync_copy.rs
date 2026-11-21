// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Terminal-safe presentation of descriptor-bound sync scan results.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use solstone_core_journal_io::{FlatDirectoryError, JournalEntryKind};
use solstone_core_system::lifecycle::{
    HeartbeatClassification, SyncCheckResult, SyncIncompleteSnapshotReason, SyncScanFailure,
    SyncUnsafeReason, is_admission_wait_marker_filename_candidate,
};
#[cfg(any(unix, windows))]
use solstone_core_system::lifecycle::{SyncRescan, rescan_sync_read_only};
use solstone_core_system::process::ProcessInstanceSource;

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

/// Format the passive waiting state.
#[must_use]
pub fn format_admission_waiting_copy() -> String {
    admission_waiting_in(solstone_core_cli_copy::ENGLISH)
}

fn admission_waiting_in(locale: &str) -> String {
    if cfg!(unix) {
        solstone_core_cli_copy::render_for(locale, "journal.cli.doctor.sync.waiting", &[])
    } else {
        solstone_core_cli_copy::render_for(locale, "journal.cli.doctor.sync.unverifiable", &[])
    }
}

/// Format the locked terminal copy for a descriptor-bound sync scan failure.
#[must_use]
pub fn format_sync_scan_failure_copy(failure: &SyncScanFailure) -> String {
    format_sync_scan_failure_copy_in(failure, solstone_core_cli_copy::ENGLISH)
}

pub fn format_sync_scan_failure_copy_in(failure: &SyncScanFailure, locale: &str) -> String {
    match failure {
        SyncScanFailure::UnsafeEntry {
            folder,
            name,
            kind,
            reason,
            source,
        } => format_unsafe_named_entry(
            locale,
            folder,
            name,
            &entry_kind_description(*kind, locale),
            unsafe_reason(reason, source.as_deref(), locale),
        ),
        SyncScanFailure::IncompleteSnapshot {
            folder,
            name,
            reason,
        } => format_unsafe_named_entry(
            locale,
            folder,
            name,
            &solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.doctor.sync.entry.replaced",
                &[],
            ),
            incomplete_snapshot_reason(reason, locale),
        ),
        SyncScanFailure::DirectoryBinding { path, reason, .. } => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.doctor.sync.folder.unsafe",
                [
                    ("path", &(sanitize_path(path))),
                    ("reason", &(flat_directory_reason(reason.as_ref(), locale))),
                ],
            )
        }
        SyncScanFailure::CountCapExceeded { folder, .. } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.doctor.sync.folder.full",
            [("folder", &(sanitize_path(folder)))],
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
    describe_sync_rescan_in(
        journal,
        self_filename,
        now,
        process_source,
        solstone_core_cli_copy::ENGLISH,
    )
}

pub fn describe_sync_rescan_in(
    journal: &Path,
    self_filename: &str,
    now: f64,
    process_source: &dyn ProcessInstanceSource,
    locale: &str,
) -> SyncRescanDiagnosis {
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (journal, self_filename, now, process_source);
        return SyncRescanDiagnosis::Unsafe(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.doctor.sync.unsupported",
            &[],
        ));
    }
    #[cfg(any(unix, windows))]
    {
        match rescan_sync_read_only(journal, self_filename, None, now) {
            Ok(SyncRescan::Absent) => SyncRescanDiagnosis::Clean(None),
            Ok(SyncRescan::Complete(result)) => {
                diagnose_complete_rescan_in(locale, result, now, process_source)
            }
            Err(failure) if admission_wait_marker_scan_failure(&failure) => {
                SyncRescanDiagnosis::AdmissionWaitNeedsAttention(
                    solstone_core_cli_copy::render_for(
                        locale,
                        "journal.cli.doctor.sync.unverifiable",
                        &[],
                    ),
                )
            }
            Err(failure) => {
                SyncRescanDiagnosis::Unsafe(format_sync_scan_failure_copy_in(&failure, locale))
            }
        }
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

fn diagnose_complete_rescan_in(
    locale: &str,
    result: SyncCheckResult,
    now: f64,
    _process_source: &dyn ProcessInstanceSource,
) -> SyncRescanDiagnosis {
    let mut has_waiting_marker = false;
    let mut has_unverifiable_marker = false;
    for peer in &result.peer_observations {
        match &peer.classification {
            HeartbeatClassification::AdmissionWaitMarker(_) => {
                let Some(observation) = result.snapshot.files.get(&peer.source_filename) else {
                    has_unverifiable_marker = true;
                    continue;
                };
                let modified = observation.entry.mtime.seconds as f64
                    + observation.entry.mtime.nanoseconds as f64 / 1_000_000_000.0;
                let provably_stale = now.is_finite()
                    && modified.is_finite()
                    && modified <= now
                    && now - modified > solstone_core_system::lifecycle::FRESH_WINDOW_SECONDS;
                if !provably_stale {
                    // Diagnostics are read-only and do not possess a local writer
                    // identity or exact cleanup authority. Treat every non-proven-old
                    // marker as waiting instead of applying this host's PID table to
                    // a possibly foreign installation.
                    has_waiting_marker = true;
                }
            }
            HeartbeatClassification::AdmissionWaitMarkerIdentityMismatch(_)
            | HeartbeatClassification::AdmissionWaitMarkerMalformed => {
                has_unverifiable_marker = true;
            }
            _ => {}
        }
    }
    if has_unverifiable_marker {
        return SyncRescanDiagnosis::AdmissionWaitNeedsAttention(
            solstone_core_cli_copy::render_for(locale, "journal.cli.doctor.sync.unverifiable", &[]),
        );
    }
    if has_waiting_marker {
        return SyncRescanDiagnosis::Waiting(admission_waiting_in(locale));
    }
    if result.live_peer_observations.iter().any(|peer| {
        matches!(
            peer.classification,
            HeartbeatClassification::BoundedMalformed
                | HeartbeatClassification::IdentityMismatch(_)
        )
    }) {
        return SyncRescanDiagnosis::AdmissionWaitNeedsAttention(
            solstone_core_cli_copy::render_for(locale, "journal.cli.doctor.sync.unverifiable", &[]),
        );
    }
    if result.is_boot_conflict() {
        return SyncRescanDiagnosis::HeartbeatNeedsAttention(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.doctor.sync.heartbeat",
            &[],
        ));
    }
    SyncRescanDiagnosis::Clean(Some(result))
}

fn format_unsafe_named_entry(
    locale: &str,
    folder: &Path,
    name: &OsString,
    entry_type: &str,
    reason: String,
) -> String {
    let path = joined_path(folder, name);
    if name.to_str().is_none() {
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.doctor.sync.entry.unsafe.bytes",
            [
                ("path", &(sanitize_path(&path))),
                ("entry_type", &entry_type),
                ("reason", &reason),
                (
                    "bytes",
                    &(sanitize_os_bytes_for_terminal(name.as_encoded_bytes())),
                ),
            ],
        )
    } else {
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.doctor.sync.entry.unsafe",
            [
                ("path", &(sanitize_path(&path))),
                ("entry_type", &entry_type),
                ("reason", &reason),
            ],
        )
    }
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

fn unsafe_reason(
    reason: &SyncUnsafeReason,
    source: Option<&FlatDirectoryError>,
    locale: &str,
) -> String {
    let reason = match reason {
        SyncUnsafeReason::NonRegular { kind } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.doctor.sync.reason.nonregular",
            [("kind", &(format!("{kind:?}")))],
        ),
        SyncUnsafeReason::OversizedRegular { size, limit } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.doctor.sync.reason.oversized",
            [("size", &size), ("limit", &limit)],
        ),
        SyncUnsafeReason::Unreadable { .. } => {
            solstone_core_cli_copy::render_for(locale, "journal.cli.doctor.sync.reason.read", &[])
        }
    };
    match source {
        Some(source) => format!("{reason}: {}", flat_directory_reason(source, locale)),
        None => reason,
    }
}

fn incomplete_snapshot_reason(reason: &SyncIncompleteSnapshotReason, locale: &str) -> String {
    match reason {
        SyncIncompleteSnapshotReason::DisappearedDuringObservation => {
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.doctor.sync.reason.disappeared",
                &[],
            )
        }
        SyncIncompleteSnapshotReason::ReplacedDuringObservation { source } => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.doctor.sync.entry.changed",
                [("reason", &(flat_directory_reason(source.as_ref(), locale)))],
            )
        }
    }
}

fn flat_directory_reason(error: &FlatDirectoryError, locale: &str) -> String {
    match error {
        FlatDirectoryError::InvalidRelativePath { path, reason } => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.doctor.sync.path.invalid",
                [
                    ("path", &(sanitize_path(path))),
                    ("reason", &(sanitize_display(reason))),
                ],
            )
        }
        FlatDirectoryError::InvalidName { name, reason } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.doctor.sync.entry.invalid",
            [
                (
                    "entry",
                    &(sanitize_os_bytes_for_terminal(name.as_encoded_bytes())),
                ),
                ("reason", &(sanitize_display(reason))),
            ],
        ),
        FlatDirectoryError::NotDirectory { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.doctor.sync.directory.required",
            [("path", &(sanitize_path(path)))],
        ),
        FlatDirectoryError::SymlinkRefused { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.doctor.sync.symlink",
            [("path", &(sanitize_path(path)))],
        ),
        FlatDirectoryError::NotRegular { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.doctor.sync.file.required",
            [("path", &(sanitize_path(path)))],
        ),
        FlatDirectoryError::SizeLimitExceeded {
            path,
            kind,
            size,
            limit,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.doctor.sync.size",
            [
                ("path", &(sanitize_path(path))),
                ("kind", &(format!("{:?}", kind))),
                ("size", &size),
                ("limit", &limit),
            ],
        ),
        FlatDirectoryError::IdentityChanged { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.doctor.sync.identity.changed",
            [("path", &(sanitize_path(path)))],
        ),
        FlatDirectoryError::EnumerationChanged { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.doctor.sync.entry.vanished",
            [("path", &(sanitize_path(path)))],
        ),
        FlatDirectoryError::Io {
            operation,
            path,
            source,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.doctor.sync.io",
            [
                ("operation", &operation),
                ("path", &(sanitize_path(path))),
                ("reason", &(sanitize_display(source))),
            ],
        ),
    }
}

fn entry_kind_description(kind: JournalEntryKind, locale: &str) -> String {
    match kind {
        JournalEntryKind::RegularFile => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.doctor.sync.kind.regularfile",
            &[],
        ),
        JournalEntryKind::Directory => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.doctor.sync.kind.directory",
            &[],
        ),
        JournalEntryKind::Symlink => {
            solstone_core_cli_copy::render_for(locale, "journal.cli.doctor.sync.kind.symlink", &[])
        }
        JournalEntryKind::Fifo => {
            solstone_core_cli_copy::render_for(locale, "journal.cli.doctor.sync.kind.fifo", &[])
        }
        JournalEntryKind::Socket => {
            solstone_core_cli_copy::render_for(locale, "journal.cli.doctor.sync.kind.socket", &[])
        }
        JournalEntryKind::CharacterDevice => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.doctor.sync.kind.characterdevice",
            &[],
        ),
        JournalEntryKind::BlockDevice => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.doctor.sync.kind.blockdevice",
            &[],
        ),
        JournalEntryKind::Other => {
            solstone_core_cli_copy::render_for(locale, "journal.cli.doctor.sync.kind.other", &[])
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    use solstone_core_journal_io::{
        FileObservation, FlatDirectoryEntry, FlatDirectoryError, JournalEntryKind, NativeMtime,
    };
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
        SyncRescanDiagnosis, diagnose_complete_rescan_in, format_admission_waiting_copy,
        format_sync_scan_failure_copy,
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
        complete_with_mtime(classification, is_live, 100)
    }

    fn complete_with_mtime(
        classification: HeartbeatClassification,
        is_live: bool,
        mtime_seconds: i64,
    ) -> SyncCheckResult {
        let peer = SyncPeerObservation {
            source_filename: OsString::from("peer.check"),
            classification,
            heartbeat: None,
            is_live,
        };
        let observation = FileObservation {
            entry: FlatDirectoryEntry {
                name: peer.source_filename.clone(),
                kind: JournalEntryKind::RegularFile,
                device: 1,
                inode: 1,
                size: 0,
                mtime: NativeMtime {
                    seconds: mtime_seconds,
                    nanoseconds: 0,
                },
            },
            bytes: Vec::new(),
        };
        SyncCheckResult {
            snapshot: SyncSnapshot {
                files: std::collections::BTreeMap::from([(
                    peer.source_filename.clone(),
                    observation,
                )]),
            },
            peer_observations: vec![peer.clone()],
            live_peer_observations: if is_live { vec![peer] } else { Vec::new() },
        }
    }

    #[test]
    fn admission_wait_copy_blocks_are_exact() {
        assert_eq!(
            format_admission_waiting_copy(),
            "Installation: waiting\na recent heartbeat from another run was found.\nthe solstone app is waiting while it checks whether that activity continues. it will check again shortly."
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
                uid: 501,
                execution: ExecutionState::Running,
                ppid: Some(1),
                pgid: Some(7),
            },
        };
        assert!(matches!(
            diagnose_complete_rescan_in(
                solstone_core_cli_copy::ENGLISH,
                complete(HeartbeatClassification::AdmissionWaitMarker(marker), false),
                100.0,
                &source,
            ),
            SyncRescanDiagnosis::Waiting(_)
        ));
    }

    #[test]
    fn unverifiable_marker_dominates_a_live_wait_marker() {
        let marker = marker();
        let live = SyncPeerObservation {
            source_filename: OsString::from("live.check"),
            classification: HeartbeatClassification::AdmissionWaitMarker(marker.clone()),
            heartbeat: None,
            is_live: false,
        };
        let malformed = SyncPeerObservation {
            source_filename: OsString::from("malformed.check"),
            classification: HeartbeatClassification::AdmissionWaitMarkerMalformed,
            heartbeat: None,
            is_live: false,
        };
        let source = FakeProcessSource {
            result: InspectResult::Present {
                instance: marker.process,
                uid: 501,
                execution: ExecutionState::Running,
                ppid: Some(1),
                pgid: Some(7),
            },
        };
        assert!(matches!(
            diagnose_complete_rescan_in(
                solstone_core_cli_copy::ENGLISH,
                SyncCheckResult {
                    snapshot: SyncSnapshot::default(),
                    peer_observations: vec![live, malformed],
                    live_peer_observations: Vec::new(),
                },
                100.0,
                &source,
            ),
            SyncRescanDiagnosis::AdmissionWaitNeedsAttention(_)
        ));
    }

    #[test]
    fn fresh_marker_waits_without_using_the_local_process_table() {
        let marker = marker();
        let source = FakeProcessSource {
            result: InspectResult::Unverifiable,
        };
        assert!(matches!(
            diagnose_complete_rescan_in(
                solstone_core_cli_copy::ENGLISH,
                complete(HeartbeatClassification::AdmissionWaitMarker(marker), false),
                100.0,
                &source,
            ),
            SyncRescanDiagnosis::Waiting(_)
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
            diagnose_complete_rescan_in(
                solstone_core_cli_copy::ENGLISH,
                complete(HeartbeatClassification::SchemaV1(heartbeat), true),
                100.0,
                &source,
            ),
            SyncRescanDiagnosis::HeartbeatNeedsAttention(_)
        ));
        assert!(matches!(
            diagnose_complete_rescan_in(
                solstone_core_cli_copy::ENGLISH,
                complete(HeartbeatClassification::BoundedMalformed, true),
                100.0,
                &source,
            ),
            SyncRescanDiagnosis::AdmissionWaitNeedsAttention(_)
        ));
    }

    #[test]
    fn stale_marker_is_passively_ignored_and_malformed_marker_needs_attention() {
        let source = FakeProcessSource {
            result: InspectResult::Absent,
        };
        assert!(matches!(
            diagnose_complete_rescan_in(
                solstone_core_cli_copy::ENGLISH,
                complete_with_mtime(
                    HeartbeatClassification::AdmissionWaitMarker(marker()),
                    false,
                    39,
                ),
                100.0,
                &source,
            ),
            SyncRescanDiagnosis::Clean(_)
        ));
        assert!(matches!(
            diagnose_complete_rescan_in(
                solstone_core_cli_copy::ENGLISH,
                complete(HeartbeatClassification::AdmissionWaitMarkerMalformed, false),
                100.0,
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
