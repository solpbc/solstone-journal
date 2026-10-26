// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The location write owner: one derived document per day, for search.
//!
//! `chronicle/<day>/location.md` holds the day's area words as plain text —
//! *"readings near Golden, Colorado, United States, 14:00 to 14:09"* — so the
//! existing search finds the day, and the owner's agents read the words through
//! the existing fetch. It never holds a coordinate. Beside it,
//! `chronicle/<day>/health/location.json` records which state of the day's
//! segments the document was built from: the day's stream marker, which every
//! accepted upload, every segment removal and every archive merge advances.
//!
//! Rebuilding is deterministic. An unchanged day writes nothing; a day whose
//! segments changed is rebuilt from the segments that survive, and a day left
//! with no location loses both files and its rows in the index.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    HealthMarkerKind, HealthMarkerState, PathOrDay, day_dirs, iter_segments, read_health_marker,
};

use crate::accuracy::{Packs, StopWords};
use crate::area::{AreaName, AreaTier};
use crate::asset::AREA_TIER_SHA256;
use crate::day::{DayLocation, DeviceDirectory, Entry, LOCATION_FILE, LocalTime, Point, read_day};
use crate::places::{PlaceStore, StayRecord};

/// The day's search document, under `chronicle/<day>/`.
pub const DAY_DOCUMENT: &str = "location.md";
/// The build record, under `chronicle/<day>/health/`.
pub const BUILD_RECORD: &str = "location.json";
const RECORD_VERSION: u32 = 1;
/// Bumped whenever [`render_day_document`] changes what it writes, so every
/// day's document is rewritten once by the next pass.
const DOCUMENT_VERSION: u32 = 4;

/// What a day's document was built from, and what it counted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildRecord {
    pub version: u32,
    /// The day's stream marker and the next day's when the build read them.
    pub source: String,
    /// The area tier the names came from.
    pub area_tier: String,
    /// The location-accuracy packs the names came from, empty without them.
    #[serde(default)]
    pub packs: String,
    /// The version of the document's wording.
    pub document: u32,
    pub readings: u64,
    pub visits: u64,
    pub devices: u64,
    /// The day's stays and visits that are precise enough to make a place,
    /// for the places pass. This file is never indexed or shown to agents.
    #[serde(default)]
    pub stays: Vec<StayRecord>,
    /// The owner's place names the document was written with.
    #[serde(default)]
    pub names: String,
}

/// The stays and visits a day contributes to places. A visit completed by a
/// report on a later day is that day's to count, so it counts once.
pub fn stay_records(view: &DayLocation) -> Vec<StayRecord> {
    let mut records = Vec::new();
    for entry in &view.entries {
        let (device, start, end, point, kind) = match entry {
            Entry::Visit { sources, .. } if sources.iter().any(|source| source.day > view.day) => {
                continue;
            }
            Entry::Visit {
                device,
                arrival,
                departure,
                point,
                ..
            } => (
                *device,
                arrival.map(|time| time.utc.timestamp()),
                departure.map(|time| time.utc.timestamp()),
                point,
                "reported",
            ),
            Entry::Stay {
                device,
                first,
                last,
                point,
                ..
            } => (
                *device,
                Some(first.utc.timestamp()),
                Some(last.utc.timestamp()),
                point,
                "worked",
            ),
            _ => continue,
        };
        let Some(margin_m) = crate::accuracy::margin_m(point.accuracy_m) else {
            continue;
        };
        records.push(StayRecord {
            day: view.day.clone(),
            start,
            end,
            latitude: point.latitude,
            longitude: point.longitude,
            margin_m,
            device: view.devices[device].key.clone(),
            kind: kind.to_owned(),
        });
    }
    records
}

