// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reading Strava workouts from `activities.csv` into tiled body segments.
//!
//! # Release Contract
//!
//! A later change deletes a run. This change does not.
//!
//! 1. Ownership lives in the tile: `workout.json`'s `import_id`. A run's tiles are the live `import.strava` tiles whose `import_id` equals its id, duplicates included. A failed run owns the tiles it wrote and is deletable too.
//! 2. The run delete removes each of its tiles through the retention door with a new reason meaning released with its import run, and removes the run's `imports/<id>/` records. It never removes `imports/.strava-zone.json`.
//! 3. A release tombstone is never written into and never claimed, by anyone. The Strava probe steps past it to the next candidate. It records no owner intent to keep the tile away. The probe stops at any other tombstone or staged removal. Reading the reason is fail-closed: a `tombstone.json` that cannot be read fails the run; one that does not parse, has no reason, or names any reason but the exact release string stops the probe; only an exact match steps past.
//! 4. Importing a released run's file again brings its workouts back, each tile at the next free candidate past its old key, normally `_301`. The slice is unchanged, because it comes from `workout.json`. An owner-deleted tile stays deleted, because its owner tombstone stops the probe wherever it sits: no candidate before an owner tombstone becomes free, since every key goes free, then live, then tombstone, and nothing removes a tombstone. A released tile whose candidates reach another workout's owner tombstone first stays out. A DST fall-back tile whose `_300` twin was owner-deleted stays out. After 99 delete-and-reimport cycles a tile reports `no_free_key`. An operator tool that removes a directory without a tombstone (`segment move`) breaks the invariant, as it does for every importer.
//! 5. No key is rewritten after a deletion, so no key takes a second chain position and the stream chain cannot loop. The probe does not read stream markers or the record's `last_marker` to apply a release. The cost is one candidate per delete-and-reimport cycle.
//! 6. Writers that place by a fixed key (chat, calendar, notes) do not probe, so a release tombstone keeps their segment deleted. A general delete that treats an import as a set needs its own rule. This contract does not claim one.
//! 7. The run delete holds the Strava lock for its whole removal, so a delete cannot race an import's placement.
//! 8. The run-delete change must test: a released tile returns at `_301`; an owner tombstone behind a release stops the probe; an unparseable or unknown reason stops; an I/O error reading a tombstone fails the run; the delete holds the Strava lock; walking `prev` from the record head after delete-and-reimport visits no key twice.

use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::path::Path;

use chrono::{DateTime, Duration, NaiveDateTime, SecondsFormat, TimeZone};
use chrono_tz::Tz;
use serde_json::{Value, json};

use solstone_core_import::RegistrySource;
use solstone_core_ingest_resolve::segment_key_candidates;
use solstone_core_journal_config::{owner_zone, parse_zone};
use solstone_core_journal_io::durability::{ArtifactId, DurableObservation, observe_json_durable};
use solstone_core_journal_io::{AtomicWriteOptions, write_bytes_exclusive};
use solstone_core_segment::owner_deleted;

use crate::save::{RenderedImport, SegmentFile};

const ACTIVITIES_CSV: &str = "activities.csv";
const MAX_HEADER_READ_BYTES: usize = 64 * 1024;
pub const DEFAULT_LIST_CAP: usize = 64 * 1024 * 1024;
const TILE_SCHEMA: &str = "solstone.import.strava.tile.v1";
const ZONE_FILE_REL: &str = "imports/.strava-zone.json";

/// Returns true if the byte buffer's first line has a first comma-separated field
/// of exactly "Activity ID" or "Aktivitäts-ID".
pub fn looks_like_strava_csv_bytes(bytes: &[u8]) -> bool {
    let mut data = bytes;
    if data.starts_with(b"\xef\xbb\xbf") {
        data = &data[3..];
    }
    let end = data
        .iter()
        .position(|&b| b == b'\n' || b == b'\r')
        .unwrap_or(data.len());
    let first_line = match std::str::from_utf8(&data[..end]) {
        Ok(s) => s.trim_end_matches('\r'),
        Err(_) => return false,
    };
    let first_field = first_line.split(',').next().unwrap_or(first_line).trim();
    first_field == "Activity ID" || first_field == "Aktivitäts-ID"
}

fn file_passes_csv_rule(path: &Path) -> bool {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = vec![0u8; MAX_HEADER_READ_BYTES];
    let n = match file.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    looks_like_strava_csv_bytes(&buf[..n])
}

