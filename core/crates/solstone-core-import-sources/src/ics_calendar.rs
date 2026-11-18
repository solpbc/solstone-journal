// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A small iCalendar (RFC 5545) reader: the calendar's own properties and its
//! top-level `VEVENT`s, with each property's value and parameters.
//!
//! The grammar, with its leniencies and refusals:
//!
//! - Folded lines are unfolded first: a line break (`CRLF` or `LF`) followed
//!   by one space or tab is removed.
//! - A leading byte-order mark is skipped.
//! - The file is one component (normally `VCALENDAR`) that must reach the end
//!   of the text, with nothing but line breaks after its `END`. A component
//!   ends at the first `END:` line; `END:` with another name (a longer one
//!   included) refuses the whole file, as does a line that is neither a
//!   property nor a component.
//! - `BEGIN`, `END` and the component names after them are matched in any
//!   case, as RFC 5545 has names; property and parameter names keep the case
//!   they are written in, and lookups of them are exact.
//! - A parameter value is everything up to the next `;` or `:`, or a quoted
//!   string with the quotes removed. An empty value reads as none.
//! - A property value runs to the end of its line, at the next `LF` or
//!   `CRLF`, whichever a file uses for that line. Text values (`SUMMARY`,
//!   `DESCRIPTION`, `LOCATION`, `UID`, `STATUS`, every `X-` name and any
//!   property marked `VALUE=TEXT`) have `\\`, `\,`, `\;`, `\:` and `\n`/`\N`
//!   escapes decoded; other values keep their text as written.
//! - On an event, a property that may repeat (`ATTENDEE`, `EXDATE`, `RDATE`,
//!   `CATEGORIES` and the rest of RFC 5545's list) keeps every occurrence in
//!   order; any other property keeps its last occurrence.
//!
//! Dates and date-times are read leniently: up to four year digits (any number
//! after a sign) and up to two digits for each other field, with whitespace
//! allowed before each number. A second of `60` reads as `59`.

use std::borrow::Cow;
use std::collections::BTreeMap;

use jiff::civil::{Date, DateTime, Time};
use jiff::tz::Offset;
use jiff::{Timestamp, ZonedDateTime};

/// A parsed calendar file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Calendar {
    properties: Vec<Property>,
    events: Vec<Event>,
}

impl Calendar {
    /// Read a calendar's text; `None` when the text is not a calendar this
    /// reader accepts.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let unfolded = unfold(text.strip_prefix('\u{feff}').unwrap_or(text));
        let (rest, root) = component(&unfolded).ok()?;
        if !rest.is_empty() {
            return None;
        }
        let (properties, children) = if root.name.eq_ignore_ascii_case("VCALENDAR") {
            (root.properties, root.components)
        } else {
            (Vec::new(), vec![root])
        };
        Some(Self {
            properties: properties.into_iter().map(Property::from_raw).collect(),
            events: children
                .into_iter()
                .filter(|child| child.name.eq_ignore_ascii_case("VEVENT"))
                .map(Event::from_raw)
                .collect(),
        })
    }

    /// The value of the calendar's first property named `key`.
    #[must_use]
    pub fn property_value(&self, key: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|property| property.key == key)
            .map(Property::value)
    }

    /// The calendar's name: its `NAME`, or else its `X-WR-CALNAME`.
    #[must_use]
    pub fn get_name(&self) -> Option<&str> {
        self.property_value("NAME")
            .or_else(|| self.property_value("X-WR-CALNAME"))
    }

    /// The calendar's own properties, in file order.
    #[cfg(test)]
    #[must_use]
    pub fn properties(&self) -> &[Property] {
        &self.properties
    }

    /// The calendar's top-level events, in file order.
    pub fn events(&self) -> impl Iterator<Item = &Event> {
        self.events.iter()
    }
}

/// Property names that may repeat on an event.
const MULTI_PROPERTIES: [&str; 13] = [
    "ATTACH",
    "ATTENDEE",
    "CATEGORIES",
    "COMMENT",
    "CONTACT",
    "EXDATE",
    "FREEBUSY",
    "IANA-PROP",
    "RDATE",
    "RELATED",
    "RESOURCES",
    "RSTATUS",
    "X-PROP",
];

/// One `VEVENT`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Event {
    properties: BTreeMap<String, Property>,
    multi_properties: BTreeMap<String, Vec<Property>>,
}

impl Event {
    fn from_raw(raw: RawComponent<'_>) -> Self {
        let mut event = Self::default();
        for property in raw.properties {
            let property = Property::from_raw(property);
            if MULTI_PROPERTIES.contains(&property.key.as_str()) {
                event
                    .multi_properties
                    .entry(property.key.clone())
                    .or_default()
                    .push(property);
            } else {
                event.properties.insert(property.key.clone(), property);
            }
        }
        event
    }

