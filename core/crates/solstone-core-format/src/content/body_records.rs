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

/// The civil grid a numeric body segment belongs to. Extra candidate names
/// distinguish segments on that grid; they do not lengthen the recorded span.
#[must_use]
pub fn grid_length(key: &str) -> Option<u64> {
    let length = key.split_once('_')?.1.parse::<u64>().ok()?;
    [86_400, 3_600, 300]
        .into_iter()
        .find(|grid| length >= *grid)
}

/// Whether two physical segments occupy the same civil grid slot.
#[must_use]
pub fn same_grid(left: &str, right: &str) -> bool {
    left.split_once('_').zip(right.split_once('_')).is_some_and(
        |((left_clock, _), (right_clock, _))| {
            left_clock == right_clock
                && grid_length(left).is_some()
                && grid_length(left) == grid_length(right)
        },
    )
}

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
    /// A row that was here and has moved: the old row's id, its document's id as
    /// `of`, and its source, type, label and run, nothing else. Every reader
    /// skips it, and a window holding only these reads as holding nothing.
    Moved,
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
    /// The source's words on this record.
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

/// How much a window's key says its rows are the copy to read, when an archive
/// merge left a second copy beside them (lower is better). The importer writes
/// to `<clock>_<len + 1>` whenever that key holds records, otherwise to the grid
/// key, so those two are the live ones, the step-around first; a key further
/// along is never written again.
#[must_use]
pub fn copy_rank(key: &str) -> u8 {
    let Some(length) = key
        .split_once('_')
        .and_then(|(_, length)| length.parse::<u64>().ok())
    else {
        return 2;
    };
    let grid = grid_length(key).unwrap_or(length);
    match length - grid {
        1 => 0,
        0 => 1,
        _ => 2,
    }
}

/// A day's windows of one stream, with their rows, keeping each row once when
/// an archive merge left a second copy: a row id in two windows at one clock
/// reads from the better copy by [`copy_rank`], then key order. `moved` rows are
/// dropped first, so a mark never hides a row's new home.
#[must_use]
pub fn one_copy_per_row(mut windows: Vec<(String, Vec<BodyRow>)>) -> Vec<(String, Vec<BodyRow>)> {
    windows
        .sort_by(|left, right| (copy_rank(&left.0), &left.0).cmp(&(copy_rank(&right.0), &right.0)));
    let mut seen = std::collections::BTreeSet::new();
    for (name, rows) in &mut windows {
        let clock = name.split('_').next().unwrap_or_default().to_owned();
        rows.retain(|row| {
            row.kind != RowKind::Moved && seen.insert((clock.clone(), row.id.clone()))
        });
    }
    windows
}

