// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only import resolution.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crate::SourceHash;
use crate::dedupe::hash_source;
use crate::stream_name::{StreamNameError, import_stream_name};
use crate::timestamp::{
    AutoTimestamp, DetectedTimestamp, Timestamp, TimestampError, validate_timestamp,
};

pub const ORDERED_FILE_IMPORTER_NAMES: &[&str] = &[
    "ics",
    "obsidian",
    "claude",
    "chatgpt",
    "kindle",
    "gemini",
    "document",
    "image",
    "journal_archive",
    "apple_health",
    "oura",
];

pub const OURA_SYNC_REMEDY: &str =
    "Oura body data imports through sync; use journal importer --sync oura";

// `cli.py:549` skips the registry sweep for generic audio/text extensions.
const DETECTION_SKIP_EXTENSIONS: &[&str] = &["m4a", "txt", "md"];
// `cli.py:688` selects text streams only for these generic extensions.
const TEXT_STREAM_EXTENSIONS: &[&str] = &["txt", "md"];

/// A compiled-in file importer selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrySource {
    Ics,
    Obsidian,
    Claude,
    Chatgpt,
    Kindle,
    Gemini,
    Document,
    Image,
    JournalArchive,
    AppleHealth,
    Oura,
}

impl RegistrySource {
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "ics" => Self::Ics,
            "obsidian" => Self::Obsidian,
            "claude" => Self::Claude,
            "chatgpt" => Self::Chatgpt,
            "kindle" => Self::Kindle,
            "gemini" => Self::Gemini,
            "document" => Self::Document,
            "image" => Self::Image,
            "journal_archive" => Self::JournalArchive,
            "apple_health" => Self::AppleHealth,
            "oura" => Self::Oura,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ics => "ics",
            Self::Obsidian => "obsidian",
            Self::Claude => "claude",
            Self::Chatgpt => "chatgpt",
            Self::Kindle => "kindle",
            Self::Gemini => "gemini",
            Self::Document => "document",
            Self::Image => "image",
            Self::JournalArchive => "journal_archive",
            Self::AppleHealth => "apple_health",
            Self::Oura => "oura",
        }
    }
}

/// Owner-facing options, after argv parsing has happened elsewhere.
pub struct ResolutionOptions<'a> {
    pub media: &'a Path,
    pub source: Option<&'a str>,
    pub timestamp: Option<&'a str>,
    pub auto: AutoTimestamp,
    pub dry_run: bool,
    pub deterministic_only: bool,
    pub force: bool,
}

/// Named externally-owned resolution decisions.
pub struct ResolutionSeams<A, C, D, M, L, T> {
    /// Called only by the source-absent Apple directory/ZIP pre-empt; errors stop resolution.
    pub apple_detector: A,
    /// Called in registry order; an error is a swallowed non-answer, matching Python's sweep.
    pub claims: C,
    /// Called for generic timestamps with the path and optional original filename; its implementation is later work.
    pub deterministic_detector: D,
    /// Called once after a deterministic generic no-match, with optional auto guidance.
    pub model_detector: M,
    /// Called only for non-dry generic deduplication; `None` means no previous manifest.
    pub manifest_lookup: L,
    /// Called only for a selected registry source missing an explicit timestamp.
    pub generated_timestamp: T,
}

