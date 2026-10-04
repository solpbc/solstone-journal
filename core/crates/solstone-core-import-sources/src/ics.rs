// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only ICS calendar source parsing.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::Path;

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use icalendar::{Calendar, CalendarDateTime, Component, DatePerhapsTime};
use serde_json::{Map, Value, json};
use solstone_core_import::{ImportPreview, RegistrySource};
use zip::ZipArchive;

use crate::save::{RenderedImport, SegmentFile};
use crate::shared::{day_key, window};

/// A calendar person read from an organizer or attendee property.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarAttendee {
    pub name: String,
    pub email: String,
}

/// A calendar event's read-only source facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarEntry {
    pub title: String,
    pub content: String,
    pub create_ts: DateTime<Utc>,
    pub ts: Option<String>,
    pub end_ts: Option<String>,
    pub duration_minutes: Option<i64>,
    pub location: Option<String>,
    pub attendees: Vec<CalendarAttendee>,
    pub recurrence: Option<String>,
}

/// Failure while reading or decoding calendar source material.
#[derive(Debug)]
pub enum IcsError {
    Read {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    Archive {
        path: std::path::PathBuf,
        source: zip::result::ZipError,
    },
}

impl fmt::Display for IcsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Archive { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl Error for IcsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Archive { source, .. } => Some(source),
        }
    }
}

/// Return whether a path has the reference ICS source shape.
#[must_use]
pub fn detect(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    if has_extension(path, "ics") {
        return true;
    }
    if !has_extension(path, "zip") {
        return false;
    }

    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let Ok(archive) = ZipArchive::new(file) else {
        return false;
    };
    archive
        .file_names()
        .any(|name| name.to_ascii_lowercase().ends_with(".ics"))
}

/// Read source calendars into in-memory event facts without mutating the source.
/// A floating or all-day time, which names no zone, is read in `zone`.
pub fn parse_events(path: &Path, zone: &impl TimeZone) -> Result<Vec<CalendarEntry>, IcsError> {
    Ok(parse_ics_data(extract_ics_data(path)?, zone))
}

/// Aggregate a calendar source into the fixed import preview contract, on the zone's days.
pub fn preview(
    path: &Path,
    zone: &impl TimeZone<Offset: fmt::Display>,
) -> Result<ImportPreview, IcsError> {
    let data = extract_ics_data(path)?;
    if data.is_empty() {
        return Ok(ImportPreview {
            date_range: (String::new(), String::new()),
            item_count: 0,
            entity_count: 0,
            summary: "No ICS data found".to_owned(),
        });
    }
    let entries = parse_ics_data(data, zone);
    if entries.is_empty() {
        return Ok(ImportPreview {
            date_range: (String::new(), String::new()),
            item_count: 0,
            entity_count: 0,
            summary: "No events found in ICS data".to_owned(),
        });
    }

    let mut days = entries
        .iter()
        .map(|entry| day_key(entry.create_ts, zone))
        .collect::<Vec<_>>();
    days.sort_unstable();
    let emails = entries
        .iter()
        .flat_map(|entry| {
            entry
                .attendees
                .iter()
                .map(|attendee| attendee.email.as_str())
        })
        .collect::<HashSet<_>>();
    let item_count = u64::try_from(entries.len()).expect("entry count fits u64");
    let entity_count = u64::try_from(emails.len()).expect("email count fits u64");

    Ok(ImportPreview {
        date_range: (days[0].clone(), days[days.len() - 1].clone()),
        item_count,
        entity_count,
        summary: format!("{item_count} events, {entity_count} unique attendees"),
    })
}

/// The transcript file a calendar segment carries.
pub const TRANSCRIPT_FILE: &str = "event_transcript.md";

