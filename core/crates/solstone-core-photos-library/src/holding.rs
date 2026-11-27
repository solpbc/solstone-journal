// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private Photos Library holding area (`config/photos-library/`), rebuildable
//! SQLite query cache, lease-based wanted list, range estimates, and dashboard
//! state snapshot builder.
//!
//! Durability & privacy contract:
//! - Plain JSON files under `config/photos-library/` are the sole durable
//!   authority for scanned metadata, selection decisions, left-out counts,
//!   moments, learned routine places, and wanted-list progress.
//! - `config/photos-library/selection/selection_cache.sqlite` is a pure cache:
//!   deleting and rebuilding it produces identical decisions and SHA-256 digest.
//! - Never indexed, searched, shown, or offered to agents.
//! - `turn_off()` removes `config/photos-library/` completely while keeping all
//!   landed `chronicle/` photos and `config/photos/received/` suppression intact.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use jiff::{SignedDuration, Timestamp};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use solstone_core_entity::faceprints;
use solstone_core_journal_io::locking::{FileLock, LockOptions, hold_lock};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, create_directory_with_mode};
use solstone_core_segment::photo_observation::read_or_project_photo_observation;

use crate::geo::{GeoResolver, parse_utc_ts};
use crate::received::{PhotoReceipt, ReceivedLedgerSnapshot};
use crate::selection::{aggregate_left_out_buckets, run_selection};
use crate::types::{
    AssetDecision, ChoiceView, DecisionKind, DeviceState, LandedPhotoItem, LandedPhotosPage,
    LeftOutBreakdown, OwnerCorrections, PhotosDashboardState, ProgressView, RangeEstimate,
    RangeSpec, SMART_HIDDEN, ScanAssetRow, ScanProgressView, SelectionConfig, SelectionOutput,
    SelectionSummary, SmartAlbumCatalogEntry, WantedItem, WantedResponse,
};

pub const HOLDING_DIR_REL: &str = "config/photos-library";
const TURNED_OFF_MARKER_REL: &str = "config/photos/turned_off.json";
const CAPABLE_DEVICES_REL: &str = "config/photos/capable_devices.json";
const CAPABLE_DEVICES_LOCK_REL: &str = "config/photos/.capable-devices.lock";
const LOCK_NAME: &str = ".photos-library.lock";
const PRIVATE_DIR_MODE: u32 = 0o700;
const PRIVATE_FILE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };
const DEFAULT_LEASE_SECONDS: i64 = 300;
const MAX_BATCH_ROWS: usize = 5_000;
const MAX_WANTED_LIMIT: usize = 100;
const PHOTOS_PAGE_SIZE: usize = 60;
const SQLITE_CACHE_SCHEMA_VERSION: &str = "2";
const DECISIONS_CACHE_CAP: usize = 8;
/// Presence window matching `solstone_core_sol_link::CLIENT_STALE_MS` (120 s).
pub const DEVICE_CONNECTED_WINDOW_SECS: i64 = 120;
const DEVICE_CONNECTED_WINDOW_MS: i64 = DEVICE_CONNECTED_WINDOW_SECS * 1_000;
const DEVICE_FUTURE_CLOCK_DRIFT_MS: i64 = 60_000;

type DecisionsCacheEntries = Vec<(PathBuf, Arc<CachedDecisions>)>;

static SELECTION_CACHE_GEN: AtomicU64 = AtomicU64::new(1);
static DECISIONS_CACHE: LazyLock<Mutex<DecisionsCacheEntries>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Feature flag in `config/journal.json` (`photos_library.enabled`).
/// ON by default: absent config or absent key reads as on, `false` or any
/// non-boolean value reads as off.
#[must_use]
pub fn photos_library_enabled(journal: &Path) -> bool {
    let bytes = match fs::read(journal.join("config/journal.json")) {
        Ok(b) => b,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return true,
        Err(_) => return false,
    };
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    match val.get("photos_library").and_then(|s| s.get("enabled")) {
        None => true,
        Some(v) => v == &serde_json::Value::Bool(true),
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct CapableDeviceRecord {
    cid: String,
    name: String,
    /// Normalized `"iphone"` or `"ipad"` (phase 1 is iPhone/iPad only).
    kind: String,
    last_seen_at: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct CapableDevicesFile {
    schema: u32,
    devices: BTreeMap<String, CapableDeviceRecord>,
}

fn normalize_capable_device_kind(kind: Option<&str>) -> Option<&'static str> {
    match kind.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("iphone") => Some("iphone"),
        Some("ipad") => Some("ipad"),
        _ => None,
    }
}

fn lock_capable_devices(journal: &Path) -> Result<FileLock, String> {
    let photos_dir = journal.join("config/photos");
    create_directory_with_mode(&photos_dir, PRIVATE_DIR_MODE).map_err(|e| e.to_string())?;
    hold_lock(
        journal.join(CAPABLE_DEVICES_LOCK_REL),
        LockOptions {
            timeout: Duration::from_secs(15),
            ..LockOptions::default()
        },
    )
    .map_err(|e| e.to_string())
}

/// Record that a paired iPhone or iPad running a photos-capable app reported in.
/// Stored outside `config/photos-library/` so reporting capability never creates
/// a holding area before the owner turns Photos on, and survives `turn_off()`.
pub fn record_capable_device(
    journal: &Path,
    cid: &str,
    device_name: Option<&str>,
    device_kind: Option<&str>,
) -> Result<bool, String> {
    let Some(kind) = normalize_capable_device_kind(device_kind) else {
        return Ok(false);
    };
    let _lock = lock_capable_devices(journal)?;
    let path = journal.join(CAPABLE_DEVICES_REL);
    let mut file: CapableDevicesFile =
        read_json_optional(&path)?.unwrap_or_else(|| CapableDevicesFile {
            schema: 1,
            devices: BTreeMap::new(),
        });
    let now = solstone_core_time::utc_rfc3339(Timestamp::now());
    let entry = file.devices.entry(cid.to_owned()).or_default();
    entry.cid = cid.to_owned();
    if let Some(n) = device_name
        && !n.trim().is_empty()
    {
        entry.name = n.trim().to_owned();
    }
    entry.kind = kind.to_owned();
    entry.last_seen_at = now;
    write_json_private(&path, &file)?;
    Ok(true)
}

/// Remove a device from the capable-device registry (e.g. on unpair).
pub fn remove_capable_device(journal: &Path, cid: &str) -> Result<bool, String> {
    let path = journal.join(CAPABLE_DEVICES_REL);
    if !path.is_file() {
        return Ok(false);
    }
    let _lock = lock_capable_devices(journal)?;
    let mut file: CapableDevicesFile = match read_json_optional(&path)? {
        Some(f) => f,
        None => return Ok(false),
    };
    let removed = file.devices.remove(cid).is_some();
    if removed {
        write_json_private(&path, &file)?;
    }
    Ok(removed)
}

fn read_authorized_client_cids(journal: &Path) -> Option<BTreeSet<String>> {
    let path = journal.join("link/authorized_clients.json");
    let bytes = fs::read(path).ok()?;
    let arr = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()?
        .as_array()?
        .clone();
    let mut cids = BTreeSet::new();
    for item in arr {
        let Some(obj) = item.as_object() else {
            continue;
        };
        if obj.get("kind").is_some_and(|k| k != "cert") {
            continue;
        }
        if let Some(fp) = obj.get("fingerprint").and_then(serde_json::Value::as_str) {
            cids.insert(fp.to_owned());
        }
    }
    Some(cids)
}

fn read_link_device_last_seen(journal: &Path) -> BTreeMap<String, String> {
    let path = journal.join("link/devices.json");
    let Ok(bytes) = fs::read(path) else {
        return BTreeMap::new();
    };
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return BTreeMap::new();
    };
    let Some(obj) = val.as_object() else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for (cid, entry) in obj {
        if let Some(ts) = entry
            .get("last_seen_at")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            out.insert(cid.clone(), ts.to_owned());
        }
    }
    out
}

/// Distinct capable device kinds (`"iphone"`, `"ipad"`) among currently paired
/// devices that have reported in.
#[must_use]
pub fn capable_device_kinds(journal: &Path) -> Vec<String> {
    let path = journal.join(CAPABLE_DEVICES_REL);
    let Ok(Some(file)) = read_json_optional::<CapableDevicesFile>(&path) else {
        return Vec::new();
    };
    let authorized = read_authorized_client_cids(journal);
    let mut has_iphone = false;
    let mut has_ipad = false;
    for rec in file.devices.values() {
        if let Some(auth) = authorized.as_ref()
            && !auth.contains(&rec.cid)
        {
            continue;
        }
        match normalize_capable_device_kind(Some(&rec.kind)) {
            Some("iphone") => has_iphone = true,
            Some("ipad") => has_ipad = true,
            _ => {}
        }
    }
    let mut kinds = Vec::new();
    if has_iphone {
        kinds.push("iphone".to_owned());
    }
    if has_ipad {
        kinds.push("ipad".to_owned());
    }
    kinds
}

fn has_existing_photos_state(journal: &Path) -> bool {
    holding_root(journal).is_dir()
        || journal.join("config/photos/received").is_dir()
        || journal.join(TURNED_OFF_MARKER_REL).is_file()
        || has_any_live_photo_in_journal(journal)
}

/// Whether the Photos row (`/app/sources/`) and page (`/app/sources/photos`)
/// are visible on this journal: requires `photos_library.enabled` AND either a
/// paired photos-capable iPhone/iPad that has reported in, or existing photos
/// holding/landed/turned-off state.
#[must_use]
pub fn photos_library_visible(journal: &Path) -> bool {
    if !photos_library_enabled(journal) {
        return false;
    }
    !capable_device_kinds(journal).is_empty() || has_existing_photos_state(journal)
}

/// Fast O(1) attention check for shell badge polling (`GET /api/shell/badges`).
///
/// Equivalent to `photos_library_visible(journal) && dashboard_state(journal).state == "needs_attention"`,
/// without reading `decisions.json`, scanning `chronicle/*/photos/`, or loading the face index.
#[must_use]
pub fn photos_needs_attention(journal: &Path) -> bool {
    if !photos_library_enabled(journal) || !holding_root(journal).is_dir() {
        return false;
    }
    load_holding_state(journal)
        .ok()
        .is_some_and(|s| s.attention_reason.is_some())
}

#[must_use]
pub fn holding_root(journal: &Path) -> PathBuf {
    journal.join(HOLDING_DIR_REL)
}

#[must_use]
pub fn sqlite_cache_path(journal: &Path) -> PathBuf {
    holding_root(journal).join("selection/selection_cache.sqlite")
}

