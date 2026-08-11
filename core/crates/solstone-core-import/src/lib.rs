// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native importer contracts and journal-host grammar seams.

use std::fmt;

pub mod audio;
pub mod cli_argv;
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
pub mod sync_audio;
pub mod sync_obsidian;
pub mod sync_plaud;
pub mod sync_state;
pub mod text;
pub mod timestamp;

pub use contract::{
    ImportPreview, ImportResult, OwnerSource, OwnerSourceMetadata, PreviewRequest, SaveRequest,
    SourceHash, SourceImmutabilityReport, observe_source_immutability, should_write_manifest,
};
pub use events::{
    EnrichmentReady, EventEmitter, FileImported, ImporterCompleted, ImporterError, ImporterStarted,
    ImporterStatus, ObservedSegment, ObservingMeta, ObservingSegment, emit_enrichment_ready,
    emit_file_imported, emit_importer_completed, emit_importer_error, emit_importer_started,
    emit_importer_status, emit_observe_observed, emit_observe_observing, emit_supervisor_drain,
};
pub use publish::{
    CreatedSegment, DayMarkerOutcome, DayMarkerStatus, IndexPublicationOutcomes, IndexedFile,
    IndexedFileError, PublicationInput, PublicationOperations, PublicationRecord,
    PublicationStatus, PublishError, SegmentBindingOutcome, SegmentPublicationOutcome, publish,
    publish_with_operations, read_publication_record, write_publication_record,
};

/// Error returned by an importer seam that has no implementation yet.
#[derive(Debug, Eq, PartialEq)]
pub enum ImportError {
    Unimplemented { module: &'static str },
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unimplemented { module } => write!(formatter, "import: unimplemented: {module}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// One importer module name and its reserved skeleton seam.
pub type ModuleStub = (&'static str, fn() -> Result<(), ImportError>);

/// The complete skeleton importer module inventory.
pub const MODULE_STUBS: &[ModuleStub] = &[
    ("contract", contract::reserved_seam),
    ("detect", detect::reserved_seam),
    ("timestamp", timestamp::reserved_seam),
    ("staging", staging::reserved_seam),
    ("metadata", metadata::reserved_seam),
    ("dedupe", dedupe::reserved_seam),
    ("audio", audio::reserved_seam),
    ("text", text::reserved_seam),
    ("consent_gate", consent_gate::reserved_seam),
    ("sync_state", sync_state::reserved_seam),
    ("sync_plaud", sync_plaud::reserved_seam),
    ("sync_obsidian", sync_obsidian::reserved_seam),
    ("sync_audio", sync_audio::reserved_seam),
    ("connect", connect::reserved_seam),
    ("cli_argv", cli_argv::reserved_seam),
    ("cli_journal_source", cli_journal_source::reserved_seam),
    ("cli_render", cli_render::reserved_seam),
];
