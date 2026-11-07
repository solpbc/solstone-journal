// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reading Oura's API documents into body records.
//!
//! `docs/BODY_SEGMENTS.md` is the contract. What is read is one table here,
//! [`read_endpoint`]. A page from an endpoint outside it is counted and named, and
//! its values are never held. Every document is kept whole in `fields`, with every
//! key as Oura gave it. Each row names Oura as `by`, since the API names no device
//! or app. `oura_run` reads staged pages through this, one document at a time.

use std::collections::BTreeSet;

use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use solstone_core_format::content::body_records::{
    BodyRow, RowKind, parse_source_time, plain_words,
};

use crate::ImportSourcesError;
use crate::apple_health::{MAX_EXPERIENCE_SECONDS, sha256_hex};
use crate::body_stream::OURA;

pub fn reserved_seam() -> Result<(), ImportSourcesError> {
    Err(ImportSourcesError::Unimplemented { module: "oura" })
}

/// Who recorded every Oura row.
pub(crate) const BY: &str = "Oura";

/// The endpoints read, by Oura's own names. Casing matters: `vO2_max` is how
/// Oura's path spells it.
pub const ENDPOINTS: [&str; 13] = [
    "daily_readiness",
    "daily_sleep",
    "daily_stress",
    "daily_resilience",
    "daily_spo2",
    "sleep",
    "daily_activity",
    "heartrate",
    "daily_cardiovascular_age",
    "workout",
    "session",
    "enhanced_tag",
    "vO2_max",
];

/// How an endpoint's documents are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    /// A night or a nap: an experience of kind `sleep`, from `bedtime_start` to
    /// `bedtime_end`, on hour tiles.
    Sleep,
    /// An experience of kind `workout` on the 300-second grid.
    Workout,
    /// An experience on the grid, or a nap on hours.
    Session,
    /// An entry the owner logged in Oura, in its start's hour.
    Tag,
    /// One point reading per item, which has no id: its time is its identity.
    Heartrate,
    /// A daily document, dated by Oura's `day`, with this label.
    Dated(&'static str),
}

/// The read table.
pub(crate) fn read_endpoint(endpoint: &str) -> Option<Shape> {
    Some(match endpoint {
        "sleep" => Shape::Sleep,
        "workout" => Shape::Workout,
        "session" => Shape::Session,
        "enhanced_tag" => Shape::Tag,
        "heartrate" => Shape::Heartrate,
        "daily_readiness" => Shape::Dated("daily readiness"),
        "daily_sleep" => Shape::Dated("daily sleep"),
        "daily_activity" => Shape::Dated("daily activity"),
        "daily_stress" => Shape::Dated("daily stress"),
        "daily_resilience" => Shape::Dated("daily resilience"),
        "daily_spo2" => Shape::Dated("daily SpO2"),
        "daily_cardiovascular_age" => Shape::Dated("cardiovascular age"),
        "vO2_max" => Shape::Dated("VO2 max"),
        // `blood_glucose` is partner-gated and absent from Oura's published API,
        // so a page of it is counted and named and never stored. A grant would
        // read it as readings here.
        _ => return None,
    })
}

/// The sleep types that are nights or naps. `rest` is a detection the owner
/// rejected and `deleted` one they deleted, so neither is.
const SLEEP_TYPES: [&str; 3] = ["long_sleep", "sleep", "late_nap"];

/// What one document reads as.
#[derive(Debug)]
pub(crate) enum Document {
    /// A row to stage. Its id is already the one the placer gives it.
    Row(Box<ReadRow>),
    /// A sleep Oura marks as no night: its would-be row id, Oura's type, and where
    /// a night stored under that id could be.
    NotANight {
        id: String,
        kind: String,
        oura_day: Option<NaiveDate>,
        start: Option<String>,
    },
    /// Not read, counted under this name.
    NotRead(String),
    /// Refused, counted under this stage.
    Refused(String),
}

/// A document read as a row, with what the scan needs to place and look it up.
#[derive(Debug)]
pub(crate) struct ReadRow {
    pub(crate) identity: String,
    pub(crate) row: BodyRow,
    /// Oura's own date for the document: `day`, or a tag's `start_day`.
    pub(crate) oura_day: Option<NaiveDate>,
}

/// An Oura document's identity: its endpoint and Oura's own id. A heartrate
/// reading's is its instant.
pub(crate) fn identity(endpoint: &str, id: &str) -> String {
    format!("{}\u{1f}{endpoint}\u{1f}{id}", OURA.source)
}