    /// Each single property, by its name as written; the last one written wins.
    #[must_use]
    pub fn properties(&self) -> &BTreeMap<String, Property> {
        &self.properties
    }

    /// Each property that may repeat, by name, every occurrence in order.
    #[must_use]
    pub fn multi_properties(&self) -> &BTreeMap<String, Vec<Property>> {
        &self.multi_properties
    }

    /// The value of the single property `key`.
    #[must_use]
    pub fn property_value(&self, key: &str) -> Option<&str> {
        self.properties.get(key).map(Property::value)
    }

    /// The event's `DTSTART`, when it reads as a date or a date-time.
    #[must_use]
    pub fn get_start(&self) -> Option<DatePerhapsTime> {
        DatePerhapsTime::from_property(self.properties.get("DTSTART")?)
    }

    /// The event's `DTEND`, when it reads as a date or a date-time.
    #[must_use]
    pub fn get_end(&self) -> Option<DatePerhapsTime> {
        DatePerhapsTime::from_property(self.properties.get("DTEND")?)
    }
}

/// One property: its name as written, its value and its parameters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Property {
    key: String,
    value: String,
    params: BTreeMap<String, Parameter>,
}

impl Property {
    fn from_raw(raw: RawProperty<'_>) -> Self {
        let mut params = BTreeMap::new();
        for (key, value) in raw.params {
            params.insert(
                key.to_owned(),
                Parameter {
                    key: key.to_owned(),
                    value: value.unwrap_or_default().to_owned(),
                },
            );
        }
        Self {
            key: raw.name.to_owned(),
            value: raw.value.into_owned(),
            params,
        }
    }

    /// The property's name as written.
    #[cfg(test)]
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The property's value, text escapes decoded where the value is text.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// The property's parameters by name as written; the last one written wins.
    #[must_use]
    pub fn params(&self) -> &BTreeMap<String, Parameter> {
        &self.params
    }

    fn value_type(&self) -> Option<ValueType> {
        self.params
            .get("VALUE")
            .and_then(|param| ValueType::from_name(&param.value))
            .or_else(|| ValueType::of_property(&self.key))
    }
}

/// One property parameter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Parameter {
    key: String,
    value: String,
}

impl Parameter {
    /// The parameter's name as written.
    #[cfg(test)]
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The parameter's value, quotes removed; empty when it has none.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// A calendar time that is a date or a date-time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DatePerhapsTime {
    DateTime(CalendarDateTime),
    Date(Date),
}

/// A date-time as the file states it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CalendarDateTime {
    /// A wall-clock time with no zone.
    Floating(DateTime),
    /// A time ending in `Z`.
    Utc(Timestamp),
    /// A wall-clock time with the zone its `TZID` names, which may be one no
    /// database knows.
    WithTimezone { date_time: DateTime, tzid: String },
}

impl DatePerhapsTime {
    /// Read a property as a date or a date-time.
    ///
    /// `VALUE=DATE` asks for a date. Otherwise a date-time is read first (with
    /// its `TZID` when it has one, else as `Z` time or wall clock), and a bare
    /// date is accepted when no date-time reads.
    #[must_use]
    pub fn from_property(property: &Property) -> Option<Self> {
        let value = property.value();
        if property.value_type() == Some(ValueType::Date) {
            return parse_ical_date(value).map(Self::Date);
        }
        if let Some(date_time) = CalendarDateTime::from_property(property) {
            return Some(Self::DateTime(date_time));
        }
        parse_ical_date(value).map(Self::Date)
    }
}

impl CalendarDateTime {
    fn from_property(property: &Property) -> Option<Self> {
        let value = property.value();
        if let Some(tzid) = property.params().get("TZID") {
            return Some(Self::WithTimezone {
                date_time: parse_ical_datetime(value)?,
                tzid: tzid.value().to_owned(),
            });
        }
        if let Some(date_time) = parse_ical_datetime(value) {
            return Some(Self::Floating(date_time));
        }
        parse_ical_utc(value).map(Self::Utc)
    }

    /// The instant this time names: a `Z` time, or a wall-clock time in a zone
    /// the database knows by exactly that name, when that wall-clock time
    /// happens exactly once. A floating time names no instant.
    #[must_use]
    pub fn try_into_utc(&self) -> Option<Timestamp> {
        match self {
            Self::Floating(_) => None,
            Self::Utc(instant) => Some(*instant),
            Self::WithTimezone { .. } => self.with_zone().map(|time| time.timestamp()),
        }
    }

