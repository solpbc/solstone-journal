// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Canonical per-photo source-bound observation record (`photo_observation.json`).
//!
//! Every photo kept in the journal — whether hand-shared, imported, or synced
//! from a photo library — gets one structured observation sidecar bound to its
//! source segment, pixel file selector, and content digest. Downstream readers
//! (search previews, dashboard, experiences, entity face links, and future
//! activity visual evidence) resolve through [`resolve_photo_observation`] so
//! trashed or replaced photos stop resolving and restored photos resolve again.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_format::content::{IMAGE_META_FILENAME, PHOTO_OBSERVATION_FILENAME};
use solstone_core_journal_io::durability::{ArtifactId, DurableObservation, observe_json_durable};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, contained_path, read_bytes};

use crate::segment_dir::{OwnerDeletionState, is_safe_stream_component, owner_deletion_state};

/// Current schema version for `photo_observation.json`.
pub const PHOTO_OBSERVATION_SCHEMA: u32 = 1;

/// Private file mode for photo observation sidecars.
const PRIVATE_FILE_MODE: u32 = 0o600;

/// Supported photo original file extensions.
const PHOTO_ORIGINAL_EXTENSIONS: [&str; 5] = ["jpg", "jpeg", "png", "webp", "gif"];

/// Source identity binding an observation to one kept photo in the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoObservationSource {
    pub day: String,
    pub stream: String,
    pub segment: String,
    /// Exact filename inside the segment directory (`original.jpg`, `original.png`, etc.).
    pub artifact_selector: String,
    /// SHA-256 hex of the pixel file bytes, used to detect in-place replacement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sha256: Option<String>,
    /// Stable library asset identifier when synced from a photo library.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<String>,
    /// Import record ID when created through the importer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_id: Option<String>,
    /// Original filename supplied by the source, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_filename: Option<String>,
}

/// Resolution of a photo's timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhotoTimePrecision {
    Second,
    Minute,
    Day,
    Unknown,
}

/// Specific provenance of a photo's timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhotoTimeProvenance {
    Exif,
    ClientCapturedAt,
    Owner,
    UploadFallback,
    LibraryMetadata,
}

/// Capture and placement timing for a photo observation.
///
/// `time_source` stays exact (`"capture" | "upload_fallback" | "owner"`) so
/// existing gates (such as GPS sighting trust and clock omission on
/// `upload_fallback`) keep working identically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoObservationTime {
    /// True capture instant before any same-second segment collision bump.
    /// `None` when `time_source == "upload_fallback"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
    /// Segment placement wall-clock (`YYYY-MM-DDTHH:MM:SS`).
    pub placed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utc_offset_seconds: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    pub precision: PhotoTimePrecision,
    pub provenance: PhotoTimeProvenance,
    /// Canonical compatibility tag: `"capture" | "upload_fallback" | "owner"`.
    pub time_source: String,
}

/// Processing state of a photo observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhotoObservationState {
    Ready,
    Pending,
    ThinkingOff,
    TransientFailure,
    AnalysisUnavailable,
}

impl PhotoObservationState {
    /// Whether this observation should be retried while the journal still holds pixels.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::Pending | Self::TransientFailure | Self::ThinkingOff
        )
    }

    /// Dashboard-compatible description state label.
    #[must_use]
    pub const fn dashboard_state(self) -> &'static str {
        match self {
            Self::Ready => "described",
            Self::Pending => "pending",
            Self::ThinkingOff => "no_processing",
            Self::TransientFailure => "transient_failure",
            Self::AnalysisUnavailable => "unavailable",
        }
    }
}

/// Belonging classification returned in the single vision read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BelongingClass {
    LivedMoment,
    ReferenceToRemember,
    ErrandOrUtilitarian,
    NoiseOrAccidental,
}

impl BelongingClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LivedMoment => "lived_moment",
            Self::ReferenceToRemember => "reference_to_remember",
            Self::ErrandOrUtilitarian => "errand_or_utilitarian",
            Self::NoiseOrAccidental => "noise_or_accidental",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "lived_moment" => Some(Self::LivedMoment),
            "reference_to_remember" => Some(Self::ReferenceToRemember),
            "errand_or_utilitarian" => Some(Self::ErrandOrUtilitarian),
            "noise_or_accidental" => Some(Self::NoiseOrAccidental),
            _ => None,
        }
    }

    /// Whether the photo is noise or accidental (used by photo library sync to
    /// drop noise copies; hand-shared photos are always kept regardless).
    #[must_use]
    pub const fn is_noise(self) -> bool {
        matches!(self, Self::NoiseOrAccidental)
    }
}

