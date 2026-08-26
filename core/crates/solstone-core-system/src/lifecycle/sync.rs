// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    FileObservation, FlatDirectory, FlatDirectoryError, JournalEntryKind, JournalRoot,
    JournalRootError, list_flat_directory, open_flat_directory_bound, read_observed_file_bounded,
};

use crate::process::ProcessInstance;

pub const DEFAULT_INTERVAL_SECONDS: f64 = 15.0;
pub const FRESH_WINDOW_MULTIPLIER: f64 = 4.0;
pub const FRESH_WINDOW_SECONDS: f64 = FRESH_WINDOW_MULTIPLIER * DEFAULT_INTERVAL_SECONDS;

pub const HEARTBEAT_SCHEMA_V1: u8 = 1;
pub const HEARTBEAT_SCHEMA_V2: u8 = 2;
pub const ADMISSION_WAIT_MARKER_SCHEMA_V2: u8 = 2;

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

/// Canonical installation identity used to name v2 heartbeat writers.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WriterId([u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriterIdParseError;

impl fmt::Display for WriterIdParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("writer ID must be 32 lowercase hexadecimal characters")
    }
}

impl Error for WriterIdParseError {}

impl WriterId {
    pub fn parse(value: &str) -> Result<Self, WriterIdParseError> {
        parse_lower_hex::<16>(value).map(Self)
    }

    pub fn as_hex(&self) -> String {
        lower_hex(&self.0)
    }
}

impl fmt::Display for WriterId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

impl Serialize for WriterId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.as_hex())
    }
}

impl<'de> Deserialize<'de> for WriterId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// One OS-CSPRNG-generated token for a supervisor boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RunId([u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunIdParseError;

impl fmt::Display for RunIdParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("run ID must be 32 lowercase hexadecimal characters")
    }
}

impl Error for RunIdParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunIdGenerationError(getrandom::Error);

impl fmt::Display for RunIdGenerationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "OS CSPRNG could not generate a supervisor run ID: {}",
            self.0
        )
    }
}

impl Error for RunIdGenerationError {}

impl From<getrandom::Error> for RunIdGenerationError {
    fn from(value: getrandom::Error) -> Self {
        Self(value)
    }
}

impl RunId {
    pub fn generate() -> Result<Self, RunIdGenerationError> {
        Self::generate_with(getrandom::fill)
    }

    pub(crate) fn generate_with(
        fill: impl FnOnce(&mut [u8]) -> Result<(), getrandom::Error>,
    ) -> Result<Self, RunIdGenerationError> {
        let mut bytes = [0_u8; 16];
        fill(&mut bytes)?;
        Ok(Self(bytes))
    }

    pub fn parse(value: &str) -> Result<Self, RunIdParseError> {
        parse_lower_hex::<16>(value)
            .map(Self)
            .map_err(|_| RunIdParseError)
    }

    pub fn as_hex(&self) -> String {
        lower_hex(&self.0)
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

impl Serialize for RunId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.as_hex())
    }
}

impl<'de> Deserialize<'de> for RunId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// The v2 heartbeat body. V1 stays frozen above as a decode-only legacy shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeartbeatV2 {
    pub schema: u8,
    pub writer_id: WriterId,
    pub run_id: RunId,
    pub hostname: String,
    pub pid: u32,
    pub wall_time: String,
    pub solstone_version: String,
    pub interval_seconds: u32,
    pub journal_path: String,
}

impl HeartbeatV2 {
    // The fixed v2 wire schema has eight required diagnostic fields; a builder
    // would only obscure that complete construction at its lifecycle call sites.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        writer_id: WriterId,
        run_id: RunId,
        hostname: String,
        pid: u32,
        wall_time: String,
        solstone_version: String,
        interval_seconds: u32,
        journal_path: String,
    ) -> Self {
        Self {
            schema: HEARTBEAT_SCHEMA_V2,
            writer_id,
            run_id,
            hostname,
            pid,
            wall_time,
            solstone_version,
            interval_seconds,
            journal_path,
        }
    }
}

/// The sole reason an admitting supervisor may publish a bounded wait marker.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AdmissionWaitReason {
    FreshNonSelfHeartbeat,
}

/// A retained proof that one exact process is in the bounded admission wait.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdmissionWaitMarker {
    pub schema: u8,
    pub writer_id: WriterId,
    pub run_id: RunId,
    pub process: ProcessInstance,
    pub reason: AdmissionWaitReason,
}

