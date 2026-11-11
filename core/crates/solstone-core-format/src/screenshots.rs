// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Screenshots a linked device sends, one to a segment.
//!
//! A screenshot arrives under the `screenshots` source as one image named
//! `screenshot_screen.<ext>`. An accepted type is the bytes exactly as the
//! platform saved them. A type outside png, jpg, jpeg and webp is converted
//! to PNG on the device, keeping its metadata, and then these are the bytes
//! of that PNG. Its capture time is in the envelope's `meta.screenshot`. The `_screen`
//! stem puts the image, and what describe writes beside it
//! (`screenshot_screen.jsonl`), in the screen family, so it is read the way a
//! screen is. This module is the one reading of that contract that needs no
//! image decoder: the name, the metadata and where the screenshot belongs.

use std::fmt;

use chrono::{DateTime, FixedOffset, Timelike};
use serde_json::{Map, Value};

/// The ingest source a screenshots stream is bound to.
pub const SCREENSHOTS_SOURCE: &str = "screenshots";

/// The stem every screenshot image carries.
pub const SCREENSHOT_STEM: &str = "screenshot_screen";

/// The extensions a screenshot may carry, which the journal decodes and can
/// hand to a vision model.
pub const SCREENSHOT_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

/// The segment length a screenshot occupies, in seconds.
pub const SCREENSHOT_SEGMENT_SECONDS: u32 = 1;

/// The `meta` key that carries a screenshot's capture facts.
pub const SCREENSHOT_META_KEY: &str = "screenshot";

/// Whether a stream's name is that of a screenshots stream, for the few labels
/// decided from the name alone. Behaviour is decided by the bound source.
pub fn is_screenshots_stream_name(stream: &str) -> bool {
    stream.ends_with("_screenshots")
}

/// Whether `name` is a screenshot image name the journal accepts.
pub fn is_screenshot_file_name(name: &str) -> bool {
    name.strip_prefix(SCREENSHOT_STEM)
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|ext| SCREENSHOT_EXTENSIONS.contains(&ext))
}

/// Whether `name` claims to be a screenshot image, accepted or not.
pub fn names_screenshot_file(name: &str) -> bool {
    name == SCREENSHOT_STEM || name.starts_with(&format!("{SCREENSHOT_STEM}."))
}

/// Where a screenshot's time came from. Never a model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TakenFrom {
    /// The platform's own record of the screenshot (a photo library's
    /// creation date, a screenshot tool's timestamp).
    Platform,
    /// The file's own embedded date or its modification time.
    File,
}

/// The checked capture facts of one screenshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScreenshotMeta {
    pub captured_at: DateTime<FixedOffset>,
    pub taken_from: TakenFrom,
}

/// Why a screenshot upload breaks the contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScreenshotError(pub String);

impl fmt::Display for ScreenshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ScreenshotError {}

fn invalid(detail: impl Into<String>) -> ScreenshotError {
    ScreenshotError(detail.into())
}

/// The segment key a screenshot taken at `captured_at` belongs in: the day and
/// clock time in the capture's own offset, one second long.
pub fn placement(captured_at: &DateTime<FixedOffset>) -> (String, String) {
    (
        captured_at.format("%Y%m%d").to_string(),
        format!(
            "{:02}{:02}{:02}_{SCREENSHOT_SEGMENT_SECONDS}",
            captured_at.hour(),
            captured_at.minute(),
            captured_at.second()
        ),
    )
}

