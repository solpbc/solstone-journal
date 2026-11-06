// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One body source's windows over a span of a day, for an agent.
//!
//! This reaches body windows and nothing else, so capture zones, untimed flags
//! and speaker names don't arise. `docs/BODY_SEGMENTS.md` is the contract.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::Path;

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Timelike};
use serde_json::{Value, json};
use solstone_core_format::content::body_records::{
    BODY_RECORD_STREAMS, BODY_RECORDS_FILE, BodyRow, RowKind, parse_source_time, render_row,
};
use solstone_core_journal_config::Tz;
use solstone_core_journal_io::paths::{
    PathOrDay, Segment, iter_segments_skipping_unreadable_streams,
};
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

struct Window {
    segment: Segment,
    name: String,
    start_seconds: u32,
    end_seconds: u32,
    label: String,
}

fn released(journal_root: &Path, day: &str, stream: &str, name: &str) -> bool {
    crate::source_release::check_segment_transcript_release(journal_root, day, stream, name)
}

/// The body window a live, in-scope, unreleased segment is, in time order.
fn readable_windows(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    day: &str,
    stream: &str,
) -> Result<Vec<Window>, DispatchError> {
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
        {
            continue;
        }
        let name = identity.name.to_owned();
        let Some(clock) = segment_clock(&name) else {
            continue;
        };
        let probe = segment_assignment_probe_path(day, stream, &name);
        if !live_allows(journal_root, &transcript_boundary, &probe, Some(stream))
            || !released(journal_root, day, stream, &name)
        {
            continue;
        }
        windows.push(Window {
            segment,
            name,
            start_seconds: clock.start_seconds,
            end_seconds: clock.exclusive_end_seconds,
            label: clock.label,
        });
    }
    windows.sort_by(|left, right| {
        (left.start_seconds, left.name.as_str()).cmp(&(right.start_seconds, right.name.as_str()))
    });
    Ok(windows)
}

/// The labels of body sources this connection can read on `day`, for a list that
/// leaves their windows out.
pub(crate) fn other_sources(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    day: &str,
) -> Vec<String> {
    let labels = source_labels(journal_root);
    BODY_RECORD_STREAMS
        .iter()
        .filter_map(|stream| {
            let label = labels.get(*stream)?;
            let readable = readable_windows(journal_root, snapshot, day, stream).ok()?;
            (!readable.is_empty()).then(|| label.clone())
        })
        .collect()
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
        let zone = stream
            .strip_prefix("import.")
            .and_then(|source| solstone_core_journal_config::placed_zone(journal_root, source));
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
                if kind.is_some_and(|kind| !is_kind(&row, kind))
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
    let empty =
        || Prepared::new(json!({ "transcripts": [] }), 0, Vec::new()).when_empty(NOTHING_READABLE);
    let Some(stream) = stream else {
        return Ok(empty());
    };
    let windows = readable_windows(journal_root, snapshot, day, stream)?;
    if windows.is_empty() {
        return Ok(empty());
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
        return Ok(
            Prepared::new(json!({ "transcripts": [], "gaps": gaps }), 0, Vec::new())
                .when_empty(KIND_ABSENT),
        );
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
        let page = solstone_core_transcripts::read_segment_transcript_page(
            &window.segment,
            None,
            None,
            None,
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
    if holds_other_content(journal_root, snapshot, day, stream)? {
        response["gaps_note"] = json!(OTHER_CONTENT);
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
    let windows = readable_windows(journal_root, snapshot, &reference.day, &reference.stream)?;
    let reach = from.saturating_sub(LOOK_BACK_SECONDS);
    let windows: Vec<&Window> = windows
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
            let Ok(rows) = rows else {
                unreadable += 1;
                continue;
            };
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