    /// The zoned time a `TZID` time names, under the same conditions as
    /// [`Self::try_into_utc`].
    #[must_use]
    pub fn with_zone(&self) -> Option<ZonedDateTime> {
        let Self::WithTimezone { date_time, tzid } = self else {
            return None;
        };
        let zone = solstone_core_time::zone(tzid)?;
        solstone_core_time::from_local(&zone, *date_time).single()
    }
}

// ---------------------------------------------------------------------------
// Dates and date-times.

#[derive(Clone, Copy)]
enum Field {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Literal(char),
}

const DATE_FIELDS: &[Field] = &[Field::Year, Field::Month, Field::Day];
const DATE_TIME_FIELDS: &[Field] = &[
    Field::Year,
    Field::Month,
    Field::Day,
    Field::Literal('T'),
    Field::Hour,
    Field::Minute,
    Field::Second,
];
const UTC_FIELDS: &[Field] = &[
    Field::Year,
    Field::Month,
    Field::Day,
    Field::Literal('T'),
    Field::Hour,
    Field::Minute,
    Field::Second,
    Field::Literal('Z'),
];

/// A date written `YYYYMMDD`, read leniently (see the module notes).
#[must_use]
pub fn parse_ical_date(text: &str) -> Option<Date> {
    let fields = read_fields(text, DATE_FIELDS)?;
    date_of(&fields)
}

/// A wall-clock date-time written `YYYYMMDDTHHMMSS`, read leniently.
#[must_use]
pub fn parse_ical_datetime(text: &str) -> Option<DateTime> {
    let fields = read_fields(text, DATE_TIME_FIELDS)?;
    date_time_of(&fields)
}

/// A UTC date-time written `YYYYMMDDTHHMMSSZ`, read leniently.
#[must_use]
pub fn parse_ical_utc(text: &str) -> Option<Timestamp> {
    let fields = read_fields(text, UTC_FIELDS)?;
    Offset::UTC.to_timestamp(date_time_of(&fields)?).ok()
}

fn date_of(fields: &[i64; 6]) -> Option<Date> {
    let year = i16::try_from(fields[0]).ok()?;
    let month = i8::try_from(fields[1]).ok()?;
    let day = i8::try_from(fields[2]).ok()?;
    Date::new(year, month, day).ok()
}

fn date_time_of(fields: &[i64; 6]) -> Option<DateTime> {
    let date = date_of(fields)?;
    let hour = i8::try_from(fields[3]).ok()?;
    let minute = i8::try_from(fields[4]).ok()?;
    let second = i8::try_from(fields[5].min(59)).ok()?;
    Some(date.to_datetime(Time::new(hour, minute, second, 0).ok()?))
}

/// Read `fields` from `text`: each number may follow whitespace and has one
/// to its field's width in digits (a year with a sign has any number), each
/// literal is exact, and nothing may follow.
fn read_fields(text: &str, fields: &[Field]) -> Option<[i64; 6]> {
    let mut rest = text;
    let mut values = [0_i64; 6];
    for field in fields {
        let (index, width, range) = match field {
            Field::Literal(literal) => {
                rest = rest.strip_prefix(*literal)?;
                continue;
            }
            Field::Year => (0, 4, i64::from(i32::MIN)..=i64::from(i32::MAX)),
            Field::Month => (1, 2, 1..=12),
            Field::Day => (2, 2, 1..=31),
            Field::Hour => (3, 2, 0..=23),
            Field::Minute => (4, 2, 0..=59),
            Field::Second => (5, 2, 0..=60),
        };
        rest = rest.trim_start();
        let value = match (field, rest.as_bytes().first()) {
            (Field::Year, Some(b'-')) => {
                let (after, value) = digits(&rest[1..], usize::MAX)?;
                rest = after;
                value.checked_neg()?
            }
            (Field::Year, Some(b'+')) => {
                let (after, value) = digits(&rest[1..], usize::MAX)?;
                rest = after;
                value
            }
            _ => {
                let (after, value) = digits(rest, width)?;
                rest = after;
                value
            }
        };
        if !range.contains(&value) {
            return None;
        }
        values[index] = value;
    }
    rest.is_empty().then_some(values)
}

/// One to `max` ASCII digits.
fn digits(text: &str, max: usize) -> Option<(&str, i64)> {
    let count = text
        .bytes()
        .take(max)
        .take_while(u8::is_ascii_digit)
        .count();
    if count == 0 {
        return None;
    }
    let mut value = 0_i64;
    for byte in text[..count].bytes() {
        value = value.checked_mul(10)?.checked_add(i64::from(byte - b'0'))?;
    }
    Some((&text[count..], value))
}

