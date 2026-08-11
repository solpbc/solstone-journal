// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only import resolution.

use std::path::Path;

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
pub struct ResolutionSeams<A, C, D, M, L> {
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
    PdfRequiresDocumentImporter,
    OuraRequiresSync,
    InvalidTimestampShape,
    InvalidTimestampCalendar {
        message: String,
    },
    ModelDetectionFailed {
        source: ME,
    },
    Hash(crate::ImportError),
    StreamName(StreamNameError),
    /// Deliberate divergence: Python later dies staging an unclaimed directory.
    UnclaimedDirectory,
}

impl<AE, ME> ResolutionError<AE, ME> {
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::MissingPath => "source path does not exist",
            Self::AppleDetection(_) => {
                "could not inspect Apple Health export; retry with a valid export"
            }
            Self::PdfRequiresDocumentImporter => "PDF imports require the document importer",
            Self::OuraRequiresSync => OURA_SYNC_REMEDY,
            Self::InvalidTimestampShape => "timestamp must be YYYYMMDD_HHMMSS format",
            Self::InvalidTimestampCalendar { message } => message,
            Self::ModelDetectionFailed { .. } => {
                "could not detect timestamp; provide --timestamp or configure a model"
            }
            Self::Hash(_) => "could not hash source",
            Self::StreamName(_) => "could not derive import stream name",
            Self::UnclaimedDirectory => {
                "path is a directory and no importer claimed it; select an importer or import a file"
            }
        }
    }
}

/// Resolves a source without staging it or writing any journal state.
pub fn resolve_import<A, C, D, M, L, AE, CE, ME>(
    options: &ResolutionOptions<'_>,
    seams: &mut ResolutionSeams<A, C, D, M, L>,
) -> Result<ResolutionOutcome, ResolutionError<AE, ME>>
where
    A: FnMut(&Path) -> Result<bool, AE>,
    C: FnMut(RegistrySource, &Path) -> Result<bool, CE>,
    D: FnMut(&Path, Option<&str>) -> Option<DetectedTimestamp>,
    M: FnMut(&Path, Option<&str>) -> Result<Option<DetectedTimestamp>, ME>,
    L: FnMut(&SourceHash) -> Option<ManifestSummary>,
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
        let sweep = options.media.is_dir()
            || !matches!(
                extension(options.media).as_deref(),
                Some("m4a" | "txt" | "md")
            );
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
        return Err(ResolutionError::PdfRequiresDocumentImporter);
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
        now_timestamp()
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
            None => (seams.model_detector)(options.media, options.auto.guidance())
                .map_err(|source| ResolutionError::ModelDetectionFailed { source })?,
        };
        match detected {
            Some(answer) if options.auto.adopts() => answer.timestamp,
            Some(answer) => {
                return Ok(ResolutionOutcome::Skipped {
                    reason: SkipReason::TimestampRequired,
                    detected_timestamp: Some(answer.timestamp),
                });
            }
            None => {
                return Ok(ResolutionOutcome::Skipped {
                    reason: SkipReason::NoDeterministicMatch,
                    detected_timestamp: None,
                });
            }
        }
    };
    if selected.is_none() && options.media.is_dir() {
        // Deliberate divergence from cli.py:681-690: its derived label is unreachable because staging raises IsADirectoryError.
        return Err(ResolutionError::UnclaimedDirectory);
    }
    let source = match selected {
        Some(source) => ResolvedSource::Registry(source),
        None if matches!(extension(options.media).as_deref(), Some("txt" | "md")) => {
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

fn now_timestamp() -> Timestamp {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time is after the Unix epoch")
        .as_secs();
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    validate_timestamp(&format!(
        "{year:04}{month:02}{day:02}_{:02}{:02}{:02}",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    ))
    .expect("formatted UTC time is valid")
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month as u32, day as u32)
}
fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
}
fn has_extension(path: &Path, expected: &str) -> bool {
    extension(path).as_deref() == Some(expected)
}

pub fn reserved_seam() -> Result<(), crate::ImportError> {
    Err(crate::ImportError::Unimplemented { module: "detect" })
}
