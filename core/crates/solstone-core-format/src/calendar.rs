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

use jiff::civil::{Date, DateTime};
use jiff::tz::{Offset, TimeZone};
use jiff::{SignedDuration, ZonedDateTime};
use serde_json::{Map, Value};
use solstone_core_time::{parse_day, parse_rfc3339, strftime_civil};

use solstone_core_window_catalog::{ENGLISH, Value as CatalogValue, render, strings, text};

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
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum When {
    /// A time in a zone, carrying its offset at that instant.
    Zoned(ZonedDateTime),
    /// A wall-clock time with no zone.
    Floating(DateTime),
    /// A date, for an all-day event.
    Date(Date),
}

/// When an added or changed event happens.
#[derive(Clone, Debug, Eq, PartialEq)]
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
    pub changed_at: ZonedDateTime,
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
        self.changed_at.timestamp().as_millisecond()
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
    pub reason: solstone_core_cli_copy::MessageDetail,
}

impl fmt::Display for CalendarFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(formatter, "line {line}: {}", self.reason),
            None => write!(formatter, "{}", self.reason),
        }
    }
}

impl CalendarFileError {
    pub fn into_detail(self) -> solstone_core_cli_copy::MessageDetail {
        match self.line {
            Some(line) => self.reason.context_slot(
                "journal.cli.journal.indexer.calendar.at",
                "reason",
                [("line", &line)],
            ),
            None => self.reason,
        }
    }
}

impl std::error::Error for CalendarFileError {}

fn whole_file(reason: solstone_core_cli_copy::MessageDetail) -> CalendarFileError {
    CalendarFileError { line: None, reason }
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
        whole_file(solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.calendar.validation.segment.grid",
            [],
        ))
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
    let window = segment_span(day, segment).ok_or_else(|| {
        whole_file(solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.calendar.validation.segment.must.name.a.span.on.its.day",
            [],
        ))
    })?;
    validate_lines(bytes, window, |note| {
        import_stamp_detail(&note.record)?;
        match note.record.get("expanded") {
            None | Some(Value::Bool(true)) => Ok(()),
            Some(_) => Err(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.expanded.must.be.true.when.present",
                [],
            )),
        }
    })
}

/// The import a line written by the importer came from, and when that import
/// happened.
pub fn import_stamp(record: &Map<String, Value>) -> Result<(String, ZonedDateTime), String> {
    import_stamp_detail(record).map_err(|detail| detail.to_string())
}

pub fn import_stamp_detail(
    record: &Map<String, Value>,
) -> Result<(String, ZonedDateTime), solstone_core_cli_copy::MessageDetail> {
    let import = match record.get("import") {
        Some(Value::String(import)) if !import.is_empty() => import.clone(),
        _ => {
            return Err(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.import.must.be.a.non.empty.string",
                [],
            ));
        }
    };
    let imported_at = record
        .get("imported_at")
        .and_then(Value::as_str)
        .and_then(|text| parse_rfc3339(text).ok())
        .ok_or_else(|| solstone_core_cli_copy::MessageDetail::message("journal.cli.journal.indexer.calendar.validation.imported.at.must.be.a.time.with.an.offset", []))?;
    Ok((import, imported_at))
}

fn check_file_name(name: &str) -> Result<(), CalendarFileError> {
    if is_calendar_file_name(name) {
        Ok(())
    } else {
        Err(whole_file(solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.calendar.validation.filename",
            [],
        )))
    }
}

fn validate_lines(
    bytes: &[u8],
    (window_start, window_end): (DateTime, DateTime),
    extra: impl Fn(&CalendarNote) -> Result<(), solstone_core_cli_copy::MessageDetail>,
) -> Result<Vec<CalendarNote>, CalendarFileError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(whole_file(solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.calendar.validation.file.size",
            [],
        )));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| {
        whole_file(solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.calendar.validation.file.encoding",
            [],
        ))
    })?;
    if text.lines().count() > MAX_FILE_LINES {
        return Err(whole_file(solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.calendar.validation.file.lines",
            [],
        )));
    }
    let mut notes = Vec::new();
    let mut identities = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        let at = |reason: solstone_core_cli_copy::MessageDetail| CalendarFileError {
            line: Some(index + 1),
            reason,
        };
        if line.trim().is_empty() {
            return Err(at(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.line.is.empty",
                [],
            )));
        }
        let note = parse_calendar_note_detail(line).map_err(at)?;
        extra(&note).map_err(at)?;
        if !identities.insert((note.event.clone(), note.occurrence.clone())) {
            return Err(at(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.event.and.occurrence.repeat.an.earlier.line",
                [],
            )));
        }
        let local = note.changed_at.datetime();
        if local < window_start || local >= window_end {
            return Err(at(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.changed.at.falls.outside.the.segment.window",
                [],
            )));
        }
        notes.push(note);
    }
    if notes.is_empty() {
        return Err(whole_file(solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.calendar.validation.file.holds.no.notes",
            [],
        )));
    }
    Ok(notes)
}

