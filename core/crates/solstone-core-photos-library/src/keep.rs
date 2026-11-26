// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Keep & Read pipeline for arriving Photos Library copies and chosen-set trash.
//!
//! When a paired device answers a wanted item with a 512 px JPEG:
//! 1. Fail-closed duplicate/suppression check against `config/photos/received/`
//!    by `asset_id` (and `cloud_id` when present). Two devices on one library
//!    never produce two copies, and an owner-trashed photo never comes back.
//! 2. Validate JPEG header and pixel bounds before staging bytes in a private
//!    temp file outside `chronicle/`.
//! 3. If the asset was staged under the others'-photos rung (`StageOthersFaceCheck`),
//!    run Part A's side-effect-free `query_photo_faces_against_journal`: discard
//!    immediately if zero meeting faceprints match.
//! 4. Run Part A's `prepare_manual_image_with_options` (structured JPEG read +
//!    belonging class): if classified `noise_or_accidental`, discard without
//!    writing to `chronicle/` and add to left-out `noise` counts.
//! 5. Otherwise install into the syncing `photos` stream at capture moment
//!    (creating `original.jpg`, `image_meta.json`, `image_transcript.md`,
//!    `photo_observation.json`, optional GPS `location.jsonl`, and optional
//!    gated desktop face pass), record durable receipt in `config/photos/received/`,
//!    and mark landed.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use image::{ImageFormat, ImageReader};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_import::CaptureLocation;
use solstone_core_import::publish::{NativePublicationOperations, PublicationOperations};
use solstone_core_import_sources::image::{
    ImageImportOptions, SystemWireClient, WireClient, install_photo_in_stream_with_faces,
    prepare_manual_image_with_options,
};
use solstone_core_import_sources::photo_faces::{
    PhotoFaceDetector, SystemPhotoFaceDetector, query_photo_faces_against_journal,
};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, create_directory_with_mode};
use solstone_core_removals::{Action, Page, Surface, Unit};
use solstone_core_segment::PHOTOS_SOURCE;
use solstone_core_segment::photo_observation::{BelongingClass, PhotoTimeProvenance};

use crate::holding::{
    WantedAssetStatus, holding_root, load_decisions, load_scan_rows, load_wanted_state,
    lock_holding, save_wanted_state,
};
use crate::received::{
    PhotoReceipt, lookup_received, record_received, record_suppression_for_segment,
};
use crate::types::{DecisionKind, DeviceState};

const MAX_COPY_PIXELS: u64 = 25_000_000;
const MAX_TRASH_SET: usize = 500;
const PRIVATE_DIR_MODE: u32 = 0o700;
const PRIVATE_FILE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };

/// Decode a standard base64 JPEG payload delivered over JSON.
pub fn decode_copy_jpeg_base64(encoded: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|e| format!("invalid base64 JPEG payload: {e}"))
}

/// Outcome of processing one delivered photo copy.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CopyIngestOutcome {
    Landed {
        day: String,
        stream: String,
        segment: String,
    },
    AlreadyReceived {
        day: String,
        stream: String,
        segment: String,
    },
    LeftOut {
        reason: String,
    },
}

/// Optional metadata override supplied alongside a copy upload (used when an
/// asset arrived between scans or in test fixtures).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CopyMetadataOverride {
    #[serde(default)]
    pub captured_at: Option<String>,
    #[serde(default)]
    pub caption: Option<String>,
    #[serde(default)]
    pub latitude: Option<f64>,
    #[serde(default)]
    pub longitude: Option<f64>,
    #[serde(default)]
    pub horizontal_accuracy_m: Option<f64>,
    #[serde(default)]
    pub original_filename: Option<String>,
    #[serde(default)]
    pub camera: Option<String>,
}

