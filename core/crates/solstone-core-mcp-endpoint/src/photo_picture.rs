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
use serde_json::{Value, json};
use solstone_core_journal_io::{JournalRoot, read_relative_file_bounded};

/// The description an image import writes for a photo.
const PHOTO_DESCRIPTION: &str = "image_transcript.md";
/// The folder every image import writes into.
const IMAGE_IMPORT_STREAM: &str = "import.image";
/// The formats an image import keeps (the importer lowercases the extension).
const PHOTO_EXTENSIONS: [&str; 5] = ["jpg", "jpeg", "png", "webp", "gif"];
/// The largest kept photo this will read.
const MAX_PHOTO_SOURCE_BYTES: usize = 64 * 1024 * 1024;
/// The long edge an agent receives. Every client that shows pictures to its
/// model accepts it (VS Code resizes to 768, Claude's standard tier to 1568).
pub(crate) const PICTURE_LONG_EDGE: u32 = 1024;
const PICTURE_JPEG_QUALITY: u8 = 85;

/// A photo's picture for an agent.
pub(crate) struct Picture {
    pub(crate) jpeg: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// The folder of a photo import, when `path` is a photo's description.
fn photo_folder(path: &str) -> Option<&str> {
    let folder = path.strip_suffix(PHOTO_DESCRIPTION)?;
    let mut parts = folder.trim_end_matches('/').rsplit('/');
    let _segment = parts.next().filter(|segment| !segment.is_empty())?;
    (parts.next()? == IMAGE_IMPORT_STREAM).then_some(folder)
}

/// Whether `path` is an imported photo's description.
pub(crate) fn is_photo_description(path: &str) -> bool {
    photo_folder(path).is_some()
}

/// The `picture` field a photo's fetch carries, and the picture when it is kept.
pub(crate) fn photo_picture(
    journal_root: &Path,
    description_path: &str,
) -> (Value, Option<Picture>) {
    match read_photo(journal_root, description_path).and_then(|bytes| bounded_picture(&bytes)) {
        Some(picture) => (
            json!({"state": "attached", "width": picture.width, "height": picture.height}),
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

/// Decode, turn upright, shrink to the long edge and encode as a fresh JPEG.
/// Nothing but pixels is carried over.
pub(crate) fn bounded_picture(bytes: &[u8]) -> Option<Picture> {
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
    if image.width().max(image.height()) > PICTURE_LONG_EDGE {
        image = image.resize(
            PICTURE_LONG_EDGE,
            PICTURE_LONG_EDGE,
            image::imageops::FilterType::Triangle,
        );
    }
    let rgb = image.to_rgb8();
    let (width, height) = rgb.dimensions();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, PICTURE_JPEG_QUALITY)
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
        let picture = bounded_picture(&encoded(4000, 3000, image::ImageFormat::Png)).unwrap();
        assert_eq!((picture.width, picture.height), (1024, 768));
        assert_eq!(
            image::guess_format(&picture.jpeg).unwrap(),
            image::ImageFormat::Jpeg
        );
    }

    #[test]
    fn a_small_photo_is_never_enlarged() {
        let picture = bounded_picture(&encoded(300, 500, image::ImageFormat::WebP)).unwrap();
        assert_eq!((picture.width, picture.height), (300, 500));
    }

    #[test]
    fn what_is_not_a_picture_gets_none() {
        assert!(bounded_picture(b"not an image").is_none());
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

        let (state, picture) = photo_picture(journal.path(), description);
        assert_eq!(state["state"], "not_kept");
        assert!(picture.is_none());

        std::fs::write(
            folder.join("original.jpg"),
            encoded(2048, 1024, image::ImageFormat::Jpeg),
        )
        .unwrap();
        let (state, picture) = photo_picture(journal.path(), description);
        assert_eq!(state["state"], "attached");
        assert_eq!(
            (state["width"].as_u64(), state["height"].as_u64()),
            (Some(1024), Some(512))
        );
        assert!(picture.is_some());
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
        );
        assert_eq!(state["state"], "not_kept");
    }
}