impl AdmissionWaitMarker {
    pub fn new(
        writer_id: WriterId,
        run_id: RunId,
        process: ProcessInstance,
        reason: AdmissionWaitReason,
    ) -> Self {
        Self {
            schema: ADMISSION_WAIT_MARKER_SCHEMA_V2,
            writer_id,
            run_id,
            process,
            reason,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V2HeartbeatFilenameError {
    InvalidFilename,
    InvalidGrammar,
}

impl fmt::Display for V2HeartbeatFilenameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidFilename => "invalid heartbeat filename",
            Self::InvalidGrammar => "invalid v2 heartbeat filename grammar",
        })
    }
}

impl Error for V2HeartbeatFilenameError {}

pub fn v2_heartbeat_filename(writer_id: &WriterId, run_id: &RunId) -> String {
    format!("solstone-v2-{writer_id}-{run_id}.check")
}

pub fn parse_v2_heartbeat_filename(
    filename: &str,
) -> Result<(WriterId, RunId), V2HeartbeatFilenameError> {
    parse_v2_filename_with_prefix(filename, "solstone-v2-")
}

pub fn admission_wait_marker_filename(writer_id: &WriterId, run_id: &RunId) -> String {
    format!("solstone-wait-v2-{writer_id}-{run_id}.check")
}

pub fn parse_admission_wait_marker_filename(
    filename: &str,
) -> Result<(WriterId, RunId), V2HeartbeatFilenameError> {
    parse_v2_filename_with_prefix(filename, "solstone-wait-v2-")
}

fn parse_v2_filename_with_prefix(
    filename: &str,
    prefix: &str,
) -> Result<(WriterId, RunId), V2HeartbeatFilenameError> {
    super::state::validate_heartbeat_filename(filename)
        .map_err(|_| V2HeartbeatFilenameError::InvalidFilename)?;
    let Some(stem) = filename.strip_suffix(".check") else {
        return Err(V2HeartbeatFilenameError::InvalidGrammar);
    };
    let Some(parts) = stem.strip_prefix(prefix) else {
        return Err(V2HeartbeatFilenameError::InvalidGrammar);
    };
    let mut parts = parts.split('-');
    let (Some(writer_id), Some(run_id), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(V2HeartbeatFilenameError::InvalidGrammar);
    };
    let writer_id =
        WriterId::parse(writer_id).map_err(|_| V2HeartbeatFilenameError::InvalidGrammar)?;
    let run_id = RunId::parse(run_id).map_err(|_| V2HeartbeatFilenameError::InvalidGrammar)?;
    Ok((writer_id, run_id))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeartbeatClassification {
    SchemaV1(Heartbeat),
    UnknownFuture(Heartbeat),
    SchemaV2(HeartbeatV2),
    /// A v2 body must be authoritative only under the filename that encodes
    /// its writer and run identity.
    IdentityMismatch(HeartbeatV2),
    AdmissionWaitMarker(AdmissionWaitMarker),
    AdmissionWaitMarkerIdentityMismatch(AdmissionWaitMarker),
    AdmissionWaitMarkerMalformed,
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
        let classification = classify_heartbeat(&name, &observation.bytes);
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
        let has_known_heartbeat = matches!(
            &classification,
            HeartbeatClassification::SchemaV1(_)
                | HeartbeatClassification::UnknownFuture(_)
                | HeartbeatClassification::SchemaV2(_)
        );
        if name == OsStr::new(self_filename)
            && !matches!(
                &classification,
                HeartbeatClassification::IdentityMismatch(_)
            )
        {
            continue;
        }
        let is_live = if has_known_heartbeat {
            fresh || changed || appeared
        } else if matches!(
            &classification,
            HeartbeatClassification::AdmissionWaitMarker(_)
                | HeartbeatClassification::AdmissionWaitMarkerIdentityMismatch(_)
                | HeartbeatClassification::AdmissionWaitMarkerMalformed
        ) {
            false
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
    scan_bound_sync(&sync, self_filename, previous, now).map(SyncRescan::Complete)
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
        HeartbeatClassification::SchemaV2(_)
        | HeartbeatClassification::IdentityMismatch(_)
        | HeartbeatClassification::AdmissionWaitMarker(_)
        | HeartbeatClassification::AdmissionWaitMarkerIdentityMismatch(_)
        | HeartbeatClassification::AdmissionWaitMarkerMalformed
        | HeartbeatClassification::BoundedMalformed => None,
    }
}

fn classify_heartbeat(name: &OsStr, bytes: &[u8]) -> HeartbeatClassification {
    if is_admission_wait_marker_candidate(name) {
        return classify_admission_wait_marker(name, bytes);
    }

    #[derive(Deserialize)]
    struct SchemaEnvelope {
        schema: u8,
    }

    match serde_json::from_slice::<SchemaEnvelope>(bytes) {
        Ok(SchemaEnvelope {
            schema: HEARTBEAT_SCHEMA_V1,
        }) => serde_json::from_slice::<Heartbeat>(bytes)
            .map(HeartbeatClassification::SchemaV1)
            .unwrap_or(HeartbeatClassification::BoundedMalformed),
        Ok(SchemaEnvelope {
            schema: HEARTBEAT_SCHEMA_V2,
        }) => match serde_json::from_slice::<HeartbeatV2>(bytes) {
            Ok(heartbeat) => match name
                .to_str()
                .and_then(|name| parse_v2_heartbeat_filename(name).ok())
            {
                Some((writer_id, run_id))
                    if writer_id == heartbeat.writer_id && run_id == heartbeat.run_id =>
                {
                    HeartbeatClassification::SchemaV2(heartbeat)
                }
                _ => HeartbeatClassification::IdentityMismatch(heartbeat),
            },
            Err(_) => HeartbeatClassification::BoundedMalformed,
        },
        Ok(_) => serde_json::from_slice::<Heartbeat>(bytes)
            .map(HeartbeatClassification::UnknownFuture)
            .unwrap_or(HeartbeatClassification::BoundedMalformed),
        Err(_) => HeartbeatClassification::BoundedMalformed,
    }
}

fn is_admission_wait_marker_candidate(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with("solstone-wait-v2-"))
}

