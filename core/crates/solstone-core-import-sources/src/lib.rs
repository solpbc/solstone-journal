// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native importer source registry and source-specific seams.

use std::fmt;
use std::path::PathBuf;

pub mod apple_export;
pub mod apple_health;
pub mod apple_health_run;
pub mod archive;
pub mod attachment;
pub mod body_placement;
pub mod body_runs;
pub mod body_space;
pub mod body_status;
pub mod body_stream;
pub mod chatgpt;
pub mod claude;
pub mod conversations;
pub mod document;
pub mod evernote;
pub mod garmin;
pub mod gemini;
pub mod ics;
mod ics_calendar;
pub mod ics_notes;
mod ics_recur;
pub mod image;
mod nofollow_open;
pub mod oura;
pub mod oura_run;
pub mod photo_faces;
pub mod producer;
pub mod registry;
pub mod save;
pub mod shared;
pub mod strava;
mod zip_names;

pub use producer::{
    NativeProducerError, NativeProducerOutcome, NativeProducerRequest, NullDocumentModelClient,
    NullPdfWorker, NullWireClient, run_native_producer,
};
pub use shared::{
    ImportPlan, PlannedEntry, PlannedSegment, SkipLocator, SkipReason, SkippedEntry, SourceError,
};
use solstone_core_cli_copy::MessageDetail;

pub use strava::{looks_like_strava_csv_bytes, looks_like_strava_download};

/// The independent safety layer that rejected an archive entry.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ArchiveSafetyPhase {
    Validation,
    Extraction,
}

impl fmt::Display for ArchiveSafetyPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Validation => "validation",
            Self::Extraction => "extraction",
        })
    }
}

/// Structured mutation disposition for archive merge failures.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MergeMutationState {
    NotMutated,
    MayHaveMutated,
    Unknown,
}

/// Error returned by a source seam or archive merge transaction.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ImportSourcesError {
    Unimplemented {
        module: &'static str,
    },
    ArchiveNotFound {
        path: PathBuf,
    },
    ArchiveTooLarge {
        path: PathBuf,
        bytes: u64,
        maximum: u64,
    },
    ArchiveInvalid {
        path: PathBuf,
        detail: MessageDetail,
    },
    ArchiveEntryEncrypted {
        phase: ArchiveSafetyPhase,
        entry: String,
    },
    ArchiveUnsafeEntry {
        phase: ArchiveSafetyPhase,
        entry: String,
        reason: MessageDetail,
    },
    ArchiveUncompressedTooLarge {
        bytes: u64,
        maximum: u64,
    },
    ArchiveInsufficientSpace {
        available: u64,
        required: u64,
        path: PathBuf,
    },
    ExtractionFailed {
        archive: PathBuf,
        extraction_dir: PathBuf,
        detail: MessageDetail,
    },
    ExtractionCleanupFailed {
        extraction_dir: PathBuf,
        detail: MessageDetail,
        mutation: MergeMutationState,
    },
    // The two secondary paths are boxed so the error stays small on Windows, where
    // `PathBuf` is wider (clippy `result_large_err`).
    LockBusy {
        protected_path: PathBuf,
        sidecar_path: Box<PathBuf>,
        owner_metadata_path: Box<PathBuf>,
        owner: Option<String>,
        remedy: MessageDetail,
    },
    LockFailed {
        path: PathBuf,
        detail: MessageDetail,
    },
    DecisionLogWrite {
        path: PathBuf,
        detail: MessageDetail,
    },
    StagingWrite {
        path: PathBuf,
        detail: MessageDetail,
    },
    SegmentMerge {
        path: PathBuf,
        detail: MessageDetail,
    },
    EntityMerge {
        entity_id: String,
        detail: MessageDetail,
    },
    FacetMerge {
        facet: String,
        detail: MessageDetail,
    },
    ImportMerge {
        path: PathBuf,
        detail: MessageDetail,
    },
    MergePublishFailed {
        detail: MessageDetail,
        mutation: MergeMutationState,
    },
}

impl ImportSourcesError {
    #[must_use]
    pub fn mutation_state(&self) -> MergeMutationState {
        match self {
            Self::ExtractionCleanupFailed { mutation, .. }
            | Self::MergePublishFailed { mutation, .. } => *mutation,
            Self::Unimplemented { .. }
            | Self::ArchiveNotFound { .. }
            | Self::ArchiveTooLarge { .. }
            | Self::ArchiveInvalid { .. }
            | Self::ArchiveEntryEncrypted { .. }
            | Self::ArchiveUnsafeEntry { .. }
            | Self::ArchiveUncompressedTooLarge { .. }
            | Self::ArchiveInsufficientSpace { .. }
            | Self::ExtractionFailed { .. }
            | Self::LockBusy { .. }
            | Self::LockFailed { .. }
            | Self::DecisionLogWrite { .. }
            | Self::StagingWrite { .. }
            | Self::SegmentMerge { .. }
            | Self::EntityMerge { .. }
            | Self::FacetMerge { .. }
            | Self::ImportMerge { .. } => MergeMutationState::NotMutated,
        }
    }
}

impl ImportSourcesError {
    pub fn terminal_message(&self, locale: &str) -> String {
        self.message().render(locale)
    }

