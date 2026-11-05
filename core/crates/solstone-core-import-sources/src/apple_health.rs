// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reading an Apple Health export into body records, and planning their windows.
//!
//! `docs/BODY_SEGMENTS.md` is the contract. What is read is one table here. A kind
//! outside it is counted and named, and its values are never held. A window is
//! merged on its own, so nothing here needs a whole import in memory:
//! `apple_health_run` streams an export through these functions in bounded
//! memory. The whole-export `read_export` and `plan` remain only as the
//! reference its tests compare against.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, FixedOffset, TimeZone, Timelike};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_body_ingest::{AppleElement, AppleItem};
#[cfg(test)]
use solstone_core_body_ingest::{BodyIngestError, scan_apple_export};
use solstone_core_format::content::body_records::{
    BODY_RECORDS_FILE, BodyRow, DATED_WINDOW, RowKind, decode_rows, encode_rows, parse_source_time,
    plain_words, sort_rows, source_words,
};
use solstone_core_import::RegistrySource;
use solstone_core_journal_config::{owner_zone, parse_zone};
use solstone_core_journal_io::{AtomicWriteOptions, day_dirs, write_bytes_exclusive};
use solstone_core_segment::{holds_foreign_content, owner_deleted};

use crate::save::{RenderedImport, SegmentFile};

pub const STREAM: &str = "import.apple_health";
pub(crate) const SOURCE: &str = "apple_health";
/// Held for a whole run, so merging into a window can't race another Apple import.
pub const LOCK_REL: &str = "imports/.apple_health";
const ZONE_FILE_REL: &str = "imports/.apple_health-zone.json";
/// A workout's or a mindfulness session's tiles, its events and its own series:
/// the ~300-second grid. Rows state their own times, so nothing reads meaning
/// from a window's length but placement.
pub(crate) const WINDOW_SECONDS: i64 = 300;
/// Sleep, and every row a workout or a mindfulness session doesn't keep.
pub(crate) const HOUR_SECONDS: i64 = 3_600;

/// Whether a row is a workout or a mindfulness session, which a reading from its
/// own source can sit beside. Sleep isn't one: its hours are the night.
pub(crate) fn is_session(row: &BodyRow) -> bool {
    row.kind == RowKind::Experience && row.experience.as_deref() != Some("sleep")
}

/// How long a row's window is. A workout or a mindfulness session, its tiles and
/// its events go on the grid, and so does a reading that one of them from its own
/// source covers (`covered`). Everything else goes on hours.
pub(crate) fn window_seconds(row: &BodyRow, covered: bool) -> i64 {
    let sleep = row.experience.as_deref() == Some("sleep") || row.kind == RowKind::Stage;
    let own = !sleep
        && matches!(
            row.kind,
            RowKind::Experience | RowKind::ExperienceTile | RowKind::Event
        );
    if own || (covered && row.kind == RowKind::Reading) {
        WINDOW_SECONDS
    } else {
        HOUR_SECONDS
    }
}

pub(crate) const MAX_EXPERIENCE_SECONDS: i64 = 14 * 86_400;
const MAX_ROW_BYTES: usize = 32 * 1024;
/// The rule a night is assembled by, Apple giving only stage intervals.
pub(crate) const NIGHT_GAP_SECONDS: i64 = 30 * 60;
pub(crate) const NIGHT_RULE: &str = "assembled from this source's adjacent sleep stage intervals 30 minutes or less apart, not a session Apple Health reported";
const WORKOUT_TYPE: &str = "Workout";
pub(crate) const SLEEP_TYPE: &str = "HKCategoryTypeIdentifierSleepAnalysis";
const MINDFUL_TYPE: &str = "HKCategoryTypeIdentifierMindfulSession";
const CATEGORY_PREFIX: &str = "HKCategoryTypeIdentifier";
const DIETARY_PREFIX: &str = "HKQuantityTypeIdentifierDietary";
/// Metadata that holds the owner's own words, which search may hold.
const WORDS_METADATA: [&str; 2] = ["HKFoodType", "HKWorkoutBrandName"];
/// The source's own identity for a record, when it gives one.
const SYNC_METADATA: [&str; 2] = ["HKMetadataKeySyncIdentifier", "HKExternalUUID"];

