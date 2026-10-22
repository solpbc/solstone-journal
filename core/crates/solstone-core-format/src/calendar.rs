// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Calendar notes a linked device sends from the owner's calendar.
//!
//! A note is one JSON line saying an event was added, changed or removed, and
//! when that change entered the owner's life (`changed_at`). Notes are stored
//! in the segment of that moment and are never read as something the owner
//! did. The search index also lists each event under the days it happens.
//!
//! This module is the one reading of the line contract: ingest validates a file
//! with [`validate_calendar_file`], the index reads one with
//! [`read_calendar_notes`], and both list days with [`listed_days`].

use std::collections::BTreeSet;
use std::fmt;

use chrono::{
    DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Timelike,
};
use serde_json::{Map, Value};

use crate::segment::{is_date_key, segment_parse};

/// The ingest source a calendar stream is bound to.
pub const CALENDAR_SOURCE: &str = "calendar";

/// The name prefix every calendar note file carries.
pub const CALENDAR_FILE_PREFIX: &str = "calendar_";

/// The most days one event is listed under.
pub const MAX_LISTED_DAYS: usize = 366;

/// The most notes one file holds; a larger set splits across files.
pub const MAX_FILE_LINES: usize = 5_000;

/// The most bytes one file holds.
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

/// The length of the window every file is sent for, on the journal's grid.
const WINDOW_SECONDS: u32 = 300;

/// Every key a note line carries, whatever its change.
const REQUIRED_KEYS: [&str; 17] = [
    "v",
    "change",
    "event",
    "occurrence",
    "changed_at",
    "calendar",
    "title",
    "location",
    "notes",
    "url",
    "start",
    "end",
    "all_day",
    "time_zone",
    "organizer",
    "attendees",
    "repeats",
];

/// Display fields that hold a string or `null`.
const TEXT_FIELDS: [&str; 7] = [
    "calendar",
    "title",
    "location",
    "notes",
    "url",
    "repeats",
    "time_zone",
];

/// Longest notes kept in rendered text. Search reads a bounded entry, so a
/// longer note is cut rather than leaving a hit that cannot be read back.
const MAX_NOTES_CHARS: usize = 2_000;
/// Longest value of any other text field kept in rendered text.
const MAX_FIELD_CHARS: usize = 300;
/// Most attendees named in rendered text.
const MAX_ATTENDEES: usize = 50;
/// Longest response status kept for one attendee.
const MAX_STATUS_CHARS: usize = 40;
/// Most bytes of rendered text. Characters can take up to four bytes, so the
/// per-field limits alone do not bound the bytes; this keeps every note well
/// inside the 64 KiB entry search reads back.
const MAX_RENDERED_BYTES: usize = 32 * 1024;

/// Whether `name` is a calendar note file name: `calendar_<64 lowercase hex>.jsonl`.
pub fn is_calendar_file_name(name: &str) -> bool {
    name.strip_prefix(CALENDAR_FILE_PREFIX)
        .and_then(|rest| rest.strip_suffix(".jsonl"))
        .is_some_and(|id| {
            id.len() == 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

/// Whether `name` claims to be a calendar note file, valid or not.
pub fn names_calendar_file(name: &str) -> bool {
    name.starts_with(CALENDAR_FILE_PREFIX)
}

/// What happened to an event. The order is the tie-break for notes recorded
/// at the same instant: a removal outranks a change, which outranks an add.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Change {
    Added,
    Changed,
    Removed,
}

impl Change {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Changed => "changed",
            Self::Removed => "removed",
        }
    }

    /// The rank the index stores for the tie-break order.
    pub fn rank(self) -> i64 {
        match self {
            Self::Added => 0,
            Self::Changed => 1,
            Self::Removed => 2,
        }
    }
}

