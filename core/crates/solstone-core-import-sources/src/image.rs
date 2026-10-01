// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native single-image import source.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use base64::Engine;
use chrono::{DateTime, NaiveDateTime, Timelike, Utc};
use chrono_tz::Tz;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use solstone_core_depict::resize_for_vlm;
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, OneShotClient,
};
use solstone_core_import::{
    CreatedSegment, ImportPreview, ManifestWriteRequest, PublicationOperations, hash_source,
    write_manifest,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, contained_path, create_directory_with_mode, install_file, path_lexists,
    write_text,
};
use solstone_core_segment::SegmentDir;
use tempfile::NamedTempFile;

/// The formats this source both decodes and can hand to a vision model. HEIC, HEIF and TIFF
/// are deliberately absent: the build carries no decoder for them, so offering them only
/// ends in an undecodable-source failure.
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];
const IMPORT_STREAM: &str = "import.image";
const TRANSCRIPT_FILENAME: &str = "image_transcript.md";
const VISION_PROMPT: &str = "Describe what is in this image faithfully and concisely. Transcribe any legible text verbatim. Return clean markdown.";
const VISION_CONTEXT: &str = "import.image.vision";
const PRIVATE_IMPORT_FILE_MODE: u32 = 0o600;
const PRIVATE_IMPORT_DIR_MODE: u32 = 0o700;
const MODEL_DERIVED_LINE_PREFIX: char = '>';
const IMAGE_SEGMENT_PROBE_LIMIT: u32 = 60;
const REMOVING_SEGMENT_PREFIX: &str = ".removing_";

/// Progress reported by one image import.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProgressUpdate {
    pub current: u64,
    pub total: u64,
    pub earliest_date: String,
    pub latest_date: String,
    pub entities_found: u64,
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
}

impl fmt::Display for ImageImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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

/// Preview one image source without writing any journal state, dated on the
/// owner's day in `zone`.
pub fn preview(path: &Path, zone: Tz) -> ImportPreview {
    let Ok((image, format, modified, _)) = read_image(path) else {
        return degenerate_preview();
    };
    let timestamp = DateTime::<Utc>::from(modified).with_timezone(&zone);
    let Some((format, _)) = format_details(format) else {
        return degenerate_preview();
    };
    ImportPreview {
        date_range: (
            timestamp.format("%Y%m%d").to_string(),
            timestamp.format("%Y%m%d").to_string(),
        ),
        item_count: 1,
        entity_count: 0,
        summary: format!("1 image ({format}, {}×{})", image.width(), image.height()),
    }
}

/// In-memory prepared image import data before publication phase.
#[derive(Debug, Clone)]
pub struct PreparedImage {
    pub path: PathBuf,
    pub image: DynamicImage,
    pub format: ImageFormat,
    pub modified: SystemTime,
    pub description: DescriptionOutcome,
    pub timestamp: DateTime<Tz>,
    pub day: String,
    pub segment: String,
    pub format_name: &'static str,
    pub mime_type: &'static str,
    pub extension: String,
    pub title: String,
}

/// Decode and generate AI description for an image in memory without touching chronicle.
pub fn prepare_image(
    path: &Path,
    wire: &dyn WireClient,
    zone: Tz,
) -> Result<PreparedImage, ImageImportError> {
    let (image, format, modified, orientation) = read_image(path)?;
    let (format_name, mime_type) =
        format_details(format).ok_or_else(|| ImageImportError::UndecodableSource {
            path: path.to_path_buf(),
            detail: format!("unsupported image format {format:?}"),
        })?;
    let timestamp = DateTime::<Utc>::from(modified).with_timezone(&zone);
    let day = timestamp.format("%Y%m%d").to_string();
    let segment = format!("{}_0", timestamp.format("%H%M%S"));
    let extension = path
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or_default();
    let title = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    let description = match vision_png(&image, orientation) {
        Ok(png) => interpret_generate(wire.execute(&build_generate_request(&png))),
        Err(error) => DescriptionOutcome::Unavailable {
            reason: format!("cannot prepare image for vision: {error}"),
        },
    };

    Ok(PreparedImage {
        path: path.to_path_buf(),
        image,
        format,
        modified,
        description,
        timestamp,
        day,
        segment,
        format_name,
        mime_type,
        extension,
        title,
    })
}