/// Render a calendar for saving.
///
/// Each event is placed at the moment it was created or last changed, not when it is
/// scheduled: the journal keeps when something entered the owner's life, and talents
/// read what it is about.
pub fn render(
    path: &Path,
    zone: &impl TimeZone<Offset: fmt::Display>,
) -> Result<RenderedImport, IcsError> {
    let entries = parse_events(path, zone)?;
    let entry_count = u64::try_from(entries.len()).expect("event count fits u64");
    let stream = format!("import.{}", RegistrySource::Ics.name());
    let windows = window(
        entries
            .into_iter()
            .map(|entry| (entry.create_ts, entry))
            .collect(),
        zone,
    );
    let mut files = Vec::with_capacity(windows.len());
    let mut items = Vec::new();
    for window in &windows {
        let mut contents = window
            .items
            .iter()
            .map(|(_, entry)| event_markdown(entry, zone))
            .collect::<Vec<_>>()
            .join("\n\n");
        contents.push('\n');
        files.push(SegmentFile {
            day: window.day.clone(),
            segment: window.segment_key.clone(),
            name: TRANSCRIPT_FILE,
            contents,
            units: window.items.len() as u64,
        });
        for (_, entry) in &window.items {
            items.push(event_item(
                items.len(),
                entry,
                &window.day,
                &window.segment_key,
                &stream,
                zone,
            ));
        }
    }
    let days = windows
        .iter()
        .map(|window| window.day.as_str())
        .collect::<HashSet<_>>()
        .len();
    Ok(RenderedImport {
        source: RegistrySource::Ics,
        files,
        items,
        entries: entry_count,
        summary: crate::save::ics_summary(entry_count, days),
    })
}

fn event_markdown(entry: &CalendarEntry, zone: &impl TimeZone<Offset: fmt::Display>) -> String {
    let mut lines = vec![format!("## {}", entry.title)];
    if let Some(days) = all_day_span(entry) {
        let start = entry
            .ts
            .as_deref()
            .unwrap_or_default()
            .get(..10)
            .unwrap_or_default();
        lines.push(if days == 1 {
            format!("**{start}** (all day)")
        } else {
            format!("**{start}** ({days} days)")
        });
    } else if let Some(start) = entry.ts.as_deref().and_then(|ts| wall_time(ts, zone)) {
        let mut when = start.format("%Y-%m-%d %I:%M %p").to_string();
        if let Some(end) = entry.end_ts.as_deref().and_then(|ts| wall_time(ts, zone)) {
            when.push_str(&format!(" – {}", end.format("%I:%M %p")));
        }
        let mut line = format!("**{when}**");
        if let Some(minutes) = entry.duration_minutes {
            line.push_str(&format!(" ({minutes} min)"));
        }
        lines.push(line);
    }
    if let Some(recurrence) = &entry.recurrence {
        lines.push(format!("Repeats: {recurrence}"));
    }
    if let Some(location) = &entry.location {
        lines.push(format!("Where: {location}"));
    }
    let names = attendee_names(entry);
    if !names.is_empty() {
        lines.push(format!("With: {}", names.join(", ")));
    }
    if !entry.content.is_empty() {
        lines.push(String::new());
        lines.push(entry.content.clone());
    }
    lines.join("\n")
}

fn event_item(
    index: usize,
    entry: &CalendarEntry,
    day: &str,
    segment: &str,
    stream: &str,
    zone: &impl TimeZone<Offset: fmt::Display>,
) -> Value {
    let mut meta = Map::new();
    let start = entry.ts.as_deref().and_then(|ts| wall_time(ts, zone));
    let end = entry.end_ts.as_deref().and_then(|ts| wall_time(ts, zone));
    let time_range = start
        .zip(end)
        .filter(|_| all_day_span(entry).is_none())
        .map(|(start, end)| {
            format!(
                "{}–{}",
                clock(&start.format("%I:%M %p").to_string()),
                clock(&end.format("%I:%M %p").to_string())
            )
        });
    if let Some(range) = &time_range {
        meta.insert("time_range".to_owned(), json!(range));
    }
    if let Some(location) = &entry.location {
        meta.insert("location".to_owned(), json!(location));
    }
    if let Some(minutes) = entry.duration_minutes {
        meta.insert("duration_minutes".to_owned(), json!(minutes));
    }
    let names = attendee_names(entry);
    if !names.is_empty() {
        meta.insert("attendee_count".to_owned(), json!(names.len()));
        meta.insert(
            "attendee_names".to_owned(),
            json!(names.iter().take(5).collect::<Vec<_>>()),
        );
    }
    if let Some(recurrence) = &entry.recurrence {
        meta.insert("recurrence".to_owned(), json!(recurrence));
    }
    let description = entry.content.trim();
    let preview = if description.is_empty() {
        let mut parts = Vec::new();
        parts.extend(time_range);
        parts.extend(all_day_span(entry).map(|days| match days {
            1 => "all day".to_owned(),
            days => format!("{days} days"),
        }));
        parts.extend(entry.location.clone());
        if !names.is_empty() {
            parts.push(names.iter().take(5).cloned().collect::<Vec<_>>().join(", "));
        }
        parts.extend(entry.recurrence.clone());
        parts.join(" · ")
    } else {
        description.to_owned()
    };
    json!({
        "id": format!("event-{index}"),
        "title": entry.title,
        "date": day,
        "type": "event",
        "preview": preview.chars().take(200).collect::<String>(),
        "meta": meta,
        "segments": [{ "day": day, "key": segment, "stream": stream }],
    })
}

