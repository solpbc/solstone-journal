// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Still-photo face detection, side-effect-free query, and entity faceprint
//! linking.
//!
//! On desktop platforms, ONNX inference runs inside the dynamically linked
//! `solstone-core-describe` sibling helper so static-musl `solstone-core`
//! binaries never load ONNX Runtime directly. On mobile platforms (iOS and
//! Android), photo face passes are disabled cleanly at compile time.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[cfg(not(any(target_os = "ios", target_os = "android")))]
use base64::Engine as _;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use serde::Deserialize;
use sha2::{Digest, Sha256};
use solstone_core_entity::faceprints::{
    self, Encoder, Face, FaceKey, Metadata, Source,
    photo_match::{
        MeetingFaceIndex, PhotoFaceCandidate, PhotoFaceMatch, load_meeting_face_index,
        match_photo_faces,
    },
};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, create_directory_with_mode};
use solstone_core_segment::photo_observation::{PhotoFaceKeyRef, PhotoFaceLink};

const PRIVATE_DIR_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;

/// Detected face output returned by a [`PhotoFaceDetector`].
#[derive(Clone, Debug)]
pub struct DetectedPhotoFaces {
    pub encoder: Encoder,
    pub width: u32,
    pub height: u32,
    pub faces: Vec<PhotoFaceCandidate>,
}

/// Per-photo detection result in a batch face query.
pub type PhotoFaceBatchResult = (PathBuf, Result<DetectedPhotoFaces, String>);

/// Boundary for still-photo face detection so tests can inject deterministic
/// synthetic embeddings without requiring ONNX Runtime assets.
pub trait PhotoFaceDetector: Send + Sync {
    fn detect_photo(
        &self,
        image_path: &Path,
        journal: Option<&Path>,
        with_crops: bool,
    ) -> Result<DetectedPhotoFaces, String>;

    fn detect_photos_batch(
        &self,
        image_paths: &[PathBuf],
        journal: Option<&Path>,
        with_crops: bool,
    ) -> Result<Vec<PhotoFaceBatchResult>, String> {
        Ok(image_paths
            .iter()
            .map(|p| (p.clone(), self.detect_photo(p, journal, with_crops)))
            .collect())
    }
}

/// Production desktop detector invoking sibling `solstone-core-describe`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemPhotoFaceDetector;