const READINGS: [&str; 14] = [
    "HKQuantityTypeIdentifierHeartRate",
    "HKQuantityTypeIdentifierRestingHeartRate",
    "HKQuantityTypeIdentifierWalkingHeartRateAverage",
    "HKQuantityTypeIdentifierHeartRateVariabilitySDNN",
    "HKQuantityTypeIdentifierBloodGlucose",
    "HKQuantityTypeIdentifierOxygenSaturation",
    "HKQuantityTypeIdentifierRespiratoryRate",
    "HKQuantityTypeIdentifierBloodPressureSystolic",
    "HKQuantityTypeIdentifierBloodPressureDiastolic",
    "HKQuantityTypeIdentifierBodyTemperature",
    "HKQuantityTypeIdentifierBasalBodyTemperature",
    "HKQuantityTypeIdentifierBodyMass",
    "HKQuantityTypeIdentifierBodyFatPercentage",
    "HKQuantityTypeIdentifierVO2Max",
];
const COUNTS: [&str; 5] = [
    "HKQuantityTypeIdentifierStepCount",
    "HKQuantityTypeIdentifierDistanceWalkingRunning",
    "HKQuantityTypeIdentifierDistanceCycling",
    "HKQuantityTypeIdentifierDistanceSwimming",
    "HKQuantityTypeIdentifierFlightsClimbed",
];
const NOT_LOGGED_CATEGORIES: [&str; 1] = ["HKCategoryTypeIdentifierAppleStandHour"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadKind {
    Sleep,
    Mindful,
    Reading,
    Count,
    Logged,
    NotRead,
}

/// The read list.
fn read_kind(record_type: &str) -> ReadKind {
    if record_type == SLEEP_TYPE {
        ReadKind::Sleep
    } else if record_type == MINDFUL_TYPE {
        ReadKind::Mindful
    } else if READINGS.contains(&record_type) {
        ReadKind::Reading
    } else if COUNTS.contains(&record_type) {
        ReadKind::Count
    } else if record_type.starts_with(DIETARY_PREFIX)
        || (record_type.starts_with(CATEGORY_PREFIX)
            && !NOT_LOGGED_CATEGORIES.contains(&record_type))
    {
        ReadKind::Logged
    } else {
        ReadKind::NotRead
    }
}

/// What an export held, as rows, plus what wasn't read and what was refused.
#[derive(Debug, Default)]
pub struct AppleExport {
    /// One row for each experience, placed when the plan tiles it.
    pub experiences: Vec<BodyRow>,
    /// Every other row, in the window of its own time.
    pub rows: Vec<BodyRow>,
    pub read: BTreeMap<String, u64>,
    pub not_read: BTreeMap<String, u64>,
    pub refused: BTreeMap<String, u64>,
}

/// A row as read, before it has an id: who recorded what and when, and the
/// row's bytes with an empty id. Rows sharing an identity are numbered by
/// content, so export order changes nothing.
pub(crate) struct Held {
    pub(crate) identity: String,
    pub(crate) start_ms: i64,
    /// A session sorts before what starts with it, so the readings it covers
    /// find it open.
    pub(crate) rank: u8,
    pub(crate) content: String,
    pub(crate) events: Vec<BodyRow>,
}

#[cfg(test)]
struct Pending {
    row: BodyRow,
    events: Vec<BodyRow>,
}

#[derive(Default)]
pub(crate) struct Reader {
    pub(crate) export: AppleExport,
    /// What the last visit read, for the caller to take.
    pub(crate) held: Vec<Held>,
}

pub(crate) fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let mut hex = String::with_capacity(32);
    for byte in &digest[..16] {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn instant_ms(text: Option<&String>) -> Option<i64> {
    parse_source_time(text?).map(|time| time.timestamp_millis())
}

fn fields_of(element: &AppleElement, core: &[&str]) -> BTreeMap<String, Value> {
    let mut fields: BTreeMap<String, Value> = element
        .attributes
        .iter()
        .filter(|(key, _)| !core.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    if !element.metadata.is_empty() {
        let metadata: Map<String, Value> = element
            .metadata
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect();
        fields.insert("metadata".to_owned(), Value::Object(metadata));
    }
    if !element.statistics.is_empty() {
        fields.insert("statistics".to_owned(), json!(element.statistics));
    }
    fields
}

fn words_of(element: &AppleElement) -> Vec<String> {
    WORDS_METADATA
        .iter()
        .filter_map(|key| element.metadata.get(*key))
        .map(|words| words.trim().to_owned())
        .filter(|words| !words.is_empty())
        .collect()
}

/// Identity is who recorded what and when, as instants, so an export rendering
/// another offset can't double a row. The value isn't part of it: a changed
/// value is an edit.
fn identity_of(record_type: &str, element: &AppleElement) -> Option<String> {
    let attributes = &element.attributes;
    let start = instant_ms(attributes.get("startDate"))?;
    let end = instant_ms(attributes.get("endDate")).unwrap_or(start);
    let created = instant_ms(attributes.get("creationDate")).unwrap_or(0);
    let by = attributes.get("sourceName").map_or("", String::as_str);
    let sync = SYNC_METADATA
        .iter()
        .find_map(|key| element.metadata.get(*key))
        .map_or("", String::as_str);
    Some(format!(
        "{SOURCE}\u{1f}{record_type}\u{1f}{by}\u{1f}{start}\u{1f}{end}\u{1f}{created}\u{1f}{sync}"
    ))
}

impl Reader {
    pub(crate) fn visit(&mut self, item: AppleItem) {
        match item {
            AppleItem::NotRead(name) => *self.export.not_read.entry(name).or_default() += 1,
            AppleItem::Refused(stage) => self.refuse(stage),
            AppleItem::Workout(element) => self.workout(&element),
            AppleItem::Record(element) => self.record(&element),
        }
    }

    pub(crate) fn refuse(&mut self, stage: &str) {
        *self.export.refused.entry(stage.to_owned()).or_default() += 1;
    }

    fn hold(&mut self, identity: String, row: BodyRow, events: Vec<BodyRow>) {
        let content = serde_json::to_string(&row).expect("a body row serializes");
        if content.len() > MAX_ROW_BYTES {
            self.refuse("row_too_large");
            return;
        }
        let start_ms = row
            .begins()
            .and_then(parse_source_time)
            .map_or(0, |time| time.timestamp_millis());
        *self.export.read.entry(row.label.clone()).or_default() += 1;
        let rank = u8::from(row.kind != RowKind::Experience);
        self.held.push(Held {
            identity,
            start_ms,
            rank,
            content,
            events,
        });
    }

    /// A span's two ends, refusing what can't be an experience.
    fn span(&mut self, element: &AppleElement) -> Option<(String, String)> {
        let (Some(start), Some(end)) = (
            element.attributes.get("startDate").cloned(),
            element.attributes.get("endDate").cloned(),
        ) else {
            self.refuse("time");
            return None;
        };
        let (Some(from), Some(to)) = (parse_source_time(&start), parse_source_time(&end)) else {
            self.refuse("time");
            return None;
        };
        if to < from {
            self.refuse("end_before_start");
            return None;
        }
        if (to - from).num_seconds() > MAX_EXPERIENCE_SECONDS {
            self.refuse("span_too_long");
            return None;
        }
        Some((start, end))
    }

    fn workout(&mut self, element: &AppleElement) {
        const CORE: [&str; 3] = ["workoutActivityType", "sourceName", "startDate"];
        let record_type = element
            .attributes
            .get("workoutActivityType")
            .map_or(WORKOUT_TYPE, String::as_str)
            .to_owned();
        let Some(identity) = identity_of(&record_type, element) else {
            self.refuse("time");
            return;
        };
        let Some((start, end)) = self.span(element) else {
            return;
        };
        let mut row = BodyRow::new(
            RowKind::Experience,
            String::new(),
            SOURCE,
            &record_type,
            plain_words(&record_type),
        );
        row.experience = Some("workout".to_owned());
        row.start = Some(start);
        row.end = Some(end);
        row.by = element.attributes.get("sourceName").cloned();
        row.fields = fields_of(element, &CORE);
        row.words = words_of(element);
        row.values = workout_values(element);
        let mut events = Vec::new();
        for event in &element.events {
            match workout_event(&row, event) {
                Some(event)
                    if serde_json::to_string(&event)
                        .expect("a body row serializes")
                        .len()
                        <= MAX_ROW_BYTES =>
                {
                    events.push(event);
                }
                Some(_) => self.refuse("row_too_large"),
                None => self.refuse("event_time"),
            }
        }
        self.hold(identity, row, events);
    }

    fn record(&mut self, element: &AppleElement) {
        const CORE: [&str; 5] = ["type", "sourceName", "startDate", "unit", "value"];
        let Some(record_type) = element.attributes.get("type").cloned() else {
            self.refuse("type");
            return;
        };
        let kind = read_kind(&record_type);
        if kind == ReadKind::NotRead {
            *self.export.not_read.entry(record_type).or_default() += 1;
            return;
        }
        let Some(identity) = identity_of(&record_type, element) else {
            self.refuse("time");
            return;
        };
        let start = element.attributes.get("startDate").cloned();
        let end = element.attributes.get("endDate").cloned();
        let row_kind = match kind {
            ReadKind::Sleep => RowKind::Stage,
            ReadKind::Mindful => RowKind::Experience,
            ReadKind::Reading => RowKind::Reading,
            ReadKind::Count => RowKind::Count,
            ReadKind::Logged => RowKind::Logged,
            ReadKind::NotRead => unreachable!("returned above"),
        };
        let label = if kind == ReadKind::Sleep {
            let value = element.attributes.get("value").map_or("", String::as_str);
            plain_words(value)
                .trim_start_matches("sleep analysis ")
                .to_owned()
        } else {
            plain_words(&record_type)
        };
        let mut row = BodyRow::new(row_kind, String::new(), SOURCE, &record_type, label);
        row.by = element.attributes.get("sourceName").cloned();
        row.value = element.attributes.get("value").cloned();
        row.unit = element.attributes.get("unit").cloned();
        row.fields = fields_of(element, &CORE);
        row.words = words_of(element);
        match kind {
            ReadKind::Reading => {
                row.at = start;
                // An instant reads as `at` alone; a span's end stays in `fields`.
                if end == row.at {
                    row.fields.remove("endDate");
                }
            }
            ReadKind::Mindful => {
                row.fields.remove("endDate");
                let Some((from, to)) = self.span(element) else {
                    return;
                };
                row.experience = Some("mindfulness".to_owned());
                row.label = "session".to_owned();
                row.start = Some(from);
                row.end = Some(to);
            }
            ReadKind::Logged => {
                row.fields.remove("endDate");
                let end = end.filter(|end| Some(end) != start.as_ref());
                if end.is_some() && self.span(element).is_none() {
                    return;
                }
                row.end = end;
                row.start = start;
            }
            ReadKind::Count | ReadKind::Sleep => {
                row.fields.remove("endDate");
                let Some((from, to)) = self.span(element) else {
                    return;
                };
                row.start = Some(from);
                row.end = Some(to);
            }
            ReadKind::NotRead => unreachable!("returned above"),
        }
        self.hold(identity, row, Vec::new());
    }

    /// The whole-export reference: give every held row its id, then assemble
    /// nights from their stages.
    #[cfg(test)]
    fn finish(mut self, pending: BTreeMap<String, BTreeMap<String, Pending>>) -> AppleExport {
        let mut stages: BTreeMap<String, Vec<BodyRow>> = BTreeMap::new();
        for (identity, contents) in pending {
            for (occurrence, Pending { mut row, events }) in contents.into_values().enumerate() {
                row.id = sha256_hex(&format!("{identity}\u{1f}{occurrence}"));
                match row.kind {
                    RowKind::Stage => stages
                        .entry(row.by.clone().unwrap_or_default())
                        .or_default()
                        .push(row),
                    RowKind::Experience => {
                        let events = number_events(&row.id, events);
                        self.export.rows.extend(events);
                        self.export.experiences.push(row);
                    }
                    _ => self.export.rows.push(row),
                }
            }
        }
        for (by, rows) in stages {
            self.assemble_nights(&by, rows);
        }
        self.export
    }

    #[cfg(test)]
    fn assemble_nights(&mut self, by: &str, mut rows: Vec<BodyRow>) {
        let start_of = |row: &BodyRow| {
            row.start
                .as_deref()
                .and_then(parse_source_time)
                .map(|time| time.timestamp())
                .unwrap_or_default()
        };
        let end_of = |row: &BodyRow| {
            row.end
                .as_deref()
                .and_then(parse_source_time)
                .map(|time| time.timestamp())
                .unwrap_or_default()
        };
        rows.sort_by_key(|row| (start_of(row), row.id.clone()));
        let mut nights: Vec<Vec<BodyRow>> = Vec::new();
        let mut reach = i64::MIN;
        for row in rows {
            if nights.is_empty() || start_of(&row) - reach > NIGHT_GAP_SECONDS {
                nights.push(Vec::new());
                reach = i64::MIN;
            }
            reach = reach.max(end_of(&row));
            nights.last_mut().expect("just pushed").push(row);
        }
        for stages in nights {
            let first = &stages[0];
            let last = stages
                .iter()
                .max_by_key(|row| end_of(row))
                .expect("a night has a stage");
            let id = sha256_hex(&format!(
                "{SOURCE}\u{1f}{SLEEP_TYPE}\u{1f}{by}\u{1f}{}",
                start_of(first)
            ));
            if end_of(last) - start_of(first) > MAX_EXPERIENCE_SECONDS {
                self.refuse("span_too_long");
                continue;
            }
            let mut night = BodyRow::new(
                RowKind::Experience,
                id.clone(),
                SOURCE,
                SLEEP_TYPE,
                "night".to_owned(),
            );
            night.experience = Some("sleep".to_owned());
            night.assembled = Some(NIGHT_RULE.to_owned());
            night.by = first.by.clone();
            night.start = first.start.clone();
            night.end = last.end.clone();
            self.export.experiences.push(night);
            for mut stage in stages {
                stage.of = Some(id.clone());
                self.export.rows.push(stage);
            }
        }
    }
}

fn workout_values(element: &AppleElement) -> BTreeMap<String, String> {
    let attributes = &element.attributes;
    let mut values = BTreeMap::new();
    for (name, value, unit) in [
        ("duration", "duration", "durationUnit"),
        ("distance", "totalDistance", "totalDistanceUnit"),
        ("energy", "totalEnergyBurned", "totalEnergyBurnedUnit"),
    ] {
        if let Some(value) = attributes.get(value) {
            let unit = attributes.get(unit).map_or("", String::as_str);
            values.insert(name.to_owned(), format!("{value} {unit}").trim().to_owned());
        }
    }
    for statistic in &element.statistics {
        let name = statistic.get("type").map_or("", String::as_str);
        let unit = statistic.get("unit").map_or("", String::as_str);
        for measure in ["sum", "average", "minimum", "maximum"] {
            if let Some(value) = statistic.get(measure) {
                values.insert(
                    format!("{} {measure}", plain_words(name)),
                    format!("{value} {unit}").trim().to_owned(),
                );
            }
        }
    }
    values
}

fn workout_event(workout: &BodyRow, event: &BTreeMap<String, String>) -> Option<BodyRow> {
    let record_type = event.get("type")?;
    let date = event.get("date")?;
    parse_source_time(date)?;
    let mut row = BodyRow::new(
        RowKind::Event,
        String::new(),
        SOURCE,
        record_type,
        plain_words(record_type),
    );
    row.by.clone_from(&workout.by);
    row.start = Some(date.clone());
    row.fields = event
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "type" | "date"))
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    Some(row)
}

/// The whole-export reference the streaming path is tested against.
#[cfg(test)]
pub fn read_export(source: &Path) -> Result<AppleExport, BodyIngestError> {
    let mut reader = Reader::default();
    let mut pending: BTreeMap<String, BTreeMap<String, Pending>> = BTreeMap::new();
    scan_apple_export(source, &mut |item| {
        reader.visit(item);
        for held in reader.held.drain(..) {
            let row = serde_json::from_str(&held.content).expect("a held row parses");
            pending.entry(held.identity).or_default().insert(
                held.content,
                Pending {
                    row,
                    events: held.events,
                },
            );
        }
    })?;
    Ok(reader.finish(pending))
}

/// Number a workout's events by content, as rows are: two splits at one moment
/// both stay, an exact repeat collapses, and export order changes nothing.
pub(crate) fn number_events(workout: &str, events: Vec<BodyRow>) -> Vec<BodyRow> {
    let mut numbered = Vec::new();
    let mut by_identity: BTreeMap<(String, String), BTreeMap<String, BodyRow>> = BTreeMap::new();
    for event in events {
        let content = serde_json::to_string(&event).expect("a body row serializes");
        by_identity
            .entry((
                event.record_type.clone(),
                event.start.clone().unwrap_or_default(),
            ))
            .or_default()
            .insert(content, event);
    }
    for ((record_type, start), contents) in by_identity {
        for (occurrence, mut event) in contents.into_values().enumerate() {
            event.of = Some(workout.to_owned());
            event.id = sha256_hex(&format!(
                "{workout}\u{1f}{record_type}\u{1f}{start}\u{1f}{occurrence}"
            ));
            numbered.push(event);
        }
    }
    numbered
}

/// The zone fixed at this source's first import, so importing again under a later
/// owner zone writes the same coordinates.
pub fn read_or_create_zone(journal: &Path) -> Result<Tz, String> {
    let path = journal.join(ZONE_FILE_REL);
    if let Ok(bytes) = fs::read(&path) {
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| "apple_health_zone_unrecognised")?;
        return value
            .get("zone")
            .and_then(Value::as_str)
            .and_then(parse_zone)
            .ok_or_else(|| "apple_health_zone_unrecognised".to_owned());
    }
    let zone = owner_zone(journal);
    let body = format!("{{\"zone\":\"{}\"}}\n", zone.name());
    match write_bytes_exclusive(
        &path,
        body.as_bytes(),
        AtomicWriteOptions { mode: Some(0o600) },
    ) {
        Ok(()) => Ok(zone),
        Err(error) => Err(error.to_string()),
    }
}

