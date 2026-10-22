// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Calendar notes written from an imported calendar file.
//!
//! Each event becomes calendar note lines with the identity a phone uses, so an
//! event imported from a file and the same event sent from a phone are one
//! event in the journal's listing of when things happen. The lines sit beside
//! the import window's transcript, stamped at the instant that placed the
//! event, and carry the import that wrote them and when it happened.
//!
//! Repeating events are expanded here, with the parser's own recurrence
//! support, up to a year after the import or the series' end. Times keep their
//! own zone's wall clock across daylight-saving changes, and nothing depends on
//! the machine's zone.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::OnceLock;

use chrono::{
    DateTime, Duration, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;
use icalendar::{Component, Property};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// The import a set of calendar notes comes from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportStamp {
    /// The import's id.
    pub id: String,
    /// When the import happened, from its own record.
    pub imported_at: DateTime<Utc>,
}

/// What an import left out of, or shortened in, the calendar listing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CalendarNoteCounts {
    /// Events with no time they were last changed or created.
    pub no_change_time: u64,
    /// Events set in a time zone no table names.
    pub unknown_zone: u64,
    /// Events whose lines would not meet the calendar note contract.
    pub invalid: u64,
    /// Repeating series with more dates than are kept.
    pub truncated_series: u64,
    /// Repeating series whose rule could not be expanded.
    pub unexpandable_series: u64,
    /// Events left out once the import reached its line limit.
    pub over_limit: u64,
}

impl CalendarNoteCounts {
    /// Events that wrote no calendar line.
    pub fn skipped(&self) -> u64 {
        self.no_change_time + self.unknown_zone + self.invalid
    }
}

/// The most dates kept for one repeating series.
pub const MAX_SERIES_OCCURRENCES: usize = 5_000;
/// How far past the import a series is expanded.
const EXPANSION_DAYS: i64 = 365;
/// Steps of a rule taken before it is treated as one that cannot be expanded.
const MAX_EXPANSION_STEPS: usize = 400_000;

const WINDOWS_ZONES: &str = include_str!("windows_zones.tsv");

/// The IANA zone a `TZID` names: an IANA name itself, or a Windows zone name
/// as Outlook writes them, through the standard Windows-to-IANA table.
pub fn resolve_zone(tzid: &str) -> Option<Tz> {
    let tzid = tzid.trim().trim_matches('"');
    if let Ok(zone) = tzid.parse::<Tz>() {
        return Some(zone);
    }
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            WINDOWS_ZONES
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .filter_map(|line| line.split_once('\t'))
                .collect()
        })
        .get(tzid)
        .and_then(|iana| iana.parse().ok())
}

/// A calendar time as the file states it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IcsTime {
    Zoned(DateTime<Tz>),
    Utc(DateTime<Utc>),
    Floating(NaiveDateTime),
    Date(NaiveDate),
}

/// A `TZID` that resolves to no zone.
#[derive(Debug)]
struct UnknownZone;

impl IcsTime {
    fn instant(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Zoned(time) => Some(time.with_timezone(&Utc)),
            Self::Utc(time) => Some(*time),
            Self::Floating(_) | Self::Date(_) => None,
        }
    }

    /// The occurrence id this time gives, written the way the contract writes
    /// occurrences.
    fn occurrence(&self) -> String {
        match self {
            Self::Zoned(_) | Self::Utc(_) => self
                .instant()
                .expect("zoned times have an instant")
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
            Self::Floating(wall) => wall.format("%Y-%m-%dT%H:%M:%S").to_string(),
            Self::Date(date) => date.format("%Y-%m-%d").to_string(),
        }
    }

    /// The time written as a note's `start` or `end`.
    fn written(&self) -> String {
        match self {
            Self::Zoned(time) => time.to_rfc3339_opts(SecondsFormat::Secs, false),
            Self::Utc(time) => time.to_rfc3339_opts(SecondsFormat::Secs, true),
            Self::Floating(wall) => wall.format("%Y-%m-%dT%H:%M:%S").to_string(),
            Self::Date(date) => date.format("%Y-%m-%d").to_string(),
        }
    }

    fn zone_name(&self) -> Option<String> {
        match self {
            Self::Zoned(time) => Some(time.timezone().name().to_owned()),
            Self::Utc(_) => Some("UTC".to_owned()),
            Self::Floating(_) | Self::Date(_) => None,
        }
    }

    fn is_date(&self) -> bool {
        matches!(self, Self::Date(_))
    }

    /// A key that orders and compares times of one series.
    fn key(&self) -> NaiveDateTime {
        match self {
            Self::Zoned(_) | Self::Utc(_) => self.instant().expect("instant").naive_utc(),
            Self::Floating(wall) => *wall,
            Self::Date(date) => date.and_hms_opt(0, 0, 0).expect("midnight"),
        }
    }

    /// The same moment shifted by a duration, keeping this time's kind.
    fn plus(&self, length: Duration) -> Self {
        match self {
            Self::Zoned(time) => Self::Zoned(*time + length),
            Self::Utc(time) => Self::Utc(*time + length),
            Self::Floating(wall) => Self::Floating(*wall + length),
            Self::Date(date) => Self::Date(*date + length),
        }
    }

    fn until(&self, end: &Self) -> Option<Duration> {
        match (self, end) {
            (Self::Date(start), Self::Date(end)) => Some(*end - *start),
            (Self::Floating(start), Self::Floating(end)) => Some(*end - *start),
            _ => Some(end.instant()? - self.instant()?),
        }
    }
}