fn attendee_names(entry: &CalendarEntry) -> Vec<String> {
    entry
        .attendees
        .iter()
        .map(|attendee| {
            if attendee.name.is_empty() {
                attendee.email.clone()
            } else {
                attendee.name.clone()
            }
        })
        .collect()
}

/// How many days an all-day event spans: floating midnight to midnight, whole days apart.
fn all_day_span(entry: &CalendarEntry) -> Option<i64> {
    let floating_midnight = |value: Option<&str>| {
        value.is_some_and(|value| value.len() == 19 && value.ends_with("T00:00:00"))
    };
    let minutes = entry.duration_minutes?;
    (floating_midnight(entry.ts.as_deref())
        && floating_midnight(entry.end_ts.as_deref())
        && minutes > 0
        && minutes % (24 * 60) == 0)
        .then_some(minutes / (24 * 60))
}

/// An event time as the owner would read it: a zoned time in the owner's zone, a floating
/// or all-day time as written.
fn wall_time(value: &str, zone: &impl TimeZone<Offset: fmt::Display>) -> Option<NaiveDateTime> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(zone).naive_local())
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S").ok())
}

fn clock(value: &str) -> &str {
    value.strip_prefix('0').unwrap_or(value)
}

fn parse_ics_data(data: Vec<Vec<u8>>, zone: &impl TimeZone) -> Vec<CalendarEntry> {
    let mut entries = Vec::new();
    for data in data {
        // Python treats an unreadable calendar blob as a zero-event calendar, so one malformed
        // member cannot prevent previewing the other members of an archive.
        let Ok(contents) = String::from_utf8(data) else {
            continue;
        };
        let Ok(calendar) = contents.parse::<Calendar>() else {
            continue;
        };
        entries.extend(
            calendar
                .events()
                .filter_map(|event| parse_event(event, zone)),
        );
    }
    entries
}

fn extract_ics_data(path: &Path) -> Result<Vec<Vec<u8>>, IcsError> {
    if has_extension(path, "ics") {
        return fs::read(path)
            .map(|data| vec![data])
            .map_err(|source| IcsError::Read {
                path: path.to_path_buf(),
                source,
            });
    }
    if !has_extension(path, "zip") {
        return Ok(Vec::new());
    }

    let file = fs::File::open(path).map_err(|source| IcsError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file).map_err(|source| IcsError::Archive {
        path: path.to_path_buf(),
        source,
    })?;
    let mut data = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|source| IcsError::Archive {
                path: path.to_path_buf(),
                source,
            })?;
        if !entry.name().to_ascii_lowercase().ends_with(".ics") {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|source| IcsError::Read {
                path: path.to_path_buf(),
                source,
            })?;
        data.push(bytes);
    }
    Ok(data)
}

