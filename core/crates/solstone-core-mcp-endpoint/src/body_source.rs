// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One body source's windows over a span of a day, for an agent.
//!
//! This reaches body windows and nothing else, so capture zones, untimed flags
//! and speaker names don't arise. `docs/BODY_SEGMENTS.md` is the contract.

use std::path::Path;

use serde_json::{Value, json};
use solstone_core_format::content::body_records::{BODY_RECORD_STREAMS, BODY_RECORDS_FILE};
use solstone_core_journal_io::paths::{
    PathOrDay, Segment, iter_segments_skipping_unreadable_streams,
};
use solstone_core_transcripts::{SegmentTranscriptReadError, segment_clock};

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
/// Entries on one page: what a single segment's page already holds.
const MAX_ENTRIES_PER_PAGE: usize = solstone_core_transcripts::MAX_TRANSCRIPT_PAGE_ITEMS;

const NOTHING_READABLE: &str = "No body source with that label holds anything this connection can read on that day. A source's label comes from `other_sources` on a day's list_transcripts, or from that day's body document.";
const SPAN_EMPTY: &str = "No entries in this span.";
const SPAN_EMPTY_CONTINUES: &str =
    "No entries in this part of the span. Pass next_cursor as cursor to read on.";
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
    let mut unreadable = 0_u64;
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

/// `get_transcript` on a `span_reference`: the windows' entries in time order.
///
/// A page is whole segment pages, so the segment reader stays as it is. Only the
/// window being paged is bound to its version: a window changed before the walk
/// reaches it simply reads as it now is. A window that changes under an open
/// cursor refuses, as any changed transcript does.
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
            {
                return Err(not_found());
            }
            cursor.position.ok_or_else(not_found)
        })
        .transpose()?;
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
    let windows = readable_windows(journal_root, snapshot, &reference.day, &reference.stream)?;
    let resume_key = position.as_ref().map(|position| {
        let start = segment_clock(&position.segment).map_or(0, |clock| clock.start_seconds);
        (start, position.segment.clone())
    });

    let mut entries = Vec::new();
    let mut targets = Vec::new();
    let mut unreadable = 0_u64;
    let mut next = None;
    let in_span = windows
        .iter()
        .filter(|window| window.start_seconds.max(from) < window.end_seconds.min(to))
        .filter(|window| {
            resume_key.as_ref().is_none_or(|(start, name)| {
                (window.start_seconds, window.name.as_str()) >= (*start, name.as_str())
            })
        });
    'windows: for (visited, window) in in_span.enumerate() {
        if visited == MAX_WINDOWS_PER_READ {
            next = Some(SourceSpanPosition::at_start(&window.name));
            break;
        }
        // Resuming inside this window only if the cursor names it.
        let mut inside = position
            .as_ref()
            .filter(|position| position.segment == window.name)
            .and_then(SourceSpanPosition::inside);
        let mut served = false;
        loop {
            let page = solstone_core_transcripts::read_segment_transcript_page(
                &window.segment,
                inside.as_ref(),
                None,
                None,
            );
            if !released(
                journal_root,
                &reference.day,
                &reference.stream,
                &window.name,
            ) {
                continue 'windows;
            }
            let page = match page {
                Ok(page) => page,
                Err(SegmentTranscriptReadError::Changed) => return Err(not_found()),
                Err(_) => {
                    unreadable += 1;
                    continue 'windows;
                }
            };
            if !entries.is_empty() && entries.len() + page.entries.len() > MAX_ENTRIES_PER_PAGE {
                next = Some(match &inside {
                    Some(cursor) => SourceSpanPosition::within(&window.name, cursor),
                    None => SourceSpanPosition::at_start(&window.name),
                });
                break 'windows;
            }
            entries.extend(
                page.entries
                    .into_iter()
                    .map(|item| solstone_core_format::markdown::literal_text(&item.text)),
            );
            if !served {
                targets.push(format!(
                    "{}/{}/{}",
                    reference.day, reference.stream, window.name
                ));
                served = true;
            }
            match page.next {
                Some(cursor) if entries.len() >= MAX_ENTRIES_PER_PAGE => {
                    next = Some(SourceSpanPosition::within(&window.name, &cursor));
                    break 'windows;
                }
                Some(cursor) => inside = Some(cursor),
                None => break,
            }
        }
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
