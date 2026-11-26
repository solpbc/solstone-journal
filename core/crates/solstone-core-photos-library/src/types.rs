// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Data structures for the Photos Library source holding area, selection
//! engine, wanted-list protocol, and dashboard state snapshots.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Apple PhotoKit collection subtypes (verified against PhotoKit runtime).
pub const ALBUM_REGULAR: i64 = 2;
pub const ALBUM_CLOUD_SHARED: i64 = 101;
pub const SMART_PANORAMAS: i64 = 201;
pub const SMART_VIDEOS: i64 = 202;
pub const SMART_FAVORITES: i64 = 203;
pub const SMART_HIDDEN: i64 = 205;
pub const SMART_BURSTS: i64 = 207;
pub const SMART_RECENTS: i64 = 209;
pub const SMART_SELFIES: i64 = 210;
pub const SMART_SCREENSHOTS: i64 = 211;
pub const SMART_PORTRAIT: i64 = 212;
pub const SMART_LIVE: i64 = 213;

/// iOS runtime smart-album subtypes guarded loudly by the contract check.
pub const SMART_RECENTLY_SAVED: i64 = 1_000_000_218;
pub const SMART_CAPTURED_BY_ME: i64 = 1_000_000_220;

/// Metadata-only album membership reference on a scanned asset.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct AssetAlbumRef {
    pub id: String,
    /// `"user"` or `"smart"`.
    pub kind: String,
    pub subtype: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Optional camera GPS metadata on a scanned asset.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AssetLocation {
    pub latitude: f64,
    pub longitude: f64,
    #[serde(
        default,
        alias = "horizontalAccuracy",
        alias = "horizontal_accuracy",
        skip_serializing_if = "Option::is_none"
    )]
    pub horizontal_accuracy_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub altitude: Option<f64>,
}

/// Extended optional metadata present on a scanned asset (never pixel bytes).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AssetExtended {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uti: Option<String>,
    #[serde(
        default,
        rename = "originalFilename",
        alias = "original_filename",
        skip_serializing_if = "Option::is_none"
    )]
    pub original_filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
}

/// One metadata-only scanned asset row from a client photo library.
/// Never carries pixel bytes. Hidden assets are refused/dropped at ingest.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ScanAssetRow {
    pub id: String,
    #[serde(
        default,
        rename = "cloudId",
        alias = "cloud_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub cloud_id: Option<String>,
    #[serde(
        default,
        rename = "capturedAt",
        alias = "captured_at",
        skip_serializing_if = "Option::is_none"
    )]
    pub captured_at: Option<String>,
    #[serde(
        default,
        rename = "modifiedAt",
        alias = "modified_at",
        skip_serializing_if = "Option::is_none"
    )]
    pub modified_at: Option<String>,
    #[serde(default, rename = "mediaType", alias = "media_type")]
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtypes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub edited: bool,
    #[serde(default)]
    pub screenshot: bool,
    #[serde(default, rename = "livePhoto", alias = "live_photo")]
    pub live_photo: bool,
    #[serde(default)]
    pub panorama: bool,
    #[serde(default)]
    pub hdr: bool,
    #[serde(default)]
    pub depth: bool,
    #[serde(
        default,
        rename = "burstIdentifier",
        alias = "burst_identifier",
        skip_serializing_if = "Option::is_none"
    )]
    pub burst_identifier: Option<String>,
    #[serde(default, rename = "representsBurst", alias = "represents_burst")]
    pub represents_burst: bool,
    #[serde(
        default,
        rename = "burstSelection",
        alias = "burst_selection",
        skip_serializing_if = "Option::is_none"
    )]
    pub burst_selection: Option<u64>,
    #[serde(
        default,
        rename = "sourceType",
        alias = "source_type",
        skip_serializing_if = "Option::is_none"
    )]
    pub source_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<AssetLocation>,
    #[serde(default)]
    pub albums: Vec<AssetAlbumRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extended: Option<AssetExtended>,
}

/// Entry in the client's smart-album catalog reported at scan start.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SmartAlbumCatalogEntry {
    pub id: String,
    pub subtype: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub count: u64,
}

/// Close-shot representative strategy (metadata-only scan uses `Middle`).
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollapsePickStrategy {
    First,
    #[default]
    Middle,
    Last,
}