/// A rebuild could not finish.
#[derive(Debug)]
pub enum ProjectionError {
    Write { path: PathBuf, detail: String },
    Index { path: PathBuf, detail: String },
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Write { path, detail } => {
                write!(formatter, "could not write {}: {detail}", path.display())
            }
            Self::Index { path, detail } => write!(
                formatter,
                "wrote {} but the search index could not take it: {detail}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ProjectionError {}

/// What a rebuild did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rebuilt {
    /// The document already said exactly this.
    Unchanged,
    Written,
    /// The day no longer holds location; its files are gone.
    Removed,
    /// The day never held location.
    Nothing,
}

pub fn document_path(journal: &Path, day: &str) -> PathBuf {
    journal.join("chronicle").join(day).join(DAY_DOCUMENT)
}

pub fn record_path(journal: &Path, day: &str) -> PathBuf {
    journal
        .join("chronicle")
        .join(day)
        .join("health")
        .join(BUILD_RECORD)
}

/// The day's stream marker as a comparable token. Unreadable reads as its own
/// token, so it is retried rather than mistaken for unchanged.
pub fn source_token(journal: &Path, day: &str) -> String {
    let nanos = |modified: std::time::SystemTime| {
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    };
    match read_health_marker(journal, day, HealthMarkerKind::Stream) {
        Ok(HealthMarkerState::Absent) => "absent".to_owned(),
        Ok(HealthMarkerState::LegacyEmpty { modified }) => format!("legacy:{}", nanos(modified)),
        Ok(HealthMarkerState::MalformedNonEmpty { modified }) => {
            format!("malformed:{}", nanos(modified))
        }
        Ok(HealthMarkerState::Versioned { marker, .. }) => format!(
            "g{}:{}",
            marker.generation,
            marker.fingerprint.unwrap_or_default()
        ),
        Err(error) => format!("unreadable:{error}"),
    }
}

/// What a day's build depends on: its own segments and the next day's, where
/// a visit's complete report lands when the owner leaves after midnight.
pub fn build_token(journal: &Path, day: &str) -> String {
    let next = NaiveDate::parse_from_str(day, "%Y%m%d")
        .ok()
        .and_then(|date| date.succ_opt())
        .map(|date| source_token(journal, &date.format("%Y%m%d").to_string()))
        .unwrap_or_default();
    format!("{}+{next}", source_token(journal, day))
}

/// Read a day's build record, if it has a current one.
pub fn read_build_record(journal: &Path, day: &str) -> Option<BuildRecord> {
    let bytes = fs::read(record_path(journal, day)).ok()?;
    serde_json::from_slice::<BuildRecord>(&bytes)
        .ok()
        .filter(|record| record.version == RECORD_VERSION)
}

fn long_name(area: &AreaName) -> Option<String> {
    let label = area.label()?;
    let mut parts: Vec<String> = Vec::new();
    let mut push = |value: &str| {
        if !value.is_empty() && !parts.iter().any(|part| part == value) {
            parts.push(value.to_owned());
        }
    };
    let preposition = match &area.within {
        Some(within) if !within.is_empty() => {
            for name in [&within.neighbourhood, &within.locality]
                .into_iter()
                .flatten()
            {
                push(name);
            }
            "in"
        }
        _ => "near",
    };
    let Some(town) = &area.town else {
        if preposition == "in" {
            for name in [&area.region, &area.country].into_iter().flatten() {
                push(name);
            }
            return Some(format!("in {}", parts.join(", ")));
        }
        return Some(format!("with no named town nearby ({label})"));
    };
    if preposition == "near" {
        let mut name = town.name.clone();
        if let Some(ascii) = &town.ascii_name {
            name.push_str(&format!(" ({ascii})"));
        }
        push(&name);
    }
    if let Some(region) = &town.region {
        push(region);
    }
    push(&town.country);
    Some(format!("{preposition} {}", parts.join(", ")))
}

/// A category as words: `coffee_shop` reads "coffee shop".
pub fn category_words(category: &str) -> String {
    category.replace('_', " ")
}

fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn place_words(place: &StopWords) -> String {
    let mut words = String::new();
    if let Some(at) = &place.at {
        words.push_str(&format!("at {}", at.name));
        if let Some(inside) = &at.inside {
            words.push_str(&format!(" in {inside}"));
        }
    }
    if !place.near.is_empty() {
        if !words.is_empty() {
            words.push_str("; ");
        }
        let names: Vec<String> = place
            .near
            .iter()
            .map(|near| match near.category.as_str() {
                "" => near.name.clone(),
                category => format!("{} ({})", near.name, category_words(category)),
            })
            .collect();
        words.push_str(&format!("near {}", listed(&names)));
    }
    words
}

fn moment(time: &LocalTime, day: &str) -> String {
    let local_day = time.day();
    if local_day == day {
        return time.clock();
    }
    match NaiveDate::parse_from_str(&local_day, "%Y%m%d") {
        Ok(date) => format!("{} at {}", date.format("%B %-d"), time.clock()),
        Err(_) => time.clock(),
    }
}

/// What the owner calls the place a stay or visit belongs to, if anything.
fn owner_words(store: &PlaceStore, point: &Point) -> Option<String> {
    let margin = crate::accuracy::margin_m(point.accuracy_m)?;
    let place = store.place_for(point.latitude, point.longitude, margin)?;
    place
        .owner_name()
        .map(|name| format!("at {name} (a place you named)"))
}

fn naming(area: &AreaName, place: Option<&StopWords>, owner: Option<String>) -> Option<String> {
    let mut name = long_name(area)?;
    if let Some(place) = place {
        name = format!("{}; {name}", place_words(place));
    }
    if let Some(owner) = owner {
        name = format!("{owner}; {name}");
    }
    Some(name)
}

/// The day's search text. It names areas, places and times, never a
/// coordinate. Names the owner gave a place are used where they apply.
pub fn render_day_document(day: &DayLocation, store: &PlaceStore) -> String {
    let heading = NaiveDate::parse_from_str(&day.day, "%Y%m%d")
        .map(|date| date.format("%A, %B %-d, %Y").to_string())
        .unwrap_or_else(|_| day.day.clone());
    let mut text = format!("# location on {heading}\n\n");
    let mut lines = 0;
    for entry in &day.entries {
        match entry {
            Entry::Readings {
                device,
                area,
                precision,
                first,
                last,
                points,
                ..
            } => {
                let Some(name) = long_name(area) else {
                    continue;
                };
                let when = if first.utc == last.utc {
                    format!("at {}", moment(first, &day.day))
                } else {
                    format!("{} to {}", moment(first, &day.day), moment(last, &day.day))
                };
                let approximate = matches!(
                    precision,
                    crate::record::Precision::Approximate | crate::record::Precision::Reduced
                );
                let count = match (points.len(), approximate) {
                    (1, false) => "1 reading".to_owned(),
                    (1, true) => "1 approximate reading".to_owned(),
                    (count, false) => format!("{count} readings"),
                    (count, true) => format!("{count} approximate readings"),
                };
                text.push_str(&format!(
                    "- readings {name}, {when}: {count} from {}.\n",
                    day.device_phrase(*device)
                ));
                lines += 1;
            }
            Entry::Visit {
                device,
                area,
                place,
                arrival,
                departure,
                point,
                ..
            } => {
                let Some(name) = naming(area, place.as_ref(), owner_words(store, point)) else {
                    continue;
                };
                let arrived = arrival.as_ref().map_or_else(
                    || "arrival not known".to_owned(),
                    |time| format!("arrived {}", moment(time, &day.day)),
                );
                let left = departure.as_ref().map_or_else(
                    || "no departure reported".to_owned(),
                    |time| format!("left {}", moment(time, &day.day)),
                );
                text.push_str(&format!(
                    "- a visit {} reported {name}: {arrived}, {left}.\n",
                    day.device_phrase(*device)
                ));
                lines += 1;
            }
            Entry::Stay {
                device,
                area,
                place,
                first,
                last,
                count,
                point,
                ..
            } => {
                let Some(name) = naming(area, place.as_ref(), owner_words(store, point)) else {
                    continue;
                };
                text.push_str(&format!(
                    "- a stay your journal worked out from {count} readings from {} {name}: first reading {}, last {}.\n",
                    day.device_phrase(*device),
                    moment(first, &day.day),
                    moment(last, &day.day)
                ));
                lines += 1;
            }
            Entry::Unreadable { .. } => {}
        }
    }
    if lines == 0 {
        text.push_str("- location was kept, but no reading could be named.\n");
    }
    text
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), ProjectionError> {
    crate::reconcile_io::replace(path, bytes).map_err(|error| ProjectionError::Write {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })
}

fn remove(path: &Path) -> Result<bool, ProjectionError> {
    crate::reconcile_io::remove(path).map_err(|error| ProjectionError::Write {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })
}

fn index_document(journal: &Path, path: &Path) -> Result<(), ProjectionError> {
    crate::reconcile_io::enqueue(journal, path).map_err(|error| ProjectionError::Index {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })
}

fn forget_document(journal: &Path, day: &str) -> Result<(), ProjectionError> {
    let rel = format!("{day}/{DAY_DOCUMENT}");
    crate::reconcile_io::enqueue(journal, Path::new(&rel)).map_err(|error| ProjectionError::Index {
        path: document_path(journal, day),
        detail: error.to_string(),
    })
}

/// Rebuild one day's document from the segments it holds now.
pub fn rebuild_day(
    journal: &Path,
    day: &str,
    tier: &AreaTier,
    packs: Option<&Packs>,
    store: &PlaceStore,
    directory: &dyn DeviceDirectory,
    owner: Tz,
) -> Result<Rebuilt, ProjectionError> {
    crate::reconcile_io::transaction(|| {
        rebuild_day_inner(journal, day, tier, packs, store, directory, owner)
    })
}

fn rebuild_day_inner(
    journal: &Path,
    day: &str,
    tier: &AreaTier,
    packs: Option<&Packs>,
    store: &PlaceStore,
    directory: &dyn DeviceDirectory,
    owner: Tz,
) -> Result<Rebuilt, ProjectionError> {
    // Read the marker before the segments: a removal that lands while this
    // build reads advances it, and the next pass rebuilds again.
    let source = build_token(journal, day);
    let view = read_day(journal, day, tier, packs, directory, owner);
    let document = document_path(journal, day);
    let record = record_path(journal, day);
    if view.is_empty() {
        let had_document = remove(&document)?;
        let had_record = remove(&record)?;
        if had_document {
            forget_document(journal, day)?;
        }
        return Ok(if had_document || had_record {
            Rebuilt::Removed
        } else {
            Rebuilt::Nothing
        });
    }
    let text = render_day_document(&view, store);
    let built = BuildRecord {
        version: RECORD_VERSION,
        source,
        area_tier: AREA_TIER_SHA256.to_owned(),
        packs: packs
            .map(|packs| packs.identity().to_owned())
            .unwrap_or_default(),
        document: DOCUMENT_VERSION,
        readings: view.readings() as u64,
        visits: view.visits() as u64,
        devices: view.devices.len() as u64,
        stays: stay_records(&view),
        names: store.names_fingerprint(),
    };
    let unchanged = fs::read(&document).is_ok_and(|existing| existing == text.as_bytes());
    if !unchanged {
        // A restored current record must not hide an owed hint if rebuilding
        // its missing/corrupt document succeeds but hint publication refuses.
        if read_build_record(journal, day).as_ref() == Some(&built) {
            remove(&record)?;
        }
        write_file(&document, text.as_bytes())?;
    }
    // A current record certifies both the document and its index hint. If hint
    // publication refuses, leave the old record so reconciliation retries.
    index_document(journal, &document)?;
    if read_build_record(journal, day).as_ref() != Some(&built) {
        let mut bytes = serde_json::to_vec_pretty(&built).unwrap_or_default();
        bytes.push(b'\n');
        write_file(&record, &bytes)?;
    }
    if unchanged {
        return Ok(Rebuilt::Unchanged);
    }
    Ok(Rebuilt::Written)
}

fn holds_location(journal: &Path, day: &str) -> bool {
    iter_segments(journal, PathOrDay::Day(day))
        .unwrap_or_default()
        .iter()
        .any(|segment| segment.path().join(LOCATION_FILE).is_file())
}

/// What one reconcile pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub checked: usize,
    pub written: usize,
    pub removed: usize,
    pub failures: Vec<String>,
}