/// Read the notes of a stored file for indexing, with each note's line index.
///
/// Stored files were checked on arrival, so this reads them the same way but
/// keeps going past a line it cannot use, reporting it as a warning. A later
/// line repeating an earlier identity is skipped the same way.
pub fn read_calendar_notes(
    text: &str,
) -> (
    Vec<(usize, CalendarNote)>,
    Vec<solstone_core_cli_copy::MessageDetail>,
) {
    let mut notes = Vec::new();
    let mut warnings = Vec::new();
    let mut identities = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match parse_calendar_note_detail(line) {
            Ok(note) => {
                if identities.insert((note.event.clone(), note.occurrence.clone())) {
                    notes.push((index, note));
                } else {
                    warnings.push(solstone_core_cli_copy::MessageDetail::message(
                        "journal.cli.journal.indexer.calendar.line.repeat",
                        [("line", &(index + 1))],
                    ));
                }
            }
            Err(reason) => {
                warnings.push(reason.context_slot(
                    "journal.cli.journal.indexer.calendar.line.skip",
                    "reason",
                    [("line", &(index + 1))],
                ));
            }
        }
    }
    (notes, warnings)
}

/// The window a segment key names on its day, start inclusive and end
/// exclusive. Only an `HHMMSS_300` key starting on the 300-second grid names
/// one.
pub(crate) fn segment_window(day: &str, segment: &str) -> Option<(DateTime, DateTime)> {
    if !is_date_key(day) {
        return None;
    }
    let date = parse_day(day)?;
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
    let start = date.to_datetime(times.clock()?);
    let end = start
        .checked_add(SignedDuration::from_secs(i64::from(WINDOW_SECONDS)))
        .ok()?;
    Some((start, end))
}

/// The span any segment key names on its day: its start, and its start plus
/// its length.
pub fn segment_span(day: &str, segment: &str) -> Option<(DateTime, DateTime)> {
    if !is_date_key(day) {
        return None;
    }
    let date = parse_day(day)?;
    let times = segment_parse(segment)?;
    let (clock, length) = segment.split_once('_')?;
    if clock.len() != 6 {
        return None;
    }
    let length: i64 = length.parse().ok().filter(|length| *length > 0)?;
    let start = date.to_datetime(times.clock()?);
    let end = start.checked_add(SignedDuration::from_secs(length)).ok()?;
    Some((start, end))
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
    parse_calendar_note_detail(line).map_err(|detail| detail.to_string())
}

pub fn parse_calendar_note_detail(
    line: &str,
) -> Result<CalendarNote, solstone_core_cli_copy::MessageDetail> {
    let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line) else {
        return Err(solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.calendar.validation.line.is.not.a.json.object",
            [],
        ));
    };
    calendar_note_from_record_detail(record)
}

/// Check one already-parsed note line against the contract.
pub fn calendar_note_from_record(record: Map<String, Value>) -> Result<CalendarNote, String> {
    calendar_note_from_record_detail(record).map_err(|detail| detail.to_string())
}

