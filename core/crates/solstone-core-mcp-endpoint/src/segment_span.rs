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

use jiff::Timestamp;
use serde_json::{Value, json};
use solstone_core_indexer_query::{AdmittedCategory, ConnectionScope};
use solstone_core_indexer_store::classification::FacetDeclarationSet;
use solstone_core_journal_io::paths::{
    PathOrDay, Segment, day_dirs, iter_segments_skipping_unreadable_streams,
};
use solstone_core_transcripts::{SegmentClock, segment_clock};

use crate::dispatch::{
    DispatchError, DispatchPrincipal, Prepared, codec, differing_capture_zone, segment_boundary,
    source_label, source_labels,
};
use crate::permissions::ConnectionReadSnapshot;
use crate::possible_names::ConnectionPossibleNames;
use crate::references::{
    ReferenceKind, ReferenceTarget, SegmentAddress, SegmentReference, SegmentSpanInside,
    SegmentSpanPosition, SegmentSpanReference, TranscriptRangeCursor, TranscriptRangePosition,
};
use crate::speaker_filter::{resolve_speaker_filter, select_spoken_line_indices};
use crate::tools::{ToolError, TranscriptRangeRequest, TranscriptReferenceRequest};

/// One call reads until it has this much text, then stops at the segment's end.
const SPAN_PAGE_BYTES: usize = 48 * 1024;
/// Wire JSON budget for multi-day or speaker-filtered spoken transcript scans.
/// Keeps every page comfortably below client response ceilings while avoiding
/// excessive paging on month-scale scans.
const RANGE_PAGE_BYTES: usize = 96 * 1024;
/// The most segments one call looks at, so a stretch of segments with nothing
/// said can't hold a call open. A page can be short for this reason.
const MAX_SEGMENTS_PER_READ: usize = 400;
/// Maximum readable spoken segments examined in one date-range page. Non-speech
/// segments are skipped cheaply before counting against this cap.
const MAX_RANGE_SPEECH_SEGMENTS_SCANNED: usize = 500;

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
            if !clock.overlaps(from, to) {
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
        && solstone_core_transcripts::segment_has_permitted_files(
            &item.segment,
            &snapshot.categories,
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
                .filter(|clock| clock.start_seconds < clock.exclusive_end_seconds)
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
    request: &TranscriptReferenceRequest,
    reference: SegmentSpanReference,
) -> Result<Prepared, DispatchError> {
    if request.speaker.is_some() {
        return read_spoken_range_inner(
            journal_root,
            principal,
            snapshot,
            SpokenRangeQuery {
                day: Some(reference.day),
                day_from: None,
                day_to: None,
                time_from: reference.time_from,
                time_to: reference.time_to,
                facet: reference.facet,
                source: reference.source,
                speaker: request.speaker.clone(),
                context_lines: request.context_lines,
                cursor: request.cursor.clone(),
            },
        );
    }
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

    let mut owner_links = crate::journal_links::Recordings::default();
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
        if !released()
            || !solstone_core_transcripts::segment_has_permitted_files(
                &item.segment,
                &snapshot.categories,
            )
        {
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
        let mut turn_parts = Vec::new();
        let mut content_kinds = Vec::new();
        let mut unresolved_conversation = false;
        let mut untimed = false;
        let mut last_version;
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
                        message: inside.message,
                        version:
                            solstone_core_transcripts::SegmentTranscriptVersion::from_fingerprint(
                                inside.version,
                            ),
                    }
                });
                crate::body_source::read_body_aware_page(
                    &item.segment,
                    continuation.as_ref(),
                    speakers.as_ref(),
                    possible_names.labels(item.segment.path()).as_ref(),
                    &left_out,
                    &snapshot.categories,
                    snapshot.places_precision(),
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
            unresolved_conversation |= page.unresolved_conversation;
            for kind in page.content_kinds {
                if !content_kinds.contains(&kind) {
                    content_kinds.push(kind);
                }
            }
            let version = page.version.fingerprint();
            last_version = version;
            let empty_page = page.entries.is_empty();
            for entry in page.entries {
                if let Some(part) = entry.turn_part {
                    turn_parts.push(json!({"entry":entries.len(),"part":part}));
                }
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
                        message: more.message,
                    });
                    if empty_page || bytes >= SPAN_PAGE_BYTES {
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
                "content_kinds": content_kinds,
                "entries": entries,
                "reference": crate::dispatch::segment_reference(journal_root, principal, snapshot, SegmentReference {
                    day: reference.day.clone(), stream: item.stream.clone(), segment: item.name.clone(),
                    version: last_version, source_index: None,
                    byte_offset: None, statement_ordinal: None, message:None,
                })?,
            });
            if !turn_parts.is_empty() {
                section["turn_parts"] = json!(turn_parts);
            }
            if unresolved_conversation {
                section["conversation_state"] = json!("unresolved");
            }
            owner_links.attach(
                journal_root,
                &mut section,
                &reference.day,
                &item.stream,
                &item.name,
            );
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