fn classify_admission_wait_marker(name: &OsStr, bytes: &[u8]) -> HeartbeatClassification {
    let Ok(name) = name.to_str().ok_or(()) else {
        return HeartbeatClassification::AdmissionWaitMarkerMalformed;
    };
    let Ok((writer_id, run_id)) = parse_admission_wait_marker_filename(name) else {
        return HeartbeatClassification::AdmissionWaitMarkerMalformed;
    };
    match serde_json::from_slice::<AdmissionWaitMarker>(bytes) {
        Ok(marker) if marker.schema == ADMISSION_WAIT_MARKER_SCHEMA_V2 => {
            if marker.writer_id == writer_id && marker.run_id == run_id {
                HeartbeatClassification::AdmissionWaitMarker(marker)
            } else {
                HeartbeatClassification::AdmissionWaitMarkerIdentityMismatch(marker)
            }
        }
        Ok(_) | Err(_) => HeartbeatClassification::AdmissionWaitMarkerMalformed,
    }
}

pub(crate) fn latest_live_freshness_horizon(result: &SyncCheckResult) -> Option<f64> {
    result
        .live_peer_observations
        .iter()
        .filter_map(|peer| result.snapshot.files.get(&peer.source_filename))
        .map(|observation| native_mtime_seconds(observation) + FRESH_WINDOW_SECONDS)
        .max_by(f64::total_cmp)
}

fn parse_lower_hex<const N: usize>(value: &str) -> Result<[u8; N], WriterIdParseError> {
    if value.len() != N * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(WriterIdParseError);
    }
    let mut output = [0_u8; N];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| WriterIdParseError)?;
    }
    Ok(output)
}