/// The id the placer gives the one row of an identity. One version of each Oura
/// document is staged per run, so its occurrence is always the first.
pub(crate) fn row_id(identity: &str) -> String {
    sha256_hex(&format!("{identity}\u{1f}0"))
}

fn text<'a>(document: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    document.get(key).and_then(Value::as_str)
}

fn number_text(document: &Map<String, Value>, key: &str) -> Option<String> {
    document
        .get(key)
        .filter(|value| value.is_number())
        .map(Value::to_string)
}

fn oura_date(document: &Map<String, Value>, key: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text(document, key)?, "%Y-%m-%d").ok()
}

/// A span's two ends, refusing what can't be one.
fn span(
    endpoint: &str,
    document: &Map<String, Value>,
    start: &str,
    end: &str,
) -> Result<(String, String), String> {
    let refused = |stage: &str| format!("{endpoint}: {stage}");
    let (Some(from), Some(to)) = (text(document, start), text(document, end)) else {
        return Err(refused("time"));
    };
    let (Some(first), Some(last)) = (parse_source_time(from), parse_source_time(to)) else {
        return Err(refused("time"));
    };
    if last < first {
        return Err(refused("end_before_start"));
    }
    if (last - first).num_seconds() > MAX_EXPERIENCE_SECONDS {
        return Err(refused("span_too_long"));
    }
    Ok((from.to_owned(), to.to_owned()))
}

/// A tag's code in plain words: `tag_generic_coffee` reads "coffee".
fn tag_label(code: Option<&str>) -> String {
    let Some(code) = code else {
        return "tag".to_owned();
    };
    let rest = code.strip_prefix("tag_").unwrap_or(code);
    let rest = rest.strip_prefix("generic_").unwrap_or(rest);
    let words = rest.replace('_', " ").trim().to_owned();
    if words.is_empty() {
        "tag".to_owned()
    } else {
        words
    }
}

/// The owner's words a field holds, when it holds any.
fn owner_text(document: &Map<String, Value>, key: &str) -> Option<String> {
    let words = text(document, key)?.trim();
    (!words.is_empty()).then(|| words.to_owned())
}

/// Read one document of `endpoint`. `document` is one item of a page's `data`.
pub(crate) fn read_document(endpoint: &str, document: &Value) -> Document {
    let Some(shape) = read_endpoint(endpoint) else {
        return Document::NotRead(endpoint.to_owned());
    };
    let Some(document) = document.as_object() else {
        return Document::Refused(format!("{endpoint}: not_a_document"));
    };
    match read_shape(endpoint, shape, document) {
        Ok(document) => document,
        Err(stage) => Document::Refused(stage),
    }
}

fn new_row(
    kind: RowKind,
    record_type: &str,
    label: String,
    document: &Map<String, Value>,
) -> BodyRow {
    let mut row = BodyRow::new(kind, String::new(), OURA.source, record_type, label);
    row.by = Some(BY.to_owned());
    row.fields = document
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    row
}

