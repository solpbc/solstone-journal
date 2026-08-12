// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native importer source registry and source-specific seams.

use std::fmt;
use std::path::PathBuf;

pub mod apple_health;
pub mod archive;
pub mod chatgpt;
pub mod claude;
pub mod document;
pub mod gemini;
pub mod ics;
pub mod image;
pub mod kindle;
pub mod obsidian;
pub mod oura;
pub mod registry;
pub mod shared;

pub use shared::{
    ImportPlan, PlannedEntry, PlannedSegment, SkipLocator, SkipReason, SkippedEntry, SourceError,
};

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
        detail: String,
    },
    ArchiveEntryEncrypted {
        phase: ArchiveSafetyPhase,
        entry: String,
    },
    ArchiveUnsafeEntry {
        phase: ArchiveSafetyPhase,
        entry: String,
        reason: String,
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
        detail: String,
    },
    ExtractionCleanupFailed {
        extraction_dir: PathBuf,
        detail: String,
    },
    LockBusy {
        protected_path: PathBuf,
        sidecar_path: PathBuf,
        owner_metadata_path: PathBuf,
        owner: Option<String>,
        remedy: String,
    },
    LockFailed {
        path: PathBuf,
        detail: String,
    },
    DecisionLogWrite {
        path: PathBuf,
        detail: String,
    },
    StagingWrite {
        path: PathBuf,
        detail: String,
    },
    SegmentMerge {
        path: PathBuf,
        detail: String,
    },
    EntityMerge {
        entity_id: String,
        detail: String,
    },
    FacetMerge {
        facet: String,
        detail: String,
    },
    ImportMerge {
        path: PathBuf,
        detail: String,
    },
}

impl fmt::Display for ImportSourcesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unimplemented { module } => {
                write!(formatter, "import-sources: unimplemented: {module}")
            }
            Self::ArchiveNotFound { path } => {
                write!(formatter, "archive not found: {}", path.display())
            }
            Self::ArchiveTooLarge {
                path,
                bytes,
                maximum,
            } => write!(
                formatter,
                "archive {} is {bytes} bytes; maximum is {maximum}",
                path.display()
            ),
            Self::ArchiveInvalid { path, detail } => {
                write!(formatter, "invalid archive {}: {detail}", path.display())
            }
            Self::ArchiveEntryEncrypted { phase, entry } => {
                write!(formatter, "encrypted archive entry during {phase}: {entry}")
            }
            Self::ArchiveUnsafeEntry {
                phase,
                entry,
                reason,
            } => write!(
                formatter,
                "unsafe archive entry during {phase}: {entry}: {reason}"
            ),
            Self::ArchiveUncompressedTooLarge { bytes, maximum } => write!(
                formatter,
                "archive expands to {bytes} bytes; maximum is {maximum}"
            ),
            Self::ArchiveInsufficientSpace {
                available,
                required,
                path,
            } => write!(
                formatter,
                "insufficient free space at {}: {available} available, {required} required",
                path.display()
            ),
            Self::ExtractionFailed {
                archive,
                extraction_dir,
                detail,
            } => write!(
                formatter,
                "failed extracting {} into {}: {detail}",
                archive.display(),
                extraction_dir.display()
            ),
            Self::ExtractionCleanupFailed {
                extraction_dir,
                detail,
            } => write!(
                formatter,
                "failed cleaning extraction {}: {detail}",
                extraction_dir.display()
            ),
            Self::LockBusy {
                protected_path,
                remedy,
                ..
            } => write!(
                formatter,
                "archive merge lock is busy at {}: {remedy}",
                protected_path.display()
            ),
            Self::LockFailed { path, detail } => write!(
                formatter,
                "archive merge lock failed at {}: {detail}",
                path.display()
            ),
            Self::DecisionLogWrite { path, detail } => write!(
                formatter,
                "decision log write failed at {}: {detail}",
                path.display()
            ),
            Self::StagingWrite { path, detail } => write!(
                formatter,
                "staging write failed at {}: {detail}",
                path.display()
            ),
            Self::SegmentMerge { path, detail } => write!(
                formatter,
                "segment merge failed at {}: {detail}",
                path.display()
            ),
            Self::EntityMerge { entity_id, detail } => {
                write!(formatter, "entity merge failed for {entity_id}: {detail}")
            }
            Self::FacetMerge { facet, detail } => {
                write!(formatter, "facet merge failed for {facet}: {detail}")
            }
            Self::ImportMerge { path, detail } => write!(
                formatter,
                "import merge failed at {}: {detail}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ImportSourcesError {}

/// One source module name and its reserved skeleton seam.
pub type ModuleStub = (&'static str, fn() -> Result<(), ImportSourcesError>);

/// The remaining importer source modules with reserved skeleton seams.
pub const MODULE_STUBS: &[ModuleStub] = &[
    ("registry", registry::reserved_seam),
    ("apple_health", apple_health::reserved_seam),
    ("oura", oura::reserved_seam),
];
