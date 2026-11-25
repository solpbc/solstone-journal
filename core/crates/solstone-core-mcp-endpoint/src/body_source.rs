// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One body source's windows over a span of a day, for an agent.
//!
//! This reaches body windows and nothing else, so capture zones, untimed flags
//! and speaker names don't arise. `docs/BODY_SEGMENTS.md` is the contract.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use jiff::civil::{Date, DateTime};
use jiff::{SignedDuration, Span, Timestamp, ZonedDateTime};
use serde_json::{Value, json};
use solstone_core_format::content::body_hidden_days::{HiddenDays, RowDays, read_hidden_days};
use solstone_core_format::content::body_records::{
    BODY_RECORD_STREAMS, BODY_RECORDS_FILE, BodyRow, RowKind, is_womens_health_row,
    only_moved_rows, parse_source_time, render_row,
};
use solstone_core_format::content::kinds::PlacesPrecision;
use solstone_core_format::content::strava_body::{STRAVA_SOURCE, STRAVA_TILE_FILE, strava_rows};
use solstone_core_journal_config::TimeZone;
use solstone_core_journal_io::paths::{
    PathOrDay, Segment, iter_segments_skipping_unreadable_streams,
};
use solstone_core_location::old_imports::{CoverScope, OldSource, covers_on_day, month_words};
use solstone_core_transcripts::segment_clock;

use crate::dispatch::{
    DispatchError, DispatchPrincipal, Prepared, boundary, codec, live_allows,
    segment_assignment_probe_path, source_label, source_labels,
};
use crate::permissions::ConnectionReadSnapshot;
use crate::references::{
    ReferenceKind, ReferenceTarget, SegmentReference, SourceSpanPosition, SourceSpanReference,
    TranscriptListCursor,
};
use crate::tools::{ToolError, ValidatedGetTranscript, ValidatedListTranscripts};
use solstone_core_indexer_query::AdmittedCategory;

/// Windows one `get_transcript` call visits. A sparse span gets short pages rather
/// than one call reading a whole day's files.
const MAX_WINDOWS_PER_READ: usize = 24;
/// How far before a span a read looks, for a count, stage or logged entry that began
/// in the window before it and runs into the span. An experience has a tile in every
/// slot it covers, so it needs no looking back. An interval that began further back
/// isn't found by a narrow span.
const LOOK_BACK_SECONDS: u32 = 3_600;
/// Entries on one page: what a single segment's page already holds.
const MAX_ENTRIES_PER_PAGE: usize = solstone_core_transcripts::MAX_TRANSCRIPT_PAGE_ITEMS;

/// What an agent is told where an answer would have held or counted body data
/// that its connection lacks the permission for.
pub(crate) const BODY_NOT_GRANTED: &str = "this connection doesn't have the body permission. the owner can grant it in the journal's agents app.";

/// Whether this connection holds the body permission: what body imports
/// brought in is read under it alone.
pub(crate) fn holds_body(snapshot: &ConnectionReadSnapshot) -> bool {
    snapshot.categories.contains(&AdmittedCategory::Body)
}

/// Whether this connection may read Women's Health body rows (requires both
/// `body` and its `womens_health` sub-option; `permissions.rs` already strips
/// `womens_health` whenever `body` is absent).
pub(crate) fn holds_womens_health(snapshot: &ConnectionReadSnapshot) -> bool {
    snapshot.categories.contains(&AdmittedCategory::Body)
        && snapshot
            .categories
            .contains(&AdmittedCategory::WomensHealth)
}

/// Stable tag mixed into body segment page cursors whenever workout coordinate
/// precision changes.
pub(crate) fn body_policy_tag(places_precision: Option<PlacesPrecision>) -> u8 {
    match places_precision {
        Some(PlacesPrecision::Full) => 0,
        Some(PlacesPrecision::Approx) => 1,
        Some(PlacesPrecision::NamesOnly) | None => 2,
    }
}

const WORKOUT_COORD_KEYS: &[&str] = &[
    "startLatitude",
    "startLongitude",
    "endLatitude",
    "endLongitude",
    "start_latitude",
    "start_longitude",
    "end_latitude",
    "end_longitude",
];

fn extract_coord_pair(
    fields: &mut std::collections::BTreeMap<String, Value>,
    lat_keys: [&str; 2],
    lon_keys: [&str; 2],
) -> Option<(f64, f64)> {
    let mut lat = None;
    for key in lat_keys {
        if let Some(value) = fields.remove(key)
            && lat.is_none()
        {
            lat = value.as_f64();
        }
    }
    let mut lon = None;
    for key in lon_keys {
        if let Some(value) = fields.remove(key)
            && lon.is_none()
        {
            lon = value.as_f64();
        }
    }
    match (lat, lon) {
        (Some(lat), Some(lon)) if lat.is_finite() && lon.is_finite() => Some((lat, lon)),
        _ => None,
    }
}

/// Render a body row for an MCP agent connection, gating raw workout GPS
/// coordinates on the connection's `places` precision (`Full` exact, `Approx`
/// H3 res-8 ~1 km cell, `NamesOnly` / no `places` omitted while keeping
/// `locationName` and all workout stats).
pub(crate) fn render_row_for_connection(
    row: &BodyRow,
    places_precision: Option<PlacesPrecision>,
) -> String {
    if matches!(places_precision, Some(PlacesPrecision::Full))
        || !WORKOUT_COORD_KEYS
            .iter()
            .any(|key| row.fields.contains_key(*key))
    {
        return render_row(row);
    }
    let mut adjusted = row.clone();
    let start_pair = extract_coord_pair(
        &mut adjusted.fields,
        ["startLatitude", "start_latitude"],
        ["startLongitude", "start_longitude"],
    );
    let end_pair = extract_coord_pair(
        &mut adjusted.fields,
        ["endLatitude", "end_latitude"],
        ["endLongitude", "end_longitude"],
    );
    if matches!(places_precision, Some(PlacesPrecision::Approx)) {
        if let Some((lat, lon)) = start_pair
            && let Some(cell) = solstone_core_location::h3_cell::res8_cell(lat, lon)
        {
            adjusted.fields.insert(
                "startLocationCell".to_owned(),
                json!({
                    "h3": cell.h3,
                    "cell_lat": cell.cell_lat,
                    "cell_lon": cell.cell_lon,
                }),
            );
        }
        if let Some((lat, lon)) = end_pair
            && let Some(cell) = solstone_core_location::h3_cell::res8_cell(lat, lon)
        {
            adjusted.fields.insert(
                "endLocationCell".to_owned(),
                json!({
                    "h3": cell.h3,
                    "cell_lat": cell.cell_lat,
                    "cell_lon": cell.cell_lon,
                }),
            );
        }
    }
    render_row(&adjusted)
}

/// Read one segment transcript page enforcing this connection's body egress
/// policies (Women's Health filtering and `places`-gated workout coordinates).
#[allow(clippy::too_many_arguments)]
pub(crate) fn read_body_aware_page(
    segment: &Segment,
    cursor: Option<&solstone_core_transcripts::SegmentTranscriptCursor>,
    speakers: Option<&solstone_core_transcripts::AgentSpeakers>,
    possible_names: Option<&solstone_core_transcripts::PossibleNameLabels>,
    left_out: &BTreeSet<String>,
    categories: &BTreeSet<AdmittedCategory>,
    places_precision: Option<PlacesPrecision>,
) -> Result<
    solstone_core_transcripts::SegmentTranscriptPage,
    solstone_core_transcripts::SegmentTranscriptReadError,
> {
    let renderer = |row: &BodyRow| render_row_for_connection(row, places_precision);
    let policy = solstone_core_transcripts::BodyReadPolicy {
        policy_tag: body_policy_tag(places_precision),
        render_row: &renderer,
    };
    solstone_core_transcripts::read_segment_transcript_page_with_body_policy(
        segment,
        cursor,
        speakers,
        possible_names,
        left_out,
        Some(categories),
        Some(&policy),
    )
}

/// The one answer for a source label that reaches nothing, whatever kind of
/// source it would have named: a label can't be used to ask what the journal
/// holds.
pub(crate) const NOTHING_READABLE: &str = "No source with that label holds anything this connection can read on that day. A source's label comes from the `source` of a listed segment, from `other_sources` on a day's list_transcripts, or from that day's body document.";
const SPAN_EMPTY: &str = "No entries in this span.";
const SPAN_EMPTY_CONTINUES: &str =
    "No entries in this part of the span. Pass next_cursor as cursor to read on.";
const KIND_ABSENT: &str = "This source holds entries in that time, but none of the kind asked for. Its entries name their kinds; list without kind to see them.";
const BEGAN_EARLIER: &str = "Nothing in this range is listed, but an entry that began before the range runs into it. Pass span_reference to get_transcript as reference to read it.";
const OTHER_CONTENT: &str = "This source also holds other content on this day that isn't listed here, so these gaps describe only its body segments. List the day without source to see it.";

/// What an earlier import of a body source holds for this day, for a connection
/// that may read the whole journal's body data. A bundle has no facet, so a
/// limited connection is told nothing of it.
fn earlier_import_note(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    day: &str,
    only: Option<OldSource>,
) -> Option<String> {
    if !holds_body(snapshot)
        || !matches!(
            snapshot.scope,
            solstone_core_indexer_query::ConnectionScope::WholeJournal
        )
    {
        return None;
    }
    let notes = covers_on_day(journal_root, day, only)
        .into_iter()
        .map(|cover| {
            let source = cover.source.words();
            match cover.scope {
                CoverScope::Day => format!(
                    "An earlier import of {source} covers this day and isn't read, so what it holds isn't listed here."
                ),
                CoverScope::Month(month) => format!(
                    "An earlier import of {source} covers the month of {} and isn't read, so what it holds isn't listed here.",
                    month_words(&month).unwrap_or(month)
                ),
            }
        })
        .collect::<Vec<_>>();
    (!notes.is_empty()).then(|| notes.join(" "))
}

