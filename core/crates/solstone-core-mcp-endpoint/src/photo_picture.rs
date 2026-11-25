// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The picture behind an imported photo's description, as an agent receives it.
//!
//! A connection that may read a photo's description may receive the photo
//! itself: the photos kind admits both. The journal's own copy sits beside the
//! description in the same import folder. An agent gets a smaller copy, turned
//! the right way up and encoded afresh, so no camera details or location leave
//! with it. A screenshot's description sits under screen and gets no picture here.

use std::io::Cursor;
use std::path::Path;

use image::{DynamicImage, ImageDecoder, ImageReader};
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_journal_io::{JournalRoot, read_relative_file_bounded};

/// The description an image import writes for a photo.
const PHOTO_DESCRIPTION: &str = "image_transcript.md";
/// The optional provenance sidecar beside an imported photo's description.
const IMAGE_META_FILENAME: &str = "image_meta.json";
/// The folder every image import writes into.
const IMAGE_IMPORT_STREAM: &str = "import.image";
/// The formats an image import keeps (the importer lowercases the extension).
const PHOTO_EXTENSIONS: [&str; 5] = ["jpg", "jpeg", "png", "webp", "gif"];
/// The largest kept photo this will read.
const MAX_PHOTO_SOURCE_BYTES: usize = 64 * 1024 * 1024;
/// Bound on reading sibling `image_meta.json`.
const MAX_IMAGE_META_BYTES: usize = 64 * 1024;
/// The long edge an agent receives at full size. Every client that shows
/// pictures to its model accepts it (VS Code resizes to 768, Claude's standard
/// tier to 1568).
pub(crate) const PICTURE_LONG_EDGE: u32 = 1024;
const PICTURE_JPEG_QUALITY: u8 = 85;
/// The long edge an agent receives when requesting a thumbnail for grids,
/// contact sheets or multi-photo batches. Above Claude's 200 px vision floor
/// and crisp on Retina contact-sheet tiles while ~10x smaller than full.
pub(crate) const THUMB_LONG_EDGE: u32 = 320;
const THUMB_JPEG_QUALITY: u8 = 78;

/// Requested output size for fetched photos and screenshots.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PictureSize {
    #[default]
    Full,
    Thumb,
}

impl PictureSize {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Thumb => "thumb",
        }
    }

    pub(crate) const fn long_edge(self) -> u32 {
        match self {
            Self::Full => PICTURE_LONG_EDGE,
            Self::Thumb => THUMB_LONG_EDGE,
        }
    }

    pub(crate) const fn jpeg_quality(self) -> u8 {
        match self {
            Self::Full => PICTURE_JPEG_QUALITY,
            Self::Thumb => THUMB_JPEG_QUALITY,
        }
    }
}

/// A photo's picture for an agent.
pub(crate) struct Picture {
    pub(crate) jpeg: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Clock time and provenance for an imported photo.
///
/// When `image_meta.json` records `time_source: "upload_fallback"`, no reliable
/// time of taking was present on the photo (its segment sits at import time),
/// so `time` is omitted rather than presenting the upload clock as when the
/// photo was taken.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PhotoClockInfo {
    pub(crate) time: Option<String>,
    pub(crate) time_source: Option<&'static str>,
}

/// Result of extracting a clean preview directly from a verified
/// `image_transcript.md` document on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PhotoDocumentPreview {
    /// Clean decoded description or caption from a structured image transcript.
    Description(String),
    /// Structured image transcript whose description is unavailable and has no
    /// caption.
    Empty,
    /// Unstructured/legacy markdown without the standard image transcript
    /// markers; fall back to chunk-based preview extraction.
    Unstructured,
}

/// The folder of a photo import or synced library photo, when `path` is a photo's description.
fn photo_folder(path: &str) -> Option<&str> {
    let folder = path.strip_suffix(PHOTO_DESCRIPTION)?;
    let mut parts = folder.trim_end_matches('/').rsplit('/');
    let _segment = parts.next().filter(|segment| !segment.is_empty())?;
    matches!(parts.next()?, IMAGE_IMPORT_STREAM | "photos").then_some(folder)
}