/// Remembers, across passes, which days are already current.
#[derive(Debug, Default)]
pub struct Reconciler {
    seen: HashMap<String, String>,
    /// The packs the seen days were checked against.
    packs: String,
    /// The owner's place names the seen days were checked against.
    names: String,
    /// Whether places have been worked out since this process started.
    places_current: bool,
}

impl Reconciler {
    /// Bring every day's document up to date with its segments. A day whose
    /// marker has not moved since the last pass costs one small read.
    pub fn reconcile(
        &mut self,
        journal: &Path,
        tier: &AreaTier,
        packs: Option<&Packs>,
        directory: &dyn DeviceDirectory,
        owner: Tz,
    ) -> ReconcileReport {
        let mut report = ReconcileReport::default();
        let identity = packs
            .map(|packs| packs.identity().to_owned())
            .unwrap_or_default();
        let store = PlaceStore::read(journal);
        let names = store.names_fingerprint();
        if identity != self.packs || names != self.names {
            // Downloading or removing the packs, or renaming a place,
            // rewrites each day once.
            self.seen.clear();
            self.packs.clone_from(&identity);
            self.names.clone_from(&names);
        }
        let mut days: Vec<String> = day_dirs(journal)
            .map(|days| days.into_keys().collect())
            .unwrap_or_default();
        days.sort_unstable();
        self.seen.retain(|day, _| days.binary_search(day).is_ok());
        for day in days {
            report.checked += 1;
            let token = build_token(journal, &day);
            if self.seen.get(&day) == Some(&token) {
                continue;
            }
            let current = read_build_record(journal, &day).is_some_and(|record| {
                record.source == token
                    && record.area_tier == AREA_TIER_SHA256
                    && record.packs == identity
                    && record.names == names
                    && record.document == DOCUMENT_VERSION
            });
            if current && document_path(journal, &day).is_file() {
                self.seen.insert(day, token);
                continue;
            }
            let has_files =
                document_path(journal, &day).exists() || record_path(journal, &day).exists();
            if !has_files && !holds_location(journal, &day) {
                self.seen.insert(day, token);
                continue;
            }
            match rebuild_day(journal, &day, tier, packs, &store, directory, owner) {
                Ok(Rebuilt::Written) => report.written += 1,
                Ok(Rebuilt::Removed) => report.removed += 1,
                Ok(Rebuilt::Unchanged | Rebuilt::Nothing) => {}
                Err(error) => {
                    report.failures.push(format!("{day}: {error}"));
                    continue;
                }
            }
            self.seen.insert(day, token);
        }
        if report.written + report.removed > 0 || !self.places_current {
            match recompute_places(journal) {
                Ok(()) => self.places_current = true,
                Err(error) => report.failures.push(format!("places: {error}")),
            }
        }
        report
    }
}