/// Parse one property's value or values (a list for `EXDATE` and `RDATE`).
fn parse_times(property: &Property) -> Result<Vec<IcsTime>, UnknownZone> {
    let tzid = property.params().get("TZID").map(|param| param.value());
    let is_date = property
        .params()
        .get("VALUE")
        .is_some_and(|param| param.value().eq_ignore_ascii_case("DATE"));
    let zone = match tzid {
        Some(tzid) => Some(resolve_zone(tzid).ok_or(UnknownZone)?),
        None => None,
    };
    let mut times = Vec::new();
    for value in property.value().split(',') {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if is_date || value.len() == 8 {
            if let Ok(date) = NaiveDate::parse_from_str(value, "%Y%m%d") {
                times.push(IcsTime::Date(date));
            }
            continue;
        }
        if let Some(utc) = value.strip_suffix('Z') {
            if let Ok(wall) = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S") {
                times.push(IcsTime::Utc(Utc.from_utc_datetime(&wall)));
            }
            continue;
        }
        let Ok(wall) = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S") else {
            continue;
        };
        match zone {
            Some(zone) => {
                if let Some(time) = zone_time(zone, wall) {
                    times.push(IcsTime::Zoned(time));
                }
            }
            None => times.push(IcsTime::Floating(wall)),
        }
    }
    Ok(times)
}

/// A wall time in a zone; one the clocks skip reads an hour on.
fn zone_time(zone: Tz, wall: NaiveDateTime) -> Option<DateTime<Tz>> {
    zone.from_local_datetime(&wall).earliest().or_else(|| {
        zone.from_local_datetime(&(wall + Duration::hours(1)))
            .earliest()
    })
}

fn first_time(event: &icalendar::Event, key: &str) -> Result<Option<IcsTime>, UnknownZone> {
    match event.properties().get(key) {
        Some(property) => Ok(parse_times(property)?.into_iter().next()),
        None => Ok(None),
    }
}

fn multi_times(event: &icalendar::Event, key: &str) -> Result<Vec<IcsTime>, UnknownZone> {
    let mut times = Vec::new();
    for property in event.multi_properties().get(key).into_iter().flatten() {
        times.extend(parse_times(property)?);
    }
    Ok(times)
}

/// One `VEVENT` with the calendar it was read from.
pub(crate) struct SourceEvent<'a> {
    pub event: &'a icalendar::Event,
    pub calendar: Option<String>,
}

/// How an event's transcript was placed, as its calendar notes read it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Placement {
    /// The event wrote no transcript.
    NotPlaced,
    /// Placed, but not by a readable `LAST-MODIFIED` or `CREATED`.
    Unstamped,
    /// Placed at this change time.
    Stamped(DateTime<Utc>),
}

/// The most calendar note lines one import writes.
pub const MAX_IMPORT_LINES: usize = 200_000;

/// An event's identity for counting: its id and its original start, if any.
type EventKey = (String, String);

