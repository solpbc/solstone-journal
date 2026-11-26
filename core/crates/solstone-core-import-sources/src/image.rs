// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native single-image import source.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use base64::Engine;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use jiff::civil::DateTime;
use jiff::tz::{Offset, TimeZone};
use jiff::{SignedDuration, ZonedDateTime};
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, OneShotClient, RefusalReason,
};
use solstone_core_import::{
    CreatedSegment, ImportPreview, ManifestWriteRequest, PublicationOperations, hash_source,
    write_manifest,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, atomic_replace, contained_path, create_directory_with_mode, install_file,
    path_lexists, write_text,
};
use solstone_core_segment::SegmentDir;
use solstone_core_segment::photo_observation::{
    AttributedText, BelongingClass, PHOTO_OBSERVATION_SCHEMA, PhotoObservation,
    PhotoObservationSource, PhotoObservationState, PhotoObservationTime, PhotoTimePrecision,
    PhotoTimeProvenance, read_or_project_photo_observation, write_photo_observation,
};
use tempfile::NamedTempFile;

/// The formats this source both decodes and can hand to a vision model. HEIC, HEIF and TIFF
/// are deliberately absent: the build carries no decoder for them, so offering them only
/// ends in an undecodable-source failure.
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];
const IMPORT_STREAM: &str = "import.image";
use sha2::{Digest, Sha256};
use solstone_core_import::SourceHash;
use solstone_core_import::triage::{self, Kind as ContentKind, Verdict};

const SCREENSHOT_TRANSCRIPT: &str = "screenshot_transcript.md";
const TRANSCRIPT_FILENAME: &str = "image_transcript.md";
const IMAGE_META_FILENAME: &str = "image_meta.json";
const VISION_MAX_LONG_EDGE: u32 = 1920;
const VISION_JPEG_QUALITY: u8 = 85;
const VISION_SCHEMA_JSON: &str = include_str!("image_vision.schema.json");
const VISION_PROMPT: &str = "Describe what is in this image faithfully and concisely. \
Transcribe every legible piece of text verbatim, including small print, labels, signs, receipts, whiteboard writing, and URLs only when visible (never invent URLs). Return JSON:\n\
- description: concise plain visual description of the scene/subjects (no Markdown links or image markup).\n\
- visible_text: complete verbatim transcription of all legible text in the image (do not summarize, omit, or truncate any readable lines; separate lines with newlines), or null only if no legible text is visible.\n\
- belonging_class: \"lived_moment\" (scene, people, selfie, place, meal, life), \"reference_to_remember\" (whiteboard, slides, book, document, sign, menu, receipt, note), \"errand_or_utilitarian\" (parking spot, shelf/price tag, serial/label, screen photo), or \"noise_or_accidental\" (pocket shot, accidental blur, blank/obstructed).";
const VISION_CONTEXT: &str = "import.image.vision";
const VISIBLE_TEXT_MARKER: &str = "> [visible text — model-derived]";
const PRIVATE_IMPORT_FILE_MODE: u32 = 0o600;
const PRIVATE_IMPORT_DIR_MODE: u32 = 0o700;
const MODEL_DERIVED_LINE_PREFIX: char = '>';
const IMAGE_SEGMENT_PROBE_LIMIT: u32 = 60;
const REMOVING_SEGMENT_PREFIX: &str = ".removing_";

/// Return the canonical JSON schema used for single-pass photo vision reads.
#[must_use]
pub fn vision_json_schema() -> serde_json::Value {
    serde_json::from_str(VISION_SCHEMA_JSON).expect("embedded image vision schema is valid JSON")
}

/// Progress reported by one image import.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProgressUpdate {
    pub current: u64,
    pub total: u64,
    pub earliest_date: String,
    pub latest_date: String,
    pub entities_found: u64,
}

/// How an imported photo was placed in time.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PhotoTimeSource {
    /// Dated from photo metadata (EXIF or phone share capture timestamp).
    Capture,
    /// No capture date was present, so import time was used as a fallback.
    UploadFallback,
    /// Explicitly chosen by the owner (or inherited from a parent note).
    Owner,
}

impl PhotoTimeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::UploadFallback => "upload_fallback",
            Self::Owner => "owner",
        }
    }
}

/// Optional provenance passed when preparing a standalone imported image or
/// synced library photo.
#[derive(Debug, Clone, Default)]
pub struct ImageImportOptions {
    pub caption: Option<String>,
    pub client_captured_at: Option<String>,
    pub original_filename: Option<String>,
    pub asset_id: Option<String>,
    pub time_provenance: Option<PhotoTimeProvenance>,
    pub capture_location: Option<solstone_core_import::CaptureLocation>,
    pub tz: Option<String>,
}

/// Structured single-pass vision output for a photo.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PhotoVisionReading {
    pub description: String,
    pub visible_text: Option<String>,
    pub belonging_class: Option<BelongingClass>,
}

/// Rich vision outcome distinguishing ready, thinking-off, retryable transient
/// failure, and non-retryable analysis unavailability.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PhotoVisionOutcome {
    Ready(PhotoVisionReading),
    ThinkingOff { reason: String },
    TransientFailure { reason: String },
    AnalysisUnavailable { reason: String },
}

impl PhotoVisionOutcome {
    #[must_use]
    pub fn to_description_outcome(&self) -> DescriptionOutcome {
        match self {
            Self::Ready(reading) => DescriptionOutcome::Generated(reading.description.clone()),
            Self::ThinkingOff { reason }
            | Self::TransientFailure { reason }
            | Self::AnalysisUnavailable { reason } => DescriptionOutcome::Unavailable {
                reason: reason.clone(),
            },
        }
    }

    #[must_use]
    pub const fn state(&self) -> PhotoObservationState {
        match self {
            Self::Ready(_) => PhotoObservationState::Ready,
            Self::ThinkingOff { .. } => PhotoObservationState::ThinkingOff,
            Self::TransientFailure { .. } => PhotoObservationState::TransientFailure,
            Self::AnalysisUnavailable { .. } => PhotoObservationState::AnalysisUnavailable,
        }
    }

    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Ready(_) => None,
            Self::ThinkingOff { reason }
            | Self::TransientFailure { reason }
            | Self::AnalysisUnavailable { reason } => Some(reason.as_str()),
        }
    }

    #[must_use]
    pub fn reading(&self) -> Option<&PhotoVisionReading> {
        match self {
            Self::Ready(reading) => Some(reading),
            _ => None,
        }
    }
}

/// Description status retained with an imported image.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum DescriptionOutcome {
    Generated(String),
    Unavailable { reason: String },
}

/// Result of writing one image source segment and its import manifest.
#[derive(Debug)]
pub struct ImageImportResult {
    pub files_created: Vec<PathBuf>,
    pub created_segment: CreatedSegment,
    pub days_affected: Vec<String>,
    pub description: DescriptionOutcome,
    pub observation: Option<PhotoObservation>,
}

/// Errors raised while importing a source image.
#[derive(Debug)]
pub enum ImageImportError {
    MissingSource {
        path: PathBuf,
    },
    SourceNotFile {
        path: PathBuf,
    },
    UndecodableSource {
        path: PathBuf,
        detail: String,
    },
    Install {
        path: PathBuf,
        detail: String,
    },
    JournalIo {
        path: PathBuf,
        detail: String,
    },
    StreamMarker {
        day: String,
        detail: String,
    },
    Manifest {
        detail: String,
    },
    ImageSegmentDayOverflow {
        day: String,
        stream: String,
        start: String,
    },
    ImageSegmentCollision {
        day: String,
        stream: String,
        start: String,
        attempts: u32,
    },
    CouldNotDetectTimestamp {
        path: PathBuf,
    },
    Stopped,
}

impl fmt::Display for ImageImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CouldNotDetectTimestamp { path } => {
                write!(
                    formatter,
                    "could not detect creation timestamp for image: {}",
                    path.display()
                )
            }
            Self::MissingSource { path } => {
                write!(formatter, "image source is missing: {}", path.display())
            }
            Self::SourceNotFile { path } => {
                write!(formatter, "image source is not a file: {}", path.display())
            }
            Self::UndecodableSource { path, detail } => {
                write!(
                    formatter,
                    "cannot decode image {}: {detail}",
                    path.display()
                )
            }
            Self::Install { path, detail } => {
                write!(
                    formatter,
                    "cannot install image source {}: {detail}",
                    path.display()
                )
            }
            Self::JournalIo { path, detail } => {
                write!(
                    formatter,
                    "cannot write image import {}: {detail}",
                    path.display()
                )
            }
            Self::StreamMarker { day, detail } => write!(
                formatter,
                "original image for {day} remains installed, but could not advance its stream marker: {detail}"
            ),
            Self::Manifest { detail } => {
                write!(formatter, "cannot write image import manifest: {detail}")
            }
            Self::ImageSegmentDayOverflow { day, stream, start } => write!(
                formatter,
                "image segment collision probe crosses day boundary for {day}/{stream} at {start}"
            ),
            Self::ImageSegmentCollision {
                day,
                stream,
                start,
                attempts,
            } => write!(
                formatter,
                "image segment collision for {day}/{stream} at {start} after {attempts} attempts; retrying this file fails until its file time changes"
            ),
            Self::Stopped => formatter.write_str("import stopped"),
        }
    }
}

impl std::error::Error for ImageImportError {}

/// A generate-wire client injected at the image import boundary.
pub trait WireClient {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError>;
}

/// Production generate-wire client.
pub struct SystemWireClient;

impl WireClient for SystemWireClient {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        OneShotClient::sibling()?.execute(request)
    }
}

/// Return whether `extension` (no leading dot, any case) names an advertised image format.
pub fn is_image_extension(extension: &str) -> bool {
    IMAGE_EXTENSIONS
        .iter()
        .any(|expected| extension.eq_ignore_ascii_case(expected))
}

/// Return whether `path` is an advertised image-source file.
pub fn detect(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(is_image_extension)
}

fn read_extra_exif(path: &Path) -> Option<Vec<u8>> {
    let mut reader = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    if !matches!(
        reader.format(),
        Some(ImageFormat::Png | ImageFormat::WebP | ImageFormat::Tiff)
    ) {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(16 * 1024 * 1024);
    reader.limits(limits);
    reader.into_decoder().ok()?.exif_metadata().ok().flatten()
}

pub fn detected_creation_stamp(
    path: &Path,
    filename: Option<&str>,
    zone: &TimeZone,
) -> Option<solstone_core_import::Timestamp> {
    let extra_exif = read_extra_exif(path);
    solstone_core_import::detect_creation_with_exif(path, filename, zone, extra_exif.as_deref())
}

/// Detect a photo's creation moment in its own capture timezone from embedded EXIF only.
/// Filenames are never consulted.
pub fn detected_photo_capture_moment(
    path: &Path,
    zone: &TimeZone,
) -> Option<solstone_core_import::CaptureMoment> {
    let extra_exif = read_extra_exif(path);
    solstone_core_import::detect_photo_capture_moment(path, zone, extra_exif.as_deref())
}

struct PlacedImageMoment {
    stamp: solstone_core_import::Timestamp,
    utc_offset_seconds: Option<i32>,
    tz: Option<String>,
    time_source: PhotoTimeSource,
    time_provenance: PhotoTimeProvenance,
}

fn place_image_moment(
    path: &Path,
    zone: &TimeZone,
    owner_timestamp: Option<&str>,
    fallback_timestamp: Option<&str>,
    extra_exif: Option<&[u8]>,
    client_captured_at: Option<&str>,
) -> Result<PlacedImageMoment, ImageImportError> {
    if let Some(owner) = owner_timestamp
        && let Ok(ts) = solstone_core_import::validate_timestamp(owner)
    {
        return Ok(PlacedImageMoment {
            stamp: ts,
            utc_offset_seconds: None,
            tz: None,
            time_source: PhotoTimeSource::Owner,
            time_provenance: PhotoTimeProvenance::Owner,
        });
    }
    if let Some(moment) = solstone_core_import::detect_photo_capture_moment(path, zone, extra_exif)
    {
        return Ok(PlacedImageMoment {
            stamp: moment.stamp,
            utc_offset_seconds: moment.utc_offset_seconds,
            tz: moment.tz,
            time_source: PhotoTimeSource::Capture,
            time_provenance: PhotoTimeProvenance::Exif,
        });
    }
    if let Some(raw) = client_captured_at
        && let Some(moment) = solstone_core_import::parse_client_captured_at(raw, zone)
    {
        return Ok(PlacedImageMoment {
            stamp: moment.stamp,
            utc_offset_seconds: moment.utc_offset_seconds,
            tz: moment.tz,
            time_source: PhotoTimeSource::Capture,
            time_provenance: PhotoTimeProvenance::ClientCapturedAt,
        });
    }
    if let Some(fallback) = fallback_timestamp
        && let Ok(ts) = solstone_core_import::validate_timestamp(fallback)
    {
        return Ok(PlacedImageMoment {
            stamp: ts,
            utc_offset_seconds: None,
            tz: None,
            time_source: PhotoTimeSource::UploadFallback,
            time_provenance: PhotoTimeProvenance::UploadFallback,
        });
    }
    Err(ImageImportError::CouldNotDetectTimestamp {
        path: path.to_path_buf(),
    })
}

fn format_capture_instant(
    naive: DateTime,
    utc_offset_seconds: Option<i32>,
    time_source: PhotoTimeSource,
) -> Option<String> {
    if time_source == PhotoTimeSource::UploadFallback {
        return None;
    }
    let civil = naive.strftime("%Y-%m-%dT%H:%M:%S").to_string();
    match utc_offset_seconds {
        Some(secs) => {
            let sign = if secs < 0 { '-' } else { '+' };
            let abs = secs.unsigned_abs();
            Some(format!(
                "{civil}{sign}{:02}:{:02}",
                abs / 3600,
                (abs % 3600) / 60
            ))
        }
        None => Some(civil),
    }
}

fn zoned_from_placed(
    naive: DateTime,
    utc_offset_seconds: Option<i32>,
    zone: &TimeZone,
) -> Option<ZonedDateTime> {
    if let Some(seconds) = utc_offset_seconds
        && let Ok(offset) = Offset::from_seconds(seconds)
        && let Ok(instant) = offset.to_timestamp(naive)
    {
        return Some(instant.to_zoned_datetime(TimeZone::fixed(offset)));
    }
    solstone_core_time::from_local(zone, naive).earliest()
}

/// Preview one image source without writing any journal state, dated on the
/// photo's capture day when known or the owner's day in `zone` otherwise.
pub fn preview(
    path: &Path,
    zone: &TimeZone,
    owner_timestamp: Option<&str>,
    fallback_timestamp: Option<&str>,
) -> ImportPreview {
    let Ok((image, format, _, _, exif, _)) = read_image(path, false) else {
        return degenerate_preview();
    };
    let Some((format, _)) = format_details(format) else {
        return degenerate_preview();
    };
    let Ok(placed) = place_image_moment(
        path,
        zone,
        owner_timestamp,
        fallback_timestamp,
        exif.as_deref(),
        None,
    ) else {
        return degenerate_preview();
    };
    let Ok(naive) = DateTime::strptime("%Y%m%d_%H%M%S", placed.stamp.as_str()) else {
        return degenerate_preview();
    };
    let Some(timestamp) = zoned_from_placed(naive, placed.utc_offset_seconds, zone) else {
        return degenerate_preview();
    };
    ImportPreview {
        date_range: (
            timestamp.strftime("%Y%m%d").to_string(),
            timestamp.strftime("%Y%m%d").to_string(),
        ),
        item_count: 1,
        entity_count: 0,
        summary: solstone_core_cli_copy::display(
            "journal.cli.importer.preview.image.one",
            [
                ("format", &format),
                ("width", &image.width()),
                ("height", &image.height()),
            ],
        ),
    }
}

/// Plan the real image destination without generation, locks, or journal writes.
pub fn preview_size(
    path: &Path,
    journal: &Path,
    zone: &TimeZone,
    owner_timestamp: Option<&str>,
    fallback_timestamp: Option<&str>,
) -> Result<solstone_core_import::size::ImportSize, ImageImportError> {
    let (_, format, _, _, exif, _) = read_image(path, false)?;
    if format_details(format).is_none() {
        return Err(ImageImportError::UndecodableSource {
            path: path.to_path_buf(),
            detail: format!("unsupported image format {format:?}"),
        });
    }
    let client_captured_at = path
        .parent()
        .filter(|parent| parent.parent() == Some(journal.join("imports").as_path()))
        .and_then(|dir| dir.file_name().and_then(|id| id.to_str()))
        .and_then(|id| {
            solstone_core_import::read_provenance(journal, id)
                .ok()
                .flatten()
        })
        .and_then(|meta| {
            meta.get("captured_at")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        });
    let placed = place_image_moment(
        path,
        zone,
        owner_timestamp,
        fallback_timestamp,
        exif.as_deref(),
        client_captured_at.as_deref(),
    )?;
    let final_stamp =
        solstone_core_import::source_timestamp_for_preview(journal, &placed.stamp, path)
            .unwrap_or(placed.stamp);
    let naive = DateTime::strptime("%Y%m%d_%H%M%S", final_stamp.as_str()).map_err(|error| {
        ImageImportError::UndecodableSource {
            path: path.to_path_buf(),
            detail: error.to_string(),
        }
    })?;
    let Some(timestamp) = zoned_from_placed(naive, placed.utc_offset_seconds, zone) else {
        return Err(ImageImportError::UndecodableSource {
            path: path.to_path_buf(),
            detail: "invalid local timestamp in timezone".to_owned(),
        });
    };
    let extension = path
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or_default();
    let planned = allocate_image_segment(
        journal,
        IMPORT_STREAM,
        timestamp.datetime(),
        path,
        &extension,
        false,
        true,
        None,
    )?;
    Ok(solstone_core_import::size::ImportSize {
        recordings: [solstone_core_import::size::Recording {
            day: planned.day,
            stream: IMPORT_STREAM.to_owned(),
            segment: planned.segment,
        }]
        .into(),
        ..Default::default()
    })
}

/// In-memory prepared image import data before publication phase.
#[derive(Debug, Clone)]
pub struct PreparedImage {
    pub path: PathBuf,
    pub image: DynamicImage,
    pub format: ImageFormat,
    pub modified: SystemTime,
    pub description: DescriptionOutcome,
    pub vision: PhotoVisionOutcome,
    pub timestamp: ZonedDateTime,
    /// Original capture instant prior to any same-second segment collision bump.
    pub captured_at: Option<String>,
    pub day: String,
    pub segment: String,
    pub format_name: &'static str,
    pub mime_type: &'static str,
    pub extension: String,
    pub title: String,
    pub time_source: PhotoTimeSource,
    pub time_provenance: PhotoTimeProvenance,
    pub utc_offset_seconds: Option<i32>,
    pub tz: Option<String>,
    pub caption: Option<String>,
    pub original_filename: Option<String>,
    pub asset_id: Option<String>,
    pub capture_location: Option<solstone_core_import::CaptureLocation>,
    triage: Option<Box<ImageTriage>>,
}

#[derive(Debug, Clone)]
struct ImageTriage {
    verdict: Verdict,
    source_hash: SourceHash,
}

/// Decode and generate AI description for an image in memory without touching chronicle.
pub fn prepare_image(
    path: &Path,
    wire: &dyn WireClient,
    zone: &TimeZone,
    owner_timestamp: Option<&str>,
    fallback_timestamp: Option<&str>,
) -> Result<PreparedImage, ImageImportError> {
    prepare(
        path,
        wire,
        zone,
        owner_timestamp,
        fallback_timestamp,
        false,
        ImageImportOptions::default(),
    )
}

/// Prepare a standalone image and determine its kind from the decoded source.
pub fn prepare_manual_image(
    path: &Path,
    wire: &dyn WireClient,
    zone: &TimeZone,
    owner_timestamp: Option<&str>,
    fallback_timestamp: Option<&str>,
) -> Result<PreparedImage, ImageImportError> {
    prepare(
        path,
        wire,
        zone,
        owner_timestamp,
        fallback_timestamp,
        true,
        ImageImportOptions::default(),
    )
}

/// Prepare a standalone image with optional provenance from `import.json`
/// (owner caption, client capture timestamp, and original filename).
pub fn prepare_manual_image_with_options(
    path: &Path,
    wire: &dyn WireClient,
    zone: &TimeZone,
    owner_timestamp: Option<&str>,
    fallback_timestamp: Option<&str>,
    options: ImageImportOptions,
) -> Result<PreparedImage, ImageImportError> {
    prepare(
        path,
        wire,
        zone,
        owner_timestamp,
        fallback_timestamp,
        true,
        options,
    )
}

fn prepare(
    path: &Path,
    wire: &dyn WireClient,
    zone: &TimeZone,
    owner_timestamp: Option<&str>,
    fallback_timestamp: Option<&str>,
    manual: bool,
    options: ImageImportOptions,
) -> Result<PreparedImage, ImageImportError> {
    let (image, format, modified, orientation, exif, triage) = read_image(path, manual)?;
    let (format_name, mime_type) =
        format_details(format).ok_or_else(|| ImageImportError::UndecodableSource {
            path: path.to_path_buf(),
            detail: format!("unsupported image format {format:?}"),
        })?;
    let placed = place_image_moment(
        path,
        zone,
        owner_timestamp,
        fallback_timestamp,
        exif.as_deref(),
        options.client_captured_at.as_deref(),
    )?;
    let naive = DateTime::strptime("%Y%m%d_%H%M%S", placed.stamp.as_str()).map_err(|error| {
        ImageImportError::UndecodableSource {
            path: path.to_path_buf(),
            detail: error.to_string(),
        }
    })?;
    let Some(timestamp) = zoned_from_placed(naive, placed.utc_offset_seconds, zone) else {
        return Err(ImageImportError::UndecodableSource {
            path: path.to_path_buf(),
            detail: "invalid local timestamp in timezone".to_owned(),
        });
    };
    let day = timestamp.strftime("%Y%m%d").to_string();
    let segment = format!("{}_0", timestamp.strftime("%H%M%S"));
    let extension = path
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or_default();
    let title = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let caption = options
        .caption
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let original_filename = options
        .original_filename
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            path.file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .filter(|value| !value.is_empty())
        });

    let time_provenance = options.time_provenance.unwrap_or(placed.time_provenance);
    let captured_at = format_capture_instant(naive, placed.utc_offset_seconds, placed.time_source);
    let tz = placed.tz.or(options.tz);
    let asset_id = options
        .asset_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());

    let vision = match vision_jpeg(path, format, &image, orientation) {
        Ok(_) if !solstone_core_import::work_continues() => PhotoVisionOutcome::TransientFailure {
            reason: "generate cancelled".to_owned(),
        },
        Ok(jpeg) => {
            let described = interpret_generate_vision(wire.execute(&build_generate_request(&jpeg)));
            if solstone_core_import::work_continues() {
                described
            } else {
                PhotoVisionOutcome::TransientFailure {
                    reason: "generate cancelled".to_owned(),
                }
            }
        }
        Err(error) => PhotoVisionOutcome::AnalysisUnavailable {
            reason: format!("cannot prepare image for vision: {error}"),
        },
    };
    let description = vision.to_description_outcome();

    let capture_location = (placed.time_source == PhotoTimeSource::Capture
        && !triage
            .as_ref()
            .is_some_and(|t| t.verdict.kind == ContentKind::Screen))
    .then(|| {
        solstone_core_import::detect_photo_capture_location(path, exif.as_deref())
            .or(options.capture_location)
    })
    .flatten();

    Ok(PreparedImage {
        path: path.to_path_buf(),
        image,
        format,
        modified,
        description,
        vision,
        timestamp,
        captured_at,
        day,
        segment,
        format_name,
        mime_type,
        extension,
        title,
        time_source: placed.time_source,
        time_provenance,
        utc_offset_seconds: placed.utc_offset_seconds,
        tz,
        caption,
        original_filename,
        asset_id,
        capture_location,
        triage,
    })
}