/// Check if a path points to a Strava export, activities CSV, or export directory.
///
/// Any I/O or parse failure returns false.
pub fn looks_like_strava_download(path: &Path, name_hint: Option<&str>) -> bool {
    let name = name_hint
        .filter(|s| !s.is_empty())
        .or_else(|| path.file_name().and_then(|n| n.to_str()))
        .unwrap_or("");

    if name.eq_ignore_ascii_case(ACTIVITIES_CSV) {
        return true;
    }

    if path.is_file() {
        let name_ends_zip = name.to_ascii_lowercase().ends_with(".zip");
        let is_zip = if name_ends_zip {
            true
        } else {
            match File::open(path) {
                Ok(mut f) => {
                    let mut magic = [0u8; 4];
                    f.read_exact(&mut magic).is_ok() && &magic == b"PK\x03\x04"
                }
                Err(_) => false,
            }
        };

        if is_zip {
            let file = match File::open(path) {
                Ok(f) => f,
                Err(_) => return false,
            };
            let archive = match zip::ZipArchive::new(file) {
                Ok(a) => a,
                Err(_) => return false,
            };
            for entry_name in archive.file_names() {
                let parts: Vec<&str> = entry_name
                    .split(['/', '\\'])
                    .filter(|s| !s.is_empty())
                    .collect();
                if (parts.len() == 1 || parts.len() == 2)
                    && parts
                        .last()
                        .is_some_and(|last| last.eq_ignore_ascii_case(ACTIVITIES_CSV))
                {
                    return true;
                }
            }
            return false;
        }

        if name.to_ascii_lowercase().ends_with(".csv") {
            return file_passes_csv_rule(path);
        }

        return false;
    }

    if path.is_dir() {
        let root_activities = path.join(ACTIVITIES_CSV);
        if root_activities.is_file() && file_passes_csv_rule(&root_activities) {
            return true;
        }

        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let child_path = entry.path();
                if child_path.is_dir() {
                    let child_activities = child_path.join(ACTIVITIES_CSV);
                    if child_activities.is_file() && file_passes_csv_rule(&child_activities) {
                        return true;
                    }
                }
            }
        }
    }

    false
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Language {
    English,
    German,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SkipCounts {
    pub id_unparseable: usize,
    pub date_unparseable: usize,
    pub elapsed_unusable: usize,
    pub ragged_or_undecodable: usize,
    pub duplicate_activity_id: usize,
    pub elapsed_over_14_days: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StravaWorkout {
    pub activity_id: u64,
    pub local_start: NaiveDateTime,
    pub activity_name: String,
    pub activity_type: String,
    pub elapsed_seconds: u64,
    pub moving_seconds: Option<i64>,
    pub distance_meters: Option<f64>,
    pub elevation_gain_meters: Option<f64>,
    pub average_heart_rate: Option<f64>,
    pub max_heart_rate: Option<f64>,
    pub average_watts: Option<f64>,
    pub weighted_average_power: Option<f64>,
    pub calories: Option<f64>,
    pub commute: Option<bool>,
    pub entered_by_hand: bool,
}

impl StravaWorkout {
    pub fn start_in(&self, zone: Tz) -> Option<DateTime<Tz>> {
        zone.from_local_datetime(&self.local_start).earliest()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkoutDisposition {
    New,
    PresentUnchanged,
    PresentUpdated,
    PresentCompleted,
    PresentTimingChanged,
    ZoneConflict,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReadWorkouts {
    pub language: Language,
    pub workouts: Vec<StravaWorkout>,
    pub skips: SkipCounts,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TileSlice {
    pub index: usize,
    pub start: DateTime<Tz>,
    pub end: DateTime<Tz>,
    pub seconds: u64,
    pub natural_day: String,
    pub natural_key: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TileAction {
    Created {
        day: String,
        segment: String,
        slice: TileSlice,
        workout: StravaWorkout,
    },
    Updated {
        day: String,
        segment: String,
        stored_import_id: String,
        stored_zone: String,
        slice: TileSlice,
        workout: StravaWorkout,
    },
    Unchanged {
        day: String,
        segment: String,
        activity_id: u64,
        slice: TileSlice,
    },
    StayedDeleted {
        day: String,
        segment: String,
        activity_id: u64,
        slice: TileSlice,
    },
    NoFreeKey {
        natural_day: String,
        natural_key: String,
        activity_id: u64,
        slice: TileSlice,
    },
    DuplicateIdentity {
        day: String,
        segment: String,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlacementCounts {
    pub new_workouts: usize,
    pub tiles_to_create: usize,
    pub present_unchanged: usize,
    pub present_updated: usize,
    pub present_completed: usize,
    pub present_timing_changed: usize,
    /// Workouts absent before whose placement created no tile because a probe stopped at
    /// a deletion. A tombstone carries no workout identity, so the deletion may be another
    /// workout's; this counts what was not brought in, not what the owner deleted.
    pub deleted: usize,
    pub zone_conflict: usize,
    pub complete_workouts: usize,
    pub incomplete_workouts: usize,
    pub created_tiles: usize,
    pub updated_tiles: usize,
    pub unchanged_tiles: usize,
    /// Number of tiles whose probe stopped at an existing deletion. A tombstone carries
    /// no workout identity, and another workout's tombstone counts.
    pub stayed_deleted_tiles: usize,
    pub no_free_key_tiles: usize,
    pub duplicate_identity_tiles: usize,
    pub republished_tiles: usize,
    pub marker_missing_in_chain_tiles: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedTilePublication {
    pub day: String,
    pub segment: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub zone: Tz,
    pub tiles: Vec<TileAction>,
    pub publish: Vec<PlannedTilePublication>,
    pub counts: PlacementCounts,
    pub first_day: Option<String>,
    pub last_day: Option<String>,
    pub workouts: Vec<StravaWorkout>,
    pub dispositions: BTreeMap<u64, WorkoutDisposition>,
}

#[derive(Debug)]
pub enum StravaError {
    ListMissing,
    ZipUnreadable(String),
    ListTooLarge {
        limit: usize,
    },
    LanguageUnsupported,
    LayoutUnrecognised {
        skips: SkipCounts,
    },
    NoWorkouts {
        skips: SkipCounts,
    },
    ZoneUnrecognised(String),
    LockUnavailable,
    TileUnreadable {
        day: String,
        key: String,
        detail: String,
    },
    DayUnreadable {
        day: String,
        detail: String,
    },
    ProbeUnreadable {
        day: String,
        key: String,
        detail: String,
    },
    MarkerUnreadable {
        day: String,
        key: String,
        detail: String,
    },
    MarkerUnparseable {
        day: String,
        key: String,
    },
    StreamRecordUnreadable(String),
}

impl StravaError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ListMissing => "strava_list_missing",
            Self::ZipUnreadable(_) => "strava_zip_unreadable",
            Self::ListTooLarge { .. } => "strava_list_too_large",
            Self::LanguageUnsupported => "strava_language_unsupported",
            Self::LayoutUnrecognised { .. } => "strava_layout_unrecognised",
            Self::NoWorkouts { .. } => "strava_no_workouts",
            Self::ZoneUnrecognised(_) => "strava_zone_unrecognised",
            Self::LockUnavailable => "strava_lock_unavailable",
            Self::TileUnreadable { .. } => "strava_tile_unreadable",
            Self::DayUnreadable { .. } => "strava_day_unreadable",
            Self::ProbeUnreadable { .. } => "strava_probe_unreadable",
            Self::MarkerUnreadable { .. } => "strava_marker_unreadable",
            Self::MarkerUnparseable { .. } => "strava_marker_unparseable",
            Self::StreamRecordUnreadable(_) => "strava_stream_record_unreadable",
        }
    }
}

impl fmt::Display for StravaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ListMissing => write!(f, "{}: activities list missing", self.code()),
            Self::ZipUnreadable(detail) => write!(f, "{}: zip unreadable: {detail}", self.code()),
            Self::ListTooLarge { limit } => {
                write!(
                    f,
                    "{}: list exceeded size cap of {limit} bytes",
                    self.code()
                )
            }
            Self::LanguageUnsupported => {
                write!(f, "{}: unsupported activities language header", self.code())
            }
            Self::LayoutUnrecognised { .. } => {
                write!(f, "{}: activities CSV layout not recognised", self.code())
            }
            Self::NoWorkouts { .. } => {
                write!(
                    f,
                    "{}: no valid workouts found in activities list",
                    self.code()
                )
            }
            Self::ZoneUnrecognised(detail) => {
                write!(f, "{}: zone record unrecognised: {detail}", self.code())
            }
            Self::LockUnavailable => {
                write!(f, "{}: failed to acquire Strava ingest lock", self.code())
            }
            Self::TileUnreadable { day, key, detail } => {
                write!(f, "{}: tile {day}/{key} unreadable: {detail}", self.code())
            }
            Self::DayUnreadable { day, detail } => {
                write!(f, "{}: day {day} unreadable: {detail}", self.code())
            }
            Self::ProbeUnreadable { day, key, detail } => {
                write!(f, "{}: probe {day}/{key} unreadable: {detail}", self.code())
            }
            Self::MarkerUnreadable { day, key, detail } => {
                write!(
                    f,
                    "{}: marker {day}/{key} unreadable: {detail}",
                    self.code()
                )
            }
            Self::MarkerUnparseable { day, key } => {
                write!(f, "{}: marker {day}/{key} unparseable", self.code())
            }
            Self::StreamRecordUnreadable(detail) => {
                write!(f, "{}: stream record unreadable: {detail}", self.code())
            }
        }
    }
}

impl std::error::Error for StravaError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultKind {
    WorkoutFile,
    DayList,
    OwnerDeleted,
    StreamMarker,
    StreamRecord,
}

pub struct ReadFault {
    pub kind: FaultKind,
    pub fail_on: u32,
    pub seen: Cell<u32>,
}

impl ReadFault {
    pub fn single(kind: FaultKind) -> Self {
        Self {
            kind,
            fail_on: 1,
            seen: Cell::new(0),
        }
    }

    pub fn check(&self, target: FaultKind) -> bool {
        if self.kind == target {
            let count = self.seen.get() + 1;
            self.seen.set(count);
            count == self.fail_on
        } else {
            false
        }
    }
}

struct ColumnPositions {
    activity_id: usize,
    activity_date: usize,
    activity_name: usize,
    activity_type: usize,
    elapsed_time: usize,
    moving_time: usize,
    distance: usize,
    elevation_gain: usize,
    average_heart_rate: usize,
    max_heart_rate: usize,
    average_watts: usize,
    weighted_average_power: usize,
    calories: usize,
    commute: usize,
    filename: usize,
}

fn resolve_columns(headers: &csv::ByteRecord, language: Language) -> Result<ColumnPositions, ()> {
    let find_nth = |target_name: &str, occurrence: usize| -> Option<usize> {
        let mut count = 0;
        for (i, field) in headers.iter().enumerate() {
            if let Ok(name) = std::str::from_utf8(field)
                && name == target_name
            {
                count += 1;
                if count == occurrence {
                    return Some(i);
                }
            }
        }
        None
    };

    let (
        id_name,
        date_name,
        name_name,
        type_name,
        elapsed_name,
        moving_name,
        dist_name,
        elev_name,
        avg_hr_name,
        max_hr_name,
        avg_w_name,
        weighted_w_name,
        cal_name,
        commute_name,
        fn_name,
    ) = match language {
        Language::English => (
            "Activity ID",
            "Activity Date",
            "Activity Name",
            "Activity Type",
            "Elapsed Time",
            "Moving Time",
            "Distance",
            "Elevation Gain",
            "Average Heart Rate",
            "Max Heart Rate",
            "Average Watts",
            "Weighted Average Power",
            "Calories",
            "Commute",
            "Filename",
        ),
        Language::German => (
            "Aktivitäts-ID",
            "Aktivitätsdatum",
            "Name der Aktivität",
            "Aktivitätsart",
            "Verstrichene Zeit",
            "Bewegungszeit",
            "Distanz",
            "Höhenzunahme",
            "Durchschnittliche Herzfrequenz",
            "Max. Herzfrequenz",
            "Durchschnittliche Watt",
            "Gewichtete durchschnittliche Leistung",
            "Kalorien",
            "Pendeln",
            "Dateiname",
        ),
    };

    Ok(ColumnPositions {
        activity_id: find_nth(id_name, 1).ok_or(())?,
        activity_date: find_nth(date_name, 1).ok_or(())?,
        activity_name: find_nth(name_name, 1).ok_or(())?,
        activity_type: find_nth(type_name, 1).ok_or(())?,
        elapsed_time: find_nth(elapsed_name, 2).ok_or(())?,
        moving_time: find_nth(moving_name, 1).ok_or(())?,
        distance: find_nth(dist_name, 2).ok_or(())?,
        elevation_gain: find_nth(elev_name, 1).ok_or(())?,
        average_heart_rate: find_nth(avg_hr_name, 1).ok_or(())?,
        max_heart_rate: find_nth(max_hr_name, 2).ok_or(())?,
        average_watts: find_nth(avg_w_name, 1).ok_or(())?,
        weighted_average_power: find_nth(weighted_w_name, 1).ok_or(())?,
        calories: find_nth(cal_name, 1).ok_or(())?,
        commute: find_nth(commute_name, 2).ok_or(())?,
        filename: find_nth(fn_name, 1).ok_or(())?,
    })
}

fn parse_activity_date(raw: &str, language: Language) -> Option<NaiveDateTime> {
    let normalized = raw.replace(['\u{202f}', '\u{00a0}'], " ");
    let trimmed = normalized.trim();
    match language {
        Language::English => NaiveDateTime::parse_from_str(trimmed, "%b %d, %Y, %I:%M:%S %p")
            .or_else(|_| NaiveDateTime::parse_from_str(trimmed, "%d %b %Y, %H:%M:%S"))
            .ok(),
        Language::German => NaiveDateTime::parse_from_str(trimmed, "%d.%m.%Y, %H:%M:%S").ok(),
    }
}

pub fn read_workouts(path: &Path, max_bytes: usize) -> Result<ReadWorkouts, StravaError> {
    if !path.exists() {
        return Err(StravaError::ListMissing);
    }

    let csv_bytes = if path.is_file() {
        let is_zip = if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
        {
            true
        } else {
            match File::open(path) {
                Ok(mut f) => {
                    let mut magic = [0u8; 4];
                    f.read_exact(&mut magic).is_ok() && &magic == b"PK\x03\x04"
                }
                Err(e) => return Err(StravaError::ZipUnreadable(e.to_string())),
            }
        };

        if is_zip {
            let file = File::open(path).map_err(|e| StravaError::ZipUnreadable(e.to_string()))?;
            let mut archive = zip::ZipArchive::new(file)
                .map_err(|e| StravaError::ZipUnreadable(e.to_string()))?;

            let mut exact_index = None;
            for i in 0..archive.len() {
                let name = match archive.by_index_raw(i) {
                    Ok(entry) => entry.name().to_owned(),
                    Err(e) => return Err(StravaError::ZipUnreadable(e.to_string())),
                };
                if name == ACTIVITIES_CSV {
                    exact_index = Some(i);
                    break;
                }
            }

            let Some(index) = exact_index else {
                return Err(StravaError::ListMissing);
            };

            let mut zip_file = archive
                .by_index(index)
                .map_err(|e| StravaError::ZipUnreadable(e.to_string()))?;
            if zip_file.size() > max_bytes as u64 {
                return Err(StravaError::ListTooLarge { limit: max_bytes });
            }

            let mut bytes = Vec::new();
            let mut chunk = [0u8; 8192];
            let mut total_read = 0_usize;
            loop {
                let n = zip_file
                    .read(&mut chunk)
                    .map_err(|e| StravaError::ZipUnreadable(e.to_string()))?;
                if n == 0 {
                    break;
                }
                total_read += n;
                if total_read > max_bytes {
                    return Err(StravaError::ListTooLarge { limit: max_bytes });
                }
                bytes.extend_from_slice(&chunk[..n]);
            }
            bytes
        } else {
            let metadata = fs::metadata(path).map_err(|_| StravaError::ListMissing)?;
            if metadata.len() > max_bytes as u64 {
                return Err(StravaError::ListTooLarge { limit: max_bytes });
            }
            let mut file = File::open(path).map_err(|_| StravaError::ListMissing)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|_| StravaError::ListMissing)?;
            if bytes.len() > max_bytes {
                return Err(StravaError::ListTooLarge { limit: max_bytes });
            }
            bytes
        }
    } else {
        return Err(StravaError::ListMissing);
    };

    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .has_headers(true)
        .from_reader(Cursor::new(csv_bytes.as_slice()));

    let headers = reader
        .byte_headers()
        .map_err(|_| StravaError::LanguageUnsupported)?
        .clone();
    let first_field = headers.get(0).unwrap_or(b"");
    let mut first_bytes = first_field;
    if first_bytes.starts_with(b"\xef\xbb\xbf") {
        first_bytes = &first_bytes[3..];
    }
    let first_str = std::str::from_utf8(first_bytes)
        .map_err(|_| StravaError::LanguageUnsupported)?
        .trim();

    let language = if first_str == "Activity ID" {
        Language::English
    } else if first_str == "Aktivitäts-ID" {
        Language::German
    } else {
        return Err(StravaError::LanguageUnsupported);
    };

    let cols = match resolve_columns(&headers, language) {
        Ok(cols) => cols,
        Err(()) => {
            return Err(StravaError::LayoutUnrecognised {
                skips: SkipCounts::default(),
            });
        }
    };

    let mut skips = SkipCounts::default();
    let mut workouts = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut decoded_rows_count = 0_usize;
    let mut dates_parsed_count = 0_usize;

    let mut record = csv::ByteRecord::new();
    let mut cursor_offset = 0_usize;

    // Find the end of the header line in csv_bytes to resume on error
    if let Some(pos) = csv_bytes.iter().position(|&b| b == b'\n') {
        cursor_offset = pos + 1;
    }

    loop {
        match reader.read_byte_record(&mut record) {
            Ok(true) => {
                if record.len() != headers.len() {
                    skips.ragged_or_undecodable += 1;
                    continue;
                }

                let mut fields = Vec::with_capacity(record.len());
                let mut utf8_err = false;
                for field in record.iter() {
                    match std::str::from_utf8(field) {
                        Ok(s) => fields.push(s),
                        Err(_) => {
                            utf8_err = true;
                            break;
                        }
                    }
                }
                if utf8_err {
                    skips.ragged_or_undecodable += 1;
                    continue;
                }

                decoded_rows_count += 1;

                let date_parsed = parse_activity_date(fields[cols.activity_date], language);
                if date_parsed.is_some() {
                    dates_parsed_count += 1;
                }

                let id_raw = fields[cols.activity_id].trim();
                let activity_id = match id_raw.parse::<u64>() {
                    Ok(id) => id,
                    Err(_) => {
                        skips.id_unparseable += 1;
                        continue;
                    }
                };

                let local_start = match date_parsed {
                    Some(dt) => dt,
                    None => {
                        skips.date_unparseable += 1;
                        continue;
                    }
                };

                let elapsed_raw = fields[cols.elapsed_time].trim();
                let elapsed_seconds = match elapsed_raw.parse::<f64>() {
                    Ok(f) if f.is_finite() && f >= 0.0 => {
                        let rounded = f.round();
                        if rounded < 0.0 {
                            skips.elapsed_unusable += 1;
                            continue;
                        }
                        rounded as u64
                    }
                    _ => {
                        skips.elapsed_unusable += 1;
                        continue;
                    }
                };

                if seen_ids.contains(&activity_id) {
                    skips.duplicate_activity_id += 1;
                    continue;
                }

                const MAX_14_DAYS_SECS: u64 = 14 * 24 * 60 * 60;
                if elapsed_seconds > MAX_14_DAYS_SECS {
                    skips.elapsed_over_14_days += 1;
                    continue;
                }

                seen_ids.insert(activity_id);

                let activity_name = fields[cols.activity_name].to_owned();
                let activity_type = fields[cols.activity_type].to_owned();

                let parse_opt_f64 = |raw: &str| -> Option<f64> {
                    let t = raw.trim();
                    if t.is_empty() {
                        return None;
                    }
                    match t.parse::<f64>() {
                        Ok(v) if v.is_finite() => Some(v),
                        _ => None,
                    }
                };

                let moving_seconds = {
                    let raw = fields[cols.moving_time].trim();
                    if raw.is_empty() {
                        None
                    } else {
                        match raw.parse::<f64>() {
                            Ok(f) if f.is_finite() => Some(f.round() as i64),
                            _ => None,
                        }
                    }
                };

                let distance_meters = parse_opt_f64(fields[cols.distance]);
                let elevation_gain_meters = parse_opt_f64(fields[cols.elevation_gain]);
                let average_heart_rate = parse_opt_f64(fields[cols.average_heart_rate]);
                let max_heart_rate = parse_opt_f64(fields[cols.max_heart_rate]);
                let average_watts = parse_opt_f64(fields[cols.average_watts]);
                let weighted_average_power = parse_opt_f64(fields[cols.weighted_average_power]);
                let calories = parse_opt_f64(fields[cols.calories]);

                let commute = {
                    let t = fields[cols.commute].trim();
                    if t == "1.0" || t == "true" {
                        Some(true)
                    } else if t == "0.0" || t == "false" {
                        Some(false)
                    } else {
                        None
                    }
                };

                let entered_by_hand = fields[cols.filename].trim().is_empty();

                workouts.push(StravaWorkout {
                    activity_id,
                    local_start,
                    activity_name,
                    activity_type,
                    elapsed_seconds,
                    moving_seconds,
                    distance_meters,
                    elevation_gain_meters,
                    average_heart_rate,
                    max_heart_rate,
                    average_watts,
                    weighted_average_power,
                    calories,
                    commute,
                    entered_by_hand,
                });
            }
            Ok(false) => break,
            Err(err) => {
                skips.ragged_or_undecodable += 1;
                // Resume from unread tail
                let err_pos = err
                    .position()
                    .map(|p| p.byte() as usize)
                    .unwrap_or(cursor_offset);
                if let Some(next_nl) = csv_bytes[err_pos..].iter().position(|&b| b == b'\n') {
                    cursor_offset = err_pos + next_nl + 1;
                    if cursor_offset >= csv_bytes.len() {
                        break;
                    }
                    let remaining = &csv_bytes[cursor_offset..];
                    reader = csv::ReaderBuilder::new()
                        .flexible(true)
                        .has_headers(false)
                        .from_reader(Cursor::new(remaining));
                } else {
                    break;
                }
            }
        }
    }
    if decoded_rows_count > 0 && dates_parsed_count == 0 {
        return Err(StravaError::LayoutUnrecognised { skips });
    }

    if workouts.is_empty() {
        return Err(StravaError::NoWorkouts { skips });
    }

    Ok(ReadWorkouts {
        language,
        workouts,
        skips,
    })
}

pub fn tile_workout(workout: &StravaWorkout, zone: Tz) -> Vec<TileSlice> {
    let Some(start) = workout.start_in(zone) else {
        return Vec::new();
    };

    let effective_elapsed = workout.elapsed_seconds.max(1);
    let tile_count = effective_elapsed.div_ceil(300);
    let mut slices = Vec::with_capacity(tile_count as usize);

    for i in 0..tile_count {
        let tile_start = start + Duration::seconds((300 * i) as i64);
        let tile_seconds = 300.min(effective_elapsed - 300 * i);
        let tile_end = tile_start + Duration::seconds(tile_seconds as i64);
        let natural_day = tile_start.format("%Y%m%d").to_string();
        let natural_key = format!("{}_{}", tile_start.format("%H%M%S"), tile_seconds);

        slices.push(TileSlice {
            index: i as usize,
            start: tile_start,
            end: tile_end,
            seconds: tile_seconds,
            natural_day,
            natural_key,
        });
    }

    slices
}

pub fn read_zone_record(journal: &Path) -> Result<Option<Tz>, StravaError> {
    let path = journal.join(ZONE_FILE_REL);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => return Err(StravaError::ZoneUnrecognised(e.to_string())),
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return Err(StravaError::ZoneUnrecognised(e.to_string())),
    };
    let Some(zone_str) = value.get("zone").and_then(Value::as_str) else {
        return Err(StravaError::ZoneUnrecognised(
            "missing zone field".to_owned(),
        ));
    };
    match parse_zone(zone_str) {
        Some(tz) => Ok(Some(tz)),
        None => Err(StravaError::ZoneUnrecognised(format!(
            "unknown IANA zone: {zone_str}"
        ))),
    }
}

pub fn create_zone_record(journal: &Path, zone: Tz) -> Result<(), StravaError> {
    let path = journal.join(ZONE_FILE_REL);
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let body = format!("{{\"zone\":\"{}\"}}\n", zone.name());
    match write_bytes_exclusive(
        &path,
        body.as_bytes(),
        AtomicWriteOptions { mode: Some(0o600) },
    ) {
        Ok(()) => Ok(()),
        Err(solstone_core_journal_io::AtomicWriteError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::AlreadyExists =>
        {
            Ok(())
        }
        Err(e) => Err(StravaError::ZoneUnrecognised(e.to_string())),
    }
}

#[derive(Clone, Debug)]
struct LiveTile {
    day: String,
    key: String,
    day_num: u64,
    time_num: u64,
    len_num: u64,
    activity_id: u64,
    index: usize,
    workout_start: Option<DateTime<Tz>>,
    workout_elapsed: u64,
    import_id: String,
    zone_str: String,
    zone: Option<Tz>,
    has_marker: bool,
    full_workout_json: Value,
}

fn parse_numeric_key(day: &str, key: &str) -> Option<(u64, u64, u64)> {
    let day_num = day.parse::<u64>().ok()?;
    let mut parts = key.split('_');
    let time_num = parts.next()?.parse::<u64>().ok()?;
    let len_num = parts.next()?.parse::<u64>().ok()?;
    Some((day_num, time_num, len_num))
}

/// The tombstone reason a piece carries when its whole import run was deleted.
pub const RELEASE_REASON: &str = "import_run_release";

/// Whether a deleted key holds a piece that was released with its import run.
///
/// Deleting a whole import run releases its pieces: importing its download again
/// brings them back, one key past where they were. Only a readable tombstone whose
/// reason is exactly [`RELEASE_REASON`] counts, and only with no removal still under
/// way at that key. Any other tombstone, including one that doesn't parse or names
/// another reason, keeps the probe stopped there, so a piece the owner deleted on its
/// own stays deleted. A tombstone that can't be read is an error, never a release.
/// Nothing ever writes into or removes a released key.
fn released_with_import(segment_dir: &Path) -> Result<bool, String> {
    let (Some(parent), Some(name)) = (segment_dir.parent(), segment_dir.file_name()) else {
        return Ok(false);
    };
    let mut staged = std::ffi::OsString::from(".removing_");
    staged.push(name);
    match fs::symlink_metadata(parent.join(staged)) {
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "reading the removal beside this deleted piece: {e}"
            ));
        }
    }
    let bytes = match fs::read(segment_dir.join("tombstone.json")) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("reading this deleted piece's tombstone: {e}")),
    };
    let reason = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| {
            value
                .get("reason")
                .and_then(|r| r.as_str())
                .map(str::to_owned)
        });
    Ok(reason.as_deref() == Some(RELEASE_REASON))
}