/// A source's name in the owner's words.
#[must_use]
pub fn source_words(source: &str) -> &str {
    match source {
        "apple_health" => "Apple Health",
        "oura" => "Oura",
        "strava" => "Strava",
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

/// The owner's own words on an account, per source: what the owner wrote, never
/// a name the source made up. Apple's export carries none, so it gives
/// nothing. Elsewhere, a generated default ("Morning Run", "Lunch Ride", a bare
/// type name) is the source's, and any other name or description is the owner's.
/// Strava's words are a workout's name, description and private note, and its
/// defaults follow the account's language, so German ones ("Lauf am Morgen")
/// are its own too.
#[must_use]
pub fn owner_words(source: &str, row: &BodyRow) -> Vec<String> {
    if source == "apple_health" {
        return Vec::new();
    }
    row.words
        .iter()
        .map(|words| words.trim())
        .filter(|words| {
            !words.is_empty()
                && !generated_name(words, &row.record_type)
                && !(source == "strava" && german_generated_name(words))
        })
        .map(str::to_owned)
        .collect()
}

/// The owner's gear on an account, as the source names it: for Strava, a
/// workout's `Activity Gear` (`Aktivitätsausrüstung` in German), a catalogue
/// brand and model with the owner's own nickname. Other sources name none.
#[must_use]
pub fn owner_gear(source: &str, row: &BodyRow) -> Vec<String> {
    use super::strava_body::{GEAR_HEADERS, strava_field};
    if source != "strava" {
        return Vec::new();
    }
    strava_field(row.fields.get("strava_fields"), &GEAR_HEADERS)
        .map(str::to_owned)
        .into_iter()
        .collect()
}

/// A name Strava writes itself in German: one word and a part of the day,
/// `<word> am Morgen` (or `Vormittag`, `Mittag`, `Nachmittag`, `Abend`), or
/// `<word> in der Nacht`, case aside.
fn german_generated_name(words: &str) -> bool {
    const PARTS: [&str; 5] = ["morgen", "vormittag", "mittag", "nachmittag", "abend"];
    let lower = words.to_lowercase();
    let tokens = lower.split_whitespace().collect::<Vec<_>>();
    match tokens.as_slice() {
        [_, "am", part] => PARTS.contains(part),
        [_, "in", "der", "nacht"] => true,
        _ => false,
    }
}

/// Whether the owner entered a row themselves, by the source's own mark. Apple
/// marks one with `HKWasUserEntered`. Oura marks a workout added by hand with
/// `source: manual`, and every tag is one: tags are what Oura's owners enter.
/// Nothing else counts as the owner's.
#[must_use]
pub fn user_entered(row: &BodyRow) -> bool {
    match row.source.as_str() {
        "oura" => match row.kind {
            RowKind::Logged => true,
            RowKind::Experience => {
                row.experience.as_deref() == Some("workout")
                    && row.fields.get("source").and_then(Value::as_str) == Some("manual")
            }
            _ => false,
        },
        _ => row
            .fields
            .get("metadata")
            .and_then(|metadata| metadata.get("HKWasUserEntered"))
            .and_then(Value::as_str)
            .is_some_and(|value| value == "1"),
    }
}

/// A name a source writes itself: a part of the day and a type, or the type alone.
fn generated_name(words: &str, record_type: &str) -> bool {
    const PARTS: [&str; 6] = [
        "morning",
        "lunch",
        "afternoon",
        "evening",
        "night",
        "early morning",
    ];
    let lower = words.to_lowercase();
    let plain = |text: &str| plain_words(text).to_lowercase();
    let kind = plain(record_type);
    let rest = PARTS
        .iter()
        .find_map(|part| lower.strip_prefix(part).map(str::trim))
        .unwrap_or(&lower);
    if rest == kind || rest == record_type.to_lowercase() {
        return true;
    }
    let mapped = mapped_workout_type("strava", rest);
    mapped.is_some() && mapped == mapped_workout_type("strava", record_type)
}

/// Who recorded an account, as an opaque id: its stream and `by`. Two accounts
/// from one recorder never group.
#[must_use]
pub fn recorder_id(stream: &str, by: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{stream}\u{1f}{}", by.unwrap_or_default()).as_bytes());
    digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A workout's type in one shared vocabulary, so one recorder's run can pair
/// with another's: Apple's `HKWorkoutActivityTypeRunning`, Strava's `Run` and
/// Oura's `running` are all `running`. Underscores are ignored, so Oura's
/// `strength_training` reads as Apple's `StrengthTraining`. A type outside the
/// table pairs with nothing.
#[must_use]
pub fn mapped_workout_type(source: &str, record_type: &str) -> Option<&'static str> {
    let name = match source {
        "apple_health" => record_type.strip_prefix("HKWorkoutActivityType")?,
        _ => record_type,
    };
    let lower: String = name
        .chars()
        .filter(|character| *character != '_')
        .map(|character| character.to_ascii_lowercase())
        .collect();
    Some(match lower.as_str() {
        "running" | "run" | "trailrun" | "virtualrun" => "running",
        "cycling" | "ride" | "virtualride" | "ebikeride" => "cycling",
        "walking" | "walk" => "walking",
        "hiking" | "hike" => "hiking",
        "swimming" | "swim" => "swimming",
        "rowing" | "rowingmachine" => "rowing",
        "yoga" => "yoga",
        "elliptical" => "elliptical",
        "highintensityintervaltraining" | "hiit" => "hiit",
        "traditionalstrengthtraining"
        | "functionalstrengthtraining"
        | "weighttraining"
        | "strengthtraining" => "strength",
        _ => return None,
    })
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

/// Whether a window's file holds rows, every one of them `moved`: a window that
/// stays on disk and reads as holding nothing. A line that isn't a row leaves the
/// window there, so the damage shows.
#[must_use]
pub fn only_moved_rows(text: &str) -> bool {
    let mut lines = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .peekable();
    lines.peek().is_some()
        && lines.all(|line| {
            serde_json::from_str::<BodyRow>(line).is_ok_and(|row| row.kind == RowKind::Moved)
        })
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
    // A source that is its own recorder, as Oura is, is named once.
    let attribution = match row.by.as_deref() {
        Some(by) if by != source => format!("as {source} recorded it, from {by}"),
        _ => format!("as {source} recorded it"),
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
/// A `moved` row reads as nothing: what it marks is in another window now.
#[must_use]
pub fn render_row(row: &BodyRow) -> String {
    let source = source_words(&row.source);
    let attribution = match row.by.as_deref() {
        // A source that is its own recorder, as Oura is, is named once.
        Some(by) if by != source => format!("{source}, {by}"),
        _ => source.to_owned(),
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
        RowKind::Moved => return String::new(),
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
    fn row(kind: RowKind, id: &str) -> BodyRow {
        BodyRow::new(kind, id.to_owned(), "apple_health", "t", "l".to_owned())
    }

    fn ids(windows: &[(String, Vec<BodyRow>)]) -> Vec<(String, Vec<String>)> {
        windows
            .iter()
            .map(|(name, rows)| {
                (
                    name.clone(),
                    rows.iter().map(|row| row.id.clone()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn the_copy_the_importer_still_writes_to_is_the_one_read() {
        assert!(copy_rank("090000_3601") < copy_rank("090000_3600"));
        assert!(copy_rank("090000_3600") < copy_rank("090000_3602"));
        assert!(copy_rank("090000_301") < copy_rank("090000_300"));
        assert!(copy_rank("000000_86401") < copy_rank("000000_86400"));
        assert_eq!(copy_rank("not a key"), 2);
    }

    #[test]
    fn facet_siblings_share_their_grid_without_sharing_other_grid_lengths() {
        for (left, right, length) in [
            ("063000_300", "063000_302", 300),
            ("060000_3600", "060000_3610", 3600),
            ("000000_86400", "000000_86405", 86400),
        ] {
            assert!(same_grid(left, right));
            assert_eq!(grid_length(right), Some(length));
        }
        assert!(!same_grid("060000_300", "060000_3600"));
        assert!(!same_grid("060000_3600", "070000_3602"));
        assert_eq!(grid_length("not a key"), None);
    }

    #[test]
    fn a_row_in_two_windows_reads_once_from_the_better_copy() {
        let read = one_copy_per_row(vec![
            (
                "090000_3600".to_owned(),
                vec![row(RowKind::Reading, "r1"), row(RowKind::Reading, "r2")],
            ),
            ("090000_3601".to_owned(), vec![row(RowKind::Reading, "r1")]),
        ]);
        assert_eq!(
            ids(&read),
            [
                ("090000_3601".to_owned(), vec!["r1".to_owned()]),
                ("090000_3600".to_owned(), vec!["r2".to_owned()]),
            ]
        );
    }

    #[test]
    fn a_moved_mark_never_hides_a_rows_new_home() {
        let read = one_copy_per_row(vec![
            ("090000_3601".to_owned(), vec![row(RowKind::Moved, "r1")]),
            ("090000_3600".to_owned(), vec![row(RowKind::Reading, "r1")]),
        ]);
        assert_eq!(
            ids(&read),
            [
                ("090000_3601".to_owned(), Vec::new()),
                ("090000_3600".to_owned(), vec!["r1".to_owned()]),
            ]
        );
    }

    #[test]
    fn a_row_live_at_two_clocks_isnt_a_merged_copy_and_reads_at_both() {
        let read = one_copy_per_row(vec![
            ("090000_3600".to_owned(), vec![row(RowKind::Reading, "r1")]),
            ("100000_3600".to_owned(), vec![row(RowKind::Reading, "r1")]),
        ]);
        assert_eq!(read.iter().map(|(_, rows)| rows.len()).sum::<usize>(), 2);
    }

    #[test]
    fn owner_words_are_only_what_the_owner_wrote() {
        let mut row = BodyRow::new(
            RowKind::Experience,
            "s1".to_owned(),
            "strava",
            "Run",
            "Run".to_owned(),
        );
        for generated in [
            "Morning Run",
            "Lunch Run",
            "Evening Run",
            "Run",
            "  Night Run ",
        ] {
            row.words = vec![generated.to_owned()];
            assert!(owner_words("strava", &row).is_empty(), "{generated}");
        }
        let mut ride = BodyRow::new(
            RowKind::Experience,
            "s2".to_owned(),
            "strava",
            "Ride",
            "Ride".to_owned(),
        );
        ride.words = vec!["Lunch Ride".to_owned(), "Hill repeats with Sam".to_owned()];
        assert_eq!(owner_words("strava", &ride), ["Hill repeats with Sam"]);
        // Apple's export carries no owner-written words, whatever a row holds.
        let mut apple = BodyRow::new(
            RowKind::Experience,
            "a1".to_owned(),
            "apple_health",
            "HKWorkoutActivityTypeRunning",
            "Running".to_owned(),
        );
        apple.words = vec!["Loop 10".to_owned()];
        assert!(owner_words("apple_health", &apple).is_empty());
    }

    use serde_json::json;

    use super::*;
    use crate::content::{ContentResolution, Family, classify, produce_chunks};

    fn oura(kind: RowKind, record_type: &str, label: &str) -> BodyRow {
        let mut row = BodyRow::new(kind, "o1".into(), "oura", record_type, label.into());
        row.by = Some("Oura".into());
        row
    }

    #[test]
    fn owner_words_for_oura_are_a_workouts_label_and_a_tags_name_and_comment() {
        let mut run = oura(RowKind::Experience, "running", "running");
        run.experience = Some("workout".into());
        run.words = vec!["synthetic hill loop".into()];
        assert_eq!(owner_words("oura", &run), ["synthetic hill loop"]);
        let mut sauna = oura(RowKind::Logged, "custom", "custom");
        sauna.words = vec!["synthetic sauna".into(), "after the long run".into()];
        assert_eq!(
            owner_words("oura", &sauna),
            ["synthetic sauna", "after the long run"]
        );
        // A workout with no label of its own has no words.
        run.words.clear();
        assert!(owner_words("oura", &run).is_empty());
    }

    #[test]
    fn a_row_is_the_owners_own_entry_only_where_its_source_marks_it() {
        // Apple: `HKWasUserEntered`, on any kind of row, and nothing else.
        let mut apple = BodyRow::new(
            RowKind::Logged,
            "a1".into(),
            "apple_health",
            "HKQuantityTypeIdentifierDietaryWater",
            "dietary water".into(),
        );
        assert!(!user_entered(&apple));
        apple
            .fields
            .insert("metadata".into(), json!({"HKWasUserEntered": "0"}));
        assert!(!user_entered(&apple));
        apple
            .fields
            .insert("metadata".into(), json!({"HKWasUserEntered": "1"}));
        assert!(user_entered(&apple));
        // Oura: every tag.
        assert!(user_entered(&oura(
            RowKind::Logged,
            "tag_generic_caffeine",
            "caffeine"
        )));
        // A workout only when Oura says it was added by hand.
        let mut workout = oura(RowKind::Experience, "running", "running");
        workout.experience = Some("workout".into());
        assert!(!user_entered(&workout));
        for detected in ["autodetected", "confirmed", "workout_heart_rate"] {
            workout.fields.insert("source".into(), json!(detected));
            assert!(!user_entered(&workout), "{detected}");
        }
        workout.fields.insert("source".into(), json!("manual"));
        assert!(user_entered(&workout));
        // Nothing else Oura sends, whatever its fields say.
        let mut session = oura(RowKind::Experience, "meditation", "meditation session");
        session.experience = Some("mindfulness".into());
        session.fields.insert("source".into(), json!("manual"));
        assert!(!user_entered(&session));
        let mut reading = oura(RowKind::Reading, "heartrate", "heart rate");
        reading.fields.insert("source".into(), json!("manual"));
        assert!(!user_entered(&reading));
        assert!(!user_entered(&oura(
            RowKind::Dated,
            "daily_readiness",
            "daily readiness"
        )));
        // Apple's mark on an Oura row isn't Oura's.
        let mut marked = oura(RowKind::Experience, "running", "running");
        marked.experience = Some("workout".into());
        marked
            .fields
            .insert("metadata".into(), json!({"HKWasUserEntered": "1"}));
        assert!(!user_entered(&marked));
    }

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
    fn a_source_that_is_its_own_recorder_is_named_once() {
        let mut row = experience();
        row.source = "oura".into();
        row.by = Some("Oura".into());
        let line = render_row(&row);
        assert!(
            line.contains("Oura") && !line.contains("Oura, Oura"),
            "{line}"
        );
        let text = search_text(&row);
        assert!(text.contains("as Oura recorded it"), "{text}");
        assert!(!text.contains("from Oura"), "{text}");
        // Apple names its device as well.
        assert!(render_row(&experience()).contains("Apple Health, Synthetic Watch"));
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

    fn moved(id: &str) -> BodyRow {
        let mut row = BodyRow::new(
            RowKind::Moved,
            id.into(),
            "oura",
            "running",
            "running".into(),
        );
        row.of = Some("o1".into());
        row.run = Some("20261104_120000".into());
        row
    }

    #[test]
    fn a_moved_row_reads_as_nothing_and_holds_only_what_it_replaces() {
        let row = moved("o1");
        let bytes = encode_rows(std::slice::from_ref(&row));
        assert_eq!(
            bytes,
            "{\"id\":\"o1\",\"kind\":\"moved\",\"label\":\"running\",\"of\":\"o1\",\"run\":\"20261104_120000\",\"source\":\"oura\",\"type\":\"running\",\"v\":1}\n"
        );
        assert_eq!(decode_rows(&bytes).unwrap(), vec![row.clone()]);
        assert_eq!(render_row(&row), "");
        assert_eq!(render_line(bytes.trim_end()), "");
        // Search holds no chunk for it, beside an experience that keeps one.
        let mut live = experience();
        live.source = "oura".into();
        let produced = produce_chunks(Family::BodyRecords, REL, &bytes);
        assert!(produced.chunks.is_empty());
        let both = encode_rows(&[row, live]);
        assert_eq!(
            produce_chunks(Family::BodyRecords, REL, &both).chunks.len(),
            1
        );
    }

    #[test]
    fn a_window_of_only_moved_rows_is_one_and_any_other_line_keeps_it() {
        let marks = encode_rows(&[moved("o1"), moved("o1@20261010/070000_3600")]);
        assert!(only_moved_rows(&marks));
        assert!(!only_moved_rows(""));
        assert!(!only_moved_rows(&format!(
            "{marks}{}",
            encode_rows(&[reading()])
        )));
        assert!(!only_moved_rows(&format!("{marks}{{not a row\n")));
    }

    #[test]
    fn workout_types_map_alike_with_or_without_underscores() {
        // Oura names its activities in snake case.
        for (oura, shared) in [
            ("running", "running"),
            ("cycling", "cycling"),
            ("walking", "walking"),
            ("hiking", "hiking"),
            ("swimming", "swimming"),
            ("rowing", "rowing"),
            ("yoga", "yoga"),
            ("elliptical", "elliptical"),
            ("hiit", "hiit"),
            ("strength_training", "strength"),
            ("weight_training", "strength"),
            ("high_intensity_interval_training", "hiit"),
        ] {
            assert_eq!(mapped_workout_type("oura", oura), Some(shared), "{oura}");
        }
        assert_eq!(mapped_workout_type("oura", "dancing"), None);
        assert_eq!(mapped_workout_type("oura", "_"), None);
        // Apple's and Strava's names map as they did.
        for (source, name, shared) in [
            (
                "apple_health",
                "HKWorkoutActivityTypeRunning",
                Some("running"),
            ),
            (
                "apple_health",
                "HKWorkoutActivityTypeTraditionalStrengthTraining",
                Some("strength"),
            ),
            (
                "apple_health",
                "HKWorkoutActivityTypeHighIntensityIntervalTraining",
                Some("hiit"),
            ),
            (
                "apple_health",
                "HKWorkoutActivityTypeRowing",
                Some("rowing"),
            ),
            ("apple_health", "HKWorkoutActivityTypeDance", None),
            ("apple_health", "Running", None),
            ("strava", "Run", Some("running")),
            ("strava", "TrailRun", Some("running")),
            ("strava", "EBikeRide", Some("cycling")),
            ("strava", "WeightTraining", Some("strength")),
            ("strava", "RowingMachine", Some("rowing")),
            ("strava", "Kayaking", None),
        ] {
            assert_eq!(mapped_workout_type(source, name), shared, "{source} {name}");
        }
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
