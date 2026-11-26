// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;
use solstone_core_format::content::conversation::{ConversationItem, conversation_item};
use solstone_core_format::content::kinds::DashboardImportItem;
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
    pub(crate) stream_layout: &'static str,
    pub(crate) start: String,
    pub(crate) end: String,
    pub(crate) types: Vec<String>,
    pub(crate) data_state: BTreeMap<String, String>,
    pub(crate) think: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) item: Option<DashboardImportItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) conversation_items: Vec<ConversationItem>,
    /// Why a recording or screen settled `empty`, `failed` or `failed_final`,
    /// by modality, as the segment detail's `processing_reason_code` says it.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) processing_reason_code: BTreeMap<String, String>,
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
    include_conversations: bool,
) {
    for segment in segments {
        let holds_notes = segment.stream.starts_with("import.")
            || segment.stream == "photos"
            || solstone_core_segment::is_photos_stream(journal_root, &segment.stream)
            || solstone_core_format::obsidian::is_obsidian_stream_name(&segment.stream);
        if !holds_notes {
            continue;
        }
        let directory = crate::segment_media::physical_segment_dir(
            journal_root,
            day,
            &segment.stream,
            &segment.key,
        );
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        let names = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect::<Vec<_>>();
        let relative = crate::segment_media::segment_rel(day, &segment.stream, &segment.key);
        if include_conversations {
            segment.conversation_items = names
                .iter()
                .filter_map(|name| conversation_item(journal_root, &relative, name))
                .collect();
            segment
                .conversation_items
                .sort_by(|left, right| left.file.cmp(&right.file));
            let mut has_conversation = false;
            for item in &segment.conversation_items {
                if let Some(kind @ ("messages" | "conversation")) = item
                    .content_kind
                    .or(item.conversation_state.map(|_| "conversation"))
                {
                    has_conversation = true;
                    if !segment.data_state.contains_key(kind) {
                        segment.types.push(kind.to_owned());
                        segment
                            .data_state
                            .insert(kind.to_owned(), "analyzed".into());
                    }
                }
            }
            // The earlier scanner groups saved dialogue and Markdown with speech.
            // Preserve a genuine recording when it shares this segment with a chat.
            if has_conversation
                && !names.iter().any(|name| {
                    let path = directory.join(name);
                    if segment
                        .conversation_items
                        .iter()
                        .any(|item| item.file == *name && item.content_kind != Some("transcripts"))
                    {
                        return false;
                    }
                    name == solstone_core_format::audio_archive::AUDIO_ARCHIVE_PLACEMENT_FILENAME
                        || path
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .and_then(|ext| {
                                solstone_core_processing_record::media_kind(
                                    &ext.to_ascii_lowercase(),
                                )
                            })
                            == Some(solstone_core_processing_record::MediaKind::Audio)
                        || solstone_core_system_health::is_audio_jsonl(&path)
                })
            {
                segment.types.retain(|kind| kind != "audio");
                segment.data_state.remove("audio");
            }
        }
        segment.item =
            DashboardImportItem::from_segment_files(&relative, names.iter().map(String::as_str));
        let markdown = names
            .iter()
            .any(|name| solstone_core_format::content::markdown_transcript_name(name));
        let jsonl = names.iter().any(|name| {
            name.ends_with("audio.jsonl")
                || name.ends_with("screen.jsonl")
                || name.ends_with("_transcript.jsonl")
        });
        let has_imported_image = (segment.stream == "import.image"
            || segment.stream == "photos"
            || solstone_core_segment::is_photos_stream(journal_root, &segment.stream))
            && names
                .iter()
                .any(|name| crate::segment_media::is_imported_image_original(name));
        if has_imported_image && !jsonl {
            segment.types = vec!["image".to_owned()];
            segment.data_state = BTreeMap::from([("image".to_owned(), "analyzed".to_owned())]);
        } else if markdown && !jsonl && segment.conversation_items.is_empty() {
            segment.types = vec!["markdown".to_owned()];
            segment.data_state = BTreeMap::from([("markdown".to_owned(), "analyzed".to_owned())]);
        }
    }
}

/// Read the processing record's reason for each recording or screen whose
/// state an owner sees as settled without a transcript. Only those segments'
/// sidecar headers are read, a bounded first row each.
pub(crate) fn attach_processing_reason_codes(
    journal_root: &Path,
    day: &str,
    segments: &mut [TranscriptSegment],
) {
    for segment in segments {
        let modalities = ["audio", "screen"]
            .into_iter()
            .filter(|modality| {
                segment
                    .data_state
                    .get(*modality)
                    .is_some_and(|state| crate::segment::reason_code_explains(state))
            })
            .collect::<Vec<_>>();
        if modalities.is_empty() {
            continue;
        }
        let directory = crate::segment_media::physical_segment_dir(
            journal_root,
            day,
            &segment.stream,
            &segment.key,
        );
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        paths.sort();
        for modality in modalities {
            let code = paths
                .iter()
                .filter(|path| sidecar_of(modality, path))
                .filter_map(|path| {
                    solstone_core_processing_record::read_processing_record_header(path)
                })
                .find_map(|record| crate::segment::record_reason_code(&record));
            if let Some(code) = code {
                segment
                    .processing_reason_code
                    .insert(modality.to_owned(), code);
            }
        }
    }
}

fn sidecar_of(modality: &str, path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    match modality {
        "audio" => solstone_core_system_health::is_audio_jsonl(path),
        "screen" => name == "screen.jsonl" || name.ends_with("_screen.jsonl"),
        _ => false,
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