/// The calendar notes of one import, written one window at a time.
///
/// Overrides are read first, so a series leaves out only the dates an
/// override actually wrote a line for. Every other event's lines, a series'
/// expansion included, are made only when its window is written, and an
/// import writes at most its line limit.
pub(crate) struct ImportNotes<'a, Z> {
    sources: &'a [SourceEvent<'a>],
    placements: Vec<Placement>,
    stamp: &'a ImportStamp,
    zone: &'a Z,
    series: Series,
    overrides: Vec<Option<Outcome>>,
    line_cap: usize,
    written: usize,
    tally: Tally,
}

type Outcome = Result<(Vec<Map<String, Value>>, Expanded), Skip>;

/// What expanding a series' rule came to.
#[derive(Clone, Copy, Default)]
struct Expanded {
    truncated: bool,
    unexpandable: bool,
}

/// Counted events, each once however many export members repeat it.
#[derive(Default)]
struct Tally {
    no_change_time: BTreeSet<EventKey>,
    unknown_zone: BTreeSet<EventKey>,
    invalid: BTreeSet<EventKey>,
    truncated: BTreeSet<String>,
    unexpandable: BTreeSet<String>,
    over_limit: BTreeSet<EventKey>,
}

impl<'a, Z: TimeZone<Offset: fmt::Display>> ImportNotes<'a, Z> {
    pub(crate) fn new(
        sources: &'a [SourceEvent<'a>],
        placements: Vec<Placement>,
        stamp: &'a ImportStamp,
        zone: &'a Z,
        line_cap: usize,
    ) -> Self {
        // Each series' own rule, which its overrides share.
        let mut series = Series::default();
        for source in sources {
            let event = source.event;
            if let (Some(uid), Ok(None), Some(rule)) = (
                event_uid(event),
                first_time(event, "RECURRENCE-ID"),
                nonempty(event.property_value("RRULE")),
            ) {
                series.rules.insert(uid, rule);
            }
        }
        // An override stands for its date only once it has a line of its own.
        let mut overrides = Vec::with_capacity(sources.len());
        let mut overridden = HashSet::new();
        for (source, placement) in sources.iter().zip(&placements) {
            let Ok(Some(recurrence_id)) = first_time(source.event, "RECURRENCE-ID") else {
                overrides.push(None);
                continue;
            };
            let outcome = outcome(source, *placement, stamp, zone, &series);
            if let (Ok((lines, _)), Some(uid)) = (&outcome, event_uid(source.event))
                && !lines.is_empty()
            {
                overridden.insert((uid, recurrence_id.occurrence()));
            }
            overrides.push(Some(outcome));
        }
        series.overridden = overridden;
        Self {
            sources,
            placements,
            stamp,
            zone,
            series,
            overrides,
            line_cap,
            written: 0,
            tally: Tally::default(),
        }
    }

    /// The lines of one event, made now and handed over; called once per
    /// event, as its window is written.
    pub(crate) fn lines(&mut self, index: usize) -> Vec<Map<String, Value>> {
        let source = &self.sources[index];
        let outcome = match self.overrides[index].take() {
            Some(outcome) => outcome,
            None => outcome(
                source,
                self.placements[index],
                self.stamp,
                self.zone,
                &self.series,
            ),
        };
        let event = source.event;
        let uid = event_uid(event).unwrap_or_else(|| derived_uid(event));
        let key = || {
            let original = first_time(event, "RECURRENCE-ID")
                .ok()
                .flatten()
                .map(|time| time.occurrence())
                .unwrap_or_default();
            (uid.clone(), original)
        };
        match outcome {
            Ok((lines, expanded)) => {
                if expanded.truncated {
                    self.tally.truncated.insert(uid.clone());
                }
                if expanded.unexpandable {
                    self.tally.unexpandable.insert(uid.clone());
                }
                if self.written + lines.len() > self.line_cap {
                    self.tally.over_limit.insert(key());
                    return Vec::new();
                }
                self.written += lines.len();
                lines
            }
            Err(skip) => {
                let counted = match skip {
                    Skip::NoChangeTime => &mut self.tally.no_change_time,
                    Skip::UnknownZone => &mut self.tally.unknown_zone,
                    Skip::Invalid => &mut self.tally.invalid,
                };
                counted.insert(key());
                Vec::new()
            }
        }
    }

    /// What the import left out of, or shortened in, the listing.
    pub(crate) fn counts(&self) -> CalendarNoteCounts {
        let count = |len: usize| u64::try_from(len).expect("count fits u64");
        CalendarNoteCounts {
            no_change_time: count(self.tally.no_change_time.len()),
            unknown_zone: count(self.tally.unknown_zone.len()),
            invalid: count(self.tally.invalid.len()),
            truncated_series: count(self.tally.truncated.len()),
            unexpandable_series: count(self.tally.unexpandable.len()),
            over_limit: count(self.tally.over_limit.len()),
        }
    }
}