pub fn place(
    journal: &Path,
    workouts: &[StravaWorkout],
    record_zone: Option<Tz>,
    fault: Option<&ReadFault>,
) -> Result<Placement, StravaError> {
    let days =
        solstone_core_segment::list_days(journal).map_err(|e| StravaError::DayUnreadable {
            day: String::new(),
            detail: e.to_string(),
        })?;

    if let Some(f) = fault
        && f.check(FaultKind::StreamRecord)
    {
        return Err(StravaError::StreamRecordUnreadable(
            "injected StreamRecord fault".to_owned(),
        ));
    }

    let stream_record: Option<Value> =
        solstone_core_segment::read_stream_record(journal, "import.strava")
            .map_err(|e| StravaError::StreamRecordUnreadable(e.to_string()))?;

    let mut inventory: Vec<LiveTile> = Vec::new();
    let mut marker_prev_set: HashSet<(String, String)> = HashSet::new();

    for (day, day_dir) in &days {
        let stream_dir = day_dir.join("import.strava");
        if !stream_dir.is_dir() {
            continue;
        }

        if let Some(f) = fault
            && f.check(FaultKind::DayList)
        {
            return Err(StravaError::DayUnreadable {
                day: day.clone(),
                detail: "injected DayList fault".to_owned(),
            });
        }

        let entries = match fs::read_dir(&stream_dir) {
            Ok(entries) => entries,
            Err(e) => {
                return Err(StravaError::DayUnreadable {
                    day: day.clone(),
                    detail: e.to_string(),
                });
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    return Err(StravaError::DayUnreadable {
                        day: day.clone(),
                        detail: e.to_string(),
                    });
                }
            };
            let seg_dir = entry.path();
            if !seg_dir.is_dir() {
                continue;
            }
            let key = entry.file_name().to_string_lossy().into_owned();

            if let Some(f) = fault
                && f.check(FaultKind::OwnerDeleted)
            {
                return Err(StravaError::ProbeUnreadable {
                    day: day.clone(),
                    key: key.clone(),
                    detail: "injected OwnerDeleted fault".to_owned(),
                });
            }

            let is_del = match owner_deleted(&seg_dir) {
                Ok(del) => del,
                Err(e) => {
                    return Err(StravaError::ProbeUnreadable {
                        day: day.clone(),
                        key: key.clone(),
                        detail: e.to_string(),
                    });
                }
            };

            let marker_path = seg_dir.join("stream.json");
            let mut has_marker = false;
            if !is_del && marker_path.exists() {
                if let Some(f) = fault
                    && f.check(FaultKind::StreamMarker)
                {
                    return Err(StravaError::MarkerUnreadable {
                        day: day.clone(),
                        key: key.clone(),
                        detail: "injected StreamMarker fault".to_owned(),
                    });
                }
                match observe_json_durable::<Value>(ArtifactId::SegmentStream, &marker_path) {
                    DurableObservation::Present(val) => {
                        has_marker = true;
                        if let Some(prev_day) = val.get("prev_day").and_then(Value::as_str)
                            && let Some(prev_seg) = val.get("prev_segment").and_then(Value::as_str)
                        {
                            marker_prev_set.insert((prev_day.to_owned(), prev_seg.to_owned()));
                        }
                    }
                    DurableObservation::Absent => {}
                    DurableObservation::Malformed { .. } => {
                        let content = fs::read(&marker_path).unwrap_or_default();
                        if !content.is_empty() {
                            return Err(StravaError::MarkerUnparseable {
                                day: day.clone(),
                                key: key.clone(),
                            });
                        }
                    }
                    DurableObservation::Unreadable { source, .. } => {
                        return Err(StravaError::MarkerUnreadable {
                            day: day.clone(),
                            key: key.clone(),
                            detail: source.to_string(),
                        });
                    }
                }
            }

            if is_del {
                continue;
            }

            let workout_path = seg_dir.join("workout.json");
            if workout_path.exists() {
                if let Some(f) = fault
                    && f.check(FaultKind::WorkoutFile)
                {
                    return Err(StravaError::TileUnreadable {
                        day: day.clone(),
                        key: key.clone(),
                        detail: "injected WorkoutFile fault".to_owned(),
                    });
                }
                let bytes = match fs::read(&workout_path) {
                    Ok(b) => b,
                    Err(e) => {
                        return Err(StravaError::TileUnreadable {
                            day: day.clone(),
                            key: key.clone(),
                            detail: e.to_string(),
                        });
                    }
                };
                if let Ok(val) = serde_json::from_slice::<Value>(&bytes)
                    && val.get("schema").and_then(Value::as_str) == Some(TILE_SCHEMA)
                    && let Some((day_num, time_num, len_num)) = parse_numeric_key(day, &key)
                {
                    let activity_id = val.get("activity_id").and_then(Value::as_u64).unwrap_or(0);
                    let index = val
                        .get("tile")
                        .and_then(|t| t.get("index"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize;
                    let import_id = val
                        .get("import_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let zone_str = val
                        .get("zone")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let zone = parse_zone(&zone_str);

                    let parse_dt = |s: &str, z: Option<Tz>| -> Option<DateTime<Tz>> {
                        let z = z?;
                        DateTime::parse_from_rfc3339(s)
                            .ok()?
                            .with_timezone(&z)
                            .into()
                    };

                    let workout_start_str = val
                        .get("workout")
                        .and_then(|w| w.get("start"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let workout_start = parse_dt(workout_start_str, zone);
                    let workout_elapsed = val
                        .get("workout")
                        .and_then(|w| w.get("elapsed_seconds"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);

                    inventory.push(LiveTile {
                        day: day.clone(),
                        key: key.clone(),
                        day_num,
                        time_num,
                        len_num,
                        activity_id,
                        index,
                        workout_start,
                        workout_elapsed,
                        import_id,
                        zone_str,
                        zone,
                        has_marker,
                        full_workout_json: val,
                    });
                }
            }
        }
    }

    let default_zone = match record_zone {
        Some(tz) => tz,
        None => {
            let mut candidates = inventory.clone();
            candidates.sort_by_key(|t| (t.day_num, t.time_num, t.len_num));
            candidates
                .into_iter()
                .find_map(|t| t.zone)
                .unwrap_or_else(|| owner_zone(journal))
        }
    };

    let mut canonical_tiles_by_identity: BTreeMap<(u64, usize), LiveTile> = BTreeMap::new();
    let mut duplicate_identity_tiles: Vec<(String, String)> = Vec::new();

    let mut sorted_inventory = inventory.clone();
    sorted_inventory.sort_by_key(|t| (t.day_num, t.time_num, t.len_num));

    for tile in sorted_inventory {
        let key = (tile.activity_id, tile.index);
        if let std::collections::btree_map::Entry::Vacant(e) =
            canonical_tiles_by_identity.entry(key)
        {
            e.insert(tile);
        } else {
            duplicate_identity_tiles.push((tile.day.clone(), tile.key.clone()));
        }
    }

    let mut workout_zones: BTreeMap<u64, Result<Tz, ()>> = BTreeMap::new();
    for workout in workouts {
        let matching_canonical: Vec<&LiveTile> = canonical_tiles_by_identity
            .values()
            .filter(|t| t.activity_id == workout.activity_id)
            .collect();
        if matching_canonical.is_empty() {
            workout_zones.insert(workout.activity_id, Ok(default_zone));
        } else {
            let first_zone = matching_canonical[0].zone;
            if let Some(fz) = first_zone {
                if matching_canonical
                    .iter()
                    .all(|t| t.zone == first_zone && t.zone_str == matching_canonical[0].zone_str)
                {
                    workout_zones.insert(workout.activity_id, Ok(fz));
                } else {
                    workout_zones.insert(workout.activity_id, Err(()));
                }
            } else {
                workout_zones.insert(workout.activity_id, Err(()));
            }
        }
    }

    // Filter out workouts where start_in returns None (date_unparseable in this zone)
    let mut date_unparseable_skips = 0_usize;
    let mut valid_workouts = Vec::new();
    for workout in workouts {
        let zone = workout_zones
            .get(&workout.activity_id)
            .copied()
            .unwrap_or(Ok(default_zone))
            .unwrap_or(default_zone);
        if workout.start_in(zone).is_some() {
            valid_workouts.push(workout.clone());
        } else {
            date_unparseable_skips += 1;
        }
    }

    if valid_workouts.is_empty() {
        return Err(StravaError::NoWorkouts {
            skips: SkipCounts {
                date_unparseable: date_unparseable_skips,
                ..Default::default()
            },
        });
    }

    let mut sorted_workouts = valid_workouts;
    sorted_workouts.sort_by(|a, b| {
        let zone_a = workout_zones
            .get(&a.activity_id)
            .copied()
            .unwrap_or(Ok(default_zone))
            .unwrap_or(default_zone);
        let zone_b = workout_zones
            .get(&b.activity_id)
            .copied()
            .unwrap_or(Ok(default_zone))
            .unwrap_or(default_zone);
        let start_a = a
            .start_in(zone_a)
            .map(|dt| dt.timestamp())
            .unwrap_or(i64::MIN);
        let start_b = b
            .start_in(zone_b)
            .map(|dt| dt.timestamp())
            .unwrap_or(i64::MIN);
        start_a
            .cmp(&start_b)
            .then_with(|| a.activity_id.cmp(&b.activity_id))
    });

    let mut tile_actions = Vec::new();
    let mut publish_list = Vec::new();
    let mut dispositions: BTreeMap<u64, WorkoutDisposition> = BTreeMap::new();
    let mut counts = PlacementCounts {
        duplicate_identity_tiles: duplicate_identity_tiles.len(),
        ..Default::default()
    };

    let mut claimed_keys: HashSet<(String, String)> = HashSet::new();
    let mut checked_owner_deleted_probe: HashSet<(String, String)> = HashSet::new();

    let mut first_day: Option<String> = None;
    let mut last_day: Option<String> = None;

    let update_days = |day: &str, first: &mut Option<String>, last: &mut Option<String>| {
        match first {
            Some(f) if day < f.as_str() => *f = day.to_owned(),
            None => *first = Some(day.to_owned()),
            _ => {}
        }
        match last {
            Some(l) if day > l.as_str() => *l = day.to_owned(),
            None => *last = Some(day.to_owned()),
            _ => {}
        }
    };

    for workout in &sorted_workouts {
        let zone_res = workout_zones
            .get(&workout.activity_id)
            .copied()
            .unwrap_or(Ok(default_zone));

        let workout_zone = match zone_res {
            Ok(z) => z,
            Err(()) => {
                let canonical_tiles: Vec<&LiveTile> = canonical_tiles_by_identity
                    .values()
                    .filter(|t| t.activity_id == workout.activity_id)
                    .collect();
                for t in &canonical_tiles {
                    counts.unchanged_tiles += 1;
                    tile_actions.push(TileAction::Unchanged {
                        day: t.day.clone(),
                        segment: t.key.clone(),
                        activity_id: workout.activity_id,
                        slice: TileSlice {
                            index: t.index,
                            start: t.workout_start.unwrap_or_else(|| {
                                default_zone.from_utc_datetime(&workout.local_start)
                            }),
                            end: t.workout_start.unwrap_or_else(|| {
                                default_zone.from_utc_datetime(&workout.local_start)
                            }),
                            seconds: 300,
                            natural_day: t.day.clone(),
                            natural_key: t.key.clone(),
                        },
                    });
                }
                let total_tiles = workout.elapsed_seconds.max(1).div_ceil(300) as usize;
                if canonical_tiles.len() == total_tiles {
                    counts.complete_workouts += 1;
                } else {
                    counts.incomplete_workouts += 1;
                }
                counts.zone_conflict += 1;
                dispositions.insert(workout.activity_id, WorkoutDisposition::ZoneConflict);
                continue;
            }
        };

        let slices = tile_workout(workout, workout_zone);
        if slices.is_empty() {
            counts.incomplete_workouts += 1;
            continue;
        }

        for slice in &slices {
            update_days(&slice.natural_day, &mut first_day, &mut last_day);
        }

        let canonical_tiles: Vec<&LiveTile> = canonical_tiles_by_identity
            .values()
            .filter(|t| t.activity_id == workout.activity_id)
            .collect();

        if !canonical_tiles.is_empty() {
            let timing_changed = canonical_tiles.iter().any(|t| {
                let workout_start_instant = workout.start_in(workout_zone).map(|dt| dt.timestamp());
                let tile_start_instant = t.workout_start.map(|dt| dt.timestamp());
                workout_start_instant != tile_start_instant
                    || workout.elapsed_seconds != t.workout_elapsed
            });

            if timing_changed {
                for t in &canonical_tiles {
                    counts.unchanged_tiles += 1;
                    tile_actions.push(TileAction::Unchanged {
                        day: t.day.clone(),
                        segment: t.key.clone(),
                        activity_id: workout.activity_id,
                        slice: TileSlice {
                            index: t.index,
                            start: t.workout_start.unwrap_or_else(|| {
                                workout_zone.from_utc_datetime(&workout.local_start)
                            }),
                            end: t.workout_start.unwrap_or_else(|| {
                                workout_zone.from_utc_datetime(&workout.local_start)
                            }),
                            seconds: 300,
                            natural_day: t.day.clone(),
                            natural_key: t.key.clone(),
                        },
                    });
                }
                let total_tiles = workout.elapsed_seconds.max(1).div_ceil(300) as usize;
                if canonical_tiles.len() == total_tiles {
                    counts.complete_workouts += 1;
                } else {
                    counts.incomplete_workouts += 1;
                }
                counts.present_timing_changed += 1;
                dispositions.insert(
                    workout.activity_id,
                    WorkoutDisposition::PresentTimingChanged,
                );
                continue;
            }
        }

        let had_canonical_before = !canonical_tiles.is_empty();
        let mut had_created_in_workout = false;
        let mut had_stayed_deleted_in_workout = false;
        let mut had_updated_in_workout = false;
        let mut all_indices_live = true;

        for slice in slices {
            let canon = canonical_tiles_by_identity.get(&(workout.activity_id, slice.index));
            if let Some(stored) = canon {
                let json_matches = {
                    let w = stored.full_workout_json.get("workout");
                    let name_eq = w.and_then(|w| w.get("name")).and_then(Value::as_str)
                        == Some(&workout.activity_name);
                    let type_eq = w.and_then(|w| w.get("type")).and_then(Value::as_str)
                        == Some(&workout.activity_type);
                    let hand_eq = w
                        .and_then(|w| w.get("entered_by_hand"))
                        .and_then(Value::as_bool)
                        == Some(workout.entered_by_hand);
                    let commute_eq = match (w.and_then(|w| w.get("commute")), workout.commute) {
                        (Some(Value::Bool(b)), Some(wb)) => *b == wb,
                        (Some(Value::Null), None) | (None, None) => true,
                        _ => false,
                    };
                    let opt_f64_eq = |key: &str, target: Option<f64>| -> bool {
                        match (w.and_then(|w| w.get(key)), target) {
                            (Some(Value::Number(n)), Some(tn)) => n.as_f64() == Some(tn),
                            (Some(Value::Null), None) | (None, None) => true,
                            _ => false,
                        }
                    };
                    let opt_i64_eq = |key: &str, target: Option<i64>| -> bool {
                        match (w.and_then(|w| w.get(key)), target) {
                            (Some(Value::Number(n)), Some(tn)) => n.as_i64() == Some(tn),
                            (Some(Value::Null), None) | (None, None) => true,
                            _ => false,
                        }
                    };

                    let start_eq = {
                        let raw_st = w
                            .and_then(|w| w.get("start"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let stored_inst = DateTime::parse_from_rfc3339(raw_st)
                            .ok()
                            .map(|dt| dt.timestamp());
                        let target_inst = workout.start_in(workout_zone).map(|dt| dt.timestamp());
                        stored_inst == target_inst
                    };

                    let tile_obj = stored.full_workout_json.get("tile");
                    let tile_start_eq = {
                        let raw_st = tile_obj
                            .and_then(|t| t.get("start"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        DateTime::parse_from_rfc3339(raw_st)
                            .ok()
                            .map(|dt| dt.timestamp())
                            == Some(slice.start.timestamp())
                    };
                    let tile_end_eq = {
                        let raw_end = tile_obj
                            .and_then(|t| t.get("end"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        DateTime::parse_from_rfc3339(raw_end)
                            .ok()
                            .map(|dt| dt.timestamp())
                            == Some(slice.end.timestamp())
                    };

                    name_eq
                        && type_eq
                        && hand_eq
                        && commute_eq
                        && start_eq
                        && tile_start_eq
                        && tile_end_eq
                        && opt_i64_eq("moving_seconds", workout.moving_seconds)
                        && opt_f64_eq("distance_m", workout.distance_meters)
                        && opt_f64_eq("elevation_gain_m", workout.elevation_gain_meters)
                        && opt_f64_eq("heart_rate_avg_bpm", workout.average_heart_rate)
                        && opt_f64_eq("heart_rate_max_bpm", workout.max_heart_rate)
                        && opt_f64_eq("power_avg_w", workout.average_watts)
                        && opt_f64_eq("power_weighted_w", workout.weighted_average_power)
                        && opt_f64_eq("calories_kcal", workout.calories)
                };

                if json_matches {
                    counts.unchanged_tiles += 1;
                    tile_actions.push(TileAction::Unchanged {
                        day: stored.day.clone(),
                        segment: stored.key.clone(),
                        activity_id: workout.activity_id,
                        slice: slice.clone(),
                    });
                } else {
                    counts.updated_tiles += 1;
                    had_updated_in_workout = true;
                    tile_actions.push(TileAction::Updated {
                        day: stored.day.clone(),
                        segment: stored.key.clone(),
                        stored_import_id: stored.import_id.clone(),
                        stored_zone: stored.zone_str.clone(),
                        slice: slice.clone(),
                        workout: workout.clone(),
                    });
                }

                if !stored.has_marker {
                    let is_head = stream_record.as_ref().is_some_and(|r| {
                        r.get("last_day").and_then(Value::as_str) == Some(&stored.day)
                            && r.get("last_segment").and_then(Value::as_str) == Some(&stored.key)
                    });
                    let named_as_prev =
                        marker_prev_set.contains(&(stored.day.clone(), stored.key.clone()));
                    if is_head || !named_as_prev {
                        counts.republished_tiles += 1;
                        publish_list.push(PlannedTilePublication {
                            day: stored.day.clone(),
                            segment: stored.key.clone(),
                        });
                    } else {
                        counts.marker_missing_in_chain_tiles += 1;
                    }
                }
            } else {
                let candidates = match segment_key_candidates(&slice.natural_key) {
                    Ok(c) => c,
                    Err(e) => {
                        return Err(StravaError::ProbeUnreadable {
                            day: slice.natural_day.clone(),
                            key: slice.natural_key.clone(),
                            detail: e.to_string(),
                        });
                    }
                };

                let mut placed_candidate = None;
                for cand in candidates {
                    let cand_dir = journal
                        .join("chronicle")
                        .join(&slice.natural_day)
                        .join("import.strava")
                        .join(&cand);

                    if !checked_owner_deleted_probe
                        .contains(&(slice.natural_day.clone(), cand.clone()))
                    {
                        checked_owner_deleted_probe
                            .insert((slice.natural_day.clone(), cand.clone()));
                        if let Some(f) = fault
                            && f.check(FaultKind::OwnerDeleted)
                        {
                            return Err(StravaError::ProbeUnreadable {
                                day: slice.natural_day.clone(),
                                key: cand.clone(),
                                detail: "injected OwnerDeleted fault".to_owned(),
                            });
                        }
                    }

                    match owner_deleted(&cand_dir) {
                        Ok(true) => {
                            match released_with_import(&cand_dir) {
                                Ok(true) => continue,
                                Ok(false) => {}
                                Err(detail) => {
                                    return Err(StravaError::ProbeUnreadable {
                                        day: slice.natural_day.clone(),
                                        key: cand,
                                        detail,
                                    });
                                }
                            }
                            placed_candidate = Some(TileAction::StayedDeleted {
                                day: slice.natural_day.clone(),
                                segment: cand,
                                activity_id: workout.activity_id,
                                slice: slice.clone(),
                            });
                            break;
                        }
                        Ok(false) => {}
                        Err(e) => {
                            return Err(StravaError::ProbeUnreadable {
                                day: slice.natural_day.clone(),
                                key: cand,
                                detail: e.to_string(),
                            });
                        }
                    }

                    let meta_res = fs::symlink_metadata(&cand_dir);
                    let is_free = match meta_res {
                        Ok(meta) => {
                            if meta.file_type().is_symlink() || !meta.is_dir() {
                                false
                            } else {
                                let mut read_entries = fs::read_dir(&cand_dir).map_err(|e| {
                                    StravaError::ProbeUnreadable {
                                        day: slice.natural_day.clone(),
                                        key: cand.clone(),
                                        detail: e.to_string(),
                                    }
                                })?;
                                match read_entries.next() {
                                    None => true,
                                    Some(Ok(_)) => false,
                                    Some(Err(e)) => {
                                        return Err(StravaError::ProbeUnreadable {
                                            day: slice.natural_day.clone(),
                                            key: cand.clone(),
                                            detail: e.to_string(),
                                        });
                                    }
                                }
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
                        Err(e) => {
                            return Err(StravaError::ProbeUnreadable {
                                day: slice.natural_day.clone(),
                                key: cand.clone(),
                                detail: e.to_string(),
                            });
                        }
                    };

                    if is_free && !claimed_keys.contains(&(slice.natural_day.clone(), cand.clone()))
                    {
                        claimed_keys.insert((slice.natural_day.clone(), cand.clone()));
                        placed_candidate = Some(TileAction::Created {
                            day: slice.natural_day.clone(),
                            segment: cand,
                            slice: slice.clone(),
                            workout: workout.clone(),
                        });
                        break;
                    }
                }

                match placed_candidate {
                    Some(TileAction::Created {
                        day,
                        segment,
                        slice,
                        workout,
                    }) => {
                        counts.created_tiles += 1;
                        counts.tiles_to_create += 1;
                        had_created_in_workout = true;
                        publish_list.push(PlannedTilePublication {
                            day: day.clone(),
                            segment: segment.clone(),
                        });
                        tile_actions.push(TileAction::Created {
                            day,
                            segment,
                            slice,
                            workout,
                        });
                    }
                    Some(TileAction::StayedDeleted {
                        day,
                        segment,
                        activity_id,
                        slice,
                    }) => {
                        counts.stayed_deleted_tiles += 1;
                        had_stayed_deleted_in_workout = true;
                        all_indices_live = false;
                        tile_actions.push(TileAction::StayedDeleted {
                            day,
                            segment,
                            activity_id,
                            slice,
                        });
                    }
                    _ => {
                        counts.no_free_key_tiles += 1;
                        all_indices_live = false;
                        tile_actions.push(TileAction::NoFreeKey {
                            natural_day: slice.natural_day.clone(),
                            natural_key: slice.natural_key.clone(),
                            activity_id: workout.activity_id,
                            slice,
                        });
                    }
                }
            }
        }

        if all_indices_live {
            counts.complete_workouts += 1;
            if !had_canonical_before {
                counts.new_workouts += 1;
                dispositions.insert(workout.activity_id, WorkoutDisposition::New);
            } else if had_created_in_workout {
                counts.present_completed += 1;
                dispositions.insert(workout.activity_id, WorkoutDisposition::PresentCompleted);
            } else if had_updated_in_workout {
                counts.present_updated += 1;
                dispositions.insert(workout.activity_id, WorkoutDisposition::PresentUpdated);
            } else {
                counts.present_unchanged += 1;
                dispositions.insert(workout.activity_id, WorkoutDisposition::PresentUnchanged);
            }
        } else {
            counts.incomplete_workouts += 1;
            if had_canonical_before {
                if had_updated_in_workout {
                    counts.present_updated += 1;
                    dispositions.insert(workout.activity_id, WorkoutDisposition::PresentUpdated);
                } else {
                    counts.present_unchanged += 1;
                    dispositions.insert(workout.activity_id, WorkoutDisposition::PresentUnchanged);
                }
            } else {
                if !had_created_in_workout && had_stayed_deleted_in_workout {
                    counts.deleted += 1;
                }
                dispositions.insert(workout.activity_id, WorkoutDisposition::New);
            }
        }
    }

    Ok(Placement {
        zone: default_zone,
        tiles: tile_actions,
        publish: publish_list,
        counts,
        first_day,
        last_day,
        workouts: sorted_workouts,
        dispositions,
    })
}

pub fn apply(
    placement: &Placement,
    creating_import_id: &str,
) -> (RenderedImport, Vec<PlannedTilePublication>) {
    let mut files = Vec::new();
    let mut written_publish = Vec::new();

    for action in &placement.tiles {
        match action {
            TileAction::Created {
                day,
                segment,
                slice,
                workout,
            } => {
                let json_str = format_workout_tile(
                    creating_import_id,
                    workout.activity_id,
                    placement.zone.name(),
                    slice.index,
                    workout.elapsed_seconds.max(1).div_ceil(300),
                    &slice.start,
                    &slice.end,
                    slice.seconds,
                    workout,
                );
                files.push(SegmentFile {
                    day: day.clone(),
                    segment: segment.clone(),
                    name: "workout.json",
                    contents: json_str,
                    units: 1,
                });
                written_publish.push(PlannedTilePublication {
                    day: day.clone(),
                    segment: segment.clone(),
                });
            }
            TileAction::Updated {
                day,
                segment,
                stored_import_id,
                stored_zone,
                slice,
                workout,
            } => {
                let json_str = format_workout_tile(
                    stored_import_id,
                    workout.activity_id,
                    stored_zone,
                    slice.index,
                    workout.elapsed_seconds.max(1).div_ceil(300),
                    &slice.start,
                    &slice.end,
                    slice.seconds,
                    workout,
                );
                files.push(SegmentFile {
                    day: day.clone(),
                    segment: segment.clone(),
                    name: "workout.json",
                    contents: json_str,
                    units: 1,
                });
            }
            TileAction::Unchanged { .. }
            | TileAction::StayedDeleted { .. }
            | TileAction::NoFreeKey { .. }
            | TileAction::DuplicateIdentity { .. } => {}
        }
    }

    for pub_tile in &placement.publish {
        if !written_publish
            .iter()
            .any(|p| p.day == pub_tile.day && p.segment == pub_tile.segment)
        {
            written_publish.push(pub_tile.clone());
        }
    }

    let summary = format_count_summary(&placement.counts);

    let rendered = RenderedImport {
        source: RegistrySource::Strava,
        files,
        items: Vec::new(),
        entries: placement.counts.new_workouts as u64,
        summary,
    };

    (rendered, written_publish)
}

pub fn reconcile_written(
    placement: &Placement,
    written_keys: &HashSet<(String, String)>,
    skipped_keys: &HashSet<(String, String)>,
    _write_stopped: bool,
) -> (PlacementCounts, Vec<PlannedTilePublication>) {
    let mut counts = PlacementCounts {
        duplicate_identity_tiles: placement.counts.duplicate_identity_tiles,
        republished_tiles: placement.counts.republished_tiles,
        marker_missing_in_chain_tiles: placement.counts.marker_missing_in_chain_tiles,
        deleted: 0,
        ..Default::default()
    };

    let mut workout_tile_states: BTreeMap<u64, Vec<(usize, &'static str)>> = BTreeMap::new();

    for action in &placement.tiles {
        match action {
            TileAction::Created {
                day,
                segment,
                slice,
                workout,
            } => {
                let key = (day.clone(), segment.clone());
                if skipped_keys.contains(&key) {
                    counts.stayed_deleted_tiles += 1;
                    workout_tile_states
                        .entry(workout.activity_id)
                        .or_default()
                        .push((slice.index, "stayed_deleted"));
                } else if written_keys.contains(&key) {
                    counts.created_tiles += 1;
                    counts.tiles_to_create += 1;
                    workout_tile_states
                        .entry(workout.activity_id)
                        .or_default()
                        .push((slice.index, "created"));
                } else {
                    workout_tile_states
                        .entry(workout.activity_id)
                        .or_default()
                        .push((slice.index, "unwritten"));
                }
            }
            TileAction::Updated {
                day,
                segment,
                slice,
                workout,
                ..
            } => {
                let key = (day.clone(), segment.clone());
                if skipped_keys.contains(&key) {
                    counts.stayed_deleted_tiles += 1;
                    workout_tile_states
                        .entry(workout.activity_id)
                        .or_default()
                        .push((slice.index, "stayed_deleted"));
                } else if written_keys.contains(&key) {
                    counts.updated_tiles += 1;
                    workout_tile_states
                        .entry(workout.activity_id)
                        .or_default()
                        .push((slice.index, "updated"));
                } else {
                    workout_tile_states
                        .entry(workout.activity_id)
                        .or_default()
                        .push((slice.index, "unwritten"));
                }
            }
            TileAction::Unchanged {
                activity_id, slice, ..
            } => {
                counts.unchanged_tiles += 1;
                workout_tile_states
                    .entry(*activity_id)
                    .or_default()
                    .push((slice.index, "unchanged"));
            }
            TileAction::StayedDeleted {
                activity_id, slice, ..
            } => {
                counts.stayed_deleted_tiles += 1;
                workout_tile_states
                    .entry(*activity_id)
                    .or_default()
                    .push((slice.index, "stayed_deleted"));
            }
            TileAction::NoFreeKey {
                activity_id, slice, ..
            } => {
                counts.no_free_key_tiles += 1;
                workout_tile_states
                    .entry(*activity_id)
                    .or_default()
                    .push((slice.index, "no_free_key"));
            }
            TileAction::DuplicateIdentity { .. } => {}
        }
    }

    for workout in &placement.workouts {
        if let Some(&disp) = placement.dispositions.get(&workout.activity_id) {
            match disp {
                WorkoutDisposition::PresentTimingChanged => {
                    counts.present_timing_changed += 1;
                    let states = workout_tile_states.get(&workout.activity_id);
                    let total_slices = workout.elapsed_seconds.max(1).div_ceil(300) as usize;
                    let is_complete = states.is_some_and(|s| s.len() == total_slices);
                    if is_complete {
                        counts.complete_workouts += 1;
                    } else {
                        counts.incomplete_workouts += 1;
                    }
                    continue;
                }
                WorkoutDisposition::ZoneConflict => {
                    counts.zone_conflict += 1;
                    let states = workout_tile_states.get(&workout.activity_id);
                    let total_slices = workout.elapsed_seconds.max(1).div_ceil(300) as usize;
                    let is_complete = states.is_some_and(|s| s.len() == total_slices);
                    if is_complete {
                        counts.complete_workouts += 1;
                    } else {
                        counts.incomplete_workouts += 1;
                    }
                    continue;
                }
                _ => {}
            }
        }

        let states = workout_tile_states.get(&workout.activity_id);
        let total_slices = workout.elapsed_seconds.max(1).div_ceil(300) as usize;

        let Some(states) = states else {
            counts.incomplete_workouts += 1;
            continue;
        };

        let had_canonical_before = states
            .iter()
            .any(|(_, st)| *st == "updated" || *st == "unchanged");
        let had_created = states.iter().any(|(_, st)| *st == "created");
        let had_updated = states.iter().any(|(_, st)| *st == "updated");
        let all_live = states.len() == total_slices
            && states
                .iter()
                .all(|(_, st)| *st == "created" || *st == "updated" || *st == "unchanged");

        if all_live {
            counts.complete_workouts += 1;
            if !had_canonical_before {
                counts.new_workouts += 1;
            } else if had_created {
                counts.present_completed += 1;
            } else if had_updated {
                counts.present_updated += 1;
            } else {
                counts.present_unchanged += 1;
            }
        } else {
            counts.incomplete_workouts += 1;
            if had_canonical_before {
                if had_updated {
                    counts.present_updated += 1;
                } else {
                    counts.present_unchanged += 1;
                }
            } else if !had_created
                && states.iter().any(|(_, st)| *st == "stayed_deleted")
                && !states.iter().any(|(_, st)| *st == "unwritten")
            {
                counts.deleted += 1;
            }
        }
    }

    let mut planned_publish = Vec::new();
    for pub_tile in &placement.publish {
        let key = (pub_tile.day.clone(), pub_tile.segment.clone());
        if skipped_keys.contains(&key) {
            continue;
        }
        let is_created = placement.tiles.iter().any(|t| match t {
            TileAction::Created { day, segment, .. } => {
                *day == pub_tile.day && *segment == pub_tile.segment
            }
            _ => false,
        });
        if is_created {
            if written_keys.contains(&key) {
                planned_publish.push(pub_tile.clone());
            }
        } else {
            // Republish key
            planned_publish.push(pub_tile.clone());
        }
    }

    (counts, planned_publish)
}

#[allow(clippy::too_many_arguments)]
fn format_workout_tile(
    import_id: &str,
    activity_id: u64,
    zone_name: &str,
    tile_index: usize,
    tile_count: u64,
    tile_start: &DateTime<Tz>,
    tile_end: &DateTime<Tz>,
    tile_seconds: u64,
    workout: &StravaWorkout,
) -> String {
    let mut map = serde_json::Map::new();
    map.insert("schema".to_owned(), json!(TILE_SCHEMA));
    map.insert("import_id".to_owned(), json!(import_id));
    map.insert("activity_id".to_owned(), json!(activity_id));
    map.insert("zone".to_owned(), json!(zone_name));

    let mut tile_map = serde_json::Map::new();
    tile_map.insert("index".to_owned(), json!(tile_index));
    tile_map.insert("count".to_owned(), json!(tile_count));
    tile_map.insert(
        "start".to_owned(),
        json!(tile_start.to_rfc3339_opts(SecondsFormat::Secs, false)),
    );
    tile_map.insert(
        "end".to_owned(),
        json!(tile_end.to_rfc3339_opts(SecondsFormat::Secs, false)),
    );
    tile_map.insert("seconds".to_owned(), json!(tile_seconds));
    map.insert("tile".to_owned(), Value::Object(tile_map));

    let mut workout_map = serde_json::Map::new();
    workout_map.insert("name".to_owned(), json!(workout.activity_name));
    workout_map.insert("type".to_owned(), json!(workout.activity_type));
    workout_map.insert(
        "start".to_owned(),
        json!(
            workout
                .start_in(tile_start.timezone())
                .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Secs, false))
                .unwrap_or_default()
        ),
    );
    workout_map.insert("elapsed_seconds".to_owned(), json!(workout.elapsed_seconds));
    workout_map.insert(
        "moving_seconds".to_owned(),
        match workout.moving_seconds {
            Some(s) => json!(s),
            None => Value::Null,
        },
    );
    workout_map.insert(
        "distance_m".to_owned(),
        match workout.distance_meters {
            Some(v) => json!(v),
            None => Value::Null,
        },
    );
    workout_map.insert(
        "elevation_gain_m".to_owned(),
        match workout.elevation_gain_meters {
            Some(v) => json!(v),
            None => Value::Null,
        },
    );
    workout_map.insert(
        "heart_rate_avg_bpm".to_owned(),
        match workout.average_heart_rate {
            Some(v) => json!(v),
            None => Value::Null,
        },
    );
    workout_map.insert(
        "heart_rate_max_bpm".to_owned(),
        match workout.max_heart_rate {
            Some(v) => json!(v),
            None => Value::Null,
        },
    );
    workout_map.insert(
        "power_avg_w".to_owned(),
        match workout.average_watts {
            Some(v) => json!(v),
            None => Value::Null,
        },
    );
    workout_map.insert(
        "power_weighted_w".to_owned(),
        match workout.weighted_average_power {
            Some(v) => json!(v),
            None => Value::Null,
        },
    );
    workout_map.insert(
        "calories_kcal".to_owned(),
        match workout.calories {
            Some(v) => json!(v),
            None => Value::Null,
        },
    );
    workout_map.insert(
        "commute".to_owned(),
        match workout.commute {
            Some(b) => json!(b),
            None => Value::Null,
        },
    );
    workout_map.insert("entered_by_hand".to_owned(), json!(workout.entered_by_hand));
    map.insert("workout".to_owned(), Value::Object(workout_map));

    let mut out = serde_json::to_string(&Value::Object(map)).unwrap();
    out.push('\n');
    out
}

pub fn format_count_summary(counts: &PlacementCounts) -> String {
    let present = counts.present_unchanged
        + counts.present_updated
        + counts.present_completed
        + counts.present_timing_changed
        + counts.zone_conflict;

    format!(
        "new={} tiles_to_create={} present={} stayed_deleted={} created={} updated={} unchanged={} no_free_key={} duplicate_identity={} republished={} marker_missing_in_chain={} present_unchanged={} present_updated={} present_completed={} present_timing_changed={} deleted={} zone_conflict={} complete={} incomplete={}",
        counts.new_workouts,
        counts.tiles_to_create,
        present,
        counts.stayed_deleted_tiles,
        counts.created_tiles,
        counts.updated_tiles,
        counts.unchanged_tiles,
        counts.no_free_key_tiles,
        counts.duplicate_identity_tiles,
        counts.republished_tiles,
        counts.marker_missing_in_chain_tiles,
        counts.present_unchanged,
        counts.present_updated,
        counts.present_completed,
        counts.present_timing_changed,
        counts.deleted,
        counts.zone_conflict,
        counts.complete_workouts,
        counts.incomplete_workouts,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use std::io::Write;
    use tempfile::TempDir;

    pub const HEADER_ENGLISH_103: &str = "Activity ID,Activity Date,Activity Name,Activity Type,Activity Description,Elapsed Time,Distance,Max Heart Rate,Relative Effort,Commute,Activity Private Note,Activity Gear,Filename,Athlete Weight,Bike Weight,Elapsed Time,Moving Time,Distance,Max Speed,Average Speed,Elevation Gain,Elevation Loss,Elevation Low,Elevation High,Max Grade,Average Grade,Average Positive Grade,Average Negative Grade,Max Cadence,Average Cadence,Max Heart Rate,Average Heart Rate,Max Watts,Average Watts,Calories,Max Temperature,Average Temperature,Relative Effort,Total Work,Number of Runs,Uphill Time,Downhill Time,Other Time,Perceived Exertion,Type,Start Time,Weighted Average Power,Power Count,Prefer Perceived Exertion,Perceived Relative Effort,Commute,Total Weight Lifted,From Upload,Grade Adjusted Distance,Weather Observation Time,Weather Condition,Weather Temperature,Apparent Temperature,Dewpoint,Humidity,Weather Pressure,Wind Speed,Wind Gust,Wind Bearing,Precipitation Intensity,Sunrise Time,Sunset Time,Moon Phase,Bike,Gear,Precipitation Probability,Precipitation Type,Cloud Cover,Weather Visibility,UV Index,Weather Ozone,Jump Count,Total Grit,Average Flow,Flagged,Average Elapsed Speed,Dirt Distance,Newly Explored Distance,Newly Explored Dirt Distance,Activity Count,Total Steps,Carbon Saved,Pool Length,Training Load,Intensity,Average Grade Adjusted Pace,Timer Time,Total Cycles,Recovery,With Pet,Competition,Long Run,For a Cause,With Kid,Downhill Distance,Total Sets,Total Reps,Media\n";

    pub const HEADER_GERMAN_92: &str = "Aktivitäts-ID,Aktivitätsdatum,Name der Aktivität,Aktivitätsart,Aktivitätsbeschreibung,Verstrichene Zeit,Distanz,Max. Herzfrequenz,Relative Leistung,Pendeln,Hinweis zur Privatsphäre für Aktivitäten,Aktivitätsausrüstung,Dateiname,Sportlergewicht,Fahrradgewicht,Verstrichene Zeit,Bewegungszeit,Distanz,Höchstgeschw.,Durchschnittliche Geschwindigkeit,Höhenzunahme,Höhenunterschied,Min. Höhe,Max. Höhe,Max. Steigung,Durchschnittliche Steigung,Durchschnittliche positive Steigung,Durchschnittliche negative Steigung,Max. Tritt-/Schrittfrequenz,Durchschnittliche Trittfrequenz,Max. Herzfrequenz,Durchschnittliche Herzfrequenz,Max. Watt,Durchschnittliche Watt,Kalorien,Max. Temperatur,Durchschnittliche Temperatur,Relative Leistung,Gesamtarbeit,Anzahl Läufe,Bergaufzeit,Bergabzeit,Andere Zeit,Gefühlte Anstrengung,Art,Startzeit,Gewichtete durchschnittliche Leistung,Leistungszahl,Gefühlte Anstrengung verwenden,Gefühlte relative Leistung,Pendeln,Insgesamt gestemmtes Gewicht,Von Upload,Auf Steigung angepasste Distanz,Wetterbeobachtungszeit,Wetterlage,Wetter: Temperatur,Scheinbare Temperatur,Taupunkt,Luftfeuchtigkeit,Wetter: Druck,Windgeschwindigkeit,Windböe,Windrichtung,Niederschlagsintensität,Sonnenaufgangszeit,Sonnenuntergangszeit,Mondphase,Fahrrad,Ausrüstung,Niederschlagswahrscheinlichkeit,Niederschlagsart,Wolkendecke,Wetter: Sichtbarkeit,UV-Index,Wetter: Ozon,Sprunganzahl,Schwierigkeit insgesamt,Durchschnittlicher Flow,Markiert,Durchschnittsgeschwindigkeit im Aufzeichnungszeitraum,Auf Schotter zurückgelegte Distanz,Neu getestete Distanz,Neu getestete Schotterdistanz,Aktivitätsanzahl,Schritte insgesamt,Eingesparte CO₂-Emissionen,Pool-Länge,Trainingsbelastung,Intensität,Durchschnittliches auf Steigung angepasstes Tempo,Medien\n";

    pub const HEADER_ENGLISH_86: &str = "Activity ID,Activity Date,Activity Name,Activity Type,Activity Description,Elapsed Time,Distance,Max Heart Rate,Relative Effort,Commute,Activity Private Note,Activity Gear,Filename,Athlete Weight,Bike Weight,Elapsed Time,Moving Time,Distance,Max Speed,Average Speed,Elevation Gain,Elevation Loss,Elevation Low,Elevation High,Max Grade,Average Grade,Average Positive Grade,Average Negative Grade,Max Cadence,Average Cadence,Max Heart Rate,Average Heart Rate,Max Watts,Average Watts,Calories,Max Temperature,Average Temperature,Relative Effort,Total Work,Number of Runs,Uphill Time,Downhill Time,Other Time,Perceived Exertion,Type,Start Time,Weighted Average Power,Power Count,Prefer Perceived Exertion,Perceived Relative Effort,Commute,Total Weight Lifted,From Upload,Grade Adjusted Distance,Weather Observation Time,Weather Condition,Weather Temperature,Apparent Temperature,Dewpoint,Humidity,Weather Pressure,Wind Speed,Wind Gust,Wind Bearing,Precipitation Intensity,Sunrise Time,Sunset Time,Moon Phase,Bike,Gear,Precipitation Probability,Precipitation Type,Cloud Cover,Weather Visibility,UV Index,Weather Ozone,Jump Count,Total Grit,Average Flow,Flagged,Average Elapsed Speed,Dirt Distance,Newly Explored Distance,Newly Explored Dirt Distance,Activity Count,Media\n";

    pub fn build_csv(header: &str, rows: &[&[(&str, usize, &str)]]) -> String {
        let cols: Vec<&str> = header.trim_end_matches('\n').split(',').collect();
        let mut out = header.to_owned();

        for row in rows {
            let mut col_occurrences: BTreeMap<&str, usize> = BTreeMap::new();
            let mut line_fields = Vec::with_capacity(cols.len());

            for col in &cols {
                let occ = col_occurrences.entry(col).or_insert(0);
                *occ += 1;
                let cur_occ = *occ;

                let val = row
                    .iter()
                    .find(|(name, o, _)| name == col && *o == cur_occ)
                    .map(|(_, _, v)| *v)
                    .unwrap_or("");
                line_fields.push(val);
            }

            out.push_str(&line_fields.join(","));
            out.push('\n');
        }

        out
    }

    #[test]
    fn detector_table_cases() {
        let temp = TempDir::new().unwrap();

        let zip_root = temp.path().join("root.zip");
        {
            let file = File::create(&zip_root).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("activities.csv", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"arbitrary").unwrap();
            zip.finish().unwrap();
        }

        let zip_nested = temp.path().join("nested.zip");
        {
            let file = File::create(&zip_nested).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file(
                "export_1/activities.csv",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"arbitrary").unwrap();
            zip.finish().unwrap();
        }

        let extensionless_zip = temp.path().join("export_raw");
        {
            let file = File::create(&extensionless_zip).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("activities.csv", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"arbitrary").unwrap();
            zip.finish().unwrap();
        }

        let upper_csv = temp.path().join("ACTIVITIES.CSV");
        fs::write(&upper_csv, b"arbitrary").unwrap();

        let regular_csv = temp.path().join("other.csv");
        fs::write(&regular_csv, b"Activity ID,Date\n1,2\n").unwrap();

        let non_strava_csv = temp.path().join("non_strava.csv");
        fs::write(&non_strava_csv, b"Date,Steps\n1,2\n").unwrap();

        let dir_root = temp.path().join("dir_root");
        fs::create_dir(&dir_root).unwrap();
        fs::write(dir_root.join("activities.csv"), b"Activity ID,Date\n1,2\n").unwrap();

        let dir_nested = temp.path().join("dir_nested");
        let dir_nested_child = dir_nested.join("child");
        fs::create_dir_all(&dir_nested_child).unwrap();
        fs::write(
            dir_nested_child.join("activities.csv"),
            b"Activity ID,Date\n1,2\n",
        )
        .unwrap();

        let dir_empty = temp.path().join("dir_empty");
        fs::create_dir(&dir_empty).unwrap();

        assert!(looks_like_strava_download(&zip_root, None));
        assert!(looks_like_strava_download(&zip_nested, None));
        assert!(looks_like_strava_download(&extensionless_zip, None));
        assert!(looks_like_strava_download(&upper_csv, None));
        assert!(looks_like_strava_download(&regular_csv, None));
        assert!(!looks_like_strava_download(&non_strava_csv, None));
        assert!(looks_like_strava_download(&dir_root, None));
        assert!(looks_like_strava_download(&dir_nested, None));
        assert!(!looks_like_strava_download(&dir_empty, None));
    }

    #[test]
    fn test_reader_bare_csv_english_and_german() {
        let temp = TempDir::new().unwrap();
        let en_csv = temp.path().join("en.csv");
        let de_csv = temp.path().join("de.csv");

        let en_row: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "12345"),
            ("Activity Date", 1, "\"Sep 20, 2026, 2:12:12 PM\""),
            ("Activity Name", 1, "Afternoon Run"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 1, "9999"),
            ("Elapsed Time", 2, "300"),
            ("Moving Time", 1, "300"),
            ("Distance", 1, "99.9"),
            ("Distance", 2, "5000"),
            ("Elevation Gain", 1, "100"),
            ("Average Heart Rate", 1, "140"),
            ("Max Heart Rate", 1, "999"),
            ("Max Heart Rate", 2, "160"),
            ("Average Watts", 1, "200"),
            ("Weighted Average Power", 1, "210"),
            ("Calories", 1, "350"),
            ("Commute", 1, "false"),
            ("Commute", 2, "true"),
            ("Filename", 1, "activities/12345.gpx"),
        ];

        let de_row: &[(&str, usize, &str)] = &[
            ("Aktivitäts-ID", 1, "12345"),
            ("Aktivitätsdatum", 1, "\"20.09.2026, 14:12:12\""),
            ("Name der Aktivität", 1, "Afternoon Run"),
            ("Aktivitätsart", 1, "Run"),
            ("Verstrichene Zeit", 1, "9999"),
            ("Verstrichene Zeit", 2, "300"),
            ("Bewegungszeit", 1, "300"),
            ("Distanz", 1, "\"10,20\""),
            ("Distanz", 2, "5000"),
            ("Höhenzunahme", 1, "100"),
            ("Durchschnittliche Herzfrequenz", 1, "140"),
            ("Max. Herzfrequenz", 1, "999"),
            ("Max. Herzfrequenz", 2, "160"),
            ("Durchschnittliche Watt", 1, "200"),
            ("Gewichtete durchschnittliche Leistung", 1, "210"),
            ("Kalorien", 1, "350"),
            ("Pendeln", 1, "false"),
            ("Pendeln", 2, "true"),
            ("Dateiname", 1, "activities/12345.gpx"),
        ];

        fs::write(&en_csv, build_csv(HEADER_ENGLISH_103, &[en_row]).as_bytes()).unwrap();
        let en86_csv = temp.path().join("en86.csv");
        fs::write(
            &en86_csv,
            build_csv(HEADER_ENGLISH_86, &[en_row]).as_bytes(),
        )
        .unwrap();
        fs::write(&de_csv, build_csv(HEADER_GERMAN_92, &[de_row]).as_bytes()).unwrap();

        let en_res = read_workouts(&en_csv, DEFAULT_LIST_CAP).unwrap();
        let en86_res = read_workouts(&en86_csv, DEFAULT_LIST_CAP).unwrap();
        let de_res = read_workouts(&de_csv, DEFAULT_LIST_CAP).unwrap();

        assert_eq!(en_res.language, Language::English);
        assert_eq!(en86_res.language, Language::English);
        assert_eq!(de_res.language, Language::German);
        assert_eq!(en_res.workouts.len(), 1);
        assert_eq!(en86_res.workouts.len(), 1);
        assert_eq!(de_res.workouts.len(), 1);

        let w_en = &en_res.workouts[0];
        let w_de = &de_res.workouts[0];
        assert_eq!(w_en.activity_id, 12345);
        assert_eq!(w_en.activity_name, "Afternoon Run");
        assert_eq!(w_en.elapsed_seconds, 300);
        assert_eq!(w_en.moving_seconds, Some(300));
        assert_eq!(w_en.distance_meters, Some(5000.0));
        assert_eq!(w_en.commute, Some(true));
        assert!(!w_en.entered_by_hand);

        assert_eq!(w_de.activity_id, 12345);
        assert_eq!(w_de.local_start, w_en.local_start);
        assert_eq!(w_de.distance_meters, Some(5000.0));
    }

    #[test]
    fn test_reader_zip_exact_activities_csv_entry() {
        let temp = TempDir::new().unwrap();
        let zip_path = temp.path().join("export.zip");
        {
            let file = File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);

            zip.start_file("activities.csv", zip::write::SimpleFileOptions::default())
                .unwrap();
            let en_row: &[(&str, usize, &str)] = &[
                ("Activity ID", 1, "123"),
                ("Activity Date", 1, "\"Sep 20, 2026, 2:12:12 PM\""),
                ("Activity Name", 1, "Afternoon Run"),
                ("Activity Type", 1, "Run"),
                ("Elapsed Time", 2, "300"),
                ("Moving Time", 1, "300"),
                ("Distance", 2, "5000"),
                ("Elevation Gain", 1, "100"),
                ("Average Heart Rate", 1, "140"),
                ("Max Heart Rate", 2, "160"),
                ("Average Watts", 1, "200"),
                ("Weighted Average Power", 1, "210"),
                ("Calories", 1, "350"),
                ("Commute", 2, "true"),
                ("Filename", 1, "activities/123.gpx"),
            ];
            zip.write_all(build_csv(HEADER_ENGLISH_103, &[en_row]).as_bytes())
                .unwrap();

            zip.start_file("messaging.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"{}").unwrap();
            zip.start_file("media/a.jpg", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"fake image bytes").unwrap();

            zip.finish().unwrap();
        }

        let res = read_workouts(&zip_path, DEFAULT_LIST_CAP).unwrap();
        assert_eq!(res.workouts.len(), 1);
        assert_eq!(res.workouts[0].activity_id, 123);
    }

    #[test]
    fn test_reader_zip_nested_or_backslash_is_list_missing() {
        let temp = TempDir::new().unwrap();

        let zip_nested = temp.path().join("nested.zip");
        {
            let file = File::create(&zip_nested).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file(
                "export_1/activities.csv",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(HEADER_ENGLISH_103.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        assert!(matches!(
            read_workouts(&zip_nested, DEFAULT_LIST_CAP),
            Err(StravaError::ListMissing)
        ));

        let zip_dot = temp.path().join("dot.zip");
        {
            let file = File::create(&zip_dot).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("./activities.csv", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(HEADER_ENGLISH_103.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        assert!(matches!(
            read_workouts(&zip_dot, DEFAULT_LIST_CAP),
            Err(StravaError::ListMissing)
        ));

        let zip_slash = temp.path().join("slash.zip");
        {
            let file = File::create(&zip_slash).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file(
                "export\\activities.csv",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(HEADER_ENGLISH_103.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        assert!(matches!(
            read_workouts(&zip_slash, DEFAULT_LIST_CAP),
            Err(StravaError::ListMissing)
        ));
    }

    #[test]
    fn test_reader_cap_enforcement_bare_and_zip() {
        let temp = TempDir::new().unwrap();

        let bare = temp.path().join("large.csv");
        fs::write(&bare, vec![b'a'; 2000]).unwrap();
        assert!(matches!(
            read_workouts(&bare, 1024),
            Err(StravaError::ListTooLarge { limit: 1024 })
        ));

        let zip_path = temp.path().join("large.zip");
        {
            let file = File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("activities.csv", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&vec![b'a'; 2000]).unwrap();
            zip.finish().unwrap();
        }
        assert!(matches!(
            read_workouts(&zip_path, 1024),
            Err(StravaError::ListTooLarge { limit: 1024 })
        ));
    }

    #[test]
    fn test_tiler_durations_and_zero_rounding() {
        let local_start = NaiveDate::from_ymd_opt(2026, 3, 5)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap();
        let w_3600 = StravaWorkout {
            activity_id: 1,
            local_start,
            activity_name: "Run 3600".to_owned(),
            activity_type: "Run".to_owned(),
            elapsed_seconds: 3600,
            moving_seconds: None,
            distance_meters: None,
            elevation_gain_meters: None,
            average_heart_rate: None,
            max_heart_rate: None,
            average_watts: None,
            weighted_average_power: None,
            calories: None,
            commute: None,
            entered_by_hand: false,
        };

        let slices_3600 = tile_workout(&w_3600, Tz::UTC);
        assert_eq!(slices_3600.len(), 12);
        assert_eq!(slices_3600[0].seconds, 300);
        assert_eq!(slices_3600[11].seconds, 300);
        assert_eq!(slices_3600[0].natural_key, "100000_300");

        let w_3650 = StravaWorkout {
            elapsed_seconds: 3650,
            ..w_3600.clone()
        };
        let slices_3650 = tile_workout(&w_3650, Tz::UTC);
        assert_eq!(slices_3650.len(), 13);
        assert_eq!(slices_3650[12].seconds, 50);
        assert_eq!(slices_3650[12].natural_key, "110000_50");

        let w_0 = StravaWorkout {
            elapsed_seconds: 0,
            ..w_3600.clone()
        };
        let slices_0 = tile_workout(&w_0, Tz::UTC);
        assert_eq!(slices_0.len(), 1);
        assert_eq!(slices_0[0].seconds, 1);
        assert_eq!(slices_0[0].natural_key, "100000_1");

        let midnight_start = NaiveDate::from_ymd_opt(2026, 3, 5)
            .unwrap()
            .and_hms_opt(23, 58, 0)
            .unwrap();
        let w_midnight = StravaWorkout {
            local_start: midnight_start,
            elapsed_seconds: 600,
            ..w_3600.clone()
        };
        let slices_mid = tile_workout(&w_midnight, Tz::UTC);
        assert_eq!(slices_mid.len(), 2);
        assert_eq!(slices_mid[0].natural_day, "20260305");
        assert_eq!(slices_mid[0].natural_key, "235800_300");
        assert_eq!(slices_mid[1].natural_day, "20260306");
        assert_eq!(slices_mid[1].natural_key, "000300_300");
    }

    #[test]
    fn test_tiler_dst_fall_back_denver_oracle() {
        let start = NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_opt(1, 30, 0)
            .unwrap();
        let denver = chrono_tz::America::Denver;
        let w = StravaWorkout {
            activity_id: 999,
            local_start: start,
            activity_name: "Fall Back".to_owned(),
            activity_type: "Run".to_owned(),
            elapsed_seconds: 5400,
            moving_seconds: None,
            distance_meters: None,
            elevation_gain_meters: None,
            average_heart_rate: None,
            max_heart_rate: None,
            average_watts: None,
            weighted_average_power: None,
            calories: None,
            commute: None,
            entered_by_hand: false,
        };

        let temp = TempDir::new().unwrap();
        let journal = temp.path();

        let placement = place(journal, &[w], Some(denver), None).unwrap();
        assert_eq!(placement.counts.created_tiles, 18);
        assert_eq!(placement.counts.new_workouts, 1);

        let created_keys: Vec<(String, String)> = placement
            .tiles
            .iter()
            .filter_map(|t| match t {
                TileAction::Created { day, segment, .. } => Some((day.clone(), segment.clone())),
                _ => None,
            })
            .collect();

        assert_eq!(created_keys.len(), 18);
        let expected_segments = [
            "013000_300",
            "013500_300",
            "014000_300",
            "014500_300",
            "015000_300",
            "015500_300",
            "010000_300",
            "010500_300",
            "011000_300",
            "011500_300",
            "012000_300",
            "012500_300",
            "013000_301",
            "013500_301",
            "014000_301",
            "014500_301",
            "015000_301",
            "015500_301",
        ];
        for (i, expected_seg) in expected_segments.iter().enumerate() {
            assert_eq!(
                created_keys[i],
                ("20261101".to_owned(), (*expected_seg).to_owned())
            );
        }
    }

    #[test]
    fn test_zone_record_read_create_and_rejection() {
        let temp = TempDir::new().unwrap();
        let journal = temp.path();

        assert_eq!(read_zone_record(journal).unwrap(), None);

        create_zone_record(journal, chrono_tz::Asia::Tokyo).unwrap();
        assert_eq!(
            read_zone_record(journal).unwrap(),
            Some(chrono_tz::Asia::Tokyo)
        );

        let zone_file = journal.join("imports/.strava-zone.json");
        fs::write(&zone_file, b"{\"zone\":\"Mars/Olympus\"}\n").unwrap();
        assert!(matches!(
            read_zone_record(journal),
            Err(StravaError::ZoneUnrecognised(_))
        ));

        fs::write(&zone_file, b"not valid json").unwrap();
        assert!(matches!(
            read_zone_record(journal),
            Err(StravaError::ZoneUnrecognised(_))
        ));
    }

    #[test]
    fn test_strava_error_codes_exhaustive() {
        assert_eq!(StravaError::ListMissing.code(), "strava_list_missing");
        assert_eq!(
            StravaError::ZipUnreadable("test".into()).code(),
            "strava_zip_unreadable"
        );
        assert_eq!(
            StravaError::ListTooLarge { limit: 1 }.code(),
            "strava_list_too_large"
        );
        assert_eq!(
            StravaError::LanguageUnsupported.code(),
            "strava_language_unsupported"
        );
        assert_eq!(
            StravaError::LayoutUnrecognised {
                skips: SkipCounts::default()
            }
            .code(),
            "strava_layout_unrecognised"
        );
        assert_eq!(
            StravaError::NoWorkouts {
                skips: SkipCounts::default()
            }
            .code(),
            "strava_no_workouts"
        );
        assert_eq!(
            StravaError::ZoneUnrecognised("test".into()).code(),
            "strava_zone_unrecognised"
        );
        assert_eq!(
            StravaError::LockUnavailable.code(),
            "strava_lock_unavailable"
        );
        assert_eq!(
            StravaError::TileUnreadable {
                day: "d".into(),
                key: "k".into(),
                detail: "".into()
            }
            .code(),
            "strava_tile_unreadable"
        );
        assert_eq!(
            StravaError::DayUnreadable {
                day: "d".into(),
                detail: "".into()
            }
            .code(),
            "strava_day_unreadable"
        );
        assert_eq!(
            StravaError::ProbeUnreadable {
                day: "d".into(),
                key: "k".into(),
                detail: "".into()
            }
            .code(),
            "strava_probe_unreadable"
        );
        assert_eq!(
            StravaError::MarkerUnreadable {
                day: "d".into(),
                key: "k".into(),
                detail: "".into()
            }
            .code(),
            "strava_marker_unreadable"
        );
        assert_eq!(
            StravaError::MarkerUnparseable {
                day: "d".into(),
                key: "k".into()
            }
            .code(),
            "strava_marker_unparseable"
        );
        assert_eq!(
            StravaError::StreamRecordUnreadable("".into()).code(),
            "strava_stream_record_unreadable"
        );
    }

    #[test]
    fn test_format_workout_tile_timestamp_has_no_fractional_seconds() {
        let local_start = NaiveDate::from_ymd_opt(2026, 3, 5)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap();
        let w = StravaWorkout {
            activity_id: 42,
            local_start,
            activity_name: "Test Run".to_owned(),
            activity_type: "Run".to_owned(),
            elapsed_seconds: 300,
            moving_seconds: Some(300),
            distance_meters: Some(5000.0),
            elevation_gain_meters: None,
            average_heart_rate: None,
            max_heart_rate: None,
            average_watts: None,
            weighted_average_power: None,
            calories: None,
            commute: None,
            entered_by_hand: false,
        };
        let start = Tz::UTC.from_utc_datetime(&local_start);
        let end = start + Duration::seconds(300);
        let json_str =
            format_workout_tile("20260305_120000", 42, "UTC", 1, 1, &start, &end, 300, &w);
        let v: Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["tile"]["start"], "2026-03-05T10:00:00+00:00");
        assert_eq!(v["tile"]["end"], "2026-03-05T10:05:00+00:00");
        assert_eq!(v["workout"]["start"], "2026-03-05T10:00:00+00:00");
    }

    #[test]
    fn test_reader_english_german_equivalence_and_nullable_fields() {
        let temp = TempDir::new().unwrap();
        let en_csv = temp.path().join("en.csv");
        let de_csv = temp.path().join("de.csv");

        // Workout 1: all nullable set
        let en_row1: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "1001"),
            ("Activity Date", 1, "\"Sep 20, 2026, 2:12:12 PM\""),
            ("Activity Name", 1, "Full Run"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 1, "99999"), // 1st occurrence: unused
            ("Elapsed Time", 2, "300"),   // 2nd occurrence: used
            ("Moving Time", 1, "280"),
            ("Distance", 1, "99999"),  // 1st occurrence: unused
            ("Distance", 2, "5000.5"), // 2nd occurrence: used
            ("Elevation Gain", 1, "120.5"),
            ("Average Heart Rate", 1, "145.2"),
            ("Max Heart Rate", 1, "999"), // 1st occurrence: unused
            ("Max Heart Rate", 2, "172.8"),
            ("Average Watts", 1, "210.4"),
            ("Weighted Average Power", 1, "225.1"),
            ("Calories", 1, "340.9"),
            ("Commute", 1, "false"),    // 1st occurrence: unused
            ("Commute", 2, "true"),     // 2nd occurrence: used
            ("Filename", 1, "#error#"), // non-empty -> entered_by_hand false
        ];
        let de_row1: &[(&str, usize, &str)] = &[
            ("Aktivitäts-ID", 1, "1001"),
            ("Aktivitätsdatum", 1, "\"20.09.2026, 14:12:12\""),
            ("Name der Aktivität", 1, "Full Run"),
            ("Aktivitätsart", 1, "Run"),
            ("Verstrichene Zeit", 1, "99999"),
            ("Verstrichene Zeit", 2, "300"),
            ("Bewegungszeit", 1, "280"),
            ("Distanz", 1, "\"10,20\""),
            ("Distanz", 2, "5000.5"),
            ("Höhenzunahme", 1, "120.5"),
            ("Durchschnittliche Herzfrequenz", 1, "145.2"),
            ("Max. Herzfrequenz", 1, "999"),
            ("Max. Herzfrequenz", 2, "172.8"),
            ("Durchschnittliche Watt", 1, "210.4"),
            ("Gewichtete durchschnittliche Leistung", 1, "225.1"),
            ("Kalorien", 1, "340.9"),
            ("Pendeln", 1, "false"),
            ("Pendeln", 2, "true"),
            ("Dateiname", 1, "#error#"),
        ];

        // Workout 2: all nullable null / None, empty Filename -> entered_by_hand true
        let en_row2: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "1002"),
            ("Activity Date", 1, "\"22 May 2024, 08:19:26\""),
            ("Activity Name", 1, "Empty Run"),
            ("Activity Type", 1, "Walk"),
            ("Elapsed Time", 2, "600"),
            ("Moving Time", 1, ""),
            ("Distance", 2, ""),
            ("Elevation Gain", 1, ""),
            ("Average Heart Rate", 1, ""),
            ("Max Heart Rate", 2, ""),
            ("Average Watts", 1, ""),
            ("Weighted Average Power", 1, ""),
            ("Calories", 1, ""),
            ("Commute", 2, ""),
            ("Filename", 1, ""), // empty -> entered_by_hand true
        ];
        let de_row2: &[(&str, usize, &str)] = &[
            ("Aktivitäts-ID", 1, "1002"),
            ("Aktivitätsdatum", 1, "\"22.05.2024, 08:19:26\""),
            ("Name der Aktivität", 1, "Empty Run"),
            ("Aktivitätsart", 1, "Walk"),
            ("Verstrichene Zeit", 2, "600"),
            ("Bewegungszeit", 1, ""),
            ("Distanz", 2, ""),
            ("Höhenzunahme", 1, ""),
            ("Durchschnittliche Herzfrequenz", 1, ""),
            ("Max. Herzfrequenz", 2, ""),
            ("Durchschnittliche Watt", 1, ""),
            ("Gewichtete durchschnittliche Leistung", 1, ""),
            ("Kalorien", 1, ""),
            ("Pendeln", 2, ""),
            ("Dateiname", 1, ""),
        ];

        fs::write(
            &en_csv,
            build_csv(HEADER_ENGLISH_103, &[en_row1, en_row2]).as_bytes(),
        )
        .unwrap();
        fs::write(
            &de_csv,
            build_csv(HEADER_GERMAN_92, &[de_row1, de_row2]).as_bytes(),
        )
        .unwrap();

        let en_res = read_workouts(&en_csv, DEFAULT_LIST_CAP).unwrap();
        let de_res = read_workouts(&de_csv, DEFAULT_LIST_CAP).unwrap();

        assert_eq!(en_res.workouts.len(), 2);
        assert_eq!(de_res.workouts.len(), 2);

        let w1_en = &en_res.workouts[0];
        let w1_de = &de_res.workouts[0];
        assert_eq!(w1_en.activity_id, 1001);
        assert_eq!(w1_en.activity_name, "Full Run");
        assert_eq!(w1_en.elapsed_seconds, 300);
        assert_eq!(w1_en.moving_seconds, Some(280));
        assert_eq!(w1_en.distance_meters, Some(5000.5));
        assert_eq!(w1_en.elevation_gain_meters, Some(120.5));
        assert_eq!(w1_en.average_heart_rate, Some(145.2));
        assert_eq!(w1_en.max_heart_rate, Some(172.8));
        assert_eq!(w1_en.average_watts, Some(210.4));
        assert_eq!(w1_en.weighted_average_power, Some(225.1));
        assert_eq!(w1_en.calories, Some(340.9));
        assert_eq!(w1_en.commute, Some(true));
        assert!(!w1_en.entered_by_hand);
        assert_eq!(w1_en, w1_de);

        let w2_en = &en_res.workouts[1];
        let w2_de = &de_res.workouts[1];
        assert_eq!(w2_en.activity_id, 1002);
        assert_eq!(w2_en.elapsed_seconds, 600);
        assert_eq!(w2_en.moving_seconds, None);
        assert_eq!(w2_en.distance_meters, None);
        assert_eq!(w2_en.elevation_gain_meters, None);
        assert_eq!(w2_en.average_heart_rate, None);
        assert_eq!(w2_en.max_heart_rate, None);
        assert_eq!(w2_en.average_watts, None);
        assert_eq!(w2_en.weighted_average_power, None);
        assert_eq!(w2_en.calories, None);
        assert_eq!(w2_en.commute, None);
        assert!(w2_en.entered_by_hand);
        assert_eq!(w2_en, w2_de);
    }

    #[test]
    fn test_reader_dropped_kept_column_or_single_distance_is_layout_unrecognised() {
        let temp = TempDir::new().unwrap();

        // Dropping one kept column from HEADER_ENGLISH_86 (e.g. drop "Filename")
        let cols_86: Vec<&str> = HEADER_ENGLISH_86.split(',').collect();
        let dropped_cols: Vec<&str> = cols_86.into_iter().filter(|c| *c != "Filename").collect();
        let bad_header = dropped_cols.join(",");

        let p1 = temp.path().join("bad86.csv");
        fs::write(&p1, format!("{bad_header}\n")).unwrap();
        assert!(matches!(
            read_workouts(&p1, DEFAULT_LIST_CAP),
            Err(StravaError::LayoutUnrecognised { .. })
        ));

        // Header with Distance only once (English 103 normally has Distance twice)
        let cols_103: Vec<&str> = HEADER_ENGLISH_103.split(',').collect();
        let mut first_dist_dropped = false;
        let mut single_dist_cols = Vec::new();
        for c in cols_103 {
            if c == "Distance" && !first_dist_dropped {
                first_dist_dropped = true;
            } else {
                single_dist_cols.push(c);
            }
        }
        let single_dist_header = single_dist_cols.join(",");
        let p2 = temp.path().join("single_dist.csv");
        fs::write(&p2, format!("{single_dist_header}\n")).unwrap();
        assert!(matches!(
            read_workouts(&p2, DEFAULT_LIST_CAP),
            Err(StravaError::LayoutUnrecognised { .. })
        ));
    }

    #[test]
    fn test_reader_decoded_rows_no_parseable_date_is_layout_unrecognised() {
        let temp = TempDir::new().unwrap();
        let p = temp.path().join("unparseable_dates.csv");
        let row: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "bad_id"),
            ("Activity Date", 1, "not-a-valid-date-format"),
            ("Activity Name", 1, "Run"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "300"),
        ];
        fs::write(&p, build_csv(HEADER_ENGLISH_103, &[row]).as_bytes()).unwrap();

        assert!(matches!(
            read_workouts(&p, DEFAULT_LIST_CAP),
            Err(StravaError::LayoutUnrecognised { .. })
        ));
    }

    #[test]
    fn test_reader_header_only_and_all_ragged_is_no_workouts() {
        let temp = TempDir::new().unwrap();

        // Header only
        let p_hdr = temp.path().join("header_only.csv");
        fs::write(&p_hdr, format!("{HEADER_ENGLISH_103}\n").as_bytes()).unwrap();
        assert!(matches!(
            read_workouts(&p_hdr, DEFAULT_LIST_CAP),
            Err(StravaError::NoWorkouts { .. })
        ));

        // All ragged (e.g. unterminated quoted string without any valid row)
        let p_ragged = temp.path().join("all_ragged.csv");
        fs::write(
            &p_ragged,
            format!("{HEADER_ENGLISH_103}\n\"unterminated\n\"").as_bytes(),
        )
        .unwrap();
        assert!(matches!(
            read_workouts(&p_ragged, DEFAULT_LIST_CAP),
            Err(StravaError::NoWorkouts { .. })
        ));
    }

    #[test]
    fn test_reader_ragged_record_recovery() {
        let temp = TempDir::new().unwrap();
        let p = temp.path().join("ragged_recovery.csv");

        let row1: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "101"),
            ("Activity Date", 1, "\"Sep 20, 2026, 2:12:12 PM\""),
            ("Activity Name", 1, "Run 1"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "300"),
        ];
        let row3: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "103"),
            ("Activity Date", 1, "\"Sep 20, 2026, 3:12:12 PM\""),
            ("Activity Name", 1, "Run 3"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "300"),
        ];

        let csv1 = build_csv(HEADER_ENGLISH_103, &[row1]);
        let csv3 = build_csv(HEADER_ENGLISH_103, &[row3]);
        // Insert ragged unclosed quote row between row 1 and row 3
        let ragged_line = "102,bad\"bare\"quote,something,\n";
        let body = format!(
            "{}\n{}{}",
            csv1.trim_end(),
            ragged_line,
            csv3.lines().skip(1).collect::<Vec<_>>().join("\n")
        );

        fs::write(&p, body.as_bytes()).unwrap();
        let res = read_workouts(&p, DEFAULT_LIST_CAP).unwrap();
        assert_eq!(res.workouts.len(), 2);
        assert_eq!(res.workouts[0].activity_id, 101);
        assert_eq!(res.workouts[1].activity_id, 103);
        assert!(res.skips.ragged_or_undecodable >= 1);
    }

    #[test]
    fn test_reader_skip_counters_comprehensive() {
        let temp = TempDir::new().unwrap();
        let p = temp.path().join("skips.csv");

        let row_valid: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "1"),
            ("Activity Date", 1, "\"Sep 20, 2026, 2:12:12 PM\""),
            ("Activity Name", 1, "Valid"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "300"),
        ];
        let row_bad_id: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "not_a_num"),
            ("Activity Date", 1, "\"Sep 20, 2026, 2:12:12 PM\""),
            ("Activity Name", 1, "Bad ID"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "300"),
        ];
        let row_bad_date: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "2"),
            ("Activity Date", 1, "\"bad-date-string\""),
            ("Activity Name", 1, "Bad Date"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "300"),
        ];
        let row_bad_elapsed: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "3"),
            ("Activity Date", 1, "\"Sep 20, 2026, 2:12:12 PM\""),
            ("Activity Name", 1, "Bad Elapsed"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "not_a_num"),
        ];
        let row_dup_id: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "1"), // Duplicate of row_valid
            ("Activity Date", 1, "\"Sep 20, 2026, 2:12:12 PM\""),
            ("Activity Name", 1, "Duplicate ID"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "300"),
        ];
        let row_14_days_exact: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "4"),
            ("Activity Date", 1, "\"Sep 20, 2026, 2:12:12 PM\""),
            ("Activity Name", 1, "14 Days Exact"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "1209600"), // 14 * 86400 = 1209600
        ];
        let row_14_days_plus_1: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "5"),
            ("Activity Date", 1, "\"Sep 20, 2026, 2:12:12 PM\""),
            ("Activity Name", 1, "14 Days Plus 1s"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "1209601"), // 14 * 86400 + 1 = 1209601
        ];

        let content = build_csv(
            HEADER_ENGLISH_103,
            &[
                row_valid,
                row_bad_id,
                row_bad_date,
                row_bad_elapsed,
                row_dup_id,
                row_14_days_exact,
                row_14_days_plus_1,
            ],
        );
        fs::write(&p, content.as_bytes()).unwrap();

        let res = read_workouts(&p, DEFAULT_LIST_CAP).unwrap();
        assert_eq!(res.workouts.len(), 2); // row_valid and row_14_days_exact
        assert_eq!(res.skips.id_unparseable, 1);
        assert_eq!(res.skips.date_unparseable, 1);
        assert_eq!(res.skips.elapsed_unusable, 1);
        assert_eq!(res.skips.duplicate_activity_id, 1);
        assert_eq!(res.skips.elapsed_over_14_days, 1);

        // Clean fixture has all skips 0
        let clean_content = build_csv(HEADER_ENGLISH_103, &[row_valid]);
        let p_clean = temp.path().join("clean.csv");
        fs::write(&p_clean, clean_content.as_bytes()).unwrap();
        let clean_res = read_workouts(&p_clean, DEFAULT_LIST_CAP).unwrap();
        assert_eq!(clean_res.skips, SkipCounts::default());
    }

    #[test]
    fn test_reader_date_formats() {
        let temp = TempDir::new().unwrap();
        let p = temp.path().join("dates.csv");

        let row1: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "1"),
            ("Activity Date", 1, "\"Sep 20, 2026, 2:12:12 PM\""),
            ("Activity Name", 1, "Date 1"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "300"),
        ];
        let row2: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "2"),
            ("Activity Date", 1, "\"Sep 20, 2026, 2:12:12\u{202f}PM\""), // With U+202F narrow no-break space
            ("Activity Name", 1, "Date 2"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "300"),
        ];
        let row3: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "3"),
            ("Activity Date", 1, "\"22 May 2024, 08:19:26\""),
            ("Activity Name", 1, "Date 3"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "300"),
        ];

        let content = build_csv(HEADER_ENGLISH_103, &[row1, row2, row3]);
        fs::write(&p, content.as_bytes()).unwrap();
        let res = read_workouts(&p, DEFAULT_LIST_CAP).unwrap();
        assert_eq!(res.workouts.len(), 3);
        assert_eq!(
            res.workouts[0].local_start,
            NaiveDate::from_ymd_opt(2026, 9, 20)
                .unwrap()
                .and_hms_opt(14, 12, 12)
                .unwrap()
        );
        assert_eq!(res.workouts[0].local_start, res.workouts[1].local_start);
        assert_eq!(
            res.workouts[2].local_start,
            NaiveDate::from_ymd_opt(2024, 5, 22)
                .unwrap()
                .and_hms_opt(8, 19, 26)
                .unwrap()
        );

        let p_de = temp.path().join("dates_de.csv");
        let content_de = build_csv(
            HEADER_GERMAN_92,
            &[&[
                ("Aktivitäts-ID", 1, "4"),
                ("Aktivitätsdatum", 1, "\"02.04.2024, 14:05:15\""),
                ("Name der Aktivität", 1, "Date 4"),
                ("Aktivitätsart", 1, "Run"),
                ("Verstrichene Zeit", 2, "300"),
            ]],
        );
        fs::write(&p_de, content_de.as_bytes()).unwrap();
        let res_de = read_workouts(&p_de, DEFAULT_LIST_CAP).unwrap();
        assert_eq!(res_de.workouts.len(), 1);
        assert_eq!(
            res_de.workouts[0].local_start,
            NaiveDate::from_ymd_opt(2024, 4, 2)
                .unwrap()
                .and_hms_opt(14, 5, 15)
                .unwrap()
        );
    }

    #[test]
    fn test_denver_and_tokyo_date_keys() {
        let denver = chrono_tz::America::Denver;
        let tokyo = chrono_tz::Asia::Tokyo;

        // 2026-03-05 06:30:00 UTC
        // In Denver (UTC-7): 2026-03-04 23:30:00 -> day 20260304 key 233000
        let w_denver = StravaWorkout {
            activity_id: 1,
            local_start: NaiveDate::from_ymd_opt(2026, 3, 4)
                .unwrap()
                .and_hms_opt(23, 30, 0)
                .unwrap(),
            activity_name: "Denver Wall".to_owned(),
            activity_type: "Run".to_owned(),
            elapsed_seconds: 300,
            moving_seconds: None,
            distance_meters: None,
            elevation_gain_meters: None,
            average_heart_rate: None,
            max_heart_rate: None,
            average_watts: None,
            weighted_average_power: None,
            calories: None,
            commute: None,
            entered_by_hand: false,
        };
        let slices_denver = tile_workout(&w_denver, denver);
        assert_eq!(slices_denver.len(), 1);
        assert_eq!(slices_denver[0].natural_day, "20260304");
        assert_eq!(slices_denver[0].natural_key, "233000_300");

        // In Tokyo (UTC+9): 2026-03-05 15:30:00 -> day 20260305 key 153000
        let w_tokyo = StravaWorkout {
            activity_id: 2,
            local_start: NaiveDate::from_ymd_opt(2026, 3, 5)
                .unwrap()
                .and_hms_opt(15, 30, 0)
                .unwrap(),
            activity_name: "Tokyo Wall".to_owned(),
            activity_type: "Run".to_owned(),
            elapsed_seconds: 300,
            moving_seconds: None,
            distance_meters: None,
            elevation_gain_meters: None,
            average_heart_rate: None,
            max_heart_rate: None,
            average_watts: None,
            weighted_average_power: None,
            calories: None,
            commute: None,
            entered_by_hand: false,
        };
        let slices_tokyo = tile_workout(&w_tokyo, tokyo);
        assert_eq!(slices_tokyo.len(), 1);
        assert_eq!(slices_tokyo[0].natural_day, "20260305");
        assert_eq!(slices_tokyo[0].natural_key, "153000_300");
    }

    #[test]
    fn test_reader_zip_bad_crc_other_files_and_bare_equality() {
        let temp = TempDir::new().unwrap();
        let zip_path = temp.path().join("export.zip");
        let bare_path = temp.path().join("activities.csv");

        let row: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "777"),
            ("Activity Date", 1, "\"Sep 20, 2026, 2:12:12 PM\""),
            ("Activity Name", 1, "Zip Run"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "300"),
        ];
        let csv_bytes = build_csv(HEADER_ENGLISH_103, &[row]);
        fs::write(&bare_path, csv_bytes.as_bytes()).unwrap();

        {
            let file = File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);

            zip.start_file("activities.csv", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(csv_bytes.as_bytes()).unwrap();

            zip.start_file("messaging.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"{\"messages\":[]}").unwrap();

            zip.start_file("media/a.jpg", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"image data").unwrap();

            // Deliberately malformed contacts.csv (bad CRC / deflate error)
            zip.start_file(
                "contacts.csv",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
            zip.write_all(b"corrupt_data_for_crc_test").unwrap();

            zip.finish().unwrap();
        }

        let mut zip_bytes = fs::read(&zip_path).unwrap();
        let contacts_pos = zip_bytes
            .windows(12)
            .position(|w| w == b"contacts.csv")
            .unwrap();
        zip_bytes[contacts_pos + 15] ^= 0xFF;
        fs::write(&zip_path, &zip_bytes).unwrap();

        let zip_res = read_workouts(&zip_path, DEFAULT_LIST_CAP).unwrap();
        let bare_res = read_workouts(&bare_path, DEFAULT_LIST_CAP).unwrap();

        assert_eq!(zip_res.workouts, bare_res.workouts);
        assert_eq!(zip_res.language, bare_res.language);
        assert_eq!(zip_res.skips, bare_res.skips);

        // Truncated zip
        let trunc_zip = temp.path().join("trunc.zip");
        let zip_bytes = fs::read(&zip_path).unwrap();
        fs::write(&trunc_zip, &zip_bytes[..zip_bytes.len() / 2]).unwrap();
        assert!(matches!(
            read_workouts(&trunc_zip, DEFAULT_LIST_CAP),
            Err(StravaError::ZipUnreadable(_))
        ));

        // Directory input
        let dir_path = temp.path().join("some_dir");
        fs::create_dir(&dir_path).unwrap();
        assert!(matches!(
            read_workouts(&dir_path, DEFAULT_LIST_CAP),
            Err(StravaError::ListMissing)
        ));

        // Unsupported first header field
        let unsupported_csv = temp.path().join("unsupported.csv");
        fs::write(&unsupported_csv, b"Unknown Field,Date,Name\n1,2026,run\n").unwrap();
        assert!(matches!(
            read_workouts(&unsupported_csv, DEFAULT_LIST_CAP),
            Err(StravaError::LanguageUnsupported)
        ));
    }

    // A 500-workout run takes most of a minute unoptimized, so it runs with the
    // full suite rather than the routine one.
    #[cfg(feature = "full-tests")]
    #[test]
    fn test_scale_500_workouts_library() {
        let temp = TempDir::new().unwrap();
        let journal = temp.path();

        let base_date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let mut workouts = Vec::with_capacity(500);
        let mut total_expected_tiles = 0;

        for i in 0..500 {
            let elapsed_minutes = 10 + (i % 171); // Durations cycling 10 to 180 minutes
            let elapsed_seconds = (elapsed_minutes * 60) as u64;
            let tile_count = elapsed_seconds.max(1).div_ceil(300) as usize;
            total_expected_tiles += tile_count;

            let cur_date = base_date + Duration::days((i % 100) as i64);
            let local_start = cur_date.and_hms_opt(7, 0, 0).unwrap();

            workouts.push(StravaWorkout {
                activity_id: 1000 + i as u64,
                local_start,
                activity_name: format!("Workout {i}"),
                activity_type: "Run".to_owned(),
                elapsed_seconds,
                moving_seconds: Some(elapsed_seconds as i64),
                distance_meters: Some(5000.0),
                elevation_gain_meters: None,
                average_heart_rate: None,
                max_heart_rate: None,
                average_watts: None,
                weighted_average_power: None,
                calories: None,
                commute: None,
                entered_by_hand: false,
            });
        }

        let start_time = std::time::Instant::now();

        // First place
        let placement1 = place(journal, &workouts, Some(Tz::UTC), None).unwrap();
        assert_eq!(placement1.counts.created_tiles, total_expected_tiles);
        assert_eq!(placement1.counts.new_workouts, 500);

        // Apply and write
        let (rendered1, _) = apply(&placement1, "20260101_100000");
        let written1 = crate::save::write_rendered(journal, Some("20260101_100000"), &rendered1);
        assert_eq!(written1.created.len(), total_expected_tiles);

        // Second place: all unchanged
        let placement2 = place(journal, &workouts, Some(Tz::UTC), None).unwrap();
        assert_eq!(placement2.counts.created_tiles, 0);
        assert_eq!(placement2.counts.unchanged_tiles, total_expected_tiles);
        assert_eq!(placement2.counts.present_unchanged, 500);
        assert_eq!(placement2.counts.new_workouts, 0);

        let (rendered2, _) = apply(&placement2, "20260101_110000");
        assert_eq!(rendered2.files.len(), 0);

        let elapsed = start_time.elapsed();
        println!("test_scale_500_workouts_library wall time: {:?}", elapsed);
    }
}