/// One end of an event's time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum When {
    /// A time in a zone, carrying its offset at that instant.
    Zoned(DateTime<FixedOffset>),
    /// A wall-clock time with no zone.
    Floating(NaiveDateTime),
    /// A date, for an all-day event.
    Date(NaiveDate),
}

/// When an added or changed event happens.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Schedule {
    pub all_day: bool,
    pub start: When,
    pub end: Option<When>,
}

/// One validated calendar note.
#[derive(Clone, Debug, PartialEq)]
pub struct CalendarNote {
    pub change: Change,
    pub event: String,
    /// The occurrence in its normalized form, or empty for an event that does
    /// not repeat. Equal instants written differently normalize the same.
    pub occurrence: String,
    pub changed_at: DateTime<FixedOffset>,
    /// When the event happens; absent for a removal.
    pub schedule: Option<Schedule>,
    /// The line as sent, for display.
    pub record: Map<String, Value>,
}

impl CalendarNote {
    /// The event's identity: its id and its normalized occurrence.
    pub fn identity(&self) -> (&str, &str) {
        (&self.event, &self.occurrence)
    }

    /// `changed_at` as milliseconds since the epoch.
    pub fn changed_at_ms(&self) -> i64 {
        self.changed_at.timestamp_millis()
    }

    /// `changed_at` exactly as the device wrote it.
    pub fn changed_at_text(&self) -> &str {
        self.record
            .get("changed_at")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// Why a calendar note file was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarFileError {
    /// The 1-based line, or `None` for the file as a whole.
    pub line: Option<usize>,
    pub reason: String,
}

impl fmt::Display for CalendarFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(formatter, "line {line}: {}", self.reason),
            None => formatter.write_str(&self.reason),
        }
    }
}

impl std::error::Error for CalendarFileError {}

fn whole_file(reason: impl Into<String>) -> CalendarFileError {
    CalendarFileError {
        line: None,
        reason: reason.into(),
    }
}

/// Check a whole calendar note file against the contract before it is stored.
///
/// `day` and `segment` are the envelope's: every note's `changed_at`, read as
/// wall-clock time in its own offset, must fall inside that segment's window.
/// Neither the envelope's `meta` nor any configured zone takes part.
pub fn validate_calendar_file(
    name: &str,
    bytes: &[u8],
    day: &str,
    segment: &str,
) -> Result<Vec<CalendarNote>, CalendarFileError> {
    check_file_name(name)?;
    let window = segment_window(day, segment).ok_or_else(|| {
        whole_file("segment must name one 300-second window on the journal's grid")
    })?;
    validate_lines(bytes, window, |_| Ok(()))
}

/// Check a calendar file the importer wrote beside an import window's
/// transcript before the index reads it.
///
/// Import segments are not on the 300-second grid, so every note's
/// `changed_at`, read in its own offset, must fall inside that segment's own
/// span: its start and its length. Each line also names the import that wrote
/// it and when that import happened, and a line written by expanding a
/// series' rule says so with `"expanded": true`.
pub fn validate_import_calendar_file(
    name: &str,
    bytes: &[u8],
    day: &str,
    segment: &str,
) -> Result<Vec<CalendarNote>, CalendarFileError> {
    check_file_name(name)?;
    let window = segment_span(day, segment)
        .ok_or_else(|| whole_file("segment must name a span on its day"))?;
    validate_lines(bytes, window, |note| {
        import_stamp(&note.record)?;
        match note.record.get("expanded") {
            None | Some(Value::Bool(true)) => Ok(()),
            Some(_) => Err("expanded must be true when present".to_owned()),
        }
    })
}

/// The import a line written by the importer came from, and when that import
/// happened.
pub fn import_stamp(
    record: &Map<String, Value>,
) -> Result<(String, DateTime<FixedOffset>), String> {
    let import = match record.get("import") {
        Some(Value::String(import)) if !import.is_empty() => import.clone(),
        _ => return Err("import must be a non-empty string".to_owned()),
    };
    let imported_at = record
        .get("imported_at")
        .and_then(Value::as_str)
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .ok_or_else(|| "imported_at must be a time with an offset".to_owned())?;
    Ok((import, imported_at))
}

