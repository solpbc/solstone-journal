// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native single-image import source.

use std::fmt;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use base64::Engine;
use chrono::{DateTime, Local};
use image::{DynamicImage, ImageFormat};
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, OneShotClient,
};
use solstone_core_import::{
    CreatedSegment, ImportPreview, ManifestWriteRequest, PublicationOperations, hash_source,
    write_manifest,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, create_directory_with_mode, install_file, segment_path, write_text,
};
use tempfile::NamedTempFile;

const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "gif", "tiff"];
const IMPORT_STREAM: &str = "import.image";
const TRANSCRIPT_FILENAME: &str = "image_transcript.md";
const VISION_PROMPT: &str = "Describe what is in this image faithfully and concisely. Transcribe any legible text verbatim. Return clean markdown.";
const VISION_CONTEXT: &str = "import.image.vision";
const PRIVATE_IMPORT_FILE_MODE: u32 = 0o600;
const PRIVATE_IMPORT_DIR_MODE: u32 = 0o700;
const MODEL_DERIVED_LINE_PREFIX: char = '>';

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
    MissingSource { path: PathBuf },
    SourceNotFile { path: PathBuf },
    UndecodableSource { path: PathBuf, detail: String },
    Install { path: PathBuf, detail: String },
    JournalIo { path: PathBuf, detail: String },
    StreamMarker { day: String, detail: String },
    Manifest { detail: String },
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
        OneShotClient::sibling()?
            .with_prefix_arguments(["generate".into()])
            .execute(request)
    }
}

/// Return whether `path` is an advertised image-source file.
pub fn detect(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                IMAGE_EXTENSIONS
                    .iter()
                    .any(|expected| extension.eq_ignore_ascii_case(expected))
            })
}

/// Preview one image source without writing any journal state.
pub fn preview(path: &Path) -> ImportPreview {
    let Ok((image, format, modified, _)) = read_image(path) else {
        return degenerate_preview();
    };
    let timestamp: DateTime<Local> = modified.into();
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

/// Install, describe, and record one image import segment.
pub fn import_image(
    path: &Path,
    journal_root: &Path,
    import_id: &str,
    mut progress: Option<&mut dyn FnMut(&ProgressUpdate)>,
    publication: &dyn PublicationOperations,
    wire: &dyn WireClient,
) -> Result<ImageImportResult, ImageImportError> {
    let (image, format, modified, source_bytes) = read_image(path)?;
    let (format_name, mime_type) =
        format_details(format).ok_or_else(|| ImageImportError::UndecodableSource {
            path: path.to_path_buf(),
            detail: format!("unsupported image format {format:?}"),
        })?;
    let timestamp: DateTime<Local> = modified.into();
    let day = timestamp.format("%Y%m%d").to_string();
    let segment = format!("{}_0", timestamp.format("%H%M%S"));
    let segment_dir =
        segment_path(journal_root, &day, &segment, IMPORT_STREAM, true).map_err(|error| {
            ImageImportError::JournalIo {
                path: journal_root.to_path_buf(),
                detail: error.to_string(),
            }
        })?;
    create_directory_with_mode(&segment_dir, PRIVATE_IMPORT_DIR_MODE).map_err(|error| {
        ImageImportError::JournalIo {
            path: segment_dir.clone(),
            detail: error.to_string(),
        }
    })?;

    let extension = path
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or_default();
    let original_path = segment_dir.join(format!("original{extension}"));
    install_source(
        path,
        &original_path,
        modified,
        journal_root,
        &day,
        publication,
    )?;

    // Python describes at images.py:113 before it creates the segment at :125;
    // tests/test_importer_images.py:152-166 consequently expect no artifacts on
    // vision failure. Native import installs first so a model outage preserves
    // the owner's ground-truth original and the transcript can explain it.
    let description =
        interpret_generate(wire.execute(&build_generate_request(&source_bytes, mime_type)));

    let transcript_path = segment_dir.join(TRANSCRIPT_FILENAME);
    let title = path.file_stem().unwrap_or_default().to_string_lossy();
    let transcript = render_image_markdown(
        &title,
        format_name,
        image.width(),
        image.height(),
        &timestamp.format("%Y-%m-%d").to_string(),
        &description,
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

    let days_affected = vec![day.clone()];
    let files_created = vec![transcript_path.clone()];
    write_import_manifest(
        path,
        journal_root,
        import_id,
        &days_affected,
        &files_created,
    )?;

    if let Some(callback) = progress.as_mut() {
        callback(&ProgressUpdate {
            current: 1,
            total: 1,
            earliest_date: day.clone(),
            latest_date: day.clone(),
            entities_found: 0,
        });
    }

    Ok(ImageImportResult {
        files_created,
        created_segment: CreatedSegment {
            day,
            segment,
            stream: IMPORT_STREAM.to_owned(),
            hints: Default::default(),
        },
        days_affected,
        description,
    })
}

fn read_image(
    path: &Path,
) -> Result<(DynamicImage, ImageFormat, SystemTime, Vec<u8>), ImageImportError> {
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
    let image =
        image::load_from_memory(&bytes).map_err(|error| ImageImportError::UndecodableSource {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
    Ok((image, format, modified, bytes))
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

fn build_generate_request(source_bytes: &[u8], mime_type: &str) -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: VISION_CONTEXT.to_owned(),
        contents: vec![
            ContentPart::Text {
                text: VISION_PROMPT.to_owned(),
            },
            ContentPart::Image {
                mime_type: mime_type.to_owned(),
                data: base64::engine::general_purpose::STANDARD.encode(source_bytes),
            },
        ],
        system_instruction: None,
        temperature: 0.3,
        max_output_tokens: 16_384,
        thinking_budget: None,
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
        Err(ClientError::Protocol(error)) => DescriptionOutcome::Unavailable {
            reason: format!("{}: {}", error.reason, error.detail),
        },
        Err(ClientError::Decode(detail))
        | Err(ClientError::Io(detail))
        | Err(ClientError::Resolve(detail)) => DescriptionOutcome::Unavailable { reason: detail },
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
    use std::cell::RefCell;
    use std::io::Cursor;

    use image::{ImageBuffer, ImageFormat, Rgb};
    use serde_json::Value;
    use solstone_core_generate::{
        GeneratedResponse, ProtocolError, ReasonCode, ReasonCodeValue, RefusalReason,
        RefusedResponse,
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
    fn request_uses_original_bytes_and_generate_defaults_without_a_process() {
        let source = png_bytes();
        let request = build_generate_request(&source, "image/png");
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
        assert_eq!(request.max_output_tokens, 16_384);
        assert!(request.enforce_responsiveness);
        assert_eq!(request.attempt_index, 0);
        assert!(!request.exclusive_admission);
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
            ClientError::Protocol(ProtocolError {
                id: None,
                reason: "protocol".to_owned(),
                detail: "detail".to_owned(),
            }),
            ClientError::Decode("decode".to_owned()),
            ClientError::Io("io".to_owned()),
            ClientError::Resolve("resolve".to_owned()),
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
        let preview = preview(&path);
        assert_eq!(preview.date_range, (String::new(), String::new()));
        assert_eq!(preview.item_count, 0);
        assert_eq!(preview.entity_count, 0);
        assert_eq!(preview.summary, "No readable image found");
    }
}
