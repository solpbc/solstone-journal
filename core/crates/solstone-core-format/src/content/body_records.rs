// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Body records: the rows of a body window's `body.jsonl`.
//!
//! `docs/BODY_SEGMENTS.md` is the contract. One window holds whatever fell in
//! it: experiences, readings, interval counts, things the owner logged and
//! dated assertions, each row stating its own times and source. Only an
//! experience renders into search; every row renders into one attributed line
//! for an agent that reads the window. Code writes every word here, restating
//! what the source recorded and computing nothing from it.

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{JsonObject, ProducedChunks, recorded_chunk};

/// The one file a body window holds.
pub const BODY_RECORDS_FILE: &str = "body.jsonl";
/// The streams body records are written to. Strava keeps its own tiles.
pub const BODY_RECORD_STREAMS: [&str; 2] = ["import.apple_health", "import.oura"];
/// A dated assertion's window: a whole day, so it reads as covering the date
/// and never as something that happened at midnight.
pub const DATED_WINDOW: &str = "000000_86400";
const RECORD_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowKind {
    Experience,
    ExperienceTile,
    Stage,
    Event,
    Reading,
    Count,
    Logged,
    Dated,
}

/// One row. Fields are declared in sorted order, so a row's bytes are stable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodyRow {
    /// The rule a source-assembled experience was built by, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assembled: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The source's own name for the app or device that recorded this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experience: Option<String>,
    /// Every field the source gave. Read by agents, never indexed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, Value>,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u64>,
    pub kind: RowKind,
    /// The type in plain words.
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
    /// The import that first wrote this row. Set once, kept through edits,
    /// outside identity, never indexed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(rename = "type")]
    pub record_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub v: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Values the source's reader chose to restate for search, in words.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, String>,
    /// The owner's own words on this record.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
}

impl BodyRow {
    #[must_use]
    pub fn new(kind: RowKind, id: String, source: &str, record_type: &str, label: String) -> Self {
        Self {
            assembled: None,
            at: None,
            by: None,
            count: None,
            date: None,
            end: None,
            experience: None,
            fields: BTreeMap::new(),
            id,
            index: None,
            kind,
            label,
            of: None,
            run: None,
            source: source.to_owned(),
            start: None,
            record_type: record_type.to_owned(),
            unit: None,
            v: RECORD_VERSION,
            value: None,
            values: BTreeMap::new(),
            words: Vec::new(),
        }
    }

    /// When this row begins. A dated row has no time at all.
    #[must_use]
    pub fn begins(&self) -> Option<&str> {
        self.at.as_deref().or(self.start.as_deref())
    }

    fn ordering_key(&self) -> (Option<i64>, &str) {
        let instant = self
            .begins()
            .and_then(parse_source_time)
            .map(|time| time.timestamp_millis());
        (instant, self.id.as_str())
    }
}

/// A source's name in the owner's words.
#[must_use]
pub fn source_words(source: &str) -> &str {
    match source {
        "apple_health" => "Apple Health",
        "oura" => "Oura",
        other => other,
    }
}

/// A source's label as agents see it: what they pass to name it. It matches how
/// the journal labels every imported stream.
#[must_use]
pub fn source_label(source: &str) -> String {
    format!("{} · imported", source_words(source))
}

/// Apple's `2026-10-10 06:31:10 -0600`, or RFC 3339, keeping the offset.
#[must_use]
pub fn parse_source_time(text: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M:%S %z")
        .ok()
        .or_else(|| DateTime::parse_from_rfc3339(text.trim()).ok())
}

/// An identifier in plain words: `HKQuantityTypeIdentifierHeartRate` reads
/// "heart rate", `HKCategoryValueSleepAnalysisAsleepCore` reads
/// "sleep analysis asleep core".
#[must_use]
pub fn plain_words(identifier: &str) -> String {
    let mut text = identifier;
    for prefix in [
        "HKQuantityTypeIdentifier",
        "HKCategoryTypeIdentifier",
        "HKWorkoutActivityType",
        "HKCategoryValue",
        "HKWorkoutEventType",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest;
            break;
        }
    }
    let mut words = String::with_capacity(text.len() + 4);
    let mut previous_lower = false;
    for character in text.chars() {
        if character == '_' {
            words.push(' ');
            previous_lower = false;
            continue;
        }
        if character.is_uppercase() && previous_lower {
            words.push(' ');
        }
        previous_lower = character.is_lowercase() || character.is_ascii_digit();
        words.extend(character.to_lowercase());
    }
    words
}

/// Rows in the order a window stores them: by time, then id. Dated rows, which
/// have no time, come first.
pub fn sort_rows(rows: &mut [BodyRow]) {
    rows.sort_by(|left, right| left.ordering_key().cmp(&right.ordering_key()));
}

/// A window's file bytes.
#[must_use]
pub fn encode_rows(rows: &[BodyRow]) -> String {
    let mut text = String::new();
    for row in rows {
        text.push_str(&serde_json::to_string(row).expect("a body row serializes"));
        text.push('\n');
    }
    text
}