fn check_file_name(name: &str) -> Result<(), CalendarFileError> {
    if is_calendar_file_name(name) {
        Ok(())
    } else {
        Err(whole_file(
            "file name must be calendar_ followed by 64 lowercase hex digits and .jsonl",
        ))
    }
}

fn validate_lines(
    bytes: &[u8],
    (window_start, window_end): (NaiveDateTime, NaiveDateTime),
    extra: impl Fn(&CalendarNote) -> Result<(), String>,
) -> Result<Vec<CalendarNote>, CalendarFileError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(whole_file("file holds more than 8 MiB"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| whole_file("file is not UTF-8"))?;
    if text.lines().count() > MAX_FILE_LINES {
        return Err(whole_file("file holds more than 5,000 notes"));
    }
    let mut notes = Vec::new();
    let mut identities = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        let at = |reason: String| CalendarFileError {
            line: Some(index + 1),
            reason,
        };
        if line.trim().is_empty() {
            return Err(at("line is empty".to_owned()));
        }
        let note = parse_calendar_note(line).map_err(at)?;
        extra(&note).map_err(at)?;
        if !identities.insert((note.event.clone(), note.occurrence.clone())) {
            return Err(at("event and occurrence repeat an earlier line".to_owned()));
        }
        let local = note.changed_at.naive_local();
        if local < window_start || local >= window_end {
            return Err(at("changed_at falls outside the segment window".to_owned()));
        }
        notes.push(note);
    }
    if notes.is_empty() {
        return Err(whole_file("file holds no notes"));
    }
    Ok(notes)
}

/// Read the notes of a stored file for indexing, with each note's line index.
///
/// Stored files were checked on arrival, so this reads them the same way but
/// keeps going past a line it cannot use, reporting it as a warning. A later
/// line repeating an earlier identity is skipped the same way.
pub fn read_calendar_notes(text: &str) -> (Vec<(usize, CalendarNote)>, Vec<String>) {
    let mut notes = Vec::new();
    let mut warnings = Vec::new();
    let mut identities = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match parse_calendar_note(line) {
            Ok(note) => {
                if identities.insert((note.event.clone(), note.occurrence.clone())) {
                    notes.push((index, note));
                } else {
                    warnings.push(format!("calendar note line {} repeats an event", index + 1));
                }
            }
            Err(reason) => {
                warnings.push(format!(
                    "calendar note line {} skipped: {reason}",
                    index + 1
                ));
            }
        }
    }
    (notes, warnings)
}

/// The window a segment key names on its day, start inclusive and end
/// exclusive. Only an `HHMMSS_300` key starting on the 300-second grid names
/// one.
fn segment_window(day: &str, segment: &str) -> Option<(NaiveDateTime, NaiveDateTime)> {
    if !is_date_key(day) {
        return None;
    }
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let times = segment_parse(segment)?;
    let (clock, length) = segment.split_once('_')?;
    if clock.len() != 6 || length != WINDOW_SECONDS.to_string() {
        return None;
    }
    let (hour, minute, second) = (
        u32::from(times.hour),
        u32::from(times.minute),
        u32::from(times.second),
    );
    if (hour * 3_600 + minute * 60 + second) % WINDOW_SECONDS != 0 {
        return None;
    }
    let start = date.and_hms_opt(hour, minute, second)?;
    Some((start, start + Duration::seconds(i64::from(WINDOW_SECONDS))))
}