pub type WindowId = (String, String);

fn floor_to_window(time: DateTime<FixedOffset>, zone: Tz, length: i64) -> DateTime<Tz> {
    let local = time.with_timezone(&zone);
    let into_window = i64::from(local.num_seconds_from_midnight()) % length;
    zone.timestamp_opt(local.timestamp() - into_window, 0)
        .single()
        .expect("a timestamp maps to one zone time")
}

fn window_id(time: DateTime<Tz>, length: i64) -> WindowId {
    (
        time.format("%Y%m%d").to_string(),
        format!("{}_{length}", time.format("%H%M%S")),
    )
}

/// The windows a span covers, in order. An end on a window's first second doesn't
/// reach into it.
fn windows_of(
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
    zone: Tz,
    length: i64,
) -> Vec<WindowId> {
    let mut windows = Vec::new();
    let mut at = floor_to_window(start, zone, length);
    loop {
        windows.push(window_id(at, length));
        at += Duration::seconds(length);
        if at.timestamp() >= end.timestamp() {
            return windows;
        }
    }
}

/// The window an ordinary row sits in, before any step around older content.
pub(crate) fn row_window(row: &BodyRow, zone: Tz, covered: bool) -> Result<WindowId, String> {
    if let Some(date) = &row.date {
        let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|_| "a kept dated row has no valid date")?;
        return Ok((date.format("%Y%m%d").to_string(), DATED_WINDOW.to_owned()));
    }
    let time = row
        .begins()
        .and_then(parse_source_time)
        .ok_or("a kept row has no time")?;
    let length = window_seconds(row, covered);
    Ok(window_id(floor_to_window(time, zone, length), length))
}