pub fn calendar_note_from_record_detail(
    record: Map<String, Value>,
) -> Result<CalendarNote, solstone_core_cli_copy::MessageDetail> {
    for key in REQUIRED_KEYS {
        if !record.contains_key(key) {
            return Err(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.field.missing",
                [("key", &(key))],
            ));
        }
    }
    if record.get("v").and_then(Value::as_u64) != Some(1) {
        return Err(solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.calendar.validation.version",
            [],
        ));
    }
    let change = match record.get("change").and_then(Value::as_str) {
        Some("added") => Change::Added,
        Some("changed") => Change::Changed,
        Some("removed") => Change::Removed,
        _ => {
            return Err(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.change.must.be.added.changed.or.removed",
                [],
            ));
        }
    };
    let event = match record.get("event") {
        Some(Value::String(event)) if !event.is_empty() => event.clone(),
        _ => {
            return Err(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.event.must.be.a.non.empty.string",
                [],
            ));
        }
    };
    let occurrence = match record.get("occurrence") {
        Some(Value::Null) => String::new(),
        Some(Value::String(value)) => normalize_occurrence(value)
            .ok_or_else(|| solstone_core_cli_copy::MessageDetail::message("journal.cli.journal.indexer.calendar.validation.occurrence.cannot.be.read.as.a.date.or.time", []))?,
        _ => return Err(solstone_core_cli_copy::MessageDetail::message("journal.cli.journal.indexer.calendar.validation.occurrence.must.be.a.string.or.null", [])),
    };
    let changed_at = record
        .get("changed_at")
        .and_then(Value::as_str)
        .and_then(|text| parse_rfc3339(text).ok())
        .ok_or_else(|| solstone_core_cli_copy::MessageDetail::message("journal.cli.journal.indexer.calendar.validation.changed.at.must.be.a.time.with.an.offset", []))?;
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

fn check_display_fields(
    record: &Map<String, Value>,
) -> Result<(), solstone_core_cli_copy::MessageDetail> {
    for key in TEXT_FIELDS {
        if !matches!(record.get(key), Some(Value::Null | Value::String(_))) {
            return Err(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.field.type",
                [("key", &(key))],
            ));
        }
    }
    // A phone that predates the account sends none.
    if !matches!(
        record.get("account"),
        None | Some(Value::Null | Value::String(_))
    ) {
        return Err(solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.calendar.validation.account.must.be.a.string.or.null",
            [],
        ));
    }
    match record.get("organizer") {
        Some(Value::Null) => {}
        Some(Value::Object(organizer)) => check_person(organizer, "organizer")?,
        _ => {
            return Err(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.organizer.must.be.an.object.or.null",
                [],
            ));
        }
    }
    let Some(Value::Array(attendees)) = record.get("attendees") else {
        return Err(solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.calendar.validation.attendees.must.be.an.array",
            [],
        ));
    };
    for attendee in attendees {
        let Value::Object(attendee) = attendee else {
            return Err(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.each.attendee.must.be.an.object",
                [],
            ));
        };
        check_person(attendee, "attendee")?;
        if !matches!(
            attendee.get("status"),
            None | Some(Value::Null | Value::String(_))
        ) {
            return Err(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.attendee.status.must.be.a.string.or.null",
                [],
            ));
        }
        if !matches!(
            attendee.get("you"),
            None | Some(Value::Null | Value::Bool(_))
        ) {
            return Err(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.attendee.you.must.be.true.or.false",
                [],
            ));
        }
    }
    Ok(())
}

fn check_person(
    person: &Map<String, Value>,
    what: &str,
) -> Result<(), solstone_core_cli_copy::MessageDetail> {
    for key in ["name", "email"] {
        if !matches!(person.get(key), None | Some(Value::Null | Value::String(_))) {
            return Err(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.person.field",
                [("what", &(what)), ("key", &(key))],
            ));
        }
    }
    Ok(())
}

