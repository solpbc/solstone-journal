// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native importer contracts and journal-host grammar seams.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

pub mod cli_journal_source;
pub mod cli_render;
pub mod connect;
pub mod consent_gate;
pub mod contract;
pub mod dedupe;
pub mod detect;
pub mod events;
pub mod metadata;
pub mod publish;
pub mod staging;
pub mod stream_name;
pub mod sync_audio;
pub mod sync_obsidian;
pub mod sync_plaud;
pub mod sync_state;
pub mod text;
pub mod timestamp;

pub use connect::{OuraConnectOutcome, OuraConnectRequest, connect_oura};
pub use consent_gate::{
    CONSENT_GATE_EXIT_CODE, ConsentGateOutcome, ConsentGateRequest, GateFailure,
    check_oura_sync_save,
};
pub use contract::{
    AudioAuto, ImportPreview, ImportResult, OwnerSource, OwnerSourceMetadata, PreviewRequest,
    SaveRequest, SourceHash, SourceImmutabilityReport, SyncBackendRequest, SyncGuidance,
    SyncPreviewRequest, SyncSaveRequest, observe_source_immutability, should_write_manifest,
};
pub use dedupe::{
    ImportManifestBackfillReport, ManifestMatch, ManifestScan, ManifestSkip, ManifestSkipReason,
    ManifestWriteRequest, backfill_retained_import_manifests, find_manifest_by_hash, hash_source,
    windowed_source_hash, write_manifest,
};
pub use detect::{
    ManifestSummary, ModelDetectionError, RegistrySource, ResolutionError, ResolutionOptions,
    ResolutionOutcome, ResolutionSeams, ResolvedSource, SkipReason,
};
pub use events::{
    EnrichmentReady, EventEmitter, FileImported, ImporterCompleted, ImporterError, ImporterStarted,
    ImporterStatus, ObservedSegment, ObservingMeta, ObservingSegment, emit_enrichment_ready,
    emit_file_imported, emit_importer_completed, emit_importer_error, emit_importer_started,
    emit_importer_status, emit_observe_observed, emit_observe_observing, emit_supervisor_drain,
};
pub use metadata::{ImportMetadata, read_import_metadata, read_provenance, write_import_metadata};
pub use publish::{
    CreatedSegment, DayMarkerOutcome, DayMarkerStatus, IndexPublicationOutcomes, IndexedFile,
    IndexedFileError, NativePublicationOperations, PublicationInput, PublicationOperations,
    PublicationRecord, PublicationStatus, PublishError, SegmentBindingOutcome,
    SegmentPublicationOutcome, publish, publish_with_operations, read_publication_record,
    write_publication_record,
};
pub use staging::{
    SourceLocation, StageOutcome, StageRequest, classify_source_location, relocate_import,
    stage_source,
};
pub use sync_state::{
    BackendName, SYNC_BACKEND_INVENTORY, SyncState, SyncStateRead, SyncStateReadClass,
    SyncStateWriteError, read_sync_state, state_path, write_sync_state,
};
pub use text::{
    SystemWireClient, TextImportError, TextWirePhase, WireClient, process_transcript,
    process_transcript_with_wire,
};
pub use timestamp::{
    AutoTimestamp, DetectedTimestamp, Timestamp, TimestampError, validate_timestamp,
};