/// Attribution tag keeping owner words separate from model-derived text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAttribution {
    Owner,
    Model,
}

/// Text paired with its explicit provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributedText {
    pub text: String,
    pub attribution: TextAttribution,
}

impl AttributedText {
    #[must_use]
    pub fn owner(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attribution: TextAttribution::Owner,
        }
    }

    #[must_use]
    pub fn model(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attribution: TextAttribution::Model,
        }
    }
}

/// Reference back to a face row stored in an entity's `faceprints.npz`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoFaceKeyRef {
    pub day: String,
    pub stream: String,
    pub stream_layout: String,
    pub segment_key: String,
    pub source_name: String,
    pub frame_id: u64,
    pub face_idx: usize,
}

/// A person matched in this photo against the owner's meeting faceprints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhotoFaceLink {
    pub entity_id: String,
    pub face_idx: usize,
    pub bbox: [f32; 4],
    pub det_score: f32,
    pub similarity: f32,
    pub margin: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_key: Option<PhotoFaceKeyRef>,
}

/// One canonical source-bound observation for a kept photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhotoObservation {
    pub schema: u32,
    pub observation_revision: u64,
    pub source: PhotoObservationSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    pub time: PhotoObservationTime,
    pub state: PhotoObservationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_reason: Option<String>,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_caption: Option<AttributedText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<AttributedText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_text: Option<AttributedText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub belonging_class: Option<BelongingClass>,
    #[serde(default)]
    pub faces: Vec<PhotoFaceLink>,
    #[serde(default)]
    pub faces_checked: bool,
    pub updated_at: String,
}

impl PhotoObservation {
    /// Build a source-bound reference to this observation for downstream records.
    #[must_use]
    pub fn reference(&self) -> PhotoObservationRef {
        PhotoObservationRef {
            day: self.source.day.clone(),
            stream: self.source.stream.clone(),
            segment: self.source.segment.clone(),
            artifact_selector: self.source.artifact_selector.clone(),
            source_sha256: self.source.source_sha256.clone(),
            observation_revision: self.observation_revision,
        }
    }
}

/// Stable reference stored by downstream consumers (activities, experiences, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoObservationRef {
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub artifact_selector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sha256: Option<String>,
    pub observation_revision: u64,
}

/// Outcome of resolving a [`PhotoObservationRef`] against the live journal.
#[derive(Debug, Clone, PartialEq)]
pub enum PhotoObservationResolution {
    /// The photo is live, matches its selector and content digest, and has an observation.
    Resolved(Box<PhotoObservation>),
    /// The photo segment is missing, trashed, tombstoned, or holds no photo observation.
    NotFound,
    /// A live photo sits at the same coordinate, but its selector or pixel bytes changed.
    Replaced,
}

impl PhotoObservationResolution {
    #[must_use]
    pub fn into_resolved(self) -> Option<PhotoObservation> {
        match self {
            Self::Resolved(obs) => Some(*obs),
            Self::NotFound | Self::Replaced => None,
        }
    }
}

fn validate_ref_components(day: &str, stream: &str, segment: &str) -> bool {
    day.len() == 8
        && day.bytes().all(|b| b.is_ascii_digit())
        && is_safe_stream_component(stream)
        && !segment.is_empty()
        && !segment.contains(['/', '\\'])
        && segment != "."
        && segment != ".."
}

fn validate_selector(selector: &str) -> bool {
    !selector.is_empty()
        && !selector.contains(['/', '\\'])
        && selector != "."
        && selector != ".."
        && !selector.starts_with('.')
}

/// Atomically write `photo_observation.json` inside `segment_dir` with mode `0o600`.
pub fn write_photo_observation(
    segment_dir: &Path,
    observation: &PhotoObservation,
) -> io::Result<()> {
    let path = segment_dir.join(PHOTO_OBSERVATION_FILENAME);
    let encoded = serde_json::to_vec_pretty(observation)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    atomic_replace(
        &path,
        &encoded,
        AtomicWriteOptions {
            mode: Some(PRIVATE_FILE_MODE),
        },
    )
    .map_err(io::Error::other)
}