/// An experience's first row and its tiles, each with the window it sits in.
/// Nothing shrinks: a stored end later than this export's is kept.
pub(crate) fn tile_experience(
    journal: &Path,
    experience: &BodyRow,
    zone: Tz,
) -> Result<Vec<(WindowId, BodyRow)>, String> {
    let length = window_seconds(experience, false);
    let start = experience
        .start
        .as_deref()
        .and_then(parse_source_time)
        .ok_or("a kept experience has no start")?;
    let mut end = experience
        .end
        .as_deref()
        .and_then(parse_source_time)
        .ok_or("a kept experience has no end")?;
    let mut first = experience.clone();
    let first_window = window_id(floor_to_window(start, zone, length), length);
    let stored_at = (
        first_window.0.clone(),
        resolve_key(journal, &first_window.0, &first_window.1),
    );
    if let Ok(Some((_, rows))) = stored_rows(journal, &stored_at)
        && let Some(stored) = rows.iter().find(|row| row.id == experience.id)
        && let Some(stored_end) = stored.end.as_deref().and_then(parse_source_time)
        && stored_end > end
    {
        end = stored_end;
        first.end.clone_from(&stored.end);
    }
    let tiles = windows_of(start, end, zone, length);
    let count = tiles.len() as u64;
    let mut placed = Vec::with_capacity(tiles.len());
    for (index, window) in tiles.into_iter().enumerate() {
        if index == 0 {
            placed.push((window, first.clone()));
            continue;
        }
        let mut tile = BodyRow::new(
            RowKind::ExperienceTile,
            format!("{}-{index}", first.id),
            SOURCE,
            &first.record_type,
            first.label.clone(),
        );
        tile.of = Some(first.id.clone());
        tile.experience.clone_from(&first.experience);
        tile.by.clone_from(&first.by);
        tile.start.clone_from(&first.start);
        tile.end.clone_from(&first.end);
        tile.index = Some(index as u64);
        tile.count = Some(count);
        placed.push((window, tile));
    }
    Ok(placed)
}