fn outcome(
    source: &SourceEvent<'_>,
    placement: Placement,
    stamp: &ImportStamp,
    zone: &impl TimeZone<Offset: fmt::Display>,
    series: &Series,
) -> Outcome {
    match placement {
        Placement::NotPlaced => Ok((Vec::new(), Expanded::default())),
        Placement::Unstamped => Err(Skip::NoChangeTime),
        Placement::Stamped(placed_at) => event_notes(source, placed_at, stamp, zone, series),
    }
}

#[derive(Default)]
struct Series {
    /// Each series' rule, by UID.
    rules: HashMap<String, String>,
    /// The dates, by UID and original start, an override wrote a line for.
    overridden: HashSet<(String, String)>,
}

enum Skip {
    NoChangeTime,
    UnknownZone,
    Invalid,
}

impl From<UnknownZone> for Skip {
    fn from(_: UnknownZone) -> Self {
        Self::UnknownZone
    }
}

fn event_uid(event: &icalendar::Event) -> Option<String> {
    event
        .property_value("UID")
        .map(str::trim)
        .filter(|uid| !uid.is_empty())
        .map(str::to_owned)
}

fn is_cancelled(event: &icalendar::Event) -> bool {
    event
        .property_value("STATUS")
        .is_some_and(|status| status.trim().eq_ignore_ascii_case("CANCELLED"))
}

/// A stable identity for an event with no `UID`, from its own content, so a
/// re-import of the same file gives the same identity.
fn derived_uid(event: &icalendar::Event) -> String {
    let mut digest = Sha256::new();
    for key in [
        "SUMMARY",
        "DTSTART",
        "DTEND",
        "DURATION",
        "RRULE",
        "LOCATION",
        "DESCRIPTION",
    ] {
        digest.update(key.as_bytes());
        digest.update([0]);
        if let Some(property) = event.properties().get(key) {
            if let Some(tzid) = property.params().get("TZID") {
                digest.update(tzid.value().as_bytes());
            }
            digest.update([1]);
            digest.update(property.value().as_bytes());
        }
        digest.update([0]);
    }
    format!("ics-{:x}", digest.finalize())
}

/// The fields every line of one event shares, besides its times.
struct EventFacts {
    event: String,
    calendar: Option<String>,
    title: Option<String>,
    location: Option<String>,
    notes: Option<String>,
    url: Option<String>,
    organizer: Value,
    attendees: Value,
    repeats: Option<String>,
    changed_at: String,
    stamp: ImportStamp,
    imported_at: String,
    /// The lines come from expanding the series' rule, so they cover every
    /// date it gives up to a year past the import.
    expanded: bool,
}

impl EventFacts {
    fn line(
        &self,
        change: &str,
        occurrence: Option<String>,
        start: Option<&IcsTime>,
        end: Option<&IcsTime>,
    ) -> Map<String, Value> {
        let removed = change == "removed";
        let text = |value: &Option<String>| match value {
            Some(value) if !removed => json!(value),
            _ => Value::Null,
        };
        let mut line = Map::new();
        line.insert(
            "all_day".into(),
            if removed {
                Value::Null
            } else {
                json!(start.is_some_and(IcsTime::is_date))
            },
        );
        line.insert(
            "attendees".into(),
            if removed {
                json!([])
            } else {
                self.attendees.clone()
            },
        );
        line.insert("calendar".into(), json!(self.calendar));
        line.insert("change".into(), json!(change));
        line.insert("changed_at".into(), json!(self.changed_at));
        let start_written = start.filter(|_| !removed);
        let end_written = end.filter(|_| !removed).filter(|end| {
            start_written
                .and_then(|start| start.until(end))
                .is_some_and(|length| length >= Duration::zero())
        });
        line.insert("end".into(), json!(end_written.map(IcsTime::written)));
        line.insert("event".into(), json!(self.event));
        if self.expanded {
            line.insert("expanded".into(), json!(true));
        }
        line.insert("import".into(), json!(self.stamp.id));
        line.insert("imported_at".into(), json!(self.imported_at));
        line.insert("location".into(), text(&self.location));
        line.insert("notes".into(), text(&self.notes));
        line.insert("occurrence".into(), json!(occurrence));
        line.insert(
            "organizer".into(),
            if removed {
                Value::Null
            } else {
                self.organizer.clone()
            },
        );
        line.insert("repeats".into(), text(&self.repeats));
        line.insert("start".into(), json!(start_written.map(IcsTime::written)));
        line.insert(
            "time_zone".into(),
            json!(start_written.and_then(IcsTime::zone_name)),
        );
        line.insert("title".into(), json!(self.title));
        line.insert("url".into(), text(&self.url));
        line.insert("v".into(), json!(1));
        line
    }
}