/// Read `photo_observation.json` from a segment directory without mutating damaged files.
#[must_use]
pub fn read_photo_observation(segment_dir: &Path) -> Option<PhotoObservation> {
    let path = segment_dir.join(PHOTO_OBSERVATION_FILENAME);
    match observe_json_durable::<PhotoObservation>(ArtifactId::SegmentPhotoObservation, &path) {
        DurableObservation::Present(obs) if obs.schema == PHOTO_OBSERVATION_SCHEMA => Some(obs),
        _ => None,
    }
}

/// Read `photo_observation.json` for a live segment, falling back to projecting
/// legacy `import.image` segments (`image_meta.json` + `image_transcript.md` + `original.*`).
#[must_use]
pub fn read_or_project_photo_observation(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Option<PhotoObservation> {
    if !validate_ref_components(day, stream, segment) {
        return None;
    }
    let dir = contained_path(journal_root, &format!("chronicle/{day}/{stream}/{segment}")).ok()?;
    if !dir.is_dir() || owner_deletion_state(&dir).ok()? != OwnerDeletionState::Live {
        return None;
    }
    if let Some(obs) = read_photo_observation(&dir) {
        return Some(obs);
    }
    project_legacy_photo_observation(&dir, day, stream, segment)
}

/// Resolve a downstream [`PhotoObservationRef`] against the live journal.
///
/// Returns [`PhotoObservationResolution::NotFound`] when the photo has been
/// trashed, tombstoned, or removed; [`PhotoObservationResolution::Replaced`]
/// when the selector or source bytes at that coordinate changed; and
/// [`PhotoObservationResolution::Resolved`] when the photo is live (including
/// after a trash restore).
#[must_use]
pub fn resolve_photo_observation(
    journal_root: &Path,
    reference: &PhotoObservationRef,
) -> PhotoObservationResolution {
    if !validate_ref_components(&reference.day, &reference.stream, &reference.segment)
        || !validate_selector(&reference.artifact_selector)
    {
        return PhotoObservationResolution::NotFound;
    }
    let Ok(dir) = contained_path(
        journal_root,
        &format!(
            "chronicle/{}/{}/{}",
            reference.day, reference.stream, reference.segment
        ),
    ) else {
        return PhotoObservationResolution::NotFound;
    };
    if !dir.is_dir() {
        return PhotoObservationResolution::NotFound;
    }
    match owner_deletion_state(&dir) {
        Ok(OwnerDeletionState::Live) => {}
        _ => return PhotoObservationResolution::NotFound,
    }
    let Some(obs) = read_photo_observation(&dir).or_else(|| {
        project_legacy_photo_observation(
            &dir,
            &reference.day,
            &reference.stream,
            &reference.segment,
        )
    }) else {
        return PhotoObservationResolution::NotFound;
    };
    if obs.source.artifact_selector != reference.artifact_selector {
        return PhotoObservationResolution::Replaced;
    }
    let selector_path = dir.join(&reference.artifact_selector);
    let Ok(selector_bytes) = read_bytes(&selector_path, vec![]) else {
        // An observation whose referenced original disappeared is no longer live.
        return PhotoObservationResolution::NotFound;
    };
    let live_sha = format!("{:x}", Sha256::digest(&selector_bytes));
    if let Some(expected_sha) = reference.source_sha256.as_deref()
        && live_sha != expected_sha
    {
        return PhotoObservationResolution::Replaced;
    }
    if let Some(obs_sha) = obs.source.source_sha256.as_deref()
        && obs_sha != live_sha
    {
        return PhotoObservationResolution::Replaced;
    }
    PhotoObservationResolution::Resolved(Box::new(obs))
}

/// Find the live non-screenshot original photo filename inside `segment_dir`.
#[must_use]
pub fn find_photo_original_name(segment_dir: &Path) -> Option<String> {
    for ext in PHOTO_ORIGINAL_EXTENSIONS {
        let name = format!("original.{ext}");
        if segment_dir.join(&name).is_file() {
            return Some(name);
        }
    }
    if segment_dir.join("image_source.json").is_file() {
        return Some("image_source.json".to_owned());
    }
    None
}

fn project_legacy_photo_observation(
    dir: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Option<PhotoObservation> {
    let artifact_selector = find_photo_original_name(dir)?;
    if artifact_selector.starts_with("original.screenshot.") {
        return None;
    }
    let source_bytes = read_bytes(dir.join(&artifact_selector), vec![]).ok()?;
    let source_sha256 = Some(format!("{:x}", Sha256::digest(&source_bytes)));
    let meta: Option<serde_json::Value> = read_bytes(dir.join(IMAGE_META_FILENAME), vec![])
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let time_source = meta
        .as_ref()
        .and_then(|v| v.get("time_source").and_then(serde_json::Value::as_str))
        .unwrap_or("capture")
        .to_owned();
    let utc_offset_seconds = meta
        .as_ref()
        .and_then(|v| {
            v.get("utc_offset_seconds")
                .and_then(serde_json::Value::as_i64)
        })
        .and_then(|v| i32::try_from(v).ok());
    let tz = meta
        .as_ref()
        .and_then(|v| v.get("tz").and_then(serde_json::Value::as_str))
        .map(ToOwned::to_owned);
    let placed_at = placed_at_from_coords(day, segment);
    let (captured_at, precision, provenance) = match time_source.as_str() {
        "upload_fallback" => (
            None,
            PhotoTimePrecision::Unknown,
            PhotoTimeProvenance::UploadFallback,
        ),
        "owner" => (
            Some(format_with_offset(&placed_at, utc_offset_seconds)),
            PhotoTimePrecision::Second,
            PhotoTimeProvenance::Owner,
        ),
        _ => (
            Some(format_with_offset(&placed_at, utc_offset_seconds)),
            PhotoTimePrecision::Second,
            PhotoTimeProvenance::Exif,
        ),
    };
    let owner_caption = meta
        .as_ref()
        .and_then(|v| v.get("caption").and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(AttributedText::owner);
    let original_filename = meta
        .as_ref()
        .and_then(|v| {
            v.get("original_filename")
                .and_then(serde_json::Value::as_str)
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let transcript = read_bytes(dir.join("image_transcript.md"), vec![])
        .ok()
        .and_then(|b| String::from_utf8(b).ok());
    let (state, description, state_reason) = match transcript.as_deref() {
        Some(text) => parse_legacy_transcript_state(text),
        None => (PhotoObservationState::Pending, None, None),
    };

    Some(PhotoObservation {
        schema: PHOTO_OBSERVATION_SCHEMA,
        observation_revision: 1,
        source: PhotoObservationSource {
            day: day.to_owned(),
            stream: stream.to_owned(),
            segment: segment.to_owned(),
            artifact_selector,
            source_sha256,
            asset_id: None,
            import_id: None,
            original_filename,
        },
        width: None,
        height: None,
        format: None,
        time: PhotoObservationTime {
            captured_at,
            placed_at,
            utc_offset_seconds,
            tz,
            precision,
            provenance,
            time_source,
        },
        state,
        state_reason,
        retry_count: 0,
        owner_caption,
        description,
        visible_text: None,
        belonging_class: None,
        faces: Vec::new(),
        faces_checked: false,
        updated_at: String::new(),
    })
}

fn placed_at_from_coords(day: &str, segment: &str) -> String {
    let clock = segment.split('_').next().unwrap_or("000000");
    if day.len() == 8 && clock.len() >= 6 {
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
        format!("{day}T{clock}")
    }
}

fn format_with_offset(civil: &str, offset_seconds: Option<i32>) -> String {
    let Some(secs) = offset_seconds else {
        return civil.to_owned();
    };
    let sign = if secs < 0 { '-' } else { '+' };
    let abs = secs.unsigned_abs();
    let hours = abs / 3600;
    let mins = (abs % 3600) / 60;
    format!("{civil}{sign}{hours:02}:{mins:02}")
}

fn parse_legacy_transcript_state(
    text: &str,
) -> (
    PhotoObservationState,
    Option<AttributedText>,
    Option<String>,
) {
    let mut model_lines = Vec::new();
    let mut in_model = false;
    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.starts_with('>') {
            let body = line
                .strip_prefix("> ")
                .or_else(|| line.strip_prefix('>'))
                .unwrap_or("");
            if !in_model && body.trim().starts_with("[image description") {
                in_model = true;
                continue;
            }
            if in_model {
                model_lines.push(body);
            }
        }
    }
    let joined = model_lines.join("\n").trim().to_owned();
    if joined.is_empty()
        || joined == "unavailable"
        || joined.starts_with("unavailable —")
        || joined.starts_with("unavailable -")
    {
        let state =
            if joined.contains("no-engine-configured") || joined.contains("No active provider") {
                PhotoObservationState::ThinkingOff
            } else {
                PhotoObservationState::AnalysisUnavailable
            };
        (state, None, (!joined.is_empty()).then_some(joined))
    } else {
        (
            PhotoObservationState::Ready,
            Some(AttributedText::model(joined)),
            None,
        )
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn sample_observation(sha: &str) -> PhotoObservation {
        PhotoObservation {
            schema: PHOTO_OBSERVATION_SCHEMA,
            observation_revision: 1,
            source: PhotoObservationSource {
                day: "20261119".to_owned(),
                stream: "import.image".to_owned(),
                segment: "120000_0".to_owned(),
                artifact_selector: "original.jpg".to_owned(),
                source_sha256: Some(sha.to_owned()),
                asset_id: None,
                import_id: Some("imp_1".to_owned()),
                original_filename: Some("IMG_0001.JPG".to_owned()),
            },
            width: Some(1024),
            height: Some(768),
            format: Some("JPEG".to_owned()),
            time: PhotoObservationTime {
                captured_at: Some("2026-11-19T12:00:00-07:00".to_owned()),
                placed_at: "2026-11-19T12:00:00".to_owned(),
                utc_offset_seconds: Some(-25200),
                tz: Some("America/Denver".to_owned()),
                precision: PhotoTimePrecision::Second,
                provenance: PhotoTimeProvenance::Exif,
                time_source: "capture".to_owned(),
            },
            state: PhotoObservationState::Ready,
            state_reason: None,
            retry_count: 0,
            owner_caption: Some(AttributedText::owner("Trailhead at sunrise")),
            description: Some(AttributedText::model("Two hikers on a snowy ridge.")),
            visible_text: Some(AttributedText::model("SOUTH MESA TRAIL")),
            belonging_class: Some(BelongingClass::LivedMoment),
            faces: Vec::new(),
            faces_checked: false,
            updated_at: "2026-11-19T19:00:00Z".to_owned(),
        }
    }

    #[test]
    fn observation_resolves_live_stops_on_trash_returns_on_restore_and_detects_replacement() {
        let root = crate::test_support::TempDir::new();
        let seg = root.path().join("chronicle/20261119/import.image/120000_0");
        std::fs::create_dir_all(&seg).unwrap();
        let pixel_bytes = b"photo-bytes-v1";
        std::fs::write(seg.join("original.jpg"), pixel_bytes).unwrap();
        let sha1 = format!("{:x}", Sha256::digest(pixel_bytes));
        let obs = sample_observation(&sha1);
        write_photo_observation(&seg, &obs).unwrap();
        let obs_ref = obs.reference();

        // Live resolves.
        assert_eq!(
            resolve_photo_observation(root.path(), &obs_ref),
            PhotoObservationResolution::Resolved(Box::new(obs.clone()))
        );

        // Trashing (placing tombstone) stops resolution.
        std::fs::write(
            seg.join("tombstone.json"),
            r#"{"trashed_at":"2026-11-20T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(
            resolve_photo_observation(root.path(), &obs_ref),
            PhotoObservationResolution::NotFound
        );

        // Restoring (removing tombstone) resolves again.
        std::fs::remove_file(seg.join("tombstone.json")).unwrap();
        assert_eq!(
            resolve_photo_observation(root.path(), &obs_ref),
            PhotoObservationResolution::Resolved(Box::new(obs))
        );

        // Replacing bytes at the same coordinate stops resolving as old ref.
        std::fs::write(seg.join("original.jpg"), b"photo-bytes-v2-different").unwrap();
        assert_eq!(
            resolve_photo_observation(root.path(), &obs_ref),
            PhotoObservationResolution::Replaced
        );
    }
}
