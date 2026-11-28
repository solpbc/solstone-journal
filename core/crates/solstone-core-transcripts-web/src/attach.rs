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
pub(crate) struct FolderFacetBadge {
    pub(crate) name: String,
    pub(crate) title: String,
    pub(crate) color: Option<String>,
    pub(crate) icon: Option<String>,
    pub(crate) emoji: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct FolderSegmentSummary {
    pub(crate) folder_id: String,
    pub(crate) folder_name: String,
    pub(crate) file_count: usize,
    pub(crate) content_count: usize,
    pub(crate) details_only_count: usize,
    pub(crate) activity_only: bool,
    pub(crate) kinds: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) facet: Option<FolderFacetBadge>,
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) folder: Option<FolderSegmentSummary>,
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

pub(crate) fn read_folder_facet_badge(journal_root: &Path, slug: &str) -> Option<FolderFacetBadge> {
    let declaration = solstone_core_facets::observe_facet_declaration(journal_root, slug)
        .ok()
        .flatten()?;
    let obj = declaration.value().as_object()?;
    if obj.contains_key("id")
        && !obj
            .get("id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(solstone_core_facets::is_well_formed_facet_id)
    {
        return None;
    }
    let title = obj
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| slug.to_owned());
    let color = obj
        .get("color")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let icon = obj
        .get("icon")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let emoji = obj
        .get("emoji")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Some(FolderFacetBadge {
        name: slug.to_owned(),
        title,
        color,
        icon,
        emoji,
    })
}

pub(crate) fn attach_folder_summaries(
    journal_root: &Path,
    day: &str,
    segments: &mut [TranscriptSegment],
) {
    const KIND_ORDER: [&str; 5] = ["notes", "documents", "transcripts", "screenshots", "photos"];
    for segment in segments {
        let directory = crate::segment_media::physical_segment_dir(
            journal_root,
            day,
            &segment.stream,
            &segment.key,
        );
        let activity_path = directory
            .join("talents")
            .join(solstone_core_format::folder::FOLDER_ACTIVITY_FILE);
        let Ok(raw) = std::fs::read(&activity_path) else {
            continue;
        };
        let Ok(activity) =
            serde_json::from_slice::<solstone_core_format::folder::StoredFolderActivity>(&raw)
        else {
            continue;
        };
        let file_count = activity.events.len();
        let content_count = activity
            .events
            .iter()
            .filter(|ev| ev.landed_file.is_some())
            .count();
        let details_only_count = file_count.saturating_sub(content_count);
        let mut kind_set: BTreeSet<&str> = BTreeSet::new();
        for ev in &activity.events {
            if ev.landed_file.is_some()
                && let Some(kind) = ev.kind.as_deref()
            {
                kind_set.insert(kind);
            }
        }
        let kinds = KIND_ORDER
            .iter()
            .filter(|k| kind_set.contains(**k))
            .map(|k| (*k).to_owned())
            .collect();
        let facet = activity
            .owner_facet
            .as_deref()
            .and_then(|slug| read_folder_facet_badge(journal_root, slug));
        let activity_only = solstone_core_segment::is_activity_only_folder_dir(&directory);
        segment.folder = Some(FolderSegmentSummary {
            folder_id: activity.folder_id,
            folder_name: activity.folder_name,
            file_count,
            content_count,
            details_only_count,
            activity_only,
            kinds,
            facet,
        });
    }
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
                segment.types.iter().any(|kind| {
                    kind == content_type || (content_type == "screen" && kind == "folder_activity")
                }) && overlaps(&segment.start, &segment.end, start, end)
            }) {
                streams.insert(segment.stream.clone());
                let seg_state = segment.data_state.get(content_type).or_else(|| {
                    (content_type == "screen")
                        .then(|| segment.data_state.get("folder_activity"))
                        .flatten()
                });
                if seg_state.map(String::as_str) == Some("analyzed") {
                    state = "analyzed";
                } else if state == "pending" && seg_state.map(String::as_str) == Some("analyzing") {
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