fn parse_event(event: &icalendar::Event, zone: &impl TimeZone) -> Option<CalendarEntry> {
    let create_ts = creation_timestamp(event, zone)?;
    let start = event.get_start();
    let end = event.get_end();
    let mut attendees = Vec::new();
    let mut seen_emails = HashSet::new();

    if let Some(organizer) = event.properties().get("ORGANIZER").and_then(parse_attendee) {
        push_attendee(&mut attendees, &mut seen_emails, organizer);
    }
    if let Some(raw_attendees) = event.multi_properties().get("ATTENDEE") {
        for attendee in raw_attendees.iter().filter_map(parse_attendee) {
            push_attendee(&mut attendees, &mut seen_emails, attendee);
        }
    }

    Some(CalendarEntry {
        title: nonempty(event.property_value("SUMMARY"))
            .unwrap_or("Untitled event")
            .to_owned(),
        content: event
            .property_value("DESCRIPTION")
            .unwrap_or_default()
            .to_owned(),
        create_ts,
        ts: start.as_ref().and_then(date_perhaps_time_iso),
        end_ts: end.as_ref().and_then(date_perhaps_time_iso),
        duration_minutes: start
            .as_ref()
            .zip(end.as_ref())
            .and_then(|(start, end)| duration_minutes(start, end, zone)),
        location: nonempty(event.property_value("LOCATION")).map(str::to_owned),
        attendees,
        recurrence: event.property_value("RRULE").and_then(describe_rrule),
    })
}

fn creation_timestamp(event: &icalendar::Event, zone: &impl TimeZone) -> Option<DateTime<Utc>> {
    ["LAST-MODIFIED", "CREATED"]
        .into_iter()
        .find_map(|field| {
            event
                .properties()
                .get(field)
                .and_then(DatePerhapsTime::from_property)
                .as_ref()
                .and_then(|value| date_perhaps_time_utc(value, zone))
        })
        .or_else(|| {
            event
                .get_start()
                .as_ref()
                .and_then(|value| date_perhaps_time_utc(value, zone))
        })
}

/// The instant of a calendar time. A time that names its own zone keeps it; a
/// floating time or an all-day date names none, so it is read in `zone`.
fn date_perhaps_time_utc(value: &DatePerhapsTime, zone: &impl TimeZone) -> Option<DateTime<Utc>> {
    match value {
        DatePerhapsTime::Date(date) => in_zone(date.and_hms_opt(0, 0, 0)?, zone),
        DatePerhapsTime::DateTime(CalendarDateTime::Floating(date_time)) => {
            in_zone(*date_time, zone)
        }
        DatePerhapsTime::DateTime(date_time) => date_time.try_into_utc(),
    }
}

/// A wall time in `zone`; one the clocks skip reads an hour on, as it would on
/// a clock that sprang forward.
fn in_zone(wall: NaiveDateTime, zone: &impl TimeZone) -> Option<DateTime<Utc>> {
    zone.from_local_datetime(&wall)
        .earliest()
        .or_else(|| {
            zone.from_local_datetime(&(wall + chrono::Duration::hours(1)))
                .earliest()
        })
        .map(|instant| instant.with_timezone(&Utc))
}

fn date_perhaps_time_iso(value: &DatePerhapsTime) -> Option<String> {
    match value {
        DatePerhapsTime::Date(date) => date
            .and_hms_opt(0, 0, 0)
            .map(|date| date.format("%Y-%m-%dT%H:%M:%S").to_string()),
        DatePerhapsTime::DateTime(CalendarDateTime::Floating(date_time)) => {
            Some(date_time.format("%Y-%m-%dT%H:%M:%S").to_string())
        }
        DatePerhapsTime::DateTime(CalendarDateTime::Utc(date_time)) => Some(date_time.to_rfc3339()),
        DatePerhapsTime::DateTime(date_time @ CalendarDateTime::WithTimezone { .. }) => {
            date_time.try_into_utc()?;
            date_time
                .clone()
                .as_dt_with_tz()
                .map(|date_time| date_time.to_rfc3339())
        }
    }
}

fn duration_minutes(
    start: &DatePerhapsTime,
    end: &DatePerhapsTime,
    zone: &impl TimeZone,
) -> Option<i64> {
    let duration = if has_offset(start) != has_offset(end) {
        naive_wall_time(end).signed_duration_since(naive_wall_time(start))
    } else {
        date_perhaps_time_utc(end, zone)?.signed_duration_since(date_perhaps_time_utc(start, zone)?)
    };
    Some((duration.num_seconds() / 60).max(0))
}