struct SpokenRangeQuery {
    day: Option<String>,
    day_from: Option<String>,
    day_to: Option<String>,
    time_from: Option<String>,
    time_to: Option<String>,
    facet: Option<String>,
    source: Option<String>,
    speaker: Option<String>,
    context_lines: usize,
    cursor: Option<String>,
}

/// Multi-day or single-day spoken transcript scan directly on `get_transcript`.
pub(crate) fn read_transcript_range(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: &TranscriptRangeRequest,
) -> Result<Prepared, DispatchError> {
    read_spoken_range_inner(
        journal_root,
        principal,
        snapshot,
        SpokenRangeQuery {
            day: request.day.clone(),
            day_from: request.day_from.clone(),
            day_to: request.day_to.clone(),
            time_from: None,
            time_to: None,
            facet: request.facet_id.clone(),
            source: request.source.clone(),
            speaker: request.speaker.clone(),
            context_lines: request.context_lines,
            cursor: request.cursor.clone(),
        },
    )
}

fn read_spoken_range_inner(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    query: SpokenRangeQuery,
) -> Result<Prepared, DispatchError> {
    let not_found = || DispatchError::Tool(ToolError::ReferenceNotFound);
    if !snapshot.categories.contains(&AdmittedCategory::Transcripts) {
        return Err(not_found());
    }
    if let (Some(facet), ConnectionScope::ChosenFacets { ids }) =
        (query.facet.as_deref(), &snapshot.scope)
        && !ids.contains(facet)
    {
        return Err(not_found());
    }
    let position = query
        .cursor
        .as_deref()
        .map(|token| {
            let ReferenceTarget::TranscriptRange(cursor) = codec()
                .resolve(
                    token,
                    ReferenceKind::TranscriptRange,
                    principal.connection,
                    snapshot.generation,
                )
                .map_err(|_| not_found())?
            else {
                return Err(not_found());
            };
            if cursor.day != query.day
                || cursor.day_from != query.day_from
                || cursor.day_to != query.day_to
                || cursor.time_from != query.time_from
                || cursor.time_to != query.time_to
                || cursor.facet != query.facet
                || cursor.source != query.source
                || cursor.speaker != query.speaker
                || cursor.context_lines != query.context_lines
                || cursor.single_segment.is_some()
            {
                return Err(not_found());
            }
            Ok(cursor.position)
        })
        .transpose()?;

    let speakers = solstone_core_talent_runtime::agent_speakers(journal_root);
    let speaker_filter = query
        .speaker
        .as_deref()
        .map(|raw| {
            resolve_speaker_filter(
                journal_root,
                principal,
                snapshot,
                query.facet.as_deref(),
                speakers.as_ref(),
                raw,
            )
        })
        .transpose()?;

    let owner_zone = solstone_core_journal_config::owner_zone(journal_root);
    let today = solstone_core_time::day_key(
        Timestamp::now()
            .to_zoned_datetime(owner_zone.clone())
            .date(),
    );
    let day_map =
        day_dirs(journal_root).map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
    let mut days: Vec<String> = if let Some(ref day) = query.day {
        if day.as_str() <= today.as_str() && day_map.contains_key(day) {
            vec![day.clone()]
        } else {
            Vec::new()
        }
    } else {
        day_map
            .into_keys()
            .filter(|day| {
                day.as_str() <= today.as_str()
                    && query
                        .day_from
                        .as_deref()
                        .is_none_or(|from| day.as_str() >= from)
                    && query.day_to.as_deref().is_none_or(|to| day.as_str() <= to)
            })
            .collect()
    };
    days.sort_unstable();

    let source_filter = query
        .source
        .as_deref()
        .map(|source| SourceFilter::new(journal_root, source));
    let labels: BTreeMap<String, String> = source_labels(journal_root);
    let hides = crate::body_source::BodyHides::read(journal_root);
    let declarations = FacetDeclarationSet::from_journal(journal_root).ok();
    let seg_boundary = segment_boundary(snapshot, query.facet.as_deref());
    let possible_names = ConnectionPossibleNames::new(journal_root, snapshot);
    let mut owner_links = crate::journal_links::Recordings::default();

    let mut read = Vec::new();
    let mut targets = Vec::new();
    let mut entries_served = 0_usize;
    let mut page_wire_bytes = 64_usize;
    let mut unreadable = 0_u64;
    let mut speech_segments_scanned = 0_usize;
    let mut next: Option<TranscriptRangePosition> = None;

    'days: for day in &days {
        if let Some(ref pos) = position
            && day.as_str() < pos.day.as_str()
        {
            continue;
        }
        let segments = day_segments(
            journal_root,
            day,
            query.time_from.as_deref(),
            query.time_to.as_deref(),
            source_filter.as_ref(),
        )?;
        let lower = position.as_ref().filter(|pos| pos.day == *day).map(|pos| {
            (
                segment_clock(&pos.segment).map_or(0, |clock| clock.start_seconds),
                pos.stream.as_str(),
                pos.segment.as_str(),
            )
        });

        for item in segments
            .iter()
            .filter(|item| lower.is_none_or(|lower| item.key() >= lower))
        {
            let Some(_left_out) = seg_boundary.as_ref().and_then(|boundary| {
                crate::may_see::segment_shown_within(
                    journal_root,
                    boundary,
                    declarations.as_ref()?,
                    &hides,
                    day,
                    &item.stream,
                    &item.name,
                    &item.segment,
                )
            }) else {
                continue;
            };
            let released = || {
                crate::source_release::check_segment_transcript_release(
                    journal_root,
                    day,
                    &item.stream,
                    &item.name,
                )
            };
            if !released()
                || !solstone_core_transcripts::segment_has_spoken_transcript_files(
                    &item.segment,
                    &snapshot.categories,
                )
            {
                continue;
            }
            if (!read.is_empty() && page_wire_bytes >= RANGE_PAGE_BYTES)
                || speech_segments_scanned >= MAX_RANGE_SPEECH_SEGMENTS_SCANNED
            {
                next = Some(TranscriptRangePosition {
                    day: day.clone(),
                    stream: item.stream.clone(),
                    segment: item.name.clone(),
                    next_line_index: None,
                    version: None,
                });
                break 'days;
            }
            speech_segments_scanned += 1;

            let resumed = position
                .as_ref()
                .filter(|pos| {
                    pos.day == *day && pos.stream == item.stream && pos.segment == item.name
                })
                .and_then(|pos| pos.next_line_index.map(|idx| (idx, pos.version)));

            #[cfg(feature = "test-hooks")]
            crate::source_freeze_barrier::checkpoint(
                crate::source_freeze_barrier::FreezePoint::Before,
            );
            let spoken = released().then(|| {
                solstone_core_transcripts::read_spoken_segment_lines(
                    &item.segment,
                    speakers.as_ref(),
                    possible_names.labels(item.segment.path()).as_ref(),
                    Some(&snapshot.categories),
                )
            });
            let allowed = spoken.is_some() && released();
            #[cfg(feature = "test-hooks")]
            crate::source_freeze_barrier::checkpoint(
                crate::source_freeze_barrier::FreezePoint::After,
            );
            if !allowed {
                continue;
            }
            let spoken = match spoken.expect("allowed reads spoken lines") {
                Ok(spoken) => spoken,
                Err(solstone_core_transcripts::SegmentTranscriptReadError::NoPermittedFiles) => {
                    continue;
                }
                Err(_) if resumed.is_some() => return Err(not_found()),
                Err(_) => {
                    unreadable += 1;
                    continue;
                }
            };
            let version = spoken.version.fingerprint();
            if let Some((_, expected_version)) = resumed
                && expected_version != Some(version)
            {
                return Err(not_found());
            }
            let selected_indices: Vec<usize> = match speaker_filter.as_ref() {
                Some(filter) => {
                    select_spoken_line_indices(&spoken.lines, filter, query.context_lines)
                }
                None => (0..spoken.lines.len()).collect(),
            };
            let start_pos = resumed.map_or(0, |(idx, _)| idx);
            if resumed.is_some() && start_pos > selected_indices.len() {
                return Err(not_found());
            }
            if start_pos >= selected_indices.len() {
                continue;
            }

            let seg_overhead =
                320_usize + spoken.speakers.iter().map(|s| s.len() + 4).sum::<usize>();
            let mut entries = Vec::new();
            let mut entries_wire = 0_usize;
            let mut split_at: Option<usize> = None;
            for (pos_idx, &line_idx) in selected_indices.iter().enumerate().skip(start_pos) {
                let text =
                    solstone_core_format::markdown::literal_text(&spoken.lines[line_idx].text);
                let item_wire =
                    serde_json::to_string(&text).map_or(text.len() + 4, |s| s.len() + 1);
                let projected = page_wire_bytes + seg_overhead + entries_wire + item_wire;
                if projected > RANGE_PAGE_BYTES {
                    if entries.is_empty() && !read.is_empty() {
                        next = Some(TranscriptRangePosition {
                            day: day.clone(),
                            stream: item.stream.clone(),
                            segment: item.name.clone(),
                            next_line_index: (start_pos > 0).then_some(start_pos),
                            version: (start_pos > 0).then_some(version),
                        });
                        break 'days;
                    }
                    if !entries.is_empty() {
                        split_at = Some(pos_idx);
                        break;
                    }
                }
                entries_wire += item_wire;
                entries.push(text);
            }
            if entries.is_empty() {
                continue;
            }
            entries_served += entries.len();
            targets.push(format!("{day}/{}/{}", item.stream, item.name));
            let mut section = json!({
                "date": day,
                "time": item.clock.as_ref().map_or("", |clock| clock.label.as_str()),
                "source": source_label(&labels, &item.stream),
                "content_kinds": ["transcripts"],
                "speakers": spoken.speakers,
                "entries": entries,
                "reference": crate::dispatch::segment_reference(
                    journal_root,
                    principal,
                    snapshot,
                    SegmentReference {
                        day: day.clone(),
                        stream: item.stream.clone(),
                        segment: item.name.clone(),
                        version,
                        source_index: None,
                        byte_offset: None,
                        statement_ordinal: None,
                        message: None,
                    },
                )?,
            });
            owner_links.attach(journal_root, &mut section, day, &item.stream, &item.name);
            if spoken.untimed {
                section["untimed"] = Value::Bool(true);
            }
            if resumed.is_some() {
                section["continued"] = Value::Bool(true);
            }
            if let Some(zone) =
                differing_capture_zone(journal_root, &owner_zone, day, &item.name, &item.segment)
            {
                section["capture_zone"] = zone;
            }
            page_wire_bytes += serde_json::to_string(&section)
                .map_or(seg_overhead + entries_wire, |s| s.len() + 1);
            read.push(section);

            if let Some(next_pos) = split_at {
                next = Some(TranscriptRangePosition {
                    day: day.clone(),
                    stream: item.stream.clone(),
                    segment: item.name.clone(),
                    next_line_index: Some(next_pos),
                    version: Some(version),
                });
                break 'days;
            }
        }
    }

    let next_cursor = next
        .map(|position| {
            codec()
                .mint(
                    principal.connection,
                    snapshot.generation,
                    ReferenceTarget::TranscriptRange(TranscriptRangeCursor {
                        day: query.day.clone(),
                        day_from: query.day_from.clone(),
                        day_to: query.day_to.clone(),
                        time_from: query.time_from.clone(),
                        time_to: query.time_to.clone(),
                        facet: query.facet.clone(),
                        source: query.source.clone(),
                        speaker: query.speaker.clone(),
                        context_lines: query.context_lines,
                        single_segment: None,
                        position,
                    }),
                )
                .map_err(|_| not_found())
        })
        .transpose()?;

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

