// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reading an Apple Health export into body records, and planning their windows.
//!
//! `docs/BODY_SEGMENTS.md` is the contract. What is read is one table here. A kind
//! outside it is counted and named, and its values are never held. Nothing in this
//! file needs a whole import in memory for the format's sake: a window is merged on
//! its own. The accumulator holds one run because the run has one flush today.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, FixedOffset, TimeZone, Timelike};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_body_ingest::{AppleElement, AppleItem, BodyIngestError, scan_apple_export};
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
const SOURCE: &str = "apple_health";
/// Held for a whole run, so merging into a window can't race another Apple import.
pub const LOCK_REL: &str = "imports/.apple_health";
/// Present from before a run's first window until that run is known to have
/// finished, so the next run repairs whatever this one left unbound.
pub const PUBLISHING_MARKER_REL: &str = "imports/.apple_health-publishing";
const ZONE_FILE_REL: &str = "imports/.apple_health-zone.json";
/// Every window's length. Rows state their own times, so nothing reads meaning
/// from this but placement.
const WINDOW_SECONDS: i64 = 300;
const MAX_EXPERIENCE_SECONDS: i64 = 14 * 86_400;
const MAX_ROW_BYTES: usize = 32 * 1024;
/// The rule a night is assembled by, Apple giving only stage intervals.
const NIGHT_GAP_SECONDS: i64 = 30 * 60;
const NIGHT_RULE: &str = "assembled from this source's adjacent sleep stage intervals 30 minutes or less apart, not a session Apple Health reported";
const WORKOUT_TYPE: &str = "Workout";
const SLEEP_TYPE: &str = "HKCategoryTypeIdentifierSleepAnalysis";
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
    pub refused: BTreeMap<&'static str, u64>,
}

struct Pending {
    row: BodyRow,
    events: Vec<BodyRow>,
}

#[derive(Default)]
struct Reader {
    export: AppleExport,
    /// By identity, then by content: an exact repeat collapses, and rows that
    /// differ are numbered by content, so export order changes nothing.
    pending: BTreeMap<String, BTreeMap<String, Pending>>,
}

