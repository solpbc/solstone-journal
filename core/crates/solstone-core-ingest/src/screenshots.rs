// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Screenshots from a linked device, and the source's memory of what it took in.
//!
//! An upload under the `screenshots` source carries one image, exactly as the
//! platform saved it, and its capture time. It lands as an ordinary segment at
//! that second in the device's stream, so the segment's trash takes all of it.
//!
//! The source keeps its own record of every screenshot it has received, keyed
//! by the SHA-256 of the image bytes, in `config/screenshots/received/`. The
//! record is the source's business, not the trash's: it is never consulted on
//! delete, holds no list of deletes, and survives a segment's erase. It is how
//! one screenshot stays one memory across the owner's devices (iCloud Photos
//! and synced folders hand the same bytes to each), and how a sync (a resend,
//! a re-scan, a second device, a turn-back-on) never brings back a screenshot
//! the owner deleted. Something the owner brings in through the Import page
//! does not come through this source, so it arrives as new data.
//!
//! A record that can't be read refuses the upload rather than reading as
//! empty: an empty reading would bring back every screenshot the owner
//! deleted.

use std::io::{self, Cursor};
use std::path::{Path, PathBuf};

use image::{ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use solstone_core_format::screenshots::{
    SCREENSHOTS_SOURCE, ScreenshotMeta, is_screenshot_file_name, names_screenshot_file,
    validate_screenshot_meta,
};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, create_directory_with_mode};

use crate::model::{IncomingFile, ReasonCode};

const RECEIVED_DIR: &str = "config/screenshots/received";

/// The most pixels a screenshot may hold. A long scrolling capture on a phone
/// is tall but well under this; a file over it is refused before anything
/// decodes it whole.
pub(crate) const MAX_SCREENSHOT_PIXELS: u64 = 100_000_000;

const PRIVATE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };

/// A screenshot upload that passed every rule.
#[derive(Clone, Debug)]
pub(crate) struct CheckedScreenshot {
    /// SHA-256 of the image bytes, lowercase hex: the screenshot's identity
    /// across the owner's devices.
    pub(crate) id: String,
    #[allow(dead_code)]
    pub(crate) meta: ScreenshotMeta,
}

/// Where the source first kept a screenshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Received {
    pub(crate) day: String,
    pub(crate) segment: String,
    pub(crate) received_at: String,
}

fn invalid(detail: impl Into<String>) -> (ReasonCode, String) {
    (ReasonCode::ScreenshotInvalid, detail.into())
}

/// Screenshot images arrive only under the screenshots source, one to an
/// upload with its capture facts, and that source carries nothing else.
pub(crate) fn validate_screenshot_upload(
    source: &str,
    day: &str,
    segment: &str,
    meta: &Map<String, Value>,
    files: &[IncomingFile],
) -> Result<Option<CheckedScreenshot>, (ReasonCode, String)> {
    if source != SCREENSHOTS_SOURCE {
        if files
            .iter()
            .any(|file| names_screenshot_file(&file.submitted))
        {
            return Err(invalid(
                "screenshot files are accepted only from the screenshots source",
            ));
        }
        return Ok(None);
    }
    let [file] = files else {
        return Err(invalid(
            "the screenshots source carries one screenshot per upload",
        ));
    };
    if !is_screenshot_file_name(&file.submitted) {
        return Err(invalid(
            "the screenshots source carries only screenshot_screen.png, .jpg, .jpeg or .webp",
        ));
    }
    check_image(&file.submitted, &file.bytes)?;
    let meta = validate_screenshot_meta(meta, day, segment).map_err(|error| invalid(error.0))?;
    Ok(Some(CheckedScreenshot {
        id: format!("{:x}", Sha256::digest(&file.bytes)),
        meta,
    }))
}

/// The bytes are an image of the kind the name says, with readable
/// dimensions inside the pixel limit. Only the header is read.
fn check_image(name: &str, bytes: &[u8]) -> Result<(), (ReasonCode, String)> {
    let expected = match name.rsplit_once('.').map(|(_, ext)| ext) {
        Some("png") => ImageFormat::Png,
        Some("jpg" | "jpeg") => ImageFormat::Jpeg,
        Some("webp") => ImageFormat::WebP,
        _ => return Err(invalid("unsupported screenshot type")),
    };
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| invalid("the screenshot can't be read"))?;
    if reader.format() != Some(expected) {
        return Err(invalid("the screenshot's bytes don't match its extension"));
    }
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| invalid("the screenshot can't be read"))?;
    if width == 0 || height == 0 {
        return Err(invalid("the screenshot has no pixels"));
    }
    if u64::from(width) * u64::from(height) > MAX_SCREENSHOT_PIXELS {
        return Err(invalid(format!(
            "the screenshot is {width}x{height}, over {MAX_SCREENSHOT_PIXELS} pixels"
        )));
    }
    Ok(())
}

fn received_path(journal: &Path, id: &str) -> PathBuf {
    journal
        .join(RECEIVED_DIR)
        .join(&id[..2])
        .join(format!("{id}.json"))
}

/// What the source remembers about this screenshot, if it has received it.
/// Anything but a clean "not received" or a readable record is an error, so
/// the caller refuses rather than taking it in a second time.
pub(crate) fn lookup(journal: &Path, id: &str) -> io::Result<Option<Received>> {
    match std::fs::read(received_path(journal, id)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // A missing shard directory is a clean miss too; anything else
            // unreadable on the way is not.
            match std::fs::symlink_metadata(journal.join(RECEIVED_DIR)) {
                Ok(metadata) if metadata.is_dir() => Ok(None),
                Ok(_) => Err(io::Error::other("received record is not a directory")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

/// Remember that this screenshot is in the journal, at its first place.
pub(crate) fn record(journal: &Path, id: &str, received: &Received) -> io::Result<()> {
    let path = received_path(journal, id);
    if let Some(parent) = path.parent() {
        for dir in [parent.parent().unwrap_or(parent), parent] {
            create_directory_with_mode(dir, 0o700).map_err(io::Error::other)?;
        }
    }
    let mut bytes = serde_json::to_vec(received).map_err(io::Error::other)?;
    bytes.push(b'\n');
    atomic_replace(&path, &bytes, PRIVATE).map_err(io::Error::other)?;
    Ok(())
}

#[cfg(test)]
#[path = "screenshots_tests.rs"]
mod tests;
