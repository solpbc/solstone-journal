// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A day's listed segments, read in one go.
//!
//! `list_transcripts` for a day hands back a `span_reference` for what it
//! matched: the same day, time range, facet and source label. `get_transcript`
//! on it reads those segments in time order, a page at a time, so an agent
//! reads a day's conversations without one call per segment.
//!
//! Every segment passes the checks a single `get_transcript` makes, at the
//! moment it is read: scope, the owner's deletion, and what the owner hid.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_indexer_query::ConnectionScope;
use solstone_core_journal_io::paths::{
    PathOrDay, Segment, iter_segments_skipping_unreadable_streams,
};
use solstone_core_transcripts::{SegmentClock, segment_clock};

use crate::dispatch::{
    DispatchError, DispatchPrincipal, Prepared, codec, differing_capture_zone, source_label,
    source_labels,
};
use crate::permissions::ConnectionReadSnapshot;
use crate::possible_names::ConnectionPossibleNames;
use crate::references::{
    ReferenceKind, ReferenceTarget, SegmentSpanInside, SegmentSpanPosition, SegmentSpanReference,
};
use crate::tools::{ToolError, ValidatedGetTranscript};

/// One call reads until it has this much text, then stops at the segment's end.
const SPAN_PAGE_BYTES: usize = 48 * 1024;
/// The most segments one call looks at, so a stretch of segments with nothing
/// said can't hold a call open. A page can be short for this reason.
const MAX_SEGMENTS_PER_READ: usize = 400;

const SPAN_EMPTY: &str =
    "Nothing was said in this span. Text shown on a screen is found with search.";
const SPAN_EMPTY_AFTER: &str = "Nothing more was said in this span.";
const SPAN_UNREADABLE: &str = "Nothing in this part of the span could be read. List that day again, or read a segment on its own.";
const SPAN_EMPTY_CONTINUES: &str =
    "Nothing was read from this part of the span. Pass next_cursor as cursor to read on.";

/// A list narrowed to one source, by the label its rows carry. A stream is
/// admitted when its row would show that label, so what an agent passes back
/// is always what it was shown.
pub(crate) struct SourceFilter {
    labels: BTreeMap<String, String>,
    source: String,
}

impl SourceFilter {
    pub(crate) fn new(journal_root: &Path, source: &str) -> Self {
        Self {
            labels: source_labels(journal_root),
            source: source.to_owned(),
        }
    }

    pub(crate) fn admits(&self, stream: &str) -> bool {
        source_label(&self.labels, stream) == self.source
    }
}

fn parse_minutes(text: &str) -> Option<u32> {
    let (hour, minute) = text.split_once(':')?;
    Some(hour.parse::<u32>().ok()? * 3600 + minute.parse::<u32>().ok()? * 60)
}

/// One segment of a day's span, at its place in time order.
pub(crate) struct SpanSegment {
    pub(crate) segment: Segment,
    pub(crate) stream: String,
    pub(crate) name: String,
    pub(crate) clock: Option<SegmentClock>,
}

impl SpanSegment {
    fn start(&self) -> u32 {
        self.clock.as_ref().map_or(0, |clock| clock.start_seconds)
    }

    fn key(&self) -> (u32, &str, &str) {
        (self.start(), self.stream.as_str(), self.name.as_str())
    }
}