/// Whether a connection without the body permission would have been told of an
/// earlier body import covering this day had it held one.
pub(crate) fn earlier_import_withheld(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    day: &str,
) -> bool {
    !holds_body(snapshot)
        && matches!(
            snapshot.scope,
            solstone_core_indexer_query::ConnectionScope::WholeJournal
        )
        && !covers_on_day(journal_root, day, None).is_empty()
}

/// The note for a plain day listing, when an old import of any body source covers it.
pub(crate) fn listing_earlier_import_note(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    day: &str,
) -> Option<String> {
    earlier_import_note(journal_root, snapshot, day, None)
}

struct Window {
    segment: Segment,
    name: String,
    start_seconds: u32,
    end_seconds: u32,
    label: String,
    /// Its rows of a night or workout that started on a day the owner hid.
    left_out: BTreeSet<String>,
}

/// How many days before a day a night or workout running into it can have
/// started: an experience can run this long.
const HIDE_REACH_DAYS: i64 = 14;

/// The owner's hidden days of body data, read once for a call, and what they
/// take from a day of one body source: the whole day when it is hidden, and on
/// a later day the rows of a night or workout that started on a hidden one.
/// While which days are hidden can't be read, no body data can be.
pub(crate) struct BodyHides {
    days: HiddenDays,
    /// Each day's row rule for a source, worked out once a read needs it.
    rules: RefCell<BTreeMap<(String, String), Rc<RowDays>>>,
}

impl BodyHides {
    pub(crate) fn read(journal_root: &Path) -> Self {
        Self {
            days: read_hidden_days(journal_root),
            rules: RefCell::default(),
        }
    }

    /// Whether none of `day`'s body data may be read: the owner hid it, or
    /// which days they hid can't be read.
    pub(crate) fn day_hidden(&self, day: &str) -> bool {
        self.days.hides(day) != Some(false)
    }

    /// The hidden days, when one of them is close enough before `day` for a
    /// night or workout that started on it to run into `day`.
    fn reaching(&self, day: &str) -> Option<&BTreeSet<String>> {
        let days = self.days.days()?;
        let date = day_date(day)?;
        let reach =
            solstone_core_time::day_key(date.checked_sub(Span::new().days(HIDE_REACH_DAYS)).ok()?);
        days.range(reach..day.to_owned()).next().map(|_| days)
    }

    /// The ids among `rows`, all of `stream` on `day`, that go with a hidden
    /// earlier day.
    fn left_out(
        &self,
        journal_root: &Path,
        day: &str,
        stream: &str,
        rows: &[BodyRow],
    ) -> BTreeSet<String> {
        let Some(hidden) = self.reaching(day) else {
            return BTreeSet::new();
        };
        let rule = self.rule(journal_root, day, stream);
        rows.iter()
            .filter(|row| rule.hidden(row, hidden))
            .map(|row| row.id.clone())
            .collect()
    }

    /// The row rule over every row `stream` holds on `day`, so a stage finds
    /// its night through a piece of it in any window of the day.
    fn rule(&self, journal_root: &Path, day: &str, stream: &str) -> Rc<RowDays> {
        let key = (day.to_owned(), stream.to_owned());
        if let Some(rule) = self.rules.borrow().get(&key) {
            return Rc::clone(rule);
        }
        let mut rows = Vec::new();
        let directory = journal_root.join("chronicle").join(day).join(stream);
        for entry in std::fs::read_dir(directory).into_iter().flatten().flatten() {
            let Ok(text) = std::fs::read_to_string(entry.path().join(BODY_RECORDS_FILE)) else {
                continue;
            };
            rows.extend(
                text.lines()
                    .filter_map(|line| serde_json::from_str::<BodyRow>(line).ok())
                    .filter(|row| row.kind != RowKind::Moved),
            );
        }
        let rule = Rc::new(RowDays::new(day, source_zone(journal_root, stream), &rows));
        self.rules.borrow_mut().insert(key, Rc::clone(&rule));
        rule
    }

    /// Whether a Strava tile on `day` is hidden: on a hidden day, or a later
    /// piece of a workout that started on one.
    fn hides_workout(&self, journal_root: &Path, day: &str, segment: &Segment) -> bool {
        if self.day_hidden(day) {
            return true;
        }
        let Some(hidden) = self.reaching(day) else {
            return false;
        };
        let Some(rows) = std::fs::read_to_string(segment.path().join(STRAVA_TILE_FILE))
            .ok()
            .and_then(|text| strava_rows(&text))
        else {
            return false;
        };
        let zone = solstone_core_journal_config::placed_zone(journal_root, STRAVA_SOURCE);
        let rule = RowDays::new(day, zone, &rows);
        rows.iter().any(|row| rule.hidden(row, hidden))
    }

    /// What of a segment holding body data may be read: `None` when nothing
    /// may (a hidden day's, a window holding only rows of a hidden night, a
    /// piece of a workout that started on a hidden day), else the rows to leave
    /// out. A body file decides it whatever else sits beside it; a segment with
    /// none is read whole.
    pub(crate) fn shown(
        &self,
        journal_root: &Path,
        day: &str,
        stream: &str,
        segment: &Segment,
        include_womens_health: bool,
    ) -> Option<BTreeSet<String>> {
        if !segment.path().join(BODY_RECORDS_FILE).is_file()
            && !segment.path().join(STRAVA_TILE_FILE).is_file()
        {
            return Some(BTreeSet::new());
        }
        if segment.path().join(STRAVA_TILE_FILE).is_file() {
            return (!self.hides_workout(journal_root, day, segment)).then(BTreeSet::new);
        }
        if self.day_hidden(day) {
            return None;
        }
        if self.reaching(day).is_none() && include_womens_health {
            return Some(BTreeSet::new());
        }
        let Ok(text) = std::fs::read_to_string(segment.path().join(BODY_RECORDS_FILE)) else {
            return Some(BTreeSet::new());
        };
        let mut rows = Vec::new();
        let mut damaged = false;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str::<BodyRow>(line) {
                Ok(row) if row.kind == RowKind::Moved => {}
                Ok(row) => rows.push(row),
                // A line that isn't a row says nothing of a day: it reads as itself.
                Err(_) => damaged = true,
            }
        }
        let mut left_out = self.left_out(journal_root, day, stream, &rows);
        if !include_womens_health {
            for row in &rows {
                if is_womens_health_row(row) {
                    left_out.insert(row.id.clone());
                }
            }
        }
        let all_hidden = !rows.is_empty() && left_out.len() == rows.len();
        (damaged || !all_hidden).then_some(left_out)
    }
}

/// The zone that placed a body stream's windows, when its source fixed one.
fn source_zone(journal_root: &Path, stream: &str) -> Option<TimeZone> {
    stream
        .strip_prefix("import.")
        .and_then(|source| solstone_core_journal_config::placed_zone(journal_root, source))
}

fn released(journal_root: &Path, day: &str, stream: &str, name: &str) -> bool {
    crate::source_release::check_segment_transcript_release(journal_root, day, stream, name)
}

/// Whether a body window holds only rows that moved elsewhere, which a reader
/// treats as no window at all. One that can't be read stays, so it's counted.
pub(crate) fn holds_only_moved(window: &Path) -> bool {
    std::fs::read_to_string(window.join(BODY_RECORDS_FILE)).is_ok_and(|text| only_moved_rows(&text))
}

/// A body window's place on its source's grid, from its key. A key off the
/// grid is no window any list shows.
pub(crate) fn grid_clock(name: &str) -> Option<solstone_core_transcripts::SegmentClock> {
    let length = solstone_core_format::content::body_records::grid_length(name)?;
    let (clock, _) = name.split_once('_')?;
    segment_clock(&format!("{clock}_{length}"))
}

/// The body windows this connection reads: none without the body permission.
fn readable_windows(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    hides: &BodyHides,
    day: &str,
    stream: &str,
) -> Result<Vec<Window>, DispatchError> {
    if !holds_body(snapshot) {
        return Ok(Vec::new());
    }
    windows_in_scope(journal_root, snapshot, hides, day, stream)
}

/// The body window a live, in-scope, unreleased segment is, in time order. One
/// holding only moved rows is left out. This is what the connection would read
/// with the body permission, whether or not it holds it.
fn windows_in_scope(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    hides: &BodyHides,
    day: &str,
    stream: &str,
) -> Result<Vec<Window>, DispatchError> {
    // A hidden day's windows aren't there to read, and while which days are
    // hidden can't be read, no day's are.
    if hides.day_hidden(day) {
        return Ok(Vec::new());
    }
    let transcript_boundary = boundary(snapshot, Some(AdmittedCategory::Body), None);
    let segments = iter_segments_skipping_unreadable_streams(journal_root, PathOrDay::Day(day))
        .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
    let mut windows = Vec::new();
    for segment in segments {
        let Ok(identity) = segment.record_identity() else {
            continue;
        };
        if identity.stream != stream
            || !segment.path().join(BODY_RECORDS_FILE).is_file()
            || !solstone_core_segment::is_body_only_segment(&segment)
            || holds_only_moved(segment.path())
        {
            continue;
        }
        let name = identity.name.to_owned();
        let Some(clock) = grid_clock(&name) else {
            continue;
        };
        let probe = segment_assignment_probe_path(day, stream, &name);
        if !live_allows(journal_root, &transcript_boundary, &probe, Some(stream))
            || !released(journal_root, day, stream, &name)
        {
            continue;
        }
        // One holding only rows of a night that started on a hidden day, or
        // only withheld Women's Health rows, reads as no window.
        let Some(left_out) = hides.shown(
            journal_root,
            day,
            stream,
            &segment,
            holds_womens_health(snapshot),
        ) else {
            continue;
        };
        windows.push(Window {
            segment,
            name,
            start_seconds: clock.start_seconds,
            end_seconds: clock.exclusive_end_seconds,
            label: clock.label,
            left_out,
        });
    }
    windows.sort_by(|left, right| {
        (left.start_seconds, left.name.as_str()).cmp(&(right.start_seconds, right.name.as_str()))
    });
    Ok(windows)
}