struct ClaimedImageSegment {
    day: String,
    segment: String,
    directory: PathBuf,
}

fn claim_image_segment(
    journal_root: &Path,
    start: NaiveDateTime,
    source: &Path,
    extension: &str,
) -> Result<ClaimedImageSegment, ImageImportError> {
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

    let day = start.format("%Y%m%d").to_string();
    let parent = contained_path(journal_root, &format!("chronicle/{day}/{IMPORT_STREAM}"))
        .map_err(|error| ImageImportError::JournalIo {
            path: journal_root.to_path_buf(),
            detail: error.to_string(),
        })?;
    fs::create_dir_all(&parent).map_err(|error| ImageImportError::JournalIo {
        path: parent.clone(),
        detail: error.to_string(),
    })?;

    let expected_original_name = format!("original{extension}");
    let mut cached_source_hash = None;

    for attempt in 0..IMAGE_SEGMENT_PROBE_LIMIT {
        let candidate_time = start + chrono::Duration::seconds(i64::from(attempt));
        if candidate_time.format("%Y%m%d").to_string() != day {
            return Err(ImageImportError::ImageSegmentDayOverflow {
                day,
                stream: IMPORT_STREAM.to_owned(),
                start: start.format("%Y-%m-%dT%H:%M:%S").to_string(),
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
        match fs::create_dir(&candidate_path) {
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
                    if name_str.starts_with("original.") {
                        original_entries.push(entry);
                    }
                }
                if original_entries.len() != 1 {
                    continue;
                }
                let original_entry = &original_entries[0];
                if original_entry.file_name() != expected_original_name.as_str() {
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
            }
            Err(error) => {
                return Err(ImageImportError::JournalIo {
                    path: candidate_path,
                    detail: error.to_string(),
                });
            }
        }

        let resolved =
            SegmentDir::resolve(journal_root, &day, &segment, IMPORT_STREAM).map_err(|error| {
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
        return Ok(ClaimedImageSegment {
            day,
            segment,
            directory: resolved.path().to_path_buf(),
        });
    }

    Err(ImageImportError::ImageSegmentCollision {
        day,
        stream: IMPORT_STREAM.to_owned(),
        start: start.format("%Y-%m-%dT%H:%M:%S").to_string(),
        attempts: IMAGE_SEGMENT_PROBE_LIMIT,
    })
}

/// Install original, transcript, and manifest for a prepared image.
pub fn install_and_publish_image(
    prepared: &PreparedImage,
    journal_root: &Path,
    import_id: &str,
    publication: &dyn PublicationOperations,
    mut progress: Option<&mut dyn FnMut(&ProgressUpdate)>,
) -> Result<ImageImportResult, ImageImportError> {
    let claimed = claim_image_segment(
        journal_root,
        prepared.timestamp.naive_local(),
        &prepared.path,
        &prepared.extension,
    )?;

    let original_path = claimed
        .directory
        .join(format!("original{}", prepared.extension));
    install_source(
        &prepared.path,
        &original_path,
        prepared.modified,
        journal_root,
        &claimed.day,
        publication,
    )?;

    let transcript_path = claimed.directory.join(TRANSCRIPT_FILENAME);
    let transcript_date = format!(
        "{}-{}-{}",
        &claimed.day[0..4],
        &claimed.day[4..6],
        &claimed.day[6..8]
    );
    let transcript = render_image_markdown(
        &prepared.title,
        prepared.format_name,
        prepared.image.width(),
        prepared.image.height(),
        &transcript_date,
        &prepared.description,
    );
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

    let days_affected = vec![claimed.day.clone()];
    let files_created = vec![transcript_path.clone()];
    write_import_manifest(
        &prepared.path,
        journal_root,
        import_id,
        &days_affected,
        &files_created,
    )?;

    if let Some(callback) = progress.as_mut() {
        callback(&ProgressUpdate {
            current: 1,
            total: 1,
            earliest_date: claimed.day.clone(),
            latest_date: claimed.day.clone(),
            entities_found: 0,
        });
    }

    Ok(ImageImportResult {
        files_created,
        created_segment: CreatedSegment {
            day: claimed.day.clone(),
            segment: claimed.segment.clone(),
            stream: IMPORT_STREAM.to_owned(),
            hints: Default::default(),
        },
        days_affected,
        description: prepared.description.clone(),
    })
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
    let prepared = prepare_image(path, wire, zone)?;
    install_and_publish_image(&prepared, journal_root, import_id, publication, progress)
}

fn read_image(
    path: &Path,
) -> Result<(DynamicImage, ImageFormat, SystemTime, Orientation), ImageImportError> {
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
    // An unreadable orientation tag leaves the pixels as stored rather than failing the import.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let image = DynamicImage::from_decoder(decoder).map_err(undecodable)?;
    Ok((image, format, modified, orientation))
}

/// The copy of an imported image that goes to the vision model: decoded pixels only, turned
/// upright, held to the shared vision size bound and re-encoded as PNG. Nothing from the source
/// container rides along, so a photo's embedded location, camera and capture time stay in the
/// owner's journal. The installed original is not touched.
fn vision_png(
    image: &DynamicImage,
    orientation: Orientation,
) -> Result<Vec<u8>, image::ImageError> {
    let mut upright = image.clone();
    upright.apply_orientation(orientation);
    let mut png = Cursor::new(Vec::new());
    resize_for_vlm(upright).write_to(&mut png, ImageFormat::Png)?;
    Ok(png.into_inner())
}

fn degenerate_preview() -> ImportPreview {
    ImportPreview {
        date_range: (String::new(), String::new()),
        item_count: 0,
        entity_count: 0,
        summary: "No readable image found".to_owned(),
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

fn build_generate_request(png: &[u8]) -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: VISION_CONTEXT.to_owned(),
        contents: vec![
            ContentPart::Text {
                text: VISION_PROMPT.to_owned(),
            },
            ContentPart::Image {
                mime_type: "image/png".to_owned(),
                data: base64::engine::general_purpose::STANDARD.encode(png),
            },
        ],
        system_instruction: None,
        temperature: 0.3,
        max_output_tokens: 2_304,
        timeout_s: None,
        json_output: false,
        json_schema: None,
        enforce_responsiveness: true,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}

fn interpret_generate(response: Result<GenerateResponse, ClientError>) -> DescriptionOutcome {
    match response {
        Ok(GenerateResponse::Generated(generated)) => {
            let description = generated.text.trim();
            if description.is_empty() {
                DescriptionOutcome::Unavailable {
                    reason: "Vision produced no description for image".to_owned(),
                }
            } else {
                DescriptionOutcome::Generated(description.to_owned())
            }
        }
        Ok(GenerateResponse::Refused(refusal)) => DescriptionOutcome::Unavailable {
            reason: format!("{}: {}", refusal.reason.as_str(), refusal.detail),
        },
        Err(error @ ClientError::Protocol(_)) => DescriptionOutcome::Unavailable {
            reason: error.to_string(),
        },
        Err(ClientError::Decode(detail) | ClientError::Resolve(detail)) => {
            DescriptionOutcome::Unavailable { reason: detail }
        }
        Err(
            error @ (ClientError::Io { .. }
            | ClientError::ProcessIo(_)
            | ClientError::InvalidResponse(_)
            | ClientError::UnexpectedChild(_)),
        ) => DescriptionOutcome::Unavailable {
            reason: error.to_string(),
        },
    }
}

fn render_image_markdown(
    title: &str,
    format: &str,
    width: u32,
    height: u32,
    date: &str,
    description: &DescriptionOutcome,
) -> String {
    let mut lines = vec![
        format!("# {title}"),
        String::new(),
        "**Type:** Image".to_owned(),
        format!("**Format:** {format}"),
        format!("**Dimensions:** {width}×{height}"),
        format!("**Date:** {date}"),
        String::new(),
        "---".to_owned(),
        String::new(),
    ];
    lines.push(render_model_block(description));
    format!("{}\n", lines.join("\n").trim_end())
}

fn render_model_block(description: &DescriptionOutcome) -> String {
    let text = match description {
        DescriptionOutcome::Generated(text) => text.to_owned(),
        DescriptionOutcome::Unavailable { reason } => format!("unavailable — {reason}"),
    };
    let mut lines = vec![format!(
        "{MODEL_DERIVED_LINE_PREFIX} [image description — model-derived]"
    )];
    lines.extend(text.split('\n').map(|line| {
        if line.is_empty() {
            MODEL_DERIVED_LINE_PREFIX.to_string()
        } else {
            format!("{MODEL_DERIVED_LINE_PREFIX} {line}")
        }
    }));
    lines.join("\n")
}

fn install_source(
    source: &Path,
    destination: &Path,
    modified: SystemTime,
    journal_root: &Path,
    day: &str,
    publication: &dyn PublicationOperations,
) -> Result<(), ImageImportError> {
    let parent = destination
        .parent()
        .expect("image destination has a parent");
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
    File::open(destination)
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
) -> Result<(), ImageImportError> {
    let source_hash = hash_source(source).map_err(|error| ImageImportError::Manifest {
        detail: error.to_string(),
    })?;
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
    })
    .map_err(|error| ImageImportError::Manifest {
        detail: error.to_string(),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::Local;
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
    fn request_carries_the_given_png_and_generate_defaults_without_a_process() {
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
        assert_eq!(mime_type, "image/png");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap(),
            source
        );
        assert_eq!(request.temperature, 0.3);
        assert_eq!(request.max_output_tokens, 2_304);
        assert!(request.enforce_responsiveness);
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

    fn png_chunk_types(png: &[u8]) -> Vec<String> {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let mut types = Vec::new();
        let mut offset = 8;
        while offset < png.len() {
            let length = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
            types.push(String::from_utf8_lossy(&png[offset + 4..offset + 8]).into_owned());
            offset += 12 + length;
        }
        assert_eq!(offset, png.len());
        types
    }

    fn vision_request_bytes(source: &[u8]) -> Vec<u8> {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("IMG_0001.jpg");
        fs::write(&path, source).unwrap();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        let prepared =
            prepare_image(&path, &wire, solstone_core_journal_config::host_zone()).unwrap();
        assert_eq!(prepared.format, ImageFormat::Jpeg);
        assert_eq!(fs::read(&path).unwrap(), source);
        let request = wire.request.borrow_mut().take().unwrap();
        let serialized = format!("{request:?}");
        assert!(!serialized.contains("IMG_0001"));
        assert!(!serialized.contains(&directory.path().display().to_string()));
        let ContentPart::Image { mime_type, data } = &request.contents[1] else {
            panic!("second content part must be an image");
        };
        assert_eq!(mime_type, "image/png");
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap()
    }

    #[test]
    fn imported_photo_reaches_the_model_as_pixels_without_its_exif() {
        for (width, height, orientation, expected) in [
            // Stored sideways with orientation 6: sent upright, same pixel count.
            (40, 24, 6, (24, 40)),
            // No rotation, larger than the shared vision bound: sent resized.
            (3000, 1500, 1, (1920, 960)),
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
            // Structural proof: the PNG holds only a header, pixel data and an end marker,
            // so there is no eXIf, text or colour-profile chunk to carry metadata.
            let chunks = png_chunk_types(&sent);
            assert_eq!(chunks.first().map(String::as_str), Some("IHDR"));
            assert_eq!(chunks.last().map(String::as_str), Some("IEND"));
            assert!(
                chunks[1..chunks.len() - 1]
                    .iter()
                    .all(|kind| kind == "IDAT")
            );
            assert!(!sent.windows(6).any(|window| window == b"Exif\0\0"));
            assert!(!sent.windows(7).any(|window| window == b"TESTCAM"));
            let decoded = image::load_from_memory_with_format(&sent, ImageFormat::Png).unwrap();
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
    fn transcript_has_a_structural_model_partition() {
        let description =
            DescriptionOutcome::Generated("A description.\n\nSecond line.".to_owned());
        let rendered = render_image_markdown("photo", "PNG", 4, 5, "2026-01-02", &description);
        assert_model_partition(&rendered, &description);
        let unavailable = DescriptionOutcome::Unavailable {
            reason: "no engine".to_owned(),
        };
        let rendered = render_image_markdown("photo", "PNG", 4, 5, "2026-01-02", &unavailable);
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
                assert!(model.contains(reason));
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
        let preview = preview(&path, solstone_core_journal_config::host_zone());
        assert_eq!(preview.date_range, (String::new(), String::new()));
        assert_eq!(preview.item_count, 0);
        assert_eq!(preview.entity_count, 0);
        assert_eq!(preview.summary, "No readable image found");
    }

    use chrono::TimeZone;
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

        fn rescan_file(
            &self,
            _journal: &Path,
            _path: &Path,
        ) -> Result<solstone_core_import::publish::RescanFileStatus, String> {
            Ok(solstone_core_import::publish::RescanFileStatus::Declined)
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

    fn write_test_png(path: &Path, pixel: [u8; 3], time: DateTime<Local>) {
        let img = DynamicImage::ImageRgb8(ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 4, Rgb(pixel)));
        img.save_with_format(path, ImageFormat::Png).unwrap();
        let file_time = fs::FileTimes::new().set_modified(time.into());
        File::open(path).unwrap().set_times(file_time).unwrap();
    }

    fn write_test_jpeg(path: &Path, pixel: [u8; 3], time: DateTime<Local>) {
        let img = DynamicImage::ImageRgb8(ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 4, Rgb(pixel)));
        img.save_with_format(path, ImageFormat::Jpeg).unwrap();
        let file_time = fs::FileTimes::new().set_modified(time.into());
        File::open(path).unwrap().set_times(file_time).unwrap();
    }

    #[test]
    fn two_different_images_with_same_mtime_second_take_consecutive_keys_and_preserve_content() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = Local
            .with_ymd_and_hms(2026, 6, 15, 12, 0, 0)
            .single()
            .unwrap();

        let img_a = source_dir.path().join("a.png");
        let img_b = source_dir.path().join("b.png");
        write_test_png(&img_a, [10, 20, 30], time);
        write_test_png(&img_b, [40, 50, 60], time);

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
            serde_json::json!([seg_a_dir.join("image_transcript.md").display().to_string()])
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
            serde_json::json!([seg_b_dir.join("image_transcript.md").display().to_string()])
        );

        assert_eq!(*pub_a.touched_days.borrow(), vec!["20260615"]);
        assert_eq!(*pub_b.touched_days.borrow(), vec!["20260615"]);
    }

    #[test]
    fn three_different_images_take_three_consecutive_keys_and_build_valid_stream_chain() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = Local
            .with_ymd_and_hms(2026, 6, 15, 12, 0, 0)
            .single()
            .unwrap();

        let img_a = source_dir.path().join("a.png");
        let img_b = source_dir.path().join("b.png");
        let img_c = source_dir.path().join("c.png");
        write_test_png(&img_a, [1, 1, 1], time);
        write_test_png(&img_b, [2, 2, 2], time);
        write_test_png(&img_c, [3, 3, 3], time);

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
        let time = Local
            .with_ymd_and_hms(2026, 6, 15, 12, 0, 0)
            .single()
            .unwrap();

        let img_png = source_dir.path().join("pic.png");
        let img_jpg = source_dir.path().join("pic.jpg");
        write_test_png(&img_png, [100, 100, 100], time);
        write_test_jpeg(&img_jpg, [100, 100, 100], time);

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
    fn tombstoned_keys_are_skipped_even_with_matching_original_hash() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = Local
            .with_ymd_and_hms(2026, 6, 15, 12, 0, 0)
            .single()
            .unwrap();

        let img = source_dir.path().join("photo.png");
        write_test_png(&img, [7, 8, 9], time);
        let img_bytes = fs::read(&img).unwrap();

        // 120000_0 has only tombstone.json
        let seg_0 = journal
            .path()
            .join("chronicle/20260615/import.image/120000_0");
        fs::create_dir_all(&seg_0).unwrap();
        fs::write(seg_0.join("tombstone.json"), b"{\"tombstone\":true}").unwrap();

        // 120001_0 has tombstone.json AND matching original.png
        let seg_1 = journal
            .path()
            .join("chronicle/20260615/import.image/120001_0");
        fs::create_dir_all(&seg_1).unwrap();
        fs::write(seg_1.join("tombstone.json"), b"{\"tombstone\":true}").unwrap();
        fs::write(seg_1.join("original.png"), &img_bytes).unwrap();

        let publication = TestPublication::new();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };

        let res =
            import_image(&img, journal.path(), "import-1", None, &publication, &wire).unwrap();
        assert_eq!(res.created_segment.segment, "120002_0");

        // Assert tombstoned state unchanged
        assert_eq!(
            fs::read(seg_0.join("tombstone.json")).unwrap(),
            b"{\"tombstone\":true}"
        );
        assert!(!seg_0.join("original.png").exists());
        assert_eq!(
            fs::read(seg_1.join("tombstone.json")).unwrap(),
            b"{\"tombstone\":true}"
        );
        assert_eq!(fs::read(seg_1.join("original.png")).unwrap(), img_bytes);
    }

    #[test]
    fn same_content_reimport_claims_existing_head_segment_without_creating_new_directory() {
        let journal = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let time = Local
            .with_ymd_and_hms(2026, 6, 15, 12, 0, 0)
            .single()
            .unwrap();

        let img = source_dir.path().join("sample.png");
        write_test_png(&img, [11, 22, 33], time);

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
        let first_time = Local
            .with_ymd_and_hms(2026, 6, 15, 12, 0, 0)
            .single()
            .unwrap();
        let later_time = Local
            .with_ymd_and_hms(2026, 6, 15, 13, 0, 0)
            .single()
            .unwrap();
        let first = source_dir.path().join("first.png");
        let later = source_dir.path().join("later.png");
        write_test_png(&first, [11, 22, 33], first_time);
        write_test_png(&later, [44, 55, 66], later_time);

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

        let segments = fs::read_dir(&stream_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| !name.starts_with('.'))
            .count();
        assert_eq!(segments, 2);
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
        let time = Local
            .with_ymd_and_hms(2026, 6, 15, 12, 0, 0)
            .single()
            .unwrap();

        let img_jpg = source_dir.path().join("photo.jpg");
        let img_jpeg = source_dir.path().join("photo.jpeg");
        write_test_jpeg(&img_jpg, [55, 66, 77], time);
        fs::copy(&img_jpg, &img_jpeg).unwrap();
        File::open(&img_jpeg)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(time.into()))
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
        let time = Local
            .with_ymd_and_hms(2026, 6, 15, 12, 0, 0)
            .single()
            .unwrap();

        // Case a: no original.*
        {
            let journal = tempfile::tempdir().unwrap();
            let source_dir = tempfile::tempdir().unwrap();
            let img = source_dir.path().join("pic.png");
            write_test_png(&img, [1, 2, 3], time);

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
            write_test_png(&img, [1, 2, 3], time);

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
            write_test_png(&img, [1, 2, 3], time);

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
            write_test_png(&img, [1, 2, 3], time);

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
        let time = Local
            .with_ymd_and_hms(2026, 6, 15, 23, 59, 59)
            .single()
            .unwrap();

        let img = source_dir.path().join("photo.png");
        write_test_png(&img, [99, 99, 99], time);

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
        let time = Local
            .with_ymd_and_hms(2026, 6, 15, 12, 0, 0)
            .single()
            .unwrap();

        // Subcase 1: Occupy 0..58 -> claims 59
        {
            let journal = tempfile::tempdir().unwrap();
            let source_dir = tempfile::tempdir().unwrap();
            let img = source_dir.path().join("photo.png");
            write_test_png(&img, [5, 5, 5], time);

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
            write_test_png(&img, [5, 5, 5], time);

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
        let time_end = Local
            .with_ymd_and_hms(2026, 6, 15, 23, 59, 59)
            .single()
            .unwrap();
        let img_1 = source_dir.path().join("end.png");
        write_test_png(&img_1, [1, 2, 3], time_end);

        let publ = TestPublication::new();
        let wire = RecordingWire {
            request: RefCell::new(None),
        };
        let res_1 = import_image(&img_1, journal.path(), "import-1", None, &publ, &wire).unwrap();
        assert_eq!(res_1.created_segment.segment, "235959_0");

        // 2. In clean journal, start at 23:59:30 with 23:59:30..23:59:58 occupied -> takes 235959_0
        let journal_2 = tempfile::tempdir().unwrap();
        let time_30 = Local
            .with_ymd_and_hms(2026, 6, 15, 23, 59, 30)
            .single()
            .unwrap();
        let img_2 = source_dir.path().join("start30.png");
        write_test_png(&img_2, [4, 5, 6], time_30);

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
        let time = Local
            .with_ymd_and_hms(2026, 6, 15, 12, 0, 0)
            .single()
            .unwrap();

        let img = source_dir.path().join("photo.png");
        write_test_png(&img, [12, 34, 56], time);
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
}