fn ensure_holding_dirs(journal: &Path) -> Result<PathBuf, String> {
    let config_dir = journal.join("config");
    create_directory_with_mode(&config_dir, PRIVATE_DIR_MODE).map_err(|e| e.to_string())?;
    let root = holding_root(journal);
    create_directory_with_mode(&root, PRIVATE_DIR_MODE).map_err(|e| e.to_string())?;
    for sub in ["scan", "selection", "wanted"] {
        create_directory_with_mode(&root.join(sub), PRIVATE_DIR_MODE).map_err(|e| e.to_string())?;
    }
    Ok(root)
}

pub(crate) fn lock_holding(journal: &Path) -> Result<FileLock, String> {
    let root = ensure_holding_dirs(journal)?;
    let lock_path = root.join(LOCK_NAME);
    hold_lock(
        &lock_path,
        LockOptions {
            timeout: Duration::from_secs(15),
            ..LockOptions::default()
        },
    )
    .map_err(|e| e.to_string())
}

fn write_json_private<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        create_directory_with_mode(parent, PRIVATE_DIR_MODE).map_err(|e| e.to_string())?;
    }
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    atomic_replace(path, &bytes, PRIVATE_FILE).map_err(|e| e.to_string())
}

fn write_json_compact_private<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        create_directory_with_mode(parent, PRIVATE_DIR_MODE).map_err(|e| e.to_string())?;
    }
    let mut bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    atomic_replace(path, &bytes, PRIVATE_FILE).map_err(|e| e.to_string())
}

fn read_json_optional<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("corrupt {}: {e}", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("read {}: {err}", path.display())),
    }
}