/// Daily movement totals, sleep and bounded reading measures from the same
/// windows the transcript reader can serve. Keep different recorders separate.
pub(crate) fn week_totals(
    journal: &Path,
    snapshot: &ConnectionReadSnapshot,
    days: &[String],
) -> Result<(BTreeMap<String, Value>, Vec<String>), DispatchError> {
    let mut summaries = BTreeMap::new();
    let mut targets = Vec::new();
    let hides = BodyHides::read(journal);
    let labels = source_labels(journal);
    for stream in BODY_RECORD_STREAMS {
        let mut nights = BTreeMap::new();
        let mut stages = BTreeMap::<String, NightStages>::new();
        let mut seen = BTreeSet::new();
        for day in days {
            let mut movement = BTreeMap::<Option<String>, Movement>::new();
            let mut readings = BTreeMap::<Option<String>, DayReadings>::new();
            let mut windows = readable_windows(journal, snapshot, &hides, day, stream)?;
            windows.sort_by_key(|window| {
                (
                    solstone_core_format::content::body_records::copy_rank(&window.name),
                    window.name.clone(),
                )
            });
            for window in windows {
                let segment_path = format!("{day}/{stream}/{}", window.name);
                let path = format!("chronicle/{segment_path}/{BODY_RECORDS_FILE}");
                let Some(bytes) = crate::activity_admit::read_contained_source(journal, &path)
                else {
                    continue;
                };
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
                if !crate::source_release::check_indexed_source_release(journal, &path) {
                    continue;
                }
                let mut used = false;
                for row in text
                    .lines()
                    .filter_map(|line| serde_json::from_str::<BodyRow>(line).ok())
                {
                    if row.kind == RowKind::Moved
                        || window.left_out.contains(&row.id)
                        || (!holds_womens_health(snapshot) && is_womens_health_row(&row))
                        || !seen.insert(row.id.clone())
                    {
                        continue;
                    }
                    match row.kind {
                        RowKind::Count
                            if matches!(
                                row.label.as_str(),
                                "step count" | "distance walking running" | "active energy burned"
                            ) =>
                        {
                            movement.entry(row.by.clone()).or_default().add_count(&row);
                        }
                        RowKind::Reading
                            if matches!(
                                row.label.as_str(),
                                "resting heart rate" | "heart rate variability sdnn" | "heart rate"
                            ) =>
                        {
                            readings
                                .entry(row.by.clone())
                                .or_default()
                                .add_reading(&row);
                        }
                        RowKind::Dated if row.record_type == "daily_activity" => {
                            let mut matched = false;
                            if let Some(steps) =
                                row.fields.get("steps").filter(|value| !value.is_null())
                            {
                                let counts = &mut movement.entry(row.by.clone()).or_default().steps;
                                if let Some(steps) = steps.as_f64() {
                                    counts.add(steps, None);
                                } else {
                                    counts.note = Some("a stored step count could not be read");
                                }
                                matched = true;
                            }
                            if let Some(kcal) = row
                                .fields
                                .get("active_calories")
                                .or_else(|| row.fields.get("activeKilocalories"))
                                .filter(|value| !value.is_null())
                            {
                                let counts = &mut movement
                                    .entry(row.by.clone())
                                    .or_default()
                                    .active_energy_kcal;
                                if let Some(kcal) = kcal.as_f64() {
                                    counts.add(kcal, None);
                                } else {
                                    counts.note =
                                        Some("a stored active energy count could not be read");
                                }
                                matched = true;
                            }
                            if !matched {
                                continue;
                            }
                        }
                        RowKind::Experience if row.experience.as_deref() == Some("sleep") => {
                            nights.insert(row.id.clone(), row);
                        }
                        RowKind::Stage
                            if let (Some(stage), Some(parent), Some(interval)) =
                                (sleep_stage_key(&row.label), &row.of, row_interval(&row)) =>
                        {
                            let entry = stages.entry(parent.clone()).or_default();
                            if stage.counts_as_asleep {
                                entry.asleep.push(interval);
                            }
                            entry.by_stage.entry(stage.key).or_default().push(interval);
                        }
                        _ => continue,
                    }
                    used = true;
                }
                if used {
                    targets.push(segment_path);
                }
            }
            for (recorder, movement) in movement {
                let steps = movement.steps.total();
                let distance_m = movement.distance_m.total();
                let active_energy_kcal = movement.active_energy_kcal.total();
                if steps != Ok(None) || distance_m != Ok(None) || active_energy_kcal != Ok(None) {
                    let mut record = json!({
                        "source": source_label(&labels, stream), "recorded_by": recorder,
                        "steps": steps.ok().flatten(), "distance_m": distance_m.ok().flatten(),
                    });
                    if let Err(note) = steps {
                        record["steps_note"] = json!(note);
                    }
                    if let Err(note) = distance_m {
                        record["distance_note"] = json!(note);
                    }
                    match active_energy_kcal {
                        Ok(Some(kcal)) => record["active_energy_kcal"] = json!(kcal),
                        Ok(None) => {}
                        Err(note) => record["active_energy_note"] = json!(note),
                    }
                    let entry = summaries.entry(day.clone()).or_insert_with(|| json!({}));
                    let list = entry
                        .as_object_mut()
                        .expect("body summary object")
                        .entry("movement")
                        .or_insert_with(|| json!([]));
                    list.as_array_mut().expect("movement array").push(record);
                }
            }
            for (recorder, day_readings) in readings {
                if let Some(record) =
                    day_readings.into_json(source_label(&labels, stream), recorder)
                {
                    let entry = summaries.entry(day.clone()).or_insert_with(|| json!({}));
                    let list = entry
                        .as_object_mut()
                        .expect("body summary object")
                        .entry("readings")
                        .or_insert_with(|| json!([]));
                    list.as_array_mut().expect("readings array").push(record);
                }
            }
        }
        for (id, row) in nights {
            let Some(wake) = row.end.as_deref().and_then(parse_source_time) else {
                continue;
            };
            let zone = source_zone(journal, stream);
            let day = solstone_core_time::day_key(
                zone.map_or_else(|| wake.date(), |zone| wake.with_time_zone(zone).date()),
            );
            if !days.contains(&day) || hides.day_hidden(&day) {
                continue;
            }
            let night_stages = stages.remove(&id);
            let asleep_seconds = night_stages
                .as_ref()
                .filter(|stage| !stage.asleep.is_empty())
                .map(|stage| covered_seconds(stage.asleep.clone()))
                .or_else(|| {
                    row.fields
                        .get("total_sleep_duration")
                        .or_else(|| row.fields.get("sleepTimeSeconds"))
                        .and_then(Value::as_u64)
                });
            let mut stages_seconds = serde_json::Map::new();
            if let Some(night_stages) = night_stages {
                for (stage_name, spans) in night_stages.by_stage {
                    let seconds = covered_seconds(spans);
                    if seconds > 0 {
                        stages_seconds.insert(stage_name.to_owned(), json!(seconds));
                    }
                }
            } else {
                for (field, key) in [
                    ("awake_time", "awake"),
                    ("awakeSleepSeconds", "awake"),
                    ("deep_sleep_duration", "deep"),
                    ("deepSleepSeconds", "deep"),
                    ("light_sleep_duration", "light"),
                    ("lightSleepSeconds", "light"),
                    ("rem_sleep_duration", "rem"),
                    ("remSleepSeconds", "rem"),
                ] {
                    if !stages_seconds.contains_key(key)
                        && let Some(seconds) = row.fields.get(field).and_then(Value::as_u64)
                        && seconds > 0
                    {
                        stages_seconds.insert(key.to_owned(), json!(seconds));
                    }
                }
            }
            let mut night = json!({
                "source": source_label(&labels, stream), "recorded_by": row.by,
                "to_bed": row.start, "waking": row.end, "asleep_seconds": asleep_seconds,
            });
            if !stages_seconds.is_empty() {
                night["stages_seconds"] = Value::Object(stages_seconds);
            }
            let entry = summaries.entry(day).or_insert_with(|| json!({}));
            let list = entry
                .as_object_mut()
                .expect("body summary object")
                .entry("nights")
                .or_insert_with(|| json!([]));
            list.as_array_mut().expect("nights array").push(night);
        }
    }
    Ok((summaries, targets))
}

#[derive(Default)]
struct NightStages {
    asleep: Vec<(i64, i64)>,
    by_stage: BTreeMap<&'static str, Vec<(i64, i64)>>,
}

struct SleepStage {
    key: &'static str,
    counts_as_asleep: bool,
}

fn sleep_stage_key(label: &str) -> Option<SleepStage> {
    match label {
        "asleep deep" => Some(SleepStage {
            key: "deep",
            counts_as_asleep: true,
        }),
        "asleep core" => Some(SleepStage {
            key: "core",
            counts_as_asleep: true,
        }),
        "asleep light" => Some(SleepStage {
            key: "light",
            counts_as_asleep: true,
        }),
        "asleep rem" => Some(SleepStage {
            key: "rem",
            counts_as_asleep: true,
        }),
        "asleep" | "asleep unspecified" => Some(SleepStage {
            key: "unspecified",
            counts_as_asleep: true,
        }),
        "awake" => Some(SleepStage {
            key: "awake",
            counts_as_asleep: false,
        }),
        _ if label.starts_with("asleep") => Some(SleepStage {
            key: "unspecified",
            counts_as_asleep: true,
        }),
        _ => None,
    }
}

#[derive(Default)]
struct Movement {
    steps: Counts,
    distance_m: Counts,
    active_energy_kcal: Counts,
}