fn read_shape(
    endpoint: &str,
    shape: Shape,
    document: &Map<String, Value>,
) -> Result<Document, String> {
    if shape == Shape::Heartrate {
        return read_heartrate(document).map(|read| Document::Row(Box::new(read)));
    }
    let id = text(document, "id")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| format!("{endpoint}: id"))?;
    let identity = identity(endpoint, id);
    let mut oura_day = oura_date(document, "day");
    let row = match shape {
        Shape::Sleep => {
            let kind = text(document, "type");
            match kind {
                Some(kind) if SLEEP_TYPES.contains(&kind) => {}
                Some(kind @ ("deleted" | "rest")) => {
                    return Ok(Document::NotANight {
                        id: row_id(&identity),
                        kind: kind.to_owned(),
                        oura_day,
                        start: text(document, "bedtime_start").map(str::to_owned),
                    });
                }
                Some(other) => return Ok(Document::NotRead(format!("sleep {other}"))),
                None => return Ok(Document::NotRead("sleep with no type".to_owned())),
            }
            let kind = kind.expect("matched above");
            let (start, end) = span(endpoint, document, "bedtime_start", "bedtime_end")?;
            let mut row = new_row(RowKind::Experience, kind, plain_words(kind), document);
            row.experience = Some("sleep".to_owned());
            row.start = Some(start);
            row.end = Some(end);
            row
        }
        Shape::Workout => {
            let activity = text(document, "activity")
                .filter(|activity| !activity.trim().is_empty())
                .ok_or_else(|| format!("{endpoint}: activity"))?;
            let (start, end) = span(endpoint, document, "start_datetime", "end_datetime")?;
            let mut row = new_row(
                RowKind::Experience,
                activity,
                plain_words(activity),
                document,
            );
            row.experience = Some("workout".to_owned());
            row.start = Some(start);
            row.end = Some(end);
            // Documented as metres and kilocalories.
            if let Some(distance) = number_text(document, "distance") {
                row.values
                    .insert("distance".to_owned(), format!("{distance} m"));
            }
            if let Some(calories) = number_text(document, "calories") {
                row.values
                    .insert("energy".to_owned(), format!("{calories} kcal"));
            }
            // Oura's sandbox writes the literal "None" beside real nulls.
            if let Some(label) = owner_text(document, "label").filter(|label| label != "None") {
                row.words.push(label);
            }
            row
        }
        Shape::Session => {
            let kind = text(document, "type").filter(|kind| !kind.trim().is_empty());
            let (start, end) = span(endpoint, document, "start_datetime", "end_datetime")?;
            let (record_type, experience, label) = match kind {
                Some("nap") => ("nap", "sleep", "nap".to_owned()),
                Some(kind) => (
                    kind,
                    "mindfulness",
                    format!("{} session", plain_words(kind)),
                ),
                None => ("session", "mindfulness", "session".to_owned()),
            };
            let mut row = new_row(RowKind::Experience, record_type, label, document);
            row.experience = Some(experience.to_owned());
            row.start = Some(start);
            row.end = Some(end);
            row
        }
        Shape::Tag => {
            oura_day = oura_date(document, "start_day");
            let code = text(document, "tag_type_code");
            let start = text(document, "start_time")
                .filter(|start| parse_source_time(start).is_some())
                .ok_or_else(|| format!("{endpoint}: time"))?
                .to_owned();
            // A null end means the tag has no duration.
            let end = match document.get("end_time") {
                None | Some(Value::Null) => None,
                Some(_) => Some(span(endpoint, document, "start_time", "end_time")?.1),
            };
            let mut row = new_row(
                RowKind::Logged,
                code.unwrap_or("tag"),
                tag_label(code),
                document,
            );
            row.start = Some(start);
            row.end = end;
            row.words.extend(owner_text(document, "custom_name"));
            row.words.extend(owner_text(document, "comment"));
            row
        }
        Shape::Dated(label) => {
            let day = oura_day.ok_or_else(|| format!("{endpoint}: day"))?;
            // Scores and assessment words stay in `fields`: the row states the
            // document exists for its date, and no value of it.
            let mut row = new_row(RowKind::Dated, endpoint, label.to_owned(), document);
            row.date = Some(day.format("%Y-%m-%d").to_string());
            row
        }
        Shape::Heartrate => unreachable!("read above"),
    };
    Ok(Document::Row(Box::new(ReadRow {
        identity,
        row,
        oura_day,
    })))
}

/// One heartrate item. `timestamp_unix` and `producer_timestamp` are kept when
/// present and required by nothing: Oura's sandbox omits the first and adds the
/// second. Oura's `source` says what the wearer was doing, which is content.
fn read_heartrate(document: &Map<String, Value>) -> Result<ReadRow, String> {
    let at = text(document, "timestamp")
        .filter(|at| parse_source_time(at).is_some())
        .ok_or("heartrate: time")?;
    let instant = parse_source_time(at)
        .expect("checked above")
        .timestamp_millis();
    let bpm = number_text(document, "bpm").ok_or("heartrate: bpm")?;
    let mut row = new_row(
        RowKind::Reading,
        "heartrate",
        "heart rate".to_owned(),
        document,
    );
    row.at = Some(at.to_owned());
    row.value = Some(bpm);
    row.unit = Some("count/min".to_owned());
    Ok(ReadRow {
        identity: identity("heartrate", &instant.to_string()),
        row,
        oura_day: None,
    })
}

/// Why something Oura answered for wasn't written whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoleReason {
    /// A revision kept as first written: a start that crosses the owner's midnight,
    /// or a placement or record on another day.
    Deferred,
    /// A document held back because a window it could already be in couldn't be
    /// read as rows.
    HeldBack,
    /// Its window couldn't be read as rows, so it was left alone.
    Unreadable,
    /// Its day's activity record couldn't be written or brought up to date.
    RecordRefused,
    /// A reading held back because its other place couldn't be read as rows.
    ReadingHeldBack,
    /// A document moving to its new time on the same day, not yet written
    /// everywhere it was and everywhere it goes.
    Converging,
}

