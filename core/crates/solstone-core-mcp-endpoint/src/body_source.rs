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

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Timelike};
use serde_json::{Value, json};
use solstone_core_format::content::body_hidden_days::{HiddenDays, RowDays, read_hidden_days};
use solstone_core_format::content::body_records::{
    BODY_RECORD_STREAMS, BODY_RECORDS_FILE, BodyRow, RowKind, only_moved_rows, parse_source_time,
    render_row,
};
use solstone_core_format::content::strava_body::{STRAVA_SOURCE, STRAVA_TILE_FILE, strava_rows};
use solstone_core_journal_config::Tz;
use solstone_core_journal_io::paths::{
    PathOrDay, Segment, iter_segments_skipping_unreadable_streams,
};
use solstone_core_location::old_imports::{CoverScope, OldSource, covers_on_day, month_words};
use solstone_core_transcripts::segment_clock;

use crate::dispatch::{
    DispatchError, DispatchPrincipal, Prepared, boundary, codec, live_allows,
    segment_assignment_probe_path, source_labels,
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

const NOTHING_READABLE: &str = "No body source with that label holds anything this connection can read on that day. A source's label comes from `other_sources` on a day's list_transcripts, or from that day's body document.";
const SPAN_EMPTY: &str = "No entries in this span.";
const SPAN_EMPTY_CONTINUES: &str =
    "No entries in this part of the span. Pass next_cursor as cursor to read on.";
const KIND_ABSENT: &str = "This source holds entries in that time, but none of the kind asked for. Its entries name their kinds; list without kind to see them.";
const BEGAN_EARLIER: &str = "Nothing in this range is listed, but an entry that began before the range runs into it. Pass span_reference to get_transcript as reference to read it.";
const OTHER_CONTENT: &str = "This source also holds other content on this day that isn't listed here, so these gaps describe only its body segments. List the day without source to see it.";

/// What an earlier import of a body source holds for this day, for a connection
/// that may read the whole journal's transcripts. A bundle has no facet, so a
/// limited connection is told nothing of it.
fn earlier_import_note(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    day: &str,
    only: Option<OldSource>,
) -> Option<String> {
    if !snapshot.categories.contains(&AdmittedCategory::Transcripts)
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
        let reach = (date - chrono::Duration::days(HIDE_REACH_DAYS))
            .format("%Y%m%d")
            .to_string();
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
        if self.reaching(day).is_none() {
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
        let left_out = self.left_out(journal_root, day, stream, &rows);
        let all_hidden = !rows.is_empty() && left_out.len() == rows.len();
        (damaged || !all_hidden).then_some(left_out)
    }
}

/// The zone that placed a body stream's windows, when its source fixed one.
fn source_zone(journal_root: &Path, stream: &str) -> Option<Tz> {
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

/// The body window a live, in-scope, unreleased segment is, in time order. One
/// holding only moved rows is left out.
fn readable_windows(
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
    let transcript_boundary = boundary(snapshot, Some(AdmittedCategory::Transcripts), None);
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
        let grid = solstone_core_format::content::body_records::grid_length(&name);
        let grid_key = grid.and_then(|length| {
            name.split_once('_')
                .map(|(clock, _)| format!("{clock}_{length}"))
        });
        let Some(clock) = grid_key.as_deref().and_then(segment_clock) else {
            continue;
        };
        let probe = segment_assignment_probe_path(day, stream, &name);
        if !live_allows(journal_root, &transcript_boundary, &probe, Some(stream))
            || !released(journal_root, day, stream, &name)
        {
            continue;
        }
        // One holding only rows of a night that started on a hidden day reads
        // as no window, as one holding only moved rows does.
        let Some(left_out) = hides.shown(journal_root, day, stream, &segment) else {
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

/// Whether this connection may read a body window, by day, stream and name: the
/// same test `readable_windows` makes, and the Transcripts category, which that
/// tool requires before it runs. No window to name, its directory gone with
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
/// may read it. `None` when it may not: no Transcripts, no grant on its window,
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
    let transcript_boundary = boundary(snapshot, Some(AdmittedCategory::Transcripts), None);
    // Body windows are transcripts: a connection without that category reads none,
    // whatever tool it calls.
    let transcripts = snapshot.categories.contains(&AdmittedCategory::Transcripts);
    // No window of a day the owner hid is read, through any tool, and none at
    // all while which days are hidden can't be read.
    let hides = BodyHides::read(journal_root);
    // A window's answer holds for the read: many records share one window.
    let known = std::cell::RefCell::new(BTreeMap::<(String, String, String), bool>::new());
    move |day: &str, stream: &str, window: Option<&str>| match window {
        Some(_) if !transcripts || hides.day_hidden(day) => false,
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
fn gaps_in(windows: &[(u32, u32)], from: u32, to: u32) -> Vec<Value> {
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
#[derive(Clone, Copy)]
struct Clock {
    date: NaiveDate,
    zone: Option<Tz>,
}

impl Clock {
    fn new(journal_root: &Path, stream: &str, date: NaiveDate) -> Self {
        let zone = source_zone(journal_root, stream);
        Self { date, zone }
    }

    /// A time's place in order, comparable with `bound`.
    fn order(self, time: DateTime<FixedOffset>) -> i64 {
        match self.zone {
            Some(_) => time.timestamp_millis(),
            None => time.naive_local().and_utc().timestamp_millis(),
        }
    }

    /// Where `seconds` into the day falls in order: the first time the clock shows
    /// it, or the last for a window's end, since an hour can repeat in the fall. A
    /// clock time skipped in the spring falls where the clock resumes.
    fn bound(self, seconds: u32, last: bool) -> i64 {
        let local = self
            .date
            .and_hms_opt(0, 0, 0)
            .expect("midnight exists as a clock time")
            + chrono::Duration::seconds(i64::from(seconds));
        let Some(zone) = self.zone else {
            return local.and_utc().timestamp_millis();
        };
        let found = zone.from_local_datetime(&local);
        let found = if last {
            found.latest()
        } else {
            found.earliest()
        };
        found
            .or_else(|| {
                zone.from_local_datetime(&(local + chrono::Duration::hours(1)))
                    .earliest()
            })
            .map_or_else(
                || local.and_utc().timestamp_millis(),
                |time| time.timestamp_millis(),
            )
    }

    /// Milliseconds into the day on this clock, for reading against a span an agent
    /// asked for. A time on an earlier day reads as the day's start, and one on a
    /// later day as its end.
    fn into_day(self, time: DateTime<FixedOffset>) -> i64 {
        let local = match self.zone {
            Some(zone) => time.with_timezone(&zone).naive_local(),
            None => time.naive_local(),
        };
        match local.date().cmp(&self.date) {
            std::cmp::Ordering::Less => 0,
            std::cmp::Ordering::Greater => 86_400_000,
            std::cmp::Ordering::Equal => {
                i64::from(local.time().num_seconds_from_midnight()) * 1000
                    + i64::from(local.time().nanosecond() / 1_000_000)
            }
        }
    }
}

fn day_date(day: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(day, "%Y%m%d").ok()
}

/// Whether a row in `window` belongs to a read of `from..to`, in milliseconds into
/// the day: a timed row inside it, or one whose interval or experience overlaps it.
/// A row's start is taken inside its window, which it's stored in by that start. A
/// row with no time states something about the date, so it always belongs.
fn within(row: &BodyRow, clock: Clock, window: &Window, from: i64, to: i64) -> bool {
    let Some(begin) = row.begins().and_then(parse_source_time) else {
        return true;
    };
    let begin = clock.into_day(begin).clamp(
        i64::from(window.start_seconds) * 1000,
        i64::from(window.end_seconds) * 1000 - 1,
    );
    let end = row
        .end
        .as_deref()
        .and_then(parse_source_time)
        .map_or(begin, |end| clock.into_day(end));
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
    clock: Clock,
    from: i64,
    to: i64,
    kind: Option<&str>,
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
                        clock.order(time).clamp(start, (end - 1).max(start))
                    });
                rows.push((at, window.name.clone(), row.id.clone(), render_row(&row)));
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
    let mut unreadable = 0_u64;
    let in_range: Vec<&Window> = match (kind, clock) {
        (Some(kind), Some(clock)) => in_range
            .into_iter()
            .filter(|window| {
                match window_rows(
                    window,
                    clock,
                    i64::from(from) * 1000,
                    i64::from(to) * 1000,
                    Some(kind),
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
        clock.is_some_and(|clock| {
            ranged.iter().any(|window| {
                window_rows(
                    window,
                    clock,
                    i64::from(from) * 1000,
                    i64::from(to) * 1000,
                    None,
                )
                .is_ok_and(|rows| !rows.is_empty())
            })
        })
    };
    // With nothing to list, an interval from before the range is still in it. A
    // window reached only by looking back isn't listed, so the answer says so.
    let mut began_earlier = false;
    if let (true, Some(clock)) = (in_range.is_empty(), clock) {
        for window in &behind {
            match window_rows(
                window,
                clock,
                i64::from(from) * 1000,
                i64::from(to) * 1000,
                kind,
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
        let page = solstone_core_transcripts::read_segment_transcript_page_leaving_out(
            &window.segment,
            None,
            None,
            None,
            &window.left_out,
        );
        // Freeze the page before the last deletion observation, as a single read does.
        if !released(journal_root, day, stream, &window.name) {
            continue;
        }
        let Ok(page) = page else {
            unreadable += 1;
            continue;
        };
        let reference = mint(ReferenceTarget::Segment(SegmentReference {
            day: day.to_owned(),
            stream: (*stream).to_owned(),
            segment: window.name.clone(),
            version: page.version.fingerprint(),
            source_index: None,
            byte_offset: None,
            statement_ordinal: None,
        }))?;
        targets.push(format!("{day}/{stream}/{}", window.name));
        listed.push(json!({
            "title": format!("Body records — {day}"),
            "date": day,
            "time": window.label,
            "source": source,
            "reference": reference,
        }));
    }
    let next_cursor = if remaining.next().is_some() {
        let last_name = last_name.ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
        Some(mint(ReferenceTarget::TranscriptList(
            TranscriptListCursor {
                day: request.day.clone(),
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
    let transcript_boundary = boundary(snapshot, Some(AdmittedCategory::Transcripts), None);
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
            let rows = window_rows(window, clock, from_ms, to_ms, kind);
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
    day: &str,
    record: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    solstone_core_format::content::is_strava_reading(record)
        && solstone_core_format::content::body_hidden_days::read_hidden_days(journal_root)
            .hides(day)
            != Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;

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