fn lower_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sha2::{Digest, Sha256};
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

    fn writer_id(value: &str) -> WriterId {
        WriterId::parse(value).expect("writer ID")
    }

    fn run_id(value: &str) -> RunId {
        RunId::parse(value).expect("run ID")
    }

    #[test]
    fn v2_filename_round_trips_and_passes_shared_filename_validation() {
        let writer_id = writer_id("0123456789abcdef0123456789abcdef");
        let run_id = run_id("fedcba9876543210fedcba9876543210");
        let filename = v2_heartbeat_filename(&writer_id, &run_id);
        assert_eq!(
            filename,
            "solstone-v2-0123456789abcdef0123456789abcdef-fedcba9876543210fedcba9876543210.check"
        );
        super::super::state::validate_heartbeat_filename(&filename)
            .expect("v2 filename passes shared validation");
        assert_eq!(
            parse_v2_heartbeat_filename(&filename).expect("parse v2 filename"),
            (writer_id, run_id)
        );
    }

    #[test]
    fn admission_wait_marker_filename_round_trips_and_is_not_a_heartbeat_conflict() {
        let temporary = temporary();
        let (_root, _health, sync) = bound_sync(temporary.path());
        let writer_id = writer_id("0123456789abcdef0123456789abcdef");
        let run_id = run_id("fedcba9876543210fedcba9876543210");
        let filename = admission_wait_marker_filename(&writer_id, &run_id);
        super::super::state::validate_heartbeat_filename(&filename)
            .expect("marker filename passes shared validation");
        assert_eq!(
            parse_admission_wait_marker_filename(&filename).expect("parse marker filename"),
            (writer_id.clone(), run_id)
        );

        let marker = AdmissionWaitMarker::new(
            writer_id,
            run_id,
            ProcessInstance {
                pid: 7,
                birth: crate::process::ProcessBirth::linux(10, 100, 100),
            },
            AdmissionWaitReason::FreshNonSelfHeartbeat,
        );
        fs::write(
            temporary
                .path()
                .join(SYNC_FOLDER_DIAGNOSTIC)
                .join(&filename),
            serde_json::to_vec(&marker).expect("marker JSON"),
        )
        .expect("marker fixture");

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("wall clock")
            .as_secs_f64();
        let result = scan_bound_sync(&sync, "self.check", None, now).expect("scan");
        assert_eq!(result.peer_observations.len(), 1);
        assert!(matches!(
            result.peer_observations[0].classification,
            HeartbeatClassification::AdmissionWaitMarker(_)
        ));
        assert!(!result.peer_observations[0].is_live);
        assert!(!result.is_boot_conflict());
    }

    #[test]
    fn admission_wait_marker_identity_mismatch_is_not_valid_for_reading() {
        let writer_id = writer_id("0123456789abcdef0123456789abcdef");
        let filename =
            admission_wait_marker_filename(&writer_id, &run_id("11111111111111111111111111111111"));
        let marker = AdmissionWaitMarker::new(
            writer_id,
            run_id("22222222222222222222222222222222"),
            ProcessInstance {
                pid: 7,
                birth: crate::process::ProcessBirth::linux(10, 100, 100),
            },
            AdmissionWaitReason::FreshNonSelfHeartbeat,
        );
        assert!(matches!(
            classify_heartbeat(
                OsStr::new(&filename),
                &serde_json::to_vec(&marker).expect("marker JSON")
            ),
            HeartbeatClassification::AdmissionWaitMarkerIdentityMismatch(_)
        ));
    }

    #[test]
    fn run_id_generation_is_fresh_and_propagates_random_failure() {
        let first = RunId::generate_with(|bytes| {
            bytes.copy_from_slice(&[1_u8; 16]);
            Ok(())
        })
        .expect("first run ID");
        let second = RunId::generate_with(|bytes| {
            bytes.copy_from_slice(&[2_u8; 16]);
            Ok(())
        })
        .expect("second run ID");
        assert_ne!(first, second);

        assert!(matches!(
            RunId::generate_with(|_| Err(getrandom::Error::UNSUPPORTED)),
            Err(error) if error.0 == getrandom::Error::UNSUPPORTED
        ));
    }

    #[test]
    fn v1_heartbeat_shape_remains_decodable() {
        let decoded: Heartbeat = serde_json::from_slice(
            br#"{"schema":1,"machine_id":"legacy-machine","hostname":"host","pid":7,"wall_time":"now","solstone_version":"1","interval_seconds":15,"journal_path":"/journal"}"#,
        )
        .expect("frozen v1 shape decodes");
        assert_eq!(decoded.schema, HEARTBEAT_SCHEMA_V1);
        assert_eq!(decoded.machine_id, "legacy-machine");
        assert_eq!(decoded.hostname, "host");
    }

    #[test]
    fn frozen_v1_heartbeat_fixture_is_digest_pinned_and_remains_v1() {
        const FIXTURE: &str = include_str!("../../../../fixtures/sync_heartbeat_v1.json");
        const SHA256: &str = "86a7997e56af5a7f9fff4a23641d4789e1cdfe12cab77ea2b3e0bd1e2e5cfd3f";

        assert_eq!(format!("{:x}", Sha256::digest(FIXTURE.as_bytes())), SHA256);
        let classification = classify_heartbeat(OsStr::new("legacy-v1.check"), FIXTURE.as_bytes());
        let HeartbeatClassification::SchemaV1(heartbeat) = classification else {
            panic!("frozen v1 fixture must not classify as v2 or an admission-wait marker");
        };
        assert_eq!(heartbeat.schema, HEARTBEAT_SCHEMA_V1);
        assert_eq!(heartbeat.machine_id, "8f2b6aa217a24ccd9e3bf15e58fc1b4a");
    }

    #[test]
    fn v2_filename_body_identity_mismatch_is_not_self_suppressed() {
        let temporary = temporary();
        let (_root, _health, sync) = bound_sync(temporary.path());
        let filename_writer = writer_id("0123456789abcdef0123456789abcdef");
        let filename_run = run_id("11111111111111111111111111111111");
        let body = HeartbeatV2::new(
            writer_id("fedcba9876543210fedcba9876543210"),
            run_id("22222222222222222222222222222222"),
            "foreign".to_owned(),
            7,
            "now".to_owned(),
            "test".to_owned(),
            15,
            "/journal".to_owned(),
        );
        let filename = v2_heartbeat_filename(&filename_writer, &filename_run);
        fs::write(
            temporary
                .path()
                .join(SYNC_FOLDER_DIAGNOSTIC)
                .join(&filename),
            serde_json::to_vec(&body).expect("v2 JSON"),
        )
        .expect("v2 heartbeat");

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("wall clock")
            .as_secs_f64();
        let result = scan_bound_sync(&sync, &filename, None, now).expect("scan");
        assert_eq!(result.peer_observations.len(), 1);
        assert!(matches!(
            result.peer_observations[0].classification,
            HeartbeatClassification::IdentityMismatch(_)
        ));
        assert!(result.peer_observations[0].is_live);
    }

    #[test]
    fn read_only_rescan_of_absent_directories_does_not_create_them() {
        let temporary = temporary();
        assert!(matches!(
            rescan_sync_read_only(temporary.path(), "self.check", None, 1.0).unwrap(),
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
        let result = scan_bound_sync(&sync, "self.check", None, now).unwrap();
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
            scan_bound_sync(&sync, "self.check", None, 1.0),
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
            scan_bound_sync(&sync, "self.check", None, 1.0),
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
            scan_bound_sync(&sync, "self.check", None, 1.0),
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
    fn scan_suppresses_only_the_retained_self_filename() {
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
            serde_json::to_vec(&heartbeat(3, "self")).unwrap(),
        )
        .unwrap();
        fs::write(sync_path.join("malformed"), b"not heartbeat").unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let result = scan_bound_sync(&sync, "self.check", None, now).unwrap();
        assert_eq!(result.snapshot.files.len(), 3);
        assert_eq!(result.peer_observations.len(), 2);
        assert!(result.peer_observations.iter().any(|peer| {
            matches!(
                peer.classification,
                HeartbeatClassification::UnknownFuture(_)
            ) && peer.is_live
        }));
        assert!(result.peer_observations.iter().any(|peer| matches!(
            peer.classification,
            HeartbeatClassification::BoundedMalformed
        )));
    }

    #[test]
    fn an_appeared_stale_future_schema_heartbeat_is_a_conflict() {
        let temporary = temporary();
        let (_root, _health, sync) = bound_sync(temporary.path());
        let sync_path = temporary.path().join(SYNC_FOLDER_DIAGNOSTIC);
        fs::write(
            sync_path.join("self.check"),
            serde_json::to_vec(&heartbeat(HEARTBEAT_SCHEMA_V1, "self")).unwrap(),
        )
        .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let first = scan_bound_sync(&sync, "self.check", None, now).unwrap();
        fs::write(
            sync_path.join("future.check"),
            serde_json::to_vec(&heartbeat(3, "foreign")).unwrap(),
        )
        .unwrap();

        let result = scan_bound_sync(
            &sync,
            "self.check",
            Some(&first.snapshot),
            now + FRESH_WINDOW_SECONDS + 1.0,
        )
        .unwrap();
        assert!(result.peer_observations.iter().any(|peer| {
            matches!(
                peer.classification,
                HeartbeatClassification::UnknownFuture(_)
            ) && peer.is_live
        }));
        assert!(result.is_boot_conflict());
    }
}
