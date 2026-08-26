// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    FileObservation, FlatDirectory, FlatDirectoryError, JournalEntryKind, JournalRoot,
    JournalRootError, list_flat_directory, open_flat_directory_bound, read_observed_file_bounded,
};

pub const DEFAULT_INTERVAL_SECONDS: f64 = 15.0;
pub const FRESH_WINDOW_MULTIPLIER: f64 = 4.0;
const FRESH_WINDOW_SECONDS: f64 = FRESH_WINDOW_MULTIPLIER * DEFAULT_INTERVAL_SECONDS;

pub const HEARTBEAT_SCHEMA_V1: u8 = 1;

/// Heartbeat-control-plane limits. Do not reuse these for unrelated data.
pub const MAX_SYNC_DIRECTORY_ENTRIES: usize = 256;
/// Heartbeat-control-plane limits. Do not reuse these for unrelated data.
pub const MAX_SYNC_HEARTBEAT_BYTES: usize = 16_384;

// `FlatDirectory` intentionally keeps its full diagnostic path private. This
// relative spelling is presentation metadata only; it is never opened.
const SYNC_FOLDER_DIAGNOSTIC: &str = "health/sync";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Heartbeat {
    pub schema: u8,
    pub machine_id: String,
    pub hostname: String,
    pub pid: u32,
    pub wall_time: String,
    pub solstone_version: String,
    pub interval_seconds: u32,
    pub journal_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeartbeatClassification {
    SchemaV1(Heartbeat),
    UnknownFuture(Heartbeat),
    BoundedMalformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncReadOperation {
    ReadObservedFileBounded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncUnsafeReason {
    NonRegular { kind: JournalEntryKind },
    OversizedRegular { size: u64, limit: usize },
    Unreadable { operation: SyncReadOperation },
}

#[derive(Debug)]
pub enum SyncIncompleteSnapshotReason {
    DisappearedDuringObservation,
    ReplacedDuringObservation { source: Box<FlatDirectoryError> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDirectoryOperation {
    BindHealth,
    BindSync,
    InspectSync,
}

#[derive(Debug)]
pub enum SyncScanFailure {
    DirectoryBinding {
        path: PathBuf,
        operation: SyncDirectoryOperation,
        reason: Box<FlatDirectoryError>,
    },
    CountCapExceeded {
        folder: PathBuf,
        found_more_than: usize,
        maximum: usize,
    },
    UnsafeEntry {
        folder: PathBuf,
        name: OsString,
        kind: JournalEntryKind,
        reason: SyncUnsafeReason,
        source: Option<Box<FlatDirectoryError>>,
    },
    IncompleteSnapshot {
        folder: PathBuf,
        name: OsString,
        reason: SyncIncompleteSnapshotReason,
    },
}

impl fmt::Display for SyncScanFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DirectoryBinding {
                path,
                operation,
                reason,
            } => write!(
                formatter,
                "could not {operation} sync directory {}: {reason}",
                path.display()
            ),
            Self::CountCapExceeded {
                folder,
                found_more_than,
                maximum,
            } => write!(
                formatter,
                "sync directory {} has more than {found_more_than} direct entries; maximum is {maximum}",
                folder.display()
            ),
            Self::UnsafeEntry {
                folder,
                name,
                kind,
                reason,
                source,
            } => {
                write!(
                    formatter,
                    "unsafe sync entry {name:?} ({kind:?}) in {}: {reason}",
                    folder.display()
                )?;
                if let Some(source) = source {
                    write!(formatter, ": {source}")?;
                }
                Ok(())
            }
            Self::IncompleteSnapshot {
                folder,
                name,
                reason,
            } => write!(
                formatter,
                "incomplete sync snapshot for {name:?} in {}: {reason}",
                folder.display()
            ),
        }
    }
}

impl Error for SyncScanFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::DirectoryBinding { reason, .. } => Some(reason.as_ref()),
            Self::UnsafeEntry {
                source: Some(source),
                ..
            } => Some(source.as_ref()),
            Self::IncompleteSnapshot {
                reason: SyncIncompleteSnapshotReason::ReplacedDuringObservation { source },
                ..
            } => Some(source.as_ref()),
            Self::CountCapExceeded { .. }
            | Self::UnsafeEntry { source: None, .. }
            | Self::IncompleteSnapshot {
                reason: SyncIncompleteSnapshotReason::DisappearedDuringObservation,
                ..
            } => None,
        }
    }
}