impl Movement {
    fn add_count(&mut self, row: &BodyRow) {
        let (counts, scale) = match (row.label.as_str(), row.unit.as_deref()) {
            ("step count", Some("count") | None) => (&mut self.steps, 1.0),
            ("distance walking running", Some("km")) => (&mut self.distance_m, 1000.0),
            ("distance walking running", Some("m")) => (&mut self.distance_m, 1.0),
            ("distance walking running", Some("mi")) => (&mut self.distance_m, 1609.344),
            ("active energy burned", Some("kcal" | "Cal")) => (&mut self.active_energy_kcal, 1.0),
            ("active energy burned", Some("cal")) => (&mut self.active_energy_kcal, 0.001),
            ("active energy burned", Some("kJ")) => (&mut self.active_energy_kcal, 1.0 / 4.184),
            ("step count", _) => {
                self.steps.note = Some("a stored step count uses an unrecognized unit");
                return;
            }
            ("distance walking running", _) => {
                self.distance_m.note = Some("a stored distance uses an unrecognized unit");
                return;
            }
            ("active energy burned", _) => {
                self.active_energy_kcal.note =
                    Some("a stored active energy count uses an unrecognized unit");
                return;
            }
            _ => return,
        };
        let Some(value) = row
            .value
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok())
        else {
            counts.note = Some("a stored value could not be read");
            return;
        };
        let interval = observed_interval(row);
        if interval.is_none() && (row.start.is_some() || row.end.is_some()) {
            counts.note = Some("a stored observation's time could not be read");
            return;
        }
        counts.add(value * scale, interval);
    }
}

#[derive(Default)]
struct DayReadings {
    resting_heart_rate_bpm: ReadingBounds,
    hrv_sdnn_ms: ReadingBounds,
    heart_rate_bpm: ReadingBounds,
}

impl DayReadings {
    fn add_reading(&mut self, row: &BodyRow) {
        let (bounds, scale, bad_value_note) = match (row.label.as_str(), row.unit.as_deref()) {
            ("resting heart rate", Some("count/min" | "bpm")) => (
                &mut self.resting_heart_rate_bpm,
                1.0,
                "a stored resting heart rate could not be read",
            ),
            ("resting heart rate", _) => {
                self.resting_heart_rate_bpm.note =
                    Some("a stored resting heart rate uses an unrecognized unit");
                return;
            }
            ("heart rate variability sdnn", Some("ms")) => (
                &mut self.hrv_sdnn_ms,
                1.0,
                "a stored HRV reading could not be read",
            ),
            ("heart rate variability sdnn", Some("s")) => (
                &mut self.hrv_sdnn_ms,
                1000.0,
                "a stored HRV reading could not be read",
            ),
            ("heart rate variability sdnn", _) => {
                self.hrv_sdnn_ms.note = Some("a stored HRV reading uses an unrecognized unit");
                return;
            }
            ("heart rate", Some("count/min" | "bpm")) => (
                &mut self.heart_rate_bpm,
                1.0,
                "a stored heart rate could not be read",
            ),
            ("heart rate", _) => {
                self.heart_rate_bpm.note = Some("a stored heart rate uses an unrecognized unit");
                return;
            }
            _ => return,
        };
        let Some(value) = row
            .value
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok())
            .map(|value| value * scale)
            .filter(|value| value.is_finite() && *value >= 0.0)
        else {
            bounds.note = Some(bad_value_note);
            return;
        };
        bounds.add(value, row);
    }

    fn into_json(self, source: String, recorded_by: Option<String>) -> Option<Value> {
        let rhr = self.resting_heart_rate_bpm.finish();
        let hrv = self.hrv_sdnn_ms.finish();
        let hr = self.heart_rate_bpm.finish();
        if rhr.is_empty() && hrv.is_empty() && hr.is_empty() {
            return None;
        }
        let mut record = json!({
            "source": source,
            "recorded_by": recorded_by,
        });
        match rhr {
            ReadingSummary::Empty => {}
            ReadingSummary::Bounds {
                min, max, count, ..
            } => {
                record["resting_heart_rate_bpm"] = json!({
                    "min": min,
                    "max": max,
                    "count": count,
                });
            }
            ReadingSummary::Note(note) => {
                record["resting_heart_rate_note"] = json!(note);
            }
        }
        match hrv {
            ReadingSummary::Empty => {}
            ReadingSummary::Bounds {
                min, max, count, ..
            } => {
                record["hrv_sdnn_ms"] = json!({
                    "min": min,
                    "max": max,
                    "count": count,
                });
            }
            ReadingSummary::Note(note) => {
                record["hrv_sdnn_note"] = json!(note);
            }
        }
        match hr {
            ReadingSummary::Empty => {}
            ReadingSummary::Bounds {
                min,
                max,
                count,
                min_at,
                max_at,
            } => {
                let mut value = json!({
                    "min": min,
                    "max": max,
                    "count": count,
                });
                if let Some(at) = min_at {
                    value["min_at"] = json!(at);
                }
                if let Some(at) = max_at {
                    value["max_at"] = json!(at);
                }
                record["heart_rate_bpm"] = value;
            }
            ReadingSummary::Note(note) => {
                record["heart_rate_note"] = json!(note);
            }
        }
        Some(record)
    }
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
struct ReadingKey {
    missing_time: bool,
    timestamp_nanos: i128,
    at: String,
    id: String,
}

impl ReadingKey {
    fn from_row(row: &BodyRow) -> Self {
        let at = row.begins().unwrap_or_default().to_owned();
        let parsed = row.begins().and_then(parse_source_time);
        Self {
            missing_time: parsed.is_none(),
            timestamp_nanos: parsed.map_or(0, |time| time.timestamp().as_nanosecond()),
            at,
            id: row.id.clone(),
        }
    }
}

#[derive(Default)]
struct ReadingBounds {
    count: u64,
    min: Option<(f64, ReadingKey, Option<String>)>,
    max: Option<(f64, ReadingKey, Option<String>)>,
    note: Option<&'static str>,
}

enum ReadingSummary {
    Empty,
    Bounds {
        min: f64,
        max: f64,
        count: u64,
        min_at: Option<String>,
        max_at: Option<String>,
    },
    Note(&'static str),
}

impl ReadingSummary {
    fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }
}

impl ReadingBounds {
    fn add(&mut self, value: f64, row: &BodyRow) {
        self.count += 1;
        let key = ReadingKey::from_row(row);
        let at = row.begins().map(str::to_owned);
        let replace_min = match &self.min {
            None => true,
            Some((current, current_key, _)) => {
                value < *current || (value == *current && key < *current_key)
            }
        };
        if replace_min {
            self.min = Some((value, key.clone(), at.clone()));
        }
        let replace_max = match &self.max {
            None => true,
            Some((current, current_key, _)) => {
                value > *current || (value == *current && key < *current_key)
            }
        };
        if replace_max {
            self.max = Some((value, key, at));
        }
    }

    fn finish(self) -> ReadingSummary {
        if let Some(note) = self.note {
            return ReadingSummary::Note(note);
        }
        match (self.min, self.max) {
            (Some((min, _, min_at)), Some((max, _, max_at))) => ReadingSummary::Bounds {
                min,
                max,
                count: self.count,
                min_at,
                max_at,
            },
            _ => ReadingSummary::Empty,
        }
    }
}

/// Overlapping observations aren't additive. A total is shown only when the
/// stored rows for this recorder partition the reported time.
#[derive(Default)]
struct Counts {
    observations: Vec<(f64, Option<(Timestamp, Timestamp)>)>,
    note: Option<&'static str>,
}

impl Counts {
    fn add(&mut self, value: f64, interval: Option<(Timestamp, Timestamp)>) {
        if value.is_finite() && value >= 0.0 {
            self.observations.push((value, interval));
        } else {
            self.note = Some("a stored value could not be read");
        }
    }

    fn total(mut self) -> Result<Option<f64>, &'static str> {
        if let Some(note) = self.note {
            return Err(note);
        }
        if self.observations.is_empty() {
            return Ok(None);
        }
        self.observations.sort_by_key(|(_, interval)| *interval);
        if self
            .observations
            .windows(2)
            .any(|pair| match (pair[0].1, pair[1].1) {
                (Some((previous_start, end)), Some((start, _))) => {
                    start < end || (previous_start == end && start == end)
                }
                _ => true,
            })
        {
            return Err("these stored observations cannot be combined into one total");
        }
        let total: f64 = self.observations.into_iter().map(|(value, _)| value).sum();
        if !total.is_finite() {
            return Err("stored values could not be summed");
        }
        Ok(Some(total))
    }
}

/// A count may be measured at one instant, and sub-second samples can be
/// adjacent without overlapping.
fn observed_interval(row: &BodyRow) -> Option<(Timestamp, Timestamp)> {
    let start = row
        .start
        .as_deref()
        .and_then(parse_source_time)?
        .timestamp();
    let end = row.end.as_deref().and_then(parse_source_time)?.timestamp();
    (end >= start).then_some((start, end))
}

fn row_interval(row: &BodyRow) -> Option<(i64, i64)> {
    let (start, end) = observed_interval(row)?;
    let (start, end) = (start.as_second(), end.as_second());
    (end > start).then_some((start, end))
}

fn covered_seconds(mut spans: Vec<(i64, i64)>) -> u64 {
    spans.sort_unstable();
    let mut end = i64::MIN;
    let mut seconds = 0;
    for (start, stop) in spans {
        if stop > end {
            seconds += (stop - start.max(end)).unsigned_abs();
            end = stop;
        }
    }
    seconds
}

/// Whether this connection may read a body window, by day, stream and name: the
/// same test `readable_windows` makes, and the body permission. No window to name, its directory gone with
/// no tombstone say, is no window to read: only the owner sees that account.
/// The facet folder and day of an activity day file's path.
pub(crate) fn activity_file_place(path: &str) -> Option<(String, String)> {
    let mut parts = path.split('/');
    if parts.next()? != "facets" {
        return None;
    }
    let facet = parts.next()?;
    if parts.next()? != "activities" {
        return None;
    }
    let day = parts.next()?.strip_suffix(".jsonl")?;
    if parts.next().is_some() {
        return None;
    }
    Some((facet.to_owned(), day.to_owned()))
}