fn nonempty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// A person from an `ORGANIZER` or `ATTENDEE` property.
fn person(property: &Property, with_status: bool) -> Value {
    let value = property.value().trim();
    let email = value
        .get(..7)
        .filter(|scheme| scheme.eq_ignore_ascii_case("mailto:"))
        .map(|_| value[7..].trim().to_ascii_lowercase())
        .filter(|email| !email.is_empty());
    let name = property
        .params()
        .get("CN")
        .map(|param| param.value().trim().trim_matches('"').to_owned())
        .filter(|name| !name.is_empty());
    let mut person = Map::new();
    person.insert("email".into(), json!(email));
    person.insert("name".into(), json!(name));
    if with_status {
        let status = match property
            .params()
            .get("PARTSTAT")
            .map(|param| param.value().to_ascii_uppercase())
            .as_deref()
        {
            Some("ACCEPTED") => "accepted",
            Some("DECLINED") => "declined",
            Some("TENTATIVE") => "tentative",
            Some("NEEDS-ACTION") => "pending",
            _ => "unknown",
        };
        person.insert("status".into(), json!(status));
        person.insert("you".into(), json!(false));
    }
    Value::Object(person)
}

fn attendees(event: &icalendar::Event) -> Value {
    let mut people = event
        .multi_properties()
        .get("ATTENDEE")
        .into_iter()
        .flatten()
        .map(|property| person(property, true))
        .collect::<Vec<_>>();
    let key = |person: &Value| {
        (
            person["email"].as_str().map(str::to_owned),
            person["name"].as_str().map(str::to_owned),
        )
    };
    people.sort_by_key(key);
    Value::Array(people)
}