// ---------------------------------------------------------------------------
// Value types: which values have text escapes decoded, and which read as a date.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ValueType {
    Date,
    Text,
    Other,
}

impl ValueType {
    /// A `VALUE` parameter's type, exactly as RFC 5545 spells it.
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "DATE" => Some(Self::Date),
            "TEXT" => Some(Self::Text),
            "BINARY" | "BOOLEAN" | "CAL-ADDRESS" | "DATE-TIME" | "DURATION" | "FLOAT"
            | "INTEGER" | "PERIOD" | "RECUR" | "TIME" | "URI" | "UTC-OFFSET" => Some(Self::Other),
            _ => None,
        }
    }

    /// A property's default type, for a name written in upper case.
    fn of_property(name: &str) -> Option<Self> {
        if name.chars().any(char::is_lowercase) {
            return None;
        }
        match name {
            "CALSCALE" | "METHOD" | "PRODID" | "VERSION" | "CATEGORIES" | "CLASS" | "COMMENT"
            | "DESCRIPTION" | "LOCATION" | "RESOURCES" | "STATUS" | "SUMMARY" | "TRANSP"
            | "TZID" | "TZNAME" | "CONTACT" | "RELATED-TO" | "UID" | "ACTION"
            | "REQUEST-STATUS" => Some(Self::Text),
            "ATTACH" | "GEO" | "PERCENT-COMPLETE" | "PRIORITY" | "COMPLETED" | "DTEND" | "DUE"
            | "DTSTART" | "DURATION" | "FREEBUSY" | "TZOFFSETFROM" | "TZOFFSETTO" | "TZURL"
            | "ATTENDEE" | "ORGANIZER" | "RECURRENCE-ID" | "URL" | "EXDATE" | "RDATE" | "RRULE"
            | "REPEAT" | "TRIGGER" | "CREATED" | "DTSTAMP" | "LAST-MODIFIED" | "SEQUENCE" => {
                Some(Self::Other)
            }
            name if name.starts_with("X-") => Some(Self::Text),
            _ => None,
        }
    }
}

/// Decode a text value's escapes in one left-to-right pass; an unknown
/// escape or a trailing backslash is kept as written.
fn unescape_text(value: &str) -> Cow<'_, str> {
    if !value.contains('\\') {
        return Cow::Borrowed(value);
    }
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match chars.next() {
            Some(escaped @ ('\\' | ',' | ';' | ':')) => out.push(escaped),
            Some('n' | 'N') => out.push('\n'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    Cow::Owned(out)
}

// ---------------------------------------------------------------------------
// The content-line grammar.

/// Remove each fold: a line break followed by one space or tab. Each pattern
/// is removed in turn within the pieces the ones before it left.
fn unfold(text: &str) -> String {
    fn remove(text: &str, patterns: &[&str], out: &mut String) {
        match patterns.split_first() {
            None => out.push_str(text),
            Some((pattern, rest)) => {
                for piece in text.split(pattern) {
                    remove(piece, rest, out);
                }
            }
        }
    }
    let mut out = String::with_capacity(text.len());
    remove(text, &["\r\n ", "\n ", "\r\n\t", "\n\t"], &mut out);
    out
}

struct RawComponent<'a> {
    name: &'a str,
    properties: Vec<RawProperty<'a>>,
    components: Vec<RawComponent<'a>>,
}

struct RawProperty<'a> {
    name: &'a str,
    value: Cow<'a, str>,
    params: Vec<(&'a str, Option<&'a str>)>,
}

/// Why a grammar rule did not match: `Soft` lets the caller try something
/// else; `Hard` refuses the whole text.
#[derive(Debug)]
enum Miss {
    Soft,
    Hard,
}

type Parsed<'a, T> = Result<(&'a str, T), Miss>;

fn skip_line_breaks(mut text: &str) -> &str {
    loop {
        if let Some(rest) = text.strip_prefix("\r\n") {
            text = rest;
        } else if let Some(rest) = text.strip_prefix('\n') {
            text = rest;
        } else {
            return text;
        }
    }
}