/// The span any segment key names on its day: its start, and its start plus
/// its length.
pub fn segment_span(day: &str, segment: &str) -> Option<(NaiveDateTime, NaiveDateTime)> {
    if !is_date_key(day) {
        return None;
    }
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let times = segment_parse(segment)?;
    let (clock, length) = segment.split_once('_')?;
    if clock.len() != 6 {
        return None;
    }
    let length: i64 = length.parse().ok().filter(|length| *length > 0)?;
    let start = date.and_hms_opt(
        u32::from(times.hour),
        u32::from(times.minute),
        u32::from(times.second),
    )?;
    Some((start, start + Duration::seconds(length)))
}

/// Write calendar notes as calendar note files: compact JSON with sorted keys,
/// lines sorted by `event` and then `occurrence` with no occurrence first, each
/// file ending in a newline and named by the SHA-256 of its bytes. A set over
/// 5,000 notes or 8 MiB splits, in order, into consecutive files.
pub fn write_calendar_files(records: Vec<Map<String, Value>>) -> Vec<(String, Vec<u8>)> {
    let mut lines = records
        .into_iter()
        .map(|record| {
            let event = record
                .get("event")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let occurrence = record
                .get("occurrence")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let mut line = sorted_keys(&Value::Object(record)).to_string();
            line.push('\n');
            ((event, occurrence), line)
        })
        .collect::<Vec<_>>();
    lines.sort_by(|left, right| left.0.cmp(&right.0));
    let mut files = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    let mut count = 0;
    for (_, line) in lines {
        if count == MAX_FILE_LINES || (count > 0 && current.len() + line.len() > MAX_FILE_BYTES) {
            files.push(std::mem::take(&mut current));
            count = 0;
        }
        current.extend_from_slice(line.as_bytes());
        count += 1;
    }
    if count > 0 {
        files.push(current);
    }
    files
        .into_iter()
        .map(|bytes| (calendar_file_name(&bytes), bytes))
        .collect()
}

/// The name of a calendar note file holding these bytes.
pub fn calendar_file_name(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{CALENDAR_FILE_PREFIX}{:x}.jsonl", Sha256::digest(bytes))
}

fn sorted_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), sorted_keys(&map[key])))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted_keys).collect()),
        other => other.clone(),
    }
}

/// Parse and check one note line against the contract.
pub fn parse_calendar_note(line: &str) -> Result<CalendarNote, String> {
    let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line) else {
        return Err("line is not a JSON object".to_owned());
    };
    calendar_note_from_record(record)
}

/// Check one already-parsed note line against the contract.
pub fn calendar_note_from_record(record: Map<String, Value>) -> Result<CalendarNote, String> {
    for key in REQUIRED_KEYS {
        if !record.contains_key(key) {
            return Err(format!("{key} is missing"));
        }
    }
    if record.get("v").and_then(Value::as_u64) != Some(1) {
        return Err("v must be 1".to_owned());
    }
    let change = match record.get("change").and_then(Value::as_str) {
        Some("added") => Change::Added,
        Some("changed") => Change::Changed,
        Some("removed") => Change::Removed,
        _ => return Err("change must be added, changed or removed".to_owned()),
    };
    let event = match record.get("event") {
        Some(Value::String(event)) if !event.is_empty() => event.clone(),
        _ => return Err("event must be a non-empty string".to_owned()),
    };
    let occurrence = match record.get("occurrence") {
        Some(Value::Null) => String::new(),
        Some(Value::String(value)) => normalize_occurrence(value)
            .ok_or_else(|| "occurrence cannot be read as a date or time".to_owned())?,
        _ => return Err("occurrence must be a string or null".to_owned()),
    };
    let changed_at = record
        .get("changed_at")
        .and_then(Value::as_str)
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .ok_or_else(|| "changed_at must be a time with an offset".to_owned())?;
    check_display_fields(&record)?;
    let schedule = if change == Change::Removed {
        None
    } else {
        Some(parse_schedule(&record)?)
    };
    Ok(CalendarNote {
        change,
        event,
        occurrence,
        changed_at,
        schedule,
        record,
    })
}