impl fmt::Display for SyncReadOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("read observed file")
    }
}

impl fmt::Display for SyncUnsafeReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonRegular { kind } => write!(formatter, "not a regular file ({kind:?})"),
            Self::OversizedRegular { size, limit } => {
                write!(formatter, "regular file is {size} bytes, limit is {limit}")
            }
            Self::Unreadable { operation } => write!(formatter, "could not {operation}"),
        }
    }
}

impl fmt::Display for SyncIncompleteSnapshotReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DisappearedDuringObservation => {
                formatter.write_str("entry disappeared during observation")
            }
            Self::ReplacedDuringObservation { source } => {
                write!(formatter, "entry changed during observation: {source}")
            }
        }
    }
}

impl fmt::Display for SyncDirectoryOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::BindHealth => "bind health",
            Self::BindSync => "bind health/sync",
            Self::InspectSync => "inspect health/sync",
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncSnapshot {
    pub files: BTreeMap<OsString, FileObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPeerObservation {
    pub source_filename: OsString,
    pub classification: HeartbeatClassification,
    pub heartbeat: Option<Heartbeat>,
    pub is_live: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncConflictEvent {
    pub hostname: String,
    pub journal_path: String,
    pub pid: Option<u32>,
    pub machine_id_prefix: String,
    pub wall_time: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCheckResult {
    pub snapshot: SyncSnapshot,
    pub peer_observations: Vec<SyncPeerObservation>,
    pub live_peer_observations: Vec<SyncPeerObservation>,
}

impl SyncCheckResult {
    pub fn is_boot_conflict(&self) -> bool {
        !self.live_peer_observations.is_empty()
    }

    pub fn is_tick_conflict(&self, previous: Option<&SyncSnapshot>) -> bool {
        previous.is_some() && !self.live_peer_observations.is_empty()
    }
}

pub enum SyncRescan {
    Absent,
    Complete(SyncCheckResult),
}

pub fn format_conflict_message(result: &SyncCheckResult) -> String {
    let Some(writer) = result.live_peer_observations.first() else {
        return String::new();
    };
    let heartbeat = writer.heartbeat.as_ref();
    format!(
        "Refusing to start - another solstone service is active on this journal.\n\
         Host: {}\nJournal: {}\nPID: {}\nMachine: {}",
        display_hostname(heartbeat.map_or("", |value| value.hostname.as_str())),
        heartbeat.map_or("", |value| value.journal_path.as_str()),
        heartbeat.map_or_else(|| "(unknown)".to_owned(), |value| value.pid.to_string()),
        machine_id_prefix(heartbeat.map_or("", |value| value.machine_id.as_str())),
    )
}

pub fn sync_conflict_event(result: &SyncCheckResult) -> Option<SyncConflictEvent> {
    result.live_peer_observations.first().map(|writer| {
        let heartbeat = writer.heartbeat.as_ref();
        SyncConflictEvent {
            hostname: heartbeat
                .map_or_else(|| "(unknown)".to_owned(), |value| value.hostname.clone()),
            journal_path: heartbeat.map_or_else(String::new, |value| value.journal_path.clone()),
            pid: heartbeat.map(|value| value.pid),
            machine_id_prefix: machine_id_prefix(
                heartbeat.map_or("", |value| value.machine_id.as_str()),
            ),
            wall_time: heartbeat.map_or_else(String::new, |value| value.wall_time.clone()),
        }
    })
}

pub fn machine_id() -> String {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string("/etc/machine-id")
            .map(|value| value.trim().to_owned())
            .unwrap_or_default()
    }
    #[cfg(target_os = "macos")]
    {
        let Ok(output) = std::process::Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
        else {
            return String::new();
        };
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines()
            .find_map(|line| line.split_once("IOPlatformUUID"))
            .and_then(|(_, value)| value.split('"').nth(1))
            .unwrap_or_default()
            .to_owned()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        String::new()
    }
}

fn display_hostname(hostname: &str) -> &str {
    if hostname.is_empty() {
        "(unknown)"
    } else {
        hostname
    }
}

fn machine_id_prefix(machine_id: &str) -> String {
    if machine_id.is_empty() {
        "(unknown)".to_owned()
    } else {
        machine_id.chars().take(8).collect()
    }
}

pub fn sanitize_hostname(hostname: &str) -> String {
    let output: String = hostname
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect();
    output.trim_matches('-').to_owned().if_empty("unknown-host")
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_owned()
        } else {
            self
        }
    }
}