/// What a run will do. Nothing here has touched the journal.
#[derive(Debug, Default)]
pub struct Plan {
    pub files: Vec<SegmentFile>,
    /// Windows that must carry the chosen facet before they're indexed.
    pub facet_windows: Vec<WindowId>,
    /// Every window this run publishes, including those it doesn't rewrite.
    pub publish: Vec<WindowId>,
    pub unchanged: u64,
    pub stayed_deleted: u64,
    /// Windows whose stored file couldn't be read, left exactly as they are.
    pub unreadable: u64,
    /// Windows this run added rows to that keep another import's facet, or none.
    pub kept_facet: u64,
    /// Windows holding earlier rows that this run's facet now also covers.
    pub refiled: u64,
}

pub(crate) fn stream_dir(journal: &Path, day: &str) -> PathBuf {
    journal.join("chronicle").join(day).join(STREAM)
}

/// Where a window's records go: beside older content, never into it. An existing
/// `_301` keeps its records there even after what it stepped around is gone.
pub(crate) fn resolve_key(journal: &Path, day: &str, key: &str) -> String {
    let stream = stream_dir(journal, day);
    let Some((clock, length)) = key.split_once('_') else {
        return key.to_owned();
    };
    let Ok(length) = length.parse::<u64>() else {
        return key.to_owned();
    };
    let beside = format!("{clock}_{}", length + 1);
    if stream.join(&beside).join(BODY_RECORDS_FILE).exists() {
        beside
    } else if stream.join(key).join(BODY_RECORDS_FILE).exists() {
        // A file that turned up later doesn't move records that are already here.
        key.to_owned()
    } else if holds_foreign_content(&stream.join(key)) {
        beside
    } else {
        key.to_owned()
    }
}

/// The rows stored in a window, or `Err` for a file that can't be read as rows.
pub(crate) fn stored_rows(
    journal: &Path,
    (day, key): &WindowId,
) -> Result<Option<(String, Vec<BodyRow>)>, ()> {
    let path = stream_dir(journal, day).join(key).join(BODY_RECORDS_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    let rows = decode_rows(&text).map_err(|_| ())?;
    Ok(Some((text, rows)))
}

fn facet_file_holds(journal: &Path, (day, key): &WindowId, facet: &str) -> bool {
    let path = stream_dir(journal, day)
        .join(key)
        .join("talents/facets.json");
    fs::read_to_string(path).is_ok_and(|text| text == facet_file_text(facet))
}

/// The facet a window belongs to, in the form the classifier reads.
#[must_use]
pub fn facet_file_text(facet: &str) -> String {
    format!("{}\n", json!([{ "facet": facet }]))
}

/// What one window needs, decided against what it already holds.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Decision {
    pub stayed_deleted: bool,
    pub unreadable: bool,
    /// The window's new bytes, when they differ from what it holds.
    pub contents: Option<String>,
    pub units: u64,
    /// The chosen facet isn't the one the window carries.
    pub facet_changed: bool,
    pub publish: bool,
    pub unchanged: bool,
    pub kept_facet: bool,
    pub refiled: bool,
    /// This run's rows the window didn't hold, held with other content, and held as they are.
    pub new_rows: u64,
    pub updated_rows: u64,
    pub unchanged_rows: u64,
}