fn check_display_fields(record: &Map<String, Value>) -> Result<(), String> {
    for key in TEXT_FIELDS {
        if !matches!(record.get(key), Some(Value::Null | Value::String(_))) {
            return Err(format!("{key} must be a string or null"));
        }
    }
    match record.get("organizer") {
        Some(Value::Null) => {}
        Some(Value::Object(organizer)) => check_person(organizer, "organizer")?,
        _ => return Err("organizer must be an object or null".to_owned()),
    }
    let Some(Value::Array(attendees)) = record.get("attendees") else {
        return Err("attendees must be an array".to_owned());
    };
    for attendee in attendees {
        let Value::Object(attendee) = attendee else {
            return Err("each attendee must be an object".to_owned());
        };
        check_person(attendee, "attendee")?;
        if !matches!(
            attendee.get("status"),
            None | Some(Value::Null | Value::String(_))
        ) {
            return Err("attendee status must be a string or null".to_owned());
        }
        if !matches!(
            attendee.get("you"),
            None | Some(Value::Null | Value::Bool(_))
        ) {
            return Err("attendee you must be true or false".to_owned());
        }
    }
    Ok(())
}

fn check_person(person: &Map<String, Value>, what: &str) -> Result<(), String> {
    for key in ["name", "email"] {
        if !matches!(person.get(key), None | Some(Value::Null | Value::String(_))) {
            return Err(format!("{what} {key} must be a string or null"));
        }
    }
    Ok(())
}

fn parse_schedule(record: &Map<String, Value>) -> Result<Schedule, String> {
    let Some(all_day) = record.get("all_day").and_then(Value::as_bool) else {
        return Err("all_day must be true or false".to_owned());
    };
    let start = match record.get("start") {
        Some(Value::String(text)) => {
            parse_when(text).ok_or_else(|| "start cannot be read as a date or time".to_owned())?
        }
        _ => return Err("start is missing".to_owned()),
    };
    let end = match record.get("end") {
        Some(Value::Null) => None,
        Some(Value::String(text)) => Some(
            parse_when(text).ok_or_else(|| "end cannot be read as a date or time".to_owned())?,
        ),
        _ => return Err("end must be a string or null".to_owned()),
    };
    for when in std::iter::once(&start).chain(end.as_ref()) {
        if all_day != matches!(when, When::Date(_)) {
            return Err("start and end must be dates exactly when all_day is true".to_owned());
        }
    }
    if let Some(end) = end {
        let before = match (start, end) {
            (When::Zoned(start), When::Zoned(end)) => end < start,
            (When::Floating(start), When::Floating(end)) => end < start,
            (When::Date(start), When::Date(end)) => end < start,
            _ => return Err("start and end must both carry an offset or both have none".to_owned()),
        };
        if before {
            return Err("end is before start".to_owned());
        }
    }
    Ok(Schedule {
        all_day,
        start,
        end,
    })
}

fn is_date_shape(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

fn parse_floating(text: &str) -> Option<NaiveDateTime> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 || !is_date_shape(&text[..10]) || bytes[10] != b'T' {
        return None;
    }
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f").ok()
}

fn parse_when(text: &str) -> Option<When> {
    if is_date_shape(text) {
        return NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .ok()
            .map(When::Date);
    }
    if let Ok(zoned) = DateTime::parse_from_rfc3339(text) {
        return Some(When::Zoned(zoned));
    }
    parse_floating(text).map(When::Floating)
}

/// The one spelling of an occurrence, so equal instants compare equal.
fn normalize_occurrence(text: &str) -> Option<String> {
    Some(match parse_when(text)? {
        When::Zoned(instant) => instant
            .naive_utc()
            .format("%Y-%m-%dT%H:%M:%S%.fZ")
            .to_string(),
        When::Floating(wall) => wall.format("%Y-%m-%dT%H:%M:%S%.f").to_string(),
        When::Date(date) => date.format("%Y-%m-%d").to_string(),
    })
}