fn validate_jpeg(bytes: &[u8]) -> Result<(u32, u32), String> {
    if bytes.is_empty() {
        return Err("empty JPEG payload".to_owned());
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "unreadable image payload".to_owned())?;
    if reader.format() != Some(ImageFormat::Jpeg) {
        return Err("photo copy must be a valid JPEG".to_owned());
    }
    let (w, h) = reader
        .into_dimensions()
        .map_err(|_| "unreadable JPEG dimensions".to_owned())?;
    if w == 0 || h == 0 {
        return Err("photo copy has zero dimensions".to_owned());
    }
    if u64::from(w) * u64::from(h) > MAX_COPY_PIXELS {
        return Err(format!(
            "photo copy is {w}x{h}, exceeding {MAX_COPY_PIXELS} pixels"
        ));
    }
    Ok((w, h))
}

/// Production entry point using `SystemWireClient`, `SystemPhotoFaceDetector`,
/// and `NativePublicationOperations`.
pub fn ingest_photo_copy(
    journal: &Path,
    cid: &str,
    asset_id: &str,
    cloud_id: Option<&str>,
    jpeg_bytes: &[u8],
    metadata_override: Option<&CopyMetadataOverride>,
) -> Result<CopyIngestOutcome, String> {
    let wire = SystemWireClient;
    let face_detector = SystemPhotoFaceDetector;
    let publication = NativePublicationOperations;
    ingest_photo_copy_with(
        journal,
        cid,
        asset_id,
        cloud_id,
        jpeg_bytes,
        metadata_override,
        &wire,
        &face_detector,
        &publication,
    )
}