struct ClaimedImageSegment {
    day: String,
    segment: String,
    directory: PathBuf,
    binding: solstone_core_journal_io::JournalRoot,
    existing_original: Option<PathBuf>,
}

fn claim_image_segment(
    journal_root: &Path,
    stream: &str,
    start: DateTime,
    source: &Path,
    extension: &str,
    snapshot: Option<&ImageTriage>,
) -> Result<ClaimedImageSegment, ImageImportError> {
    let planned = allocate_image_segment(
        journal_root,
        stream,
        start,
        source,
        extension,
        true,
        snapshot.is_some(),
        snapshot.map(|source| &source.source_hash),
    )?;
    Ok(ClaimedImageSegment {
        day: planned.day,
        segment: planned.segment,
        directory: planned.directory,
        binding: planned
            .binding
            .expect("publication acquired a segment binding"),
        existing_original: planned.existing_original,
    })
}

struct ImageAllocation {
    day: String,
    segment: String,
    directory: PathBuf,
    binding: Option<solstone_core_journal_io::JournalRoot>,
    existing_original: Option<PathBuf>,
}

#[allow(clippy::too_many_arguments)]
fn allocate_image_segment(
    journal_root: &Path,
    stream: &str,
    start: DateTime,
    source: &Path,
    extension: &str,
    publish: bool,
    manual: bool,
    snapshot_hash: Option<&SourceHash>,
) -> Result<ImageAllocation, ImageImportError> {
    let source_meta = fs::symlink_metadata(source).map_err(|error| ImageImportError::Install {
        path: source.to_path_buf(),
        detail: error.to_string(),
    })?;
    if !source_meta.is_file() {
        return Err(ImageImportError::Install {
            path: source.to_path_buf(),
            detail: "source is not a regular file".to_owned(),
        });
    }
    let source_len = source_meta.len();

    let day = start.strftime("%Y%m%d").to_string();
    let parent =
        contained_path(journal_root, &format!("chronicle/{day}/{stream}")).map_err(|error| {
            ImageImportError::JournalIo {
                path: journal_root.to_path_buf(),
                detail: error.to_string(),
            }
        })?;
    if publish {
        fs::create_dir_all(&parent).map_err(|error| ImageImportError::JournalIo {
            path: parent.clone(),
            detail: error.to_string(),
        })?;
    }

    let expected_original_name = format!("original{extension}");
    let screenshot_original_name = format!("original.screenshot{extension}");
    let mut cached_source_hash = snapshot_hash.cloned();

    for attempt in 0..IMAGE_SEGMENT_PROBE_LIMIT {
        let candidate_time = start
            .checked_add(SignedDuration::from_secs(i64::from(attempt)))
            .expect("a probed segment time is a representable time");
        if candidate_time.strftime("%Y%m%d").to_string() != day {
            return Err(ImageImportError::ImageSegmentDayOverflow {
                day,
                stream: stream.to_owned(),
                start: start.strftime("%Y-%m-%dT%H:%M:%S").to_string(),
            });
        }
        let segment = format!(
            "{:02}{:02}{:02}_0",
            candidate_time.hour(),
            candidate_time.minute(),
            candidate_time.second()
        );
        let removing_path = parent.join(format!("{REMOVING_SEGMENT_PREFIX}{segment}"));
        match path_lexists(&removing_path) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(error) => {
                return Err(ImageImportError::JournalIo {
                    path: removing_path,
                    detail: error.to_string(),
                });
            }
        }

        let candidate_path = parent.join(&segment);
        let candidate_io =
            |error: solstone_core_segment::SegmentError| ImageImportError::JournalIo {
                path: candidate_path.clone(),
                detail: error.to_string(),
            };
        // A key the owner removed is free again: what arrives is new data. Only
        // a removal still in progress steps on. A preview does not touch the
        // tombstone; a publish lets it give way before the write lock is taken.
        let removed = match solstone_core_segment::owner_deletion_state(&candidate_path)
            .map_err(candidate_io)?
        {
            solstone_core_segment::OwnerDeletionState::Live => false,
            solstone_core_segment::OwnerDeletionState::Deleted => true,
            solstone_core_segment::OwnerDeletionState::Staged
            | solstone_core_segment::OwnerDeletionState::Occupied => continue,
        };
        let reclaimed = removed && publish && {
            match solstone_core_segment::reclaim_for_arrival(&candidate_path)
                .map_err(candidate_io)?
            {
                solstone_core_segment::Arrival::Reclaimed => true,
                solstone_core_segment::Arrival::Free => false,
                solstone_core_segment::Arrival::Live | solstone_core_segment::Arrival::Removing => {
                    continue;
                }
            }
        };
        let _allocation = if publish {
            Some(
                solstone_core_segment::output_attempt::SegmentMutation::acquire(
                    &candidate_path,
                    solstone_core_journal_io::LockOptions::default(),
                )
                .map_err(|error| ImageImportError::JournalIo {
                    path: candidate_path.clone(),
                    detail: error.to_string(),
                })?,
            )
        } else {
            None
        };
        let create = if reclaimed {
            // Under the lock, the reclaimed folder is ours only while it is
            // still empty; anything written into it meanwhile steps us on.
            match fs::read_dir(&candidate_path).map(|mut entries| entries.next().is_none()) {
                Ok(true) => Ok(()),
                Ok(false) => continue,
                Err(error) => Err(error),
            }
        } else if removed && !publish {
            Ok(())
        } else if publish {
            fs::create_dir(&candidate_path)
        } else {
            match fs::symlink_metadata(&candidate_path) {
                Ok(_) => Err(io::Error::from(io::ErrorKind::AlreadyExists)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        };
        let mut existing_original = None;
        match create {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let candidate_meta = match fs::symlink_metadata(&candidate_path) {
                    Ok(meta) => meta,
                    Err(error) => {
                        return Err(ImageImportError::JournalIo {
                            path: candidate_path,
                            detail: error.to_string(),
                        });
                    }
                };
                if candidate_meta.file_type().is_symlink() || !candidate_meta.is_dir() {
                    continue;
                }
                let tombstone_path = candidate_path.join("tombstone.json");
                match path_lexists(&tombstone_path) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        return Err(ImageImportError::JournalIo {
                            path: tombstone_path,
                            detail: error.to_string(),
                        });
                    }
                }
                let entries = match fs::read_dir(&candidate_path) {
                    Ok(entries) => entries,
                    Err(error) => {
                        return Err(ImageImportError::JournalIo {
                            path: candidate_path,
                            detail: error.to_string(),
                        });
                    }
                };
                let mut original_entries = Vec::new();
                for entry in entries {
                    let entry = entry.map_err(|error| ImageImportError::JournalIo {
                        path: candidate_path.clone(),
                        detail: error.to_string(),
                    })?;
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("original.") && !name_str.ends_with(".lock") {
                        original_entries.push(entry);
                    }
                }
                if original_entries.len() != 1 {
                    continue;
                }
                let original_entry = &original_entries[0];
                let original_name = original_entry.file_name();
                if original_name != expected_original_name.as_str()
                    && !(manual && original_name == screenshot_original_name.as_str())
                {
                    continue;
                }
                let occupant_path = original_entry.path();
                let occupant_meta = match fs::symlink_metadata(&occupant_path) {
                    Ok(meta) => meta,
                    Err(_) => continue,
                };
                if occupant_meta.file_type().is_symlink() || !occupant_meta.is_file() {
                    continue;
                }
                if occupant_meta.len() != source_len {
                    continue;
                }
                let occupant_hash = match hash_source(&occupant_path) {
                    Ok(hash) => hash,
                    Err(_) => continue,
                };
                let source_hash = match &cached_source_hash {
                    Some(hash) => hash,
                    None => {
                        let hash =
                            hash_source(source).map_err(|error| ImageImportError::Install {
                                path: source.to_path_buf(),
                                detail: error.to_string(),
                            })?;
                        cached_source_hash.insert(hash)
                    }
                };
                if occupant_hash.as_str() != source_hash.as_str() {
                    continue;
                }
                existing_original = Some(occupant_path);
            }
            Err(error) => {
                return Err(ImageImportError::JournalIo {
                    path: candidate_path,
                    detail: error.to_string(),
                });
            }
        }

        if !publish {
            return Ok(ImageAllocation {
                day,
                segment,
                directory: candidate_path,
                binding: None,
                existing_original,
            });
        }
        let resolved =
            SegmentDir::resolve(journal_root, &day, &segment, stream).map_err(|error| {
                ImageImportError::JournalIo {
                    path: journal_root.to_path_buf(),
                    detail: error.to_string(),
                }
            })?;
        create_directory_with_mode(resolved.path(), PRIVATE_IMPORT_DIR_MODE).map_err(|error| {
            ImageImportError::JournalIo {
                path: resolved.path().to_path_buf(),
                detail: error.to_string(),
            }
        })?;
        return Ok(ImageAllocation {
            day,
            segment,
            directory: resolved.path().to_path_buf(),
            existing_original,
            binding: Some(
                solstone_core_journal_io::JournalRoot::open(resolved.path()).map_err(|error| {
                    ImageImportError::JournalIo {
                        path: resolved.path().to_path_buf(),
                        detail: error.to_string(),
                    }
                })?,
            ),
        });
    }

    Err(ImageImportError::ImageSegmentCollision {
        day,
        stream: stream.to_owned(),
        start: start.strftime("%Y-%m-%dT%H:%M:%S").to_string(),
        attempts: IMAGE_SEGMENT_PROBE_LIMIT,
    })
}

/// Install the original and transcript at `start`, without an image import record.
///
/// The note's time is `start`. The image import record is a separate import, and
/// this path does not write one.
pub fn install_image_at(
    prepared: &PreparedImage,
    journal_root: &Path,
    start: DateTime,
    publication: &dyn PublicationOperations,
    attachment: Option<(&str, &str)>,
) -> Result<ImageImportResult, ImageImportError> {
    install_photo_in_stream_with_faces(
        prepared,
        journal_root,
        IMPORT_STREAM,
        start,
        publication,
        attachment,
        None,
        &crate::photo_faces::SystemPhotoFaceDetector,
    )
}