fn parse_schedule(
    record: &Map<String, Value>,
) -> Result<Schedule, solstone_core_cli_copy::MessageDetail> {
    let Some(all_day) = record.get("all_day").and_then(Value::as_bool) else {
        return Err(solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.calendar.validation.all.day.must.be.true.or.false",
            [],
        ));
    };
    let start = match record.get("start") {
        Some(Value::String(text)) => {
            parse_when(text).ok_or_else(|| solstone_core_cli_copy::MessageDetail::message("journal.cli.journal.indexer.calendar.validation.start.cannot.be.read.as.a.date.or.time", []))?
        }
        _ => return Err(solstone_core_cli_copy::MessageDetail::message("journal.cli.journal.indexer.calendar.validation.start.is.missing", [])),
    };
    let end = match record.get("end") {
        Some(Value::Null) => None,
        Some(Value::String(text)) => Some(
            parse_when(text).ok_or_else(|| solstone_core_cli_copy::MessageDetail::message("journal.cli.journal.indexer.calendar.validation.end.cannot.be.read.as.a.date.or.time", []))?,
        ),
        _ => return Err(solstone_core_cli_copy::MessageDetail::message("journal.cli.journal.indexer.calendar.validation.end.must.be.a.string.or.null", [])),
    };
    for when in std::iter::once(&start).chain(end.as_ref()) {
        if all_day != matches!(when, When::Date(_)) {
            return Err(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.start.and.end.must.be.dates.exactly.when.all.day.is.true",
                [],
            ));
        }
    }
    if let Some(end) = &end {
        let before = match (&start, end) {
            (When::Zoned(start), When::Zoned(end)) => end < start,
            (When::Floating(start), When::Floating(end)) => end < start,
            (When::Date(start), When::Date(end)) => end < start,
            _ => {
                return Err(solstone_core_cli_copy::MessageDetail::message(
                    "journal.cli.journal.indexer.calendar.validation.start.and.end.must.both.carry.an.offset.or.both.have.none",
                    [],
                ));
            }
        };
        if before {
            return Err(solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.calendar.validation.end.is.before.start",
                [],
            ));
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

fn parse_floating(text: &str) -> Option<DateTime> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 || !is_date_shape(&text[..10]) || bytes[10] != b'T' {
        return None;
    }
    DateTime::strptime("%Y-%m-%dT%H:%M:%S%.f", text).ok()
}

fn parse_when(text: &str) -> Option<When> {
    if is_date_shape(text) {
        return Date::strptime("%Y-%m-%d", text).ok().map(When::Date);
    }
    if let Ok(zoned) = parse_rfc3339(text) {
        return Some(When::Zoned(zoned));
    }
    parse_floating(text).map(When::Floating)
}

/// The one spelling of an occurrence, so equal instants compare equal.
fn normalize_occurrence(text: &str) -> Option<String> {
    Some(match parse_when(text)? {
        When::Zoned(instant) => strftime_civil(
            Offset::UTC.to_datetime(instant.timestamp()),
            "%Y-%m-%dT%H:%M:%S%.fZ",
        ),
        When::Floating(wall) => strftime_civil(wall, "%Y-%m-%dT%H:%M:%S%.f"),
        When::Date(date) => date.strftime("%Y-%m-%d").to_string(),
    })
}