/// Read and check `meta.screenshot` against the envelope's day and segment.
///
/// `captured_at` is RFC 3339 with an offset (`Z` is `+00:00`), and the
/// envelope's day and segment must be exactly [`placement`] of it, so a
/// screenshot lands at the minute it was taken whatever the device's zone.
pub fn validate_screenshot_meta(
    meta: &Map<String, Value>,
    day: &str,
    segment: &str,
) -> Result<ScreenshotMeta, ScreenshotError> {
    let Some(Value::Object(screenshot)) = meta.get(SCREENSHOT_META_KEY) else {
        return Err(invalid("meta.screenshot is required and must be an object"));
    };
    match screenshot.get("v") {
        Some(Value::Number(v)) if v.as_u64() == Some(1) => {}
        _ => return Err(invalid("meta.screenshot.v must be 1")),
    }
    let Some(Value::String(captured_at)) = screenshot.get("captured_at") else {
        return Err(invalid("meta.screenshot.captured_at is required"));
    };
    let captured_at = DateTime::parse_from_rfc3339(captured_at)
        .map_err(|_| invalid("meta.screenshot.captured_at must be RFC 3339 with an offset"))?;
    let taken_from = match screenshot.get("taken_from") {
        Some(Value::String(value)) if value == "platform" => TakenFrom::Platform,
        Some(Value::String(value)) if value == "file" => TakenFrom::File,
        _ => {
            return Err(invalid(
                "meta.screenshot.taken_from must be platform or file",
            ));
        }
    };
    let (expected_day, expected_segment) = placement(&captured_at);
    if day != expected_day || segment != expected_segment {
        return Err(invalid(format!(
            "day and segment must be {expected_day} {expected_segment}, the capture time in its own offset"
        )));
    }
    Ok(ScreenshotMeta {
        captured_at,
        taken_from,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn meta(value: Value) -> Map<String, Value> {
        let Value::Object(map) = json!({ "screenshot": value }) else {
            unreachable!()
        };
        map
    }

    #[test]
    fn a_screenshot_belongs_at_its_capture_second_in_its_own_offset() {
        let at = DateTime::parse_from_rfc3339("2026-11-10T14:32:07-07:00").unwrap();
        assert_eq!(
            placement(&at),
            ("20261110".to_owned(), "143207_1".to_owned())
        );
        let checked = validate_screenshot_meta(
            &meta(json!({"v": 1, "captured_at": "2026-11-10T14:32:07-07:00", "taken_from": "platform"})),
            "20261110",
            "143207_1",
        )
        .unwrap();
        assert_eq!(checked.taken_from, TakenFrom::Platform);
        // The same instant written in another offset names another second.
        let error = validate_screenshot_meta(
            &meta(json!({"v": 1, "captured_at": "2026-11-10T21:32:07Z", "taken_from": "file"})),
            "20261110",
            "143207_1",
        )
        .unwrap_err();
        assert!(error.0.contains("20261110 213207_1"), "{error}");
    }

    #[test]
    fn capture_facts_must_be_complete() {
        for bad in [
            json!(null),
            json!({"captured_at": "2026-11-10T14:32:07-07:00", "taken_from": "platform"}),
            json!({"v": 2, "captured_at": "2026-11-10T14:32:07-07:00", "taken_from": "platform"}),
            json!({"v": 1, "captured_at": "2026-11-10T14:32:07", "taken_from": "platform"}),
            json!({"v": 1, "captured_at": "2026-11-10T14:32:07-07:00", "taken_from": "model"}),
            json!({"v": 1, "captured_at": "2026-11-10T14:32:07-07:00"}),
        ] {
            assert!(
                validate_screenshot_meta(&meta(bad.clone()), "20261110", "143207_1").is_err(),
                "{bad}"
            );
        }
        assert!(validate_screenshot_meta(&Map::new(), "20261110", "143207_1").is_err());
    }

    #[test]
    fn only_the_screenshot_stem_with_a_decodable_extension_is_accepted() {
        for name in [
            "screenshot_screen.png",
            "screenshot_screen.jpg",
            "screenshot_screen.jpeg",
            "screenshot_screen.webp",
        ] {
            assert!(is_screenshot_file_name(name), "{name}");
            assert!(names_screenshot_file(name), "{name}");
        }
        for name in [
            "screenshot_screen.heic",
            "screenshot_screen.PNG",
            "screenshot_screen",
        ] {
            assert!(!is_screenshot_file_name(name), "{name}");
            assert!(names_screenshot_file(name), "{name}");
        }
        for name in [
            "screen.png",
            "photo.png",
            "screenshot.png",
            "center_screen.webm",
        ] {
            assert!(!names_screenshot_file(name), "{name}");
        }
    }
}