/// The activity day files one query has read: many matching rows share a file,
/// and a query reads each one once.
#[derive(Clone, Default)]
pub(crate) struct ActivityDayFiles(Arc<Mutex<BTreeMap<String, Option<DayRecords>>>>);

type DayRecords = Arc<Vec<solstone_core_format::content::JsonObject>>;

impl ActivityDayFiles {
    fn records(&self, journal_root: &Path, path: &str) -> Option<DayRecords> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(path.to_owned())
            .or_insert_with(|| {
                let text = std::fs::read_to_string(journal_root.join(path)).ok()?;
                Some(Arc::new(
                    solstone_core_format::content::parse_jsonl_objects(&text),
                ))
            })
            .clone()
    }
}

/// The composed record behind a body record's search entry, as this connection
/// may read it. `None` when it may not: no body permission, no grant on its window,
/// or a window that's gone.
pub(crate) fn body_entry_face(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    files: &ActivityDayFiles,
    path: &str,
    idx: i64,
) -> Option<solstone_core_location::body_records::Face> {
    let (facet, day) = activity_file_place(path)?;
    let records = files.records(journal_root, path)?;
    let position = usize::try_from(idx).ok()?;
    let record = solstone_core_format::content::body_record_at(&records, position)?;
    crate::dispatch::body_face_for(journal_root, snapshot, &facet, &day, record).ok()
}

/// Admits a body record's search entry for this connection, inside the query.
pub(crate) fn body_entry_admission(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    files: ActivityDayFiles,
) -> solstone_core_indexer_query::BodyEntryAdmission {
    let journal = journal_root.to_path_buf();
    let snapshot = snapshot.clone();
    std::sync::Arc::new(move |path: &str, idx: i64| {
        body_entry_face(&journal, &snapshot, &files, path, idx).is_some()
    })
}

pub(crate) fn window_gate<'a>(
    journal_root: &'a Path,
    snapshot: &'a ConnectionReadSnapshot,
) -> impl Fn(&str, &str, Option<&str>) -> bool + 'a {
    let transcript_boundary = boundary(snapshot, Some(AdmittedCategory::Body), None);
    // Body windows are body data: a connection without the body permission reads
    // none, whatever tool it calls.
    let body = holds_body(snapshot);
    // No window of a day the owner hid is read, through any tool, and none at
    // all while which days are hidden can't be read.
    let hides = BodyHides::read(journal_root);
    // A window's answer holds for the read: many records share one window.
    let known = std::cell::RefCell::new(BTreeMap::<(String, String, String), bool>::new());
    move |day: &str, stream: &str, window: Option<&str>| match window {
        Some(_) if !body || hides.day_hidden(day) => false,
        Some(name) => {
            let key = (day.to_owned(), stream.to_owned(), name.to_owned());
            if let Some(&allowed) = known.borrow().get(&key) {
                return allowed;
            }
            let probe = segment_assignment_probe_path(day, stream, name);
            let allowed = live_allows(journal_root, &transcript_boundary, &probe, Some(stream))
                && released(journal_root, day, stream, name);
            known.borrow_mut().insert(key, allowed);
            allowed
        }
        None => false,
    }
}

/// The windows a body face was composed from, named as the audit names a
/// window `get_transcript` read, so a re-import that changes an answer
/// changes what it names.
pub(crate) fn face_windows(face: &solstone_core_location::body_records::Face) -> Vec<String> {
    face.members
        .iter()
        .filter_map(|member| {
            Some(format!(
                "{}/{}/{}",
                member.day,
                member.stream,
                member.window.as_ref()?
            ))
        })
        .collect()
}

/// Add each of `windows` to `targets` once.
pub(crate) fn name_windows(targets: &mut Vec<String>, windows: Vec<String>) {
    for window in windows {
        if !targets.contains(&window) {
            targets.push(window);
        }
    }
}

/// The labels of body sources this connection can read on `day`, for a list that
/// leaves their windows out.
pub(crate) fn other_sources(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    hides: &BodyHides,
    day: &str,
) -> Vec<String> {
    let labels = source_labels(journal_root);
    BODY_RECORD_STREAMS
        .iter()
        .filter_map(|stream| {
            let label = labels.get(*stream)?;
            let readable = readable_windows(journal_root, snapshot, hides, day, stream).ok()?;
            (!readable.is_empty()).then(|| label.clone())
        })
        .collect()
}