/// The days an event happens on, in the journal's zone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Listing {
    /// Each day, in order, at most [`MAX_LISTED_DAYS`].
    pub days: Vec<Date>,
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
pub fn listed_days(schedule: &Schedule, zone: &TimeZone) -> Listing {
    let hour = |wall: DateTime| Some(u32::from(wall.hour().unsigned_abs()));
    let (first, last, start_hour) = match (&schedule.start, &schedule.end) {
        (When::Date(start), end) => {
            let start = *start;
            let last = match end {
                Some(When::Date(end)) if *end > start => end.yesterday().unwrap_or(start),
                _ => start,
            };
            (start, last, None)
        }
        (When::Zoned(start), end) => {
            let local_start = zone.to_datetime(start.timestamp());
            let last = match end {
                Some(When::Zoned(end)) if end > start => end
                    .timestamp()
                    .checked_sub(SignedDuration::from_nanos(1))
                    .map_or(local_start.date(), |last| zone.to_datetime(last).date()),
                _ => local_start.date(),
            };
            (local_start.date(), last, hour(local_start))
        }
        (When::Floating(start), end) => {
            let start = *start;
            let last = match end {
                Some(When::Floating(end)) if *end > start => end
                    .checked_sub(SignedDuration::from_nanos(1))
                    .map_or(start.date(), DateTime::date),
                _ => start.date(),
            };
            (start.date(), last, hour(start))
        }
    };
    let mut days = Vec::new();
    let mut day = first;
    while day <= last && days.len() < MAX_LISTED_DAYS {
        days.push(day);
        let Ok(next) = day.tomorrow() else {
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

/// The owner's own email addresses, from their identity in the journal
/// settings, compared trimmed and ignoring case. An attendee with an email is
/// the owner only when it is one of these. A phone's own `you` mark is never read: on
/// a calendar shared to the phone, it marks the calendar's owner instead.
///
/// A phone can send an invite with no addresses on it, as iOS does once an
/// invitation is answered. For one event's invites, [`Self::carried_from`]
/// also knows the names that carried one of the owner's addresses on any of
/// its notes, so the owner is still found where their address went missing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnerAddresses {
    addresses: BTreeSet<String>,
    names: BTreeSet<String>,
}

impl OwnerAddresses {
    /// No addresses: nobody on an invite is the owner.
    pub const NONE: Self = Self {
        addresses: BTreeSet::new(),
        names: BTreeSet::new(),
    };

    pub fn new<I, S>(addresses: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            addresses: addresses
                .into_iter()
                .map(|address| folded(address.as_ref()))
                .filter(|address| !address.is_empty())
                .collect(),
            names: BTreeSet::new(),
        }
    }

    /// The owner on one event's invites, given every note recorded for it.
    /// A name counts as the owner's when it carried one of the owner's
    /// addresses on one of those notes, and never carried any other address,
    /// so two people who share a name are both left unmarked.
    pub fn carried_from<'a, I>(&self, records: I) -> Self
    where
        I: IntoIterator<Item = &'a Map<String, Value>>,
    {
        let mut owner_names = BTreeSet::new();
        let mut other_names = BTreeSet::new();
        for person in records.into_iter().flat_map(people_on_invite) {
            let (Some(name), Some(email)) =
                (person_text(person, "name"), person_text(person, "email"))
            else {
                continue;
            };
            if self.has(&email) {
                owner_names.insert(folded(&name));
            } else {
                other_names.insert(folded(&name));
            }
        }
        Self {
            addresses: self.addresses.clone(),
            names: owner_names.difference(&other_names).cloned().collect(),
        }
    }

    /// Whether `email` is one of the owner's addresses.
    pub fn has(&self, email: &str) -> bool {
        self.addresses.contains(&folded(email))
    }

    /// Whether this person on an invite is the owner: their email is one of
    /// the owner's addresses, or, with no email, their name carried one on
    /// another note of the same event.
    pub fn is_owner(&self, person: &Map<String, Value>) -> bool {
        match person_text(person, "email") {
            Some(email) => self.has(&email),
            None => {
                person_text(person, "name").is_some_and(|name| self.names.contains(&folded(&name)))
            }
        }
    }

    /// The addresses as one stable text, so a change can be noticed.
    pub fn stamp(&self) -> String {
        self.addresses
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn folded(text: &str) -> String {
    text.trim().to_lowercase()
}

fn person_text(person: &Map<String, Value>, key: &str) -> Option<String> {
    let value = person.get(key)?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// The organizer and the attendees of one note.
fn people_on_invite(record: &Map<String, Value>) -> impl Iterator<Item = &Map<String, Value>> {
    let attendees = record
        .get("attendees")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    record
        .get("organizer")
        .into_iter()
        .chain(attendees)
        .filter_map(Value::as_object)
}

/// Render a note as plain text the owner can read: what changed, the title,
/// when it happens, the calendar, the place, who is invited, the notes, how it
/// repeats and when the change was recorded. Absent fields are left out, and
/// every field is bounded so the text stays readable as one search entry. The
/// attendee who is the owner, by `owner`'s addresses, is written as you.
pub fn render_calendar_note(note: &CalendarNote, owner: &OwnerAddresses) -> String {
    calendar_note_for_locale(note, owner, ENGLISH)
}

/// Render the journal's labels in the reader's language, retaining calendar-supplied text.
pub fn calendar_note_for_locale(
    note: &CalendarNote,
    owner: &OwnerAddresses,
    locale: &str,
) -> String {
    let record = &note.record;
    let title = text_field(record, "title", MAX_FIELD_CHARS);
    let heading = match (note.change, title.is_some()) {
        (Change::Added, false) => "journal.window.calendar.note.added",
        (Change::Changed, false) => "journal.window.calendar.note.changed",
        (Change::Removed, false) => "journal.window.calendar.note.removed",
        (Change::Added, true) => "journal.window.calendar.note.added.titled",
        (Change::Changed, true) => "journal.window.calendar.note.changed.titled",
        (Change::Removed, true) => "journal.window.calendar.note.removed.titled",
    };
    let mut lines = vec![match title {
        Some(title) => strings(locale, heading, [("title", &title)]),
        None => text(locale, heading),
    }];
    if let Some(schedule) = &note.schedule {
        lines.push(describe_schedule(
            schedule,
            text_field(record, "time_zone", MAX_FIELD_CHARS),
            locale,
        ));
    }
    if let Some(calendar) = text_field(record, "calendar", MAX_FIELD_CHARS) {
        // Two calendars can share a name across accounts; a calendar named after its account says it once.
        lines.push(match text_field(record, "account", MAX_FIELD_CHARS) {
            Some(account) if !account.eq_ignore_ascii_case(&calendar) => strings(
                locale,
                "journal.window.calendar.note.calendar.account",
                [("calendar", &calendar), ("account", &account)],
            ),
            _ => strings(
                locale,
                "journal.window.calendar.note.calendar",
                [("calendar", &calendar)],
            ),
        });
    }
    if let Some(location) = text_field(record, "location", MAX_FIELD_CHARS) {
        lines.push(strings(
            locale,
            "journal.window.calendar.note.where",
            [("location", &location)],
        ));
    }
    if let Some(invited) = describe_attendees(record, owner, locale) {
        lines.push(invited);
    }
    if let Some(notes) = text_field(record, "notes", MAX_NOTES_CHARS) {
        lines.push(strings(
            locale,
            "journal.window.calendar.note.notes",
            [("notes", &notes)],
        ));
    }
    if let Some(repeats) = text_field(record, "repeats", MAX_FIELD_CHARS) {
        lines.push(strings(
            locale,
            "journal.window.calendar.note.repeats",
            [("repeats", &repeats)],
        ));
    }
    let at = note.changed_at.datetime();
    lines.push(strings(
        locale,
        "journal.window.calendar.note.recorded",
        [
            ("date", &describe_date(at.date(), locale)),
            ("time", &describe_clock(at, locale)),
            ("offset", &note.changed_at.strftime("%:z").to_string()),
        ],
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

fn describe_attendees(
    record: &Map<String, Value>,
    owner: &OwnerAddresses,
    locale: &str,
) -> Option<String> {
    let attendees = record.get("attendees")?.as_array()?;
    let mut named = Vec::new();
    let mut more = 0;
    for attendee in attendees.iter().filter_map(Value::as_object) {
        let who = text_field(attendee, "name", MAX_FIELD_CHARS)
            .or_else(|| text_field(attendee, "email", MAX_FIELD_CHARS));
        let you = owner.is_owner(attendee);
        if named.len() == MAX_ATTENDEES {
            more += i64::from(who.is_some() || you);
            continue;
        }
        let who = match (who, you) {
            (Some(who), true) => strings(
                locale,
                "journal.window.calendar.note.attendee.owner",
                [("name", &who)],
            ),
            (Some(who), false) => who,
            (None, true) => text(locale, "journal.window.search.message.you"),
            (None, false) => continue,
        };
        match text_field(attendee, "status", MAX_STATUS_CHARS) {
            Some(status) => named.push(strings(
                locale,
                "journal.window.calendar.note.attendee.status",
                [("name", &who), ("status", &status)],
            )),
            None => named.push(who),
        }
    }
    let mut named = named.into_iter();
    let first = named.next()?;
    let attendees = named.fold(first, |prior, name| {
        strings(
            locale,
            "journal.window.calendar.note.attendee.pair",
            [("attendees", &prior), ("attendee", &name)],
        )
    });
    Some(if more == 0 {
        strings(
            locale,
            "journal.window.calendar.note.invited",
            [("attendees", &attendees)],
        )
    } else {
        render(
            locale,
            "journal.window.calendar.note.invited.more",
            &[
                ("attendees", CatalogValue::String(&attendees)),
                ("remaining", CatalogValue::Cardinal(more)),
            ],
        )
    })
}

fn describe_date(date: Date, locale: &str) -> String {
    const WEEKDAYS: [&str; 7] = [
        "journal.window.date.weekdays.monday",
        "journal.window.date.weekdays.tuesday",
        "journal.window.date.weekdays.wednesday",
        "journal.window.date.weekdays.thursday",
        "journal.window.date.weekdays.friday",
        "journal.window.date.weekdays.saturday",
        "journal.window.date.weekdays.sunday",
    ];
    const MONTHS: [&str; 12] = [
        "journal.window.shared.date.nav.months.label.january",
        "journal.window.shared.date.nav.months.label.february",
        "journal.window.shared.date.nav.months.label.march",
        "journal.window.shared.date.nav.months.label.april",
        "journal.window.shared.date.nav.months.label.may",
        "journal.window.shared.date.nav.months.label.june",
        "journal.window.shared.date.nav.months.label.july",
        "journal.window.shared.date.nav.months.label.august",
        "journal.window.shared.date.nav.months.label.september",
        "journal.window.shared.date.nav.months.label.october",
        "journal.window.shared.date.nav.months.label.november",
        "journal.window.shared.date.nav.months.label.december",
    ];
    strings(
        locale,
        "journal.window.calendar.note.date",
        [
            (
                "weekday",
                &text(
                    locale,
                    WEEKDAYS[date.weekday().to_monday_zero_offset() as usize],
                ),
            ),
            ("month", &text(locale, MONTHS[(date.month() - 1) as usize])),
            ("day", &date.day().to_string()),
            ("year", &date.year().to_string()),
        ],
    )
}

fn describe_clock(time: DateTime, locale: &str) -> String {
    let hour = match time.hour() % 12 {
        0 => 12,
        hour => hour,
    };
    strings(
        locale,
        if time.hour() >= 12 {
            "journal.window.body.date.clock.afternoon"
        } else {
            "journal.window.body.date.clock.morning"
        },
        [
            ("hour", &hour.to_string()),
            ("minute", &format!("{:02}", time.minute())),
        ],
    )
}

fn describe_schedule(schedule: &Schedule, zone: Option<String>, locale: &str) -> String {
    let (start, end) = match (&schedule.start, &schedule.end) {
        (When::Date(start), end) => {
            let last = match end {
                Some(When::Date(end)) if end > start => end.yesterday().unwrap_or(*start),
                _ => *start,
            };
            let date = describe_date(*start, locale);
            return if last == *start {
                strings(
                    locale,
                    "journal.window.calendar.note.when.all.day",
                    [("date", &date)],
                )
            } else {
                strings(
                    locale,
                    "journal.window.calendar.note.when.all.days",
                    [
                        ("start_date", &date),
                        ("end_date", &describe_date(last, locale)),
                    ],
                )
            };
        }
        (When::Zoned(start), Some(When::Zoned(end))) if end > start => {
            (start.datetime(), Some(end.datetime()))
        }
        (When::Zoned(start), _) => (start.datetime(), None),
        (When::Floating(start), Some(When::Floating(end))) if end > start => (*start, Some(*end)),
        (When::Floating(start), _) => (*start, None),
    };
    let start_date = describe_date(start.date(), locale);
    let start_time = describe_clock(start, locale);
    let end_time;
    let end_date;
    let (plain, zoned, mut fields) = match end {
        None => (
            "journal.window.calendar.note.when.time",
            "journal.window.calendar.note.when.time.zoned",
            vec![("date", start_date.as_str()), ("time", start_time.as_str())],
        ),
        Some(end) if end.date() == start.date() => {
            end_time = describe_clock(end, locale);
            (
                "journal.window.calendar.note.when.same.day",
                "journal.window.calendar.note.when.same.day.zoned",
                vec![
                    ("date", start_date.as_str()),
                    ("start_time", start_time.as_str()),
                    ("end_time", end_time.as_str()),
                ],
            )
        }
        Some(end) => {
            end_date = describe_date(end.date(), locale);
            end_time = describe_clock(end, locale);
            (
                "journal.window.calendar.note.when.across.days",
                "journal.window.calendar.note.when.across.days.zoned",
                vec![
                    ("start_date", start_date.as_str()),
                    ("start_time", start_time.as_str()),
                    ("end_date", end_date.as_str()),
                    ("end_time", end_time.as_str()),
                ],
            )
        }
    };
    let message = match zone.as_deref().filter(|_| !schedule.all_day) {
        Some(zone) => {
            fields.push(("zone", zone));
            zoned
        }
        None => plain,
    };
    render(
        locale,
        message,
        &fields
            .into_iter()
            .map(|(name, value)| (name, CatalogValue::String(value)))
            .collect::<Vec<_>>(),
    )
}

#[cfg(test)]
#[path = "calendar_tests.rs"]
mod tests;