/// The days an event happens on, in the journal's zone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Listing {
    /// Each day, in order, at most [`MAX_LISTED_DAYS`].
    pub days: Vec<NaiveDate>,
    /// The event goes on past the last listed day.
    pub continues: bool,
    /// The scheduled start's hour on the first day, for a timed event.
    pub start_hour: Option<u32>,
}

/// List the days an event happens on.
///
/// A timed event belongs to each local day it overlaps in `zone`, with its end
/// exclusive; a floating time is already wall-clock time in `zone`. An all-day
/// event belongs to its own dates with no zone shift, end exclusive. A missing
/// end, or one equal to the start, means the start day only.
pub fn listed_days<Tz: TimeZone>(schedule: &Schedule, zone: &Tz) -> Listing {
    let (first, last, start_hour) = match (schedule.start, schedule.end) {
        (When::Date(start), end) => {
            let last = match end {
                Some(When::Date(end)) if end > start => end.pred_opt().unwrap_or(start),
                _ => start,
            };
            (start, last, None)
        }
        (When::Zoned(start), end) => {
            let local_start = start.with_timezone(zone).naive_local();
            let last = match end {
                Some(When::Zoned(end)) if end > start => (end - Duration::nanoseconds(1))
                    .with_timezone(zone)
                    .naive_local()
                    .date(),
                _ => local_start.date(),
            };
            (local_start.date(), last, Some(local_start.hour()))
        }
        (When::Floating(start), end) => {
            let last = match end {
                Some(When::Floating(end)) if end > start => (end - Duration::nanoseconds(1)).date(),
                _ => start.date(),
            };
            (start.date(), last, Some(start.hour()))
        }
    };
    let mut days = Vec::new();
    let mut day = first;
    while day <= last && days.len() < MAX_LISTED_DAYS {
        days.push(day);
        let Some(next) = day.succ_opt() else {
            break;
        };
        day = next;
    }
    let continues = days.last().is_some_and(|listed| *listed < last);
    Listing {
        days,
        continues,
        start_hour,
    }
}

/// Render a note as plain text the owner can read: what changed, the title,
/// when it happens, the calendar, the place, who is invited, the notes, how it
/// repeats and when the change was recorded. Absent fields are left out, and
/// every field is bounded so the text stays readable as one search entry.
pub fn render_calendar_note(note: &CalendarNote) -> String {
    let record = &note.record;
    let mut lines = Vec::new();
    let heading = match note.change {
        Change::Added => "Calendar event added",
        Change::Changed => "Calendar event changed",
        Change::Removed => "Calendar event removed",
    };
    match text_field(record, "title", MAX_FIELD_CHARS) {
        Some(title) => lines.push(format!("{heading}: {title}")),
        None => lines.push(heading.to_owned()),
    }
    if let Some(schedule) = &note.schedule {
        lines.push(format!(
            "When: {}",
            describe_schedule(schedule, text_field(record, "time_zone", MAX_FIELD_CHARS))
        ));
    }
    if let Some(calendar) = text_field(record, "calendar", MAX_FIELD_CHARS) {
        lines.push(format!("Calendar: {calendar}"));
    }
    if let Some(location) = text_field(record, "location", MAX_FIELD_CHARS) {
        lines.push(format!("Where: {location}"));
    }
    if let Some(invited) = describe_attendees(record) {
        lines.push(format!("Invited: {invited}"));
    }
    if let Some(notes) = text_field(record, "notes", MAX_NOTES_CHARS) {
        lines.push(format!("Notes: {notes}"));
    }
    if let Some(repeats) = text_field(record, "repeats", MAX_FIELD_CHARS) {
        lines.push(format!("Repeats: {repeats}"));
    }
    lines.push(format!(
        "Recorded: {}",
        describe_zoned(note.changed_at, true)
    ));
    let mut text = lines.join("\n");
    if text.len() > MAX_RENDERED_BYTES {
        let mut boundary = MAX_RENDERED_BYTES - '…'.len_utf8();
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        text.truncate(boundary);
        text.push('…');
    }
    text
}