fn naive_wall_time(value: &DatePerhapsTime) -> NaiveDateTime {
    match value {
        DatePerhapsTime::Date(date) => date.and_hms_opt(0, 0, 0).expect("midnight is valid"),
        DatePerhapsTime::DateTime(CalendarDateTime::Floating(date_time)) => *date_time,
        DatePerhapsTime::DateTime(CalendarDateTime::Utc(date_time)) => date_time.naive_utc(),
        DatePerhapsTime::DateTime(CalendarDateTime::WithTimezone { date_time, .. }) => *date_time,
    }
}

fn has_offset(value: &DatePerhapsTime) -> bool {
    matches!(
        value,
        DatePerhapsTime::DateTime(CalendarDateTime::Utc(_) | CalendarDateTime::WithTimezone { .. })
    )
}

fn parse_attendee(property: &icalendar::Property) -> Option<CalendarAttendee> {
    let email = property
        .value()
        .trim()
        .strip_prefix("mailto:")
        .or_else(|| property.value().trim().strip_prefix("MAILTO:"))
        .unwrap_or(property.value().trim())
        .trim();
    if email.is_empty() || !email.contains('@') {
        return None;
    }
    let name = property
        .params()
        .get("CN")
        .map_or("", icalendar::Parameter::value)
        .trim()
        .to_owned();
    Some(CalendarAttendee {
        name,
        email: email.to_ascii_lowercase(),
    })
}

fn push_attendee(
    attendees: &mut Vec<CalendarAttendee>,
    seen_emails: &mut HashSet<String>,
    attendee: CalendarAttendee,
) {
    if seen_emails.insert(attendee.email.clone()) {
        attendees.push(attendee);
    }
}

fn describe_rrule(value: &str) -> Option<String> {
    let fields = value
        .split(';')
        .filter_map(|part| part.split_once('='))
        .collect::<std::collections::HashMap<_, _>>();
    let frequency = fields.get("FREQ")?;
    let (plural, adjective) = match *frequency {
        "DAILY" => ("days", "Daily"),
        "WEEKLY" => ("weeks", "Weekly"),
        "MONTHLY" => ("months", "Monthly"),
        "YEARLY" => ("years", "Yearly"),
        _ => return None,
    };
    let interval = fields
        .get("INTERVAL")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1);
    let mut description = if interval == 1 {
        adjective.to_owned()
    } else {
        format!("Every {interval} {plural}")
    };
    if let Some(days) = fields.get("BYDAY") {
        let names = days
            .split(',')
            .map(|day| {
                match day.trim_matches(|character: char| {
                    character.is_ascii_digit() || character == '+' || character == '-'
                }) {
                    "MO" => "Mon",
                    "TU" => "Tue",
                    "WE" => "Wed",
                    "TH" => "Thu",
                    "FR" => "Fri",
                    "SA" => "Sat",
                    "SU" => "Sun",
                    other => other,
                }
            })
            .collect::<Vec<_>>();
        description.push_str(&format!(" on {}", names.join(", ")));
    }
    if let Some(days) = fields.get("BYMONTHDAY") {
        description.push_str(&format!(" on day {days}"));
    }
    if let Some(months) = fields.get("BYMONTH") {
        let names = months
            .split(',')
            .map(|month| match month {
                "1" => "Jan",
                "2" => "Feb",
                "3" => "Mar",
                "4" => "Apr",
                "5" => "May",
                "6" => "Jun",
                "7" => "Jul",
                "8" => "Aug",
                "9" => "Sep",
                "10" => "Oct",
                "11" => "Nov",
                "12" => "Dec",
                other => other,
            })
            .collect::<Vec<_>>();
        description.push_str(&format!(" in {}", names.join(", ")));
    }
    if let Some(count) = fields.get("COUNT") {
        description.push_str(&format!(", {count} times"));
    }
    if let Some(until) = fields.get("UNTIL").and_then(|until| parse_until(until)) {
        description.push_str(&format!(", until {}", until.format("%Y-%m-%d")));
    }
    Some(description)
}

fn parse_until(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y%m%d").ok().or_else(|| {
        NaiveDateTime::parse_from_str(value.trim_end_matches('Z'), "%Y%m%dT%H%M%S")
            .ok()
            .map(|value| value.date())
    })
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

fn has_extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}