/// Merge one window's rows from this run into what it holds. A row takes the
/// stored row's place by id and keeps the run that first wrote it; the rest are
/// kept, because an export that omits a record isn't a deletion. `run` marks rows
/// this window didn't hold.
pub fn decide(
    journal: &Path,
    window: &WindowId,
    rows: BTreeMap<String, BodyRow>,
    facet: Option<&str>,
    run: Option<&str>,
) -> Result<Decision, String> {
    let dir = stream_dir(journal, &window.0).join(&window.1);
    let mut decision = Decision::default();
    if owner_deleted(&dir).map_err(|error| error.to_string())? {
        decision.stayed_deleted = true;
        return Ok(decision);
    }
    let Ok(stored) = stored_rows(journal, window) else {
        decision.unreadable = true;
        return Ok(decision);
    };
    let (stored_text, stored_rows) = stored.unzip();
    let mut merged_rows = stored_rows.unwrap_or_default();
    let mut incoming = rows;
    for row in &mut merged_rows {
        if let Some(mut new) = incoming.remove(&row.id) {
            new.run.clone_from(&row.run);
            if new == *row {
                decision.unchanged_rows += 1;
            } else {
                decision.updated_rows += 1;
                *row = new;
            }
        }
    }
    for mut new in incoming.into_values() {
        decision.new_rows += 1;
        new.run = run.map(str::to_owned);
        merged_rows.push(new);
    }
    sort_rows(&mut merged_rows);
    let contents = encode_rows(&merged_rows);
    let rows_changed = stored_text.as_deref() != Some(contents.as_str());
    decision.facet_changed = facet.is_some_and(|facet| !facet_file_holds(journal, window, facet));
    let unbound = !dir.join("stream.json").exists();
    decision.refiled = decision.facet_changed && stored_text.is_some();
    // A window carries one facet, so its rows share whichever it has.
    decision.kept_facet =
        facet.is_none() && rows_changed && dir.join("talents/facets.json").exists();
    decision.units = merged_rows.len() as u64;
    if rows_changed {
        decision.contents = Some(contents);
    }
    decision.publish = rows_changed || decision.facet_changed || unbound;
    decision.unchanged = !decision.publish;
    Ok(decision)
}

/// A day's windows, each with this run's rows for it by id.
pub type DayRows = Vec<(WindowId, BTreeMap<String, BodyRow>)>;

/// The length a window's key states, its step-around key included.
fn key_length(key: &str) -> Option<i64> {
    let length = key.split_once('_')?.1.parse::<i64>().ok()?;
    match length {
        WINDOW_SECONDS | 301 => Some(WINDOW_SECONDS),
        HOUR_SECONDS | 3_601 => Some(HOUR_SECONDS),
        _ => None,
    }
}