fn text_field(record: &Map<String, Value>, key: &str, limit: usize) -> Option<String> {
    let value = record.get(key)?.as_str()?.trim();
    if value.is_empty() {
        return None;
    }
    Some(bounded(value, limit))
}

fn bounded(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let mut cut: String = value.chars().take(limit).collect();
    cut.push('…');
    cut
}

fn describe_attendees(record: &Map<String, Value>) -> Option<String> {
    let attendees = record.get("attendees")?.as_array()?;
    let mut named = Vec::new();
    for attendee in attendees.iter().filter_map(Value::as_object) {
        let who = text_field(attendee, "name", MAX_FIELD_CHARS)
            .or_else(|| text_field(attendee, "email", MAX_FIELD_CHARS));
        let you = attendee.get("you").and_then(Value::as_bool) == Some(true);
        let who = match (who, you) {
            (Some(who), true) => format!("{who}, you"),
            (Some(who), false) => who,
            (None, true) => "you".to_owned(),
            (None, false) => continue,
        };
        match text_field(attendee, "status", MAX_STATUS_CHARS) {
            Some(status) => named.push(format!("{who} ({status})")),
            None => named.push(who),
        }
    }
    if named.is_empty() {
        return None;
    }
    let more = named.len().saturating_sub(MAX_ATTENDEES);
    named.truncate(MAX_ATTENDEES);
    let mut text = named.join("; ");
    if more > 0 {
        text.push_str(&format!("; and {more} more"));
    }
    Some(text)
}

fn describe_date(date: NaiveDate) -> String {
    format!(
        "{}, {} {}, {}",
        date.format("%A"),
        date.format("%B"),
        date.day(),
        date.year()
    )
}

fn describe_clock(time: NaiveDateTime) -> String {
    let (pm, hour) = time.hour12();
    format!(
        "{hour}:{:02} {}",
        time.minute(),
        if pm { "PM" } else { "AM" }
    )
}

fn describe_wall(time: NaiveDateTime) -> String {
    format!("{}, {}", describe_date(time.date()), describe_clock(time))
}

fn describe_zoned(time: DateTime<FixedOffset>, with_offset: bool) -> String {
    let wall = describe_wall(time.naive_local());
    if with_offset {
        format!("{wall} ({})", time.format("%:z"))
    } else {
        wall
    }
}

fn describe_schedule(schedule: &Schedule, zone: Option<String>) -> String {
    let span = match (schedule.start, schedule.end) {
        (When::Date(start), end) => {
            let last = match end {
                Some(When::Date(end)) if end > start => end.pred_opt().unwrap_or(start),
                _ => start,
            };
            if last == start {
                format!("{}, all day", describe_date(start))
            } else {
                format!(
                    "{} to {}, all day",
                    describe_date(start),
                    describe_date(last)
                )
            }
        }
        (When::Zoned(start), Some(When::Zoned(end))) if end > start => {
            let (start, end) = (start.naive_local(), end.naive_local());
            span_text(start, end)
        }
        (When::Zoned(start), _) => describe_wall(start.naive_local()),
        (When::Floating(start), Some(When::Floating(end))) if end > start => span_text(start, end),
        (When::Floating(start), _) => describe_wall(start),
    };
    match zone {
        Some(zone) if !schedule.all_day => format!("{span} ({zone})"),
        _ => span,
    }
}

fn span_text(start: NaiveDateTime, end: NaiveDateTime) -> String {
    if start.date() == end.date() {
        format!("{} to {}", describe_wall(start), describe_clock(end))
    } else {
        format!("{} to {}", describe_wall(start), describe_wall(end))
    }
}

#[cfg(test)]
#[path = "calendar_tests.rs"]
mod tests;