pub(crate) fn scan_bound_sync(
    sync: &FlatDirectory,
    self_filename: &str,
    self_machine_id: &str,
    previous: Option<&SyncSnapshot>,
    now: f64,
) -> Result<SyncCheckResult, SyncScanFailure> {
    let folder = PathBuf::from(SYNC_FOLDER_DIAGNOSTIC);
    let entries = list_flat_directory(sync, MAX_SYNC_DIRECTORY_ENTRIES).map_err(|reason| {
        SyncScanFailure::DirectoryBinding {
            path: folder.clone(),
            operation: SyncDirectoryOperation::InspectSync,
            reason: Box::new(reason),
        }
    })?;
    let entries = entries.ok_or_else(|| SyncScanFailure::CountCapExceeded {
        folder: folder.clone(),
        found_more_than: MAX_SYNC_DIRECTORY_ENTRIES,
        maximum: MAX_SYNC_DIRECTORY_ENTRIES,
    })?;

    let mut pending = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.kind != JournalEntryKind::RegularFile {
            return Err(SyncScanFailure::UnsafeEntry {
                folder: folder.clone(),
                name: entry.name,
                kind: entry.kind,
                reason: SyncUnsafeReason::NonRegular { kind: entry.kind },
                source: None,
            });
        }
        let name = entry.name;
        let observation = match read_observed_file_bounded(sync, &name, MAX_SYNC_HEARTBEAT_BYTES) {
            Ok(Some(observation)) => observation,
            Ok(None) => {
                return Err(SyncScanFailure::IncompleteSnapshot {
                    folder: folder.clone(),
                    name,
                    reason: SyncIncompleteSnapshotReason::DisappearedDuringObservation,
                });
            }
            Err(FlatDirectoryError::SizeLimitExceeded { size, limit, .. }) => {
                return Err(SyncScanFailure::UnsafeEntry {
                    folder: folder.clone(),
                    name,
                    kind: JournalEntryKind::RegularFile,
                    reason: SyncUnsafeReason::OversizedRegular { size, limit },
                    source: None,
                });
            }
            Err(
                error @ (FlatDirectoryError::IdentityChanged { .. }
                | FlatDirectoryError::NotRegular { .. }
                | FlatDirectoryError::EnumerationChanged { .. }),
            ) => {
                return Err(SyncScanFailure::IncompleteSnapshot {
                    folder: folder.clone(),
                    name,
                    reason: SyncIncompleteSnapshotReason::ReplacedDuringObservation {
                        source: Box::new(error),
                    },
                });
            }
            Err(error) => {
                return Err(SyncScanFailure::UnsafeEntry {
                    folder: folder.clone(),
                    name,
                    kind: JournalEntryKind::RegularFile,
                    reason: SyncUnsafeReason::Unreadable {
                        operation: SyncReadOperation::ReadObservedFileBounded,
                    },
                    source: Some(Box::new(error)),
                });
            }
        };
        let classification = match serde_json::from_slice::<Heartbeat>(&observation.bytes) {
            Ok(heartbeat) if heartbeat.schema == HEARTBEAT_SCHEMA_V1 => {
                HeartbeatClassification::SchemaV1(heartbeat)
            }
            Ok(heartbeat) => HeartbeatClassification::UnknownFuture(heartbeat),
            Err(_) => HeartbeatClassification::BoundedMalformed,
        };
        pending.push((observation, classification));
    }

    let mut snapshot = SyncSnapshot::default();
    let mut peer_observations = Vec::new();
    for (observation, classification) in pending {
        let name = observation.entry.name.clone();
        let fresh = now - native_mtime_seconds(&observation) <= FRESH_WINDOW_SECONDS;
        let prior = previous.and_then(|snapshot| snapshot.files.get(&name));
        let changed = prior.is_some_and(|prior| prior != &observation);
        let appeared = previous.is_some() && prior.is_none();
        snapshot.files.insert(name.clone(), observation);

        let heartbeat = classification_heartbeat(&classification).cloned();
        if name == OsStr::new(self_filename)
            || heartbeat.as_ref().is_some_and(|heartbeat| {
                !self_machine_id.is_empty() && heartbeat.machine_id == self_machine_id
            })
        {
            continue;
        }
        let is_live = if heartbeat.is_some() {
            fresh || changed || appeared
        } else {
            fresh
        };
        peer_observations.push(SyncPeerObservation {
            source_filename: name,
            classification,
            heartbeat,
            is_live,
        });
    }
    let live_peer_observations = peer_observations
        .iter()
        .filter(|peer| peer.is_live)
        .cloned()
        .collect();
    Ok(SyncCheckResult {
        snapshot,
        peer_observations,
        live_peer_observations,
    })
}