/// Durable holding-area lifecycle state (`config/photos-library/state.json`).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HoldingState {
    pub schema: u32,
    pub scan_active: bool,
    pub scan_complete: bool,
    pub scan_paused: bool,
    pub scan_items_checked: usize,
    pub scan_items_total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_earliest_item_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_scan_at: Option<String>,
    /// Selected lookback range (`"30d"`, `"6m"`, `"1y"`, `"all"`, `"none"`, or `"custom:..."`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen_range: Option<String>,
    /// Inclusive UTC RFC 3339 lower bound for the chosen range (`None` for `"all"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_start_at: Option<String>,
    /// Optional inclusive UTC RFC 3339 upper bound for custom ranges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_end_at: Option<String>,
    /// UTC timestamp when the owner confirmed their choice (used as forward-only floor for `"none"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen_at: Option<String>,
    pub paused: bool,
    /// `"access_limited"`, `"access_off"`, `"smart_album_contract_failed"`, etc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// Durable per-asset wanted/arrival tracking (`config/photos-library/wanted/state.json`).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WantedAssetStatus {
    Leased {
        cid: String,
        leased_at: String,
        expires_at: String,
    },
    Landed {
        day: String,
        stream: String,
        segment: String,
        at: String,
    },
    NoiseDropped {
        at: String,
    },
    OthersNoFaceMatch {
        at: String,
    },
    Gone {
        at: String,
    },
    IcloudWaiting {
        cid: String,
        at: String,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct WantedStateFile {
    pub schema: u32,
    pub assets: BTreeMap<String, WantedAssetStatus>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct DevicesFile {
    schema: u32,
    devices: BTreeMap<String, DeviceState>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ScanBatchFile {
    batch_index: u32,
    rows: Vec<ScanAssetRow>,
}

fn filter_non_hidden(rows: Vec<ScanAssetRow>) -> Vec<ScanAssetRow> {
    rows.into_iter()
        .filter(|r| {
            !r.hidden
                && !r
                    .albums
                    .iter()
                    .any(|a| a.kind == "smart" && a.subtype == SMART_HIDDEN)
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileFingerprint {
    len: u64,
    mtime_ns: i128,
    ino: u64,
}

fn file_fingerprint(path: &Path) -> io::Result<Option<FileFingerprint>> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i128);
    #[cfg(unix)]
    let ino = {
        use std::os::unix::fs::MetadataExt;
        meta.ino()
    };
    #[cfg(not(unix))]
    let ino = 0u64;
    Ok(Some(FileFingerprint {
        len: meta.len(),
        mtime_ns,
        ino,
    }))
}

#[derive(Clone, Debug)]
struct CompactDecision {
    id: String,
    cloud_id: Option<String>,
    captured_at: Option<String>,
    captured_ts: f64,
    local_date: Option<String>,
    decision: DecisionKind,
    reason: String,
}

struct CachedDecisions {
    fingerprint: Option<FileFingerprint>,
    generation: u64,
    all: Vec<CompactDecision>,
    /// Indices into `all` of `Keep` and `StageOthersFaceCheck`, sorted newest-first
    /// (`captured_ts DESC`, `id ASC`) so `lease_wanted` never rescans or re-sorts.
    wanted_order: Vec<usize>,
}

impl CachedDecisions {
    fn from_compact(
        fingerprint: Option<FileFingerprint>,
        generation: u64,
        all: Vec<CompactDecision>,
    ) -> Self {
        let mut wanted_order = Vec::new();
        for (idx, d) in all.iter().enumerate() {
            match d.decision {
                DecisionKind::Keep | DecisionKind::StageOthersFaceCheck => {
                    wanted_order.push(idx);
                }
                DecisionKind::LeftOut => {}
            }
        }
        wanted_order.sort_by(|&a, &b| {
            let da = &all[a];
            let db = &all[b];
            db.captured_ts
                .total_cmp(&da.captured_ts)
                .then_with(|| da.id.cmp(&db.id))
        });
        Self {
            fingerprint,
            generation,
            all,
            wanted_order,
        }
    }

    fn from_decisions(
        fingerprint: Option<FileFingerprint>,
        generation: u64,
        decisions: &[AssetDecision],
    ) -> Self {
        let all = decisions
            .iter()
            .map(|d| CompactDecision {
                id: d.id.clone(),
                cloud_id: d.cloud_id.clone(),
                captured_at: d.captured_at.clone(),
                captured_ts: d.captured_ts,
                local_date: d.local_date.clone(),
                decision: d.decision,
                reason: d.reason.clone(),
            })
            .collect();
        Self::from_compact(fingerprint, generation, all)
    }

    fn empty(generation: u64) -> Self {
        Self {
            fingerprint: None,
            generation,
            all: Vec::new(),
            wanted_order: Vec::new(),
        }
    }
}

fn decisions_cache_key(journal: &Path) -> PathBuf {
    let p = holding_root(journal).join("selection/decisions.json");
    fs::canonicalize(&p).unwrap_or(p)
}

fn invalidate_decisions_cache(journal: &Path) {
    SELECTION_CACHE_GEN.fetch_add(1, Ordering::AcqRel);
    let raw = holding_root(journal).join("selection/decisions.json");
    let canon = fs::canonicalize(&raw).ok();
    if let Ok(mut cache) = DECISIONS_CACHE.lock() {
        cache.retain(|(k, _)| k != &raw && canon.as_ref().is_none_or(|c| k != c));
    }
}

fn store_decisions_cache(key: PathBuf, entry: Arc<CachedDecisions>) {
    if let Ok(mut cache) = DECISIONS_CACHE.lock() {
        if cache
            .iter()
            .any(|(k, existing)| k == &key && existing.generation > entry.generation)
        {
            return;
        }
        cache.retain(|(k, _)| k != &key);
        if cache.len() >= DECISIONS_CACHE_CAP {
            cache.remove(0);
        }
        cache.push((key, entry));
    }
}

fn parse_decision_kind(raw: &str) -> DecisionKind {
    match raw {
        "keep" => DecisionKind::Keep,
        "stage_others_face_check" => DecisionKind::StageOthersFaceCheck,
        _ => DecisionKind::LeftOut,
    }
}

fn open_verified_sqlite_cache(db_path: &Path, expected: FileFingerprint) -> Option<Connection> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let read_meta = |key: &str| -> Option<String> {
        conn.query_row(
            "SELECT value FROM cache_meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
    };
    if read_meta("schema_version").as_deref() != Some(SQLITE_CACHE_SCHEMA_VERSION) {
        return None;
    }
    if read_meta("decisions_len")?.parse::<u64>().ok()? != expected.len {
        return None;
    }
    if read_meta("decisions_mtime_ns")?.parse::<i128>().ok()? != expected.mtime_ns {
        return None;
    }
    if read_meta("decisions_ino")?.parse::<u64>().ok()? != expected.ino {
        return None;
    }
    Some(conn)
}

fn load_compact_decisions_from_sqlite(
    journal: &Path,
    expected: FileFingerprint,
) -> Option<Vec<CompactDecision>> {
    let conn = open_verified_sqlite_cache(&sqlite_cache_path(journal), expected)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, cloud_id, captured_at_utc, captured_ts, local_date, decision, reason FROM assets",
        )
        .ok()?;
    let rows = stmt
        .query_map([], |row| {
            let dec_str: String = row.get(5)?;
            Ok(CompactDecision {
                id: row.get(0)?,
                cloud_id: row.get(1)?,
                captured_at: row.get(2)?,
                captured_ts: row.get(3)?,
                local_date: row.get(4)?,
                decision: parse_decision_kind(&dec_str),
                reason: row.get(6)?,
            })
        })
        .ok()?;
    let mut out = Vec::new();
    for item in rows {
        out.push(item.ok()?);
    }
    Some(out)
}

fn load_cached_decisions(journal: &Path) -> Result<Arc<CachedDecisions>, String> {
    let dec_path = holding_root(journal).join("selection/decisions.json");
    let fp =
        file_fingerprint(&dec_path).map_err(|e| format!("stat {}: {e}", dec_path.display()))?;
    let generation = SELECTION_CACHE_GEN.load(Ordering::Acquire);
    let Some(fp) = fp else {
        invalidate_decisions_cache(journal);
        return Ok(Arc::new(CachedDecisions::empty(generation)));
    };
    let key = decisions_cache_key(journal);
    if let Ok(cache) = DECISIONS_CACHE.lock()
        && let Some((_, existing)) = cache.iter().find(|(k, _)| k == &key)
        && existing.fingerprint == Some(fp)
        && existing.generation == generation
    {
        return Ok(Arc::clone(existing));
    }

    let built = if let Some(compact) = load_compact_decisions_from_sqlite(journal, fp) {
        Arc::new(CachedDecisions::from_compact(Some(fp), generation, compact))
    } else {
        let decisions = load_decisions(journal)?;
        Arc::new(CachedDecisions::from_decisions(
            Some(fp),
            generation,
            &decisions,
        ))
    };
    store_decisions_cache(key, Arc::clone(&built));
    Ok(built)
}

/// Narrow single-asset metadata needed by `ingest_photo_copy_with`, fetched in
/// O(1) from `selection_cache.sqlite` (rebuilding once from plain JSON if the
/// cache is absent or older than `decisions.json`).
#[derive(Clone, Debug, Default)]
pub(crate) struct CopyAssetLookup {
    pub decision: Option<DecisionKind>,
    pub cloud_id: Option<String>,
    pub timezone: Option<String>,
    pub local_iso: Option<String>,
    pub captured_at: Option<String>,
    pub caption: Option<String>,
    pub original_filename: Option<String>,
    pub camera: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub horizontal_accuracy_m: Option<f64>,
}

fn copy_lookup_from_rows(
    decision: Option<&AssetDecision>,
    scan_row: Option<&ScanAssetRow>,
) -> Option<CopyAssetLookup> {
    if decision.is_none() && scan_row.is_none() {
        return None;
    }
    let ext = scan_row.and_then(|r| r.extended.as_ref());
    let loc = scan_row.and_then(|r| r.location.as_ref());
    Some(CopyAssetLookup {
        decision: decision.map(|d| d.decision),
        cloud_id: decision
            .and_then(|d| d.cloud_id.clone())
            .or_else(|| scan_row.and_then(|r| r.cloud_id.clone())),
        timezone: decision.and_then(|d| d.timezone.clone()),
        local_iso: decision.and_then(|d| d.local_iso.clone()),
        captured_at: decision
            .and_then(|d| d.captured_at.clone())
            .or_else(|| scan_row.and_then(|r| r.captured_at.clone())),
        caption: ext.and_then(|e| e.caption.clone()),
        original_filename: ext.and_then(|e| e.original_filename.clone()),
        camera: ext.and_then(|e| e.camera.clone()),
        latitude: loc.map(|l| l.latitude),
        longitude: loc.map(|l| l.longitude),
        horizontal_accuracy_m: loc.and_then(|l| l.horizontal_accuracy_m),
    })
}

/// Look up one asset's decision and scan metadata for Keep & Read ingest.
/// Caller holds [`lock_holding`].
pub(crate) fn lookup_asset_for_copy(
    journal: &Path,
    asset_id: &str,
) -> Result<Option<CopyAssetLookup>, String> {
    let dec_path = holding_root(journal).join("selection/decisions.json");
    let fp =
        file_fingerprint(&dec_path).map_err(|e| format!("stat {}: {e}", dec_path.display()))?;
    if let Some(fp) = fp {
        if let Some(conn) = open_verified_sqlite_cache(&sqlite_cache_path(journal), fp) {
            let found = conn
                .query_row(
                    "SELECT decision, cloud_id, timezone, local_iso, captured_at_utc, \
                     caption, original_filename, camera, latitude, longitude, horizontal_accuracy_m \
                     FROM assets WHERE id = ?1",
                    params![asset_id],
                    |row| {
                        let dec_str: String = row.get(0)?;
                        Ok(CopyAssetLookup {
                            decision: Some(parse_decision_kind(&dec_str)),
                            cloud_id: row.get(1)?,
                            timezone: row.get(2)?,
                            local_iso: row.get(3)?,
                            captured_at: row.get(4)?,
                            caption: row.get(5)?,
                            original_filename: row.get(6)?,
                            camera: row.get(7)?,
                            latitude: row.get(8)?,
                            longitude: row.get(9)?,
                            horizontal_accuracy_m: row.get(10)?,
                        })
                    },
                )
                .optional();
            if let Ok(row_opt) = found {
                return Ok(row_opt);
            }
        }
        // SQLite cache missing, stale, or pre-v2 (e.g. first photo after upgrading from 2.0.92/2.0.93):
        // load plain JSON once and rebuild the SQLite cache so subsequent photos stay O(1).
        let decisions = load_decisions(journal)?;
        let scan_rows = load_scan_rows(journal)?;
        if let Err(err) = rebuild_sqlite_index(&sqlite_cache_path(journal), &scan_rows, &decisions)
        {
            log::warn!("photos selection sqlite cache refresh skipped: {err}");
        }
        let decision = decisions.iter().find(|d| d.id == asset_id);
        let scan_row = scan_rows.iter().find(|r| r.id == asset_id);
        return Ok(copy_lookup_from_rows(decision, scan_row));
    }

    // No `decisions.json` yet: check plain scan batches directly if present.
    let scan_rows = load_scan_rows(journal)?;
    let scan_row = scan_rows.iter().find(|r| r.id == asset_id);
    Ok(copy_lookup_from_rows(None, scan_row))
}

/// Rebuild the fast SQLite query cache from plain JSON scan & decision files.
/// Safe to delete and call at any time; never owns authoritative state.
pub fn rebuild_sqlite_index(
    db_path: &Path,
    rows: &[ScanAssetRow],
    decisions: &[AssetDecision],
) -> Result<(), String> {
    if let Some(parent) = db_path.parent() {
        create_directory_with_mode(parent, PRIVATE_DIR_MODE).map_err(|e| e.to_string())?;
    }
    let decisions_fp = db_path.parent().and_then(|parent| {
        file_fingerprint(&parent.join("decisions.json"))
            .ok()
            .flatten()
    });
    let tmp_path = db_path.with_extension("sqlite.tmp");
    let _ = fs::remove_file(&tmp_path);

    let mut conn = Connection::open(&tmp_path).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "PRAGMA synchronous = OFF;
        PRAGMA journal_mode = OFF;
        CREATE TABLE cache_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE assets (
            id TEXT PRIMARY KEY,
            cloud_id TEXT,
            captured_at_utc TEXT,
            captured_ts REAL,
            local_date TEXT,
            local_iso TEXT,
            timezone TEXT,
            coarse_place TEXT,
            away_from_routine INTEGER,
            media_type TEXT,
            in_cbm INTEGER,
            in_saved INTEGER,
            screenshot INTEGER,
            owner_marked INTEGER,
            marks_json TEXT,
            decision TEXT,
            reason TEXT,
            moment_id TEXT,
            collapsed_into TEXT,
            near_moment_id TEXT,
            caption TEXT,
            original_filename TEXT,
            camera TEXT,
            latitude REAL,
            longitude REAL,
            horizontal_accuracy_m REAL
        );
        CREATE INDEX idx_assets_local_date ON assets(local_date);
        CREATE INDEX idx_assets_decision ON assets(decision, reason);
        CREATE INDEX idx_assets_moment ON assets(moment_id);
        CREATE INDEX idx_assets_wanted ON assets(decision, captured_ts DESC, id ASC);",
    )
    .map_err(|e| e.to_string())?;

    let by_id: BTreeMap<&str, &ScanAssetRow> = rows.iter().map(|r| (r.id.as_str(), r)).collect();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    {
        tx.execute(
            "INSERT INTO cache_meta (key, value) VALUES (?1, ?2)",
            params!["schema_version", SQLITE_CACHE_SCHEMA_VERSION],
        )
        .map_err(|e| e.to_string())?;
        if let Some(fp) = decisions_fp {
            tx.execute(
                "INSERT INTO cache_meta (key, value) VALUES (?1, ?2)",
                params!["decisions_len", fp.len.to_string()],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO cache_meta (key, value) VALUES (?1, ?2)",
                params!["decisions_mtime_ns", fp.mtime_ns.to_string()],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO cache_meta (key, value) VALUES (?1, ?2)",
                params!["decisions_ino", fp.ino.to_string()],
            )
            .map_err(|e| e.to_string())?;
        }
        let mut stmt = tx
            .prepare(
                "INSERT INTO assets VALUES (\
                 ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,\
                 ?21,?22,?23,?24,?25,?26)",
            )
            .map_err(|e| e.to_string())?;
        for d in decisions {
            let scan_row = by_id.get(d.id.as_str()).copied();
            let cap_utc = d
                .captured_at
                .as_deref()
                .or_else(|| scan_row.and_then(|r| r.captured_at.as_deref()));
            let away_i: Option<i64> = d.away_from_routine.map(i64::from);
            let marks_json =
                serde_json::to_string(&d.owner_marks).unwrap_or_else(|_| "[]".to_owned());
            let dec_str = match d.decision {
                DecisionKind::Keep => "keep",
                DecisionKind::StageOthersFaceCheck => "stage_others_face_check",
                DecisionKind::LeftOut => "left_out",
            };
            let ext = scan_row.and_then(|r| r.extended.as_ref());
            let loc = scan_row.and_then(|r| r.location.as_ref());
            stmt.execute(params![
                d.id,
                d.cloud_id,
                cap_utc,
                d.captured_ts,
                d.local_date,
                d.local_iso,
                d.timezone,
                d.coarse_place,
                away_i,
                d.media_type,
                i64::from(d.in_cbm),
                i64::from(d.in_saved),
                i64::from(d.screenshot),
                i64::from(d.owner_marked),
                marks_json,
                dec_str,
                d.reason,
                d.moment_id,
                d.collapsed_into,
                d.near_moment_id,
                ext.and_then(|e| e.caption.as_deref()),
                ext.and_then(|e| e.original_filename.as_deref()),
                ext.and_then(|e| e.camera.as_deref()),
                loc.map(|l| l.latitude),
                loc.map(|l| l.longitude),
                loc.and_then(|l| l.horizontal_accuracy_m),
            ])
            .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    drop(conn);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp_path, db_path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Load all persisted metadata scan rows from `config/photos-library/scan/batch-*.json`.
pub fn load_scan_rows(journal: &Path) -> Result<Vec<ScanAssetRow>, String> {
    let scan_dir = holding_root(journal).join("scan");
    if !scan_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut batch_files: Vec<PathBuf> = fs::read_dir(&scan_dir)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("batch-") && n.ends_with(".json"))
        })
        .collect();
    batch_files.sort();

    let mut by_id: BTreeMap<String, ScanAssetRow> = BTreeMap::new();
    for path in batch_files {
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        let batch: ScanBatchFile = serde_json::from_slice(&bytes)
            .map_err(|e| format!("corrupt scan batch {}: {e}", path.display()))?;
        for row in filter_non_hidden(batch.rows) {
            by_id.insert(row.id.clone(), row);
        }
    }
    Ok(by_id.into_values().collect())
}

fn load_smart_catalog(journal: &Path) -> Result<Option<Vec<SmartAlbumCatalogEntry>>, String> {
    read_json_optional(&holding_root(journal).join("scan/smart_albums.json"))
}

fn load_corrections(journal: &Path) -> Result<OwnerCorrections, String> {
    Ok(
        read_json_optional(&holding_root(journal).join("selection/corrections.json"))?
            .unwrap_or_else(|| OwnerCorrections {
                version: 1,
                ..OwnerCorrections::default()
            }),
    )
}

pub(crate) fn load_holding_state(journal: &Path) -> Result<HoldingState, String> {
    Ok(
        read_json_optional(&holding_root(journal).join("state.json"))?.unwrap_or(HoldingState {
            schema: 1,
            ..HoldingState::default()
        }),
    )
}

fn save_holding_state(journal: &Path, state: &HoldingState) -> Result<(), String> {
    write_json_private(&holding_root(journal).join("state.json"), state)
}

fn load_devices(journal: &Path) -> Result<DevicesFile, String> {
    Ok(
        read_json_optional(&holding_root(journal).join("devices.json"))?.unwrap_or(DevicesFile {
            schema: 1,
            devices: BTreeMap::new(),
        }),
    )
}

fn save_devices(journal: &Path, devices: &DevicesFile) -> Result<(), String> {
    write_json_private(&holding_root(journal).join("devices.json"), devices)
}

pub(crate) fn load_wanted_state(journal: &Path) -> Result<WantedStateFile, String> {
    Ok(
        read_json_optional(&holding_root(journal).join("wanted/state.json"))?.unwrap_or(
            WantedStateFile {
                schema: 1,
                assets: BTreeMap::new(),
            },
        ),
    )
}

pub(crate) fn save_wanted_state(journal: &Path, state: &WantedStateFile) -> Result<(), String> {
    write_json_compact_private(&holding_root(journal).join("wanted/state.json"), state)
}

pub fn load_decisions(journal: &Path) -> Result<Vec<AssetDecision>, String> {
    Ok(
        read_json_optional(&holding_root(journal).join("selection/decisions.json"))?
            .unwrap_or_default(),
    )
}

pub fn load_summary(journal: &Path) -> Result<Option<SelectionSummary>, String> {
    read_json_optional(&holding_root(journal).join("selection/summary.json"))
}

fn write_selection_outputs(
    journal: &Path,
    rows: &[ScanAssetRow],
    out: &SelectionOutput,
) -> Result<(), String> {
    let sel_dir = holding_root(journal).join("selection");
    create_directory_with_mode(&sel_dir, PRIVATE_DIR_MODE).map_err(|e| e.to_string())?;
    write_json_private(&sel_dir.join("summary.json"), &out.summary)?;
    let dec_path = sel_dir.join("decisions.json");
    write_json_compact_private(&dec_path, &out.decisions)?;
    write_json_compact_private(&sel_dir.join("left_out.json"), &out.left_out)?;
    write_json_compact_private(&sel_dir.join("moments.json"), &out.moments)?;
    write_json_compact_private(&sel_dir.join("routine_places.json"), &out.routine_places)?;
    rebuild_sqlite_index(&sqlite_cache_path(journal), rows, &out.decisions)?;
    let generation = SELECTION_CACHE_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    let fp = file_fingerprint(&dec_path).ok().flatten();
    if let Some(fp) = fp {
        let entry = Arc::new(CachedDecisions::from_decisions(
            Some(fp),
            generation,
            &out.decisions,
        ));
        store_decisions_cache(decisions_cache_key(journal), entry);
    } else {
        invalidate_decisions_cache(journal);
    }
    Ok(())
}

/// Re-run deterministic selection from the persisted plain-JSON scan batches and
/// rebuild the SQLite cache. Proves zero reliance on SQLite as durable state.
pub fn rebuild_from_plain_json(journal: &Path) -> Result<SelectionOutput, String> {
    let _lock = lock_holding(journal)?;
    rebuild_locked(journal, &SelectionConfig::v2())
}

fn rebuild_locked(journal: &Path, config: &SelectionConfig) -> Result<SelectionOutput, String> {
    let rows = load_scan_rows(journal)?;
    rebuild_locked_with_rows(journal, &rows, config)
}

fn rebuild_locked_with_rows(
    journal: &Path,
    rows: &[ScanAssetRow],
    config: &SelectionConfig,
) -> Result<SelectionOutput, String> {
    let smart_catalog = load_smart_catalog(journal)?;
    let corrections = load_corrections(journal)?;
    let mut state = load_holding_state(journal)?;

    match run_selection(
        rows,
        config,
        &corrections,
        smart_catalog.as_deref(),
        GeoResolver::shared(),
    ) {
        Ok(out) => {
            write_selection_outputs(journal, rows, &out)?;
            if state.attention_reason.as_deref() == Some("smart_album_contract_failed") {
                state.attention_reason = None;
                state.attention_detail = None;
            }
            state.updated_at = Some(solstone_core_time::utc_rfc3339(Timestamp::now()));
            save_holding_state(journal, &state)?;
            Ok(out)
        }
        Err(contract_err) => {
            state.attention_reason = Some("smart_album_contract_failed".to_owned());
            state.attention_detail = Some(contract_err.0.clone());
            state.updated_at = Some(solstone_core_time::utc_rfc3339(Timestamp::now()));
            save_holding_state(journal, &state)?;
            Err(contract_err.0)
        }
    }
}

/// Begin a fresh full metadata-only scan from a paired device.
pub fn scan_start(
    journal: &Path,
    cid: &str,
    device_name: &str,
    device_kind: &str,
    access_state: &str,
    total_items: usize,
    smart_albums: Vec<SmartAlbumCatalogEntry>,
) -> Result<(), String> {
    let _lock = lock_holding(journal)?;
    let root = ensure_holding_dirs(journal)?;
    let _ = fs::remove_file(journal.join(TURNED_OFF_MARKER_REL));
    let now = solstone_core_time::utc_rfc3339(Timestamp::now());

    // Clear stale batch files from any prior incomplete scan.
    let scan_dir = root.join("scan");
    if let Ok(entries) = fs::read_dir(&scan_dir) {
        for entry in entries.filter_map(Result::ok) {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("batch-") && n.ends_with(".json"))
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    write_json_private(&scan_dir.join("smart_albums.json"), &smart_albums)?;

    let _ = record_capable_device(journal, cid, Some(device_name), Some(device_kind));

    let mut devices = load_devices(journal)?;
    let dev = devices.devices.entry(cid.to_owned()).or_default();
    dev.cid = cid.to_owned();
    dev.name = device_name.to_owned();
    dev.kind = if device_kind.is_empty() {
        "iphone".to_owned()
    } else {
        device_kind.to_owned()
    };
    dev.connected = true;
    dev.last_seen_at = Some(now.clone());
    dev.access_state = access_state.to_owned();
    save_devices(journal, &devices)?;

    let mut state = load_holding_state(journal)?;
    state.schema = 1;
    state.scan_active = true;
    state.scan_paused = false;
    state.scan_items_checked = 0;
    state.scan_items_total = total_items;
    state.attention_reason = attention_for_access(access_state);
    state.attention_detail = None;
    state.updated_at = Some(now);
    save_holding_state(journal, &state)?;
    Ok(())
}

/// Append one metadata-only scan batch (zero pixel bytes; hidden assets filtered out).
pub fn scan_batch(
    journal: &Path,
    cid: &str,
    batch_index: u32,
    rows: Vec<ScanAssetRow>,
    items_checked: Option<usize>,
) -> Result<usize, String> {
    if rows.len() > MAX_BATCH_ROWS {
        return Err(format!(
            "scan batch exceeds {MAX_BATCH_ROWS} rows ({})",
            rows.len()
        ));
    }
    let _lock = lock_holding(journal)?;
    let root = ensure_holding_dirs(journal)?;
    let filtered = filter_non_hidden(rows);
    let kept_len = filtered.len();

    let mut earliest: Option<String> = None;
    for r in &filtered {
        if let Some(cap) = r.captured_at.as_deref()
            && !cap.trim().is_empty()
            && earliest.as_deref().is_none_or(|e| cap < e)
        {
            earliest = Some(cap.to_owned());
        }
    }

    let batch_path = root.join(format!("scan/batch-{batch_index:06}.json"));
    write_json_compact_private(
        &batch_path,
        &ScanBatchFile {
            batch_index,
            rows: filtered,
        },
    )?;

    let mut state = load_holding_state(journal)?;
    state.scan_active = true;
    state.scan_paused = false;
    state.scan_items_checked = items_checked.unwrap_or(state.scan_items_checked + kept_len);
    if state.scan_items_total < state.scan_items_checked {
        state.scan_items_total = state.scan_items_checked;
    }
    if let Some(e) = earliest
        && state
            .scan_earliest_item_at
            .as_deref()
            .is_none_or(|cur| e.as_str() < cur)
    {
        state.scan_earliest_item_at = Some(e);
    }
    let now = solstone_core_time::utc_rfc3339(Timestamp::now());
    state.updated_at = Some(now.clone());
    save_holding_state(journal, &state)?;

    let mut devices = load_devices(journal)?;
    if let Some(dev) = devices.devices.get_mut(cid) {
        dev.connected = true;
        dev.last_seen_at = Some(now);
        save_devices(journal, &devices)?;
    }

    Ok(kept_len)
}

/// Complete a metadata scan, run Rules v2 + smart-album contract check, and
/// write durable JSON outputs + SQLite query cache.
pub fn scan_finish(
    journal: &Path,
    cid: &str,
    items_checked: Option<usize>,
    earliest_item_at: Option<String>,
    access_state: Option<&str>,
    icloud_waiting_count: Option<u64>,
) -> Result<SelectionSummary, String> {
    let _lock = lock_holding(journal)?;
    let now = solstone_core_time::utc_rfc3339(Timestamp::now());
    let mut state = load_holding_state(journal)?;
    state.scan_active = false;
    state.scan_complete = true;
    state.scan_paused = false;
    if let Some(checked) = items_checked {
        state.scan_items_checked = checked;
        if state.scan_items_total < checked {
            state.scan_items_total = checked;
        }
    }
    if let Some(earliest) = earliest_item_at
        && state
            .scan_earliest_item_at
            .as_deref()
            .is_none_or(|cur| earliest.as_str() < cur)
    {
        state.scan_earliest_item_at = Some(earliest);
    }
    state.last_scan_at = Some(now.clone());
    if let Some(acc) = access_state {
        state.attention_reason = attention_for_access(acc);
    }
    state.updated_at = Some(now.clone());
    save_holding_state(journal, &state)?;

    let mut devices = load_devices(journal)?;
    if let Some(dev) = devices.devices.get_mut(cid) {
        dev.connected = true;
        dev.last_seen_at = Some(now.clone());
        dev.last_scan_at = Some(now);
        if let Some(acc) = access_state {
            dev.access_state = acc.to_owned();
        }
        if let Some(cnt) = icloud_waiting_count {
            dev.icloud_waiting_count = cnt;
        }
        save_devices(journal, &devices)?;
    }

    let out = rebuild_locked(journal, &SelectionConfig::v2())?;
    Ok(out.summary)
}

/// Apply incremental PhotoKit metadata changes (added/updated/removed assets)
/// and recompute selection deterministically.
pub fn scan_changes(
    journal: &Path,
    cid: &str,
    added_or_updated: Vec<ScanAssetRow>,
    removed_ids: Vec<String>,
    smart_albums: Option<Vec<SmartAlbumCatalogEntry>>,
    access_state: Option<&str>,
    icloud_waiting_count: Option<u64>,
) -> Result<SelectionSummary, String> {
    if !holding_root(journal).is_dir() && journal.join(TURNED_OFF_MARKER_REL).is_file() {
        return Err("photos library source is turned off".to_owned());
    }
    let _lock = lock_holding(journal)?;
    let root = ensure_holding_dirs(journal)?;
    let now = solstone_core_time::utc_rfc3339(Timestamp::now());

    let mut catalog_changed = false;
    if let Some(catalog) = smart_albums {
        let existing_catalog = load_smart_catalog(journal)?;
        if existing_catalog.as_ref() != Some(&catalog) {
            write_json_private(&root.join("scan/smart_albums.json"), &catalog)?;
            catalog_changed = true;
        }
    }

    let incoming_rows = filter_non_hidden(added_or_updated);
    let mut state = load_holding_state(journal)?;
    let had_contract_error =
        state.attention_reason.as_deref() == Some("smart_album_contract_failed");

    let has_existing_selection = holding_root(journal)
        .join("selection/decisions.json")
        .is_file()
        && load_summary(journal)?.is_some();

    let (rows_changed, total_rows, all_rows) = if incoming_rows.is_empty()
        && removed_ids.is_empty()
        && !catalog_changed
        && !had_contract_error
        && has_existing_selection
    {
        (false, state.scan_items_total, None)
    } else {
        let mut by_id: BTreeMap<String, ScanAssetRow> = load_scan_rows(journal)?
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();
        let mut changed = false;
        for rid in &removed_ids {
            if by_id.remove(rid).is_some() {
                changed = true;
            }
        }
        for row in incoming_rows {
            if by_id.get(&row.id) != Some(&row) {
                by_id.insert(row.id.clone(), row);
                changed = true;
            }
        }

        let total = by_id.len();
        let rows_opt = if changed {
            // Rewrite compact canonical batch files so plain JSON stays authoritative.
            let scan_dir = root.join("scan");
            if let Ok(entries) = fs::read_dir(&scan_dir) {
                for entry in entries.filter_map(Result::ok) {
                    if entry
                        .file_name()
                        .to_str()
                        .is_some_and(|n| n.starts_with("batch-") && n.ends_with(".json"))
                    {
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
            let rows: Vec<ScanAssetRow> = by_id.into_values().collect();
            for (idx, chunk) in rows.chunks(1_000).enumerate() {
                let batch_path = scan_dir.join(format!("batch-{idx:06}.json"));
                write_json_compact_private(
                    &batch_path,
                    &ScanBatchFile {
                        batch_index: idx as u32,
                        rows: chunk.to_vec(),
                    },
                )?;
            }
            Some(rows)
        } else if catalog_changed || had_contract_error || !has_existing_selection {
            Some(by_id.into_values().collect())
        } else {
            None
        };
        (changed, total, rows_opt)
    };

    // Mark removed IDs as Gone in wanted state if they had not landed yet.
    if !removed_ids.is_empty() {
        let mut wanted = load_wanted_state(journal)?;
        let mut wanted_dirty = false;
        for rid in &removed_ids {
            if !matches!(
                wanted.assets.get(rid),
                Some(WantedAssetStatus::Landed { .. } | WantedAssetStatus::Gone { .. })
            ) {
                wanted
                    .assets
                    .insert(rid.clone(), WantedAssetStatus::Gone { at: now.clone() });
                wanted_dirty = true;
            }
        }
        if wanted_dirty {
            save_wanted_state(journal, &wanted)?;
        }
    }

    state.scan_active = false;
    state.scan_complete = true;
    state.scan_items_checked = total_rows;
    state.scan_items_total = total_rows;
    state.last_scan_at = Some(now.clone());
    if let Some(acc) = access_state {
        state.attention_reason = attention_for_access(acc);
    }
    state.updated_at = Some(now.clone());
    save_holding_state(journal, &state)?;

    let mut devices = load_devices(journal)?;
    if let Some(dev) = devices.devices.get_mut(cid) {
        dev.connected = true;
        dev.last_seen_at = Some(now.clone());
        dev.last_scan_at = Some(now);
        if let Some(acc) = access_state {
            dev.access_state = acc.to_owned();
        }
        if let Some(cnt) = icloud_waiting_count {
            dev.icloud_waiting_count = cnt;
        }
        save_devices(journal, &devices)?;
    }

    if !rows_changed
        && !catalog_changed
        && !had_contract_error
        && holding_root(journal)
            .join("selection/decisions.json")
            .is_file()
        && let Some(existing_summary) = load_summary(journal)?
    {
        return Ok(existing_summary);
    }

    let out = match all_rows {
        Some(rows) => rebuild_locked_with_rows(journal, &rows, &SelectionConfig::v2())?,
        None => rebuild_locked(journal, &SelectionConfig::v2())?,
    };
    Ok(out.summary)
}

/// Update paired device status (capability check-in, access state, iCloud waiting count, scan pause).
///
/// When `source_enabled == Some(false)`, this records the paired iPhone/iPad as
/// photos-capable without creating the `config/photos-library/` holding area or
/// raising access attention before the owner turns Photos on.
#[allow(clippy::too_many_arguments)]
pub fn update_device_status(
    journal: &Path,
    cid: &str,
    device_name: Option<&str>,
    device_kind: Option<&str>,
    access_state: Option<&str>,
    icloud_waiting_count: Option<u64>,
    scan_paused: Option<bool>,
    source_enabled: Option<bool>,
) -> Result<(), String> {
    let _ = record_capable_device(journal, cid, device_name, device_kind)?;
    let now = solstone_core_time::utc_rfc3339(Timestamp::now());

    if source_enabled == Some(false) {
        if !holding_root(journal).is_dir() {
            return Ok(());
        }
        let _lock = lock_holding(journal)?;
        let mut devices = load_devices(journal)?;
        if let Some(dev) = devices.devices.get_mut(cid) {
            if let Some(n) = device_name
                && !n.trim().is_empty()
            {
                dev.name = n.to_owned();
            }
            if let Some(k) = device_kind
                && !k.trim().is_empty()
            {
                dev.kind = k.to_owned();
            }
            dev.connected = true;
            dev.last_seen_at = Some(now);
            if let Some(acc) = access_state {
                dev.access_state = acc.to_owned();
            }
            if let Some(cnt) = icloud_waiting_count {
                dev.icloud_waiting_count = cnt;
            }
            save_devices(journal, &devices)?;
        }
        return Ok(());
    }

    let _lock = lock_holding(journal)?;
    let _ = ensure_holding_dirs(journal)?;
    let mut devices = load_devices(journal)?;
    let dev = devices.devices.entry(cid.to_owned()).or_default();
    dev.cid = cid.to_owned();
    if let Some(n) = device_name
        && !n.trim().is_empty()
    {
        dev.name = n.to_owned();
    }
    if let Some(k) = device_kind
        && !k.trim().is_empty()
    {
        dev.kind = k.to_owned();
    } else if dev.kind.is_empty() {
        dev.kind = "iphone".to_owned();
    }
    dev.connected = true;
    dev.last_seen_at = Some(now.clone());
    if let Some(acc) = access_state {
        dev.access_state = acc.to_owned();
    }
    if let Some(cnt) = icloud_waiting_count {
        dev.icloud_waiting_count = cnt;
    }
    save_devices(journal, &devices)?;

    let mut state = load_holding_state(journal)?;
    if let Some(sp) = scan_paused {
        state.scan_paused = sp;
    }
    if let Some(acc) = access_state {
        // Keep smart-album contract failure sticky until a clean scan succeeds.
        if state.attention_reason.as_deref() != Some("smart_album_contract_failed") {
            state.attention_reason = attention_for_access(acc);
        }
    }
    state.updated_at = Some(now);
    save_holding_state(journal, &state)?;
    Ok(())
}

fn attention_for_access(access_state: &str) -> Option<String> {
    match access_state {
        "limited" => Some("access_limited".to_owned()),
        "denied" | "restricted" => Some("access_off".to_owned()),
        _ => None,
    }
}

/// Resolve a `RangeSpec` into `(canonical_key, range_start_at_utc, range_end_at_utc)`.
fn resolve_range_bounds(
    spec: &RangeSpec,
    now: Timestamp,
) -> Result<(String, Option<String>, Option<String>), String> {
    match spec {
        RangeSpec::Preset(raw) => {
            let key = raw.trim().to_ascii_lowercase();
            let start = match key.as_str() {
                "30d" => Some(
                    now.checked_sub(SignedDuration::from_hours(30 * 24))
                        .unwrap_or(Timestamp::UNIX_EPOCH),
                ),
                "6m" => Some(
                    now.checked_sub(SignedDuration::from_hours(183 * 24))
                        .unwrap_or(Timestamp::UNIX_EPOCH),
                ),
                "1y" => Some(
                    now.checked_sub(SignedDuration::from_hours(365 * 24))
                        .unwrap_or(Timestamp::UNIX_EPOCH),
                ),
                "all" => None,
                "none" => Some(now),
                _ => return Err(format!("unsupported range preset: {raw}")),
            };
            Ok((key, start.map(solstone_core_time::utc_rfc3339), None))
        }
        RangeSpec::Custom { since, until } => {
            let start_ts = parse_date_or_rfc3339(since, false)
                .ok_or_else(|| format!("invalid range start: {since}"))?;
            let end_ts = match until.as_deref() {
                Some(u) => Some(
                    parse_date_or_rfc3339(u, true)
                        .ok_or_else(|| format!("invalid range end: {u}"))?,
                ),
                None => None,
            };
            Ok((
                spec.canonical_key(),
                Some(solstone_core_time::utc_rfc3339(start_ts)),
                end_ts.map(solstone_core_time::utc_rfc3339),
            ))
        }
    }
}

fn parse_date_or_rfc3339(raw: &str, end_of_day: bool) -> Option<Timestamp> {
    let trimmed = raw.trim();
    if let Ok(ts) = trimmed.parse::<Timestamp>() {
        return Some(ts);
    }
    if let Ok(z) = solstone_core_time::parse_rfc3339(trimmed) {
        return Some(z.timestamp());
    }
    if let Ok(date) = trimmed.parse::<jiff::civil::Date>() {
        let dt = if end_of_day {
            date.at(23, 59, 59, 0)
        } else {
            date.at(0, 0, 0, 0)
        };
        return jiff::tz::Offset::UTC.to_timestamp(dt).ok();
    }
    None
}

/// Save the owner's initial range choice and publish the wanted list.
pub fn choose_range(journal: &Path, spec: &RangeSpec) -> Result<PhotosDashboardState, String> {
    choose_range_with_live_seen(journal, spec, None)
}

/// Save the owner's initial range choice and return the dashboard snapshot once,
/// using optional live in-process link presence.
pub fn choose_range_with_live_seen(
    journal: &Path,
    spec: &RangeSpec,
    live_seen: Option<&BTreeMap<String, String>>,
) -> Result<PhotosDashboardState, String> {
    let _lock = lock_holding(journal)?;
    let now = Timestamp::now();
    let (key, start_at, end_at) = resolve_range_bounds(spec, now)?;
    let mut state = load_holding_state(journal)?;
    state.chosen_range = Some(key);
    state.range_start_at = start_at;
    state.range_end_at = end_at;
    state.chosen_at = Some(solstone_core_time::utc_rfc3339(now));
    state.paused = false;
    state.updated_at = Some(solstone_core_time::utc_rfc3339(now));
    save_holding_state(journal, &state)?;
    drop(_lock);
    dashboard_state_at(journal, live_seen, now)
}

/// Widen the owner's lookback range without re-requesting already-received photos.
pub fn widen_range(journal: &Path, spec: &RangeSpec) -> Result<PhotosDashboardState, String> {
    widen_range_with_live_seen(journal, spec, None)
}

/// Widen the owner's lookback range and return the dashboard snapshot once.
pub fn widen_range_with_live_seen(
    journal: &Path,
    spec: &RangeSpec,
    live_seen: Option<&BTreeMap<String, String>>,
) -> Result<PhotosDashboardState, String> {
    let _lock = lock_holding(journal)?;
    let now = Timestamp::now();
    let (key, new_start_at, new_end_at) = resolve_range_bounds(spec, now)?;
    let mut state = load_holding_state(journal)?;
    // Only move range_start_at earlier (or to None for "all").
    let effective_start = match (&state.range_start_at, &new_start_at) {
        (_, None) => None,
        (None, Some(_)) => None,
        (Some(cur), Some(cand)) => {
            if parse_utc_ts(Some(cand)) < parse_utc_ts(Some(cur)) {
                Some(cand.clone())
            } else {
                Some(cur.clone())
            }
        }
    };
    state.chosen_range = Some(key);
    state.range_start_at = effective_start;
    if new_end_at.is_some() {
        state.range_end_at = new_end_at;
    }
    if state.chosen_at.is_none() {
        state.chosen_at = Some(solstone_core_time::utc_rfc3339(now));
    }
    state.paused = false;
    state.updated_at = Some(solstone_core_time::utc_rfc3339(now));
    save_holding_state(journal, &state)?;
    drop(_lock);
    dashboard_state_at(journal, live_seen, now)
}

/// Pause source arrival (stops asking paired devices for 512 px copies and new photos).
pub fn set_paused(journal: &Path, paused: bool) -> Result<PhotosDashboardState, String> {
    set_paused_with_live_seen(journal, paused, None)
}

/// Pause or resume source arrival and return the dashboard snapshot once.
pub fn set_paused_with_live_seen(
    journal: &Path,
    paused: bool,
    live_seen: Option<&BTreeMap<String, String>>,
) -> Result<PhotosDashboardState, String> {
    let _lock = lock_holding(journal)?;
    let now = Timestamp::now();
    let mut state = load_holding_state(journal)?;
    state.paused = paused;
    state.updated_at = Some(solstone_core_time::utc_rfc3339(now));
    save_holding_state(journal, &state)?;
    drop(_lock);
    dashboard_state_at(journal, live_seen, now)
}

/// Turn off the Photos Library source and delete the private holding area
/// (`config/photos-library/`). Landed `chronicle/` photos and durable
/// `config/photos/received/` suppression stay untouched.
pub fn turn_off(journal: &Path) -> Result<PhotosDashboardState, String> {
    turn_off_with_live_seen(journal, None)
}

/// Turn off the Photos Library source and return the dashboard snapshot once.
pub fn turn_off_with_live_seen(
    journal: &Path,
    live_seen: Option<&BTreeMap<String, String>>,
) -> Result<PhotosDashboardState, String> {
    let root = holding_root(journal);
    if root.exists() {
        let lock = lock_holding(journal)?;
        drop(lock);
        fs::remove_dir_all(&root).map_err(|e| format!("remove holding area: {e}"))?;
    }
    invalidate_decisions_cache(journal);
    let marker = journal.join(TURNED_OFF_MARKER_REL);
    let now = Timestamp::now();
    let now_iso = solstone_core_time::utc_rfc3339(now);
    write_json_private(&marker, &serde_json::json!({ "turned_off_at": now_iso }))?;
    dashboard_state_at(journal, live_seen, now)
}

/// Whether a capture timestamp falls within `[start_ts, end_ts]` bounds.
fn decision_in_window_ts(captured_ts: f64, start_ts: Option<f64>, end_ts: Option<f64>) -> bool {
    if let Some(s) = start_ts
        && captured_ts < s
    {
        return false;
    }
    if let Some(e) = end_ts
        && captured_ts > e
    {
        return false;
    }
    true
}

/// Whether the journal can currently run face matching for the others'-photos rung.
pub fn can_run_others_face_check(journal: &Path) -> bool {
    if !faceprints::photo_faces_active(journal).unwrap_or(false) {
        return false;
    }
    faceprints::photo_match::load_meeting_face_index(journal)
        .map(|idx| !idx.is_empty())
        .unwrap_or(false)
}

/// Lease up to `limit` wanted photos for paired device `cid` (sorted newest-first).
///
/// Guarantees:
/// - Nothing is leased before the owner chooses a range, while paused, or while
///   `attention_reason` is active (such as a smart-album contract failure).
/// - Assets already in `config/photos/received/` (live OR owner-trashed) are
///   never asked for again.
/// - One device answers each ask via a 5-minute lease that automatically expires
///   if the device goes offline.
pub fn lease_wanted(
    journal: &Path,
    cid: &str,
    limit: usize,
    lease_seconds: Option<i64>,
) -> Result<WantedResponse, String> {
    // Check meeting face availability outside `.photos-library.lock` so we never
    // nest `.entity-trust.lock` inside `.photos-library.lock`.
    let pre_state = load_holding_state(journal).ok();
    let others_enabled = if pre_state.as_ref().is_some_and(|s| {
        !s.paused && s.attention_reason.is_none() && s.scan_complete && s.chosen_range.is_some()
    }) {
        can_run_others_face_check(journal)
    } else {
        false
    };

    let _lock = lock_holding(journal)?;
    let now = Timestamp::now();
    let now_iso = solstone_core_time::utc_rfc3339(now);
    let mut devices = load_devices(journal)?;
    if let Some(dev) = devices.devices.get_mut(cid) {
        dev.connected = true;
        dev.last_seen_at = Some(now_iso.clone());
        save_devices(journal, &devices)?;
    }

    let state = load_holding_state(journal)?;
    let range = state.chosen_range.clone();

    if state.paused || state.attention_reason.is_some() || !state.scan_complete || range.is_none() {
        return Ok(WantedResponse {
            paused: state.paused,
            range,
            wanted: Vec::new(),
            remaining: 0,
        });
    }

    let start_ts = state
        .range_start_at
        .as_deref()
        .map(|s| parse_utc_ts(Some(s)));
    let end_ts = state.range_end_at.as_deref().map(|s| parse_utc_ts(Some(s)));
    let cached = load_cached_decisions(journal)?;
    let mut wanted_state = load_wanted_state(journal)?;
    let mut received = ReceivedLedgerSnapshot::load(journal)
        .map_err(|e| format!("failed reading photos received ledger: {e}"))?;

    let now_ts = now.as_second() as f64;
    let lease_secs = lease_seconds.unwrap_or(DEFAULT_LEASE_SECONDS).max(30);
    let expires_iso = solstone_core_time::utc_rfc3339(
        now.checked_add(SignedDuration::from_secs(lease_secs))
            .unwrap_or(now),
    );
    let take_n = limit.clamp(1, MAX_WANTED_LIMIT);

    // `cached.wanted_order` is already filtered to Keep/StageOthersFaceCheck and
    // sorted newest-first (deterministic tie-break by id).
    let mut total_candidates = 0usize;
    let mut leased = Vec::with_capacity(take_n);
    for &idx in &cached.wanted_order {
        let d = &cached.all[idx];
        if !decision_in_window_ts(d.captured_ts, start_ts, end_ts) {
            continue;
        }
        if d.decision == DecisionKind::StageOthersFaceCheck && !others_enabled {
            continue;
        }
        // Fail-closed durable suppression check (survives trash erase & turn-off).
        if received
            .lookup(&d.id, d.cloud_id.as_deref())
            .map_err(|e| format!("failed reading photos received ledger: {e}"))?
            .is_some()
        {
            continue;
        }
        // Skip terminal or actively leased states.
        match wanted_state.assets.get(&d.id) {
            Some(
                WantedAssetStatus::Landed { .. }
                | WantedAssetStatus::NoiseDropped { .. }
                | WantedAssetStatus::OthersNoFaceMatch { .. }
                | WantedAssetStatus::Gone { .. },
            ) => continue,
            Some(WantedAssetStatus::Leased {
                cid: lease_cid,
                expires_at,
                ..
            }) => {
                let exp_ts = parse_utc_ts(Some(expires_at));
                if lease_cid != cid && exp_ts > now_ts {
                    continue;
                }
            }
            Some(WantedAssetStatus::IcloudWaiting { .. }) | None => {}
        }
        total_candidates += 1;
        if leased.len() < take_n {
            leased.push(d);
        }
    }

    let mut out_items = Vec::with_capacity(leased.len());
    for d in leased {
        wanted_state.assets.insert(
            d.id.clone(),
            WantedAssetStatus::Leased {
                cid: cid.to_owned(),
                leased_at: now_iso.clone(),
                expires_at: expires_iso.clone(),
            },
        );
        out_items.push(WantedItem {
            asset_id: d.id.clone(),
            cloud_id: d.cloud_id.clone(),
            max_edge: 512,
            captured_at: d.captured_at.clone(),
            reason_kind: match d.decision {
                DecisionKind::StageOthersFaceCheck => "others_face_check".to_owned(),
                _ => "keep".to_owned(),
            },
        });
    }
    if !out_items.is_empty() {
        save_wanted_state(journal, &wanted_state)?;
    }

    Ok(WantedResponse {
        paused: false,
        range,
        remaining: total_candidates.saturating_sub(out_items.len()),
        wanted: out_items,
    })
}

/// Record that a wanted asset has left the device library (`"gone"`) or is
/// waiting on iCloud download (`"icloud_waiting"`).
pub fn mark_wanted_non_copy(
    journal: &Path,
    cid: &str,
    asset_id: &str,
    status: &str,
) -> Result<(), String> {
    let _lock = lock_holding(journal)?;
    let now = solstone_core_time::utc_rfc3339(Timestamp::now());
    let mut devices = load_devices(journal)?;
    if let Some(dev) = devices.devices.get_mut(cid) {
        dev.connected = true;
        dev.last_seen_at = Some(now.clone());
        save_devices(journal, &devices)?;
    }
    let mut wanted = load_wanted_state(journal)?;
    match status {
        "gone" => {
            wanted
                .assets
                .insert(asset_id.to_owned(), WantedAssetStatus::Gone { at: now });
        }
        "icloud_waiting" => {
            wanted.assets.insert(
                asset_id.to_owned(),
                WantedAssetStatus::IcloudWaiting {
                    cid: cid.to_owned(),
                    at: now,
                },
            );
        }
        other => return Err(format!("unsupported non-copy status: {other}")),
    }
    save_wanted_state(journal, &wanted)?;
    Ok(())
}

/// Fast short-circuit check for whether at least one live photo exists under `chronicle/*/photos/`.
fn has_any_live_photo_in_journal(journal: &Path) -> bool {
    let chronicle = journal.join("chronicle");
    let Ok(days) = fs::read_dir(&chronicle) else {
        return false;
    };
    for day_entry in days.filter_map(Result::ok) {
        let photos_dir = day_entry.path().join("photos");
        let Ok(segs) = fs::read_dir(&photos_dir) else {
            continue;
        };
        for seg_entry in segs.filter_map(Result::ok) {
            if seg_entry.file_type().is_ok_and(|t| t.is_dir())
                && !seg_entry.path().join("tombstone.json").is_file()
            {
                return true;
            }
        }
    }
    false
}

/// Count all live landed library photos in `chronicle/*/photos/`.
fn count_live_photos_in_journal(journal: &Path) -> usize {
    let chronicle = journal.join("chronicle");
    let Ok(days) = fs::read_dir(&chronicle) else {
        return 0;
    };
    let mut count = 0usize;
    for day_entry in days.filter_map(Result::ok) {
        let photos_dir = day_entry.path().join("photos");
        let Ok(segs) = fs::read_dir(&photos_dir) else {
            continue;
        };
        for seg_entry in segs.filter_map(Result::ok) {
            if seg_entry.file_type().is_ok_and(|t| t.is_dir())
                && !seg_entry.path().join("tombstone.json").is_file()
            {
                count += 1;
            }
        }
    }
    count
}

#[derive(Default)]
struct PresetAccumulator<'a> {
    photos: usize,
    days: HashSet<&'a str>,
    delta_photos: usize,
    delta_days: HashSet<&'a str>,
    earliest_date: Option<&'a str>,
    left_out_reasons: Vec<&'a str>,
}

impl<'a> PresetAccumulator<'a> {
    fn observe(
        &mut self,
        d: &'a CompactDecision,
        wanted_status: Option<&WantedAssetStatus>,
        is_received: bool,
        has_active_choice: bool,
        current_range_is_all: bool,
        current_start_ts: Option<f64>,
    ) {
        if let Some(ld) = d.local_date.as_deref()
            && self.earliest_date.is_none_or(|e| ld < e)
        {
            self.earliest_date = Some(ld);
        }
        match d.decision {
            DecisionKind::Keep => {
                self.photos += 1;
                if let Some(ld) = d.local_date.as_deref() {
                    self.days.insert(ld);
                }
                let already_covered = current_range_is_all
                    || (has_active_choice
                        && current_start_ts.is_some_and(|cs| d.captured_ts >= cs))
                    || is_received;
                if !already_covered {
                    self.delta_photos += 1;
                    if let Some(ld) = d.local_date.as_deref() {
                        self.delta_days.insert(ld);
                    }
                }
                if matches!(wanted_status, Some(WantedAssetStatus::NoiseDropped { .. })) {
                    self.left_out_reasons.push("read_as_noise");
                }
            }
            DecisionKind::StageOthersFaceCheck => {
                if matches!(
                    wanted_status,
                    Some(WantedAssetStatus::OthersNoFaceMatch { .. })
                ) {
                    self.left_out_reasons.push("others_no_face_match");
                } else {
                    self.left_out_reasons.push(d.reason.as_str());
                }
            }
            DecisionKind::LeftOut => {
                self.left_out_reasons.push(d.reason.as_str());
            }
        }
    }

    fn finish(self, covered: bool) -> RangeEstimate {
        RangeEstimate {
            photos: self.photos,
            days: self.days.len(),
            thoughts: self.photos,
            delta_photos: if covered { 0 } else { self.delta_photos },
            delta_days: if covered { 0 } else { self.delta_days.len() },
            delta_thoughts: if covered { 0 } else { self.delta_photos },
            covered,
            earliest_date: self.earliest_date.map(str::to_owned),
            left_out: aggregate_left_out_buckets(self.left_out_reasons),
        }
    }
}

fn read_thinking_and_band4(journal: &Path) -> (bool, bool) {
    let policy = solstone_core_system::work_policy::WorkPolicy::read(journal).ok();
    let band4_paused = policy.as_ref().is_some_and(|p| p.backfill_paused);
    let thinking_ready = policy.as_ref().is_some_and(|p| {
        !matches!(
            p.lane,
            solstone_core_system::work_policy::ThinkingLane::Unavailable
        )
    });
    (thinking_ready, band4_paused)
}

fn parse_rfc3339_ts(raw: &str) -> Option<Timestamp> {
    let trimmed = raw.trim();
    trimmed.parse::<Timestamp>().ok().or_else(|| {
        solstone_core_time::parse_rfc3339(trimmed)
            .ok()
            .map(|z| z.timestamp())
    })
}

fn device_is_connected(
    dev: &DeviceState,
    authorized_cids: Option<&BTreeSet<String>>,
    live_seen: Option<&BTreeMap<String, String>>,
    link_last_seen: &BTreeMap<String, String>,
    now: Timestamp,
) -> bool {
    if let Some(auth) = authorized_cids
        && !auth.contains(&dev.cid)
    {
        return false;
    }
    let freshest = [
        live_seen.and_then(|m| m.get(&dev.cid).map(String::as_str)),
        link_last_seen.get(&dev.cid).map(String::as_str),
        dev.last_seen_at.as_deref(),
        dev.last_upload_at.as_deref(),
        dev.last_scan_at.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter_map(parse_rfc3339_ts)
    .max();
    let Some(ts) = freshest else {
        return false;
    };
    let age_ms = now.as_millisecond() - ts.as_millisecond();
    (-DEVICE_FUTURE_CLOCK_DRIFT_MS..DEVICE_CONNECTED_WINDOW_MS).contains(&age_ms)
}

/// Build the complete dashboard state snapshot for `GET /app/sources/photos/api/state`.
pub fn dashboard_state(journal: &Path) -> Result<PhotosDashboardState, String> {
    dashboard_state_at(journal, None, Timestamp::now())
}

/// Build the complete dashboard state snapshot using live in-process link presence.
pub fn dashboard_state_with_live_seen(
    journal: &Path,
    live_seen: &BTreeMap<String, String>,
) -> Result<PhotosDashboardState, String> {
    dashboard_state_at(journal, Some(live_seen), Timestamp::now())
}

/// Build the dashboard state snapshot at an explicit timestamp `now` (deterministic for tests).
pub fn dashboard_state_at(
    journal: &Path,
    live_seen: Option<&BTreeMap<String, String>>,
    now: Timestamp,
) -> Result<PhotosDashboardState, String> {
    let root = holding_root(journal);
    let (thinking_ready, band4_paused) = read_thinking_and_band4(journal);
    let profile_pics_enabled = can_run_others_face_check(journal);
    let live_in_journal = count_live_photos_in_journal(journal);
    let capable_kinds = capable_device_kinds(journal);

    if !root.is_dir() {
        let is_off = live_in_journal > 0
            || journal.join("config/photos/received").is_dir()
            || journal.join(TURNED_OFF_MARKER_REL).is_file();
        return Ok(PhotosDashboardState {
            source: "photos".to_owned(),
            state: if is_off {
                "off".to_owned()
            } else {
                "ready_to_set_up".to_owned()
            },
            substate: if is_off {
                "off".to_owned()
            } else {
                "never_set_up".to_owned()
            },
            attention_reason: None,
            devices: Vec::new(),
            scan: ScanProgressView::default(),
            choice: ChoiceView::default(),
            ranges: BTreeMap::new(),
            left_out: LeftOutBreakdown::default(),
            progress: ProgressView {
                in_journal: live_in_journal,
                ..ProgressView::default()
            },
            band4_paused,
            thinking_ready,
            profile_pics_enabled,
            capable_device_kinds: capable_kinds,
        });
    }

    let state = load_holding_state(journal)?;
    let devices_file = load_devices(journal)?;
    let cached = load_cached_decisions(journal)?;
    let wanted = load_wanted_state(journal)?;
    let mut received = ReceivedLedgerSnapshot::load(journal)
        .unwrap_or_else(|_| ReceivedLedgerSnapshot::empty(journal));

    let has_active_choice = state.chosen_range.is_some();
    let current_range_is_all = state.chosen_range.as_deref() == Some("all");
    let current_start_ts = state
        .range_start_at
        .as_deref()
        .map(|s| parse_utc_ts(Some(s)));
    let current_end_ts = state.range_end_at.as_deref().map(|s| parse_utc_ts(Some(s)));

    // Single pass across cached decisions for all 4 standard presets + active range.
    const PRESETS: [&str; 4] = ["30d", "6m", "1y", "all"];
    let mut preset_bounds = [None; 4];
    for (idx, preset) in PRESETS.iter().enumerate() {
        let (_, bound_iso, _) =
            resolve_range_bounds(&RangeSpec::Preset((*preset).to_owned()), now)?;
        preset_bounds[idx] = bound_iso.as_deref().map(|s| parse_utc_ts(Some(s)));
    }
    let mut preset_accs = [
        PresetAccumulator::default(),
        PresetAccumulator::default(),
        PresetAccumulator::default(),
        PresetAccumulator::default(),
    ];

    let mut chosen_total = 0usize;
    let mut arrived = 0usize;
    let mut in_journal_in_range = 0usize;
    let mut moved_to_trash = 0usize;
    let mut waiting_on_phone = 0usize;
    let mut active_left_out_reasons: Vec<&str> = Vec::new();

    for d in &cached.all {
        let wanted_status = wanted.assets.get(&d.id);
        let receipt: Option<PhotoReceipt> =
            if !received.is_empty() && d.decision != DecisionKind::LeftOut {
                received.lookup(&d.id, d.cloud_id.as_deref()).ok().flatten()
            } else {
                None
            };
        let is_received = receipt.is_some();

        for (idx, acc) in preset_accs.iter_mut().enumerate() {
            if decision_in_window_ts(d.captured_ts, preset_bounds[idx], None) {
                acc.observe(
                    d,
                    wanted_status,
                    is_received,
                    has_active_choice,
                    current_range_is_all,
                    current_start_ts,
                );
            }
        }

        if has_active_choice
            && !decision_in_window_ts(d.captured_ts, current_start_ts, current_end_ts)
        {
            continue;
        }
        match d.decision {
            DecisionKind::Keep => {
                if has_active_choice {
                    chosen_total += 1;
                    match (receipt.as_ref(), wanted_status) {
                        (Some(rec), _) => {
                            arrived += 1;
                            if received.is_live(rec) {
                                in_journal_in_range += 1;
                            } else {
                                moved_to_trash += 1;
                            }
                        }
                        (None, Some(WantedAssetStatus::NoiseDropped { .. })) => {
                            arrived += 1;
                            active_left_out_reasons.push("read_as_noise");
                        }
                        (None, Some(WantedAssetStatus::Gone { .. })) => {
                            arrived += 1;
                        }
                        (None, _) => {
                            waiting_on_phone += 1;
                        }
                    }
                }
            }
            DecisionKind::StageOthersFaceCheck => match (receipt.as_ref(), wanted_status) {
                (Some(rec), _) => {
                    if has_active_choice {
                        chosen_total += 1;
                        arrived += 1;
                        if received.is_live(rec) {
                            in_journal_in_range += 1;
                        } else {
                            moved_to_trash += 1;
                        }
                    }
                }
                (None, Some(WantedAssetStatus::NoiseDropped { .. })) => {
                    active_left_out_reasons.push("read_as_noise");
                }
                (None, Some(WantedAssetStatus::OthersNoFaceMatch { .. })) => {
                    active_left_out_reasons.push("others_no_face_match");
                }
                (None, Some(WantedAssetStatus::Gone { .. } | WantedAssetStatus::Landed { .. })) => {
                }
                (
                    None,
                    None
                    | Some(
                        WantedAssetStatus::Leased { .. } | WantedAssetStatus::IcloudWaiting { .. },
                    ),
                ) => {
                    if has_active_choice && profile_pics_enabled {
                        chosen_total += 1;
                        waiting_on_phone += 1;
                    } else {
                        active_left_out_reasons.push(d.reason.as_str());
                    }
                }
            },
            DecisionKind::LeftOut => {
                active_left_out_reasons.push(d.reason.as_str());
            }
        }
    }

    let mut ranges = BTreeMap::new();
    for (idx, (preset, acc)) in PRESETS.iter().zip(preset_accs).enumerate() {
        let covered = has_active_choice
            && (current_range_is_all
                || match (current_start_ts, preset_bounds[idx]) {
                    (Some(cur), Some(cand)) => cur <= cand,
                    _ => false,
                });
        ranges.insert((*preset).to_owned(), acc.finish(covered));
    }
    ranges.insert(
        "none".to_owned(),
        RangeEstimate {
            covered: state.chosen_range.as_deref() == Some("none"),
            ..RangeEstimate::default()
        },
    );

    let left_out = aggregate_left_out_buckets(active_left_out_reasons);

    let (top_state, substate) = if let Some(reason) = state.attention_reason.as_deref() {
        let sub = match reason {
            "access_limited" => "access_limited",
            "access_off" => "access_off",
            "smart_album_contract_failed" => "album_contract_error",
            _ => "access_off",
        };
        ("needs_attention", sub)
    } else if state.scan_active {
        (
            "setting_up",
            if state.scan_paused {
                "scan_paused"
            } else {
                "scanning"
            },
        )
    } else if !state.scan_complete {
        ("ready_to_set_up", "never_set_up")
    } else if !has_active_choice {
        ("setting_up", "choose")
    } else if state.paused {
        ("paused", "paused")
    } else if waiting_on_phone > 0 {
        ("on", "bringing_in")
    } else {
        ("on", "steady")
    };

    let authorized_cids = read_authorized_client_cids(journal);
    let link_last_seen = read_link_device_last_seen(journal);
    let mut devices: Vec<DeviceState> = devices_file
        .devices
        .into_values()
        .map(|mut dev| {
            dev.connected = device_is_connected(
                &dev,
                authorized_cids.as_ref(),
                live_seen,
                &link_last_seen,
                now,
            );
            dev
        })
        .collect();
    devices.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.cid.cmp(&b.cid)));

    let in_journal_display = if has_active_choice {
        in_journal_in_range.max(live_in_journal)
    } else {
        live_in_journal
    };

    Ok(PhotosDashboardState {
        source: "photos".to_owned(),
        state: top_state.to_owned(),
        substate: substate.to_owned(),
        attention_reason: state.attention_reason,
        devices,
        scan: ScanProgressView {
            items_checked: state.scan_items_checked,
            items_total: state.scan_items_total,
            earliest_item_at: state.scan_earliest_item_at,
            photos_brought_in_so_far: if has_active_choice { arrived } else { 0 },
            paused: state.scan_paused,
        },
        choice: ChoiceView {
            range: state.chosen_range.clone(),
            range_start_at: state.range_start_at.clone(),
            can_widen: state.chosen_range.as_deref() != Some("all"),
        },
        ranges,
        left_out,
        progress: ProgressView {
            chosen_total,
            arrived,
            in_journal: in_journal_display,
            waiting_on_phone,
            moved_to_trash_by_owner: moved_to_trash,
        },
        band4_paused,
        thinking_ready,
        profile_pics_enabled,
        capable_device_kinds: capable_kinds,
    })
}

/// Paginated list of landed `photos` segments for `GET /app/sources/photos/api/photos`.
pub fn list_landed_photos(
    journal: &Path,
    month_filter: Option<&str>,
    cursor: Option<&str>,
    limit: Option<usize>,
) -> Result<LandedPhotosPage, String> {
    let chronicle = journal.join("chronicle");
    if !chronicle.is_dir() {
        return Ok(LandedPhotosPage::default());
    }
    let mut months_set: BTreeSet<String> = BTreeSet::new();
    let mut all_items: Vec<(String, LandedPhotoItem)> = Vec::new();

    let day_entries = fs::read_dir(&chronicle).map_err(|e| e.to_string())?;
    for day_entry in day_entries.filter_map(Result::ok) {
        let Some(day) = day_entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if day.len() != 8 || !day.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let photos_dir = day_entry.path().join("photos");
        if !photos_dir.is_dir() {
            continue;
        }
        let month_key = format!("{}-{}", &day[0..4], &day[4..6]);
        let seg_entries = match fs::read_dir(&photos_dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for seg_entry in seg_entries.filter_map(Result::ok) {
            let seg_path = seg_entry.path();
            if !seg_path.is_dir() || seg_path.join("tombstone.json").is_file() {
                continue;
            }
            let Some(segment) = seg_entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            months_set.insert(month_key.clone());
            if let Some(mf) = month_filter
                && !mf.trim().is_empty()
                && mf.trim() != month_key
            {
                continue;
            }

            let obs = read_or_project_photo_observation(&seg_path, &day, "photos", &segment);
            let meta_json: Option<serde_json::Value> = fs::read(seg_path.join("image_meta.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
            let device_name = meta_json
                .as_ref()
                .and_then(|v| v.get("device_name").and_then(serde_json::Value::as_str))
                .map(str::to_owned);
            let original_file = obs
                .as_ref()
                .map(|o| o.source.artifact_selector.clone())
                .unwrap_or_else(|| "original.jpg".to_owned());
            let captured_at = obs.as_ref().and_then(|o| o.time.captured_at.clone());
            let caption = obs.as_ref().and_then(|o| {
                o.owner_caption
                    .as_ref()
                    .map(|c| c.text.clone())
                    .or_else(|| o.description.as_ref().map(|d| d.text.clone()))
            });
            let has_location = seg_path.join("location.jsonl").is_file();
            let day_dashed = format!("{}-{}-{}", &day[0..4], &day[4..6], &day[6..8]);
            let sort_key = format!(
                "{}|{}|{}",
                captured_at.as_deref().unwrap_or(&day_dashed),
                day,
                segment
            );
            all_items.push((
                sort_key,
                LandedPhotoItem {
                    day: day.clone(),
                    stream: "photos".to_owned(),
                    segment: segment.clone(),
                    captured_at,
                    thumb_url: format!(
                        "/app/transcripts/api/media/{day}/photos/{segment}/{original_file}"
                    ),
                    caption,
                    has_location,
                    device_name,
                },
            ));
        }
    }

    // Newest first.
    all_items.sort_by(|a, b| b.0.cmp(&a.0));

    let page_size = limit.unwrap_or(PHOTOS_PAGE_SIZE).clamp(1, 200);
    let offset: usize = cursor.and_then(|c| c.parse::<usize>().ok()).unwrap_or(0);
    let total = all_items.len();
    let items: Vec<LandedPhotoItem> = all_items
        .into_iter()
        .skip(offset)
        .take(page_size)
        .map(|(_, item)| item)
        .collect();
    let next_cursor = (offset + items.len() < total).then(|| (offset + items.len()).to_string());
    let months: Vec<String> = months_set.into_iter().rev().collect();

    Ok(LandedPhotosPage {
        months,
        items,
        next_cursor,
    })
}