/// Full injectable Keep & Read pipeline for a single delivered photo copy.
#[allow(clippy::too_many_arguments)]
pub fn ingest_photo_copy_with(
    journal: &Path,
    cid: &str,
    asset_id: &str,
    cloud_id: Option<&str>,
    jpeg_bytes: &[u8],
    metadata_override: Option<&CopyMetadataOverride>,
    wire: &dyn WireClient,
    face_detector: &dyn PhotoFaceDetector,
    publication: &dyn PublicationOperations,
) -> Result<CopyIngestOutcome, String> {
    let asset_id = asset_id.trim();
    if asset_id.is_empty() {
        return Err("missing asset_id".to_owned());
    }
    validate_jpeg(jpeg_bytes)?;

    // 1. Read holding metadata & check durable received ledger before slow I/O.
    let (decision, scan_row, effective_cloud_id, device_name) = {
        let _lock = lock_holding(journal)?;
        let decisions = load_decisions(journal)?;
        let decision = decisions.into_iter().find(|d| d.id == asset_id);
        let effective_cloud = cloud_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| decision.as_ref().and_then(|d| d.cloud_id.clone()));

        if let Some(rec) = lookup_received(journal, asset_id, effective_cloud.as_deref())
            .map_err(|e| format!("failed reading photos received ledger: {e}"))?
        {
            return Ok(CopyIngestOutcome::AlreadyReceived {
                day: rec.day,
                stream: rec.stream,
                segment: rec.segment,
            });
        }

        let scan_row = load_scan_rows(journal)?
            .into_iter()
            .find(|r| r.id == asset_id);
        let device_name = read_device_name(journal, cid);
        (decision, scan_row, effective_cloud, device_name)
    };

    // 2. Stage JPEG bytes in a private temporary directory outside chronicle/.
    let stage_root = holding_root(journal).join(".stage");
    create_directory_with_mode(&stage_root, PRIVATE_DIR_MODE).map_err(|e| e.to_string())?;
    let digest_prefix = format!("{:x}", Sha256::digest(asset_id.as_bytes()));
    let nonce = Timestamp::now().as_nanosecond();
    let stage_path = stage_root.join(format!("{}-{nonce}.jpg", &digest_prefix[..16]));
    atomic_replace(&stage_path, jpeg_bytes, PRIVATE_FILE).map_err(|e| e.to_string())?;

    // Guard so staged temp file is always deleted on every exit path.
    struct StageGuard(std::path::PathBuf);
    impl Drop for StageGuard {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _stage_guard = StageGuard(stage_path.clone());

    let now_iso = solstone_core_time::utc_rfc3339(Timestamp::now());

    // 3. Others'-photos rung: require calibrated face match before keeping.
    if decision
        .as_ref()
        .is_some_and(|d| d.decision == DecisionKind::StageOthersFaceCheck)
    {
        let matches = query_photo_faces_against_journal(journal, &stage_path, face_detector)?;
        if matches.is_empty() {
            let _lock = lock_holding(journal)?;
            let mut wanted = load_wanted_state(journal)?;
            wanted.assets.insert(
                asset_id.to_owned(),
                WantedAssetStatus::OthersNoFaceMatch { at: now_iso },
            );
            save_wanted_state(journal, &wanted)?;
            return Ok(CopyIngestOutcome::LeftOut {
                reason: "others_no_face_match".to_owned(),
            });
        }
    }

    // 4. Prepare Part A observation (JPEG vision read + belonging class).
    let tz_name = decision
        .as_ref()
        .and_then(|d| d.timezone.clone())
        .unwrap_or_else(|| "UTC".to_owned());
    let zone = TimeZone::get(&tz_name).unwrap_or(TimeZone::UTC);

    // Prefer exact local ISO derived from capture time + capture location timezone.
    let client_captured_at = decision
        .as_ref()
        .and_then(|d| d.local_iso.clone().or_else(|| d.captured_at.clone()))
        .or_else(|| scan_row.as_ref().and_then(|r| r.captured_at.clone()))
        .or_else(|| metadata_override.and_then(|m| m.captured_at.clone()));

    let caption = scan_row
        .as_ref()
        .and_then(|r| r.extended.as_ref())
        .and_then(|e| e.caption.clone())
        .or_else(|| metadata_override.and_then(|m| m.caption.clone()))
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty());

    let original_filename = scan_row
        .as_ref()
        .and_then(|r| r.extended.as_ref())
        .and_then(|e| e.original_filename.clone())
        .or_else(|| metadata_override.and_then(|m| m.original_filename.clone()));

    let camera = scan_row
        .as_ref()
        .and_then(|r| r.extended.as_ref())
        .and_then(|e| e.camera.clone())
        .or_else(|| metadata_override.and_then(|m| m.camera.clone()));

    let capture_location = scan_row
        .as_ref()
        .and_then(|r| r.location.as_ref())
        .map(|loc| CaptureLocation {
            latitude: loc.latitude,
            longitude: loc.longitude,
            accuracy_m: loc.horizontal_accuracy_m,
            camera_make: None,
            camera_model: camera.clone(),
        })
        .or_else(|| {
            metadata_override.and_then(|m| {
                Some(CaptureLocation {
                    latitude: m.latitude?,
                    longitude: m.longitude?,
                    accuracy_m: m.horizontal_accuracy_m,
                    camera_make: None,
                    camera_model: camera.clone(),
                })
            })
        });

    let options = ImageImportOptions {
        caption,
        client_captured_at,
        original_filename,
        asset_id: Some(asset_id.to_owned()),
        time_provenance: Some(PhotoTimeProvenance::LibraryMetadata),
        capture_location,
        tz: Some(tz_name),
    };

    let prepared = prepare_manual_image_with_options(&stage_path, wire, &zone, None, None, options)
        .map_err(|e| format!("prepare photo observation failed: {e}"))?;

    // Drop copies that Part A classifies as `noise_or_accidental` without keeping.
    if prepared.vision.reading().and_then(|r| r.belonging_class)
        == Some(BelongingClass::NoiseOrAccidental)
    {
        let _lock = lock_holding(journal)?;
        let mut wanted = load_wanted_state(journal)?;
        wanted.assets.insert(
            asset_id.to_owned(),
            WantedAssetStatus::NoiseDropped { at: now_iso },
        );
        save_wanted_state(journal, &wanted)?;
        return Ok(CopyIngestOutcome::LeftOut {
            reason: "noise".to_owned(),
        });
    }

    // 5. Acquire holding lock for atomic dedup check + chronicle install + receipt write.
    //    Guarantees two concurrent devices on one library produce strictly one copy.
    let _lock = lock_holding(journal)?;
    if let Some(rec) = lookup_received(journal, asset_id, effective_cloud_id.as_deref())
        .map_err(|e| format!("failed reading photos received ledger: {e}"))?
    {
        return Ok(CopyIngestOutcome::AlreadyReceived {
            day: rec.day,
            stream: rec.stream,
            segment: rec.segment,
        });
    }

    let imported = install_photo_in_stream_with_faces(
        &prepared,
        journal,
        PHOTOS_SOURCE,
        prepared.timestamp.datetime(),
        publication,
        None,
        None,
        face_detector,
    )
    .map_err(|e| format!("install library photo failed: {e}"))?;

    let day = imported.created_segment.day.clone();
    let stream = imported.created_segment.stream.clone();
    let segment = imported.created_segment.segment.clone();

    // Enrich image_meta.json with source library identity and device name.
    enrich_image_meta(
        &journal
            .join("chronicle")
            .join(&day)
            .join(&stream)
            .join(&segment),
        asset_id,
        effective_cloud_id.as_deref(),
        cid,
        device_name.as_deref(),
    );

    // Record durable receipt OUTSIDE the holding area (survives trash erase & turn-off).
    let receipt = PhotoReceipt {
        schema: 1,
        day: day.clone(),
        stream: stream.clone(),
        segment: segment.clone(),
        received_at: now_iso.clone(),
    };
    record_received(journal, asset_id, effective_cloud_id.as_deref(), &receipt)
        .map_err(|e| format!("record photos receipt: {e}"))?;

    let mut wanted = load_wanted_state(journal)?;
    wanted.assets.insert(
        asset_id.to_owned(),
        WantedAssetStatus::Landed {
            day: day.clone(),
            stream: stream.clone(),
            segment: segment.clone(),
            at: now_iso.clone(),
        },
    );
    save_wanted_state(journal, &wanted)?;
    update_device_last_upload(journal, cid, &now_iso);

    Ok(CopyIngestOutcome::Landed {
        day,
        stream,
        segment,
    })
}