/// A Strava tile a transcript list leaves out: a live workout, body data alone.
/// The id of the workout a Strava tile belongs to, or its path when its file
/// can't say, so a tile that can't be read still counts as one.
pub(crate) fn workout_id(segment: &Segment) -> String {
    std::fs::read(segment.path().join(STRAVA_TILE_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|tile| tile.get("activity_id").map(Value::to_string))
        .unwrap_or_else(|| segment.path().display().to_string())
}

pub(crate) fn is_left_out_workout(segment: &Segment) -> bool {
    segment.path().join(STRAVA_TILE_FILE).is_file()
        && solstone_core_segment::is_body_only_segment(segment)
}

fn minutes(seconds: u32) -> String {
    format!("{:02}:{:02}", seconds / 3600, (seconds % 3600) / 60)
}

fn parse_minutes(text: &str) -> Option<u32> {
    let (hour, minute) = text.split_once(':')?;
    Some(hour.parse::<u32>().ok()? * 3600 + minute.parse::<u32>().ok()? * 60)
}

/// The stretches of a range no window covers. A whole-day window states something
/// about a date, not a stretch of time, so it isn't coverage.
pub(crate) fn gaps_in(windows: &[(u32, u32)], from: u32, to: u32) -> Vec<Value> {
    let mut gaps = Vec::new();
    let mut reached = from;
    for &(start, end) in windows {
        if end - start >= 86_400 {
            continue;
        }
        if start > reached {
            gaps.push(json!({ "from": minutes(reached), "to": minutes(start) }));
        }
        reached = reached.max(end);
    }
    if reached < to {
        gaps.push(json!({ "from": minutes(reached), "to": minutes(to) }));
    }
    gaps
}

/// One row ready to serve, at its place in time order: its instant, kept inside
/// its window, then its window, then its id.
type Keyed = (i64, String, String, String);

/// The one clock a read goes by. Windows sit on the zone a source fixed at its first
/// import, whatever offset a row states, so a row is found and ordered by its instant
/// against that zone. A source with no such record goes by each row's stated clock,
/// and a row's place stays inside its window either way, so no read can lose it.
#[derive(Clone)]
struct Clock {
    date: Date,
    zone: Option<TimeZone>,
}

impl Clock {
    fn new(journal_root: &Path, stream: &str, date: Date) -> Self {
        let zone = source_zone(journal_root, stream);
        Self { date, zone }
    }

    /// A time's place in order, comparable with `bound`.
    fn order(&self, time: &ZonedDateTime) -> i64 {
        match self.zone {
            Some(_) => instant_millis(time),
            None => civil_millis(time.datetime()),
        }
    }

    /// Where `seconds` into the day falls in order: the first time the clock shows
    /// it, or the last for a window's end, since an hour can repeat in the fall. A
    /// clock time skipped in the spring falls where the clock resumes.
    fn bound(&self, seconds: u32, last: bool) -> i64 {
        let midnight = self.date.to_datetime(jiff::civil::Time::midnight());
        let into = SignedDuration::from_secs(i64::from(seconds));
        let Ok(local) = midnight.checked_add(into) else {
            // Past the last representable clock time: the plain clock reading.
            return civil_millis(midnight) + i64::from(seconds) * 1000;
        };
        let Some(zone) = &self.zone else {
            return civil_millis(local);
        };
        let found = solstone_core_time::from_local(zone, local);
        let found = if last {
            found.latest()
        } else {
            found.earliest()
        };
        found
            .or_else(|| {
                local
                    .checked_add(SignedDuration::from_hours(1))
                    .ok()
                    .and_then(|later| solstone_core_time::from_local(zone, later).earliest())
            })
            .map_or_else(|| civil_millis(local), |time| instant_millis(&time))
    }

    /// Milliseconds into the day on this clock, for reading against a span an agent
    /// asked for. A time on an earlier day reads as the day's start, and one on a
    /// later day as its end.
    fn millis_into_day(&self, time: &ZonedDateTime) -> i64 {
        let local = match &self.zone {
            Some(zone) => time.with_time_zone(zone.clone()).datetime(),
            None => time.datetime(),
        };
        match local.date().cmp(&self.date) {
            std::cmp::Ordering::Less => 0,
            std::cmp::Ordering::Greater => 86_400_000,
            std::cmp::Ordering::Equal => {
                let time = local.time();
                (i64::from(time.hour()) * 3600
                    + i64::from(time.minute()) * 60
                    + i64::from(time.second()))
                    * 1000
                    + i64::from(time.subsec_nanosecond() / 1_000_000)
            }
        }
    }
}

fn day_date(day: &str) -> Option<Date> {
    solstone_core_time::parse_day(day)
}

/// Whole milliseconds since the Unix epoch, rounded toward the past.
fn floor_millis(nanos: i128) -> i64 {
    i64::try_from(nanos.div_euclid(1_000_000)).unwrap_or(i64::MAX)
}

/// The instant `time` names, as [`floor_millis`].
fn instant_millis(time: &ZonedDateTime) -> i64 {
    floor_millis(time.timestamp().as_nanosecond())
}

/// A clock reading taken as if it were UTC, as [`floor_millis`].
fn civil_millis(local: DateTime) -> i64 {
    floor_millis(
        local
            .duration_since(DateTime::constant(1970, 1, 1, 0, 0, 0, 0))
            .as_nanos(),
    )
}

/// Whether a row in `window` belongs to a read of `from..to`, in milliseconds into
/// the day: a timed row inside it, or one whose interval or experience overlaps it.
/// A row's start is taken inside its window, which it's stored in by that start. A
/// row with no time states something about the date, so it always belongs.
fn within(row: &BodyRow, clock: &Clock, window: &Window, from: i64, to: i64) -> bool {
    let Some(begin) = row.begins().and_then(parse_source_time) else {
        return true;
    };
    let begin = clock.millis_into_day(&begin).clamp(
        i64::from(window.start_seconds) * 1000,
        i64::from(window.end_seconds) * 1000 - 1,
    );
    let end = row
        .end
        .as_deref()
        .and_then(parse_source_time)
        .map_or(begin, |end| clock.millis_into_day(&end));
    if end > begin {
        begin < to && end > from
    } else {
        begin >= from && begin < to
    }
}

/// Whether a row is of the kind an agent asked for, by the name its entry gives
/// it: a reading's or count's name, an experience's kind, a sleep stage as sleep
/// and a workout's event as workout.
fn is_kind(row: &BodyRow, kind: &str) -> bool {
    let kind = kind.trim();
    row.label.eq_ignore_ascii_case(kind)
        || row
            .experience
            .as_deref()
            .is_some_and(|experience| experience.eq_ignore_ascii_case(kind))
        || (row.kind == RowKind::Stage && kind.eq_ignore_ascii_case("sleep"))
        || (row.kind == RowKind::Event && kind.eq_ignore_ascii_case("workout"))
}

/// The row ids a same-clock sibling this reader may also read holds, when that
/// sibling is the better copy of this window. A sibling that can't be read or
/// is withheld holds nothing, so no row is lost to it.
fn ids_in_a_better_copy(
    journal_root: &Path,
    readable: &[Window],
    window: &Window,
    day: &str,
    stream: &str,
) -> BTreeSet<String> {
    let clock = window.name.split('_').next().unwrap_or_default();
    let rank = (
        solstone_core_format::content::body_records::copy_rank(&window.name),
        window.name.as_str(),
    );
    let mut ids = BTreeSet::new();
    for sibling in readable.iter().filter(|sibling| {
        sibling.name.split('_').next() == Some(clock)
            && (
                solstone_core_format::content::body_records::copy_rank(&sibling.name),
                sibling.name.as_str(),
            ) < rank
            && released(journal_root, day, stream, &sibling.name)
    }) {
        let Ok(text) = std::fs::read_to_string(sibling.segment.path().join(BODY_RECORDS_FILE))
        else {
            continue;
        };
        ids.extend(
            text.lines()
                .filter_map(|line| serde_json::from_str::<BodyRow>(line).ok())
                .filter(|row| row.kind != RowKind::Moved)
                .map(|row| row.id),
        );
    }
    ids
}

/// A window's rows that belong to a read, each at its place in time order, which
/// stays inside the window. A row before the window's start, such as a tile of an
/// experience that began earlier, takes the window's start. A damaged line reads as
/// itself, at the window's start, and only when no kind is asked for. Under a kind,
/// a window with no line that reads as a row can't be read. From a window that ends
/// before `from`, reached by looking back, only intervals that run into it belong.
fn window_rows(
    window: &Window,
    clock: &Clock,
    from: i64,
    to: i64,
    kind: Option<&str>,
    include_womens_health: bool,
    places_precision: Option<PlacesPrecision>,
) -> Result<Vec<Keyed>, ()> {
    let text =
        std::fs::read_to_string(window.segment.path().join(BODY_RECORDS_FILE)).map_err(|_| ())?;
    let start = clock.bound(window.start_seconds, false);
    let end = clock.bound(window.end_seconds, true);
    let behind = i64::from(window.end_seconds) * 1000 <= from;
    let mut rows = Vec::new();
    let (mut good, mut damaged) = (0_u32, 0_u32);
    for (index, line) in text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
    {
        match serde_json::from_str::<BodyRow>(line) {
            Ok(row) => {
                good += 1;
                if row.kind == RowKind::Moved
                    || window.left_out.contains(&row.id)
                    || (!include_womens_health && is_womens_health_row(&row))
                    || kind.is_some_and(|kind| !is_kind(&row, kind))
                    || (behind
                        && !matches!(row.kind, RowKind::Count | RowKind::Stage | RowKind::Logged))
                    || !within(&row, clock, window, from, to)
                {
                    continue;
                }
                let at = row
                    .begins()
                    .and_then(parse_source_time)
                    .map_or(start, |time| {
                        clock.order(&time).clamp(start, (end - 1).max(start))
                    });
                rows.push((
                    at,
                    window.name.clone(),
                    row.id.clone(),
                    render_row_for_connection(&row, places_precision),
                ));
            }
            Err(_) if kind.is_none() && !behind => rows.push((
                start,
                window.name.clone(),
                format!("~{index:08}"),
                line.to_owned(),
            )),
            Err(_) => damaged += 1,
        }
    }
    if good == 0 && damaged > 0 {
        return Err(());
    }
    Ok(rows)
}

/// `list_transcripts` with `source`: that source's windows, what no window covers,
/// and a reference that reads them all in order.
/// Whether a source label names a body source, whose list and span read are
/// its own.
pub(crate) fn is_body_source(journal_root: &Path, source: &str) -> bool {
    let labels = source_labels(journal_root);
    BODY_RECORD_STREAMS
        .iter()
        .any(|stream| labels.get(*stream).is_some_and(|label| label == source))
}

pub(crate) fn list_source(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: &ValidatedListTranscripts,
    source: &str,
    cursor: Option<&TranscriptListCursor>,
) -> Result<Prepared, DispatchError> {
    let day = request
        .day
        .as_deref()
        .ok_or(DispatchError::Tool(ToolError::SourceArguments))?;
    let labels = source_labels(journal_root);
    let stream = BODY_RECORD_STREAMS
        .iter()
        .find(|stream| labels.get(**stream).is_some_and(|label| label == source));
    let Some(stream) = stream else {
        return Ok(
            Prepared::new(json!({ "transcripts": [] }), 0, Vec::new()).when_empty(NOTHING_READABLE)
        );
    };
    // The source this listing names, so its note speaks only of that source.
    let source_kind = match *stream {
        "import.apple_health" => Some(OldSource::AppleHealth),
        "import.oura" => Some(OldSource::Oura),
        _ => None,
    };
    let earlier =
        source_kind.and_then(|only| earlier_import_note(journal_root, snapshot, day, Some(only)));
    let hides = BodyHides::read(journal_root);
    if !holds_body(snapshot) {
        // Said only where the permission is all that stands between the
        // connection and a window: what its scope can't reach leaves no trace.
        if windows_in_scope(journal_root, snapshot, &hides, day, stream)?.is_empty() {
            return Ok(Prepared::new(json!({ "transcripts": [] }), 0, Vec::new())
                .when_empty(NOTHING_READABLE));
        }
        return Ok(Prepared::new(
            json!({ "transcripts": [], "body_note": BODY_NOT_GRANTED }),
            0,
            Vec::new(),
        )
        .when_empty(BODY_NOT_GRANTED));
    }
    let windows = readable_windows(journal_root, snapshot, &hides, day, stream)?;
    if windows.is_empty() {
        let mut response = json!({ "transcripts": [] });
        if let Some(note) = &earlier {
            response["gaps_note"] = json!(note);
        }
        return Ok(Prepared::new(response, 0, Vec::new()).when_empty(NOTHING_READABLE));
    }
    let from = request
        .time_from
        .as_deref()
        .and_then(parse_minutes)
        .unwrap_or(0);
    let to = request
        .time_to
        .as_deref()
        .and_then(parse_minutes)
        .unwrap_or(86_400);
    let in_range: Vec<&Window> = windows
        .iter()
        .filter(|window| window.start_seconds.max(from) < window.end_seconds.min(to))
        .collect();
    let spans: Vec<(u32, u32)> = in_range
        .iter()
        .map(|window| (window.start_seconds, window.end_seconds))
        .collect();
    let gaps = gaps_in(&spans, from, to);
    // A kind keeps only the windows holding it in the span; what no window covers
    // still describes the source, not the kind.
    let kind = request.kind.as_deref();
    let reach = from.saturating_sub(LOOK_BACK_SECONDS);
    let behind: Vec<&Window> = windows
        .iter()
        .filter(|window| window.start_seconds.max(reach) < window.end_seconds.min(from))
        .collect();
    let ranged = in_range.clone();
    let clock = day_date(day).map(|date| Clock::new(journal_root, stream, date));
    let include_womens_health = holds_womens_health(snapshot);
    let places_precision = snapshot.places_precision();
    let mut unreadable = 0_u64;
    let in_range: Vec<&Window> = match (kind, clock.as_ref()) {
        (Some(kind), Some(clock)) => in_range
            .into_iter()
            .filter(|window| {
                match window_rows(
                    window,
                    clock,
                    i64::from(from) * 1000,
                    i64::from(to) * 1000,
                    Some(kind),
                    include_womens_health,
                    places_precision,
                ) {
                    Ok(rows) => !rows.is_empty(),
                    // It can't say whether it holds the kind, so it isn't listed as
                    // holding it, and it isn't left out unsaid.
                    Err(()) => {
                        unreadable += 1;
                        false
                    }
                }
            })
            .collect(),
        _ => in_range,
    };
    // Entries in the range, none of the kind: say so. With nothing in the range,
    // the list reads as it would without a kind, and with a window that couldn't
    // say, the count stands in for the note.
    let holds_entries = || {
        clock.as_ref().is_some_and(|clock| {
            ranged.iter().any(|window| {
                window_rows(
                    window,
                    clock,
                    i64::from(from) * 1000,
                    i64::from(to) * 1000,
                    None,
                    include_womens_health,
                    places_precision,
                )
                .is_ok_and(|rows| !rows.is_empty())
            })
        })
    };
    // With nothing to list, an interval from before the range is still in it. A
    // window reached only by looking back isn't listed, so the answer says so.
    let mut began_earlier = false;
    if let (true, Some(clock)) = (in_range.is_empty(), clock.as_ref()) {
        for window in &behind {
            match window_rows(
                window,
                clock,
                i64::from(from) * 1000,
                i64::from(to) * 1000,
                kind,
                include_womens_health,
                places_precision,
            ) {
                Ok(rows) => began_earlier |= !rows.is_empty(),
                Err(()) if kind.is_some() => unreadable += 1,
                Err(()) => {}
            }
        }
    }
    if kind.is_some() && in_range.is_empty() && unreadable == 0 && !began_earlier && holds_entries()
    {
        let mut response = json!({ "transcripts": [], "gaps": gaps });
        if let Some(note) = &earlier {
            response["gaps_note"] = json!(note);
        }
        return Ok(Prepared::new(response, 0, Vec::new()).when_empty(KIND_ABSENT));
    }

    let mint = |target| {
        codec()
            .mint(principal.connection, snapshot.generation, target)
            .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))
    };
    let span_reference = mint(ReferenceTarget::SourceSpan(SourceSpanReference {
        day: day.to_owned(),
        stream: (*stream).to_owned(),
        time_from: request.time_from.clone(),
        time_to: request.time_to.clone(),
        kind: request.kind.clone(),
        position: None,
    }))?;

    let after = |window: &&&Window| {
        cursor.is_none_or(|cursor| {
            let last = segment_clock(&cursor.last_name).map_or(0, |clock| clock.start_seconds);
            (window.start_seconds, window.name.as_str()) > (last, cursor.last_name.as_str())
        })
    };
    let mut remaining = in_range.iter().filter(after);
    let mut listed = Vec::new();
    let mut targets = Vec::new();
    let mut last_name = None;
    for window in remaining.by_ref().take(request.limit) {
        last_name = Some(window.name.clone());
        let page = read_body_aware_page(
            &window.segment,
            None,
            None,
            None,
            &window.left_out,
            &snapshot.categories,
            places_precision,
        );
        // Freeze the page before the last deletion observation, as a single read does.
        if !released(journal_root, day, stream, &window.name) {
            continue;
        }
        let Ok(page) = page else {
            unreadable += 1;
            continue;
        };
        let reference = crate::dispatch::segment_reference(
            journal_root,
            principal,
            snapshot,
            SegmentReference {
                day: day.to_owned(),
                stream: (*stream).to_owned(),
                segment: window.name.clone(),
                version: page.version.fingerprint(),
                source_index: None,
                byte_offset: None,
                statement_ordinal: None,
                message: None,
            },
        )?;
        targets.push(format!("{day}/{stream}/{}", window.name));
        listed.push(json!({
            "title": format!("Body records — {day}"),
            "date": day,
            "time": window.label,
            "source": source,
            "content_kinds": [AdmittedCategory::Body.as_str()],
            "reference": reference,
        }));
    }
    let next_cursor = if remaining.next().is_some() {
        let last_name = last_name.ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
        Some(mint(ReferenceTarget::TranscriptList(
            TranscriptListCursor {
                day: request.day.clone(),
                day_from: None,
                day_to: None,
                time_from: request.time_from.clone(),
                time_to: request.time_to.clone(),
                facet: None,
                source: Some(source.to_owned()),
                kind: request.kind.clone(),
                last_day: day.to_owned(),
                last_stream: (*stream).to_owned(),
                last_name,
            },
        ))?)
    } else {
        None
    };
    let count = listed.len();
    let mut response = json!({
        "transcripts": listed,
        "gaps": gaps,
        "span_reference": span_reference,
    });
    if let Some(cursor) = next_cursor {
        response["next_cursor"] = json!(cursor);
    }
    if unreadable > 0 {
        response["unreadable"] = json!(unreadable);
    }
    let mut gaps_notes = Vec::new();
    if holds_other_content(journal_root, snapshot, day, stream)? {
        gaps_notes.push(OTHER_CONTENT.to_owned());
    }
    gaps_notes.extend(earlier);
    if !gaps_notes.is_empty() {
        response["gaps_note"] = json!(gaps_notes.join(" "));
    }
    if began_earlier {
        response["began_earlier"] = json!(BEGAN_EARLIER);
    }
    Ok(Prepared::new(response, count, targets))
}