/// Model detector errors classified at the seam boundary.
pub enum ModelDetectionError<E> {
    /// Reference no-engine, validation, or parse errors; resolution treats this as no detection.
    Unavailable,
    /// A provider failure that must become the owner-facing named remedy.
    Failed(E),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManifestSummary {
    pub entry_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SkipReason {
    AlreadyImported,
    NoDeterministicMatch,
    TimestampRequired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedSource {
    Registry(RegistrySource),
    GenericAudio,
    GenericText,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolutionOutcome {
    RouteAppleHealth,
    Skipped {
        reason: SkipReason,
        detected_timestamp: Option<Timestamp>,
    },
    Resolved {
        source: ResolvedSource,
        timestamp: Timestamp,
        stream: String,
    },
}

#[derive(Debug)]
pub enum ResolutionError<AE, ME> {
    MissingPath,
    AppleDetection(AE),
    PdfRequiresDocumentImporter {
        path: PathBuf,
    },
    OuraRequiresSync,
    InvalidTimestampShape,
    InvalidTimestampCalendar {
        message: String,
    },
    ModelDetectionFailed {
        source: ME,
    },
    CouldNotDetectTimestamp,
    Hash(crate::ImportError),
    StreamName(StreamNameError),
    /// Deliberate divergence: Python later dies staging an unclaimed directory.
    UnclaimedDirectory,
}

impl<AE, ME> ResolutionError<AE, ME> {
    #[must_use]
    pub fn message(&self) -> Cow<'_, str> {
        match self {
            Self::MissingPath => Cow::Borrowed("source path does not exist"),
            Self::AppleDetection(_) => {
                Cow::Borrowed("could not inspect Apple Health export; retry with a valid export")
            }
            Self::PdfRequiresDocumentImporter { path } => Cow::Owned(format!(
                "PDF import requires the document importer: {}",
                path.display()
            )),
            Self::OuraRequiresSync => Cow::Borrowed(OURA_SYNC_REMEDY),
            Self::InvalidTimestampShape => {
                Cow::Borrowed("timestamp must be YYYYMMDD_HHMMSS format")
            }
            Self::InvalidTimestampCalendar { message } => Cow::Borrowed(message),
            Self::ModelDetectionFailed { .. } => Cow::Borrowed(
                "could not detect timestamp; provide --timestamp or configure a model",
            ),
            Self::CouldNotDetectTimestamp => Cow::Borrowed(
                "Could not detect timestamp. Please provide --timestamp YYYYMMDD_HHMMSS",
            ),
            Self::Hash(_) => Cow::Borrowed("could not hash source"),
            Self::StreamName(_) => Cow::Borrowed("could not derive import stream name"),
            Self::UnclaimedDirectory => Cow::Borrowed(
                "path is a directory and no importer claimed it; select an importer or import a file",
            ),
        }
    }
}

/// Resolves a source without staging it or writing any journal state.
pub fn resolve_import<A, C, D, M, L, T, AE, CE, ME>(
    options: &ResolutionOptions<'_>,
    seams: &mut ResolutionSeams<A, C, D, M, L, T>,
) -> Result<ResolutionOutcome, ResolutionError<AE, ME>>
where
    A: FnMut(&Path) -> Result<bool, AE>,
    C: FnMut(RegistrySource, &Path) -> Result<bool, CE>,
    D: FnMut(&Path, Option<&str>) -> Option<DetectedTimestamp>,
    M: FnMut(&Path, Option<&str>) -> Result<Option<DetectedTimestamp>, ModelDetectionError<ME>>,
    L: FnMut(&SourceHash) -> Option<ManifestSummary>,
    T: FnMut() -> Timestamp,
{
    if !options.media.exists() {
        return Err(ResolutionError::MissingPath);
    }
    let mut selected = None;
    if options.source.is_none()
        && (options.media.is_dir() || has_extension(options.media, "zip"))
        && (seams.apple_detector)(options.media).map_err(ResolutionError::AppleDetection)?
    {
        return Ok(ResolutionOutcome::RouteAppleHealth);
    }
    if let Some(name) = options.source {
        selected = RegistrySource::from_name(name);
    }
    if selected.is_none() {
        let sweep =
            options.media.is_dir() || !has_extension_in(options.media, DETECTION_SKIP_EXTENSIONS);
        if sweep {
            for name in ORDERED_FILE_IMPORTER_NAMES {
                let source =
                    RegistrySource::from_name(name).expect("compiled registry names are valid");
                if (seams.claims)(source, options.media).unwrap_or(false) {
                    selected = Some(source);
                    break;
                }
            }
        }
    }
    if selected.is_none() && has_extension(options.media, "pdf") {
        return Err(ResolutionError::PdfRequiresDocumentImporter {
            path: options.media.to_owned(),
        });
    }
    match selected {
        Some(RegistrySource::AppleHealth) => return Ok(ResolutionOutcome::RouteAppleHealth),
        Some(RegistrySource::Oura) => return Err(ResolutionError::OuraRequiresSync),
        _ => {}
    }
    if selected.is_none() && !options.dry_run {
        let hash = hash_source(options.media).map_err(ResolutionError::Hash)?;
        if !options.force
            && (seams.manifest_lookup)(&hash).is_some_and(|manifest| manifest.entry_count > 0)
        {
            return Ok(ResolutionOutcome::Skipped {
                reason: SkipReason::AlreadyImported,
                detected_timestamp: None,
            });
        }
    }
    let timestamp = if let Some(raw) = options.timestamp {
        validate(raw)?
    } else if selected.is_some() {
        (seams.generated_timestamp)()
    } else {
        let detected = (seams.deterministic_detector)(
            options.media,
            options.media.file_name().and_then(|value| value.to_str()),
        );
        let detected = match detected {
            Some(answer) => Some(answer),
            None if options.deterministic_only => {
                return Ok(ResolutionOutcome::Skipped {
                    reason: SkipReason::NoDeterministicMatch,
                    detected_timestamp: None,
                });
            }
            None => match (seams.model_detector)(options.media, options.auto.guidance()) {
                Ok(answer) => answer,
                Err(ModelDetectionError::Unavailable) => None,
                Err(ModelDetectionError::Failed(source)) => {
                    return Err(ResolutionError::ModelDetectionFailed { source });
                }
            },
        };
        match detected {
            Some(answer) if options.auto.adopts() => answer.timestamp,
            Some(answer) => {
                return Ok(ResolutionOutcome::Skipped {
                    reason: SkipReason::TimestampRequired,
                    detected_timestamp: Some(answer.timestamp),
                });
            }
            None => return Err(ResolutionError::CouldNotDetectTimestamp),
        }
    };
    if selected.is_none() && options.media.is_dir() {
        // Deliberate divergence from cli.py:681-690: its derived label is unreachable because staging raises IsADirectoryError.
        return Err(ResolutionError::UnclaimedDirectory);
    }
    let source = match selected {
        Some(source) => ResolvedSource::Registry(source),
        None if has_extension_in(options.media, TEXT_STREAM_EXTENSIONS) => {
            ResolvedSource::GenericText
        }
        None => ResolvedSource::GenericAudio,
    };
    let label = match source {
        ResolvedSource::Registry(value) => value.name(),
        ResolvedSource::GenericText => "text",
        ResolvedSource::GenericAudio => "audio",
    };
    Ok(ResolutionOutcome::Resolved {
        source,
        timestamp,
        stream: import_stream_name(label).map_err(ResolutionError::StreamName)?,
    })
}

fn validate<AE, ME>(raw: &str) -> Result<Timestamp, ResolutionError<AE, ME>> {
    validate_timestamp(raw).map_err(|error| match error {
        TimestampError::Shape => ResolutionError::InvalidTimestampShape,
        TimestampError::Calendar { message } => {
            ResolutionError::InvalidTimestampCalendar { message }
        }
    })
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
}
fn has_extension(path: &Path, expected: &str) -> bool {
    extension(path).as_deref() == Some(expected)
}

fn has_extension_in(path: &Path, expected: &[&str]) -> bool {
    extension(path).is_some_and(|extension| expected.contains(&extension.as_str()))
}

pub fn reserved_seam() -> Result<(), crate::ImportError> {
    Err(crate::ImportError::Unimplemented { module: "detect" })
}