fn event_notes(
    source: &SourceEvent<'_>,
    placed_at: DateTime<Utc>,
    stamp: &ImportStamp,
    zone: &impl TimeZone<Offset: fmt::Display>,
    series: &Series,
) -> Outcome {
    let event = source.event;
    let start = first_time(event, "DTSTART")?.ok_or(Skip::Invalid)?;
    let end = first_time(event, "DTEND")?;
    let recurrence_id = first_time(event, "RECURRENCE-ID")?;
    let exdates = multi_times(event, "EXDATE")?;
    let rdates = multi_times(event, "RDATE")?;
    let uid = event_uid(event).unwrap_or_else(|| derived_uid(event));
    let rule = nonempty(event.property_value("RRULE"));
    // An override reads as part of its series.
    let series_rule = rule.clone().or_else(|| {
        recurrence_id
            .as_ref()
            .and_then(|_| series.rules.get(&uid).cloned())
    });
    let changed_at = in_zone_whole_seconds(placed_at, zone);
    let imported_at = in_zone_whole_seconds(stamp.imported_at, zone);
    let organizer = event
        .properties()
        .get("ORGANIZER")
        .map(|property| person(property, false))
        .filter(|person| !person["email"].is_null() || !person["name"].is_null())
        .unwrap_or(Value::Null);
    let mut facts = EventFacts {
        event: uid.clone(),
        calendar: source.calendar.clone(),
        title: nonempty(event.property_value("SUMMARY")),
        location: nonempty(event.property_value("LOCATION")),
        notes: nonempty(event.property_value("DESCRIPTION")),
        url: nonempty(event.property_value("URL")),
        organizer,
        attendees: attendees(event),
        repeats: series_rule.as_deref().and_then(crate::ics::describe_rrule),
        changed_at,
        stamp: stamp.clone(),
        imported_at,
        expanded: false,
    };
    let length = end.as_ref().and_then(|end| start.until(end));
    let cancelled = is_cancelled(event);

    // An override, or a one-off event.
    if rule.is_none() && rdates.is_empty() {
        let occurrence = recurrence_id.as_ref().map(IcsTime::occurrence);
        let line = if cancelled {
            facts.line("removed", occurrence, None, None)
        } else {
            facts.line("added", occurrence, Some(&start), end.as_ref())
        };
        return checked(vec![line], Expanded::default());
    }

    // A repeating series: its own start, then every date its rule gives, or
    // only the most recent of them when there are more than a series keeps.
    let overridden = |occurrence: &String| {
        series
            .overridden
            .contains(&(uid.clone(), occurrence.clone()))
    };
    let bound = stamp.imported_at + Duration::days(EXPANSION_DAYS);
    let mut expanded = Expanded::default();
    let mut starts = match rule.as_deref().map(|rule| expand(rule, &start, bound)) {
        None => vec![start],
        Some(Ok(expansion)) => {
            // Lines from a rule cover what it gives up to the bound.
            facts.expanded = true;
            expanded.truncated = expansion.truncated;
            let mut starts = expansion.starts;
            if !expansion.truncated {
                starts.insert(0, start);
            }
            starts
        }
        Some(Err(())) => {
            expanded.unexpandable = true;
            vec![start]
        }
    };
    starts.extend(rdates.iter().copied().filter(|rdate| {
        rdate
            .instant()
            .zip(Some(bound))
            .is_none_or(|(instant, bound)| instant <= bound)
    }));
    let excluded = exdates
        .iter()
        .map(IcsTime::occurrence)
        .collect::<BTreeSet<_>>();
    let mut lines = Vec::new();
    let mut seen = BTreeSet::new();
    for occurrence_start in &starts {
        let occurrence = occurrence_start.occurrence();
        // An override's own line stands for its occurrence.
        if overridden(&occurrence) || !seen.insert(occurrence.clone()) {
            continue;
        }
        let line = if cancelled || excluded.contains(&occurrence) {
            facts.line("removed", Some(occurrence), None, None)
        } else {
            let end = length.map(|length| occurrence_start.plus(length));
            facts.line(
                "added",
                Some(occurrence),
                Some(occurrence_start),
                end.as_ref(),
            )
        };
        lines.push(line);
    }
    // Each excluded date says it is gone, unless an override takes its place:
    // the override's own line then stands for it.
    for exdate in &exdates {
        let occurrence = exdate.occurrence();
        if overridden(&occurrence) || seen.contains(&occurrence) {
            continue;
        }
        seen.insert(occurrence.clone());
        lines.push(facts.line("removed", Some(occurrence), None, None));
    }
    checked(lines, expanded)
}