/// Whether `path` is an imported photo's description.
pub(crate) fn is_photo_description(path: &str) -> bool {
    photo_folder(path).is_some()
}

/// Undo the importer's literal encoding inside its model block. Fetch will
/// escape the whole value at release, including the header, caption and block
/// markers. Leaving the model text encoded at that point would escape it twice.
pub(crate) fn decode_model_text(text: &str) -> Option<String> {
    const MARKER: &str = "\n> [image description — model-derived]\n";
    let (header, tail) = text.rsplit_once(MARKER)?;
    let mut decoded = String::with_capacity(text.len());
    decoded.push_str(header);
    decoded.push_str(MARKER);
    let mut in_model = true;
    for line in tail.split_inclusive('\n') {
        if in_model && let Some(body) = line.strip_prefix("> ") {
            decoded.push_str("> ");
            let mut chars = body.chars().peekable();
            let mut plain = String::with_capacity(body.len());
            while let Some(ch) = chars.next() {
                if ch == '\\' && chars.peek().is_some_and(char::is_ascii_punctuation) {
                    plain.push(chars.next().expect("escaped punctuation"));
                } else {
                    plain.push(ch);
                }
            }
            decoded.push_str(
                &plain
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&amp;", "&"),
            );
        } else if line == "> [visible text — model-derived]\n"
            || line == "> [visible text — model-derived]"
        {
            in_model = true;
            decoded.push_str(line);
        } else {
            in_model &= line == ">\n" || line == "\n";
            decoded.push_str(line);
        }
    }
    Some(decoded)
}

/// Clock time (`HH:MM:SS`) and `time_source` for an imported photo description
/// path. Omits `time` when `image_meta.json` records `upload_fallback` so an
/// import timestamp is never presented as when the photo was taken.
pub(crate) fn photo_clock_info(journal_root: &Path, description_path: &str) -> PhotoClockInfo {
    let Some(folder) = photo_folder(description_path) else {
        return PhotoClockInfo::default();
    };
    let clock = solstone_core_format::segment::segment_parse(description_path)
        .map(|times| format!("{:02}:{:02}:{:02}", times.hour, times.minute, times.second));
    let time_source = read_image_meta_time_source(journal_root, folder);
    match time_source {
        Some("upload_fallback") => PhotoClockInfo {
            time: None,
            time_source: Some("upload_fallback"),
        },
        source => PhotoClockInfo {
            time: clock,
            time_source: source,
        },
    }
}

fn read_image_meta_time_source(journal_root: &Path, folder: &str) -> Option<&'static str> {
    let root = JournalRoot::open(journal_root).ok()?;
    let prefixes: &[&str] = if folder.starts_with("chronicle/") {
        &[""]
    } else {
        &["chronicle/", ""]
    };
    for prefix in prefixes {
        let relative = format!("{prefix}{folder}{IMAGE_META_FILENAME}");
        if let Ok(Some(observation)) =
            read_relative_file_bounded(&root, Path::new(&relative), MAX_IMAGE_META_BYTES)
        {
            let value: Value = serde_json::from_slice(&observation.bytes).ok()?;
            return match value.get("time_source").and_then(Value::as_str) {
                Some("capture") => Some("capture"),
                Some("owner") => Some("owner"),
                Some("upload_fallback") => Some("upload_fallback"),
                _ => None,
            };
        }
    }
    None
}

/// Extract the clean decoded preview (model description, or caption when the
/// model description is unavailable) from a verified `image_transcript.md`.
pub(crate) fn photo_preview_from_document(
    full_text: &str,
    max_chars: usize,
) -> PhotoDocumentPreview {
    const MARKER: &str = "\n> [image description — model-derived]\n";
    let Some(decoded) = decode_model_text(full_text) else {
        return PhotoDocumentPreview::Unstructured;
    };
    let Some((header, tail)) = decoded.rsplit_once(MARKER) else {
        return PhotoDocumentPreview::Unstructured;
    };
    let mut lines = Vec::new();
    for line in tail.lines() {
        if let Some(body) = line.strip_prefix("> ") {
            lines.push(body);
        } else if line == ">" {
            lines.push("");
        } else {
            break;
        }
    }
    let body = lines.join("\n");
    let body = body.trim();
    if !body.is_empty()
        && body != "unavailable"
        && !body.starts_with("unavailable —")
        && !body.starts_with("unavailable -")
    {
        return PhotoDocumentPreview::Description(body.chars().take(max_chars).collect());
    }
    if let Some(caption) = extract_caption_block(header) {
        return PhotoDocumentPreview::Description(caption.chars().take(max_chars).collect());
    }
    PhotoDocumentPreview::Empty
}