/// Settle where a day's readings go before the day is decided.
///
/// A reading has two places on its day: its hour, and the grid slot of a session
/// from its own source that covers it. Which one depends on the export, so it can
/// change between imports. A reading already saved in its other place stays
/// there, so no reading is ever in two windows. One whose other place the owner
/// deleted isn't added, because a tombstone says which files it removed, not which
/// readings, and a deleted reading must not come back. For a reading bound for its
/// hour, the slot only counts when no session at all covers it here
/// (`covered_any`): a slot a session still holds was that session's tile, which a
/// reading from another source was never in. An other place whose file can't be
/// read as rows holds the reading back too, as `decide` leaves such a window alone.
/// A place the journal can't be asked about at all stops the day, as a write there
/// would. Returns the windows, without any this left with nothing, and how many
/// readings were held back.
pub fn settle_readings(
    journal: &Path,
    day: &str,
    zone: Tz,
    windows: DayRows,
    covered_any: &BTreeSet<String>,
) -> Result<(DayRows, u64), String> {
    let mut by_key: BTreeMap<String, BTreeMap<String, BodyRow>> = windows
        .into_iter()
        .map(|((_, key), rows)| (key, rows))
        .collect();
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();
    let mut stored: BTreeMap<String, Result<BTreeSet<String>, ()>> = BTreeMap::new();
    let mut deleted: BTreeMap<String, bool> = BTreeMap::new();
    // The ids a window holds; `Err(())` when its file can't be read as rows.
    let holds = |stored: &mut BTreeMap<String, Result<BTreeSet<String>, ()>>,
                 key: &str|
     -> Result<Result<BTreeSet<String>, ()>, String> {
        if let Some(known) = stored.get(key) {
            return Ok(known.clone());
        }
        let path = stream_dir(journal, day).join(key).join(BODY_RECORDS_FILE);
        let found = match fs::read_to_string(&path) {
            Ok(text) => decode_rows(&text)
                .map(|rows| rows.into_iter().map(|row| row.id).collect())
                .map_err(|_| ()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        stored.insert(key.to_owned(), found.clone());
        Ok(found)
    };
    // (window, id, where it goes: another window, or nowhere)
    let mut moves: Vec<(String, String, Option<String>)> = Vec::new();
    for (key, rows) in &by_key {
        let Some(length) = key_length(key) else {
            continue;
        };
        for (id, row) in rows {
            if row.kind != RowKind::Reading || row.date.is_some() {
                continue;
            }
            let Some(time) = row.begins().and_then(parse_source_time) else {
                continue;
            };
            let other_length = if length == WINDOW_SECONDS {
                HOUR_SECONDS
            } else {
                WINDOW_SECONDS
            };
            let (other_day, raw) =
                window_id(floor_to_window(time, zone, other_length), other_length);
            if other_day != day {
                continue;
            }
            let other = resolved
                .entry(raw.clone())
                .or_insert_with(|| resolve_key(journal, day, &raw))
                .clone();
            match holds(&mut stored, &other)? {
                Ok(ids) if ids.contains(id) => {
                    moves.push((key.clone(), id.clone(), Some(other)));
                    continue;
                }
                Ok(_) => {}
                Err(()) => {
                    moves.push((key.clone(), id.clone(), None));
                    continue;
                }
            }
            if length == HOUR_SECONDS && covered_any.contains(id) {
                continue;
            }
            let gone = match deleted.get(&other) {
                Some(gone) => *gone,
                None => {
                    let gone = owner_deleted(&stream_dir(journal, day).join(&other))
                        .map_err(|error| error.to_string())?;
                    deleted.insert(other.clone(), gone);
                    gone
                }
            };
            if !gone {
                continue;
            }
            // Saved where it's bound already, it stays there.
            if holds(&mut stored, key)?.is_ok_and(|ids| ids.contains(id)) {
                continue;
            }
            moves.push((key.clone(), id.clone(), None));
        }
    }
    let mut held_back = 0_u64;
    for (from, id, to) in moves {
        let Some(row) = by_key.get_mut(&from).and_then(|rows| rows.remove(&id)) else {
            continue;
        };
        match to {
            Some(to) => {
                by_key.entry(to).or_default().insert(id, row);
            }
            None => held_back += 1,
        }
    }
    let windows = by_key
        .into_iter()
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(key, rows)| ((day.to_owned(), key), rows))
        .collect();
    Ok((windows, held_back))
}

/// The whole-export reference: merge an export's rows into their windows and
/// decide what each window needs. `repair` publishes every stored window too.
#[cfg(test)]
pub fn plan(
    journal: &Path,
    export: &AppleExport,
    zone: Tz,
    facet: Option<&str>,
    repair: bool,
) -> Result<Plan, String> {
    plan_with(journal, export, zone, facet, repair)
}

/// The reference keeps its own account of which readings a session covers,
/// straight from the export, apart from the streaming placer's.
#[cfg(test)]
pub(crate) fn plan_with(
    journal: &Path,
    export: &AppleExport,
    zone: Tz,
    facet: Option<&str>,
    repair: bool,
) -> Result<Plan, String> {
    let mut desired: BTreeMap<WindowId, BTreeMap<String, BodyRow>> = BTreeMap::new();
    let mut place = |window: &WindowId, row: BodyRow| {
        let key = resolve_key(journal, &window.0, &window.1);
        desired
            .entry((window.0.clone(), key))
            .or_default()
            .insert(row.id.clone(), row);
    };
    let millis = |text: Option<&str>| {
        text.and_then(parse_source_time)
            .map(|time| time.timestamp_millis())
    };
    let mut sessions = Vec::new();
    for experience in &export.experiences {
        let tiles = tile_experience(journal, experience, zone)?;
        if is_session(experience)
            && let (Some(start), Some(end)) = (
                millis(experience.start.as_deref()),
                millis(tiles[0].1.end.as_deref()),
            )
        {
            sessions.push((experience.by.clone(), start, end));
        }
        for (window, row) in tiles {
            place(&window, row);
        }
    }
    for row in &export.rows {
        let covered = row.kind == RowKind::Reading
            && row.date.is_none()
            && millis(row.begins()).is_some_and(|at| {
                sessions
                    .iter()
                    .any(|(by, start, end)| *by == row.by && *start <= at && at < *end)
            });
        place(&row_window(row, zone, covered)?, row.clone());
    }
    let mut plan = Plan::default();
    for (window, rows) in desired {
        let decision = decide(journal, &window, rows, facet, None)?;
        plan.absorb(&window, decision);
    }
    if repair {
        let already: BTreeSet<WindowId> = plan.publish.iter().cloned().collect();
        for window in stored_windows(journal)? {
            if !already.contains(&window) {
                plan.publish.push(window);
            }
        }
    }
    Ok(plan)
}

/// Every live body window in the stream.
#[cfg(test)]
fn stored_windows(journal: &Path) -> Result<Vec<WindowId>, String> {
    let mut windows = Vec::new();
    for day in day_dirs(journal)
        .map_err(|error| error.to_string())?
        .into_keys()
    {
        windows.extend(stored_windows_of_day(journal, &day)?);
    }
    Ok(windows)
}

/// The days that hold this stream, in order.
pub fn stream_days(journal: &Path) -> Result<Vec<String>, String> {
    let mut days: Vec<String> = day_dirs(journal)
        .map_err(|error| error.to_string())?
        .into_keys()
        .filter(|day| stream_dir(journal, day).is_dir())
        .collect();
    days.sort();
    Ok(days)
}

/// Every live body window on one day.
pub fn stored_windows_of_day(journal: &Path, day: &str) -> Result<Vec<WindowId>, String> {
    let mut windows = Vec::new();
    let Ok(entries) = fs::read_dir(stream_dir(journal, day)) else {
        return Ok(windows);
    };
    for entry in entries.flatten() {
        let Some(key) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        // Skip a removal in progress, whose directory still holds its file.
        if key.starts_with('.') {
            continue;
        }
        let dir = entry.path();
        if dir.join(BODY_RECORDS_FILE).is_file()
            && !owner_deleted(&dir).map_err(|error| error.to_string())?
        {
            windows.push((day.to_owned(), key));
        }
    }
    windows.sort();
    Ok(windows)
}

impl Plan {
    #[cfg(test)]
    fn absorb(&mut self, window: &WindowId, decision: Decision) {
        if decision.stayed_deleted {
            self.stayed_deleted += 1;
            return;
        }
        if decision.unreadable {
            self.unreadable += 1;
            return;
        }
        if decision.facet_changed {
            self.facet_windows.push(window.clone());
        }
        self.refiled += u64::from(decision.refiled);
        self.kept_facet += u64::from(decision.kept_facet);
        if let Some(contents) = decision.contents {
            self.files.push(SegmentFile {
                day: window.0.clone(),
                segment: window.1.clone(),
                name: BODY_RECORDS_FILE.to_owned(),
                units: decision.units,
                contents,
                calendar: false,
            });
        }
        if decision.publish {
            self.publish.push(window.clone());
        } else {
            self.unchanged += 1;
        }
    }

    /// The files to write, in the shape the ordinary save takes.
    #[must_use]
    pub fn rendered(&mut self) -> RenderedImport {
        RenderedImport {
            skipped: 0,
            source: RegistrySource::AppleHealth,
            entries: self.files.iter().map(|file| file.units).sum(),
            files: std::mem::take(&mut self.files),
            items: Vec::new(),
            summary: String::new(),
            calendar_counts: Default::default(),
        }
    }
}

fn count_of_windows(count: u64) -> String {
    if count == 1 {
        "1 window".to_owned()
    } else {
        format!("{count} windows")
    }
}

/// What a run did, for its summary. Names and counts only, never a value.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SummaryCounts {
    pub read: BTreeMap<String, u64>,
    pub not_read: BTreeMap<String, u64>,
    pub refused: BTreeMap<String, u64>,
    pub written: u64,
    pub unchanged: u64,
    pub stayed_deleted: u64,
    /// Readings not added because a place they could have been was deleted, or
    /// couldn't be read.
    #[serde(default)]
    pub held_back: u64,
    pub unreadable: u64,
    pub kept_facet: u64,
    pub refiled: u64,
    /// Windows or days that couldn't be published, as "day key: stage".
    #[serde(default)]
    pub not_saved: Vec<String>,
    /// Plain sentences about the run itself, such as a repair it made.
    #[serde(default)]
    pub notes: Vec<String>,
}