/// Single-segment `get_transcript` when filtered by `speaker` (and optional `context_lines`).
pub(crate) fn read_spoken_single_segment(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    reference: &SegmentAddress,
    request: &TranscriptReferenceRequest,
    wire_reference: Option<&str>,
) -> Result<Prepared, DispatchError> {
    let not_found = || DispatchError::Tool(ToolError::ReferenceNotFound);
    if !snapshot.categories.contains(&AdmittedCategory::Transcripts) {
        return Err(not_found());
    }
    let owner_zone = solstone_core_journal_config::owner_zone(journal_root);
    let segment =
        iter_segments_skipping_unreadable_streams(journal_root, PathOrDay::Day(&reference.day))
            .map_err(|_| not_found())?
            .into_iter()
            .find(|segment| {
                segment
                    .record_identity()
                    .is_ok_and(|id| id.stream == reference.stream && id.name == reference.segment)
            })
            .ok_or_else(not_found)?;
    let _left_out = crate::may_see::segment_shown(
        journal_root,
        snapshot,
        &crate::body_source::BodyHides::read(journal_root),
        None,
        &reference.day,
        &reference.stream,
        &reference.segment,
        &segment,
    )
    .ok_or_else(not_found)?;

    let resumed = request
        .cursor
        .as_deref()
        .map(|token| {
            let ReferenceTarget::TranscriptRange(cursor) = codec()
                .resolve(
                    token,
                    ReferenceKind::TranscriptRange,
                    principal.connection,
                    snapshot.generation,
                )
                .map_err(|_| not_found())?
            else {
                return Err(not_found());
            };
            let expected_seg = Some((
                reference.day.clone(),
                reference.stream.clone(),
                reference.segment.clone(),
            ));
            if cursor.single_segment != expected_seg
                || cursor.speaker != request.speaker
                || cursor.context_lines != request.context_lines
            {
                return Err(not_found());
            }
            let idx = cursor.position.next_line_index.ok_or_else(not_found)?;
            Ok((idx, cursor.position.version))
        })
        .transpose()?;

    let speakers = solstone_core_talent_runtime::agent_speakers(journal_root);
    let speaker_raw = request
        .speaker
        .as_deref()
        .ok_or(DispatchError::InvalidInput)?;
    let speaker_filter = resolve_speaker_filter(
        journal_root,
        principal,
        snapshot,
        None,
        speakers.as_ref(),
        speaker_raw,
    )?;

    #[cfg(feature = "test-hooks")]
    crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::Before);
    let released = || {
        crate::source_release::check_segment_transcript_release(
            journal_root,
            &reference.day,
            &reference.stream,
            &reference.segment,
        )
    };
    let allowed = released();
    let spoken = if allowed {
        let possible_names =
            ConnectionPossibleNames::new(journal_root, snapshot).labels(segment.path());
        Some(solstone_core_transcripts::read_spoken_segment_lines(
            &segment,
            speakers.as_ref(),
            possible_names.as_ref(),
            Some(&snapshot.categories),
        ))
    } else {
        None
    };
    let segment_has_audio = allowed
        && snapshot.categories.contains(&AdmittedCategory::AudioMedia)
        && spoken.as_ref().is_some_and(Result::is_ok)
        && crate::audio_media::has_audio(journal_root, segment.path());
    let allowed = allowed && released();
    #[cfg(feature = "test-hooks")]
    crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::After);
    if !allowed {
        return Err(not_found());
    }
    let spoken = match spoken.expect("allowed reads spoken lines") {
        Ok(spoken) => spoken,
        Err(solstone_core_transcripts::SegmentTranscriptReadError::NoPermittedFiles) => {
            return Err(not_found());
        }
        Err(_) => return Err(DispatchError::Tool(ToolError::FileUnreadable)),
    };
    let version = spoken.version.fingerprint();
    if let Some((_, expected_version)) = resumed
        && expected_version != Some(version)
    {
        return Err(not_found());
    }

    let selected_indices =
        select_spoken_line_indices(&spoken.lines, &speaker_filter, request.context_lines);
    let start_pos = resumed.map_or(0, |(idx, _)| idx);
    if resumed.is_some() && start_pos > selected_indices.len() {
        return Err(not_found());
    }
    let mut entries = Vec::new();
    let mut entries_wire = 0_usize;
    let mut split_at: Option<usize> = None;
    for (pos_idx, &line_idx) in selected_indices.iter().enumerate().skip(start_pos) {
        let text = solstone_core_format::markdown::literal_text(&spoken.lines[line_idx].text);
        let item_wire = serde_json::to_string(&text).map_or(text.len() + 4, |s| s.len() + 1);
        if !entries.is_empty() && entries_wire + item_wire > RANGE_PAGE_BYTES {
            split_at = Some(pos_idx);
            break;
        }
        entries_wire += item_wire;
        entries.push(text);
    }

    let next_cursor = split_at
        .map(|next_pos| {
            codec()
                .mint(
                    principal.connection,
                    snapshot.generation,
                    ReferenceTarget::TranscriptRange(TranscriptRangeCursor {
                        day: None,
                        day_from: None,
                        day_to: None,
                        time_from: None,
                        time_to: None,
                        facet: None,
                        source: None,
                        speaker: request.speaker.clone(),
                        context_lines: request.context_lines,
                        single_segment: Some((
                            reference.day.clone(),
                            reference.stream.clone(),
                            reference.segment.clone(),
                        )),
                        position: TranscriptRangePosition {
                            day: reference.day.clone(),
                            stream: reference.stream.clone(),
                            segment: reference.segment.clone(),
                            next_line_index: Some(next_pos),
                            version: Some(version),
                        },
                    }),
                )
                .map_err(|_| not_found())
        })
        .transpose()?;

    let kept_reference = wire_reference.map_or_else(
        || {
            crate::dispatch::segment_reference(
                journal_root,
                principal,
                snapshot,
                SegmentReference {
                    day: reference.day.clone(),
                    stream: reference.stream.clone(),
                    segment: reference.segment.clone(),
                    version,
                    source_index: None,
                    byte_offset: None,
                    statement_ordinal: None,
                    message: None,
                },
            )
        },
        |token| Ok(token.to_owned()),
    )?;
    let count = entries.len();
    let mut response = json!({
        "entries": entries,
        "content_kinds": ["transcripts"],
        "speakers": spoken.speakers,
        "reference": kept_reference,
        "next_cursor": next_cursor,
    });
    crate::journal_links::Recordings::default().attach(
        journal_root,
        &mut response,
        &reference.day,
        &reference.stream,
        &reference.segment,
    );
    if spoken.untimed {
        response["untimed"] = Value::Bool(true);
    }
    if segment_has_audio {
        response["has_audio"] = Value::Bool(true);
    }
    if let Some(zone) = differing_capture_zone(
        journal_root,
        &owner_zone,
        &reference.day,
        &reference.segment,
        &segment,
    ) {
        response["capture_zone"] = zone;
    }
    let targets = vec![format!(
        "{}/{}/{}",
        reference.day, reference.stream, reference.segment
    )];
    Ok(Prepared::new(response, count, targets)
        .served_as_is(&["entries"])
        .when_empty(SPAN_EMPTY))
}