/// Every stay and visit the days' build records hold, in day order.
pub fn all_stays(journal: &Path) -> Vec<StayRecord> {
    let mut stays = Vec::new();
    let mut days: Vec<String> = day_dirs(journal)
        .map(|days| days.into_keys().collect())
        .unwrap_or_default();
    days.sort_unstable();
    for day in days {
        if let Some(record) = read_build_record(journal, &day) {
            stays.extend(record.stays);
        }
    }
    stays
}

/// Work out the places again from every day's recorded stays.
pub fn recompute_places(journal: &Path) -> std::io::Result<()> {
    let stays = all_stays(journal);
    PlaceStore::update(journal, |store| {
        crate::places::recompute(store, &stays);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::day::tests::{directory, fix, iphone_header, tier, write};

    const DAY: &str = "20261001";

    fn seed(root: &Path) {
        let header = iphone_header("full", "full");
        write(
            root,
            DAY,
            "phone_mobile-segment",
            "140000_300",
            &format!(
                "{header}\n{}\n{}\n",
                fix("2026-10-01T20:00:10Z", 39.7555, -105.2211, 10.0),
                fix("2026-10-01T20:04:10Z", 39.7556, -105.2212, 10.0)
            ),
        );
        write(
            root,
            DAY,
            "phone_mobile-segment",
            "150000_300",
            &format!(
                "{header}\n{}\n",
                r#"{"arrival":"2026-10-01T21:02:00Z","departure":"2026-10-01T22:40:00Z","h_acc":40,"lat":39.7527,"lon":-105.0001,"schema":"solstone.location.visit/1"}"#
            ),
        );
    }

    fn rebuild(root: &Path) -> Rebuilt {
        rebuild_day(
            root,
            DAY,
            tier(),
            None,
            &PlaceStore::default(),
            &directory(),
            chrono_tz::America::Denver,
        )
        .unwrap()
    }

    #[test]
    fn location_save_and_removal_queue_without_waiting_for_the_index() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        let lease = solstone_core_journal_io::hold_lock(
            solstone_core_indexer_store::db::db_path(root.path()),
            Default::default(),
        )
        .unwrap();
        assert_eq!(rebuild(root.path()), Rebuilt::Written);
        let expected = format!("{DAY}/{DAY_DOCUMENT}");
        assert_eq!(
            solstone_core_indexer_store::todo::inspect(root.path())
                .unwrap()
                .keys
                .as_slice(),
            std::slice::from_ref(&expected)
        );
        drop(lease);
        assert_eq!(
            solstone_core_indexer_store::todo::drain(root.path(), Default::default())
                .unwrap()
                .failed,
            0
        );
        let conn = solstone_core_indexer_store::open_index_reader(root.path()).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM chunks WHERE path=?1 AND chunks MATCH 'Golden'",
                [&expected],
                |row| row.get(0),
            )
            .unwrap();
        assert!(count > 0);
        let check = solstone_core_indexer_store::scan::scan_journal(root.path(), false).unwrap();
        assert_eq!((check.indexed, check.removed), (0, 0));
        let lease = solstone_core_journal_io::hold_lock(
            solstone_core_indexer_store::db::db_path(root.path()),
            Default::default(),
        )
        .unwrap();
        fs::remove_dir_all(
            root.path()
                .join("chronicle")
                .join(DAY)
                .join("phone_mobile-segment"),
        )
        .unwrap();
        rebuild(root.path());
        assert_eq!(
            solstone_core_indexer_store::todo::inspect(root.path())
                .unwrap()
                .keys
                .as_slice(),
            std::slice::from_ref(&expected)
        );
        drop(lease);
        assert_eq!(
            solstone_core_indexer_store::todo::drain(root.path(), Default::default())
                .unwrap()
                .failed,
            0
        );
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM files WHERE path=?1",
                [&expected],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        let check = solstone_core_indexer_store::scan::scan_journal(root.path(), false).unwrap();
        assert_eq!((check.indexed, check.removed), (0, 0));
    }

    #[test]
    fn the_document_holds_area_words_and_times_and_no_coordinates() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        assert_eq!(rebuild(root.path()), Rebuilt::Written);
        let text = fs::read_to_string(document_path(root.path(), DAY)).unwrap();
        assert!(text.starts_with("# location on Thursday, October 1, 2026\n"));
        assert!(text.contains(
            "- readings near Golden, Colorado, United States, 14:00 to 14:04: 2 readings from your iphone."
        ));
        assert!(text.contains(
            "- a visit your iphone reported near Denver, Colorado, United States: arrived 15:02, left 16:40."
        ));
        for coordinate in ["39.75", "105.2", "105.0"] {
            assert!(
                !text.contains(coordinate),
                "{coordinate} leaked into {text}"
            );
        }
        assert!(text.lines().any(|line| line.starts_with("- ")));
        let store = crate::places::PlaceStore {
            version: 1,
            places: vec![crate::places::Place {
                id: "named".to_owned(),
                latitude: 39.7527,
                longitude: -105.0001,
                name: Some("$source_packet".to_owned()),
                alias: None,
                hidden: false,
            }],
        };
        assert_eq!(
            rebuild_day(
                root.path(),
                DAY,
                tier(),
                None,
                &store,
                &directory(),
                chrono_tz::America::Denver,
            )
            .unwrap(),
            Rebuilt::Written
        );
        let named = fs::read_to_string(document_path(root.path(), DAY)).unwrap();
        assert!(named.contains("$source_packet"), "{named}");
        assert!(
            named
                .lines()
                .any(|line| { line.starts_with("- ") && line.contains("$source_packet") })
        );
        for coordinate in ["39.75", "105.2", "105.0"] {
            assert!(
                !named.contains(coordinate),
                "{coordinate} leaked into {named}"
            );
        }
        let record = read_build_record(root.path(), DAY).unwrap();
        assert_eq!((record.readings, record.visits, record.devices), (2, 1, 1));
        assert!(matches!(
            solstone_core_indexer_store::scan::rescan_file(
                root.path(),
                &document_path(root.path(), DAY)
            ),
            Ok(solstone_core_indexer_store::scan::RescanFileStatus::Indexed { .. })
        ));
    }

    #[test]
    fn with_the_download_a_stop_is_at_and_near_and_a_reading_is_in() {
        let root = tempfile::tempdir().unwrap();
        let header = iphone_header("full", "full");
        // The fixture's invented plaza is centred here; its coffee shop is
        // 30 m east and its diner 45 m south.
        write(
            root.path(),
            DAY,
            "phone_mobile-segment",
            "150000_300",
            &format!(
                "{header}\n{}\n{}\n",
                r#"{"arrival":"2026-10-01T21:02:00Z","departure":"2026-10-01T22:40:00Z","h_acc":48,"lat":39.7527,"lon":-105.0003,"schema":"solstone.location.visit/1"}"#,
                fix("2026-10-01T21:00:10Z", 39.7527, -105.0003, 8.0)
            ),
        );
        let packs = crate::accuracy::tests::fixture_packs();
        let built = rebuild_day(
            root.path(),
            DAY,
            tier(),
            Some(&packs),
            &PlaceStore::default(),
            &directory(),
            chrono_tz::America::Denver,
        )
        .unwrap();
        assert_eq!(built, Rebuilt::Written);
        let text = fs::read_to_string(document_path(root.path(), DAY)).unwrap();
        assert!(
            text.contains("- readings in Lower Invented, Inventedville, Colorado, United States, at 15:00: 1 reading from your iphone."),
            "{text}"
        );
        assert!(
            text.contains("- a visit your iphone reported at Invented Plaza; near Invented Coffee (coffee shop) and Invented Diner (restaurant); in Lower Invented, Inventedville, Colorado, United States: arrived 15:02, left 16:40."),
            "{text}"
        );
        let record = read_build_record(root.path(), DAY).unwrap();
        assert_eq!(record.packs, "venues:fixture-1;containment:fixture-1");
        // Without the download the same day is named from the area tier.
        rebuild(root.path());
        let text = fs::read_to_string(document_path(root.path(), DAY)).unwrap();
        assert!(!text.contains("Invented"), "{text}");
        assert!(
            read_build_record(root.path(), DAY)
                .unwrap()
                .packs
                .is_empty()
        );
    }

    #[test]
    fn visits_on_two_dates_make_a_place_and_its_owner_name_reaches_search() {
        let root = tempfile::tempdir().unwrap();
        let header = iphone_header("full", "light");
        for (day, iso) in [("20261001", "2026-10-01"), ("20261002", "2026-10-02")] {
            write(
                root.path(),
                day,
                "phone_mobile-segment",
                "150000_300",
                &format!(
                    "{header}\n{{\"arrival\":\"{iso}T21:02:00Z\",\"departure\":\"{iso}T22:40:00Z\",\"h_acc\":20,\"lat\":39.7527,\"lon\":-105.0001,\"schema\":\"solstone.location.visit/1\"}}\n"
                ),
            );
        }
        let mut reconciler = Reconciler::default();
        let mut pass = || {
            reconciler.reconcile(
                root.path(),
                tier(),
                None,
                &directory(),
                chrono_tz::America::Denver,
            )
        };
        let first = pass();
        assert_eq!((first.written, first.failures.len()), (2, 0), "{first:?}");
        let store = PlaceStore::read(root.path());
        assert_eq!(store.places.len(), 1);
        PlaceStore::update(root.path(), |store| {
            store.places[0].name = Some("the climbing gym".to_owned());
        })
        .unwrap();
        assert_eq!(
            pass().written,
            2,
            "a new name rewrites the days that use it"
        );
        let text = fs::read_to_string(document_path(root.path(), "20261002")).unwrap();
        assert!(
            text.contains("- a visit your iphone reported at the climbing gym (a place you named); near Denver"),
            "{text}"
        );
        assert_eq!(pass().written, 0);
        // Deleting both days' segments takes the place, and its name, away.
        for day in ["20261001", "20261002"] {
            fs::remove_dir_all(
                root.path()
                    .join("chronicle")
                    .join(day)
                    .join("phone_mobile-segment"),
            )
            .unwrap();
            solstone_core_journal_io::bump_stream_marker(root.path(), day).unwrap();
        }
        assert_eq!(pass().removed, 2);
        assert!(PlaceStore::read(root.path()).places.is_empty());
    }

    #[test]
    fn a_visit_completed_the_next_day_rewrites_its_arrival_day_and_counts_once() {
        let root = tempfile::tempdir().unwrap();
        let header = iphone_header("full", "light");
        let visit = |arrival: &str, departure: &str| {
            format!(
                "{header}\n{{\"arrival\":\"{arrival}\",\"departure\":{departure},\"h_acc\":20,\"lat\":39.7527,\"lon\":-105.0001,\"schema\":\"solstone.location.visit/1\"}}\n"
            )
        };
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "211000_300",
            &visit("2026-10-02T03:10:00Z", "null"),
        );
        solstone_core_journal_io::bump_stream_marker(root.path(), "20261001").unwrap();
        let mut reconciler = Reconciler::default();
        let mut pass = || {
            reconciler.reconcile(
                root.path(),
                tier(),
                None,
                &directory(),
                chrono_tz::America::Denver,
            )
        };
        assert_eq!(pass().written, 1);
        let text = fs::read_to_string(document_path(root.path(), "20261001")).unwrap();
        assert!(
            text.contains("arrived 21:10, no departure reported."),
            "{text}"
        );
        // The owner leaves after midnight: the complete report lands on the
        // next day, and the arrival day is written again without the orphan.
        write(
            root.path(),
            "20261002",
            "phone_mobile-segment",
            "080000_300",
            &visit("2026-10-02T03:11:00Z", "\"2026-10-02T14:00:00Z\""),
        );
        solstone_core_journal_io::bump_stream_marker(root.path(), "20261002").unwrap();
        assert_eq!(pass().written, 2);
        let text = fs::read_to_string(document_path(root.path(), "20261001")).unwrap();
        assert!(!text.contains("no departure reported"), "{text}");
        assert!(
            text.contains("arrived 21:11, left October 2 at 08:00."),
            "{text}"
        );
        let stays = all_stays(root.path());
        assert_eq!(stays.len(), 1, "{stays:?}");
        assert_eq!(stays[0].day, "20261002");
        assert_eq!(pass().written, 0);
    }

    #[test]
    fn downloading_the_packs_renames_every_day_once() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        let packs = crate::accuracy::tests::fixture_packs();
        let mut reconciler = Reconciler::default();
        let mut pass = |packs: Option<&Packs>| {
            reconciler.reconcile(
                root.path(),
                tier(),
                packs,
                &directory(),
                chrono_tz::America::Denver,
            )
        };
        assert_eq!(pass(None).written, 1);
        assert_eq!(pass(None).written, 0);
        let renamed = pass(Some(&packs));
        assert_eq!(
            (renamed.written, renamed.failures.len()),
            (1, 0),
            "a new identity rewrites the day"
        );
        assert_eq!(pass(Some(&packs)).written, 0);
        assert_eq!(pass(None).written, 1, "removing them renames it back");
    }

    #[test]
    fn a_reading_far_from_any_town_reads_as_a_sentence_without_in() {
        let root = tempfile::tempdir().unwrap();
        let header = iphone_header("full", "full");
        write(
            root.path(),
            DAY,
            "phone_mobile-segment",
            "120000_300",
            &format!(
                "{header}\n{}\n",
                fix("2026-10-01T18:00:10Z", 43.0, -107.5, 10.0)
            ),
        );
        rebuild(root.path());
        let text = fs::read_to_string(document_path(root.path(), DAY)).unwrap();
        assert!(
            text.contains("- readings with no named town nearby (Wyoming, United States), at 12:00: 1 reading from your iphone."),
            "{text}"
        );
    }

    #[test]
    fn an_unchanged_day_writes_nothing_on_replay() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        rebuild(root.path());
        let before = fs::metadata(document_path(root.path(), DAY))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(rebuild(root.path()), Rebuilt::Unchanged);
        let after = fs::metadata(document_path(root.path(), DAY))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn deleting_a_segment_rebuilds_from_what_survives_and_an_empty_day_loses_its_files() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        rebuild(root.path());
        let segments = root
            .path()
            .join("chronicle")
            .join(DAY)
            .join("phone_mobile-segment");
        fs::remove_dir_all(segments.join("150000_300")).unwrap();
        assert_eq!(rebuild(root.path()), Rebuilt::Written);
        let text = fs::read_to_string(document_path(root.path(), DAY)).unwrap();
        assert!(
            !text.contains("Denver"),
            "the deleted visit is gone: {text}"
        );
        fs::remove_dir_all(segments.join("140000_300")).unwrap();
        assert_eq!(rebuild(root.path()), Rebuilt::Removed);
        assert!(!document_path(root.path(), DAY).exists());
        assert!(!record_path(root.path(), DAY).exists());
    }

    #[test]
    fn a_reconcile_rebuilds_a_day_only_when_its_marker_moves() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        fs::create_dir_all(root.path().join("chronicle/20261002/laptop/090000_300")).unwrap();
        let mut reconciler = Reconciler::default();
        let pass = |reconciler: &mut Reconciler| {
            reconciler.reconcile(
                root.path(),
                tier(),
                None,
                &directory(),
                chrono_tz::America::Denver,
            )
        };
        let first = pass(&mut reconciler);
        assert_eq!((first.checked, first.written, first.removed), (2, 1, 0));
        assert!(first.failures.is_empty(), "{:?}", first.failures);
        assert!(!document_path(root.path(), "20261002").exists());
        assert_eq!(pass(&mut reconciler).written, 0);

        fs::remove_dir_all(
            root.path()
                .join("chronicle")
                .join(DAY)
                .join("phone_mobile-segment/150000_300"),
        )
        .unwrap();
        assert_eq!(
            pass(&mut reconciler).written,
            0,
            "the marker has not moved yet"
        );
        solstone_core_journal_io::bump_stream_marker(root.path(), DAY).unwrap();
        assert_eq!(pass(&mut reconciler).written, 1);
        let mut fresh = Reconciler::default();
        assert_eq!(
            pass(&mut fresh).written,
            0,
            "a restart trusts a current record"
        );
    }
}