/// All deterministic selection parameters (serialized into durable receipts).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SelectionConfig {
    pub version: String,
    pub owner_marks_beat_rule1: bool,
    /// When true (v2 default), an `edited`-only mark still beats Rule 1 for
    /// admission, while collapsing inside `<= collapse_gap_seconds` close groups
    /// so iOS auto-adjustments on consecutive shots do not create duplicates.
    pub edited_only_collapses_in_close_group: bool,
    pub collapse_gap_seconds: f64,
    pub collapse_max_span_seconds: Option<f64>,
    pub collapse_pick_strategy: CollapsePickStrategy,
    pub moment_gap_seconds: f64,
    pub moment_cap_routine: Option<usize>,
    pub moment_cap_away: Option<usize>,
    pub routine_cluster_radius_km: f64,
    pub routine_window_days: i64,
    pub routine_min_days: usize,
    pub routine_min_months: usize,
    pub routine_min_span_days: i64,
    pub nearest_tz_max_hours: f64,
    /// Temporal corroboration window around own moments for photos others took
    /// (default: 7200 s = ±2 hours).
    pub others_window_seconds: f64,
}

impl Default for SelectionConfig {
    fn default() -> Self {
        Self::v2()
    }
}

impl SelectionConfig {
    /// Production Rules v2 configuration calibrated on real photo libraries.
    #[must_use]
    pub fn v2() -> Self {
        Self {
            version: "v2".to_owned(),
            owner_marks_beat_rule1: true,
            edited_only_collapses_in_close_group: true,
            collapse_gap_seconds: 10.0,
            collapse_max_span_seconds: Some(10.0),
            collapse_pick_strategy: CollapsePickStrategy::Middle,
            moment_gap_seconds: 300.0,
            moment_cap_routine: Some(3),
            moment_cap_away: Some(5),
            routine_cluster_radius_km: 1.5,
            routine_window_days: 365,
            routine_min_days: 5,
            routine_min_months: 3,
            routine_min_span_days: 60,
            nearest_tz_max_hours: 12.0,
            others_window_seconds: 7200.0,
        }
    }
}

/// Durable owner selection overrides stored at `selection/corrections.json`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct OwnerCorrections {
    #[serde(default = "default_one")]
    pub version: u32,
    #[serde(default)]
    pub always_albums: Vec<String>,
    #[serde(default)]
    pub never_albums: Vec<String>,
    #[serde(default)]
    pub keep_assets: Vec<String>,
    #[serde(default)]
    pub drop_assets: Vec<String>,
    #[serde(default)]
    pub place_routine_overrides: BTreeMap<String, bool>,
}

const fn default_one() -> u32 {
    1
}

/// Deterministic decision assigned to each scanned asset.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    /// Selected to be asked for and kept (unless the describe call classifies it as noise).
    Keep,
    /// Unmarked photo others took within ±2 h of an own moment: staged for local
    /// side-effect-free face matching against meeting faceprints before keeping.
    StageOthersFaceCheck,
    /// Left out by deterministic rules (never asked for).
    LeftOut,
}

/// One per-asset deterministic selection record (zero image bytes).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AssetDecision {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_id: Option<String>,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
    pub captured_ts: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_iso: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_hm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utc_offset_seconds: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coarse_place: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place_cluster_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub away_from_routine: Option<bool>,
    pub in_cbm: bool,
    pub in_saved: bool,
    pub in_cloud_shared: bool,
    pub admitted_via_owner_mark: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admitted_via_mark_source: Option<String>,
    pub screenshot: bool,
    pub owner_marked: bool,
    pub owner_marks: Vec<String>,
    pub decision: DecisionKind,
    pub reason: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed_into: Option<String>,
    /// Nearby own moment id when `decision == DecisionKind::StageOthersFaceCheck`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub near_moment_id: Option<String>,
}

/// Durable left-out record (no image content).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LeftOutRecord {
    pub id: String,
    pub reason: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed_into: Option<String>,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_iso: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coarse_place: Option<String>,
}

/// One five-minute owner moment record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MomentRecord {
    pub moment_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_local_iso: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_local_iso: Option<String>,
    pub start_ts: f64,
    pub end_ts: f64,
    pub duration_seconds: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coarse_place: Option<String>,
    pub away_from_routine: bool,
    pub has_owner_mark: bool,
    pub raw_count: usize,
    pub close_group_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap_applied: Option<usize>,
    pub kept_ids: Vec<String>,
    pub dropped_ids: Vec<String>,
}

/// Learned routine-place cluster.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RoutineCluster {
    pub cluster_id: String,
    pub lat: f64,
    pub lon: f64,
    pub days: usize,
    pub months: usize,
    pub span_days: i64,
    pub photos: usize,
    pub dist_home_km: f64,
    pub is_routine: bool,
    pub is_home: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coarse_place: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

/// Learned routine-place model persisted at `selection/routine_places.json`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RoutinePlacesModel {
    pub cluster_radius_km: f64,
    pub window_days: i64,
    pub data_span_days: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_cluster_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_timezone: Option<String>,
    pub routine_count: usize,
    pub total_clusters: usize,
    pub clusters: Vec<RoutineCluster>,
}

