// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only ICS calendar source parsing.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::Path;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use icalendar::{Calendar, CalendarDateTime, Component, DatePerhapsTime};
use solstone_core_import::ImportPreview;
use zip::ZipArchive;

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
    pub day: String,
    pub ts: Option<String>,
    pub end_ts: Option<String>,
    pub duration_minutes: Option<i64>,
    pub location: Option<String>,
    pub attendees: Vec<CalendarAttendee>,
    pub recurrence: Option<String>,
}

/// A later-writer-ready calendar attendee entity projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarEntity {
    pub day: String,
    pub name: String,
    pub email: String,
    pub entity_type: String,
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
    Decode {
        path: std::path::PathBuf,
        source: std::string::FromUtf8Error,
    },
    Calendar {
        source: String,
    },
}

impl fmt::Display for IcsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Archive { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Decode { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Calendar { source } => write!(formatter, "ICS calendar: {source}"),
        }
    }
}

impl Error for IcsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Archive { source, .. } => Some(source),
            Self::Decode { source, .. } => Some(source),
            Self::Calendar { .. } => None,
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
pub fn parse_events(path: &Path) -> Result<Vec<CalendarEntry>, IcsError> {
    parse_ics_data(extract_ics_data(path)?, path)
}

/// Aggregate a calendar source into the fixed import preview contract.
pub fn preview(path: &Path) -> Result<ImportPreview, IcsError> {
    let data = extract_ics_data(path)?;
    if data.is_empty() {
        return Ok(ImportPreview {
            date_range: (String::new(), String::new()),
            item_count: 0,
            entity_count: 0,
            summary: "No ICS data found".to_owned(),
        });
    }
    let entries = parse_ics_data(data, path)?;
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
        .map(|entry| entry.day.as_str())
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
        date_range: (days[0].to_owned(), days[days.len() - 1].to_owned()),
        item_count,
        entity_count,
        summary: format!("{item_count} events, {entity_count} unique attendees"),
    })
}

fn parse_ics_data(data: Vec<Vec<u8>>, path: &Path) -> Result<Vec<CalendarEntry>, IcsError> {
    let mut entries = Vec::new();
    for data in data {
        let contents = String::from_utf8(data).map_err(|source| IcsError::Decode {
            path: path.to_path_buf(),
            source,
        })?;
        let calendar: Calendar = contents
            .parse()
            .map_err(|source| IcsError::Calendar { source })?;
        entries.extend(calendar.events().filter_map(parse_event));
    }
    Ok(entries)
}

/// Project named calendar attendees to deterministic Person entity facts.
#[must_use]
pub fn attendee_entities(entries: &[CalendarEntry]) -> Vec<CalendarEntity> {
    let mut seen = HashSet::new();
    let mut entities = Vec::new();
    for entry in entries {
        for attendee in &entry.attendees {
            if attendee.name.is_empty()
                || attendee.email.is_empty()
                || !seen.insert(&attendee.email)
            {
                continue;
            }
            entities.push(CalendarEntity {
                day: entry.day.clone(),
                name: attendee.name.clone(),
                email: attendee.email.clone(),
                entity_type: "Person".to_owned(),
            });
        }
    }
    entities
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

fn parse_event(event: &icalendar::Event) -> Option<CalendarEntry> {
    let create_ts = creation_timestamp(event)?;
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
        day: create_ts.format("%Y%m%d").to_string(),
        ts: start.as_ref().and_then(date_perhaps_time_iso),
        end_ts: end.as_ref().and_then(date_perhaps_time_iso),
        duration_minutes: start
            .as_ref()
            .and_then(date_perhaps_time_utc)
            .zip(end.as_ref().and_then(date_perhaps_time_utc))
            .and_then(|(start, end)| duration_minutes(&start, &end)),
        location: nonempty(event.property_value("LOCATION")).map(str::to_owned),
        attendees,
        recurrence: event.property_value("RRULE").and_then(describe_rrule),
    })
}

fn creation_timestamp(event: &icalendar::Event) -> Option<DateTime<Utc>> {
    ["LAST-MODIFIED", "CREATED"]
        .into_iter()
        .find_map(|field| {
            event
                .properties()
                .get(field)
                .and_then(DatePerhapsTime::from_property)
                .as_ref()
                .and_then(date_perhaps_time_utc)
        })
        .or_else(|| event.get_start().as_ref().and_then(date_perhaps_time_utc))
}

fn date_perhaps_time_utc(value: &DatePerhapsTime) -> Option<DateTime<Utc>> {
    match value {
        DatePerhapsTime::Date(date) => date.and_hms_opt(0, 0, 0).map(|date| date.and_utc()),
        DatePerhapsTime::DateTime(CalendarDateTime::Floating(date_time)) => {
            Some(date_time.and_utc())
        }
        DatePerhapsTime::DateTime(date_time) => date_time.try_into_utc(),
    }
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

fn duration_minutes(start: &DateTime<Utc>, end: &DateTime<Utc>) -> Option<i64> {
    Some((end.signed_duration_since(*start).num_seconds() / 60).max(0))
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