fn sha256_hex(text: &str) -> String {
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
    fn visit(&mut self, item: AppleItem) {
        match item {
            AppleItem::NotRead(name) => *self.export.not_read.entry(name).or_default() += 1,
            AppleItem::Workout(element) => self.workout(&element),
            AppleItem::Record(element) => self.record(&element),
        }
    }

    fn refuse(&mut self, stage: &'static str) {
        *self.export.refused.entry(stage).or_default() += 1;
    }

    fn hold(&mut self, identity: String, row: BodyRow, events: Vec<BodyRow>) {
        let content = serde_json::to_string(&row).expect("a body row serializes");
        if content.len() > MAX_ROW_BYTES {
            self.refuse("row_too_large");
            return;
        }
        *self.export.read.entry(row.label.clone()).or_default() += 1;
        self.pending
            .entry(identity)
            .or_default()
            .insert(content, Pending { row, events });
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

    /// Give every held row its id, then assemble nights from their stages.
    fn finish(mut self) -> AppleExport {
        let mut stages: BTreeMap<String, Vec<BodyRow>> = BTreeMap::new();
        for (identity, contents) in std::mem::take(&mut self.pending) {
            for (occurrence, Pending { mut row, events }) in contents.into_values().enumerate() {
                row.id = sha256_hex(&format!("{identity}\u{1f}{occurrence}"));
                match row.kind {
                    RowKind::Stage => stages
                        .entry(row.by.clone().unwrap_or_default())
                        .or_default()
                        .push(row),
                    RowKind::Experience => {
                        self.number_events(&row.id, events);
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

    /// Number a workout's events by content, as rows are: two splits at one moment
    /// both stay, an exact repeat collapses, and export order changes nothing.
    fn number_events(&mut self, workout: &str, events: Vec<BodyRow>) {
        let mut by_identity: BTreeMap<(String, String), BTreeMap<String, BodyRow>> =
            BTreeMap::new();
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
                self.export.rows.push(event);
            }
        }
    }

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

/// Read an export. Its values reach nothing but the returned rows.
pub fn read_export(source: &Path) -> Result<AppleExport, BodyIngestError> {
    let mut reader = Reader::default();
    scan_apple_export(source, &mut |item| reader.visit(item))?;
    Ok(reader.finish())
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

type WindowId = (String, String);

fn floor_to_window(time: DateTime<FixedOffset>, zone: Tz) -> DateTime<Tz> {
    let local = time.with_timezone(&zone);
    let into_window = i64::from(local.num_seconds_from_midnight()) % WINDOW_SECONDS;
    zone.timestamp_opt(local.timestamp() - into_window, 0)
        .single()
        .expect("a timestamp maps to one zone time")
}

fn window_id(time: DateTime<Tz>) -> WindowId {
    (
        time.format("%Y%m%d").to_string(),
        format!("{}_{WINDOW_SECONDS}", time.format("%H%M%S")),
    )
}

/// The windows a span covers, in order. An end on a window's first second doesn't
/// reach into it.
fn windows_of(start: DateTime<FixedOffset>, end: DateTime<FixedOffset>, zone: Tz) -> Vec<WindowId> {
    let mut windows = Vec::new();
    let mut at = floor_to_window(start, zone);
    loop {
        windows.push(window_id(at));
        at += Duration::seconds(WINDOW_SECONDS);
        if at.timestamp() >= end.timestamp() {
            return windows;
        }
    }
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

fn stream_dir(journal: &Path, day: &str) -> PathBuf {
    journal.join("chronicle").join(day).join(STREAM)
}

/// Where a window's records go: beside older content, never into it. An existing
/// `_301` keeps its records there even after what it stepped around is gone.
fn resolve_key(journal: &Path, day: &str, key: &str) -> String {
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
fn stored_rows(
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

/// Merge an export's rows into their windows and decide what each window needs.
///
/// A window takes this export's rows by id and keeps the rest, because an export
/// that omits a record isn't a deletion. `repair` means the last run didn't finish,
/// so every window already in the stream publishes again, in this export or not.
pub fn plan(
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
    for row in &export.rows {
        let window = if let Some(date) = &row.date {
            let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .map_err(|_| "a kept dated row has no valid date")?;
            (date.format("%Y%m%d").to_string(), DATED_WINDOW.to_owned())
        } else {
            let time = row
                .begins()
                .and_then(parse_source_time)
                .ok_or("a kept row has no time")?;
            window_id(floor_to_window(time, zone))
        };
        place(&window, row.clone());
    }
    for experience in &export.experiences {
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
        let first_window = window_id(floor_to_window(start, zone));
        let stored_at = (
            first_window.0.clone(),
            resolve_key(journal, &first_window.0, &first_window.1),
        );
        // Nothing shrinks: an export with fewer stages can't shorten a stored night.
        if let Ok(Some((_, rows))) = stored_rows(journal, &stored_at)
            && let Some(stored) = rows.iter().find(|row| row.id == experience.id)
            && let Some(stored_end) = stored.end.as_deref().and_then(parse_source_time)
            && stored_end > end
        {
            end = stored_end;
            first.end.clone_from(&stored.end);
        }
        let tiles = windows_of(start, end, zone);
        let count = tiles.len() as u64;
        place(&tiles[0], first.clone());
        for (index, window) in tiles.iter().enumerate().skip(1) {
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
            place(window, tile);
        }
    }

    let mut plan = Plan::default();
    for (window, rows) in desired {
        let dir = stream_dir(journal, &window.0).join(&window.1);
        if owner_deleted(&dir).map_err(|error| error.to_string())? {
            plan.stayed_deleted += 1;
            continue;
        }
        let Ok(stored) = stored_rows(journal, &window) else {
            plan.unreadable += 1;
            continue;
        };
        let (stored_text, mut merged) = stored.unzip();
        let mut merged_rows = merged.take().unwrap_or_default();
        merged_rows.retain(|row| !rows.contains_key(&row.id));
        merged_rows.extend(rows.into_values());
        sort_rows(&mut merged_rows);
        let contents = encode_rows(&merged_rows);
        let rows_changed = stored_text.as_deref() != Some(contents.as_str());
        let facet_changed = facet.is_some_and(|facet| !facet_file_holds(journal, &window, facet));
        let unbound = !dir.join("stream.json").exists();
        if facet_changed {
            plan.facet_windows.push(window.clone());
            if stored_text.is_some() {
                plan.refiled += 1;
            }
        }
        // A window carries one facet, so its rows share whichever it has.
        if facet.is_none() && rows_changed && dir.join("talents/facets.json").exists() {
            plan.kept_facet += 1;
        }
        if rows_changed {
            plan.files.push(SegmentFile {
                day: window.0.clone(),
                segment: window.1.clone(),
                name: BODY_RECORDS_FILE.to_owned(),
                units: merged_rows.len() as u64,
                contents,
                calendar: false,
            });
        }
        if rows_changed || facet_changed || unbound {
            plan.publish.push(window);
        } else {
            plan.unchanged += 1;
        }
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
fn stored_windows(journal: &Path) -> Result<Vec<WindowId>, String> {
    let mut windows = Vec::new();
    for day in day_dirs(journal)
        .map_err(|error| error.to_string())?
        .into_keys()
    {
        let Ok(entries) = fs::read_dir(stream_dir(journal, &day)) else {
            continue;
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
                windows.push((day.clone(), key));
            }
        }
    }
    Ok(windows)
}

impl Plan {
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

/// The import's summary: what was read, what wasn't and what was refused, by name
/// and count. Never a value.
#[must_use]
pub fn summary_text(
    export: &AppleExport,
    plan: &Plan,
    written: u64,
    facet: Option<&str>,
    zone: Tz,
) -> String {
    let source = source_words(SOURCE);
    let mut text = format!(
        "# {source} import\n\n{} written, {} unchanged, {} stayed deleted, {} unreadable and left alone. Days and times placed using {}.\n\n",
        count_of_windows(written),
        plan.unchanged,
        plan.stayed_deleted,
        plan.unreadable,
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
    if plan.kept_facet > 0 {
        let _ = writeln!(
            text,
            "{} a facet from an earlier import, so what this import added is readable under that facet too.\n",
            if plan.kept_facet == 1 {
                "1 window already carries".to_owned()
            } else {
                format!("{} windows already carry", plan.kept_facet)
            }
        );
    }
    if plan.refiled > 0 {
        let _ = writeln!(
            text,
            "{} already held records, and those are now filed under this facet instead of the one they had.\n",
            count_of_windows(plan.refiled)
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
        export
            .read
            .iter()
            .map(|(name, count)| (name.clone(), *count))
            .collect(),
    );
    section(
        "Not read",
        export
            .not_read
            .iter()
            .map(|(name, count)| (name.clone(), *count))
            .collect(),
    );
    section(
        "Refused",
        export
            .refused
            .iter()
            .map(|(name, count)| ((*name).to_owned(), *count))
            .collect(),
    );
    text
}

#[cfg(test)]
#[path = "apple_health_tests.rs"]
mod tests;