#[cfg(test)]
impl SummaryCounts {
    pub(crate) fn of(export: &AppleExport, plan: &Plan, written: u64) -> Self {
        Self {
            read: export.read.clone(),
            not_read: export.not_read.clone(),
            refused: export.refused.clone(),
            written,
            unchanged: plan.unchanged,
            stayed_deleted: plan.stayed_deleted,
            held_back: 0,
            unreadable: plan.unreadable,
            kept_facet: plan.kept_facet,
            refiled: plan.refiled,
            not_saved: Vec::new(),
            notes: Vec::new(),
        }
    }
}

/// The import's summary: what was read, what wasn't and what was refused, by name
/// and count. Never a value.
#[must_use]
pub fn summary_text(counts: &SummaryCounts, facet: Option<&str>, zone: Tz) -> String {
    let source = source_words(SOURCE);
    let mut text = format!(
        "# {source} import\n\n{} written, {} unchanged, {} stayed deleted, {} unreadable and left alone.{} Days and times placed using {}.\n\n",
        count_of_windows(counts.written),
        counts.unchanged,
        counts.stayed_deleted,
        counts.unreadable,
        match counts.held_back {
            0 => String::new(),
            1 => " 1 reading wasn't added, because the part of the journal it belongs to was deleted or couldn't be read.".to_owned(),
            n => format!(" {n} readings weren't added, because the part of the journal they belong to was deleted or couldn't be read."),
        },
        zone.name()
    );
    match facet {
        Some(facet) => {
            let _ = writeln!(text, "Filed under the {facet} facet.\n");
        }
        None => text.push_str(
            "Filed under no facet: a connection that can see the whole journal reads it, and one limited to chosen facets doesn't.\n\n",
        ),
    }
    if counts.kept_facet > 0 {
        let _ = writeln!(
            text,
            "{} a facet from an earlier import, so what this import added is readable under that facet too.\n",
            if counts.kept_facet == 1 {
                "1 window already carries".to_owned()
            } else {
                format!("{} windows already carry", counts.kept_facet)
            }
        );
    }
    if counts.refiled > 0 {
        let _ = writeln!(
            text,
            "{} already held records, and those are now filed under this facet instead of the one they had.\n",
            count_of_windows(counts.refiled)
        );
    }
    let mut section = |title: &str, items: Vec<(String, u64)>| {
        if !items.is_empty() {
            let _ = writeln!(text, "## {title}\n");
            for (name, count) in items {
                let _ = writeln!(text, "- {name}: {count}");
            }
            text.push('\n');
        }
    };
    section(
        "Read",
        counts
            .read
            .iter()
            .map(|(name, count)| (name.clone(), *count))
            .collect(),
    );
    section(
        "Not read",
        counts
            .not_read
            .iter()
            .map(|(name, count)| (name.clone(), *count))
            .collect(),
    );
    section(
        "Refused",
        counts
            .refused
            .iter()
            .map(|(name, count)| (name.clone(), *count))
            .collect(),
    );
    if !counts.not_saved.is_empty() {
        let _ = writeln!(text, "## Not saved\n");
        for line in &counts.not_saved {
            let _ = writeln!(text, "- {line}");
        }
        text.push('\n');
    }
    for note in &counts.notes {
        let _ = writeln!(text, "{note}\n");
    }
    text
}

#[cfg(test)]
#[path = "apple_health_tests.rs"]
mod tests;