/// Whether this stream holds, on this day, a segment this connection may read that
/// isn't body records. Scope comes first: what it can't read leaves no trace.
fn holds_other_content(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    day: &str,
    stream: &str,
) -> Result<bool, DispatchError> {
    let Some(transcript_boundary) = crate::dispatch::segment_boundary(snapshot, None) else {
        return Ok(false);
    };
    let segments = iter_segments_skipping_unreadable_streams(journal_root, PathOrDay::Day(day))
        .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
    Ok(segments.iter().any(|segment| {
        segment.record_identity().is_ok_and(|identity| {
            identity.stream == stream
                && !solstone_core_segment::is_body_only_segment(segment)
                // A deleted segment has nothing to read, whatever it held.
                && !matches!(solstone_core_segment::owner_deleted(segment.path()), Ok(true))
                && live_allows(
                    journal_root,
                    &transcript_boundary,
                    &segment_assignment_probe_path(day, stream, identity.name),
                    Some(stream),
                )
        })
    }))
}

/// `get_transcript` on a `span_reference`: the entries in the span, in time order
/// across every window it touches, of one kind when the reference names one.
///
/// Windows of different lengths overlap, so rows are merged by their instants
/// rather than served a window at a time. A window is read only once the walk
/// reaches its start, and one call serves from at most `MAX_WINDOWS_PER_READ` of
/// them; under a kind, a window holding none of it doesn't count.
/// The cursor is the place of the first row not yet served, so a window that
/// changes under it simply reads as it now is from there on.
pub(crate) fn read_source_span(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: &ValidatedGetTranscript,
    reference: SourceSpanReference,
) -> Result<Prepared, DispatchError> {
    let not_found = || DispatchError::Tool(ToolError::ReferenceNotFound);
    // Only a body source has a span of its own, as only one is listed with one.
    if !BODY_RECORD_STREAMS.contains(&reference.stream.as_str()) {
        return Err(not_found());
    }
    let position = request
        .cursor
        .as_deref()
        .map(|token| {
            let ReferenceTarget::SourceSpan(cursor) = codec()
                .resolve(
                    token,
                    ReferenceKind::SourceSpan,
                    principal.connection,
                    snapshot.generation,
                )
                .map_err(|_| not_found())?
            else {
                return Err(not_found());
            };
            if cursor.day != reference.day
                || cursor.stream != reference.stream
                || cursor.time_from != reference.time_from
                || cursor.time_to != reference.time_to
                || cursor.kind != reference.kind
            {
                return Err(not_found());
            }
            cursor.position.ok_or_else(not_found)
        })
        .transpose()?;
    let date = day_date(&reference.day).ok_or_else(not_found)?;
    let clock = Clock::new(journal_root, &reference.stream, date);
    let from = reference
        .time_from
        .as_deref()
        .and_then(parse_minutes)
        .unwrap_or(0);
    let to = reference
        .time_to
        .as_deref()
        .and_then(parse_minutes)
        .unwrap_or(86_400);
    let (from_ms, to_ms) = (i64::from(from) * 1000, i64::from(to) * 1000);
    let kind = reference.kind.as_deref();
    let lower: Option<(i64, String, String)> = position.as_ref().map(|position| {
        (
            position.at_ms,
            position.segment.clone(),
            position.row.clone(),
        )
    });
    let hides = BodyHides::read(journal_root);
    let readable = readable_windows(
        journal_root,
        snapshot,
        &hides,
        &reference.day,
        &reference.stream,
    )?;
    let reach = from.saturating_sub(LOOK_BACK_SECONDS);
    let windows: Vec<&Window> = readable
        .iter()
        .filter(|window| window.start_seconds.max(reach) < window.end_seconds.min(to))
        // A window that ends before the cursor holds nothing still to serve.
        .filter(|window| {
            lower
                .as_ref()
                .is_none_or(|(at, _, _)| clock.bound(window.end_seconds, true) > *at)
        })
        .collect();

    let mut waiting: BinaryHeap<Reverse<Keyed>> = BinaryHeap::new();
    let mut entries = Vec::new();
    let mut targets: Vec<String> = Vec::new();
    let mut unreadable = 0_u64;
    let mut next: Option<SourceSpanPosition> = None;
    let mut reached = 0_usize;
    let mut visited = 0_usize;
    loop {
        // Read every window that starts at or before the earliest row waiting.
        while let Some(window) = windows.get(reached) {
            let start = clock.bound(window.start_seconds, false);
            if waiting.peek().is_some_and(|Reverse(first)| start > first.0)
                || visited == MAX_WINDOWS_PER_READ
            {
                break;
            }
            reached += 1;
            let rows = window_rows(
                window,
                &clock,
                from_ms,
                to_ms,
                kind,
                holds_womens_health(snapshot),
                snapshot.places_precision(),
            );
            // Under a kind, a window holding none of it doesn't count toward the cap.
            if kind.is_none() || rows.as_ref().is_ok_and(|rows| !rows.is_empty()) {
                visited += 1;
            }
            // Freeze the rows before the last deletion observation, as a single read does.
            if !released(
                journal_root,
                &reference.day,
                &reference.stream,
                &window.name,
            ) {
                continue;
            }
            let Ok(mut rows) = rows else {
                unreadable += 1;
                continue;
            };
            // A row an archive merge left a second copy of is read once, from
            // the copy the importer still writes to.
            let better = ids_in_a_better_copy(
                journal_root,
                &readable,
                window,
                &reference.day,
                &reference.stream,
            );
            rows.retain(|row| !better.contains(&row.2));
            waiting.extend(
                rows.into_iter()
                    .filter(|row| {
                        lower.as_ref().is_none_or(|(at, segment, id)| {
                            (row.0, row.1.as_str(), row.2.as_str())
                                >= (*at, segment.as_str(), id.as_str())
                        })
                    })
                    .map(Reverse),
            );
        }
        // Rows before the next unread window's start are all here.
        let bound = windows.get(reached).map(|window| {
            (
                clock.bound(window.start_seconds, false),
                window.name.clone(),
            )
        });
        let first = waiting.peek().map(|Reverse(first)| first.0);
        match (first, &bound) {
            (Some(at), Some((start, _))) if at >= *start => {
                if visited < MAX_WINDOWS_PER_READ {
                    continue;
                }
                next = Some(SourceSpanPosition {
                    at_ms: *start,
                    segment: String::new(),
                    row: String::new(),
                });
                break;
            }
            (None, Some((start, _))) => {
                // Nothing left in what's been read, and the next window waits for a
                // later call.
                next = Some(SourceSpanPosition {
                    at_ms: *start,
                    segment: String::new(),
                    row: String::new(),
                });
                break;
            }
            (None, None) => break,
            _ => {}
        }
        if entries.len() == MAX_ENTRIES_PER_PAGE {
            let Some(Reverse((at_ms, segment, row, _))) = waiting.pop() else {
                break;
            };
            next = Some(SourceSpanPosition {
                at_ms,
                segment,
                row,
            });
            break;
        }
        let Some(Reverse((_, segment, _, text))) = waiting.pop() else {
            break;
        };
        let target = format!("{}/{}/{segment}", reference.day, reference.stream);
        if !targets.contains(&target) {
            targets.push(target);
        }
        entries.push(solstone_core_format::markdown::literal_text(&text));
    }
    let next_cursor = next
        .map(|position| {
            codec()
                .mint(
                    principal.connection,
                    snapshot.generation,
                    ReferenceTarget::SourceSpan(SourceSpanReference {
                        position: Some(position),
                        ..reference.clone()
                    }),
                )
                .map_err(|_| not_found())
        })
        .transpose()?;
    let note = if next_cursor.is_some() {
        SPAN_EMPTY_CONTINUES
    } else {
        SPAN_EMPTY
    };
    let count = entries.len();
    let labels = source_labels(journal_root);
    let mut response = json!({
        "entries": entries,
        "source": crate::dispatch::source_label(&labels, &reference.stream),
        "content_kinds": [AdmittedCategory::Body.as_str()],
        "next_cursor": next_cursor,
    });
    if unreadable > 0 {
        response["unreadable"] = json!(unreadable);
    }
    Ok(Prepared::new(response, count, targets)
        .served_as_is(&["entries"])
        .when_empty(note))
}