/// A window's rows, or the first line that isn't one. A caller that merges into
/// a window refuses it rather than dropping what it couldn't read.
pub fn decode_rows(text: &str) -> Result<Vec<BodyRow>, serde_json::Error> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect()
}

pub(super) fn render(records: &[JsonObject]) -> ProducedChunks {
    let mut agent_override = None;
    let chunks = records
        .iter()
        .filter_map(|record| {
            let row: BodyRow = serde_json::from_value(Value::Object(record.clone())).ok()?;
            agent_override.get_or_insert_with(|| format!("import.{}", row.source));
            if row.kind != RowKind::Experience {
                return None;
            }
            let occurrence = row
                .begins()
                .and_then(parse_source_time)
                .map(|time| time.timestamp_millis())
                .unwrap_or(0);
            Some(recorded_chunk(search_text(&row), occurrence, record))
        })
        .collect();
    ProducedChunks {
        chunks,
        agent_override,
        header: None,
        error: None,
        warnings: Vec::new(),
    }
}

fn clock_span(row: &BodyRow) -> String {
    let format = |text: &str| {
        parse_source_time(text).map_or_else(
            || text.to_owned(),
            |time| time.format("%Y-%m-%d %H:%M %z").to_string(),
        )
    };
    match (row.start.as_deref(), row.end.as_deref()) {
        (Some(start), Some(end)) => format!("{} to {}", format(start), format(end)),
        (Some(start), None) => format(start),
        _ => String::new(),
    }
}

/// An experience as search holds it: names, words, times and the values its
/// reader chose. `fields` never reaches this text, so a key nobody listed can't.
fn search_text(row: &BodyRow) -> String {
    let source = source_words(&row.source);
    let kind = row.experience.as_deref().unwrap_or("experience");
    let mut lines = vec![format!("## {source} {kind}: {}", row.label)];
    lines.push(clock_span(row));
    if let Some(assembled) = &row.assembled {
        lines.push(assembled.clone());
    }
    if !row.words.is_empty() {
        lines.push(row.words.join(" · "));
    }
    let attribution = match row.by.as_deref() {
        Some(by) => format!("as {source} recorded it, from {by}"),
        None => format!("as {source} recorded it"),
    };
    if row.values.is_empty() {
        lines.push(attribution);
    } else {
        let values = row
            .values
            .iter()
            .map(|(name, value)| format!("{name} {value}"))
            .collect::<Vec<_>>()
            .join(" · ");
        lines.push(format!("{attribution}: {values}"));
    }
    lines.join("\n")
}

fn field_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// One row as an agent reads it, attributed, with every field the source gave.
#[must_use]
pub fn render_row(row: &BodyRow) -> String {
    let source = source_words(&row.source);
    let attribution = match row.by.as_deref() {
        Some(by) => format!("{source}, {by}"),
        None => source.to_owned(),
    };
    let span = match (row.start.as_deref(), row.end.as_deref()) {
        (Some(start), Some(end)) => format!("{start} to {end}"),
        (Some(start), None) => start.to_owned(),
        _ => String::new(),
    };
    let measured = match (row.value.as_deref(), row.unit.as_deref()) {
        (Some(value), Some(unit)) => format!("{} {value} {unit}", row.label),
        (Some(value), None) if value.starts_with("HK") => {
            format!("{}: {}", row.label, plain_words(value))
        }
        (Some(value), None) => format!("{} {value}", row.label),
        _ => row.label.clone(),
    };
    let what = match row.kind {
        RowKind::Reading => format!("{} {measured}", row.at.as_deref().unwrap_or_default()),
        RowKind::Count => format!("{span} {measured}"),
        RowKind::Logged => format!("{span} logged {measured}"),
        RowKind::Dated => format!("for {} {measured}", row.date.as_deref().unwrap_or_default()),
        RowKind::Stage => format!("{span} sleep stage {measured}"),
        RowKind::Event => format!("{span} workout event {measured}"),
        RowKind::Experience => {
            let kind = row.experience.as_deref().unwrap_or("experience");
            let mut text = format!("{kind} {}, {span}", row.label);
            if let Some(assembled) = &row.assembled {
                text.push_str(&format!(" ({assembled})"));
            }
            text
        }
        RowKind::ExperienceTile => {
            let kind = row.experience.as_deref().unwrap_or("experience");
            format!(
                "part {} of {} of {kind} {}, {span}",
                row.index.map_or(0, |index| index + 1),
                row.count.unwrap_or(0),
                row.label
            )
        }
    };
    let mut parts = vec![what, attribution];
    if !row.words.is_empty() {
        parts.push(row.words.join(", "));
    }
    parts.extend(
        row.values
            .iter()
            .map(|(name, value)| format!("{name}: {value}")),
    );
    parts.extend(
        row.fields
            .iter()
            .map(|(name, value)| format!("{name}: {}", field_text(value))),
    );
    parts.join(" · ")
}