/// The segments a day's list matches, in the list's order, before scope: not
/// calendar notes, not segments holding only body records.
pub(crate) fn day_segments(
    journal_root: &Path,
    day: &str,
    time_from: Option<&str>,
    time_to: Option<&str>,
    source: Option<&SourceFilter>,
) -> Result<Vec<SpanSegment>, DispatchError> {
    let range = (time_from.is_some() || time_to.is_some()).then(|| {
        (
            time_from.and_then(parse_minutes).unwrap_or(0),
            time_to.and_then(parse_minutes).unwrap_or(86_400),
        )
    });
    let mut matching = Vec::new();
    for segment in iter_segments_skipping_unreadable_streams(journal_root, PathOrDay::Day(day))
        .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?
    {
        let Ok(identity) = segment.record_identity() else {
            continue;
        };
        let stream = identity.stream.to_string();
        let name = identity.name.to_string();
        if source.is_some_and(|source| !source.admits(&stream))
            || solstone_core_segment::is_calendar_stream(journal_root, &stream)
            || solstone_core_segment::is_body_only_segment(&segment)
        {
            continue;
        }
        let clock = segment_clock(&name);
        if let Some((from, to)) = range {
            let Some(clock) = &clock else {
                continue;
            };
            if clock.start_seconds.max(from) >= clock.exclusive_end_seconds.min(to) {
                continue;
            }
        }
        matching.push(SpanSegment {
            segment,
            stream,
            name,
            clock,
        });
    }
    matching.sort_by(|a, b| a.key().cmp(&b.key()));
    Ok(matching)
}

/// Whether this connection may read the segment now: the test a read of it
/// makes, then the owner's deletion.
fn readable(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    hides: &crate::body_source::BodyHides,
    facet: Option<&str>,
    day: &str,
    item: &SpanSegment,
) -> bool {
    crate::may_see::segment_shown(
        journal_root,
        snapshot,
        hides,
        facet,
        day,
        &item.stream,
        &item.name,
        &item.segment,
    )
    .is_some()
        && crate::source_release::check_segment_transcript_release(
            journal_root,
            day,
            &item.stream,
            &item.name,
        )
}

/// Whether the label names anything this connection can read on the day.
/// Scope comes first: a source it can't read leaves no trace.
pub(crate) fn day_holds_readable(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    day: &str,
    source: &SourceFilter,
) -> Result<bool, DispatchError> {
    let hides = crate::body_source::BodyHides::read(journal_root);
    Ok(day_segments(journal_root, day, None, None, Some(source))?
        .iter()
        .any(|item| readable(journal_root, snapshot, &hides, None, day, item)))
}

/// The stretches of a day's list no segment this connection can read covers.
pub(crate) fn day_gaps(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    facet: Option<&str>,
    day: &str,
    time_from: Option<&str>,
    time_to: Option<&str>,
    source: Option<&SourceFilter>,
) -> Result<Vec<Value>, DispatchError> {
    let from = time_from.and_then(parse_minutes).unwrap_or(0);
    let to = time_to.and_then(parse_minutes).unwrap_or(86_400);
    let hides = crate::body_source::BodyHides::read(journal_root);
    let spans: Vec<(u32, u32)> = day_segments(journal_root, day, time_from, time_to, source)?
        .iter()
        .filter(|item| readable(journal_root, snapshot, &hides, facet, day, item))
        .filter_map(|item| {
            item.clock
                .as_ref()
                .map(|clock| (clock.start_seconds, clock.exclusive_end_seconds))
        })
        .collect();
    Ok(crate::body_source::gaps_in(&spans, from, to))
}

/// The reference a day's list hands back for what it matched.
pub(crate) fn span_reference(
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    day: &str,
    time_from: Option<&str>,
    time_to: Option<&str>,
    facet: Option<&str>,
    source: Option<&str>,
) -> Result<String, DispatchError> {
    codec()
        .mint(
            principal.connection,
            snapshot.generation,
            ReferenceTarget::SegmentSpan(SegmentSpanReference {
                day: day.to_owned(),
                time_from: time_from.map(str::to_owned),
                time_to: time_to.map(str::to_owned),
                facet: facet.map(str::to_owned),
                source: source.map(str::to_owned),
                position: None,
            }),
        )
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))
}