/// Whether an activity line thinking once wrote for a Strava workout is a hidden
/// day's: it goes with its file's day, and while the hidden days can't be read
/// no connection reads one.
pub(crate) fn strava_line_hidden(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    day: &str,
    record: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    solstone_core_format::content::is_strava_reading(record)
        && (!holds_body(snapshot) || !strava_day_shown(journal_root, day))
}

/// Whether an unconverted Strava line is left out only for want of the body
/// permission: a line on a day the owner hid is not there to mention.
pub(crate) fn strava_line_needs_body(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    day: &str,
    record: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    solstone_core_format::content::is_strava_reading(record)
        && !holds_body(snapshot)
        && strava_day_shown(journal_root, day)
}

fn strava_day_shown(journal_root: &Path, day: &str) -> bool {
    solstone_core_format::content::body_hidden_days::read_hidden_days(journal_root).hides(day)
        == Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movement_totals_distinguish_missing_from_unsummable() {
        let window = |start, end| {
            Some((
                Timestamp::from_second(start).unwrap(),
                Timestamp::from_second(end).unwrap(),
            ))
        };
        assert_eq!(Counts::default().total(), Ok(None));
        let mut adjacent = Counts::default();
        adjacent.add(2.0, window(0, 1));
        adjacent.add(3.0, window(1, 2));
        assert_eq!(adjacent.total(), Ok(Some(5.0)));

        let mut mixed = Counts::default();
        mixed.add(8.0, None);
        mixed.add(3.0, window(1, 2));
        assert!(mixed.total().is_err());
        let mut overlapping = Counts::default();
        overlapping.add(2.0, window(0, 2));
        overlapping.add(3.0, window(1, 3));
        assert!(overlapping.total().is_err());
        let mut point_overlap = Counts::default();
        point_overlap.add(2.0, window(1, 1));
        point_overlap.add(3.0, window(1, 3));
        assert!(point_overlap.total().is_err());

        let mut movement = Movement::default();
        let mut distance = BodyRow::new(
            RowKind::Count,
            "distance".to_owned(),
            "apple_health",
            "HKQuantityTypeIdentifierDistanceWalkingRunning",
            "distance walking running".to_owned(),
        );
        distance.value = Some("7".to_owned());
        distance.unit = Some("km".to_owned());
        distance.start = Some("2026-03-08T12:05:00Z".to_owned());
        distance.end = distance.start.clone();
        let mut instantaneous = Movement::default();
        instantaneous.add_count(&distance);
        assert_eq!(instantaneous.distance_m.total(), Ok(Some(7000.0)));
        distance.start = None;
        distance.end = None;
        movement.add_count(&distance);
        distance.unit = Some("ft".to_owned());
        movement.add_count(&distance);
        assert!(movement.distance_m.total().is_err());
    }

    #[test]
    fn active_energy_and_reading_bounds_handle_units_ties_and_corrupt_values() {
        let mut movement = Movement::default();
        let mut kj = BodyRow::new(
            RowKind::Count,
            "e1".to_owned(),
            "apple_health",
            "HKQuantityTypeIdentifierActiveEnergyBurned",
            "active energy burned".to_owned(),
        );
        kj.value = Some("418.4".to_owned());
        kj.unit = Some("kJ".to_owned());
        kj.start = Some("2026-03-08T08:00:00Z".to_owned());
        kj.end = Some("2026-03-08T09:00:00Z".to_owned());
        movement.add_count(&kj);
        let mut kcal = kj.clone();
        kcal.id = "e2".to_owned();
        kcal.value = Some("58".to_owned());
        kcal.unit = Some("kcal".to_owned());
        kcal.start = Some("2026-03-08T09:00:00Z".to_owned());
        kcal.end = Some("2026-03-08T10:00:00Z".to_owned());
        movement.add_count(&kcal);
        let total = movement.active_energy_kcal.total().unwrap().unwrap();
        assert!((total - 158.0).abs() < 1e-9, "{total}");

        let mut bad_energy = Movement::default();
        let mut watts = kj.clone();
        watts.unit = Some("W".to_owned());
        bad_energy.add_count(&watts);
        assert!(bad_energy.active_energy_kcal.total().is_err());

        // ReadingBounds tie-breaks min_at/max_at deterministically by earliest instant.
        let mut readings = DayReadings::default();
        for (id, at, val) in [
            ("hr-late-tie", "2026-03-08T15:00:00Z", "120"),
            ("hr-early-max", "2026-03-08T10:00:00Z", "120"),
            ("hr-late-min", "2026-03-08T08:00:00Z", "55"),
            ("hr-early-min", "2026-03-08T06:00:00Z", "55"),
        ] {
            let mut row = BodyRow::new(
                RowKind::Reading,
                id.to_owned(),
                "apple_health",
                "HKQuantityTypeIdentifierHeartRate",
                "heart rate".to_owned(),
            );
            row.at = Some(at.to_owned());
            row.unit = Some("count/min".to_owned());
            row.value = Some(val.to_owned());
            readings.add_reading(&row);
        }
        let mut hrv_s = BodyRow::new(
            RowKind::Reading,
            "hrv-s".to_owned(),
            "apple_health",
            "HKQuantityTypeIdentifierHeartRateVariabilitySDNN",
            "heart rate variability sdnn".to_owned(),
        );
        hrv_s.at = Some("2026-03-08T06:45:00Z".to_owned());
        hrv_s.unit = Some("s".to_owned());
        hrv_s.value = Some("0.057".to_owned());
        readings.add_reading(&hrv_s);
        let mut bad_rhr = BodyRow::new(
            RowKind::Reading,
            "rhr-neg".to_owned(),
            "apple_health",
            "HKQuantityTypeIdentifierRestingHeartRate",
            "resting heart rate".to_owned(),
        );
        bad_rhr.unit = Some("count/min".to_owned());
        bad_rhr.value = Some("-5".to_owned());
        readings.add_reading(&bad_rhr);

        let rendered = readings
            .into_json(
                "Apple Health · imported".to_owned(),
                Some("Watch".to_owned()),
            )
            .unwrap();
        assert_eq!(rendered["heart_rate_bpm"]["min"], 55.0);
        assert_eq!(rendered["heart_rate_bpm"]["max"], 120.0);
        assert_eq!(rendered["heart_rate_bpm"]["count"], 4);
        assert_eq!(rendered["heart_rate_bpm"]["min_at"], "2026-03-08T06:00:00Z");
        assert_eq!(rendered["heart_rate_bpm"]["max_at"], "2026-03-08T10:00:00Z");
        assert!((rendered["hrv_sdnn_ms"]["min"].as_f64().unwrap() - 57.0).abs() < 1e-9);
        assert!(rendered.get("resting_heart_rate_bpm").is_none());
        assert!(rendered["resting_heart_rate_note"].is_string());
    }

    #[test]
    fn gaps_name_what_no_window_covers_and_a_whole_day_covers_nothing() {
        let windows = [
            (0, 86_400),
            (21_600, 21_900),
            (21_900, 22_200),
            (25_200, 25_500),
        ];
        assert_eq!(
            gaps_in(&windows, 21_600, 28_800),
            vec![
                json!({"from": "06:10", "to": "07:00"}),
                json!({"from": "07:05", "to": "08:00"}),
            ]
        );
        assert_eq!(
            gaps_in(&[], 21_600, 28_800),
            vec![json!({"from": "06:00", "to": "08:00"})]
        );
    }
}