fn enrich_image_meta(
    seg_dir: &Path,
    asset_id: &str,
    cloud_id: Option<&str>,
    cid: &str,
    device_name: Option<&str>,
) {
    let meta_path = seg_dir.join("image_meta.json");
    let mut map = fs::read(&meta_path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    map.insert(
        "asset_id".to_owned(),
        serde_json::Value::String(asset_id.to_owned()),
    );
    if let Some(cloud) = cloud_id {
        map.insert(
            "cloud_id".to_owned(),
            serde_json::Value::String(cloud.to_owned()),
        );
    }
    map.insert(
        "device_cid".to_owned(),
        serde_json::Value::String(cid.to_owned()),
    );
    if let Some(name) = device_name
        && !name.trim().is_empty()
    {
        map.insert(
            "device_name".to_owned(),
            serde_json::Value::String(name.to_owned()),
        );
    }
    if let Ok(mut bytes) = serde_json::to_vec(&serde_json::Value::Object(map)) {
        bytes.push(b'\n');
        let _ = atomic_replace(&meta_path, &bytes, PRIVATE_FILE);
    }
}

#[derive(Deserialize, Serialize)]
struct DevicesFileSlim {
    schema: u32,
    devices: std::collections::BTreeMap<String, DeviceState>,
}

fn read_device_name(journal: &Path, cid: &str) -> Option<String> {
    let path = holding_root(journal).join("devices.json");
    let bytes = fs::read(&path).ok()?;
    let file: DevicesFileSlim = serde_json::from_slice(&bytes).ok()?;
    file.devices.get(cid).map(|d| d.name.clone())
}

fn update_device_last_upload(journal: &Path, cid: &str, now_iso: &str) {
    let path = holding_root(journal).join("devices.json");
    let Ok(bytes) = fs::read(&path) else {
        return;
    };
    let Ok(mut file) = serde_json::from_slice::<DevicesFileSlim>(&bytes) else {
        return;
    };
    if let Some(dev) = file.devices.get_mut(cid) {
        dev.connected = true;
        dev.last_upload_at = Some(now_iso.to_owned());
        if let Ok(mut out) = serde_json::to_vec_pretty(&file) {
            out.push(b'\n');
            let _ = atomic_replace(&path, &out, PRIVATE_FILE);
        }
    }
}

/// One segment coordinate requested for chosen-set trash.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TrashSegmentRef {
    pub day: String,
    pub stream: String,
    pub segment: String,
}