/// `get_transcript` on a day list's `span_reference`: what was said in the
/// listed segments, in time order, each under its time and source.
pub(crate) fn read_segment_span(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: &ValidatedGetTranscript,
    reference: SegmentSpanReference,
) -> Result<Prepared, DispatchError> {
    let not_found = || DispatchError::Tool(ToolError::ReferenceNotFound);
    let position = request
        .cursor
        .as_deref()
        .map(|token| {
            let ReferenceTarget::SegmentSpan(cursor) = codec()
                .resolve(
                    token,
                    ReferenceKind::SegmentSpan,
                    principal.connection,
                    snapshot.generation,
                )
                .map_err(|_| not_found())?
            else {
                return Err(not_found());
            };
            if cursor.day != reference.day
                || cursor.time_from != reference.time_from
                || cursor.time_to != reference.time_to
                || cursor.facet != reference.facet
                || cursor.source != reference.source
            {
                return Err(not_found());
            }
            cursor.position.ok_or_else(not_found)
        })
        .transpose()?;
    // The facet a span was listed under is still checked against what this
    // connection reads now, as the list checks it on every call.
    if let (Some(facet), ConnectionScope::ChosenFacets { ids }) =
        (reference.facet.as_deref(), &snapshot.scope)
        && !ids.contains(facet)
    {
        return Err(not_found());
    }
    let source = reference
        .source
        .as_deref()
        .map(|source| SourceFilter::new(journal_root, source));
    let segments = day_segments(
        journal_root,
        &reference.day,
        reference.time_from.as_deref(),
        reference.time_to.as_deref(),
        source.as_ref(),
    )?;
    let lower = position.as_ref().map(|position| {
        (
            segment_clock(&position.segment).map_or(0, |clock| clock.start_seconds),
            position.stream.as_str(),
            position.segment.as_str(),
        )
    });
    let labels: BTreeMap<String, String> = source_labels(journal_root);
    let owner_zone = solstone_core_journal_config::owner_zone(journal_root);
    let hides = crate::body_source::BodyHides::read(journal_root);
    let speakers = solstone_core_talent_runtime::agent_speakers(journal_root);
    let possible_names = ConnectionPossibleNames::new(journal_root, snapshot);

    let mut read = Vec::new();
    let mut targets = Vec::new();
    let mut entries_served = 0_usize;
    let mut bytes = 0_usize;
    let mut unreadable = 0_u64;
    let mut next: Option<SegmentSpanPosition> = None;
    // Segments this connection reads, counted for the cap. One it can't read
    // neither counts nor earns a cursor, so it leaves no trace in the paging.
    let mut readable_seen = 0_usize;
    'segments: for item in segments
        .iter()
        .filter(|item| lower.is_none_or(|lower| item.key() >= lower))
    {
        // The test a read of this segment alone makes: out of scope, and
        // what the owner hid of it, read as not there.
        let Some(left_out) = crate::may_see::segment_shown(
            journal_root,
            snapshot,
            &hides,
            reference.facet.as_deref(),
            &reference.day,
            &item.stream,
            &item.name,
            &item.segment,
        ) else {
            continue;
        };
        let released = || {
            crate::source_release::check_segment_transcript_release(
                journal_root,
                &reference.day,
                &item.stream,
                &item.name,
            )
        };
        if !released() {
            continue;
        }
        // Enough for one call: the next one starts at this segment, which
        // this connection reads.
        if bytes >= SPAN_PAGE_BYTES || readable_seen == MAX_SEGMENTS_PER_READ {
            next = Some(SegmentSpanPosition {
                stream: item.stream.clone(),
                segment: item.name.clone(),
                inside: None,
            });
            break;
        }
        readable_seen += 1;
        // The place inside this segment the last call stopped at, then each
        // further page of it while there is room.
        let resumed = position
            .as_ref()
            .filter(|position| position.stream == item.stream && position.segment == item.name)
            .and_then(|position| position.inside.clone());
        let mut inside = resumed.clone();
        let mut entries = Vec::new();
        let mut untimed = false;
        loop {
            #[cfg(feature = "test-hooks")]
            crate::source_freeze_barrier::checkpoint(
                crate::source_freeze_barrier::FreezePoint::Before,
            );
            let page = released().then(|| {
                let continuation = inside.as_ref().map(|inside| {
                    solstone_core_transcripts::SegmentTranscriptCursor {
                        source_index: inside.source_index,
                        byte_offset: inside.byte_offset,
                        statement_ordinal: inside.statement_ordinal,
                        version:
                            solstone_core_transcripts::SegmentTranscriptVersion::from_fingerprint(
                                inside.version,
                            ),
                    }
                });
                solstone_core_transcripts::read_segment_transcript_page_leaving_out(
                    &item.segment,
                    continuation.as_ref(),
                    speakers.as_ref(),
                    possible_names.labels(item.segment.path()).as_ref(),
                    &left_out,
                )
            });
            // Freeze the page before the last deletion observation, as a
            // single read does.
            let allowed = page.is_some() && released();
            #[cfg(feature = "test-hooks")]
            crate::source_freeze_barrier::checkpoint(
                crate::source_freeze_barrier::FreezePoint::After,
            );
            if !allowed {
                // Deleted while it was being read: what was read of it goes
                // with it.
                continue 'segments;
            }
            let page = match page.expect("allowed reads page") {
                Ok(page)
                    if inside
                        .as_ref()
                        .is_none_or(|inside| inside.version == page.version.fingerprint()) =>
                {
                    page
                }
                // A place inside a transcript that has since changed can't be
                // kept, and reading on would drop the rest of it unsaid: the
                // span is read again, as a single read is.
                _ if inside.is_some() => return Err(not_found()),
                _ => {
                    unreadable += 1;
                    continue 'segments;
                }
            };
            untimed |= page.untimed;
            let version = page.version.fingerprint();
            for entry in page.entries {
                let text = solstone_core_format::markdown::literal_text(&entry.text);
                bytes += text.len();
                entries.push(text);
            }
            match page.next {
                Some(more) => {
                    inside = Some(SegmentSpanInside {
                        version,
                        source_index: more.source_index,
                        byte_offset: more.byte_offset,
                        statement_ordinal: more.statement_ordinal,
                    });
                    if bytes >= SPAN_PAGE_BYTES {
                        next = Some(SegmentSpanPosition {
                            stream: item.stream.clone(),
                            segment: item.name.clone(),
                            inside: inside.clone(),
                        });
                        break;
                    }
                }
                None => break,
            }
        }
        if !entries.is_empty() {
            entries_served += entries.len();
            targets.push(format!("{}/{}/{}", reference.day, item.stream, item.name));
            let mut section = json!({
                "time": item.clock.as_ref().map_or("", |clock| clock.label.as_str()),
                "source": source_label(&labels, &item.stream),
                "entries": entries,
            });
            if untimed {
                section["untimed"] = Value::Bool(true);
            }
            if resumed.is_some() {
                section["continued"] = Value::Bool(true);
            }
            if let Some(zone) = differing_capture_zone(
                journal_root,
                &owner_zone,
                &reference.day,
                &item.name,
                &item.segment,
            ) {
                section["capture_zone"] = zone;
            }
            read.push(section);
        }
        if next.is_some() {
            break;
        }
    }
    let next_cursor = next
        .map(|position| {
            codec()
                .mint(
                    principal.connection,
                    snapshot.generation,
                    ReferenceTarget::SegmentSpan(SegmentSpanReference {
                        position: Some(position),
                        ..reference.clone()
                    }),
                )
                .map_err(|_| not_found())
        })
        .transpose()?;
    // An empty page says what its emptiness means: more to come, something
    // that couldn't be read, the end of a span already read from, or a span
    // with nothing said in it.
    let note = if next_cursor.is_some() {
        SPAN_EMPTY_CONTINUES
    } else if unreadable > 0 {
        SPAN_UNREADABLE
    } else if position.is_some() {
        SPAN_EMPTY_AFTER
    } else {
        SPAN_EMPTY
    };
    let mut response = json!({
        "segments": read,
        "next_cursor": next_cursor,
    });
    if unreadable > 0 {
        response["unreadable"] = json!(unreadable);
    }
    Ok(Prepared::new(response, entries_served, targets)
        .served_as_is(&["entries"])
        .when_empty(note))
}