pub fn rescan_sync_read_only(
    journal: &Path,
    self_filename: &str,
    self_machine_id: &str,
    previous: Option<&SyncSnapshot>,
    now: f64,
) -> Result<SyncRescan, SyncScanFailure> {
    let root =
        JournalRoot::open(journal).map_err(|error| directory_binding_from_root(journal, error))?;
    let health = open_flat_directory_bound(&root, OsStr::new("health"), root.canonical_path())
        .map_err(|reason| SyncScanFailure::DirectoryBinding {
            path: root.canonical_path().join("health"),
            operation: SyncDirectoryOperation::BindHealth,
            reason: Box::new(reason),
        })?;
    let Some(health) = health else {
        return Ok(SyncRescan::Absent);
    };
    let health_diagnostic = root.canonical_path().join("health");
    let sync = open_flat_directory_bound(&health, OsStr::new("sync"), &health_diagnostic).map_err(
        |reason| SyncScanFailure::DirectoryBinding {
            path: health_diagnostic.join("sync"),
            operation: SyncDirectoryOperation::BindSync,
            reason: Box::new(reason),
        },
    )?;
    let Some(sync) = sync else {
        return Ok(SyncRescan::Absent);
    };
    scan_bound_sync(&sync, self_filename, self_machine_id, previous, now).map(SyncRescan::Complete)
}

pub(crate) fn directory_binding_from_root(
    journal: &Path,
    error: JournalRootError,
) -> SyncScanFailure {
    let reason = match error {
        JournalRootError::Changed => FlatDirectoryError::IdentityChanged {
            path: journal.to_path_buf(),
        },
        JournalRootError::Invalid { root, reason, .. }
        | JournalRootError::Unsupported { root, reason, .. } => FlatDirectoryError::Io {
            operation: "open journal root",
            path: root,
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, reason),
        },
        JournalRootError::Io {
            operation,
            path,
            source,
        } => FlatDirectoryError::Io {
            operation,
            path,
            source,
        },
    };
    SyncScanFailure::DirectoryBinding {
        path: journal.to_path_buf(),
        operation: SyncDirectoryOperation::BindHealth,
        reason: Box::new(reason),
    }
}

fn native_mtime_seconds(observation: &FileObservation) -> f64 {
    let mtime = observation.entry.mtime;
    mtime.seconds as f64 + (mtime.nanoseconds as f64 / 1_000_000_000.0)
}