impl HoleReason {
    #[must_use]
    pub fn words(self) -> &'static str {
        match self {
            Self::Deferred => "changed in Oura since; the journal keeps it as first written",
            Self::HeldBack | Self::ReadingHeldBack => {
                "not added yet: it may already be in a part of the journal that couldn't be read; an import that asks for this day again tries again"
            }
            Self::Unreadable => {
                "the part of the journal it belongs in couldn't be read; an import that asks for this day again tries again"
            }
            Self::RecordRefused => {
                "it couldn't be shown on its day; an import that asks for this day again tries again"
            }
            Self::Converging => {
                "changed in Oura since, and the journal hasn't finished moving it to its new time; an import that asks for this day again finishes it"
            }
        }
    }
}

/// Something Oura answered for that the journal couldn't write whole: an endpoint
/// and an Oura day, or for heartrate the hour.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hole {
    pub endpoint: String,
    /// An Oura day, `YYYY-MM-DD`. For heartrate, the UTC hour the reading falls in,
    /// `YYYY-MM-DDTHH:00:00Z`.
    pub at: String,
    pub reason: HoleReason,
}

/// The endpoint an Oura row came from. `None` for any other source's row.
fn endpoint_of(row: &BodyRow) -> Option<&'static str> {
    if row.source != OURA.source {
        return None;
    }
    match row.kind {
        RowKind::Reading => Some("heartrate"),
        RowKind::Dated => ENDPOINTS
            .iter()
            .find(|endpoint| **endpoint == row.record_type)
            .copied(),
        RowKind::Logged => Some("enhanced_tag"),
        RowKind::Experience | RowKind::ExperienceTile => match row.experience.as_deref() {
            Some("workout") => Some("workout"),
            Some("mindfulness") => Some("session"),
            Some("sleep") if row.record_type == "nap" => Some("session"),
            Some("sleep") => Some("sleep"),
            _ => None,
        },
        // A moved row marks a place, so its absence is no hole.
        RowKind::Stage | RowKind::Event | RowKind::Count | RowKind::Moved => None,
    }
}

/// The date part of a time as Oura wrote it, which is the wearer's own.
fn written_date(text: Option<&str>) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text?.get(..10)?, "%Y-%m-%d").ok()
}

/// The hour a heartrate reading falls in, in UTC.
fn reading_hour(row: &BodyRow) -> Option<String> {
    let at = parse_source_time(row.at.as_deref()?)?.with_timezone(&chrono::Utc);
    Some(at.format("%Y-%m-%dT%H:00:00Z").to_string())
}

/// Where an Oura row's absence would be a hole: its endpoint and Oura day, or the
/// hour of a reading. A tile names no Oura day, so it gives every date its span
/// was written across, which can only claim more holes than there are. Any other
/// source's row gives none.
#[must_use]
pub fn hole_points(row: &BodyRow) -> Vec<(&'static str, String)> {
    let Some(endpoint) = endpoint_of(row) else {
        return Vec::new();
    };
    let day = |key: &str| {
        row.fields
            .get(key)
            .and_then(Value::as_str)
            .and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok())
            .or_else(|| written_date(row.start.as_deref()))
    };
    let dates: Vec<NaiveDate> = match row.kind {
        RowKind::Reading => {
            return reading_hour(row)
                .map(|hour| vec![(endpoint, hour)])
                .unwrap_or_default();
        }
        RowKind::Dated => row
            .date
            .as_deref()
            .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
            .into_iter()
            .collect(),
        RowKind::Logged => day("start_day").into_iter().collect(),
        RowKind::ExperienceTile => {
            let (Some(first), Some(last)) = (
                written_date(row.start.as_deref()),
                written_date(row.end.as_deref()),
            ) else {
                return Vec::new();
            };
            let mut dates = Vec::new();
            let mut date = first;
            while date <= last && dates.len() <= 15 {
                dates.push(date);
                date += Duration::days(1);
            }
            dates
        }
        _ => day("day").into_iter().collect(),
    };
    dates
        .into_iter()
        .map(|date| (endpoint, date.format("%Y-%m-%d").to_string()))
        .collect()
}

/// The holes of rows that weren't written whole, for one reason.
pub fn holes_of<'a>(
    rows: impl IntoIterator<Item = &'a BodyRow>,
    reason: HoleReason,
) -> BTreeSet<Hole> {
    rows.into_iter()
        .flat_map(hole_points)
        .map(|(endpoint, at)| Hole {
            endpoint: endpoint.to_owned(),
            at,
            reason,
        })
        .collect()
}

#[cfg(test)]
#[path = "oura_tests.rs"]
mod tests;