fn line_break(text: &str) -> &str {
    text.strip_prefix("\r\n")
        .or_else(|| text.strip_prefix('\n'))
        .unwrap_or(text)
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

fn is_name_character(character: char) -> bool {
    matches!(character, '.' | ',' | '/' | '_' | '-') || character.is_alphanumeric()
}

/// The longest run of name characters; possibly empty.
fn name(text: &str) -> (&str, &str) {
    let end = text
        .char_indices()
        .find(|(_, character)| !is_name_character(*character))
        .map_or(text.len(), |(index, _)| index);
    (&text[end..], &text[..end])
}

fn component(text: &str) -> Parsed<'_, RawComponent<'_>> {
    let rest = skip_line_breaks(text);
    let rest = strip_prefix_ignore_case(rest, "BEGIN:").ok_or(Miss::Soft)?;
    let (rest, component_name) = name(rest);
    let mut rest = skip_line_breaks(rest);
    let mut properties = Vec::new();
    let mut components = Vec::new();
    loop {
        let at = skip_line_breaks(rest);
        if let Some(after_end) = strip_prefix_ignore_case(at, "END:") {
            let (after_name, end_name) = name(after_end);
            if !end_name.eq_ignore_ascii_case(component_name) {
                return Err(Miss::Hard);
            }
            rest = skip_line_breaks(after_name);
            break;
        }
        let before = rest.len();
        let next = match component(at) {
            Ok((after, child)) => {
                components.push(child);
                skip_line_breaks(after)
            }
            Err(Miss::Hard) => return Err(Miss::Hard),
            Err(Miss::Soft) => {
                let (after, property) = property(at)?;
                properties.push(property);
                skip_line_breaks(after)
            }
        };
        if next.len() == before {
            return Err(Miss::Soft);
        }
        rest = next;
    }
    Ok((
        rest.trim_start_matches('\n'),
        RawComponent {
            name: component_name,
            properties,
            components,
        },
    ))
}

/// One content line. A name that is `BEGIN` or `END` (any case) followed by
/// `:`, `;` or the end of the text refuses the whole text. A line with no `:`
/// after its name and parameters reads as a property with no value.
fn property(text: &str) -> Parsed<'_, RawProperty<'_>> {
    let at = text.trim_start_matches([' ', '\t', '\r', '\n']);
    for reserved in ["BEGIN", "END"] {
        if let Some(after) = strip_prefix_ignore_case(at, reserved) {
            if after.is_empty() || after.starts_with([':', ';']) {
                return Err(Miss::Hard);
            }
            break;
        }
    }
    let (rest, property_name) = name(at);
    let (rest, params) = parameters(rest);
    if let Some(value_start) = rest.strip_prefix(':') {
        let end = value_start
            .find('\n')
            .map_or(value_start.len(), |line_feed| {
                if value_start[..line_feed].ends_with('\r') {
                    line_feed - 1
                } else {
                    line_feed
                }
            });
        let raw_value = &value_start[..end];
        let value = if value_type_when_read(property_name, &params) == Some(ValueType::Text) {
            unescape_text(raw_value)
        } else {
            Cow::Borrowed(raw_value)
        };
        return Ok((
            line_break(&value_start[end..]),
            RawProperty {
                name: property_name,
                value,
                params,
            },
        ));
    }
    let (rest, property_name) = name(text);
    Ok((
        line_break(rest),
        RawProperty {
            name: property_name,
            value: Cow::Borrowed(""),
            params: Vec::new(),
        },
    ))
}

/// The type that decides escape decoding as a line is read: the first
/// `VALUE` parameter's, when it has a known one, else the name's.
fn value_type_when_read(property_name: &str, params: &[(&str, Option<&str>)]) -> Option<ValueType> {
    params
        .iter()
        .find(|(key, _)| *key == "VALUE")
        .and_then(|(_, value)| *value)
        .and_then(ValueType::from_name)
        .or_else(|| ValueType::of_property(property_name))
}

fn parameters(mut text: &str) -> (&str, Vec<(&str, Option<&str>)>) {
    let mut params = Vec::new();
    while let Some(after_semicolon) = text.strip_prefix(';') {
        let (rest, key) = name(after_semicolon.trim_start_matches([' ', '\t']));
        let Some(value_start) = rest.strip_prefix('=') else {
            params.push((key, None));
            text = rest;
            continue;
        };
        let (rest, value) = parameter_value(value_start);
        params.push((key, value));
        text = rest;
    }
    (text, params)
}

/// A quoted string (at least one character between the quotes), else the
/// text up to the next `;` or `:`.
fn parameter_value(text: &str) -> (&str, Option<&str>) {
    if let Some(inner) = text.strip_prefix('"')
        && let Some(close) = inner.find('"')
        && close > 0
    {
        return (&inner[close + 1..], Some(&inner[..close]));
    }
    let end = text.find([';', ':']).unwrap_or(text.len());
    if end == 0 {
        (text, None)
    } else {
        (&text[end..], Some(&text[..end]))
    }
}

#[cfg(test)]
#[path = "ics_calendar_tests.rs"]
mod tests;
