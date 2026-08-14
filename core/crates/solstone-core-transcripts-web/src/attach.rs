// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;
use solstone_core_system_health::{
    DataStateMap, FilesystemHealthLogSource, SegmentInput, ThoughtVerdict, TimeRange,
    lookup_segment_progress, read_segment_progress, segment_fully_sensed, segment_fully_thought,
    segment_requires_processing,
};

use crate::TranscriptError;

#[derive(Clone, Serialize)]
pub(crate) struct TranscriptSegment {
    pub(crate) key: String,
    pub(crate) stream: String,
    pub(crate) start: String,
    pub(crate) end: String,
    pub(crate) types: Vec<String>,
    pub(crate) data_state: BTreeMap<String, String>,
    pub(crate) think: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct RangePayload {
    start: String,
    end: String,
    streams: Vec<String>,
    state: String,
    think: Option<String>,
}

pub(crate) fn normalize_markdown_only_segments(
    journal_root: &Path,
    day: &str,
    segments: &mut [TranscriptSegment],
) {
    for segment in segments {
        let directory = journal_root
            .join("chronicle")
            .join(day)
            .join(&segment.stream)
            .join(&segment.key);
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        let names = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect::<Vec<_>>();
        let markdown = names
            .iter()
            .any(|name| name == "imported.md" || name.ends_with("_transcript.md"));
        let jsonl = names.iter().any(|name| {
            name.ends_with("audio.jsonl")
                || name.ends_with("screen.jsonl")
                || name.ends_with("_transcript.jsonl")
        });
        if segment.stream.starts_with("import.") && markdown && !jsonl {
            segment.types = vec!["markdown".to_owned()];
            segment.data_state = BTreeMap::from([("markdown".to_owned(), "analyzed".to_owned())]);
        }
    }
}

pub(crate) fn attach_think_to_segments(
    journal_root: &Path,
    day: &str,
    segments: &mut [TranscriptSegment],
) -> Result<(), TranscriptError> {
    let progress = read_segment_progress(&FilesystemHealthLogSource::new(journal_root), day)
        .map_err(TranscriptError::health)?
        .value;
    for segment in segments {
        let input = SegmentInput {
            key: segment.key.clone(),
            stream: segment.stream.clone(),
            data_state: DataStateMap(segment.data_state.clone()),
        };
        segment.think =
            if !segment_requires_processing(&input) || !segment_fully_sensed(&input.data_state) {
                None
            } else if segment_fully_thought(lookup_segment_progress(
                &progress,
                &segment.stream,
                &segment.key,
            )) == ThoughtVerdict::Complete
            {
                Some("thought".to_owned())
            } else {
                Some("awaiting".to_owned())
            };
    }
    Ok(())
}

pub(crate) fn attach_visible_streams_to_ranges(
    ranges: &[TimeRange],
    segments: &[TranscriptSegment],
    content_type: &str,
) -> Vec<RangePayload> {
    attach_streams_to_ranges(ranges, segments, content_type)
        .into_iter()
        .filter(|range| !range.streams.is_empty())
        .collect()
}

fn attach_streams_to_ranges(
    ranges: &[TimeRange],
    segments: &[TranscriptSegment],
    content_type: &str,
) -> Vec<RangePayload> {
    ranges
        .iter()
        .map(|(start, end)| {
            let mut streams = BTreeSet::new();
            let mut state = "pending";
            let mut think = None;
            for segment in segments.iter().filter(|segment| {
                segment.types.iter().any(|kind| kind == content_type)
                    && overlaps(&segment.start, &segment.end, start, end)
            }) {
                streams.insert(segment.stream.clone());
                if segment.data_state.get(content_type).map(String::as_str) == Some("analyzed") {
                    state = "analyzed";
                } else if state == "pending"
                    && segment.data_state.get(content_type).map(String::as_str) == Some("analyzing")
                {
                    state = "analyzing";
                }
                if segment.think.as_deref() == Some("awaiting") {
                    think = Some("awaiting".to_owned());
                } else if think.is_none() && segment.think.as_deref() == Some("thought") {
                    think = Some("thought".to_owned());
                }
            }
            RangePayload {
                start: start.clone(),
                end: end.clone(),
                streams: streams.into_iter().collect(),
                state: state.to_owned(),
                think,
            }
        })
        .collect()
}

fn overlaps(start: &str, end: &str, range_start: &str, range_end: &str) -> bool {
    // scan_day formats every boundary as zero-padded HH:MM, so lexical order
    // is the same order as the reference's minute conversion, including 24:00.
    start < range_end && end > range_start
}