#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[derive(Deserialize)]
struct WireEncoder {
    id: String,
    sha256: String,
    width: usize,
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[derive(Deserialize)]
struct WireFace {
    face_idx: usize,
    #[serde(rename = "box")]
    bbox: [f32; 4],
    det_score: f32,
    sharpness: f32,
    quality: f32,
    embedding: Vec<f32>,
    #[serde(default)]
    crop_jpeg_b64: Option<String>,
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[derive(Deserialize)]
struct WireQueryOutput {
    encoder: WireEncoder,
    width: u32,
    height: u32,
    faces: Vec<WireFace>,
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[derive(Deserialize)]
struct WireBatchEntry {
    path: String,
    #[serde(default)]
    output: Option<WireQueryOutput>,
    #[serde(default)]
    error: Option<String>,
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn decode_wire_output(wire: WireQueryOutput) -> Result<DetectedPhotoFaces, String> {
    let mut faces = Vec::with_capacity(wire.faces.len());
    for f in wire.faces {
        let crop_jpeg = match f.crop_jpeg_b64 {
            Some(b64) => Some(
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| format!("invalid crop base64: {e}"))?,
            ),
            None => None,
        };
        faces.push(PhotoFaceCandidate {
            face_idx: f.face_idx,
            bbox: f.bbox,
            det_score: f.det_score,
            sharpness: f.sharpness,
            quality: f.quality,
            embedding: f.embedding,
            crop_jpeg,
        });
    }
    Ok(DetectedPhotoFaces {
        encoder: Encoder {
            id: wire.encoder.id,
            sha256: wire.encoder.sha256,
            width: wire.encoder.width,
        },
        width: wire.width,
        height: wire.height,
        faces,
    })
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn describe_helper_binary() -> Result<PathBuf, String> {
    if let Some(explicit) = std::env::var_os("SOLSTONE_DESCRIBE_BIN")
        && !explicit.is_empty()
    {
        return Ok(PathBuf::from(explicit));
    }
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let name = if cfg!(windows) {
        "solstone-core-describe.exe"
    } else {
        "solstone-core-describe"
    };
    let candidate = current.with_file_name(name);
    if candidate.is_file() {
        return Ok(candidate);
    }
    // Cargo test binaries live under `target/<profile>/deps/`; look one directory up.
    if let Some(parent) = current.parent().and_then(Path::parent) {
        let sibling = parent.join(name);
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    Err(format!(
        "describe helper not found at {}",
        candidate.display()
    ))
}

impl PhotoFaceDetector for SystemPhotoFaceDetector {
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    fn detect_photo(
        &self,
        image_path: &Path,
        journal: Option<&Path>,
        with_crops: bool,
    ) -> Result<DetectedPhotoFaces, String> {
        use solstone_core_segment::face_backfill;
        use std::process::Command;

        let binary = describe_helper_binary()?;
        let mut cmd = Command::new(&binary);
        cmd.arg(face_backfill::PHOTO_QUERY_OPTION);
        if with_crops {
            cmd.arg(face_backfill::PHOTO_WITH_CROPS_FLAG);
        }
        if let Some(j) = journal {
            cmd.arg(face_backfill::PHOTO_JOURNAL_FLAG).arg(j);
        }
        cmd.arg(image_path);
        let output = cmd.output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "photo face query exited {}: {}",
                output.status,
                stderr.trim()
            ));
        }
        let parsed: WireQueryOutput =
            serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
        decode_wire_output(parsed)
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    fn detect_photos_batch(
        &self,
        image_paths: &[PathBuf],
        journal: Option<&Path>,
        with_crops: bool,
    ) -> Result<Vec<PhotoFaceBatchResult>, String> {
        use solstone_core_segment::face_backfill;
        use std::io::Write;
        use std::process::{Command, Stdio};

        if image_paths.is_empty() {
            return Ok(Vec::new());
        }
        let binary = describe_helper_binary()?;
        let mut cmd = Command::new(&binary);
        cmd.arg(face_backfill::PHOTO_QUERY_BATCH_OPTION);
        if with_crops {
            cmd.arg(face_backfill::PHOTO_WITH_CROPS_FLAG);
        }
        if let Some(j) = journal {
            cmd.arg(face_backfill::PHOTO_JOURNAL_FLAG).arg(j);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        if let Some(mut stdin) = child.stdin.take() {
            for path in image_paths {
                writeln!(stdin, "{}", path.display()).map_err(|e| e.to_string())?;
            }
        }
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "photo face batch query exited {}: {}",
                output.status,
                stderr.trim()
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut out = Vec::with_capacity(image_paths.len());
        for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
            let entry: WireBatchEntry = serde_json::from_str(line).map_err(|e| e.to_string())?;
            let path = PathBuf::from(&entry.path);
            let res = match (entry.output, entry.error) {
                (Some(wire), _) => decode_wire_output(wire),
                (None, Some(err)) => Err(err),
                (None, None) => Err("empty batch face query entry".to_owned()),
            };
            out.push((path, res));
        }
        Ok(out)
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    fn detect_photo(
        &self,
        _image_path: &Path,
        _journal: Option<&Path>,
        _with_crops: bool,
    ) -> Result<DetectedPhotoFaces, String> {
        Err("photo face detection is desktop-only".to_owned())
    }
}

/// Side-effect-free face query for a staged photo copy (used by photo library
/// selection to check whether someone from the owner's meeting faceprints
/// appears in a photo without writing crops, sidecars, or entity rows).
///
/// Honors `faces.enabled` (desktop only); does NOT require the photo-pass write
/// flag because it performs zero mutations and only answers a selection query.
pub fn query_photo_faces_against_journal(
    journal: &Path,
    image_path: &Path,
    detector: &dyn PhotoFaceDetector,
) -> Result<Vec<PhotoFaceMatch>, String> {
    if cfg!(any(target_os = "ios", target_os = "android")) {
        return Ok(Vec::new());
    }
    if !faceprints::enabled(journal)? {
        return Ok(Vec::new());
    }
    let index = load_meeting_face_index(journal)?;
    if index.is_empty() {
        return Ok(Vec::new());
    }
    let detected = detector.detect_photo(image_path, Some(journal), false)?;
    match_with_encoder_check(&index, &detected)
}

/// Side-effect-free batch face query over multiple staged photo copies.
pub fn query_photo_faces_batch_against_journal(
    journal: &Path,
    image_paths: &[PathBuf],
    detector: &dyn PhotoFaceDetector,
) -> Result<Vec<(PathBuf, Vec<PhotoFaceMatch>)>, String> {
    if cfg!(any(target_os = "ios", target_os = "android")) || image_paths.is_empty() {
        return Ok(Vec::new());
    }
    if !faceprints::enabled(journal)? {
        return Ok(image_paths
            .iter()
            .map(|p| (p.clone(), Vec::new()))
            .collect());
    }
    let index = load_meeting_face_index(journal)?;
    if index.is_empty() {
        return Ok(image_paths
            .iter()
            .map(|p| (p.clone(), Vec::new()))
            .collect());
    }
    let batch = detector.detect_photos_batch(image_paths, Some(journal), false)?;
    let mut results = Vec::with_capacity(batch.len());
    for (path, detected) in batch {
        match detected {
            Ok(det) => {
                let matches = match_with_encoder_check(&index, &det)?;
                results.push((path, matches));
            }
            Err(_) => results.push((path, Vec::new())),
        }
    }
    Ok(results)
}

fn match_with_encoder_check(
    index: &MeetingFaceIndex,
    detected: &DetectedPhotoFaces,
) -> Result<Vec<PhotoFaceMatch>, String> {
    if let Some(expected) = index.encoder.as_ref()
        && expected != &detected.encoder
    {
        return Err("face encoder mismatch between meeting archive and photo detector".to_owned());
    }
    Ok(match_photo_faces(index, &detected.faces))
}

/// Outcome of running the desktop photo face pass on one kept photo segment.
#[derive(Debug, Clone, Default)]
pub struct PhotoFacePassOutcome {
    pub checked: bool,
    pub links: Vec<PhotoFaceLink>,
}

/// Run the desktop photo face pass for one kept photo segment, gated behind
/// BOTH `faces.enabled` and `faces.photo_match_enabled` (OFF by default).
///
/// Only matched faces write a private crop under `.faces-v1-<prefix>-crops/`
/// and a `Source::Photo` row into the matched entity's `faceprints.npz`.
pub fn apply_photo_face_pass(
    journal: &Path,
    segment_dir: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    source_name: &str,
    detector: &dyn PhotoFaceDetector,
) -> Result<PhotoFacePassOutcome, String> {
    if cfg!(any(target_os = "ios", target_os = "android")) {
        return Ok(PhotoFacePassOutcome::default());
    }
    if !faceprints::photo_faces_active(journal)? {
        return Ok(PhotoFacePassOutcome::default());
    }
    let index = load_meeting_face_index(journal)?;
    if index.is_empty() {
        return Ok(PhotoFacePassOutcome {
            checked: true,
            links: Vec::new(),
        });
    }
    let photo_path = segment_dir.join(source_name);
    if !photo_path.is_file() {
        return Ok(PhotoFacePassOutcome::default());
    }
    let detected = detector.detect_photo(&photo_path, Some(journal), true)?;
    let matches = match_with_encoder_check(&index, &detected)?;
    let prefix = faceprints::accumulation::source_prefix(source_name);
    let crops_dir = segment_dir.join(format!("{prefix}-crops"));
    let segment_rel = format!("chronicle/{day}/{stream}/{segment}");
    let now = solstone_core_time::utc_rfc3339(jiff::Timestamp::now());

    let mut desired: BTreeMap<String, Vec<Face>> = BTreeMap::new();
    let mut links = Vec::with_capacity(matches.len());

    for m in matches {
        let crop_rel = if let Some(jpeg) = m.crop_jpeg.as_deref()
            && !jpeg.is_empty()
        {
            create_directory_with_mode(&crops_dir, PRIVATE_DIR_MODE).map_err(|e| e.to_string())?;
            let filename = format!("0-{}-{:x}.jpg", m.face_idx, Sha256::digest(jpeg));
            let rel = format!("{prefix}-crops/{filename}");
            atomic_replace(
                crops_dir.join(&filename),
                jpeg,
                AtomicWriteOptions {
                    mode: Some(PRIVATE_FILE_MODE),
                },
            )
            .map_err(|e| e.to_string())?;
            Some(rel)
        } else {
            None
        };

        let key = FaceKey {
            day: day.to_owned(),
            stream: stream.to_owned(),
            stream_layout: "named".to_owned(),
            segment_key: segment.to_owned(),
            source_name: source_name.to_owned(),
            frame_id: 0,
            face_idx: m.face_idx,
        };
        if let Some(c) = crop_rel.as_deref() {
            faceprints::crop_path(journal, &key, c)?;
        }
        let face_key_ref = PhotoFaceKeyRef {
            day: key.day.clone(),
            stream: key.stream.clone(),
            stream_layout: key.stream_layout.clone(),
            segment_key: key.segment_key.clone(),
            source_name: key.source_name.clone(),
            frame_id: key.frame_id,
            face_idx: key.face_idx,
        };
        let face = Face {
            embedding: m.embedding,
            metadata: Metadata {
                schema_version: 1,
                key,
                activities: BTreeSet::new(),
                source: Source::Photo,
                label_text: None,
                quality: m.quality,
                det_score: m.det_score,
                face_w: m.bbox[2],
                category: "photo".to_owned(),
                crop: crop_rel.clone(),
                added_at: now.clone(),
                rejected: false,
            },
        };
        desired.entry(m.entity_id.clone()).or_default().push(face);
        links.push(PhotoFaceLink {
            entity_id: m.entity_id,
            face_idx: m.face_idx,
            bbox: m.bbox,
            det_score: m.det_score,
            similarity: m.similarity,
            margin: m.margin,
            crop: crop_rel,
            face_key: Some(face_key_ref),
        });
    }

    faceprints::record_photo_faces(journal, &segment_rel, &detected.encoder, &desired)?;
    Ok(PhotoFacePassOutcome {
        checked: true,
        links,
    })
}