/// Aggregate selection summary persisted at `selection/summary.json`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SelectionSummary {
    pub config: SelectionConfig,
    pub total_assets: usize,
    pub eligible_own_stills: usize,
    pub mark_promoted_total: usize,
    pub mark_promoted_by_source: BTreeMap<String, usize>,
    pub close_groups: usize,
    pub moments: usize,
    pub kept: usize,
    pub staged_others_face_check: usize,
    pub left_out: usize,
    pub keep_by_reason: BTreeMap<String, usize>,
    pub left_out_by_reason: BTreeMap<String, usize>,
    pub routine_places_learned: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_timezone: Option<String>,
    pub decisions_sha256: String,
    pub left_out_sha256: String,
    pub moments_sha256: String,
}

/// Complete deterministic selection run output.
#[derive(Clone, Debug)]
pub struct SelectionOutput {
    pub summary: SelectionSummary,
    pub decisions: Vec<AssetDecision>,
    pub left_out: Vec<LeftOutRecord>,
    pub moments: Vec<MomentRecord>,
    pub routine_places: RoutinePlacesModel,
}

/// Range choice for historical lookback (supports standard presets or custom dates).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum RangeSpec {
    Preset(String),
    Custom {
        since: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<String>,
    },
}

impl RangeSpec {
    /// Normalize preset string (`"30d"`, `"6m"`, `"1y"`, `"all"`, `"none"`).
    pub fn canonical_key(&self) -> String {
        match self {
            Self::Preset(s) => s.trim().to_ascii_lowercase(),
            Self::Custom { since, until } => match until {
                Some(u) => format!("custom:{since}..{u}"),
                None => format!("custom:{since}.."),
            },
        }
    }
}

/// Per-device metadata and sync state tracked in `devices.json`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct DeviceState {
    pub cid: String,
    pub name: String,
    /// `"iphone"`, `"ipad"`, `"mac"`, etc.
    pub kind: String,
    pub connected: bool,
    /// `"authorized"`, `"limited"`, `"denied"`, `"restricted"`, `"not_determined"`.
    pub access_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_scan_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_upload_at: Option<String>,
    #[serde(default)]
    pub icloud_waiting_count: u64,
}

/// One wanted photo item leased to a paired device.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct WantedItem {
    pub asset_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_id: Option<String>,
    pub max_edge: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
    /// `"keep"` or `"others_face_check"`.
    pub reason_kind: String,
}

/// Response returned by `GET /app/devices/photos-library/wanted`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct WantedResponse {
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    pub wanted: Vec<WantedItem>,
    pub remaining: usize,
}

/// Dashboard left-out counts grouped into VPX's owner-facing buckets.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct LeftOutBreakdown {
    pub total: usize,
    pub reasons: BTreeMap<String, usize>,
}

/// Dashboard per-range estimate (zero money figures: photos, days, thoughts only).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct RangeEstimate {
    pub photos: usize,
    pub days: usize,
    pub thoughts: usize,
    pub delta_photos: usize,
    pub delta_days: usize,
    pub delta_thoughts: usize,
    pub covered: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earliest_date: Option<String>,
    pub left_out: LeftOutBreakdown,
}

/// Scan progress block on `GET /app/sources/photos/api/state`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct ScanProgressView {
    pub items_checked: usize,
    pub items_total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earliest_item_at: Option<String>,
    pub photos_brought_in_so_far: usize,
    pub paused: bool,
}

/// Active range choice block on `GET /app/sources/photos/api/state`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct ChoiceView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_start_at: Option<String>,
    pub can_widen: bool,
}

/// Arrival and journal count block on `GET /app/sources/photos/api/state`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct ProgressView {
    pub chosen_total: usize,
    pub arrived: usize,
    pub in_journal: usize,
    pub waiting_on_phone: usize,
    pub moved_to_trash_by_owner: usize,
}

/// Complete dashboard state snapshot matching the locked `/app/sources/photos/api/state` contract.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct PhotosDashboardState {
    pub source: String,
    pub state: String,
    pub substate: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_reason: Option<String>,
    pub devices: Vec<DeviceState>,
    pub scan: ScanProgressView,
    pub choice: ChoiceView,
    pub ranges: BTreeMap<String, RangeEstimate>,
    pub left_out: LeftOutBreakdown,
    pub progress: ProgressView,
    pub band4_paused: bool,
    pub thinking_ready: bool,
    pub profile_pics_enabled: bool,
}

/// One landed library photo item returned by `GET /app/sources/photos/api/photos`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct LandedPhotoItem {
    pub day: String,
    pub stream: String,
    pub segment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
    pub thumb_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    pub has_location: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
}

/// Paginated response for `GET /app/sources/photos/api/photos`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct LandedPhotosPage {
    pub months: Vec<String>,
    pub items: Vec<LandedPhotoItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}