/// One stored line as an agent reads it. A line that isn't a row reads as itself,
/// so a damaged file shows the damage rather than hiding it.
#[must_use]
pub fn render_line(line: &str) -> String {
    serde_json::from_str::<BodyRow>(line).map_or_else(|_| line.to_owned(), |row| render_row(&row))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::content::{ContentResolution, Family, classify, produce_chunks};

    const REL: &str = "20261010/import.apple_health/063000_300/body.jsonl";

    fn experience() -> BodyRow {
        let mut row = BodyRow::new(
            RowKind::Experience,
            "aa".into(),
            "apple_health",
            "HKWorkoutActivityTypeRunning",
            "running".into(),
        );
        row.experience = Some("workout".into());
        row.start = Some("2026-10-10 06:31:00 -0600".into());
        row.end = Some("2026-10-10 07:20:00 -0600".into());
        row.by = Some("Synthetic Watch".into());
        row.values.insert("distance".into(), "8.1 km".into());
        row.fields.insert(
            "device".into(),
            json!("<<HKDevice: 0x1>, name:Watch, uuid:ABC-DEF>"),
        );
        row
    }

    fn reading() -> BodyRow {
        let mut row = BodyRow::new(
            RowKind::Reading,
            "bb".into(),
            "apple_health",
            "HKQuantityTypeIdentifierHeartRate",
            "heart rate".into(),
        );
        row.at = Some("2026-10-10 06:31:10 -0600".into());
        row.value = Some("4817".into());
        row.unit = Some("count/min".into());
        row.by = Some("Synthetic Watch".into());
        row.fields
            .insert("HKMetadataKeySyncIdentifier".into(), json!("sync-123"));
        row
    }

    #[test]
    fn only_an_experience_reaches_search_and_never_its_raw_fields() {
        assert_eq!(
            classify(REL),
            ContentResolution::Indexed(Family::BodyRecords)
        );
        assert_ne!(
            classify("20261010/import.strava/063000_300/body.jsonl"),
            ContentResolution::Indexed(Family::BodyRecords)
        );
        let text = encode_rows(&[experience(), reading()]);
        let produced = produce_chunks(Family::BodyRecords, REL, &text);
        assert_eq!(produced.chunks.len(), 1);
        assert_eq!(
            produced.agent_override.as_deref(),
            Some("import.apple_health")
        );
        let chunk = &produced.chunks[0].content;
        assert!(chunk.contains("running"));
        assert!(chunk.contains("Synthetic Watch"));
        assert!(chunk.contains("8.1 km"));
        assert!(!chunk.contains("4817"));
        assert!(!chunk.contains("HKDevice"));
        assert!(!chunk.contains("ABC-DEF"));
        assert!(!chunk.contains("sync-123"));
    }

    #[test]
    fn every_row_reads_as_one_attributed_line_with_every_field() {
        let line = render_row(&reading());
        assert!(line.contains("2026-10-10 06:31:10 -0600"));
        assert!(line.contains("heart rate 4817 count/min"));
        assert!(line.contains("Apple Health, Synthetic Watch"));
        assert!(line.contains("sync-123"));
        assert!(render_row(&experience()).contains("uuid:ABC-DEF"));
    }

    #[test]
    fn a_dated_row_states_its_date_and_no_time() {
        let mut row = BodyRow::new(
            RowKind::Dated,
            "cc".into(),
            "oura",
            "daily_readiness",
            "readiness score".into(),
        );
        row.date = Some("2026-10-10".into());
        row.value = Some("82".into());
        let text = render_row(&row);
        assert!(text.starts_with("for 2026-10-10 readiness score 82"));
        assert!(!text.contains("00:00"));
        assert!(row.begins().is_none());
        let bytes = encode_rows(std::slice::from_ref(&row));
        assert_eq!(decode_rows(&bytes).unwrap(), vec![row]);
        assert!(
            produce_chunks(Family::BodyRecords, REL, &bytes)
                .chunks
                .is_empty()
        );
    }

    #[test]
    fn rows_sort_by_instant_across_offsets_and_keep_stable_bytes() {
        let mut early = reading();
        early.id = "zz".into();
        early.at = Some("2026-10-10 08:00:00 +0100".into());
        let mut late = reading();
        late.id = "aa".into();
        late.at = Some("2026-10-10 06:31:10 -0600".into());
        let mut rows = vec![late.clone(), early.clone()];
        sort_rows(&mut rows);
        assert_eq!(rows, vec![early, late]);
        let bytes = encode_rows(&rows);
        assert_eq!(encode_rows(&decode_rows(&bytes).unwrap()), bytes);
        assert!(decode_rows("{not json\n").is_err());
    }

    #[test]
    fn identifiers_read_in_plain_words() {
        assert_eq!(
            plain_words("HKQuantityTypeIdentifierHeartRate"),
            "heart rate"
        );
        assert_eq!(plain_words("HKWorkoutActivityTypeRunning"), "running");
        assert_eq!(plain_words("HKCategoryTypeIdentifierHeadache"), "headache");
        assert_eq!(
            plain_words("HKCategoryValueSleepAnalysisAsleepCore"),
            "sleep analysis asleep core"
        );
    }
}