    fn message(&self) -> MessageDetail {
        match self {
            Self::Unimplemented { module } => MessageDetail::message(
                "journal.cli.journal.archive.import.unimplemented",
                [("module", &(module))],
            ),
            Self::ArchiveNotFound { path } => MessageDetail::message(
                "journal.cli.journal.archive.import.missing",
                [("path", &(path.display()))],
            ),
            Self::ArchiveTooLarge {
                path,
                bytes,
                maximum,
            } => MessageDetail::cardinal(
                "journal.cli.journal.archive.import.compressed",
                [(
                    "count",
                    i64::from(i32::try_from(*bytes).unwrap_or(i32::MAX)),
                )],
                format!(
                    "archive {} is {bytes} bytes; maximum is {maximum}",
                    path.display()
                ),
                [
                    ("bytes", bytes),
                    ("maximum", maximum),
                    ("path", &path.display()),
                ],
            ),
            Self::ArchiveUncompressedTooLarge { bytes, maximum } => MessageDetail::cardinal(
                "journal.cli.journal.archive.import.expanded",
                [(
                    "count",
                    i64::from(i32::try_from(*bytes).unwrap_or(i32::MAX)),
                )],
                format!("archive expands to {bytes} bytes; maximum is {maximum}"),
                [("bytes", bytes), ("maximum", maximum)],
            ),
            Self::ArchiveInvalid { path, detail } => detail.clone().context_with(
                "journal.cli.journal.archive.import.invalid",
                [("path", &(path.display()))],
            ),
            Self::ArchiveEntryEncrypted {
                phase: ArchiveSafetyPhase::Validation,
                entry,
            } => MessageDetail::message(
                "journal.cli.journal.archive.import.encrypted.validation",
                [("entry", &(entry))],
            ),
            Self::ArchiveUnsafeEntry {
                phase: ArchiveSafetyPhase::Validation,
                entry,
                reason,
            } => reason.clone().context_with(
                "journal.cli.journal.archive.import.unsafe.validation",
                [("entry", &(entry))],
            ),
            Self::ArchiveEntryEncrypted {
                phase: ArchiveSafetyPhase::Extraction,
                entry,
            } => MessageDetail::message(
                "journal.cli.journal.archive.import.encrypted.extraction",
                [("entry", &(entry))],
            ),
            Self::ArchiveUnsafeEntry {
                phase: ArchiveSafetyPhase::Extraction,
                entry,
                reason,
            } => reason.clone().context_with(
                "journal.cli.journal.archive.import.unsafe.extraction",
                [("entry", &(entry))],
            ),
            Self::ArchiveInsufficientSpace {
                available,
                required,
                path,
            } => MessageDetail::message(
                "journal.cli.journal.archive.import.space",
                [
                    ("path", &(path.display())),
                    ("available", &(available)),
                    ("required", &(required)),
                ],
            ),
            Self::ExtractionFailed {
                archive,
                extraction_dir,
                detail,
            } => detail.clone().context_with(
                "journal.cli.journal.archive.import.extract",
                [
                    ("archive", &(archive.display())),
                    ("dir", &(extraction_dir.display())),
                ],
            ),
            Self::ExtractionCleanupFailed {
                extraction_dir,
                detail,
                ..
            } => detail.clone().context_with(
                "journal.cli.journal.archive.import.cleanup",
                [("path", &(extraction_dir.display()))],
            ),
            Self::LockBusy {
                protected_path,
                remedy,
                ..
            } => remedy.clone().context_with(
                "journal.cli.journal.archive.import.busy",
                [("path", &(protected_path.display()))],
            ),
            Self::LockFailed { path, detail, .. } => detail.clone().context_with(
                "journal.cli.journal.archive.import.lock",
                [("path", &(path.display()))],
            ),
            Self::DecisionLogWrite { path, detail, .. } => detail.clone().context_with(
                "journal.cli.journal.archive.import.log",
                [("path", &(path.display()))],
            ),
            Self::StagingWrite { path, detail, .. } => detail.clone().context_with(
                "journal.cli.journal.archive.import.stage",
                [("path", &(path.display()))],
            ),
            Self::SegmentMerge { path, detail, .. } => detail.clone().context_with(
                "journal.cli.journal.archive.import.segment",
                [("path", &(path.display()))],
            ),
            Self::EntityMerge {
                entity_id, detail, ..
            } => detail.clone().context_with(
                "journal.cli.journal.archive.import.entity",
                [("entity_id", &(entity_id))],
            ),
            Self::FacetMerge { facet, detail, .. } => detail.clone().context_with(
                "journal.cli.journal.archive.import.facet",
                [("facet", &(facet))],
            ),
            Self::ImportMerge { path, detail, .. } => detail.clone().context_with(
                "journal.cli.journal.archive.import.merge",
                [("path", &(path.display()))],
            ),
            Self::MergePublishFailed { detail, .. } => detail
                .clone()
                .context_with("journal.cli.journal.archive.import.publish", []),
        }
    }
}

impl fmt::Display for ImportSourcesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message().fmt(formatter)
    }
}

impl std::error::Error for ImportSourcesError {}

/// One source module name and its reserved skeleton seam.
pub type ModuleStub = (&'static str, fn() -> Result<(), ImportSourcesError>);

/// The remaining importer source modules with reserved skeleton seams.
pub const MODULE_STUBS: &[ModuleStub] = &[
    ("registry", registry::reserved_seam),
    ("oura", oura::reserved_seam),
];

#[cfg(test)]
mod archive_size_copy_tests {
    use super::ImportSourcesError;

    #[test]
    fn archive_refusals_show_the_full_byte_count_past_the_plural_selector_limit() {
        let bytes = u64::MAX;
        let maximum = 50 * 1024_u64.pow(3);
        for error in [
            ImportSourcesError::ArchiveTooLarge {
                path: "large.zip".into(),
                bytes,
                maximum,
            },
            ImportSourcesError::ArchiveUncompressedTooLarge { bytes, maximum },
        ] {
            let line = error.terminal_message("en-US");
            assert_eq!(line, error.to_string());
            assert!(line.contains(&format!("{bytes} bytes; maximum is {maximum}")));
        }
    }
}