/// An instant in the journal zone's offset at that instant, in whole seconds,
/// never written as `Z`.
fn in_zone_whole_seconds(
    instant: DateTime<Utc>,
    zone: &impl TimeZone<Offset: fmt::Display>,
) -> String {
    let whole = instant - Duration::nanoseconds(i64::from(instant.nanosecond()));
    whole
        .with_timezone(zone)
        .to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// Keep only lines the contract accepts, each small enough for a file of its
/// own; one that fails leaves the event out.
fn checked(lines: Vec<Map<String, Value>>, expanded: Expanded) -> Outcome {
    use solstone_core_format::calendar::{MAX_FILE_BYTES, calendar_note_from_record};
    for line in &lines {
        let bytes = serde_json::to_string(line).map_or(usize::MAX, |text| text.len() + 1);
        if bytes > MAX_FILE_BYTES || calendar_note_from_record(line.clone()).is_err() {
            return Err(Skip::Invalid);
        }
    }
    Ok((lines, expanded))
}

#[derive(Debug, Default)]
struct Expansion {
    starts: Vec<IcsTime>,
    truncated: bool,
}

/// Expand a rule from its first start up to `bound`, keeping the most recent
/// dates when there are more than a series keeps.
fn expand(rule: &str, start: &IcsTime, bound: DateTime<Utc>) -> Result<Expansion, ()> {
    use icalendar::rrule::{RRule, Tz as RuleZone, Unvalidated};
    // UNTIL is read here, in the series' own frame, since files often write it
    // with no Z where the parser asks for UTC.
    let mut until = None;
    let mut parts = Vec::new();
    for part in rule.split(';') {
        match part.split_once('=') {
            Some((key, value)) if key.trim().eq_ignore_ascii_case("UNTIL") => {
                until = Some(until_instant(value.trim(), start).ok_or(())?);
            }
            _ => parts.push(part),
        }
    }
    let mut parsed: RRule<Unvalidated> = parts.join(";").parse().map_err(|_| ())?;
    if let Some(until) = until {
        parsed = parsed.until(until.with_timezone(&RuleZone::UTC));
    }
    // Floating and all-day times are expanded as wall clock with no zone, so
    // the machine's own zone never takes part.
    let (zone, first) = match start {
        IcsTime::Zoned(time) => {
            let zone = RuleZone::Tz(time.timezone());
            (zone, time.with_timezone(&zone))
        }
        IcsTime::Utc(time) => (RuleZone::UTC, time.with_timezone(&RuleZone::UTC)),
        IcsTime::Floating(_) | IcsTime::Date(_) => {
            (RuleZone::UTC, RuleZone::UTC.from_utc_datetime(&start.key()))
        }
    };
    let set = parsed.build(first).map_err(|_| ())?;
    let bound = bound.with_timezone(&zone);
    let mut kept = VecDeque::new();
    let mut truncated = false;
    for (step, time) in (&set).into_iter().enumerate() {
        if step >= MAX_EXPANSION_STEPS {
            return Err(());
        }
        if time > bound {
            break;
        }
        if kept.len() == MAX_SERIES_OCCURRENCES {
            kept.pop_front();
            truncated = true;
        }
        kept.push_back(time);
    }
    let starts = kept
        .into_iter()
        .map(|time| match start {
            IcsTime::Zoned(original) => IcsTime::Zoned(time.with_timezone(&original.timezone())),
            IcsTime::Utc(_) => IcsTime::Utc(time.with_timezone(&Utc)),
            IcsTime::Floating(_) => IcsTime::Floating(time.naive_utc()),
            IcsTime::Date(_) => IcsTime::Date(time.naive_utc().date()),
        })
        .collect();
    Ok(Expansion { starts, truncated })
}

/// The last moment a rule's `UNTIL` allows, in the frame its series is
/// expanded in: the start's own zone for a zoned series, and wall clock as UTC
/// for floating and all-day ones. A date `UNTIL` takes in its whole day; a
/// time with no `Z` is read in the series' own zone.
fn until_instant(value: &str, start: &IcsTime) -> Option<DateTime<Utc>> {
    let (wall, utc) = if let Some(utc) = value.strip_suffix('Z') {
        (
            NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?,
            true,
        )
    } else if let Ok(date) = NaiveDate::parse_from_str(value, "%Y%m%d") {
        (date.and_hms_opt(23, 59, 59)?, false)
    } else {
        (
            NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?,
            false,
        )
    };
    match start {
        IcsTime::Zoned(time) if !utc => Some(zone_time(time.timezone(), wall)?.with_timezone(&Utc)),
        _ => Some(Utc.from_utc_datetime(&wall)),
    }
}

/// Keep one line per identity: the latest `changed_at`, then a removal over
/// an addition, then the earliest calendar name.
pub(crate) fn one_per_identity(records: Vec<Map<String, Value>>) -> Vec<Map<String, Value>> {
    let rank = |record: &Map<String, Value>| {
        let changed = record
            .get("changed_at")
            .and_then(Value::as_str)
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|time| time.timestamp());
        let removed = record.get("change").and_then(Value::as_str) == Some("removed");
        // The earliest name wins, and a missing name loses to every name.
        let calendar = record.get("calendar").and_then(Value::as_str);
        (
            changed,
            removed,
            calendar.is_some(),
            std::cmp::Reverse(calendar.map(str::to_owned)),
        )
    };
    let mut kept: BTreeMap<(String, Option<String>), Map<String, Value>> = BTreeMap::new();
    for record in records {
        let identity = (
            record
                .get("event")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            // Written by this importer, so already in the contract's form.
            record
                .get("occurrence")
                .and_then(Value::as_str)
                .map(str::to_owned),
        );
        match kept.get(&identity) {
            Some(current) if rank(current) >= rank(&record) => {}
            _ => {
                kept.insert(identity, record);
            }
        }
    }
    kept.into_values().collect()
}

#[cfg(test)]
#[path = "ics_notes_tests.rs"]
mod tests;