/// Error returned by an importer seam that has no implementation yet.
#[derive(Debug)]
pub enum ImportError {
    Unimplemented {
        module: &'static str,
    },
    ExistingImportDirectory {
        path: PathBuf,
    },
    MetadataMismatchOnForce {
        path: PathBuf,
        key: &'static str,
    },
    ImportDirectoryIsSymlink {
        path: PathBuf,
    },
    ImportDirectoryEscapesImports {
        path: PathBuf,
        imports: PathBuf,
    },
    SourceMissing {
        path: PathBuf,
    },
    SourceNotFile {
        path: PathBuf,
    },
    NonUtf8DirectoryEntry {
        path: PathBuf,
    },
    DestinationExists {
        path: PathBuf,
    },
    PromotionFailed {
        path: PathBuf,
        message: String,
    },
    AuditSinkFailed {
        message: String,
    },
    RemovalFailed {
        path: PathBuf,
        message: String,
    },
    MetadataCorrupt {
        path: PathBuf,
        message: String,
    },
    MetadataWriteFailed {
        path: PathBuf,
        message: String,
    },
    ManifestWriteFailed {
        path: PathBuf,
        message: String,
    },
    PathResolution {
        path: PathBuf,
        message: String,
    },
    RelocationFailed {
        path: PathBuf,
        message: String,
    },
    InvalidImportId {
        import_id: String,
    },
    InvalidDestinationName {
        name: OsString,
    },
    AudioDurationUnavailable {
        path: PathBuf,
        detail: String,
    },
    AudioInputUnreadable {
        path: PathBuf,
        detail: String,
    },
    AudioSliceRejected {
        path: PathBuf,
        chunk_index: u64,
        start_offset_seconds: f64,
        duration_seconds: f64,
        detail: String,
    },
    AudioSegmentDirectory {
        path: PathBuf,
        message: String,
    },
    AudioSegmentCollision {
        day: String,
        stream: String,
        start: String,
        attempts: u32,
    },
    AudioSegmentDayOverflow {
        day: String,
        stream: String,
        start: String,
    },
    NoAudioSegmentsCreated {
        path: PathBuf,
    },
    AudioRecordRead {
        path: PathBuf,
        message: String,
    },
    AudioRecordWrite {
        path: PathBuf,
        message: String,
    },
    AudioProcessingWait {
        detail: String,
    },
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unimplemented { module } => write!(formatter, "import: unimplemented: {module}"),
            Self::ExistingImportDirectory { path } => {
                write!(formatter, "import already exists: {}", path.display())
            }
            Self::MetadataMismatchOnForce { path, key } => write!(
                formatter,
                "import metadata mismatch for {key}: {}",
                path.display()
            ),
            Self::ImportDirectoryIsSymlink { path } => write!(
                formatter,
                "import directory is a symlink: {}",
                path.display()
            ),
            Self::ImportDirectoryEscapesImports { path, imports } => write!(
                formatter,
                "import directory escapes imports root {}: {}",
                imports.display(),
                path.display()
            ),
            Self::SourceMissing { path } => {
                write!(formatter, "import source is missing: {}", path.display())
            }
            Self::SourceNotFile { path } => {
                write!(formatter, "import source is not a file: {}", path.display())
            }
            Self::NonUtf8DirectoryEntry { path } => write!(
                formatter,
                "directory source contains a non-UTF-8 entry: {}",
                path.display()
            ),
            Self::DestinationExists { path } => write!(
                formatter,
                "import destination already exists: {}",
                path.display()
            ),
            Self::PromotionFailed { path, message }
            | Self::MetadataWriteFailed { path, message }
            | Self::ManifestWriteFailed { path, message }
            | Self::PathResolution { path, message }
            | Self::RelocationFailed { path, message }
            | Self::AudioRecordRead { path, message }
            | Self::AudioRecordWrite { path, message } => {
                write!(formatter, "{}: {message}", path.display())
            }
            Self::AuditSinkFailed { message } => {
                write!(formatter, "import audit failed: {message}")
            }
            Self::RemovalFailed { path, message } => {
                write!(formatter, "{}: {message}", path.display())
            }
            Self::MetadataCorrupt { path, message } => write!(
                formatter,
                "invalid import metadata {}: {message}",
                path.display()
            ),
            Self::InvalidImportId { import_id } => {
                write!(formatter, "invalid import id: {import_id}")
            }
            Self::InvalidDestinationName { name } => {
                write!(
                    formatter,
                    "invalid import destination name: {}",
                    name.to_string_lossy()
                )
            }
            Self::AudioDurationUnavailable { path, detail } => {
                write!(
                    formatter,
                    "could not determine audio duration {}: {detail}",
                    path.display()
                )
            }
            Self::AudioInputUnreadable { path, detail } => {
                write!(
                    formatter,
                    "could not read audio input {}: {detail}",
                    path.display()
                )
            }
            Self::AudioSliceRejected {
                path,
                chunk_index,
                start_offset_seconds,
                duration_seconds,
                detail,
            } => write!(
                formatter,
                "audio slice rejected for chunk {chunk_index} [{start_offset_seconds}, {}) of {}: {detail}",
                start_offset_seconds + duration_seconds,
                path.display()
            ),
            Self::AudioSegmentDirectory { path, message } => {
                write!(
                    formatter,
                    "could not create audio segment directory {}: {message}",
                    path.display()
                )
            }
            Self::AudioSegmentCollision {
                day,
                stream,
                start,
                attempts,
            } => write!(
                formatter,
                "audio segment collision for {day}/{stream} at {start} after {attempts} attempts"
            ),
            Self::AudioSegmentDayOverflow { day, stream, start } => write!(
                formatter,
                "audio segment collision probe crosses day boundary for {day}/{stream} at {start}"
            ),
            Self::NoAudioSegmentsCreated { path } => {
                write!(
                    formatter,
                    "no audio segments created from {}",
                    path.display()
                )
            }
            Self::AudioProcessingWait { detail } => {
                write!(formatter, "audio processing wait failed: {detail}")
            }
        }
    }
}

impl std::error::Error for ImportError {}

/// One importer module name and its reserved skeleton seam.
pub type ModuleStub = (&'static str, fn() -> Result<(), ImportError>);

/// The importer crate has no remaining reserved module seams.
pub const MODULE_STUBS: &[ModuleStub] = &[];

/// Ordered metadata retained for one staged import.
pub type OrderedMetadata = Map<String, Value>;

/// Record of a force-reimport audit action.
#[derive(Debug, Clone)]
pub struct ForceReimportAudit {
    pub import_dir: PathBuf,
    pub inventory: Value,
    pub days_affected: Vec<String>,
    pub dry_run: bool,
}

impl ForceReimportAudit {
    #[must_use]
    pub fn params(&self) -> Value {
        let mut params = Map::new();
        params.insert(
            "import_dir".to_owned(),
            Value::String(self.import_dir.display().to_string()),
        );
        params.insert("inventory".to_owned(), self.inventory.clone());
        params.insert(
            "days_affected".to_owned(),
            Value::Array(
                self.days_affected
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
        params.insert("dry_run".to_owned(), Value::Bool(self.dry_run));
        Value::Object(params)
    }
}

/// Failure returned by the force-reimport audit sink.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AuditSinkError {
    pub message: String,
}

impl fmt::Display for AuditSinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AuditSinkError {}

/// Failure returned while removing a force-reimport directory.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RemovalError {
    pub message: String,
}

impl fmt::Display for RemovalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RemovalError {}

/// Ordered effects for a forced reimport.
///
/// The production journal-action sink is wired by a later owner-facing verb.
/// Keeping both effects in this port makes their order testable without adding a
/// production dependency on the facets writer.
pub trait ImportForceEffects {
    fn append_force_reimport(&self, audit: &ForceReimportAudit) -> Result<(), AuditSinkError>;

    fn remove_import_directory(&self, import_dir: &Path) -> Result<(), RemovalError>;
}