fn extract_caption_block(header: &str) -> Option<String> {
    let (_, after) = header.split_once("\n**Caption:**\n")?;
    let mut lines = Vec::new();
    for line in after.lines() {
        if line.trim() == "---" {
            break;
        }
        lines.push(line);
    }
    let caption = lines.join("\n");
    let caption = caption.trim();
    (!caption.is_empty()).then(|| caption.to_owned())
}

/// Repair stray spaces inserted before escaped punctuation when falling back to
/// indexed Markdown chunks (when the source `image_transcript.md` cannot be
/// verified from disk).
pub(crate) fn repair_chunked_punctuation(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ' '
            && i > 0
            && !chars[i - 1].is_whitespace()
            && let Some(&next) = chars.get(i + 1)
        {
            let drop_before_punct = matches!(next, ',' | '.' | ':' | ';' | '!' | '?' | ')' | ']');
            let drop_before_hyphen = next == '-'
                && chars[i - 1].is_alphanumeric()
                && chars.get(i + 2).is_some_and(|ch| ch.is_alphanumeric());
            let drop_before_apostrophe = next == '\''
                && chars[i - 1].is_alphanumeric()
                && chars
                    .get(i + 2)
                    .is_none_or(|ch| ch.is_alphanumeric() || ch.is_whitespace());
            if drop_before_punct || drop_before_hyphen || drop_before_apostrophe {
                i += 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// The `picture` field a photo's fetch carries, and the picture when it is kept.
pub(crate) fn photo_picture(
    journal_root: &Path,
    description_path: &str,
    size: PictureSize,
) -> (Value, Option<Picture>) {
    match read_photo(journal_root, description_path).and_then(|bytes| bounded_picture(&bytes, size))
    {
        Some(picture) => (
            json!({
                "state": "attached",
                "size": size.as_str(),
                "width": picture.width,
                "height": picture.height,
            }),
            Some(picture),
        ),
        None => (json!({"state": "not_kept"}), None),
    }
}

fn read_photo(journal_root: &Path, description_path: &str) -> Option<Vec<u8>> {
    let folder = photo_folder(description_path)?;
    let root = JournalRoot::open(journal_root).ok()?;
    // An indexed path is day-rooted; the files live under chronicle/.
    let prefixes: &[&str] = if folder.starts_with("chronicle/") {
        &[""]
    } else {
        &["chronicle/", ""]
    };
    for prefix in prefixes {
        for extension in PHOTO_EXTENSIONS {
            let relative = format!("{prefix}{folder}original.{extension}");
            if let Ok(Some(observation)) =
                read_relative_file_bounded(&root, Path::new(&relative), MAX_PHOTO_SOURCE_BYTES)
            {
                return Some(observation.bytes);
            }
        }
    }
    None
}

/// Decode, turn upright, shrink to the requested long edge and encode as a
/// fresh JPEG. Nothing but pixels is carried over.
pub(crate) fn bounded_picture(bytes: &[u8], size: PictureSize) -> Option<Picture> {
    let mut decoder = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    encode_picture(image, size)
}

pub(crate) fn encode_picture(mut image: DynamicImage, size: PictureSize) -> Option<Picture> {
    let long_edge = size.long_edge();
    if image.width().max(image.height()) > long_edge {
        image = image.resize(long_edge, long_edge, image::imageops::FilterType::Triangle);
    }
    let rgb = image.to_rgb8();
    let (width, height) = rgb.dimensions();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, size.jpeg_quality())
        .encode_image(&rgb)
        .ok()?;
    Some(Picture {
        jpeg,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
        let image = image::RgbImage::from_pixel(width, height, image::Rgb([200, 120, 0]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    #[test]
    fn only_a_photo_description_has_a_picture() {
        assert!(is_photo_description(
            "20261118/import.image/120000_0/image_transcript.md"
        ));
        assert!(is_photo_description(
            "chronicle/20261118/import.image/120000_0/image_transcript.md"
        ));
        assert!(!is_photo_description(
            "20261118/import.image/120000_0/screenshot_transcript.md"
        ));
        assert!(!is_photo_description(
            "20261118/import.document/120000_0/image_transcript.md"
        ));
        assert!(!is_photo_description(
            "20261118/import.image/image_transcript.md"
        ));
    }

    #[test]
    fn a_large_photo_arrives_at_the_long_edge_as_jpeg() {
        let raw = encoded(4000, 3000, image::ImageFormat::Png);
        let full = bounded_picture(&raw, PictureSize::Full).unwrap();
        assert_eq!((full.width, full.height), (1024, 768));
        assert_eq!(
            image::guess_format(&full.jpeg).unwrap(),
            image::ImageFormat::Jpeg
        );

        let thumb = bounded_picture(&raw, PictureSize::Thumb).unwrap();
        assert_eq!((thumb.width, thumb.height), (320, 240));
        assert_eq!(
            image::guess_format(&thumb.jpeg).unwrap(),
            image::ImageFormat::Jpeg
        );
        assert!(thumb.jpeg.len() < full.jpeg.len());
    }

    #[test]
    fn a_small_photo_is_never_enlarged() {
        let full = bounded_picture(
            &encoded(300, 500, image::ImageFormat::WebP),
            PictureSize::Full,
        )
        .unwrap();
        assert_eq!((full.width, full.height), (300, 500));

        let tiny = bounded_picture(
            &encoded(120, 90, image::ImageFormat::WebP),
            PictureSize::Thumb,
        )
        .unwrap();
        assert_eq!((tiny.width, tiny.height), (120, 90));
    }

    #[test]
    fn what_is_not_a_picture_gets_none() {
        assert!(bounded_picture(b"not an image", PictureSize::Full).is_none());
        assert!(bounded_picture(b"not an image", PictureSize::Thumb).is_none());
    }

    #[test]
    fn a_kept_photo_is_attached_and_a_missing_one_is_not_kept() {
        let journal = tempfile::tempdir().unwrap();
        let folder = journal
            .path()
            .join("chronicle/20261118/import.image/120000_0");
        std::fs::create_dir_all(&folder).unwrap();
        let description = "20261118/import.image/120000_0/image_transcript.md";
        std::fs::write(folder.join("image_transcript.md"), "a red barn").unwrap();

        let (state, picture) = photo_picture(journal.path(), description, PictureSize::Full);
        assert_eq!(state["state"], "not_kept");
        assert!(picture.is_none());

        std::fs::write(
            folder.join("original.jpg"),
            encoded(2048, 1024, image::ImageFormat::Jpeg),
        )
        .unwrap();
        let (state, picture) = photo_picture(journal.path(), description, PictureSize::Full);
        assert_eq!(state["state"], "attached");
        assert_eq!(state["size"], "full");
        assert_eq!(
            (state["width"].as_u64(), state["height"].as_u64()),
            (Some(1024), Some(512))
        );
        assert!(picture.is_some());

        let (thumb_state, thumb_picture) =
            photo_picture(journal.path(), description, PictureSize::Thumb);
        assert_eq!(thumb_state["state"], "attached");
        assert_eq!(thumb_state["size"], "thumb");
        assert_eq!(
            (
                thumb_state["width"].as_u64(),
                thumb_state["height"].as_u64()
            ),
            (Some(320), Some(160))
        );
        assert!(thumb_picture.is_some());
    }

    #[test]
    fn a_screenshot_original_beside_it_is_never_sent() {
        let journal = tempfile::tempdir().unwrap();
        let folder = journal
            .path()
            .join("chronicle/20261118/import.image/120000_0");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("original.screenshot.png"),
            encoded(64, 64, image::ImageFormat::Png),
        )
        .unwrap();
        let (state, _) = photo_picture(
            journal.path(),
            "20261118/import.image/120000_0/image_transcript.md",
            PictureSize::Full,
        );
        assert_eq!(state["state"], "not_kept");
    }

    #[test]
    fn photo_clock_info_honors_time_source_and_omits_upload_fallback_time() {
        let journal = tempfile::tempdir().unwrap();
        let make = |seg: &str, meta: Option<&str>| {
            let dir = journal
                .path()
                .join(format!("chronicle/20251022/import.image/{seg}"));
            std::fs::create_dir_all(&dir).unwrap();
            if let Some(meta) = meta {
                std::fs::write(dir.join("image_meta.json"), meta).unwrap();
            }
            format!("20251022/import.image/{seg}/image_transcript.md")
        };

        let capture = make(
            "061500_0",
            Some(r#"{"schema":1,"time_source":"capture","utc_offset_seconds":3600}"#),
        );
        assert_eq!(
            photo_clock_info(journal.path(), &capture),
            PhotoClockInfo {
                time: Some("06:15:00".to_owned()),
                time_source: Some("capture"),
            }
        );

        let owner = make("121530_0", Some(r#"{"schema":1,"time_source":"owner"}"#));
        assert_eq!(
            photo_clock_info(journal.path(), &owner),
            PhotoClockInfo {
                time: Some("12:15:30".to_owned()),
                time_source: Some("owner"),
            }
        );

        let fallback = make(
            "184500_0",
            Some(r#"{"schema":1,"time_source":"upload_fallback"}"#),
        );
        assert_eq!(
            photo_clock_info(journal.path(), &fallback),
            PhotoClockInfo {
                time: None,
                time_source: Some("upload_fallback"),
            }
        );

        let legacy = make("215200_0", None);
        assert_eq!(
            photo_clock_info(journal.path(), &legacy),
            PhotoClockInfo {
                time: Some("21:52:00".to_owned()),
                time_source: None,
            }
        );
    }

    #[test]
    fn photo_preview_from_document_decodes_escaped_punctuation_and_falls_back_to_caption() {
        let doc = concat!(
            "# The desk at first light\n\n",
            "**Type:** Image\n**Format:** JPEG\n**Dimensions:** 1200×896\n**Date:** 2025-01-01\n\n",
            "---\n\n",
            "> [image description — model-derived]\n",
            "> The desk at first light\\: the account book open at a fresh leaf\\, one line entered\\, pounce\\-box and other men\\'s shoulders\\.\n",
        );
        assert_eq!(
            photo_preview_from_document(doc, 800),
            PhotoDocumentPreview::Description(
                "The desk at first light: the account book open at a fresh leaf, one line entered, pounce-box and other men's shoulders.".to_owned()
            )
        );

        let caption_only = concat!(
            "# Captured\n\n**Type:** Image\n\n---\n\n**Caption:**\nA handwritten caption\n\n---\n\n",
            "> [image description — model-derived]\n> unavailable — No active provider configured\n",
        );
        assert_eq!(
            photo_preview_from_document(caption_only, 800),
            PhotoDocumentPreview::Description("A handwritten caption".to_owned())
        );

        let unavailable = concat!(
            "# Untitled\n\n**Type:** Image\n\n---\n\n",
            "> [image description — model-derived]\n> unavailable — No active provider configured\n",
        );
        assert_eq!(
            photo_preview_from_document(unavailable, 800),
            PhotoDocumentPreview::Empty
        );

        assert_eq!(
            photo_preview_from_document("a plum tree in bloom", 800),
            PhotoDocumentPreview::Unstructured
        );
    }

    #[test]
    fn repair_chunked_punctuation_cleans_escaped_split_spaces() {
        assert_eq!(
            repair_chunked_punctuation(
                "The tilt , from behind the barrier and other men 's shoulders . pounce -box — plain - dash ."
            ),
            "The tilt, from behind the barrier and other men's shoulders. pounce-box — plain - dash."
        );
    }
}