/// Install one prepared photo into `stream` (such as `import.image` or a syncing
/// `photos` stream), writing its original, `image_meta.json`, `image_transcript.md`,
/// optional GPS sighting, optional desktop photo face pass, and canonical
/// `photo_observation.json`.
#[allow(clippy::too_many_arguments)]
pub fn install_photo_in_stream_with_faces(
    prepared: &PreparedImage,
    journal_root: &Path,
    stream: &str,
    start: DateTime,
    publication: &dyn PublicationOperations,
    attachment: Option<(&str, &str)>,
    import_id: Option<&str>,
    face_detector: &dyn crate::photo_faces::PhotoFaceDetector,
) -> Result<ImageImportResult, ImageImportError> {
    if !solstone_core_import::work_continues() {
        return Err(ImageImportError::Stopped);
    }
    let snapshot = prepared.triage.as_deref();
    let claimed = claim_image_segment(
        journal_root,
        stream,
        start,
        &prepared.path,
        &prepared.extension,
        snapshot,
    )?;
    let original_prefix =
        if snapshot.is_some_and(|snapshot| snapshot.verdict.kind == ContentKind::Screen) {
            "original.screenshot"
        } else {
            "original"
        };
    // Reusing a segment with the same original keeps its existing kind, even
    // if the source has since been renamed or the classifier has changed.
    let original_path = claimed.existing_original.clone().unwrap_or_else(|| {
        claimed
            .directory
            .join(format!("{original_prefix}{}", prepared.extension))
    });
    let screenshot = original_path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("original.screenshot."));
    let transcript_path = claimed.directory.join(if screenshot {
        SCREENSHOT_TRANSCRIPT
    } else {
        TRANSCRIPT_FILENAME
    });
    if attachment.is_none() {
        install_source(
            &prepared.path,
            &original_path,
            prepared.modified,
            &claimed.binding,
            journal_root,
            &claimed.day,
            publication,
            snapshot,
            claimed.existing_original.is_none(),
        )?;
    }
    let transcript_date = format!(
        "{}-{}-{}",
        &claimed.day[0..4],
        &claimed.day[4..6],
        &claimed.day[6..8]
    );
    let transcript = render_image_document(
        &prepared.title,
        prepared.format_name,
        prepared.image.width(),
        prepared.image.height(),
        &transcript_date,
        prepared.caption.as_deref(),
        &prepared.description,
        prepared
            .vision
            .reading()
            .and_then(|r| r.visible_text.as_deref()),
    );
    let claims = solstone_core_segment::output_attempt::OutputClaims::acquire(
        [transcript_path.with_extension("jsonl")],
        solstone_core_journal_io::LockOptions::default(),
    )
    .map_err(|error| ImageImportError::JournalIo {
        path: transcript_path.clone(),
        detail: error.to_string(),
    })?;
    let commit =
        claims
            .commit(&claimed.directory)
            .map_err(|error| ImageImportError::JournalIo {
                path: transcript_path.clone(),
                detail: error.to_string(),
            })?;
    claimed
        .binding
        .revalidate_canonical_binding()
        .map_err(|error| ImageImportError::JournalIo {
            path: transcript_path.clone(),
            detail: error.to_string(),
        })?;
    let source_selector_and_hash = if let Some((attach_import_id, filename)) = attachment {
        let hash = hash_source(&prepared.path).map_err(|error| ImageImportError::Manifest {
            detail: error.to_string(),
        })?;
        let reference = serde_json::json!({
            "import_id": attach_import_id,
            "source_name": filename,
            "source_sha256": hash.as_str(),
            "source_timestamp": prepared.timestamp.timestamp().as_second(),
        });
        let path = claimed.directory.join("image_source.json");
        let encoded = reference.to_string();
        atomic_replace(
            &path,
            encoded.as_bytes(),
            AtomicWriteOptions {
                mode: Some(PRIVATE_IMPORT_FILE_MODE),
            },
        )
        .map_err(|error| ImageImportError::JournalIo {
            path: path.clone(),
            detail: error.to_string(),
        })?;
        let selector_sha = format!("{:x}", Sha256::digest(encoded.as_bytes()));
        ("image_source.json".to_owned(), Some(selector_sha))
    } else {
        write_image_meta(&claimed.directory, prepared)?;
        let selector_name = original_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("{original_prefix}{}", prepared.extension));
        let selector_sha = snapshot
            .map(|t| t.source_hash.as_str().to_owned())
            .or_else(|| {
                hash_source(&original_path)
                    .ok()
                    .map(|h| h.as_str().to_owned())
            });
        (selector_name, selector_sha)
    };
    let mut extra_created = Vec::new();
    if !screenshot
        && attachment.is_none()
        && prepared.time_source == PhotoTimeSource::Capture
        && let Some(loc) = &prepared.capture_location
    {
        let location_path =
            write_photo_sighting_location(&claimed.directory, prepared.timestamp.timestamp(), loc)?;
        extra_created.push(location_path);
    }
    let _ = solstone_core_format::stored_markdown::remove_display_receipt(&transcript_path);
    write_text(
        &transcript_path,
        &transcript,
        AtomicWriteOptions {
            mode: Some(PRIVATE_IMPORT_FILE_MODE),
        },
    )
    .map_err(|error| ImageImportError::JournalIo {
        path: transcript_path.clone(),
        detail: error.to_string(),
    })?;
    let _ = transcript.save_display_receipt_private(&transcript_path);

    let observation = if !screenshot {
        let face_outcome = if attachment.is_none() {
            crate::photo_faces::apply_photo_face_pass(
                journal_root,
                &claimed.directory,
                &claimed.day,
                stream,
                &claimed.segment,
                &source_selector_and_hash.0,
                face_detector,
            )
            .unwrap_or_default()
        } else {
            crate::photo_faces::PhotoFacePassOutcome::default()
        };
        let obs = build_photo_observation(
            prepared,
            &claimed.day,
            stream,
            &claimed.segment,
            source_selector_and_hash.0,
            source_selector_and_hash.1,
            import_id,
            face_outcome.links,
            face_outcome.checked,
        );
        write_photo_observation(&claimed.directory, &obs).map_err(|error| {
            ImageImportError::JournalIo {
                path: claimed
                    .directory
                    .join(solstone_core_format::content::PHOTO_OBSERVATION_FILENAME),
                detail: error.to_string(),
            }
        })?;
        Some(obs)
    } else {
        None
    };

    drop(commit);
    let days_affected = vec![claimed.day.clone()];
    let mut files_created = vec![transcript_path.clone()];
    files_created.extend(extra_created);
    Ok(ImageImportResult {
        files_created,
        created_segment: CreatedSegment {
            day: claimed.day.clone(),
            segment: claimed.segment.clone(),
            stream: stream.to_owned(),
            hints: Default::default(),
        },
        days_affected,
        description: prepared.description.clone(),
        observation,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_photo_observation(
    prepared: &PreparedImage,
    day: &str,
    stream: &str,
    segment: &str,
    artifact_selector: String,
    source_sha256: Option<String>,
    import_id: Option<&str>,
    faces: Vec<solstone_core_segment::photo_observation::PhotoFaceLink>,
    faces_checked: bool,
) -> PhotoObservation {
    let clock = segment.split('_').next().unwrap_or("000000");
    let placed_at = if day.len() == 8 && clock.len() >= 6 {
        format!(
            "{}-{}-{}T{}:{}:{}",
            &day[0..4],
            &day[4..6],
            &day[6..8],
            &clock[0..2],
            &clock[2..4],
            &clock[4..6]
        )
    } else {
        prepared
            .timestamp
            .datetime()
            .strftime("%Y-%m-%dT%H:%M:%S")
            .to_string()
    };
    let precision = if prepared.time_source == PhotoTimeSource::UploadFallback {
        PhotoTimePrecision::Unknown
    } else {
        PhotoTimePrecision::Second
    };
    let reading = prepared.vision.reading();
    PhotoObservation {
        schema: PHOTO_OBSERVATION_SCHEMA,
        observation_revision: 1,
        source: PhotoObservationSource {
            day: day.to_owned(),
            stream: stream.to_owned(),
            segment: segment.to_owned(),
            artifact_selector,
            source_sha256,
            asset_id: prepared.asset_id.clone(),
            import_id: import_id.map(ToOwned::to_owned),
            original_filename: prepared.original_filename.clone(),
        },
        width: Some(prepared.image.width()),
        height: Some(prepared.image.height()),
        format: Some(prepared.format_name.to_owned()),
        time: PhotoObservationTime {
            captured_at: prepared.captured_at.clone(),
            placed_at,
            utc_offset_seconds: prepared.utc_offset_seconds,
            tz: prepared.tz.clone(),
            precision,
            provenance: prepared.time_provenance,
            time_source: prepared.time_source.as_str().to_owned(),
        },
        state: prepared.vision.state(),
        state_reason: prepared.vision.reason().map(ToOwned::to_owned),
        retry_count: 0,
        owner_caption: prepared.caption.as_deref().map(AttributedText::owner),
        description: reading.map(|r| AttributedText::model(r.description.clone())),
        visible_text: reading
            .and_then(|r| r.visible_text.as_deref())
            .map(AttributedText::model),
        belonging_class: reading.and_then(|r| r.belonging_class),
        faces,
        faces_checked,
        updated_at: solstone_core_time::utc_rfc3339(jiff::Timestamp::now()),
    }
}

fn write_image_meta(directory: &Path, prepared: &PreparedImage) -> Result<(), ImageImportError> {
    let mut meta = serde_json::Map::new();
    meta.insert("schema".to_owned(), serde_json::json!(1));
    meta.insert(
        "time_source".to_owned(),
        serde_json::json!(prepared.time_source.as_str()),
    );
    if let Some(offset) = prepared.utc_offset_seconds {
        meta.insert("utc_offset_seconds".to_owned(), serde_json::json!(offset));
    }
    if let Some(tz) = prepared.tz.as_deref() {
        meta.insert("tz".to_owned(), serde_json::json!(tz));
    }
    if let Some(caption) = prepared.caption.as_deref() {
        meta.insert("caption".to_owned(), serde_json::json!(caption));
    }
    if let Some(filename) = prepared.original_filename.as_deref() {
        meta.insert("original_filename".to_owned(), serde_json::json!(filename));
    }
    if prepared.time_source == PhotoTimeSource::Capture
        && let Some(loc) = &prepared.capture_location
    {
        let mut loc_obj = serde_json::Map::new();
        loc_obj.insert("lat".to_owned(), serde_json::json!(loc.latitude));
        loc_obj.insert("lon".to_owned(), serde_json::json!(loc.longitude));
        if let Some(acc) = loc.accuracy_m {
            loc_obj.insert("h_acc_m".to_owned(), serde_json::json!(acc));
        }
        if let Some(make) = loc.camera_make.as_deref() {
            loc_obj.insert("camera_make".to_owned(), serde_json::json!(make));
        }
        if let Some(model) = loc.camera_model.as_deref() {
            loc_obj.insert("camera_model".to_owned(), serde_json::json!(model));
        }
        meta.insert(
            "capture_location".to_owned(),
            serde_json::Value::Object(loc_obj),
        );
    }
    let path = directory.join(IMAGE_META_FILENAME);
    let encoded = serde_json::to_vec(&serde_json::Value::Object(meta)).map_err(|error| {
        ImageImportError::JournalIo {
            path: path.clone(),
            detail: error.to_string(),
        }
    })?;
    atomic_replace(
        &path,
        &encoded,
        AtomicWriteOptions {
            mode: Some(PRIVATE_IMPORT_FILE_MODE),
        },
    )
    .map_err(|error| ImageImportError::JournalIo {
        path,
        detail: error.to_string(),
    })
}

/// Nominal horizontal GPS margin (metres) used when camera EXIF carries valid
/// GPS coordinates without an explicit `GPSHPositioningError` tag (`0x001f`).
const DEFAULT_PHOTO_GPS_ACCURACY_M: f64 = 25.0;

fn write_photo_sighting_location(
    directory: &Path,
    captured_utc: jiff::Timestamp,
    loc: &solstone_core_import::CaptureLocation,
) -> Result<PathBuf, ImageImportError> {
    let path = directory.join("location.jsonl");
    let header = serde_json::json!({
        "schema": "solstone.location.segment/1",
        "kind": "location",
        "source": "photo_exif",
        "platform": "photo",
        "accuracy": "full",
        "fix_count": 0,
        "gap": false,
    });
    let h_acc = loc.accuracy_m.unwrap_or(DEFAULT_PHOTO_GPS_ACCURACY_M);
    let mut sighting = serde_json::Map::new();
    sighting.insert(
        "schema".to_owned(),
        serde_json::json!("solstone.location.sighting/1"),
    );
    sighting.insert(
        "t".to_owned(),
        serde_json::json!(solstone_core_time::utc_rfc3339(captured_utc)),
    );
    sighting.insert("lat".to_owned(), serde_json::json!(loc.latitude));
    sighting.insert("lon".to_owned(), serde_json::json!(loc.longitude));
    sighting.insert("h_acc".to_owned(), serde_json::json!(h_acc));
    let camera = match (loc.camera_make.as_deref(), loc.camera_model.as_deref()) {
        (Some(make), Some(model)) if !model.starts_with(make) => Some(format!("{make} {model}")),
        (_, Some(model)) => Some(model.to_owned()),
        (Some(make), None) => Some(make.to_owned()),
        (None, None) => None,
    };
    if let Some(cam) = camera {
        sighting.insert("camera".to_owned(), serde_json::json!(cam));
    }
    let content = format!("{}\n{}\n", header, serde_json::Value::Object(sighting));
    atomic_replace(
        &path,
        content.as_bytes(),
        AtomicWriteOptions {
            mode: Some(PRIVATE_IMPORT_FILE_MODE),
        },
    )
    .map_err(|error| ImageImportError::JournalIo {
        path: path.clone(),
        detail: error.to_string(),
    })?;
    Ok(path)
}

/// Install original, transcript, and manifest for a prepared image.
pub fn install_and_publish_image(
    prepared: &PreparedImage,
    journal_root: &Path,
    import_id: &str,
    publication: &dyn PublicationOperations,
    mut progress: Option<&mut dyn FnMut(&ProgressUpdate)>,
) -> Result<ImageImportResult, ImageImportError> {
    if !solstone_core_import::work_continues() {
        return Err(ImageImportError::Stopped);
    }
    let provenance = solstone_core_import::read_provenance(journal_root, import_id)
        .ok()
        .flatten();
    let effective_start = provenance
        .as_ref()
        .and_then(|m| solstone_core_import::stored_source_timestamp(m, import_id).ok())
        .and_then(|ts| DateTime::strptime("%Y%m%d_%H%M%S", ts.as_str()).ok())
        .unwrap_or_else(|| prepared.timestamp.datetime());
    let mut effective_prepared = prepared.clone();
    if let Some(meta) = provenance.as_ref() {
        if meta
            .get("owner_timestamp")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|v| !v.trim().is_empty())
        {
            effective_prepared.time_source = PhotoTimeSource::Owner;
            effective_prepared.time_provenance = PhotoTimeProvenance::Owner;
            effective_prepared.utc_offset_seconds = None;
            effective_prepared.tz = None;
            effective_prepared.captured_at =
                Some(effective_start.strftime("%Y-%m-%dT%H:%M:%S").to_string());
            effective_prepared.capture_location = None;
        }
        if effective_prepared.caption.is_none() {
            effective_prepared.caption = meta
                .get("caption")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
        }
        if effective_prepared.original_filename.is_none() {
            effective_prepared.original_filename = meta
                .get("original_filename")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
        }
    }
    let result = install_photo_in_stream_with_faces(
        &effective_prepared,
        journal_root,
        IMPORT_STREAM,
        effective_start,
        publication,
        None,
        Some(import_id),
        &crate::photo_faces::SystemPhotoFaceDetector,
    )?;
    write_import_manifest(
        &prepared.path,
        journal_root,
        import_id,
        &result.days_affected,
        &result.files_created,
        prepared.triage.as_ref().map(|data| &data.source_hash),
    )?;
    if let Some(callback) = progress.as_mut() {
        let day = result.days_affected.first().cloned().unwrap_or_default();
        callback(&ProgressUpdate {
            current: 1,
            total: 1,
            earliest_date: day.clone(),
            latest_date: day,
            entities_found: 0,
        });
    }
    Ok(result)
}

/// Install, describe, and record one image import segment.
pub fn import_image(
    path: &Path,
    journal_root: &Path,
    import_id: &str,
    progress: Option<&mut dyn FnMut(&ProgressUpdate)>,
    publication: &dyn PublicationOperations,
    wire: &dyn WireClient,
) -> Result<ImageImportResult, ImageImportError> {
    let zone = solstone_core_journal_config::owner_zone(journal_root);
    let prepared = prepare_manual_image(path, wire, &zone, None, None)?;
    install_and_publish_image(&prepared, journal_root, import_id, publication, progress)
}

type DecodedImage = (
    DynamicImage,
    ImageFormat,
    SystemTime,
    Orientation,
    Option<Vec<u8>>,
    Option<Box<ImageTriage>>,
);

fn read_image(path: &Path, manual: bool) -> Result<DecodedImage, ImageImportError> {
    let metadata = fs::metadata(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => ImageImportError::MissingSource {
            path: path.to_path_buf(),
        },
        _ => ImageImportError::UndecodableSource {
            path: path.to_path_buf(),
            detail: error.to_string(),
        },
    })?;
    if !metadata.is_file() {
        return Err(ImageImportError::SourceNotFile {
            path: path.to_path_buf(),
        });
    }
    let modified = metadata
        .modified()
        .map_err(|error| ImageImportError::UndecodableSource {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
    let bytes = fs::read(path).map_err(|error| ImageImportError::UndecodableSource {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    let format =
        image::guess_format(&bytes).map_err(|error| ImageImportError::UndecodableSource {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
    let undecodable = |error: image::ImageError| ImageImportError::UndecodableSource {
        path: path.to_path_buf(),
        detail: error.to_string(),
    };
    let mut decoder = ImageReader::with_format(Cursor::new(bytes.as_slice()), format)
        .into_decoder()
        .map_err(undecodable)?;
    let exif = decoder.exif_metadata().ok().flatten();
    // An unreadable orientation tag leaves the pixels as stored rather than failing the import.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let image = DynamicImage::from_decoder(decoder).map_err(undecodable)?;
    let triage = manual.then(|| {
        Box::new(ImageTriage {
            verdict: triage::classify(
                &bytes,
                &path.file_name().unwrap_or_default().to_string_lossy(),
                path.parent().unwrap_or(Path::new("")),
                None,
                None,
            ),
            source_hash: SourceHash::new(format!("{:x}", Sha256::digest(&bytes))),
        })
    });
    Ok((image, format, modified, orientation, exif, triage))
}

/// The copy of an imported image that goes to the vision model: upright pixels
/// capped at 1920 px on the longest side (never upscaling smaller copies such
/// as 512 px library photos), with zero source metadata containers (EXIF, GPS,
/// IPTC, XMP, ICC, comments, or trailing bytes).
///
/// When the source is already an upright RGB/grayscale JPEG within the 1920 px
/// cap, its compressed DCT scan passes through without a second lossy JPEG
/// generation while stripping every `APP*` and `COM` segment; any rotated,
/// oversized, non-JPEG, or non-standard JPEG falls back to fresh JPEG encoding.
fn vision_jpeg(
    source_path: &Path,
    format: ImageFormat,
    image: &DynamicImage,
    orientation: Orientation,
) -> Result<Vec<u8>, image::ImageError> {
    if format == ImageFormat::Jpeg
        && orientation == Orientation::NoTransforms
        && image.width().max(image.height()) <= VISION_MAX_LONG_EDGE
        && matches!(image.color(), image::ColorType::Rgb8 | image::ColorType::L8)
        && let Ok(raw) = fs::read(source_path)
        && let Some(stripped) = strip_jpeg_metadata_segments(&raw)
    {
        return Ok(stripped);
    }
    let mut upright = image.clone();
    upright.apply_orientation(orientation);
    if upright.width().max(upright.height()) > VISION_MAX_LONG_EDGE {
        upright = upright.resize(
            VISION_MAX_LONG_EDGE,
            VISION_MAX_LONG_EDGE,
            image::imageops::FilterType::Triangle,
        );
    }
    let rgb = upright.to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, VISION_JPEG_QUALITY)
        .encode_image(&rgb)?;
    Ok(jpeg)
}

/// Strip every `APP0..APP15` and `COM` marker (EXIF, GPS, XMP, IPTC, ICC,
/// comments, thumbnails) and any trailing bytes after `EOI` from a standard
/// baseline/progressive JPEG without re-encoding DCT blocks. Returns `None`
/// on `APP14` (Adobe color transform) or any unrecognized/malformed framing
/// so the caller cleanly falls back to fresh pixel re-encoding.
fn strip_jpeg_metadata_segments(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&[0xFF, 0xD8]);
    let mut i = 2usize;
    let mut saw_sos = false;
    while i < bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        while i < bytes.len() && bytes[i] == 0xFF {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        let marker = bytes[i];
        i += 1;
        match marker {
            // EOI terminates the JPEG cleanly; drop any trailing bytes.
            0xD9 => {
                if !saw_sos {
                    return None;
                }
                out.extend_from_slice(&[0xFF, 0xD9]);
                return Some(out);
            }
            // APP14 can declare non-standard Adobe color transforms (e.g., CMYK/YCCK).
            0xEE => return None,
            // Drop all APP0..APP15 metadata/ICC/EXIF/XMP/IPTC and COM comment segments.
            0xE0..=0xEF | 0xFE => {
                if i + 2 > bytes.len() {
                    return None;
                }
                let seg_len = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
                if seg_len < 2 || i + seg_len > bytes.len() {
                    return None;
                }
                i += seg_len;
            }
            // Keep essential frame/table/restart-interval segments verbatim.
            0xC0..=0xC3 | 0xC4 | 0xDB | 0xDD => {
                if i + 2 > bytes.len() {
                    return None;
                }
                let seg_len = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
                if seg_len < 2 || i + seg_len > bytes.len() {
                    return None;
                }
                out.push(0xFF);
                out.push(marker);
                out.extend_from_slice(&bytes[i..i + seg_len]);
                i += seg_len;
            }
            // Start of Scan: copy header + entropy-coded scan until the next marker.
            0xDA => {
                if i + 2 > bytes.len() {
                    return None;
                }
                let seg_len = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
                if seg_len < 2 || i + seg_len > bytes.len() {
                    return None;
                }
                saw_sos = true;
                out.push(0xFF);
                out.push(marker);
                out.extend_from_slice(&bytes[i..i + seg_len]);
                i += seg_len;
                let scan_start = i;
                while i < bytes.len() {
                    if bytes[i] == 0xFF {
                        if i + 1 >= bytes.len() {
                            return None;
                        }
                        let next = bytes[i + 1];
                        if next == 0x00 || (0xD0..=0xD7).contains(&next) {
                            i += 2;
                            continue;
                        }
                        if next == 0xFF {
                            i += 1;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                out.extend_from_slice(&bytes[scan_start..i]);
            }
            _ => return None,
        }
    }
    None
}

fn degenerate_preview() -> ImportPreview {
    ImportPreview {
        date_range: (String::new(), String::new()),
        item_count: 0,
        entity_count: 0,
        summary: solstone_core_cli_copy::text("journal.cli.importer.summary.image.none"),
    }
}

fn format_details(format: ImageFormat) -> Option<(&'static str, &'static str)> {
    match format {
        ImageFormat::Gif => Some(("GIF", "image/gif")),
        ImageFormat::Jpeg => Some(("JPEG", "image/jpeg")),
        ImageFormat::Png => Some(("PNG", "image/png")),
        ImageFormat::WebP => Some(("WEBP", "image/webp")),
        _ => None,
    }
}

fn build_generate_request(jpeg: &[u8]) -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: VISION_CONTEXT.to_owned(),
        contents: vec![
            ContentPart::Text {
                text: VISION_PROMPT.to_owned(),
            },
            ContentPart::Image {
                mime_type: "image/jpeg".to_owned(),
                data: base64::engine::general_purpose::STANDARD.encode(jpeg),
            },
        ],
        system_instruction: None,
        temperature: 0.3,
        max_output_tokens: 2_304,
        timeout_s: None,
        json_output: true,
        json_schema: Some(vision_json_schema()),
        attempt_index: 0,
        exclusive_admission: false,
        overflow_text: None,
        transport_retries: None,
    }
}

fn parse_vision_json(raw: &str) -> Option<PhotoVisionReading> {
    let trimmed = raw.trim();
    let candidate = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    let value: serde_json::Value = serde_json::from_str(candidate).ok()?;
    let obj = value.as_object()?;
    let description = obj
        .get("description")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_owned();
    let visible_text = obj
        .get("visible_text")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let belonging_class = obj
        .get("belonging_class")
        .and_then(serde_json::Value::as_str)
        .and_then(BelongingClass::parse);
    Some(PhotoVisionReading {
        description,
        visible_text,
        belonging_class,
    })
}

fn interpret_generate_vision(
    response: Result<GenerateResponse, ClientError>,
) -> PhotoVisionOutcome {
    match response {
        Ok(GenerateResponse::Generated(generated)) => {
            let trimmed = generated.text.trim();
            if trimmed.is_empty() {
                return PhotoVisionOutcome::AnalysisUnavailable {
                    reason: "Vision produced no description for image".to_owned(),
                };
            }
            if let Some(reading) = parse_vision_json(trimmed) {
                PhotoVisionOutcome::Ready(reading)
            } else {
                // Shape-tolerant fallback if a provider/stub emits plain text.
                PhotoVisionOutcome::Ready(PhotoVisionReading {
                    description: trimmed.to_owned(),
                    visible_text: None,
                    belonging_class: None,
                })
            }
        }
        Ok(GenerateResponse::Refused(refusal)) => {
            let reason = format!("{}: {}", refusal.reason.as_str(), refusal.detail);
            if refusal.reason == RefusalReason::NoEngineConfigured {
                PhotoVisionOutcome::ThinkingOff { reason }
            } else if refusal.retryable
                || matches!(
                    refusal.reason,
                    RefusalReason::AttestationStale
                        | RefusalReason::ConfidentialChannelClosed
                        | RefusalReason::IncompleteJson
                        | RefusalReason::IncompleteText
                )
            {
                PhotoVisionOutcome::TransientFailure { reason }
            } else {
                PhotoVisionOutcome::AnalysisUnavailable { reason }
            }
        }
        Ok(GenerateResponse::Cancelled) => PhotoVisionOutcome::TransientFailure {
            reason: "generate cancelled".to_owned(),
        },
        Err(
            error @ (ClientError::Protocol(_)
            | ClientError::Io { .. }
            | ClientError::ProcessIo(_)
            | ClientError::InvalidResponse(_)
            | ClientError::UnexpectedChild(_)),
        ) => PhotoVisionOutcome::TransientFailure {
            reason: error.to_string(),
        },
        Err(ClientError::Decode(detail) | ClientError::Resolve(detail)) => {
            PhotoVisionOutcome::AnalysisUnavailable { reason: detail }
        }
    }
}

#[cfg(test)]
fn interpret_generate(response: Result<GenerateResponse, ClientError>) -> DescriptionOutcome {
    interpret_generate_vision(response).to_description_outcome()
}

#[cfg(test)]
fn render_image_markdown(
    title: &str,
    format: &str,
    width: u32,
    height: u32,
    date: &str,
    caption: Option<&str>,
    description: &DescriptionOutcome,
) -> String {
    render_image_document(
        title,
        format,
        width,
        height,
        date,
        caption,
        description,
        None,
    )
    .into_text()
}

#[allow(clippy::too_many_arguments)]
fn render_image_document(
    title: &str,
    format: &str,
    width: u32,
    height: u32,
    date: &str,
    caption: Option<&str>,
    description: &DescriptionOutcome,
    visible_text: Option<&str>,
) -> solstone_core_format::stored_markdown::Document {
    use solstone_core_format::stored_markdown::{Document, imported::Copy};

    let mut doc = Document::plain(format!("# {title}\n\n"));
    doc.push_imported(Copy::ImageType);
    doc.push_str("\n");
    doc.push_imported(Copy::ImageFormat {
        format: format.to_owned(),
    });
    doc.push_str("\n");
    doc.push_imported(Copy::ImageDimensions { width, height });
    doc.push_str("\n");
    doc.push_imported(Copy::ImageDate {
        date: date.to_owned(),
    });
    doc.push_str("\n\n---\n\n");
    if let Some(caption) = caption.map(str::trim).filter(|value| !value.is_empty()) {
        doc.push_imported(Copy::ImageCaption);
        doc.push_str("\n");
        doc.push_str(caption);
        doc.push_str("\n\n---\n\n");
    }
    push_model_block(&mut doc, description, visible_text);
    let trimmed_len = doc.trim_end().len();
    doc.truncate(trimmed_len);
    doc.push_str("\n");
    doc
}

fn push_model_block(
    doc: &mut solstone_core_format::stored_markdown::Document,
    description: &DescriptionOutcome,
    visible_text: Option<&str>,
) {
    use solstone_core_format::stored_markdown::imported::Copy;

    doc.push_imported(Copy::ImageDescriptionHeader);
    match description {
        DescriptionOutcome::Generated(text) => {
            for line in text.split('\n') {
                if line.is_empty() {
                    doc.push_str(&format!("\n{MODEL_DERIVED_LINE_PREFIX}"));
                } else {
                    doc.push_str(&format!(
                        "\n{MODEL_DERIVED_LINE_PREFIX} {}",
                        solstone_core_format::markdown::literal_text(line)
                    ));
                }
            }
            if let Some(vtext) = visible_text.map(str::trim).filter(|s| !s.is_empty()) {
                doc.push_str(&format!("\n\n{VISIBLE_TEXT_MARKER}"));
                for line in vtext.split('\n') {
                    if line.is_empty() {
                        doc.push_str(&format!("\n{MODEL_DERIVED_LINE_PREFIX}"));
                    } else {
                        doc.push_str(&format!(
                            "\n{MODEL_DERIVED_LINE_PREFIX} {}",
                            solstone_core_format::markdown::literal_text(line)
                        ));
                    }
                }
            }
        }
        DescriptionOutcome::Unavailable { reason } => {
            let mut lines = reason.split('\n');
            let first = lines.next().unwrap_or("");
            doc.push_str("\n");
            doc.push_imported(Copy::ImageDescriptionUnavailable {
                reason: solstone_core_format::markdown::literal_text(first),
            });
            for line in lines {
                if line.is_empty() {
                    doc.push_str(&format!("\n{MODEL_DERIVED_LINE_PREFIX}"));
                } else {
                    doc.push_str(&format!(
                        "\n{MODEL_DERIVED_LINE_PREFIX} {}",
                        solstone_core_format::markdown::literal_text(line)
                    ));
                }
            }
        }
    }
}

/// Result of attempting to retry a pending or transiently failed photo observation.
#[derive(Debug, Clone)]
pub struct PhotoRetryOutcome {
    pub observation: PhotoObservation,
    pub retried: bool,
}

/// Retry vision description (and optional photo face pass) for a live photo
/// observation in a retryable state (`Pending`, `TransientFailure`, or
/// `ThinkingOff`) while the journal still holds the original pixels.
///
/// Bumps `observation_revision` and `retry_count`, atomically updates
/// `photo_observation.json` and `image_transcript.md` (with its `.display`
/// receipt), and touches the day's stream health marker so `sense_static` and
/// search re-index the updated observation.
pub fn retry_photo_observation(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    wire: &dyn WireClient,
    face_detector: &dyn crate::photo_faces::PhotoFaceDetector,
) -> Result<Option<PhotoRetryOutcome>, ImageImportError> {
    let Some(mut obs) = read_or_project_photo_observation(journal_root, day, stream, segment)
    else {
        return Ok(None);
    };
    if !obs.state.is_retryable() {
        return Ok(Some(PhotoRetryOutcome {
            observation: obs,
            retried: false,
        }));
    }
    let seg_dir = contained_path(journal_root, &format!("chronicle/{day}/{stream}/{segment}"))
        .map_err(|error| ImageImportError::JournalIo {
            path: journal_root.to_path_buf(),
            detail: error.to_string(),
        })?;
    let pixel_path = seg_dir.join(&obs.source.artifact_selector);
    if !pixel_path.is_file() || obs.source.artifact_selector == "image_source.json" {
        return Ok(Some(PhotoRetryOutcome {
            observation: obs,
            retried: false,
        }));
    }
    let (image, format, _, orientation, _, _) = read_image(&pixel_path, false)?;
    let (format_name, _) =
        format_details(format).ok_or_else(|| ImageImportError::UndecodableSource {
            path: pixel_path.clone(),
            detail: format!("unsupported image format {format:?}"),
        })?;
    let live_sha = hash_source(&pixel_path)
        .map_err(|error| ImageImportError::JournalIo {
            path: pixel_path.clone(),
            detail: error.to_string(),
        })?
        .as_str()
        .to_owned();
    let vision = match vision_jpeg(&pixel_path, format, &image, orientation) {
        Ok(jpeg) => interpret_generate_vision(wire.execute(&build_generate_request(&jpeg))),
        Err(error) => PhotoVisionOutcome::AnalysisUnavailable {
            reason: format!("cannot prepare image for vision: {error}"),
        },
    };
    let transcript_path = seg_dir.join(TRANSCRIPT_FILENAME);
    let claims = solstone_core_segment::output_attempt::OutputClaims::acquire(
        [transcript_path.with_extension("jsonl")],
        solstone_core_journal_io::LockOptions::default(),
    )
    .map_err(|error| ImageImportError::JournalIo {
        path: transcript_path.clone(),
        detail: error.to_string(),
    })?;
    let commit = claims
        .commit(&seg_dir)
        .map_err(|error| ImageImportError::JournalIo {
            path: transcript_path.clone(),
            detail: error.to_string(),
        })?;
    if !matches!(
        solstone_core_segment::owner_deletion_state(&seg_dir),
        Ok(solstone_core_segment::OwnerDeletionState::Live)
    ) {
        return Ok(None);
    }
    obs.observation_revision = obs.observation_revision.saturating_add(1);
    obs.retry_count = obs.retry_count.saturating_add(1);
    obs.source.source_sha256 = Some(live_sha);
    obs.width = Some(image.width());
    obs.height = Some(image.height());
    obs.format = Some(format_name.to_owned());
    obs.state = vision.state();
    obs.state_reason = vision.reason().map(ToOwned::to_owned);
    if let Some(reading) = vision.reading() {
        obs.description = Some(AttributedText::model(reading.description.clone()));
        obs.visible_text = reading.visible_text.as_deref().map(AttributedText::model);
        obs.belonging_class = reading.belonging_class;
    }
    if !obs.faces_checked
        && let Ok(face_outcome) = crate::photo_faces::apply_photo_face_pass(
            journal_root,
            &seg_dir,
            day,
            stream,
            segment,
            &obs.source.artifact_selector,
            face_detector,
        )
        && face_outcome.checked
    {
        obs.faces = face_outcome.links;
        obs.faces_checked = true;
    }
    obs.updated_at = solstone_core_time::utc_rfc3339(jiff::Timestamp::now());

    let title = obs
        .source
        .original_filename
        .as_deref()
        .and_then(|name| Path::new(name).file_stem().and_then(|s| s.to_str()))
        .unwrap_or("photo");
    let transcript_date = if day.len() == 8 {
        format!("{}-{}-{}", &day[0..4], &day[4..6], &day[6..8])
    } else {
        day.to_owned()
    };
    let desc_outcome = vision.to_description_outcome();
    let transcript = render_image_document(
        title,
        format_name,
        image.width(),
        image.height(),
        &transcript_date,
        obs.owner_caption.as_ref().map(|c| c.text.as_str()),
        &desc_outcome,
        obs.visible_text.as_ref().map(|v| v.text.as_str()),
    );
    let _ = solstone_core_format::stored_markdown::remove_display_receipt(&transcript_path);
    write_text(
        &transcript_path,
        &transcript,
        AtomicWriteOptions {
            mode: Some(PRIVATE_IMPORT_FILE_MODE),
        },
    )
    .map_err(|error| ImageImportError::JournalIo {
        path: transcript_path.clone(),
        detail: error.to_string(),
    })?;
    let _ = transcript.save_display_receipt_private(&transcript_path);
    write_photo_observation(&seg_dir, &obs).map_err(|error| ImageImportError::JournalIo {
        path: seg_dir.join(solstone_core_format::content::PHOTO_OBSERVATION_FILENAME),
        detail: error.to_string(),
    })?;
    drop(commit);
    let _ = solstone_core_journal_io::bump_stream_marker(journal_root, day);
    Ok(Some(PhotoRetryOutcome {
        observation: obs,
        retried: true,
    }))
}

/// Scan all live photo segments on `day` and retry any `Pending`,
/// `TransientFailure`, or `ThinkingOff` observations whose pixels remain.
pub fn retry_pending_photo_observations_for_day(
    journal_root: &Path,
    day: &str,
    wire: &dyn WireClient,
    face_detector: &dyn crate::photo_faces::PhotoFaceDetector,
) -> Vec<PhotoRetryOutcome> {
    let mut outcomes = Vec::new();
    let Ok(segments) = solstone_core_journal_io::iter_segments(
        journal_root,
        solstone_core_journal_io::PathOrDay::Day(day),
    ) else {
        return outcomes;
    };
    for seg in segments {
        let Some(stream) = seg
            .stream()
            .directory()
            .and_then(|s| s.to_str())
            .map(ToOwned::to_owned)
        else {
            continue;
        };
        let Some(seg_key) = seg.name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        if !seg
            .path()
            .join(solstone_core_format::content::PHOTO_OBSERVATION_FILENAME)
            .is_file()
        {
            continue;
        }
        if let Ok(Some(outcome)) =
            retry_photo_observation(journal_root, day, &stream, &seg_key, wire, face_detector)
            && outcome.retried
        {
            outcomes.push(outcome);
        }
    }
    outcomes
}

#[allow(clippy::too_many_arguments)]
fn install_source(
    source: &Path,
    destination: &Path,
    modified: SystemTime,
    binding: &solstone_core_journal_io::JournalRoot,
    journal_root: &Path,
    day: &str,
    publication: &dyn PublicationOperations,
    snapshot: Option<&ImageTriage>,
    write_kind: bool,
) -> Result<(), ImageImportError> {
    let parent = destination
        .parent()
        .expect("image destination has a parent");
    let claims = solstone_core_segment::output_attempt::OutputClaims::acquire(
        [destination.with_extension("jsonl")],
        solstone_core_journal_io::LockOptions::default(),
    )
    .map_err(|error| ImageImportError::Install {
        path: destination.to_path_buf(),
        detail: error.to_string(),
    })?;
    let mut source_file = File::open(source).map_err(|error| ImageImportError::Install {
        path: destination.to_path_buf(),
        detail: error.to_string(),
    })?;
    let mut temporary =
        NamedTempFile::new_in(parent).map_err(|error| ImageImportError::Install {
            path: destination.to_path_buf(),
            detail: error.to_string(),
        })?;
    io::copy(&mut source_file, temporary.as_file_mut()).map_err(|error| {
        ImageImportError::Install {
            path: destination.to_path_buf(),
            detail: error.to_string(),
        }
    })?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| ImageImportError::Install {
            path: destination.to_path_buf(),
            detail: error.to_string(),
        })?;
    if let Some(snapshot) = snapshot {
        let copied = hash_source(temporary.path()).map_err(|error| ImageImportError::Install {
            path: destination.to_path_buf(),
            detail: error.to_string(),
        })?;
        if copied != snapshot.source_hash {
            return Err(ImageImportError::Install {
                path: source.to_path_buf(),
                detail: "the file no longer matches the copy your journal prepared, so choose the file again".to_owned(),
            });
        }
    }
    let _commit = claims
        .commit(parent)
        .map_err(|error| ImageImportError::Install {
            path: destination.to_path_buf(),
            detail: error.to_string(),
        })?;
    binding
        .revalidate_canonical_binding()
        .map_err(|error| ImageImportError::Install {
            path: destination.to_path_buf(),
            detail: error.to_string(),
        })?;
    if write_kind && let Some(snapshot) = snapshot {
        let path = parent.join(triage::METADATA_FILENAME);
        triage::write_metadata(&path, &snapshot.verdict).map_err(|error| {
            ImageImportError::Install {
                path,
                detail: error.to_string(),
            }
        })?;
    }
    let temporary_path =
        temporary
            .into_temp_path()
            .keep()
            .map_err(|error| ImageImportError::Install {
                path: destination.to_path_buf(),
                detail: error.error.to_string(),
            })?;
    install_file(
        &temporary_path,
        destination,
        AtomicWriteOptions {
            mode: Some(PRIVATE_IMPORT_FILE_MODE),
        },
    )
    .map_err(|error| ImageImportError::Install {
        path: destination.to_path_buf(),
        detail: error.to_string(),
    })?;
    // The original is installed: mark the day dirty before anything that can still fail,
    // so an interrupted or failed import is repaired rather than silently skipped.
    publication
        .touch_stream_health_marker(journal_root, day)
        .map_err(|detail| ImageImportError::StreamMarker {
            day: day.to_owned(),
            detail,
        })?;
    crate::shared::open_to_set_times(destination)
        .and_then(|file| file.set_times(fs::FileTimes::new().set_modified(modified)))
        .map_err(|error| ImageImportError::Install {
            path: destination.to_path_buf(),
            detail: error.to_string(),
        })
}

fn write_import_manifest(
    source: &Path,
    journal_root: &Path,
    import_id: &str,
    days_affected: &[String],
    files_created: &[PathBuf],
    snapshot_hash: Option<&SourceHash>,
) -> Result<(), ImageImportError> {
    let source_hash = match snapshot_hash {
        Some(hash) => hash.clone(),
        None => hash_source(source).map_err(|error| ImageImportError::Manifest {
            detail: error.to_string(),
        })?,
    };
    let files_created = files_created
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    write_manifest(&ManifestWriteRequest {
        journal_root,
        import_id,
        source_type: "image",
        source_hash: &source_hash,
        entry_count: 1,
        days_affected,
        files_created: &files_created,
        imported_via: "native",
        link_id: None,
        observer_handle: None,
        raw_retention: None,
        span: None,
    })
    .map_err(|error| ImageImportError::Manifest {
        detail: error.to_string(),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::Cursor;

    use image::codecs::jpeg::JpegEncoder;
    use image::{GenericImageView, ImageBuffer, ImageFormat, Rgb};
    use serde_json::Value;
    use solstone_core_generate::{
        CapturedStream, ChildStatus, GeneratedResponse, ProtocolError, ProtocolFailure, ReasonCode,
        ReasonCodeValue, RefusalReason, RefusedResponse, UnexpectedChildFailure,
    };

    use super::*;

    const GRAMMAR: &str = include_str!("../../../fixtures/import_reference_grammar.json");
    const RESOLVER: &str = include_str!("../../../fixtures/import_resolver_corpus.json");
    const CAPTURE_REV: &str = "86fd678a6b3aec2eb4f33a4c934f0cf34a099542";

    struct RecordingWire {
        request: RefCell<Option<GenerateRequest>>,
    }

    impl WireClient for RecordingWire {
        fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            self.request.replace(Some(request.clone()));
            Ok(generated("description"))
        }
    }

    fn import_image(
        path: &Path,
        journal_root: &Path,
        import_id: &str,
        progress: Option<&mut dyn FnMut(&ProgressUpdate)>,
        publication: &dyn PublicationOperations,
        wire: &dyn WireClient,
    ) -> Result<ImageImportResult, ImageImportError> {
        let zone = solstone_core_journal_config::owner_zone(journal_root);
        let selected = jiff::Timestamp::try_from(fs::metadata(path).unwrap().modified().unwrap())
            .unwrap()
            .to_zoned_datetime(zone.clone())
            .strftime("%Y%m%d_%H%M%S")
            .to_string();
        let prepared = super::prepare_manual_image(path, wire, &zone, Some(&selected), None)?;
        super::install_and_publish_image(&prepared, journal_root, import_id, publication, progress)
    }

    fn generated(text: &str) -> GenerateResponse {
        GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text: text.to_owned(),
            model: "test".to_owned(),
            usage: serde_json::json!({}),
            finish_reason: "stop".to_owned(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        }))
    }

    fn refused(reason: RefusalReason) -> GenerateResponse {
        GenerateResponse::Refused(RefusedResponse {
            id: None,
            reason,
            reason_code: Some(ReasonCodeValue::Known(ReasonCode::new("unknown").unwrap())),
            retryable: false,
            blocking: true,
            reset_at_ms: None,
            provider: None,
            detail: "wire detail".to_owned(),
        })
    }

    fn png_bytes() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 4, Rgb([1, 2, 3])))
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn upload_temp_names_do_not_hide_png_creation_metadata() {
        let root = tempfile::tempdir().unwrap();
        for name in ["photo.png", "upload.tmp"] {
            let path = root.path().join(name);
            fs::write(
                &path,
                include_bytes!("../../solstone-core-import/tests/fixtures/creation-photo.png"),
            )
            .unwrap();
            let stamp = detected_creation_stamp(&path, Some("photo.png"), &TimeZone::UTC)
                .expect("PNG creation time");
            assert_eq!(stamp.as_str(), "20260624_003000");
        }
    }

    #[test]
    fn a_metadata_free_image_requires_a_date_or_a_named_fallback() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("photo.png");
        fs::write(&source, png_bytes()).unwrap();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        assert!(matches!(
            super::prepare_image(&source, &wire, &TimeZone::UTC, None, None),
            Err(ImageImportError::CouldNotDetectTimestamp { .. })
        ));
        let fallback = super::prepare_image(
            &source,
            &wire,
            &TimeZone::UTC,
            None,
            Some("20260623_183000"),
        )
        .unwrap();
        assert_eq!(fallback.day, "20260623");
        assert!(fallback.segment.starts_with("183000"));
    }

    #[test]
    fn request_carries_the_given_jpeg_and_generate_defaults_without_a_process() {
        let source = png_bytes();
        let request = build_generate_request(&source);
        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        wire.execute(&request).unwrap();
        let request = wire.request.borrow_mut().take().unwrap();
        assert_eq!(request.context, VISION_CONTEXT);
        assert_eq!(request.contents.len(), 2);
        assert!(matches!(
            &request.contents[0],
            ContentPart::Text { text } if text == VISION_PROMPT
        ));
        let ContentPart::Image { mime_type, data } = &request.contents[1] else {
            panic!("second content part must be an image");
        };
        assert_eq!(mime_type, "image/jpeg");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap(),
            source
        );
        assert_eq!(request.temperature, 0.3);
        assert_eq!(request.max_output_tokens, 2_304);
        assert!(request.json_output);
        assert_eq!(request.json_schema, Some(vision_json_schema()));
        assert_eq!(request.attempt_index, 0);
        assert!(!request.exclusive_admission);
    }

    /// A little-endian TIFF block for an APP1 Exif segment: IFD0 carries the orientation, a
    /// camera make and a pointer to a GPS IFD holding a latitude.
    fn exif_tiff(orientation: u16) -> Vec<u8> {
        let mut tiff = Vec::new();
        let entry = |tiff: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: [u8; 4]| {
            tiff.extend_from_slice(&tag.to_le_bytes());
            tiff.extend_from_slice(&kind.to_le_bytes());
            tiff.extend_from_slice(&count.to_le_bytes());
            tiff.extend_from_slice(&value);
        };
        // Layout: header 8, IFD0 at 8 (3 entries, 42 bytes), make at 50, GPS IFD at 58
        // (2 entries, 30 bytes), latitude rationals at 88.
        tiff.extend_from_slice(b"II*\0");
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        let [low, high] = orientation.to_le_bytes();
        entry(&mut tiff, 0x010f, 2, 8, 50u32.to_le_bytes());
        entry(&mut tiff, 0x0112, 3, 1, [low, high, 0, 0]);
        entry(&mut tiff, 0x8825, 4, 1, 58u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(tiff.len(), 50);
        tiff.extend_from_slice(b"TESTCAM\0");
        assert_eq!(tiff.len(), 58);
        tiff.extend_from_slice(&2u16.to_le_bytes());
        entry(&mut tiff, 0x0001, 2, 2, *b"N\0\0\0");
        entry(&mut tiff, 0x0002, 5, 3, 88u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(tiff.len(), 88);
        for degrees in [40u32, 26, 46] {
            tiff.extend_from_slice(&degrees.to_le_bytes());
            tiff.extend_from_slice(&1u32.to_le_bytes());
        }
        tiff
    }

    /// A JPEG with an APP1 Exif segment (orientation, camera make, GPS latitude) placed
    /// directly after the start-of-image marker, the way cameras and phones write it.
    fn jpeg_with_exif(width: u32, height: u32, orientation: u16) -> Vec<u8> {
        let pixels = ImageBuffer::from_fn(width, height, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        });
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, 80)
            .encode_image(&DynamicImage::ImageRgb8(pixels))
            .unwrap();
        assert_eq!(&encoded[..2], [0xff, 0xd8]);
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(&exif_tiff(orientation));
        let length = u16::try_from(payload.len() + 2).unwrap();
        let mut jpeg = encoded[..2].to_vec();
        jpeg.extend_from_slice(&[0xff, 0xe1]);
        jpeg.extend_from_slice(&length.to_be_bytes());
        jpeg.extend_from_slice(&payload);
        jpeg.extend_from_slice(&encoded[2..]);
        jpeg
    }

    fn jpeg_marker_types(jpeg: &[u8]) -> Vec<u8> {
        assert_eq!(&jpeg[..2], [0xff, 0xd8]);
        let mut markers = Vec::new();
        let mut offset = 2;
        while offset + 1 < jpeg.len() {
            assert_eq!(jpeg[offset], 0xff);
            while offset < jpeg.len() && jpeg[offset] == 0xff {
                offset += 1;
            }
            let marker = jpeg[offset];
            offset += 1;
            markers.push(marker);
            if marker == 0xd9 || marker == 0xda {
                break;
            }
            let length = u16::from_be_bytes([jpeg[offset], jpeg[offset + 1]]) as usize;
            assert!(length >= 2);
            offset += length;
        }
        markers
    }

    fn vision_request_bytes(source: &[u8]) -> Vec<u8> {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("IMG_0001.jpg");
        fs::write(&path, source).unwrap();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        let prepared = prepare_image(
            &path,
            &wire,
            &solstone_core_journal_config::host_zone(),
            Some("20260510_120000"),
            None,
        )
        .unwrap();
        assert_eq!(prepared.format, ImageFormat::Jpeg);
        assert_eq!(fs::read(&path).unwrap(), source);
        let request = wire.request.borrow_mut().take().unwrap();
        let serialized = format!("{request:?}");
        assert!(!serialized.contains("IMG_0001"));
        assert!(!serialized.contains(&directory.path().display().to_string()));
        let ContentPart::Image { mime_type, data } = &request.contents[1] else {
            panic!("second content part must be an image");
        };
        assert_eq!(mime_type, "image/jpeg");
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap()
    }

    #[test]
    fn imported_photo_reaches_the_model_as_pixels_without_its_exif() {
        for (width, height, orientation, expected) in [
            // Stored sideways with orientation 6: sent upright, same pixel count (never upscaled).
            (40, 24, 6, (24, 40)),
            // 512 px library copy: stays 512 px (never upscaled).
            (512, 384, 1, (512, 384)),
            // No rotation, larger than the 1920 px vision bound: sent resized.
            (3000, 16, 1, (1920, 10)),
        ] {
            let source = jpeg_with_exif(width, height, orientation);
            let mut decoder = ImageReader::with_format(Cursor::new(&source), ImageFormat::Jpeg)
                .into_decoder()
                .unwrap();
            let exif = decoder
                .exif_metadata()
                .unwrap()
                .expect("the source must carry a readable Exif block");
            assert!(
                exif.windows(2)
                    .any(|window| window == 0x8825u16.to_le_bytes()),
                "the source Exif block must point at a GPS IFD"
            );
            assert_eq!(
                decoder.orientation().unwrap(),
                Orientation::from_exif(orientation as u8).unwrap()
            );
            assert!(source.windows(6).any(|window| window == b"Exif\0\0"));
            assert!(source.windows(7).any(|window| window == b"TESTCAM"));

            let sent = vision_request_bytes(&source);
            // Structural proof: freshly encoded JPEG carries zero APP1 (EXIF),
            // APP2 (ICC), APP13 (IPTC) or COM comment metadata segments.
            let markers = jpeg_marker_types(&sent);
            assert!(!markers.contains(&0xe1), "no APP1/EXIF segment");
            assert!(!markers.contains(&0xe2), "no APP2/ICC segment");
            assert!(!markers.contains(&0xed), "no APP13/IPTC segment");
            assert!(!markers.contains(&0xfe), "no COM segment");
            assert!(!sent.windows(6).any(|window| window == b"Exif\0\0"));
            assert!(!sent.windows(7).any(|window| window == b"TESTCAM"));
            let decoded = image::load_from_memory_with_format(&sent, ImageFormat::Jpeg).unwrap();
            assert_eq!(decoded.dimensions(), expected);
        }
    }

    #[test]
    fn interpretation_covers_every_wire_door() {
        assert_eq!(
            interpret_generate(Ok(generated("  detail  "))),
            DescriptionOutcome::Generated("detail".to_owned())
        );
        assert!(matches!(
            interpret_generate(Ok(generated(" \n "))),
            DescriptionOutcome::Unavailable { .. }
        ));
        let DescriptionOutcome::Unavailable { reason } =
            interpret_generate(Ok(refused(RefusalReason::NoEngineConfigured)))
        else {
            panic!("no-engine refusal must be unavailable");
        };
        assert!(reason.contains(RefusalReason::NoEngineConfigured.as_str()));
        assert!(reason.contains("wire detail"));
        assert!(matches!(
            interpret_generate(Ok(refused(RefusalReason::AttestationStale))),
            DescriptionOutcome::Unavailable { .. }
        ));
        for error in [
            ClientError::Protocol(Box::new(ProtocolFailure {
                error: ProtocolError {
                    id: None,
                    reason: "protocol".to_owned(),
                    detail: "detail".to_owned(),
                },
                status: ChildStatus {
                    exit_code: Some(70),
                    signal: None,
                },
                stdout: CapturedStream::empty(),
                stderr: CapturedStream::empty(),
                stdin_closed_early: false,
            })),
            ClientError::Decode("decode".to_owned()),
            ClientError::Io {
                primary: "io".to_owned(),
                cleanup: None,
            },
            ClientError::Resolve("resolve".to_owned()),
            ClientError::UnexpectedChild(Box::new(UnexpectedChildFailure {
                status: ChildStatus {
                    exit_code: Some(64),
                    signal: None,
                },
                stdout: CapturedStream::empty(),
                stderr: CapturedStream::empty(),
                stdin_closed_early: false,
            })),
        ] {
            assert!(matches!(
                interpret_generate(Err(error)),
                DescriptionOutcome::Unavailable { .. }
            ));
        }
    }

    #[test]
    fn model_descriptions_remain_literal_even_without_their_own_fences() {
        use pulldown_cmark::{Event, Options, Parser, Tag};

        let text = "![cover](https://example.test/cover.png)\n<img src=x> &amp; 日本語\n```markdown\n[open](https://example.test/)\n```";
        for description in [
            DescriptionOutcome::Generated(text.to_owned()),
            DescriptionOutcome::Unavailable {
                reason: text.to_owned(),
            },
        ] {
            let rendered =
                render_image_markdown("photo", "PNG", 4, 5, "2026-01-02", None, &description);
            assert_model_partition(&rendered, &description);
            let (_, model) = rendered.split_once("\n---\n\n").unwrap();
            let mut visible = String::new();
            for event in Parser::new_ext(model, Options::all()) {
                match event {
                    Event::Start(Tag::Image { .. } | Tag::Link { .. })
                    | Event::Html(_)
                    | Event::InlineHtml(_) => panic!("active model markup in {model:?}"),
                    Event::Text(value) => visible.push_str(&value),
                    Event::SoftBreak | Event::HardBreak => visible.push('\n'),
                    _ => {}
                }
            }
            assert!(visible.contains(text), "{visible:?}");
        }
    }

    #[test]
    fn transcript_has_a_structural_model_partition() {
        let description =
            DescriptionOutcome::Generated("A description.\n\nSecond line.".to_owned());
        let rendered =
            render_image_markdown("photo", "PNG", 4, 5, "2026-01-02", None, &description);
        assert_model_partition(&rendered, &description);
        let unavailable = DescriptionOutcome::Unavailable {
            reason: "no engine".to_owned(),
        };
        let rendered =
            render_image_markdown("photo", "PNG", 4, 5, "2026-01-02", None, &unavailable);
        assert_model_partition(&rendered, &unavailable);
    }

    fn assert_model_partition(rendered: &str, description: &DescriptionOutcome) {
        let (header, model) = rendered
            .split_once("\n---\n\n")
            .expect("transcript must contain a deterministic header divider");
        assert!(
            header
                .lines()
                .all(|line| !line.starts_with(MODEL_DERIVED_LINE_PREFIX))
        );
        assert!(
            model
                .lines()
                .all(|line| line.starts_with(MODEL_DERIVED_LINE_PREFIX))
        );
        match description {
            DescriptionOutcome::Generated(text) => {
                assert!(
                    text.lines()
                        .all(|line| line.is_empty() || !header.contains(line))
                );
            }
            DescriptionOutcome::Unavailable { reason } => {
                assert!(!header.contains(reason));
                for line in reason.lines() {
                    assert!(model.contains(&solstone_core_format::markdown::literal_text(line)));
                }
            }
        }
    }

    #[test]
    fn extension_set_and_degenerate_preview_match_frozen_import_oracles() {
        let grammar: Value = serde_json::from_str(GRAMMAR).unwrap();
        assert_eq!(grammar["provenance"]["captured_from_rev"], CAPTURE_REV);
        let patterns = grammar["importers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == "image")
            .unwrap()["file_patterns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().trim_start_matches("*."))
            .collect::<Vec<_>>();
        assert_eq!(patterns.len(), IMAGE_EXTENSIONS.len());
        assert!(patterns.iter().all(|pattern| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|extension| pattern.eq_ignore_ascii_case(extension))
        }));

        let resolver: Value = serde_json::from_str(RESOLVER).unwrap();
        let expected = resolver["passes"]["native_detector_answers_no"]["bare::pic.png"]["stdout"]
            .as_str()
            .unwrap();
        assert!(expected.contains("No readable image found"));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pic.png");
        fs::write(&path, b"not an image").unwrap();
        let preview = preview(
            &path,
            &solstone_core_journal_config::host_zone(),
            None,
            None,
        );
        assert_eq!(preview.date_range, (String::new(), String::new()));
        assert_eq!(preview.item_count, 0);
        assert_eq!(preview.entity_count, 0);
        assert_eq!(preview.summary, "No readable image found");
    }

    use solstone_core_segment::StreamHints;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    struct TestPublication {
        touched_days: RefCell<Vec<String>>,
    }

    impl TestPublication {
        fn new() -> Self {
            Self {
                touched_days: RefCell::new(Vec::new()),
            }
        }
    }

    #[test]
    fn raw_install_admits_the_claimed_directory_and_refuses_deletion_or_path_reuse() {
        for mutation in ["unchanged", "deleted", "replaced"] {
            let root = tempfile::tempdir().unwrap();
            let source = root.path().join("source.png");
            fs::write(&source, b"synthetic image bytes").unwrap();
            let claimed = claim_image_segment(
                root.path(),
                IMPORT_STREAM,
                jiff::civil::date(2026, 1, 2).at(12, 0, 0, 0),
                &source,
                ".png",
                None,
            )
            .unwrap();
            match mutation {
                "deleted" => fs::write(claimed.directory.join("tombstone.json"), b"{}").unwrap(),
                "replaced" => {
                    fs::rename(&claimed.directory, root.path().join("previous")).unwrap();
                    fs::create_dir(&claimed.directory).unwrap();
                }
                _ => (),
            }
            let destination = claimed.directory.join("original.png");
            let result = install_source(
                &source,
                &destination,
                SystemTime::now(),
                &claimed.binding,
                root.path(),
                &claimed.day,
                &TestPublication::new(),
                None,
                false,
            );
            if mutation == "unchanged" {
                result.unwrap();
                assert_eq!(fs::read(destination).unwrap(), b"synthetic image bytes");
            } else {
                assert!(result.is_err(), "{mutation}");
                assert!(!destination.exists(), "{mutation}");
            }
        }
    }

    impl PublicationOperations for TestPublication {
        fn advance_stream(
            &self,
            _journal: &Path,
            _segment: &CreatedSegment,
        ) -> Result<
            solstone_core_segment::StreamAdvance,
            solstone_core_segment::UnboundStreamAdvanceError,
        > {
            panic!(
                "test PublicationOperations::advance_stream must not be called; chain tests call advance_unbound_stream directly"
            );
        }

        fn enqueue_path(
            &self,
            _journal: &Path,
            _path: &Path,
        ) -> Result<solstone_core_import::publish::QueueFileOutcome, String> {
            Ok(solstone_core_import::publish::QueueFileOutcome::Declined)
        }

        fn touch_stream_health_marker(&self, _journal: &Path, day: &str) -> Result<(), String> {
            self.touched_days.borrow_mut().push(day.to_owned());
            Ok(())
        }

        fn emit_observed(
            &self,
            _journal: &Path,
            _revision: Option<&str>,
            _day: &str,
            _segment: &str,
            _stream: &str,
        ) {
        }

        fn emit_enrichment_ready(
            &self,
            _journal: &Path,
            _revision: Option<&str>,
            _import_id: &str,
            _importer: &str,
            _days: &[String],
            _entries_written: u64,
        ) {
        }

        fn emit_drain(&self, _journal: &Path, _revision: Option<&str>, _day: &str) {}
    }

    /// A wall-clock time on this computer's own zone.
    fn local_time(wall: jiff::civil::DateTime) -> ZonedDateTime {
        solstone_core_time::from_local(&jiff::tz::TimeZone::system(), wall)
            .single()
            .unwrap()
    }

    fn write_test_png(path: &Path, pixel: [u8; 3], time: &ZonedDateTime) {
        let img = DynamicImage::ImageRgb8(ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 4, Rgb(pixel)));
        img.save_with_format(path, ImageFormat::Png).unwrap();
        let file_time = fs::FileTimes::new()
            .set_modified(std::time::SystemTime::try_from(time.timestamp()).unwrap());
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(file_time)
            .unwrap();
    }

    fn write_test_jpeg(path: &Path, pixel: [u8; 3], time: &ZonedDateTime) {
        let img = DynamicImage::ImageRgb8(ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 4, Rgb(pixel)));
        img.save_with_format(path, ImageFormat::Jpeg).unwrap();
        let file_time = fs::FileTimes::new()
            .set_modified(std::time::SystemTime::try_from(time.timestamp()).unwrap());
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(file_time)
            .unwrap();
    }

    #[test]
    fn a_changed_image_during_vision_does_not_store_mismatched_bytes_or_kind() {
        struct ChangingWire<'a>(&'a Path);
        impl WireClient for ChangingWire<'_> {
            fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
                fs::write(self.0, b"source changed while the model was running").unwrap();
                Ok(generated("red dialog"))
            }
        }
        let journal = tempfile::tempdir().unwrap();
        let sources = tempfile::tempdir().unwrap();
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));
        let path = sources.path().join("Screenshot_20260615-120000.png");
        write_test_png(&path, [20, 30, 40], &time);
        let publication = TestPublication::new();
        let error = import_image(
            &path,
            journal.path(),
            "changed",
            None,
            &publication,
            &ChangingWire(&path),
        )
        .unwrap_err();
        assert!(matches!(error, ImageImportError::Install { .. }));
        let segment = journal
            .path()
            .join("chronicle/20260615/import.image/120000_0");
        assert!(!segment.join("original.screenshot.png").exists());
        assert!(!segment.join(SCREENSHOT_TRANSCRIPT).exists());
        assert!(!segment.join(triage::METADATA_FILENAME).exists());
        assert!(
            !journal
                .path()
                .join("imports/changed/manifest.json")
                .exists()
        );
        assert!(publication.touched_days.borrow().is_empty());
    }

    #[test]
    fn screenshot_reimport_under_another_name_keeps_one_screen_description() {
        use solstone_core_format::content::{
            AdmittedCategory, OfferedTo, file_spec,
            kinds::{indexed_permission, segment_file_permission},
        };
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));
        let source = source_dir.path().join("Screenshot_20260615-120000.png");
        write_test_png(&source, [10, 80, 90], &time);
        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        let publication = TestPublication::new();
        let first =
            import_image(&source, journal.path(), "first", None, &publication, &wire).unwrap();
        let description = &first.files_created[0];
        let original = description.with_file_name("original.screenshot.png");
        let metadata = description.with_file_name(triage::METADATA_FILENAME);
        assert_eq!(fs::read(&original).unwrap(), fs::read(&source).unwrap());
        let decision = fs::read(&metadata).unwrap();
        let decision_json: Value = serde_json::from_slice(&decision).unwrap();
        assert_eq!(decision_json["kind"], "screen_text");
        assert_eq!(
            decision_json["reasons"],
            serde_json::json!(["aosp_screenshot_name"])
        );
        assert_eq!(decision_json["ruleset_version"], 8);
        assert!(description.ends_with("screenshot_transcript.md"));
        let description_rel = format!(
            "{}/import.image/{}/screenshot_transcript.md",
            first.created_segment.day, first.created_segment.segment
        );
        assert_eq!(
            indexed_permission(
                Some(journal.path()),
                &description_rel,
                AdmittedCategory::Transcripts
            ),
            AdmittedCategory::ScreenText
        );
        assert_eq!(
            segment_file_permission(&description_rel),
            AdmittedCategory::ScreenText
        );
        let original_rel =
            description_rel.replace("screenshot_transcript.md", "original.screenshot.png");
        assert_eq!(
            file_spec(&original_rel).unwrap().offered_to,
            OfferedTo::NONE
        );
        let metadata_rel = description_rel.replace("screenshot_transcript.md", "import_kind.json");
        assert_eq!(
            file_spec(&metadata_rel).unwrap().offered_to,
            OfferedTo::NONE
        );

        let renamed = source_dir.path().join("unmarked.png");
        fs::rename(&source, &renamed).unwrap();
        let second = import_image(
            &renamed,
            journal.path(),
            "second",
            None,
            &publication,
            &wire,
        )
        .unwrap();
        assert_eq!(second.files_created, first.files_created);
        assert_eq!(fs::read(&metadata).unwrap(), decision);
        assert!(!description.with_file_name("image_transcript.md").exists());
        assert!(!description.with_file_name("original.png").exists());
    }

    #[test]
    fn an_imported_image_keeps_its_bytes_and_source_time() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));
        let source = source_dir.path().join("photo.png");
        write_test_png(&source, [10, 20, 30], &time);

        let publication = TestPublication::new();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        let result = import_image(
            &source,
            journal.path(),
            "import-1",
            None,
            &publication,
            &wire,
        )
        .unwrap();

        let segment = journal
            .path()
            .join("chronicle")
            .join(&result.created_segment.day)
            .join("import.image")
            .join(&result.created_segment.segment);
        let original = segment.join("original.png");
        assert_eq!(fs::read(&original).unwrap(), fs::read(&source).unwrap());
        assert_eq!(
            fs::metadata(&original).unwrap().modified().unwrap(),
            SystemTime::try_from(time.timestamp()).unwrap()
        );
        assert!(segment.join("image_transcript.md").is_file());
        assert!(
            journal
                .path()
                .join("imports/import-1/manifest.json")
                .is_file()
        );
    }

    #[test]
    fn two_different_images_with_same_mtime_second_take_consecutive_keys_and_preserve_content() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));

        let img_a = source_dir.path().join("a.png");
        let img_b = source_dir.path().join("b.png");
        write_test_png(&img_a, [10, 20, 30], &time);
        write_test_png(&img_b, [40, 50, 60], &time);

        let pub_a = TestPublication::new();
        let wire_a = RecordingWire {
            request: RefCell::new(None),
        };
        let res_a =
            import_image(&img_a, journal.path(), "import-1", None, &pub_a, &wire_a).unwrap();
        assert_eq!(res_a.created_segment.segment, "120000_0");
        assert_eq!(res_a.created_segment.day, "20260615");
        solstone_core_segment::advance_unbound_stream(
            journal.path(),
            "import.image",
            &res_a.created_segment.day,
            &res_a.created_segment.segment,
            StreamHints::default(),
        )
        .unwrap();

        let seg_a_dir = journal
            .path()
            .join("chronicle/20260615/import.image/120000_0");
        let seg_a_orig = fs::read(seg_a_dir.join("original.png")).unwrap();
        let seg_a_transcript = fs::read(seg_a_dir.join("image_transcript.md")).unwrap();

        let pub_b = TestPublication::new();
        let wire_b = RecordingWire {
            request: RefCell::new(None),
        };
        let res_b =
            import_image(&img_b, journal.path(), "import-2", None, &pub_b, &wire_b).unwrap();
        assert_eq!(res_b.created_segment.segment, "120001_0");
        assert_eq!(res_b.created_segment.day, "20260615");
        solstone_core_segment::advance_unbound_stream(
            journal.path(),
            "import.image",
            &res_b.created_segment.day,
            &res_b.created_segment.segment,
            StreamHints::default(),
        )
        .unwrap();

        assert_eq!(
            fs::read(seg_a_dir.join("original.png")).unwrap(),
            seg_a_orig
        );
        assert_eq!(
            fs::read(seg_a_dir.join("image_transcript.md")).unwrap(),
            seg_a_transcript
        );

        assert_ne!(res_a.files_created, res_b.files_created);

        let manifest_a: Value = serde_json::from_slice(
            &fs::read(journal.path().join("imports/import-1/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            manifest_a["files_created"],
            serde_json::json!([seg_a_dir
                .join("image_transcript.md")
                .canonicalize()
                .unwrap()
                .display()
                .to_string()])
        );

        let seg_b_dir = journal
            .path()
            .join("chronicle/20260615/import.image/120001_0");
        let manifest_b: Value = serde_json::from_slice(
            &fs::read(journal.path().join("imports/import-2/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            manifest_b["files_created"],
            serde_json::json!([seg_b_dir
                .join("image_transcript.md")
                .canonicalize()
                .unwrap()
                .display()
                .to_string()])
        );

        assert_eq!(*pub_a.touched_days.borrow(), vec!["20260615"]);
        assert_eq!(*pub_b.touched_days.borrow(), vec!["20260615"]);
    }

    #[test]
    fn three_different_images_take_three_consecutive_keys_and_build_valid_stream_chain() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));

        let img_a = source_dir.path().join("a.png");
        let img_b = source_dir.path().join("b.png");
        let img_c = source_dir.path().join("c.png");
        write_test_png(&img_a, [1, 1, 1], &time);
        write_test_png(&img_b, [2, 2, 2], &time);
        write_test_png(&img_c, [3, 3, 3], &time);

        let publication = TestPublication::new();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };

        let res_a = import_image(
            &img_a,
            journal.path(),
            "import-1",
            None,
            &publication,
            &wire,
        )
        .unwrap();
        assert_eq!(res_a.created_segment.segment, "120000_0");
        solstone_core_segment::advance_unbound_stream(
            journal.path(),
            "import.image",
            &res_a.created_segment.day,
            &res_a.created_segment.segment,
            StreamHints::default(),
        )
        .unwrap();

        let res_b = import_image(
            &img_b,
            journal.path(),
            "import-2",
            None,
            &publication,
            &wire,
        )
        .unwrap();
        assert_eq!(res_b.created_segment.segment, "120001_0");
        solstone_core_segment::advance_unbound_stream(
            journal.path(),
            "import.image",
            &res_b.created_segment.day,
            &res_b.created_segment.segment,
            StreamHints::default(),
        )
        .unwrap();

        let res_c = import_image(
            &img_c,
            journal.path(),
            "import-3",
            None,
            &publication,
            &wire,
        )
        .unwrap();
        assert_eq!(res_c.created_segment.segment, "120002_0");
        solstone_core_segment::advance_unbound_stream(
            journal.path(),
            "import.image",
            &res_c.created_segment.day,
            &res_c.created_segment.segment,
            StreamHints::default(),
        )
        .unwrap();

        let stream_rec: Value = serde_json::from_slice(
            &fs::read(journal.path().join("streams/import.image.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(stream_rec["seq"], 3);

        let marker_c: Value = serde_json::from_slice(
            &fs::read(
                journal
                    .path()
                    .join("chronicle/20260615/import.image/120002_0/stream.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(marker_c["seq"], 3);
        assert_eq!(marker_c["prev_segment"], "120001_0");

        let marker_b: Value = serde_json::from_slice(
            &fs::read(
                journal
                    .path()
                    .join("chronicle/20260615/import.image/120001_0/stream.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(marker_b["seq"], 2);
        assert_eq!(marker_b["prev_segment"], "120000_0");

        let marker_a: Value = serde_json::from_slice(
            &fs::read(
                journal
                    .path()
                    .join("chronicle/20260615/import.image/120000_0/stream.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(marker_a["seq"], 1);
        assert!(marker_a["prev_segment"].is_null());
    }

    #[test]
    fn same_second_png_then_jpg_creates_two_separate_segments() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));

        let img_png = source_dir.path().join("pic.png");
        let img_jpg = source_dir.path().join("pic.jpg");
        write_test_png(&img_png, [100, 100, 100], &time);
        write_test_jpeg(&img_jpg, [100, 100, 100], &time);

        let publication = TestPublication::new();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };

        let res_png = import_image(
            &img_png,
            journal.path(),
            "import-1",
            None,
            &publication,
            &wire,
        )
        .unwrap();
        assert_eq!(res_png.created_segment.segment, "120000_0");
        solstone_core_segment::advance_unbound_stream(
            journal.path(),
            "import.image",
            &res_png.created_segment.day,
            &res_png.created_segment.segment,
            StreamHints::default(),
        )
        .unwrap();

        let res_jpg = import_image(
            &img_jpg,
            journal.path(),
            "import-2",
            None,
            &publication,
            &wire,
        )
        .unwrap();
        assert_eq!(res_jpg.created_segment.segment, "120001_0");
        solstone_core_segment::advance_unbound_stream(
            journal.path(),
            "import.image",
            &res_jpg.created_segment.day,
            &res_jpg.created_segment.segment,
            StreamHints::default(),
        )
        .unwrap();

        let seg_1 = journal
            .path()
            .join("chronicle/20260615/import.image/120000_0");
        let seg_2 = journal
            .path()
            .join("chronicle/20260615/import.image/120001_0");

        assert!(seg_1.join("original.png").is_file());
        assert!(!seg_1.join("original.jpg").exists());

        assert!(seg_2.join("original.jpg").is_file());
        assert!(!seg_2.join("original.png").exists());
    }

    #[test]
    fn an_image_lands_at_a_removed_key_and_steps_past_a_removal_in_progress() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));

        let img = source_dir.path().join("photo.png");
        write_test_png(&img, [7, 8, 9], &time);
        let img_bytes = fs::read(&img).unwrap();

        // 120000_0 has tombstone.json AND matching original.png: a removal
        // part-way through, so the image steps past it.
        let seg_0 = journal
            .path()
            .join("chronicle/20260615/import.image/120000_0");
        fs::create_dir_all(&seg_0).unwrap();
        fs::write(seg_0.join("tombstone.json"), b"{\"tombstone\":true}").unwrap();
        fs::write(seg_0.join("original.png"), &img_bytes).unwrap();

        // 120001_0 has only tombstone.json: the owner removed it, and what
        // arrives again lands there.
        let seg_1 = journal
            .path()
            .join("chronicle/20260615/import.image/120001_0");
        fs::create_dir_all(&seg_1).unwrap();
        fs::write(seg_1.join("tombstone.json"), b"{\"tombstone\":true}").unwrap();

        let publication = TestPublication::new();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };

        let res =
            import_image(&img, journal.path(), "import-1", None, &publication, &wire).unwrap();
        assert_eq!(res.created_segment.segment, "120001_0");
        assert!(!seg_1.join("tombstone.json").exists());
        assert_eq!(fs::read(seg_1.join("original.png")).unwrap(), img_bytes);
        assert!(
            !journal
                .path()
                .join("chronicle/20260615/import.image/120002_0")
                .exists()
        );

        // The removal in progress is untouched.
        assert_eq!(
            fs::read(seg_0.join("tombstone.json")).unwrap(),
            b"{\"tombstone\":true}"
        );
        assert_eq!(fs::read(seg_0.join("original.png")).unwrap(), img_bytes);
    }

    #[test]
    fn same_content_reimport_claims_existing_head_segment_without_creating_new_directory() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));

        let img = source_dir.path().join("sample.png");
        write_test_png(&img, [11, 22, 33], &time);

        let publication = TestPublication::new();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };

        let res_1 =
            import_image(&img, journal.path(), "import-1", None, &publication, &wire).unwrap();
        assert_eq!(res_1.created_segment.segment, "120000_0");
        solstone_core_segment::advance_unbound_stream(
            journal.path(),
            "import.image",
            &res_1.created_segment.day,
            &res_1.created_segment.segment,
            StreamHints::default(),
        )
        .unwrap();

        let seg_dir = journal
            .path()
            .join("chronicle/20260615/import.image/120000_0");
        let orig_bytes = fs::read(seg_dir.join("original.png")).unwrap();
        let transcript_bytes = fs::read(seg_dir.join("image_transcript.md")).unwrap();

        let res_2 =
            import_image(&img, journal.path(), "import-2", None, &publication, &wire).unwrap();
        assert_eq!(res_2.created_segment.segment, "120000_0");
        assert!(
            !journal
                .path()
                .join("chronicle/20260615/import.image/120001_0")
                .exists()
        );

        assert_eq!(fs::read(seg_dir.join("original.png")).unwrap(), orig_bytes);
        assert_eq!(
            fs::read(seg_dir.join("image_transcript.md")).unwrap(),
            transcript_bytes
        );
    }

    #[test]
    fn retry_after_a_later_import_reuses_its_segment_and_keeps_its_chain_position() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let first_time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));
        let later_time = local_time(jiff::civil::date(2026, 6, 15).at(13, 0, 0, 0));
        let first = source_dir.path().join("first.png");
        let later = source_dir.path().join("later.png");
        write_test_png(&first, [11, 22, 33], &first_time);
        write_test_png(&later, [44, 55, 66], &later_time);

        let publication = TestPublication::new();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        let bind = |result: &ImageImportResult| {
            solstone_core_segment::advance_unbound_stream(
                journal.path(),
                "import.image",
                &result.created_segment.day,
                &result.created_segment.segment,
                StreamHints::default(),
            )
            .unwrap()
        };

        let first_result = import_image(
            &first,
            journal.path(),
            "import-1",
            None,
            &publication,
            &wire,
        )
        .unwrap();
        let first_advance = bind(&first_result);
        let later_result = import_image(
            &later,
            journal.path(),
            "import-2",
            None,
            &publication,
            &wire,
        )
        .unwrap();
        let later_advance = bind(&later_result);
        assert_eq!((first_advance.seq, later_advance.seq), (1, 2));

        let stream_dir = journal.path().join("chronicle/20260615/import.image");
        let first_marker = fs::read(stream_dir.join("120000_0/stream.json")).unwrap();
        let later_marker = fs::read(stream_dir.join("130000_0/stream.json")).unwrap();

        let retry = import_image(
            &first,
            journal.path(),
            "import-3",
            None,
            &publication,
            &wire,
        )
        .unwrap();
        assert_eq!(retry.created_segment.segment, "120000_0");
        assert_eq!(bind(&retry), first_advance);

        let mut segments = fs::read_dir(&stream_dir)
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.unwrap();
                entry
                    .file_type()
                    .unwrap()
                    .is_dir()
                    .then(|| entry.file_name().into_string().unwrap())
            })
            .filter(|name| !name.starts_with('.'))
            .collect::<Vec<_>>();
        segments.sort();
        assert_eq!(segments, ["120000_0", "130000_0"]);
        assert_eq!(
            fs::read(stream_dir.join("120000_0/stream.json")).unwrap(),
            first_marker
        );
        assert_eq!(
            fs::read(stream_dir.join("130000_0/stream.json")).unwrap(),
            later_marker
        );
        let record: Value = serde_json::from_slice(
            &fs::read(journal.path().join("streams/import.image.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record["seq"], 2);
        assert_eq!(record["last_segment"], "130000_0");
    }

    #[test]
    fn same_bytes_with_different_extensions_do_not_share_segment() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));

        let img_jpg = source_dir.path().join("photo.jpg");
        let img_jpeg = source_dir.path().join("photo.jpeg");
        write_test_jpeg(&img_jpg, [55, 66, 77], &time);
        fs::copy(&img_jpg, &img_jpeg).unwrap();
        crate::shared::open_to_set_times(&img_jpeg)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(std::time::SystemTime::try_from(time.timestamp()).unwrap()),
            )
            .unwrap();

        let publication = TestPublication::new();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };

        let res_1 = import_image(
            &img_jpg,
            journal.path(),
            "import-1",
            None,
            &publication,
            &wire,
        )
        .unwrap();
        assert_eq!(res_1.created_segment.segment, "120000_0");
        solstone_core_segment::advance_unbound_stream(
            journal.path(),
            "import.image",
            &res_1.created_segment.day,
            &res_1.created_segment.segment,
            StreamHints::default(),
        )
        .unwrap();

        let res_2 = import_image(
            &img_jpeg,
            journal.path(),
            "import-2",
            None,
            &publication,
            &wire,
        )
        .unwrap();
        assert_eq!(res_2.created_segment.segment, "120001_0");

        let seg_0 = journal
            .path()
            .join("chronicle/20260615/import.image/120000_0");
        let seg_1 = journal
            .path()
            .join("chronicle/20260615/import.image/120001_0");

        let seg_0_entries = fs::read_dir(&seg_0)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        assert!(seg_0_entries.contains(&"original.jpg".to_owned()));
        assert!(!seg_0_entries.contains(&"original.jpeg".to_owned()));

        assert!(seg_1.join("original.jpeg").is_file());
    }

    #[test]
    fn corrupt_or_staged_occupants_are_skipped_without_touching_or_creating_candidate() {
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));

        // Case a: no original.*
        {
            let journal = tempfile::tempdir().unwrap();
            let source_dir = tempfile::tempdir().unwrap();
            let img = source_dir.path().join("pic.png");
            write_test_png(&img, [1, 2, 3], &time);

            let seg_0 = journal
                .path()
                .join("chronicle/20260615/import.image/120000_0");
            fs::create_dir_all(&seg_0).unwrap();
            fs::write(seg_0.join("other.txt"), b"hello").unwrap();

            let publ = TestPublication::new();
            let wire = RecordingWire {
                request: RefCell::new(None),
            };
            let res = import_image(&img, journal.path(), "import-1", None, &publ, &wire).unwrap();
            assert_eq!(res.created_segment.segment, "120001_0");
            assert!(seg_0.join("other.txt").is_file());
            assert!(!seg_0.join("original.png").exists());
        }

        // Case b: two original.*
        {
            let journal = tempfile::tempdir().unwrap();
            let source_dir = tempfile::tempdir().unwrap();
            let img = source_dir.path().join("pic.png");
            write_test_png(&img, [1, 2, 3], &time);

            let seg_0 = journal
                .path()
                .join("chronicle/20260615/import.image/120000_0");
            fs::create_dir_all(&seg_0).unwrap();
            fs::write(seg_0.join("original.png"), b"one").unwrap();
            fs::write(seg_0.join("original.jpg"), b"two").unwrap();

            let publ = TestPublication::new();
            let wire = RecordingWire {
                request: RefCell::new(None),
            };
            let res = import_image(&img, journal.path(), "import-1", None, &publ, &wire).unwrap();
            assert_eq!(res.created_segment.segment, "120001_0");
            assert_eq!(fs::read(seg_0.join("original.png")).unwrap(), b"one");
            assert_eq!(fs::read(seg_0.join("original.jpg")).unwrap(), b"two");
        }

        // Case c: original.png is a directory
        {
            let journal = tempfile::tempdir().unwrap();
            let source_dir = tempfile::tempdir().unwrap();
            let img = source_dir.path().join("pic.png");
            write_test_png(&img, [1, 2, 3], &time);

            let seg_0 = journal
                .path()
                .join("chronicle/20260615/import.image/120000_0");
            fs::create_dir_all(seg_0.join("original.png")).unwrap();

            let publ = TestPublication::new();
            let wire = RecordingWire {
                request: RefCell::new(None),
            };
            let res = import_image(&img, journal.path(), "import-1", None, &publ, &wire).unwrap();
            assert_eq!(res.created_segment.segment, "120001_0");
            assert!(seg_0.join("original.png").is_dir());
        }

        // Case d: sibling .removing_120000_0 with candidate 120000_0 absent
        {
            let journal = tempfile::tempdir().unwrap();
            let source_dir = tempfile::tempdir().unwrap();
            let img = source_dir.path().join("pic.png");
            write_test_png(&img, [1, 2, 3], &time);

            let parent = journal.path().join("chronicle/20260615/import.image");
            fs::create_dir_all(&parent).unwrap();
            let staged = parent.join(".removing_120000_0");
            fs::create_dir_all(&staged).unwrap();

            let publ = TestPublication::new();
            let wire = RecordingWire {
                request: RefCell::new(None),
            };
            let res = import_image(&img, journal.path(), "import-1", None, &publ, &wire).unwrap();
            assert_eq!(res.created_segment.segment, "120001_0");
            assert!(!parent.join("120000_0").exists());
            assert!(staged.is_dir());
        }
    }

    #[test]
    fn start_at_day_boundary_overflows_into_error_without_creating_directories() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = local_time(jiff::civil::date(2026, 6, 15).at(23, 59, 59, 0));

        let img = source_dir.path().join("photo.png");
        write_test_png(&img, [99, 99, 99], &time);

        // Occupy 23:59:59 with other content
        let seg = journal
            .path()
            .join("chronicle/20260615/import.image/235959_0");
        fs::create_dir_all(&seg).unwrap();
        fs::write(seg.join("other.bin"), b"block").unwrap();

        let publ = TestPublication::new();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        let err = import_image(&img, journal.path(), "import-1", None, &publ, &wire).unwrap_err();

        match err {
            ImageImportError::ImageSegmentDayOverflow { day, stream, start } => {
                assert_eq!(day, "20260615");
                assert_eq!(stream, "import.image");
                assert_eq!(start, "2026-06-15T23:59:59");
            }
            other => panic!("expected ImageSegmentDayOverflow, got {other:?}"),
        }

        assert!(!journal.path().join("chronicle/20260616").exists());
    }

    #[test]
    fn probe_limit_exhaustion_returns_collision_error_and_does_not_create_probe_limit_plus_one_key()
    {
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));

        // Subcase 1: Occupy 0..58 -> claims 59
        {
            let journal = tempfile::tempdir().unwrap();
            let source_dir = tempfile::tempdir().unwrap();
            let img = source_dir.path().join("photo.png");
            write_test_png(&img, [5, 5, 5], &time);

            for sec in 0..59 {
                let seg = journal
                    .path()
                    .join(format!("chronicle/20260615/import.image/1200{:02}_0", sec));
                fs::create_dir_all(&seg).unwrap();
                fs::write(seg.join("blocker.dat"), b"block").unwrap();
            }

            let publ = TestPublication::new();
            let wire = RecordingWire {
                request: RefCell::new(None),
            };
            let res = import_image(&img, journal.path(), "import-1", None, &publ, &wire).unwrap();
            assert_eq!(res.created_segment.segment, "120059_0");
        }

        // Subcase 2: Occupy 0..59 -> returns ImageSegmentCollision with attempts 60, no 120100_0
        {
            let journal = tempfile::tempdir().unwrap();
            let source_dir = tempfile::tempdir().unwrap();
            let img = source_dir.path().join("photo.png");
            write_test_png(&img, [5, 5, 5], &time);

            for sec in 0..60 {
                let seg = journal
                    .path()
                    .join(format!("chronicle/20260615/import.image/1200{:02}_0", sec));
                fs::create_dir_all(&seg).unwrap();
                fs::write(seg.join("blocker.dat"), b"block").unwrap();
            }

            let publ = TestPublication::new();
            let wire = RecordingWire {
                request: RefCell::new(None),
            };
            let err =
                import_image(&img, journal.path(), "import-1", None, &publ, &wire).unwrap_err();

            match err {
                ImageImportError::ImageSegmentCollision {
                    day,
                    stream,
                    start,
                    attempts,
                } => {
                    assert_eq!(day, "20260615");
                    assert_eq!(stream, "import.image");
                    assert_eq!(start, "2026-06-15T12:00:00");
                    assert_eq!(attempts, 60);
                }
                other => panic!("expected ImageSegmentCollision, got {other:?}"),
            }

            assert!(
                !journal
                    .path()
                    .join("chronicle/20260615/import.image/120100_0")
                    .exists()
            );
        }
    }

    #[test]
    fn start_approaching_day_boundary_takes_last_second_of_day_without_overflow() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();

        // 1. Free 23:59:59 taken as 235959_0
        let time_end = local_time(jiff::civil::date(2026, 6, 15).at(23, 59, 59, 0));
        let img_1 = source_dir.path().join("end.png");
        write_test_png(&img_1, [1, 2, 3], &time_end);

        let publ = TestPublication::new();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        let res_1 = import_image(&img_1, journal.path(), "import-1", None, &publ, &wire).unwrap();
        assert_eq!(res_1.created_segment.segment, "235959_0");

        // 2. In clean journal, start at 23:59:30 with 23:59:30..23:59:58 occupied -> takes 235959_0
        let journal_2 = tempfile::tempdir().unwrap();
        let time_30 = local_time(jiff::civil::date(2026, 6, 15).at(23, 59, 30, 0));
        let img_2 = source_dir.path().join("start30.png");
        write_test_png(&img_2, [4, 5, 6], &time_30);

        for sec in 30..59 {
            let seg = journal_2
                .path()
                .join(format!("chronicle/20260615/import.image/2359{:02}_0", sec));
            fs::create_dir_all(&seg).unwrap();
            fs::write(seg.join("block.dat"), b"block").unwrap();
        }

        let res_2 = import_image(&img_2, journal_2.path(), "import-2", None, &publ, &wire).unwrap();
        assert_eq!(res_2.created_segment.segment, "235959_0");
    }

    #[cfg(unix)]
    #[test]
    fn fresh_claim_and_same_content_take_normalize_directory_permissions_and_reject_symlinks() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));

        let img = source_dir.path().join("photo.png");
        write_test_png(&img, [12, 34, 56], &time);
        let img_bytes = fs::read(&img).unwrap();

        // 1. Fresh claim mode 0o700
        let publ = TestPublication::new();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        let res_1 = import_image(&img, journal.path(), "import-1", None, &publ, &wire).unwrap();
        assert_eq!(res_1.created_segment.segment, "120000_0");
        let seg_0 = journal
            .path()
            .join("chronicle/20260615/import.image/120000_0");
        let mode_0 = fs::metadata(&seg_0).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode_0, 0o700);

        // 2. Same-content take with pre-set 0o755 directory ends at 0o700
        let journal_2 = tempfile::tempdir().unwrap();
        let seg_2_0 = journal_2
            .path()
            .join("chronicle/20260615/import.image/120000_0");
        fs::create_dir_all(&seg_2_0).unwrap();
        fs::write(seg_2_0.join("original.png"), &img_bytes).unwrap();
        fs::set_permissions(&seg_2_0, fs::Permissions::from_mode(0o755)).unwrap();

        let res_2 = import_image(&img, journal_2.path(), "import-2", None, &publ, &wire).unwrap();
        assert_eq!(res_2.created_segment.segment, "120000_0");
        let mode_2_0 = fs::metadata(&seg_2_0).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode_2_0, 0o700);

        // 3. Symlink candidate to directory with matching original.png is not written through, import takes next second
        let journal_3 = tempfile::tempdir().unwrap();
        let target_dir = tempfile::tempdir().unwrap();
        fs::write(target_dir.path().join("original.png"), &img_bytes).unwrap();
        let target_entries_before = fs::read_dir(target_dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>();

        let parent_3 = journal_3.path().join("chronicle/20260615/import.image");
        fs::create_dir_all(&parent_3).unwrap();
        std::os::unix::fs::symlink(target_dir.path(), parent_3.join("120000_0")).unwrap();

        let res_3 = import_image(&img, journal_3.path(), "import-3", None, &publ, &wire).unwrap();
        assert_eq!(res_3.created_segment.segment, "120001_0");

        let target_entries_after = fs::read_dir(target_dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(target_entries_before, target_entries_after);
        assert_eq!(
            fs::read(target_dir.path().join("original.png")).unwrap(),
            img_bytes
        );
    }

    /// A JPEG with an APP1 Exif segment holding:
    /// - Header: 0xFF 0xD8 (SOI), 0xFF 0xE1 (APP1 marker), length (be u16), b"Exif\0\0"
    /// - TIFF header: "II" (little-endian), 0x002A (u16), 0x00000008 (u32 offset to IFD0)
    /// - IFD0 at offset 8:
    ///   - Entry count: 1 (u16)
    ///   - Tag 0x8769 (Exif IFD pointer, LONG, count 1, value offset 26)
    ///   - Next IFD: 0 (u32)
    /// - Exif IFD at offset 26:
    ///   - Entry count: 2 (u16)
    ///   - Tag 0x0132 (ModifyDate, ASCII, count 20, value offset 56): "1999:01:01 00:00:00\0"
    ///   - Tag 0x9003 (DateTimeOriginal, ASCII, count 20, value offset 76): "2026:05:10 14:30:00\0"
    ///   - Next IFD: 0 (u32)
    /// - String values:
    ///   - offset 56: b"1999:01:01 00:00:00\0"
    ///   - offset 76: b"2026:05:10 14:30:00\0"
    /// - Image data: encoded JPEG stream following SOI
    fn jpeg_with_exif_dates(dt_original: &str, mod_date: &str) -> Vec<u8> {
        let pixels = ImageBuffer::from_fn(4, 4, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        });
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, 80)
            .encode_image(&DynamicImage::ImageRgb8(pixels))
            .unwrap();
        assert_eq!(&encoded[..2], [0xff, 0xd8]);

        let mut tiff = Vec::new();
        tiff.extend_from_slice(&[b'I', b'I', 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00]);

        let exif_ifd_offset = 26u32;
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x8769u16.to_le_bytes());
        tiff.extend_from_slice(&4u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&exif_ifd_offset.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());

        let dt_bytes = format!("{dt_original}\0").into_bytes();
        let mod_bytes = format!("{mod_date}\0").into_bytes();
        let num_entries = 2u16;
        let ifd_len = 2 + (num_entries as usize * 12) + 4;
        let mut string_data_offset = (26 + ifd_len) as u32;

        tiff.extend_from_slice(&num_entries.to_le_bytes());

        // Entry 1: Tag 0x0132 (ModifyDate)
        tiff.extend_from_slice(&0x0132u16.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes());
        tiff.extend_from_slice(&(mod_bytes.len() as u32).to_le_bytes());
        tiff.extend_from_slice(&string_data_offset.to_le_bytes());
        string_data_offset += mod_bytes.len() as u32;

        // Entry 2: Tag 0x9003 (DateTimeOriginal)
        tiff.extend_from_slice(&0x9003u16.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes());
        tiff.extend_from_slice(&(dt_bytes.len() as u32).to_le_bytes());
        tiff.extend_from_slice(&string_data_offset.to_le_bytes());

        tiff.extend_from_slice(&0u32.to_le_bytes());

        tiff.extend_from_slice(&mod_bytes);
        tiff.extend_from_slice(&dt_bytes);

        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(&tiff);
        let length = u16::try_from(payload.len() + 2).unwrap();
        let mut jpeg = encoded[..2].to_vec();
        jpeg.extend_from_slice(&[0xff, 0xe1]);
        jpeg.extend_from_slice(&length.to_be_bytes());
        jpeg.extend_from_slice(&payload);
        jpeg.extend_from_slice(&encoded[2..]);
        jpeg
    }

    #[test]
    fn exif_photo_preview_size_and_prepare_share_creation_day() {
        let temp = tempfile::tempdir().unwrap();
        let photo_path = temp.path().join("photo.jpg");
        fs::write(
            &photo_path,
            jpeg_with_exif_dates("2026:05:10 14:30:00", "1999:01:01 00:00:00"),
        )
        .unwrap();

        let old_time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_704_067_200);
        let file = std::fs::File::options()
            .write(true)
            .open(&photo_path)
            .unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(old_time))
            .unwrap();

        let preview_res = preview(&photo_path, &TimeZone::UTC, None, None);
        assert_eq!(preview_res.date_range.0, "20260510");
        assert_eq!(preview_res.date_range.1, "20260510");

        let journal_dir = tempfile::tempdir().unwrap();
        let size_res =
            preview_size(&photo_path, journal_dir.path(), &TimeZone::UTC, None, None).unwrap();
        let size_day = &size_res.recordings.iter().next().unwrap().day;
        assert_eq!(size_day, "20260510");

        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        let prepared = prepare_image(&photo_path, &wire, &TimeZone::UTC, None, None).unwrap();
        assert_eq!(prepared.day, "20260510");
        assert!(prepared.segment.starts_with("143000"));
    }

    #[cfg(all(test, feature = "full-tests"))]
    struct StoppingWire {
        request: RefCell<Option<GenerateRequest>>,
        stop: solstone_core_import::WorkStop,
    }

    #[cfg(all(test, feature = "full-tests"))]
    impl WireClient for StoppingWire {
        fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            self.request.replace(Some(request.clone()));
            self.stop.request();
            Ok(generated("secret description"))
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn files_named(root: &Path, name: &str) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.file_name().is_some_and(|item| item == name) {
                    found.push(path);
                }
            }
        }
        found.sort();
        found
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn a_stopped_vision_call_discards_the_description_and_installs_nothing() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));
        let source = source_dir.path().join("photo.png");
        write_test_png(&source, [10, 20, 30], &time);
        let source_bytes = fs::read(&source).unwrap();
        let stop = solstone_core_import::WorkStop::new();
        let _guard = solstone_core_import::WorkStopGuard::install(&stop);
        let wire = StoppingWire {
            request: RefCell::new(None),
            stop: stop.clone(),
        };
        let zone = solstone_core_journal_config::owner_zone(journal.path());
        let selected =
            jiff::Timestamp::try_from(fs::metadata(&source).unwrap().modified().unwrap())
                .unwrap()
                .to_zoned_datetime(zone.clone())
                .strftime("%Y%m%d_%H%M%S")
                .to_string();
        let prepared = prepare_image(&source, &wire, &zone, Some(&selected), None).unwrap();
        assert!(matches!(
            prepared.description,
            DescriptionOutcome::Unavailable { ref reason } if reason == "generate cancelled"
        ));
        assert!(wire.request.borrow().is_some());
        let publication = TestPublication::new();
        let error =
            install_and_publish_image(&prepared, journal.path(), "import-1", &publication, None)
                .unwrap_err();
        assert!(matches!(error, ImageImportError::Stopped));
        assert_eq!(error.to_string(), "import stopped");
        assert!(publication.touched_days.borrow().is_empty());
        assert_eq!(fs::read(&source).unwrap(), source_bytes);
        assert!(files_named(journal.path(), "original.png").is_empty());
        assert!(files_named(journal.path(), "image_transcript.md").is_empty());
        assert!(
            !journal
                .path()
                .join("imports/import-1/manifest.json")
                .exists()
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn an_already_stopped_image_does_not_call_the_model() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));
        let source = source_dir.path().join("photo.png");
        write_test_png(&source, [10, 20, 30], &time);
        let stop = solstone_core_import::WorkStop::new();
        stop.request();
        let _guard = solstone_core_import::WorkStopGuard::install(&stop);
        let wire = StoppingWire {
            request: RefCell::new(None),
            stop: stop.clone(),
        };
        let publication = TestPublication::new();
        let error = import_image(
            &source,
            journal.path(),
            "import-1",
            None,
            &publication,
            &wire,
        )
        .unwrap_err();
        assert!(matches!(error, ImageImportError::Stopped));
        assert_eq!(error.to_string(), "import stopped");
        assert!(wire.request.borrow().is_none());
        assert!(publication.touched_days.borrow().is_empty());
        assert!(files_named(journal.path(), "original.png").is_empty());
        assert!(
            !journal
                .path()
                .join("imports/import-1/manifest.json")
                .exists()
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn a_stopped_second_image_keeps_the_admitted_original() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = local_time(jiff::civil::date(2026, 6, 15).at(12, 0, 0, 0));
        let first = source_dir.path().join("first.png");
        let second = source_dir.path().join("second.png");
        write_test_png(&first, [10, 20, 30], &time);
        write_test_png(&second, [40, 50, 60], &time);
        let first_bytes = fs::read(&first).unwrap();
        let publication = TestPublication::new();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        let admitted = import_image(
            &first,
            journal.path(),
            "import-1",
            None,
            &publication,
            &wire,
        )
        .unwrap();
        let original = journal
            .path()
            .join("chronicle")
            .join(&admitted.created_segment.day)
            .join("import.image")
            .join(&admitted.created_segment.segment)
            .join("original.png");
        let transcript = original.with_file_name("image_transcript.md");
        let manifest = journal.path().join("imports/import-1/manifest.json");
        let kept_original = fs::read(&original).unwrap();
        let kept_transcript = fs::read(&transcript).unwrap();
        let kept_manifest = fs::read(&manifest).unwrap();
        assert_eq!(kept_original, first_bytes);

        let stop = solstone_core_import::WorkStop::new();
        let _guard = solstone_core_import::WorkStopGuard::install(&stop);
        let stopping = StoppingWire {
            request: RefCell::new(None),
            stop: stop.clone(),
        };
        let stopped_publication = TestPublication::new();
        let error = import_image(
            &second,
            journal.path(),
            "import-2",
            None,
            &stopped_publication,
            &stopping,
        )
        .unwrap_err();
        assert!(matches!(error, ImageImportError::Stopped));
        assert_eq!(error.to_string(), "import stopped");
        assert!(stopping.request.borrow().is_some());
        assert!(stopped_publication.touched_days.borrow().is_empty());
        assert_eq!(fs::read(&original).unwrap(), kept_original);
        assert_eq!(fs::read(&transcript).unwrap(), kept_transcript);
        assert_eq!(fs::read(&manifest).unwrap(), kept_manifest);
        assert_eq!(files_named(journal.path(), "original.png"), vec![original]);
        assert!(
            !journal
                .path()
                .join("imports/import-2/manifest.json")
                .exists()
        );
        assert!(!transcript.to_string_lossy().contains("secret description"));
        assert!(!String::from_utf8_lossy(&kept_transcript).contains("secret description"));
    }

    #[test]
    fn photo_keeps_capture_zone_ignores_filename_and_writes_caption_and_meta() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        // Conflicting filename date (2019-03-04) + Tokyo capture time during US DST spring-forward gap
        // (2026-03-08 02:30:00 +09:00, which does not exist in America/Denver).
        let source = source_dir.path().join("20190304_120000.jpg");
        let mut base = Vec::new();
        JpegEncoder::new_with_quality(&mut base, 80)
            .encode_image(&DynamicImage::ImageRgb8(ImageBuffer::from_pixel(
                8,
                8,
                Rgb([40, 80, 120]),
            )))
            .unwrap();
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II*\0");
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x8769u16.to_le_bytes());
        tiff.extend_from_slice(&4u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&26u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        let dt = b"2026:03:08 02:30:00\0";
        let off = b"+09:00\0";
        let val_start = 26 + 2 + 2 * 12 + 4;
        tiff.extend_from_slice(&2u16.to_le_bytes());
        for (tag, len, ptr) in [
            (0x9003u16, dt.len() as u32, val_start as u32),
            (0x9011u16, off.len() as u32, (val_start + dt.len()) as u32),
        ] {
            tiff.extend_from_slice(&tag.to_le_bytes());
            tiff.extend_from_slice(&2u16.to_le_bytes());
            tiff.extend_from_slice(&len.to_le_bytes());
            tiff.extend_from_slice(&ptr.to_le_bytes());
        }
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(dt);
        tiff.extend_from_slice(off);
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(&tiff);
        let len = u16::try_from(payload.len() + 2).unwrap();
        let mut jpeg = base[..2].to_vec();
        jpeg.extend_from_slice(&[0xFF, 0xE1]);
        jpeg.extend_from_slice(&len.to_be_bytes());
        jpeg.extend_from_slice(&payload);
        jpeg.extend_from_slice(&base[2..]);
        fs::write(&source, &jpeg).unwrap();

        let denver = solstone_core_time::zone("America/Denver").unwrap();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        let prepared = prepare_manual_image_with_options(
            &source,
            &wire,
            &denver,
            None,
            Some("20261119_100000"),
            ImageImportOptions {
                caption: Some("late night shinjuku".to_owned()),
                client_captured_at: None,
                original_filename: Some("20190304_120000.jpg".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(prepared.day, "20260308");
        assert_eq!(prepared.segment, "023000_0");
        assert_eq!(prepared.time_source, PhotoTimeSource::Capture);
        assert_eq!(prepared.utc_offset_seconds, Some(32400));

        let publication = TestPublication::new();
        let result = install_and_publish_image(
            &prepared,
            journal.path(),
            "import-tokyo",
            &publication,
            None,
        )
        .unwrap();
        assert_eq!(result.created_segment.day, "20260308");
        assert_eq!(result.created_segment.segment, "023000_0");
        let seg_dir = journal
            .path()
            .join("chronicle/20260308/import.image/023000_0");
        let meta: serde_json::Value =
            serde_json::from_slice(&fs::read(seg_dir.join("image_meta.json")).unwrap()).unwrap();
        assert_eq!(meta["time_source"], "capture");
        assert_eq!(meta["utc_offset_seconds"], 32400);
        assert_eq!(meta["caption"], "late night shinjuku");
        let transcript = fs::read_to_string(&result.files_created[0]).unwrap();
        let cap_pos = transcript
            .find("**Caption:**\nlate night shinjuku")
            .unwrap();
        let model_pos = transcript.find("> [image description").unwrap();
        assert!(cap_pos < model_pos);
    }

    #[test]
    fn metadata_free_photo_records_upload_fallback_in_image_meta() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        // Filename date must NOT date a photo.
        let source = source_dir.path().join("20190304_120000.png");
        fs::write(&source, png_bytes()).unwrap();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        let prepared = prepare_manual_image(
            &source,
            &wire,
            &TimeZone::UTC,
            None,
            Some("20261119_101500"),
        )
        .unwrap();
        assert_eq!(prepared.day, "20261119");
        assert_eq!(prepared.segment, "101500_0");
        assert_eq!(prepared.time_source, PhotoTimeSource::UploadFallback);
        assert_eq!(prepared.utc_offset_seconds, None);

        let publication = TestPublication::new();
        install_and_publish_image(&prepared, journal.path(), "import-fb", &publication, None)
            .unwrap();
        let meta: serde_json::Value = serde_json::from_slice(
            &fs::read(
                journal
                    .path()
                    .join("chronicle/20261119/import.image/101500_0/image_meta.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(meta["time_source"], "upload_fallback");
        assert!(meta.get("utc_offset_seconds").is_none());
    }

    struct SequenceWire {
        responses: RefCell<std::collections::VecDeque<Result<GenerateResponse, ClientError>>>,
    }

    impl SequenceWire {
        fn new(responses: Vec<Result<GenerateResponse, ClientError>>) -> Self {
            Self {
                responses: RefCell::new(responses.into()),
            }
        }
    }

    impl WireClient for SequenceWire {
        fn execute(&self, _request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            self.responses
                .borrow_mut()
                .pop_front()
                .expect("expected mock vision response")
        }
    }

    struct MockPhotoFaceDetector {
        encoder: solstone_core_entity::faceprints::Encoder,
        faces: Vec<solstone_core_entity::faceprints::photo_match::PhotoFaceCandidate>,
    }

    impl crate::photo_faces::PhotoFaceDetector for MockPhotoFaceDetector {
        fn detect_photo(
            &self,
            _image_path: &Path,
            _journal: Option<&Path>,
            with_crops: bool,
        ) -> Result<crate::photo_faces::DetectedPhotoFaces, String> {
            let faces = self
                .faces
                .iter()
                .map(|f| {
                    let mut c = f.clone();
                    if !with_crops {
                        c.crop_jpeg = None;
                    }
                    c
                })
                .collect();
            Ok(crate::photo_faces::DetectedPhotoFaces {
                encoder: self.encoder.clone(),
                width: 640,
                height: 480,
                faces,
            })
        }
    }

    fn distinct_jpeg(seed: u8) -> Vec<u8> {
        let pixels = ImageBuffer::from_fn(64, 48, |x, y| {
            Rgb([
                ((x as u8).wrapping_add(seed)),
                ((y as u8).wrapping_mul(3).wrapping_add(seed)),
                seed.wrapping_mul(17),
            ])
        });
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 85)
            .encode_image(&DynamicImage::ImageRgb8(pixels))
            .unwrap();
        out
    }

    fn unit_face_vec(dim: usize) -> Vec<f32> {
        let mut v = vec![0.0_f32; solstone_core_entity::faceprints::WIDTH];
        v[dim] = 1.0;
        v
    }

    fn seed_meeting_person_with_looks(
        journal: &Path,
        entity_id: &str,
        name: &str,
        dim: usize,
        looks: usize,
    ) -> solstone_core_entity::faceprints::Encoder {
        use solstone_core_entity::faceprints::{self, Encoder, Face, FaceKey, Metadata, Source};
        use std::collections::{BTreeMap, BTreeSet};

        let folder = entity_id.to_owned();
        let ent_dir = journal.join("entities").join(&folder);
        fs::create_dir_all(&ent_dir).unwrap();
        let entity_json = serde_json::json!({
            "id": entity_id,
            "name": name,
            "type": "Person",
        });
        fs::write(
            ent_dir.join("entity.json"),
            serde_json::to_vec(&entity_json).unwrap(),
        )
        .unwrap();

        let encoder = Encoder {
            id: "opencv-sface-2021dec".to_owned(),
            sha256: "test-sface-sha256".to_owned(),
            width: faceprints::WIDTH,
        };
        let mut desired: BTreeMap<String, Vec<Face>> = BTreeMap::new();
        let mut meeting_faces = Vec::new();
        for i in 0..looks {
            let mut emb = unit_face_vec(dim);
            emb[20 + i] = 0.42; // distinct looks across segments (cos < 0.93) while staying in consensus
            let norm = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
            for x in &mut emb {
                *x /= norm;
            }
            let day = format!("202611{:02}", 10 + i);
            let seg_dir = journal
                .join("chronicle")
                .join(&day)
                .join("screen")
                .join("100000_300");
            fs::create_dir_all(&seg_dir).unwrap();
            let prefix = faceprints::accumulation::source_prefix("screen.webm");
            let crops_dir = seg_dir.join(format!("{prefix}-crops"));
            fs::create_dir_all(&crops_dir).unwrap();
            let crop_name = format!("0-{i}-meeting.jpg");
            fs::write(crops_dir.join(&crop_name), distinct_jpeg(i as u8)).unwrap();
            let key = FaceKey {
                day: day.clone(),
                stream: "screen".to_owned(),
                stream_layout: "named".to_owned(),
                segment_key: "100000_300".to_owned(),
                source_name: "screen.webm".to_owned(),
                frame_id: i as u64,
                face_idx: 0,
            };
            meeting_faces.push(Face {
                embedding: emb,
                metadata: Metadata {
                    schema_version: 1,
                    key,
                    activities: BTreeSet::from([format!("[\"work\",\"{day}\",\"act-{i}\"]")]),
                    source: Source::Anchored,
                    label_text: Some(name.to_owned()),
                    quality: 0.70,
                    det_score: 0.95,
                    face_w: 80.0,
                    category: "meeting".to_owned(),
                    crop: Some(format!("{prefix}-crops/{crop_name}")),
                    added_at: "2026-11-10T10:00:00Z".to_owned(),
                    rejected: false,
                },
            });
        }
        desired.insert(entity_id.to_owned(), meeting_faces);
        faceprints::reconcile(
            journal,
            "[\"work\",\"20261110\",\"act-0\"]",
            &encoder,
            &desired,
        )
        .unwrap();
        encoder
    }

    #[test]
    fn photo_observations_pass_all_four_closure_tests_and_preserve_same_second_capture() {
        use solstone_core_segment::photo_observation::{
            PhotoObservationResolution, TextAttribution, resolve_photo_observation,
        };

        let journal = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        let p1 = src_dir.path().join("burst_1.jpg");
        let p2 = src_dir.path().join("burst_2.jpg");
        let p3 = src_dir.path().join("undated_noise.jpg");
        fs::write(&p1, distinct_jpeg(1)).unwrap();
        fs::write(&p2, distinct_jpeg(2)).unwrap();
        fs::write(&p3, distinct_jpeg(3)).unwrap();

        let wire = SequenceWire::new(vec![
            Ok(generated(
                r#"{"description":"Two people standing by a trailhead sign.","visible_text":"SOUTH MESA TRAIL 7,200 FT","belonging_class":"lived_moment"}"#,
            )),
            Ok(generated(
                r#"{"description":"Close-up of a handwritten coffee menu on a chalkboard.","visible_text":"ESPRESSO $4 / CORTADO $5","belonging_class":"reference_to_remember"}"#,
            )),
            Ok(generated(
                r#"{"description":"Dark blurry inside of a coat pocket.","visible_text":null,"belonging_class":"noise_or_accidental"}"#,
            )),
        ]);
        let publication = TestPublication::new();

        // Closure 1 & 2: Two distinct photos captured in the EXACT SAME SECOND with offset,
        // plus one metadata-free hand-shared noise photo (must STILL be kept!).
        let prep1 = prepare_manual_image_with_options(
            &p1,
            &wire,
            &TimeZone::UTC,
            None,
            None,
            ImageImportOptions {
                caption: Some("Early hike".to_owned()),
                client_captured_at: Some("2026-11-19T12:00:00-07:00".to_owned()),
                original_filename: Some("IMG_1001.JPG".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
        let prep2 = prepare_manual_image_with_options(
            &p2,
            &wire,
            &TimeZone::UTC,
            None,
            None,
            ImageImportOptions {
                caption: Some("Coffee stop".to_owned()),
                client_captured_at: Some("2026-11-19T12:00:00-07:00".to_owned()),
                original_filename: Some("IMG_1002.JPG".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
        let prep3 = prepare_manual_image(&p3, &wire, &TimeZone::UTC, None, Some("20261119_153000"))
            .unwrap();

        let res1 =
            install_and_publish_image(&prep1, journal.path(), "imp-burst-1", &publication, None)
                .unwrap();
        let res2 =
            install_and_publish_image(&prep2, journal.path(), "imp-burst-2", &publication, None)
                .unwrap();
        let res3 =
            install_and_publish_image(&prep3, journal.path(), "imp-noise-3", &publication, None)
                .unwrap();

        let obs1 = res1.observation.expect("photo 1 observation");
        let obs2 = res2.observation.expect("photo 2 observation");
        let obs3 = res3.observation.expect("hand-shared noise photo kept");

        // Same-second collision yields distinct segment coordinates while BOTH preserve true capture instant!
        assert_eq!(obs1.source.segment, "120000_0");
        assert_eq!(obs2.source.segment, "120001_0");
        assert_ne!(obs1.source.source_sha256, obs2.source.source_sha256);
        assert_eq!(
            obs1.time.captured_at.as_deref(),
            Some("2026-11-19T12:00:00-07:00")
        );
        assert_eq!(
            obs2.time.captured_at.as_deref(),
            Some("2026-11-19T12:00:00-07:00")
        );
        assert_eq!(obs1.time.utc_offset_seconds, Some(-25200));
        assert_eq!(obs2.time.utc_offset_seconds, Some(-25200));
        assert_eq!(obs1.time.precision, PhotoTimePrecision::Second);
        assert_eq!(obs1.time.provenance, PhotoTimeProvenance::ClientCapturedAt);
        assert_eq!(obs1.time.time_source, "capture");

        // Attribution separation: owner caption, model visual description, model visible text.
        assert_eq!(
            obs1.owner_caption,
            Some(AttributedText {
                text: "Early hike".to_owned(),
                attribution: TextAttribution::Owner
            })
        );
        assert_eq!(
            obs1.description,
            Some(AttributedText {
                text: "Two people standing by a trailhead sign.".to_owned(),
                attribution: TextAttribution::Model
            })
        );
        assert_eq!(
            obs1.visible_text,
            Some(AttributedText {
                text: "SOUTH MESA TRAIL 7,200 FT".to_owned(),
                attribution: TextAttribution::Model
            })
        );
        assert_eq!(obs1.belonging_class, Some(BelongingClass::LivedMoment));
        assert_eq!(
            obs2.belonging_class,
            Some(BelongingClass::ReferenceToRemember)
        );
        // Transcript contains all three separated for single-pass sense_static consumption.
        let md1 = fs::read_to_string(&res1.files_created[0]).unwrap();
        assert!(md1.contains("**Caption:**\nEarly hike"));
        assert!(md1.contains("> [image description — model-derived]"));
        assert!(md1.contains("> [visible text — model-derived]"));
        assert!(md1.contains("SOUTH MESA TRAIL"));

        // Hand-shared noise photo is kept with upload_fallback provenance and NoiseOrAccidental class.
        assert_eq!(
            obs3.belonging_class,
            Some(BelongingClass::NoiseOrAccidental)
        );
        assert_eq!(obs3.time.time_source, "upload_fallback");
        assert_eq!(obs3.time.captured_at, None);
        assert_eq!(obs3.time.precision, PhotoTimePrecision::Unknown);
        assert_eq!(obs3.time.provenance, PhotoTimeProvenance::UploadFallback);

        // Closure 4: Downstream resolution lifecycle (live -> trash -> restore -> replace).
        let ref1 = obs1.reference();
        assert_eq!(
            resolve_photo_observation(journal.path(), &ref1),
            PhotoObservationResolution::Resolved(Box::new(obs1.clone()))
        );
        let seg1_dir = journal
            .path()
            .join("chronicle/20261119/import.image/120000_0");
        fs::write(
            seg1_dir.join("tombstone.json"),
            r#"{"trashed_at":"2026-11-20T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(
            resolve_photo_observation(journal.path(), &ref1),
            PhotoObservationResolution::NotFound
        );
        fs::remove_file(seg1_dir.join("tombstone.json")).unwrap();
        assert_eq!(
            resolve_photo_observation(journal.path(), &ref1),
            PhotoObservationResolution::Resolved(Box::new(obs1))
        );
        fs::write(seg1_dir.join("original.jpg"), distinct_jpeg(99)).unwrap();
        assert_eq!(
            resolve_photo_observation(journal.path(), &ref1),
            PhotoObservationResolution::Replaced
        );
    }

    #[test]
    fn failure_states_are_distinct_and_transient_failure_retries_while_pixels_exist() {
        let journal = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        let p = src_dir.path().join("sunset.jpg");
        fs::write(&p, distinct_jpeg(42)).unwrap();

        // 1. Thinking off (`NoEngineConfigured`) vs transient failure (`retryable: true`) vs non-retryable.
        let thinking_off =
            interpret_generate_vision(Ok(refused(RefusalReason::NoEngineConfigured)));
        assert_eq!(thinking_off.state(), PhotoObservationState::ThinkingOff);

        let transient = interpret_generate_vision(Ok(GenerateResponse::Refused(RefusedResponse {
            id: None,
            reason: RefusalReason::Unknown,
            reason_code: None,
            retryable: true,
            blocking: false,
            reset_at_ms: None,
            provider: None,
            detail: "rate limited".to_owned(),
        })));
        assert_eq!(transient.state(), PhotoObservationState::TransientFailure);

        let unavailable =
            interpret_generate_vision(Ok(refused(RefusalReason::SchemaValidationFailed)));
        assert_eq!(
            unavailable.state(),
            PhotoObservationState::AnalysisUnavailable
        );

        // 2. Import with transient failure -> retry succeeds and bumps observation_revision.
        let wire = SequenceWire::new(vec![
            Ok(GenerateResponse::Refused(RefusedResponse {
                id: None,
                reason: RefusalReason::AttestationStale,
                reason_code: None,
                retryable: true,
                blocking: false,
                reset_at_ms: None,
                provider: None,
                detail: "temporary hiccup".to_owned(),
            })),
            Ok(generated(
                r#"{"description":"Orange sky over the Flatirons at dusk.","visible_text":null,"belonging_class":"lived_moment"}"#,
            )),
        ]);
        let prep = prepare_manual_image_with_options(
            &p,
            &wire,
            &TimeZone::UTC,
            None,
            None,
            ImageImportOptions {
                caption: Some("Evening view".to_owned()),
                client_captured_at: Some("2026-11-19T17:45:00-07:00".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
        let publication = TestPublication::new();
        let installed =
            install_and_publish_image(&prep, journal.path(), "imp-retry", &publication, None)
                .unwrap();
        let obs_v1 = installed.observation.unwrap();
        assert_eq!(obs_v1.state, PhotoObservationState::TransientFailure);
        assert_eq!(obs_v1.observation_revision, 1);
        assert_eq!(obs_v1.retry_count, 0);

        let retried = retry_photo_observation(
            journal.path(),
            "20261119",
            "import.image",
            &obs_v1.source.segment,
            &wire,
            &crate::photo_faces::SystemPhotoFaceDetector,
        )
        .unwrap()
        .unwrap();
        assert!(retried.retried);
        assert_eq!(retried.observation.state, PhotoObservationState::Ready);
        assert_eq!(retried.observation.observation_revision, 2);
        assert_eq!(retried.observation.retry_count, 1);
        assert_eq!(
            retried
                .observation
                .owner_caption
                .as_ref()
                .map(|c| c.text.as_str()),
            Some("Evening view")
        );
        assert_eq!(
            retried
                .observation
                .description
                .as_ref()
                .map(|d| d.text.as_str()),
            Some("Orange sky over the Flatirons at dusk.")
        );
        assert_eq!(
            retried.observation.belonging_class,
            Some(BelongingClass::LivedMoment)
        );
    }

    #[test]
    fn photo_face_pass_honors_flags_links_profile_pics_without_altering_meeting_thresholds_and_query_is_side_effect_free()
     {
        use solstone_core_entity::faceprints::{self, Source, panel, photo_match};

        let journal = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        let photo_path = src_dir.path().join("portrait.jpg");
        fs::write(&photo_path, distinct_jpeg(7)).unwrap();

        // Seed Alice with 3 meeting looks -> State::Possible (below SUGGESTED_LOOKS=5).
        let encoder = seed_meeting_person_with_looks(journal.path(), "ent-alice", "Alice", 1, 3);
        let detector = MockPhotoFaceDetector {
            encoder,
            faces: vec![photo_match::PhotoFaceCandidate {
                face_idx: 0,
                bbox: [100.0, 80.0, 96.0, 96.0],
                det_score: 0.96,
                sharpness: 380.0,
                quality: 0.92,
                embedding: unit_face_vec(1),
                crop_jpeg: Some(distinct_jpeg(55)),
            }],
        };

        // 1. Side-effect-free query matches Alice without touching entities or disk.
        let before_archive = faceprints::load(journal.path(), "ent-alice")
            .unwrap()
            .0
            .unwrap();
        let query_matches = crate::photo_faces::query_photo_faces_against_journal(
            journal.path(),
            &photo_path,
            &detector,
        )
        .unwrap();
        assert_eq!(query_matches.len(), 1);
        assert_eq!(query_matches[0].entity_id, "ent-alice");
        assert!(query_matches[0].similarity >= photo_match::PHOTO_MATCH_COSINE_MIN);
        let after_query_archive = faceprints::load(journal.path(), "ent-alice")
            .unwrap()
            .0
            .unwrap();
        assert_eq!(before_archive, after_query_archive);

        // 2. Explicit faces.photo_match_enabled: false (while faces.enabled is true)
        // disables BOTH side-effect-free selection queries and the photo face pass.
        fs::create_dir_all(journal.path().join("config")).unwrap();
        fs::write(
            journal.path().join("config/journal.json"),
            r#"{"faces":{"enabled":true,"photo_match_enabled":false}}"#,
        )
        .unwrap();
        assert!(
            crate::photo_faces::query_photo_faces_against_journal(
                journal.path(),
                &photo_path,
                &detector,
            )
            .unwrap()
            .is_empty()
        );
        let batch_off = crate::photo_faces::query_photo_faces_batch_against_journal(
            journal.path(),
            std::slice::from_ref(&photo_path),
            &detector,
        )
        .unwrap();
        assert_eq!(batch_off.len(), 1);
        assert!(batch_off[0].1.is_empty());

        let wire = SequenceWire::new(vec![
            Ok(generated(
                r#"{"description":"Alice smiling outdoors.","visible_text":null,"belonging_class":"lived_moment"}"#,
            )),
            Ok(generated(
                r#"{"description":"Alice smiling outdoors.","visible_text":null,"belonging_class":"lived_moment"}"#,
            )),
        ]);
        let publication = TestPublication::new();
        let prep_off = prepare_manual_image_with_options(
            &photo_path,
            &wire,
            &TimeZone::UTC,
            None,
            None,
            ImageImportOptions {
                client_captured_at: Some("2026-11-19T14:00:00Z".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
        let res_off = install_photo_in_stream_with_faces(
            &prep_off,
            journal.path(),
            IMPORT_STREAM,
            prep_off.timestamp.datetime(),
            &publication,
            None,
            None,
            &detector,
        )
        .unwrap();
        let obs_off = res_off.observation.unwrap();
        assert!(!obs_off.faces_checked);
        assert!(obs_off.faces.is_empty());
        assert_eq!(
            faceprints::load(journal.path(), "ent-alice")
                .unwrap()
                .0
                .unwrap()
                .faces
                .len(),
            3
        );

        // 3. Default config (photo_match_enabled absent -> ON by default on desktop) links Alice & adds photo tile!
        fs::write(journal.path().join("config/journal.json"), r#"{}"#).unwrap();
        let photo_path_2 = src_dir.path().join("portrait2.jpg");
        fs::write(&photo_path_2, distinct_jpeg(8)).unwrap();
        let prep_on = prepare_manual_image_with_options(
            &photo_path_2,
            &wire,
            &TimeZone::UTC,
            None,
            None,
            ImageImportOptions {
                client_captured_at: Some("2026-11-19T14:05:00Z".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
        let res_on = install_photo_in_stream_with_faces(
            &prep_on,
            journal.path(),
            IMPORT_STREAM,
            prep_on.timestamp.datetime(),
            &publication,
            None,
            None,
            &detector,
        )
        .unwrap();
        let obs_on = res_on.observation.unwrap();
        assert!(obs_on.faces_checked);
        assert_eq!(obs_on.faces.len(), 1);
        assert_eq!(obs_on.faces[0].entity_id, "ent-alice");
        assert!(obs_on.faces[0].crop.is_some());

        // Check Alice's panel: stays Possible (3 meeting looks, NOT promoted to Suggested),
        // includes the new Source::Photo face as a candidate profile pic, and auto-default stays None!
        let (archive, state) = faceprints::load(journal.path(), "ent-alice").unwrap();
        let archive = archive.unwrap();
        assert_eq!(archive.faces.len(), 4);
        assert!(
            archive
                .faces
                .iter()
                .any(|f| f.metadata.source == Source::Photo)
        );
        let p = panel::panel(&archive.faces, &state, false, None, None);
        assert_eq!(p.state, panel::State::Possible);
        assert_eq!(p.looks, 3);
        assert_eq!(p.default, None);
        let photo_key = archive
            .faces
            .iter()
            .find(|f| f.metadata.source == Source::Photo)
            .unwrap()
            .metadata
            .key
            .clone();
        assert!(p.faces.contains(&photo_key));

        // Choosing the photo face confirms it as Alice's profile pic!
        faceprints::choose(journal.path(), "ent-alice", &photo_key).unwrap();
        let confirmed = faceprints::confirmed_photo(journal.path(), "ent-alice").unwrap();
        assert_eq!(confirmed, Some(distinct_jpeg(55)));
    }

    #[test]
    fn vision_jpeg_passthrough_strips_exif_and_preserves_pixels() {
        let dir = tempfile::tempdir().unwrap();
        let rgb = image::RgbImage::from_fn(320, 240, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 239) as u8, ((x + y) % 223) as u8])
        });
        let mut base = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut base, 75)
            .encode_image(&rgb)
            .unwrap();
        assert_eq!(&base[..2], &[0xFF, 0xD8]);

        // Inject synthetic APP1 (EXIF/GPS secret), COM comment secret, and trailing bytes after EOI.
        let exif_payload = b"Exif\0\0SECRET_GPS_40.015_-105.270";
        let com_payload = b"SECRET_CAMERA_COMMENT";
        let mut with_meta = Vec::new();
        with_meta.extend_from_slice(&[0xFF, 0xD8]);
        with_meta.push(0xFF);
        with_meta.push(0xE1);
        with_meta.extend_from_slice(&((exif_payload.len() + 2) as u16).to_be_bytes());
        with_meta.extend_from_slice(exif_payload);
        with_meta.push(0xFF);
        with_meta.push(0xFE);
        with_meta.extend_from_slice(&((com_payload.len() + 2) as u16).to_be_bytes());
        with_meta.extend_from_slice(com_payload);
        with_meta.extend_from_slice(&base[2..]);
        with_meta.extend_from_slice(b"SECRET_TRAILING_BYTES");

        let path = dir.path().join("photo.jpg");
        fs::write(&path, &with_meta).unwrap();
        let decoded = image::load_from_memory_with_format(&with_meta, ImageFormat::Jpeg).unwrap();

        let out = vision_jpeg(
            &path,
            ImageFormat::Jpeg,
            &decoded,
            Orientation::NoTransforms,
        )
        .unwrap();
        assert!(!out.windows(10).any(|w| w == b"SECRET_GPS"));
        assert!(!out.windows(13).any(|w| w == b"SECRET_CAMERA"));
        assert!(!out.windows(15).any(|w| w == b"SECRET_TRAILING"));
        // Stripped passthrough matches the original stripped scan (no re-encode at q=85),
        // and decodes to byte-identical RGB pixels!
        let out_decoded = image::load_from_memory_with_format(&out, ImageFormat::Jpeg).unwrap();
        assert_eq!(out_decoded.to_rgb8(), decoded.to_rgb8());
    }
}