fn classification_heartbeat(classification: &HeartbeatClassification) -> Option<&Heartbeat> {
    match classification {
        HeartbeatClassification::SchemaV1(heartbeat)
        | HeartbeatClassification::UnknownFuture(heartbeat) => Some(heartbeat),
        HeartbeatClassification::BoundedMalformed => None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::Builder;

    use super::*;
    use solstone_core_journal_io::create_or_open_flat_directory_bound;

    fn temporary() -> tempfile::TempDir {
        Builder::new()
            .prefix("solstone-sync-")
            .tempdir_in("/var/tmp")
            .unwrap()
    }

    fn bound_sync(root_path: &Path) -> (JournalRoot, FlatDirectory, FlatDirectory) {
        let root = JournalRoot::open(root_path).unwrap();
        let health = create_or_open_flat_directory_bound(
            &root,
            OsStr::new("health"),
            0o700,
            root.canonical_path(),
        )
        .unwrap();
        let health_diagnostic = root.canonical_path().join("health");
        let sync = create_or_open_flat_directory_bound(
            &health,
            OsStr::new("sync"),
            0o700,
            &health_diagnostic,
        )
        .unwrap();
        (root, health, sync)
    }

    fn heartbeat(schema: u8, machine_id: &str) -> Heartbeat {
        Heartbeat {
            schema,
            machine_id: machine_id.to_owned(),
            hostname: "foreign".to_owned(),
            pid: 7,
            wall_time: "now".to_owned(),
            solstone_version: "test".to_owned(),
            interval_seconds: 15,
            journal_path: "/journal".to_owned(),
        }
    }

    #[test]
    fn read_only_rescan_of_absent_directories_does_not_create_them() {
        let temporary = temporary();
        assert!(matches!(
            rescan_sync_read_only(temporary.path(), "self.check", "self", None, 1.0).unwrap(),
            SyncRescan::Absent
        ));
        assert!(!temporary.path().join("health").exists());
    }

    #[test]
    fn scan_includes_stage_and_claim_names_without_filename_filtering() {
        let temporary = temporary();
        let (_root, _health, sync) = bound_sync(temporary.path());
        let sync_path = temporary.path().join(SYNC_FOLDER_DIAGNOSTIC);
        fs::write(sync_path.join(".tmp_1_1.tmp"), b"not json").unwrap();
        fs::write(
            sync_path.join("!solstone-claim-00000001-0000000000000001"),
            b"also not json",
        )
        .unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let result = scan_bound_sync(&sync, "self.check", "self", None, now).unwrap();
        assert_eq!(result.snapshot.files.len(), 2);
        assert_eq!(result.peer_observations.len(), 2);
        assert!(result.peer_observations.iter().all(|peer| matches!(
            peer.classification,
            HeartbeatClassification::BoundedMalformed
        )));
    }

    #[test]
    fn scan_fails_closed_for_count_unsafe_kind_and_oversized_file() {
        let temporary = temporary();
        let (_root, _health, sync) = bound_sync(temporary.path());
        let sync_path = temporary.path().join(SYNC_FOLDER_DIAGNOSTIC);
        for index in 0..=MAX_SYNC_DIRECTORY_ENTRIES {
            fs::write(sync_path.join(format!("entry-{index}")), b"{}").unwrap();
        }
        assert!(matches!(
            scan_bound_sync(&sync, "self.check", "self", None, 1.0),
            Err(SyncScanFailure::CountCapExceeded {
                found_more_than: MAX_SYNC_DIRECTORY_ENTRIES,
                maximum: MAX_SYNC_DIRECTORY_ENTRIES,
                ..
            })
        ));

        for entry in fs::read_dir(&sync_path).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        fs::create_dir(sync_path.join("unsafe")).unwrap();
        assert!(matches!(
            scan_bound_sync(&sync, "self.check", "self", None, 1.0),
            Err(SyncScanFailure::UnsafeEntry {
                reason: SyncUnsafeReason::NonRegular {
                    kind: JournalEntryKind::Directory
                },
                ..
            })
        ));
        fs::remove_dir(sync_path.join("unsafe")).unwrap();
        fs::write(
            sync_path.join("oversized"),
            vec![0_u8; MAX_SYNC_HEARTBEAT_BYTES + 1],
        )
        .unwrap();
        assert!(matches!(
            scan_bound_sync(&sync, "self.check", "self", None, 1.0),
            Err(SyncScanFailure::UnsafeEntry {
                reason: SyncUnsafeReason::OversizedRegular {
                    size,
                    limit: MAX_SYNC_HEARTBEAT_BYTES,
                },
                ..
            }) if size == (MAX_SYNC_HEARTBEAT_BYTES + 1) as u64
        ));
    }

    #[test]
    fn scan_classifies_and_suppresses_parsed_heartbeats_without_schema_filter() {
        let temporary = temporary();
        let (_root, _health, sync) = bound_sync(temporary.path());
        let sync_path = temporary.path().join(SYNC_FOLDER_DIAGNOSTIC);
        fs::write(
            sync_path.join("self.check"),
            serde_json::to_vec(&heartbeat(HEARTBEAT_SCHEMA_V1, "other")).unwrap(),
        )
        .unwrap();
        fs::write(
            sync_path.join("future.check"),
            serde_json::to_vec(&heartbeat(2, "self")).unwrap(),
        )
        .unwrap();
        fs::write(sync_path.join("malformed"), b"not heartbeat").unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let result = scan_bound_sync(&sync, "self.check", "self", None, now).unwrap();
        assert_eq!(result.snapshot.files.len(), 3);
        assert_eq!(result.peer_observations.len(), 1);
        assert!(matches!(
            result.peer_observations[0].classification,
            HeartbeatClassification::BoundedMalformed
        ));
        assert!(result.peer_observations[0].is_live);
    }
}