/// Result of moving one chosen set of library photos to the journal's trash.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct TrashPhotosResult {
    pub moved: usize,
    pub failed: usize,
    pub trash_ids: Vec<String>,
}

/// Move a chosen set of `photos` segments to the journal's standard Trash
/// (honoring configured `erase_after_days`, 1–30 d, default 7 d) and ensure
/// durable `asset_id` suppression survives trash erase and turn-off.
pub fn trash_chosen_photos(
    journal: &Path,
    segments: &[TrashSegmentRef],
    user_agent: Option<&str>,
) -> Result<TrashPhotosResult, String> {
    if segments.is_empty() {
        return Ok(TrashPhotosResult::default());
    }
    if segments.len() > MAX_TRASH_SET {
        return Err(format!(
            "too many segments in one set (max {MAX_TRASH_SET})"
        ));
    }

    let mut chosen = Vec::new();
    let mut failed = 0usize;
    for seg in segments {
        if seg.stream != PHOTOS_SOURCE
            || seg.day.len() != 8
            || !seg.day.bytes().all(|b| b.is_ascii_digit())
            || seg.segment.is_empty()
            || seg.segment.contains('/')
            || seg.segment.contains('\\')
            || seg.segment.contains("..")
        {
            failed += 1;
            continue;
        }
        let dir = journal
            .join("chronicle")
            .join(&seg.day)
            .join(&seg.stream)
            .join(&seg.segment);
        if !dir.is_dir() || dir.join("tombstone.json").is_file() {
            failed += 1;
            continue;
        }
        // Record durable suppression by asset_id before detaching into trash.
        let _ = record_suppression_for_segment(journal, &seg.day, &seg.stream, &seg.segment);
        chosen.push(solstone_core_trash::Chosen {
            unit: Unit::Segment {
                day: seg.day.clone(),
                stream: seg.stream.clone(),
                segment: seg.segment.clone(),
            },
            paths: vec![format!(
                "chronicle/{}/{}/{}",
                seg.day, seg.stream, seg.segment
            )],
            label: None,
        });
    }

    let mut moved = 0usize;
    let mut trash_ids = Vec::new();
    if !chosen.is_empty() {
        let surface = Surface::of_request(Page::Transcripts, user_agent);
        let results = solstone_core_import::trash_kinds::with_kinds(journal, |kinds| {
            solstone_core_trash::detach_set_with(
                journal,
                kinds,
                chosen,
                Action::SegmentDelete,
                surface,
                false,
                false,
            )
        })
        .map_err(|e| e.to_string())?;
        for res in results {
            match res {
                solstone_core_trash::SetResult::Moved { item, .. } => {
                    moved += 1;
                    trash_ids.push(item.id);
                }
                solstone_core_trash::SetResult::Refused(_) => {
                    failed += 1;
                }
            }
        }
    }

    Ok(TrashPhotosResult {
        moved,
        failed,
        trash_ids,
    })
}
