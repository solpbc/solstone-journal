// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::Extension;
use axum::Json;
use axum::extract::{Path as RoutePath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use jiff::Timestamp;
use jiff::tz::{Offset, TimeZone};
use serde::Serialize;
use serde_json::{Map, Value, json};
use solstone_core_convey_http::{
    coalesce::ResponseLocale,
    window_catalog::{ENGLISH, response_locale, text},
};
use solstone_core_format::audio_archive::{
    AUDIO_ARCHIVE_PLACEMENT_FILENAME, AudioArchivePlacement,
};
use solstone_core_format::content::{
    Family, RawPerceptFamily, iter_talent_text_projections_for_locale, produce_chunks_for_locale,
    produce_raw_percept_chunks, produce_raw_percept_chunks_for_locale,
};
use solstone_core_format::content::{
    FileFacts, FileFormat, FormatterView, SegmentReader, format_file, segment_file_offers,
};
use solstone_core_format::objects::objects_overlay_for_locale;
use solstone_core_format::segment::segment_parse;
use solstone_core_processing_record::vocab;
use solstone_core_system_health::{DataState, derive_modality_state};
use solstone_core_time::Precision;
use solstone_core_transcripts::POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE;

use crate::day::valid_day;
use crate::segment_media::{
    SegmentMedia, discover, is_imported_image_original, markdown_files, markdown_only,
};
use crate::segment_speakers::{embedding_ids, load, statement_ordinals};
use crate::{AppState, legacy_error_response};

#[derive(Clone, Serialize)]
pub(crate) struct WarningDetail {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) file: String,
    pub(crate) message: String,
    pub(crate) ts: String,
}

struct SegmentContext<'a> {
    day: &'a str,
    stream: &'a str,
    key: &'a str,
    dir: &'a Path,
    root: &'a Path,
    /// The journal's owner zone, which the page shows every time in.
    zone: TimeZone,
    now: Timestamp,
    /// Possible names the owner may be shown beside the speech they label.
    possible_names: Vec<solstone_core_possible_names::LiveEntry>,
}

pub(crate) async fn segment_content(
    selected_locale: Option<Extension<ResponseLocale>>,
    State(state): State<Arc<AppState>>,
    RoutePath((day, stream, key)): RoutePath<(String, String, String)>,
) -> Response {
    let locale = response_locale(selected_locale);
    let now = state.clock.now();
    let journal_root = state.journal_root.clone();
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::TranscriptsSegment,
        move || match (
            in_trash(&journal_root, &day, &stream, &key),
            prepare_segment_localized(locale, &journal_root, &day, &stream, &key, now),
        ) {
            // In the trash: what waits there, not an empty segment.
            (Some(trash), _) => Json(serde_json::json!({ "trash": trash })).into_response(),
            (None, Ok(value)) => Json(value).into_response(),
            (None, Err(response)) => response,
        },
    )
    .await
}

/// Where a segment that left for the trash waits: its item, its erase date,
/// and the stretch of time it went with, if it did. A segment in a stretch is
/// recovered or erased only with the stretch.
fn in_trash(root: &Path, day: &str, stream: &str, key: &str) -> Option<Value> {
    let rel = solstone_core_retention::layout::segment_rel(day, stream, key);
    let holder = solstone_core_trash::holders(root, &rel, "")
        .ok()?
        .into_iter()
        .find_map(|id| solstone_core_trash::item(root, &id).ok().flatten())?;
    // Only a segment that left whole: its originals alone in the trash leave
    // the segment itself in the journal.
    if !matches!(
        holder.unit,
        solstone_core_removals::Unit::Segment { .. } | solstone_core_removals::Unit::Import { .. }
    ) {
        return None;
    }
    let mut trash = serde_json::json!({"id": holder.id, "erase_at": holder.erase_at});
    if let Some(stretch) = holder.stretch {
        // The whole stretch's bounds: what all of it, waiting now, spans.
        let members = solstone_core_trash::stretch_members(root, &stretch.id).unwrap_or_default();
        let parts: Vec<_> = members
            .iter()
            .filter_map(|item| item.stretch.as_ref())
            .collect();
        let from = parts
            .iter()
            .map(|part| part.from.as_str())
            .filter(|at| !at.is_empty())
            .min();
        let to = parts.iter().map(|part| part.to.as_str()).max();
        trash["stretch"] = serde_json::json!({"id": stretch.id, "from": from, "to": to});
    }
    Some(trash)
}

#[cfg(test)]
#[allow(clippy::result_large_err)]
fn prepare_segment(
    root: &Path,
    day: &str,
    stream: &str,
    key: &str,
    now: Timestamp,
) -> Result<Value, Response> {
    prepare_segment_localized(ENGLISH, root, day, stream, key, now)
}

#[allow(clippy::result_large_err)]
fn prepare_segment_localized(
    locale: &str,
    root: &Path,
    day: &str,
    stream: &str,
    key: &str,
    now: Timestamp,
) -> Result<Value, Response> {
    if !valid_day(day) {
        return Err(legacy_error_response(
            "invalid_day",
            text(
                locale,
                "journal.window.transcripts.response.error.that.day.couldn.t.be.used",
            ),
            text(
                locale,
                "journal.window.transcripts.response.detail.invalid.day.format",
            ),
            StatusCode::NOT_FOUND,
        ));
    }
    if !valid_stream(stream) {
        return Err(invalid(
            locale,
            text(
                locale,
                "journal.window.transcripts.response.detail.invalid.stream.format",
            ),
        ));
    }
    if !valid_key(key) {
        return Err(invalid(
            locale,
            text(
                locale,
                "journal.window.transcripts.response.detail.invalid.segment.key.format",
            ),
        ));
    }
    let dir = root
        .join("chronicle")
        .join(crate::segment_media::segment_rel(day, stream, key));
    if !dir.is_dir() {
        return Err(invalid(
            locale,
            text(
                locale,
                "journal.window.transcripts.response.detail.segment.directory.not.found",
            ),
        ));
    }
    let markdown_only = markdown_only(&dir, stream);
    let zone = solstone_core_journal_config::owner_zone(root);
    let context = SegmentContext {
        day,
        stream,
        key,
        dir: &dir,
        root,
        zone: zone.clone(),
        now,
        possible_names: solstone_core_possible_names::live_entries(
            root,
            &dir,
            &solstone_core_possible_names::Viewer::Owner,
        ),
    };
    let unclaimed_images = solstone_core_system_health::unclaimed_image_state(&dir, stream, now)
        .map_err(|error| {
            legacy_error_response(
                "invalid_segment_or_stream",
                text(
                    locale,
                    "journal.window.transcripts.response.error.that.segment.or.stream.couldn.t",
                ),
                error.to_string(),
                StatusCode::NOT_FOUND,
            )
        })?;
    let unclaimed_image_names: std::collections::BTreeSet<String> = unclaimed_images
        .raw_paths
        .iter()
        .filter_map(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .collect();
    let mut media = discover(&dir, markdown_only, &unclaimed_image_names);
    let mut files = fs::read_dir(&dir)
        .map_err(|error| {
            legacy_error_response(
                "invalid_segment_or_stream",
                text(
                    locale,
                    "journal.window.transcripts.response.error.that.segment.or.stream.couldn.t",
                ),
                error.to_string(),
                StatusCode::NOT_FOUND,
            )
        })?
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    files.sort();
    let placement = fs::read(dir.join(AUDIO_ARCHIVE_PLACEMENT_FILENAME))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<AudioArchivePlacement>(&bytes).ok())
        .filter(|p| !p.parts.is_empty());
    let placement_part_stems: std::collections::HashSet<String> = placement
        .as_ref()
        .map(|p| {
            p.parts
                .iter()
                .filter_map(|part| {
                    Path::new(&part.stored_name)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();
    let has_audio_transcript = files.iter().any(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.ends_with("audio.jsonl")
                    || (name.ends_with(".jsonl")
                        && placement_part_stems.contains(name.trim_end_matches(".jsonl")))
            })
    });
    let mut speakers = load(locale, &dir, root, now, has_audio_transcript);
    let mut warnings = std::mem::take(&mut speakers.warnings);
    let audio_capture = crate::audio_capture::read_audio_capture(&dir);
    if let Some(capture) = &audio_capture {
        warnings.extend(crate::audio_capture::warnings(locale, capture, now));
    }
    let mut chunks = Vec::<Value>::new();
    let mut damaged_files = Vec::<FileDamage>::new();
    let mut conversation_turns = Vec::new();
    let mut conversation_items = Vec::new();
    let relative = crate::segment_media::segment_rel(day, stream, key);
    let mut has_jsonl = BTreeMap::from([("audio".to_owned(), false), ("screen".to_owned(), false)]);
    let mut records: BTreeMap<String, Option<Value>> =
        BTreeMap::from([("audio".to_owned(), None), ("screen".to_owned(), None)]);
    let mut duration = 0.0_f64;
    let mut markdown_added = false;
    let calendar_stream = solstone_core_segment::is_calendar_stream(root, stream);
    for path in &files {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let conversation =
            solstone_core_format::content::conversation::conversation_item(root, &relative, name);
        let written = conversation
            .as_ref()
            .is_some_and(|item| item.content_kind == Some("messages"));
        if let Some(item) = conversation {
            conversation_items.push(item);
        }
        let offered = segment_file_offers(
            &format!("{day}/{stream}/{key}/{name}"),
            SegmentReader::Dashboard,
            FileFacts {
                archive_part: name.ends_with(".jsonl")
                    && placement_part_stems.contains(name.trim_end_matches(".jsonl")),
                calendar_stream,
                ..Default::default()
            },
        );
        let offered_format = offered.first().map(|offer| offer.format);
        let is_audio = offered_format == Some(FileFormat::Raw(RawPerceptFamily::Audio));
        if is_audio && !written {
            has_jsonl.insert("audio".into(), true);
            match read_segment_entries(root, "audio", path, &mut damaged_files) {
                Ok(entries) => {
                    if records["audio"].is_none() {
                        records.insert("audio".into(), processing_record(&entries));
                    }
                    duration = duration.max(audio_duration(&entries, key));
                    audio_chunks(
                        &mut chunks,
                        &mut media,
                        &mut speakers,
                        &context,
                        name,
                        &entries,
                        placement.as_ref(),
                    );
                }
                Err(error) => warnings.push(warning("audio", path, error, now)),
            }
        } else if written || offered_format == Some(FileFormat::Content(Family::AiChat)) {
            match read_segment_entries(root, "transcript", path, &mut damaged_files) {
                Ok(entries) => conversation_turns.extend(imported_conversation_turns(
                    &context, name, written, &entries,
                )),
                Err(error) => warnings.push(warning("transcript", path, error, now)),
            }
        } else if offered_format == Some(FileFormat::Raw(RawPerceptFamily::RawScreen)) {
            has_jsonl.insert("screen".into(), true);
            match read_segment_entries(root, "screen", path, &mut damaged_files) {
                Ok(entries) => {
                    if records["screen"].is_none() {
                        records.insert("screen".into(), processing_record(&entries));
                    }
                    screen_chunks(&mut chunks, &mut media, &context, name, &entries, locale);
                }
                Err(error) => warnings.push(warning("screen", path, error, now)),
            }
        } else if offered_format == Some(FileFormat::Content(Family::Browser)) {
            match read_segment_entries(root, "browser", path, &mut damaged_files) {
                Ok(entries) => browser_chunks(&mut chunks, &zone, name, &entries, locale),
                Err(error) => warnings.push(warning("browser", path, error, now)),
            }
        } else if offered_format == Some(FileFormat::Content(Family::Calendar)) {
            // Each calendar note as plain text, at the moment it was recorded,
            // with the owner found by their own addresses as search has it.
            match fs::read_to_string(path) {
                Ok(text) => {
                    let owner = solstone_core_format::calendar::OwnerAddresses::new(
                        solstone_core_entity::journal_identity_emails(root).unwrap_or_default(),
                    );
                    let notes = text
                        .lines()
                        .filter_map(|line| {
                            solstone_core_format::calendar::parse_calendar_note(line).ok()
                        })
                        .collect::<Vec<_>>();
                    let owners =
                        solstone_core_indexer_store::calendar::note_owners(root, &owner, &notes);
                    for (note, owner) in notes.iter().zip(&owners) {
                        let timestamp = note.changed_at_ms();
                        chunks.push(json!({
                            "type": "calendar",
                            "time": local_time(&zone, timestamp),
                            "timestamp": timestamp,
                            "text": format_file(FileFormat::Content(Family::Calendar), name, "", FormatterView::LocalizedCalendarNote { note, owner, locale }).chunks.into_iter().map(|chunk| chunk.content).collect::<String>(),
                        }));
                    }
                }
                Err(error) => warnings.push(warning("calendar", path, error, now)),
            }
        } else if offered_format == Some(FileFormat::Content(Family::ProtectedVideo)) {
            match fs::read_to_string(path) {
                Ok(text) => {
                    let produced =
                        produce_chunks_for_locale(Family::ProtectedVideo, name, &text, locale);
                    for chunk in produced.chunks {
                        let timestamp = chunk.occurrence_time_ms.map(|value| value.0).unwrap_or(0);
                        chunks.push(json!({
                            "type": "markdown",
                            "time": local_time(&zone, timestamp),
                            "timestamp": timestamp,
                            "markdown": chunk.content,
                        }));
                    }
                }
                Err(error) => warnings.push(warning("protected_video", path, error, now)),
            }
        } else if offered_format == Some(FileFormat::Content(Family::DesktopActivity)) {
            match fs::read_to_string(path) {
                Ok(text) => {
                    let four_part_rel = format!("{day}/{stream}/{key}/{name}");
                    let produced = produce_chunks_for_locale(
                        Family::DesktopActivity,
                        &four_part_rel,
                        &text,
                        locale,
                    );
                    for message in produced.warnings {
                        warnings.push(warning("desktop_activity", path, message.to_string(), now));
                    }
                    for chunk in produced.chunks {
                        let timestamp = chunk.occurrence_time_ms.map(|value| value.0).unwrap_or(0);
                        chunks.push(json!({
                            "type": "markdown",
                            "time": local_time(&zone, timestamp),
                            "timestamp": timestamp,
                            "markdown": chunk.content,
                        }));
                    }
                }
                Err(error) => warnings.push(warning("desktop_activity", path, error, now)),
            }
        }
    }
    warnings.extend(std::mem::take(&mut speakers.warnings));
    for raw_path in &unclaimed_images.raw_paths {
        let Some(raw_name) = raw_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let jsonl_path = raw_path.with_extension("jsonl");
        if jsonl_path.is_file() {
            match read_segment_entries(root, "image", &jsonl_path, &mut damaged_files) {
                Ok(entries) => {
                    let record = processing_record(&entries);
                    let is_failed = record
                        .as_ref()
                        .and_then(|r| r.get("state"))
                        .and_then(Value::as_str)
                        == Some("failed");
                    if !is_failed {
                        let mut registered = false;
                        for entry in &entries {
                            if entry.get("text").and_then(Value::as_str).is_some() {
                                if !registered {
                                    media.register_image_url(day, stream, key, raw_name);
                                    registered = true;
                                }
                                let offset_sec = entry
                                    .get("start")
                                    .and_then(Value::as_str)
                                    .map(hms_seconds)
                                    .unwrap_or(0);
                                let time = wall_time(key, offset_sec as f64);
                                let timestamp = day_timestamp(&zone, day, &time, 0);
                                let mut source_ref = json!({
                                    "raw": raw_name,
                                    "media_kind": "image",
                                });
                                if let Some(err) =
                                    entry.get("detection_error").filter(|v| v.is_object())
                                {
                                    source_ref["detection_error"] =
                                        depict_detection_error(locale, record.as_ref(), entry, err);
                                }
                                let detections = entry.get("detections");
                                if let Some(overlay) =
                                    detections.and_then(|d| objects_overlay_for_locale(d, locale))
                                {
                                    source_ref["objects"] = overlay;
                                }
                                let markdown = format_file(
                                    FileFormat::PhotoDescription,
                                    raw_name,
                                    &serde_json::to_string(entry).expect("JSON object serializes"),
                                    FormatterView::Localized(locale),
                                )
                                .chunks
                                .into_iter()
                                .map(|chunk| chunk.content)
                                .collect::<String>();
                                chunks.push(json!({
                                    "type": "image",
                                    "time": time,
                                    "timestamp": timestamp,
                                    "markdown": markdown,
                                    "source_ref": source_ref,
                                }));
                            }
                        }
                    }
                }
                Err(error) => warnings.push(warning("image", &jsonl_path, error, now)),
            }
        }
    }
    let imported_image_handled = imported_image_entry(&mut chunks, &mut media, &context, &files);
    let legacy_markdown = conversation_items
        .iter()
        .filter(|item| item.file.ends_with(".md"))
        .map(|item| dir.join(&item.file))
        .collect::<Vec<_>>();
    if markdown_only || !legacy_markdown.is_empty() {
        let time = wall_time(key, 0.0);
        let timestamp = day_timestamp(&zone, day, &time, 0);
        for path in if markdown_only {
            markdown_files(&dir)
        } else {
            legacy_markdown
        } {
            if imported_image_handled
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        matches!(name, "image_transcript.md" | "screenshot_transcript.md")
                    })
            {
                continue;
            }
            match fs::read_to_string(&path) {
                Ok(markdown) if !markdown.trim().is_empty() => {
                    let markdown = format_file(
                        FileFormat::Content(Family::Markdown),
                        &path.to_string_lossy(),
                        &markdown,
                        FormatterView::LiteralMarkdown,
                    )
                    .chunks
                    .into_iter()
                    .map(|chunk| chunk.content)
                    .collect::<String>();
                    let display = if solstone_core_format::stored_markdown::has_named_receipt(&path)
                    {
                        solstone_core_format::stored_markdown::project_file(
                            root,
                            &path,
                            markdown.trim(),
                            locale,
                            None,
                        )
                    } else {
                        None
                    };
                    chunks.push(json!({"type":"markdown","time":time,"timestamp":timestamp,"markdown":display.as_deref().unwrap_or(markdown.trim()),"source_ref":{"filename":path.file_name().and_then(|name| name.to_str()).unwrap_or_default()}}));
                    markdown_added = true;
                }
                Ok(_) => {}
                Err(error) => warnings.push(warning("markdown", &path, error, now)),
            }
        }
    }
    chunks.sort_by_key(|chunk| chunk.get("timestamp").and_then(Value::as_i64).unwrap_or(0));
    // A conversation reads in the order its turns were written.
    chunks.extend(conversation_turns);
    let warning_types = warnings
        .iter()
        .map(|warning| warning.kind.as_str())
        .chain(damaged_files.iter().map(|damage| damage.kind.as_str()))
        .filter(|kind| matches!(*kind, "audio" | "screen"))
        .collect::<Vec<_>>();
    let mut data_state = BTreeMap::new();
    for modality in ["audio", "screen"] {
        let has_chunks = chunks
            .iter()
            .any(|chunk| chunk.get("type").and_then(Value::as_str) == Some(modality));
        let state = if has_chunks {
            derive_modality_state(
                &dir,
                modality,
                true,
                has_jsonl[modality],
                media.has_raw_present[modality],
                records[modality].as_ref(),
                now,
            )
        } else if media.purged(modality) {
            DataState::Purged
        } else {
            derive_modality_state(
                &dir,
                modality,
                false,
                has_jsonl[modality],
                media.has_raw_present[modality],
                records[modality].as_ref(),
                now,
            )
        };
        if state != DataState::Absent {
            let state = if !has_chunks
                && warning_types.contains(&modality)
                && state == DataState::Pending
            {
                DataState::Failed
            } else {
                state
            };
            data_state.insert(modality.to_owned(), state.as_str().to_owned());
        }
    }
    if unclaimed_images.state != DataState::Absent {
        data_state.insert("image".into(), unclaimed_images.state.as_str().into());
    }
    if imported_image_handled {
        data_state.insert("image".into(), DataState::Analyzed.as_str().into());
    }
    if markdown_added {
        data_state.insert("markdown".into(), DataState::Analyzed.as_str().into());
    }
    if chunks
        .iter()
        .any(|chunk| chunk.get("type").and_then(Value::as_str) == Some("browser"))
    {
        data_state.insert("browser".into(), DataState::Analyzed.as_str().into());
    }
    screenshot_entry(&mut chunks, &mut media, &context, &files, &data_state);
    let talents_dir = dir.join("talents");
    let mut sense_display = None;
    let mut md_files: std::collections::BTreeMap<_, _> =
        iter_talent_text_projections_for_locale(&talents_dir, "", None, locale)
            .unwrap_or_default()
            .into_iter()
            .map(|entry| {
                if entry.key == "sense"
                    && entry.source_suffix == ".json"
                    && let Some((full, owner)) =
                        solstone_core_format::stored_markdown::project_sense_record(
                            root,
                            &talents_dir.join(&entry.relative_path),
                            &entry.text,
                            locale,
                        )
                {
                    sense_display = owner;
                    return (entry.key, full);
                }
                if entry.key == "sense" {
                    if entry.source_suffix == ".md" {
                        sense_display = solstone_core_format::stored_markdown::project_sense_file(
                            root,
                            &talents_dir.join(&entry.relative_path),
                            &entry.text,
                            locale,
                        );
                    }
                    if sense_display.is_none() {
                        sense_display = solstone_core_format::content::owner_sense_text(&entry.text)
                    }
                }
                let screen = entry.key == "screen_text";
                let projected = if entry.source_suffix == ".md"
                    && matches!(
                        entry.key.as_str(),
                        "objects" | "topics" | "activity" | "screen_text" | "sense"
                    ) {
                    solstone_core_format::stored_markdown::project_file(
                        root,
                        &talents_dir.join(&entry.relative_path),
                        &entry.text,
                        locale,
                        screen.then_some(key),
                    )
                } else {
                    None
                };
                let text = match projected {
                    Some(text) => text,
                    None if screen => {
                        solstone_core_format::content::screen_text::screen_text_with_clock(
                            &entry.text,
                            key,
                        )
                        .into_owned()
                    }
                    None => entry.text,
                };
                (entry.key, text)
            })
            .collect();
    if data_state.contains_key("audio") {
        md_files.remove("audio");
    }
    if data_state.contains_key("screen") {
        md_files.remove("screen");
    }
    let reason_code = extract_modality_reason_codes(&records);
    let processing_reason_code = processing_reason_codes(&records, &data_state);
    relative_warning_files(root, &mut warnings);
    let mut payload = json!({
        "chunks": chunks,
        "audio_file": media.audio_file,
        "duration": duration,
        "video_files": media.video_files,
        "image_files": media.image_files,
        "md_files": md_files,
        "segment_key": key,
        "capture_zone": crate::capture_zone::capture_zone_view(
            day,
            key,
            solstone_core_callosum::read_reported_zone(root, &dir),
            &zone,
        ),
        "media_sizes": media.media_sizes,
        "media_purged": {
            "audio": media.purged("audio"),
            "screen": media.purged("screen"),
        },
        "media_removal": media.media_removal(
            locale,
            root,
            &solstone_core_retention::layout::segment_rel(day, stream, key),
            &dir,
        ),
        "data_state": data_state,
        "signals": signals(&dir, &zone, locale),
        "transcripts_copy": copy_payload(locale),
        "speaker_labels": speakers.state,
        "warnings": warnings.len(),
        "warning_details": warnings,
    });
    if !conversation_items.is_empty() {
        payload["conversation_items"] = json!(conversation_items);
    }
    if let Some(item) =
        solstone_core_format::content::kinds::DashboardImportItem::from_segment_files(
            &crate::segment_media::segment_rel(day, stream, key),
            files.iter().filter_map(|path| path.file_name()?.to_str()),
        )
    {
        payload["item"] = json!(item);
    }
    if let Some(text) = sense_display {
        payload["sense_display"] = Value::String(text);
    }
    if let Some(capture) = audio_capture {
        payload["audio_capture"] = capture;
    }
    if let Some(reason_code) = reason_code
        && let Some(obj) = payload.as_object_mut()
    {
        obj.insert("reason_code".to_owned(), Value::Object(reason_code));
    }
    if !processing_reason_code.is_empty() {
        payload["processing_reason_code"] = json!(processing_reason_code);
    }
    if !damaged_files.is_empty() {
        payload["damaged_lines"] = json!(damaged_files);
    }
    if solstone_core_system_health::holds_location_file(&dir)
        && let Some(obj) = payload.as_object_mut()
    {
        obj.insert("holds_location".to_owned(), Value::Bool(true));
        if let Some(readings) = crate::location::location_reading_count(&dir) {
            obj.insert(
                "location_readings".to_owned(),
                Value::Number(readings.into()),
            );
        }
    }
    if stream.starts_with(GRANOLA_STREAM_PREFIX)
        && let Some(note) = granola_context(&dir)
        && let Some(obj) = payload.as_object_mut()
    {
        obj.insert("granola".to_owned(), note);
    }
    if let Some(folder_activity) = build_folder_activity_payload(root, &dir)
        && let Some(obj) = payload.as_object_mut()
    {
        if folder_activity
            .get("activity_only")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && let Some(ds) = obj.get_mut("data_state").and_then(Value::as_object_mut)
        {
            ds.insert(
                "folder_activity".to_owned(),
                Value::String("analyzed".to_owned()),
            );
        }
        if let Some(cards) = folder_activity
            .get("inline_cards")
            .and_then(Value::as_array)
            && let Some(images) = obj.get_mut("image_files").and_then(Value::as_object_mut)
        {
            for card in cards {
                if let Some(file) = card.get("file").and_then(Value::as_str)
                    && !file.contains(['/', '\\'])
                    && dir.join(file).is_file()
                {
                    images.entry(file.to_owned()).or_insert_with(|| {
                        Value::String(crate::segment_media::url(day, stream, key, file))
                    });
                }
            }
        }
        obj.insert("folder_activity".to_owned(), folder_activity);
    }
    Ok(payload)
}

fn folder_origin_domain(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let after_scheme = trimmed.split_once("://").map_or(trimmed, |(_, rest)| rest);
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
    let host = if host_port.starts_with('[') {
        host_port
            .split(']')
            .next()
            .unwrap_or("")
            .trim_start_matches('[')
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

fn folder_event_verb(
    event: solstone_core_format::folder::FolderEventKind,
    kind: Option<&str>,
) -> &'static str {
    use solstone_core_format::folder::FolderEventKind;
    match event {
        FolderEventKind::Added => "added",
        FolderEventKind::Renamed => "renamed",
        FolderEventKind::Removed => "removed",
        FolderEventKind::Changed => match kind {
            Some("documents" | "transcripts") => "revised",
            _ => "edited",
        },
    }
}

fn build_folder_activity_payload(root: &Path, seg_dir: &Path) -> Option<Value> {
    let activity_path = seg_dir
        .join("talents")
        .join(solstone_core_format::folder::FOLDER_ACTIVITY_FILE);
    let raw = fs::read(&activity_path).ok()?;
    let mut activity: solstone_core_format::folder::StoredFolderActivity =
        serde_json::from_slice(&raw).ok()?;
    activity.events.sort_by(|a, b| {
        a.offset_ms
            .cmp(&b.offset_ms)
            .then_with(|| a.at_ms.cmp(&b.at_ms))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.key.cmp(&b.key))
    });

    let activity_only = solstone_core_segment::is_activity_only_folder_dir(seg_dir);
    let facet = activity
        .owner_facet
        .as_deref()
        .and_then(|slug| crate::attach::read_folder_facet_badge(root, slug));
    let file_count = activity.events.len();
    let content_count = activity
        .events
        .iter()
        .filter(|ev| ev.landed_file.is_some())
        .count();
    let details_only_count = file_count.saturating_sub(content_count);

    let mut kind_set = std::collections::BTreeSet::new();
    let mut events_json = Vec::with_capacity(activity.events.len());
    for ev in &activity.events {
        let details_only = ev.landed_file.is_none();
        if !details_only && let Some(k) = ev.kind.as_deref() {
            kind_set.insert(k.to_owned());
        }
        let verb = folder_event_verb(ev.event, ev.kind.as_deref());
        let origin_domain = ev.origin_url.as_deref().and_then(folder_origin_domain);
        events_json.push(json!({
            "key": ev.key,
            "event": ev.event,
            "verb": verb,
            "path": ev.path,
            "previous_path": ev.previous_path,
            "at": ev.at,
            "offset_ms": ev.offset_ms,
            "size": ev.size,
            "file_type": ev.file_type,
            "origin_domain": origin_domain,
            "details_only": details_only,
            "kind": ev.kind,
            "landed_file": ev.landed_file,
        }));
    }

    let note_md = fs::read_to_string(seg_dir.join("note_transcript.md")).ok();
    let doc_md =
        fs::read_to_string(seg_dir.join(solstone_core_format::folder::DOCUMENT_TRANSCRIPT_FILE))
            .ok();
    let tr_lines: Vec<Value> =
        fs::read_to_string(seg_dir.join(solstone_core_format::folder::FOLDER_TRANSCRIPT_FILE))
            .ok()
            .map(|raw_tr| {
                raw_tr
                    .lines()
                    .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                    .filter(|v| v.get("text").and_then(Value::as_str).is_some())
                    .map(|v| {
                        json!({
                            "speaker": v.get("speaker").and_then(Value::as_str),
                            "text": v.get("text").and_then(Value::as_str).unwrap_or(""),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

    let mut inline_cards = Vec::new();
    let mut emitted_notes = false;
    let mut emitted_docs = false;
    let mut emitted_transcripts = false;
    for ev in &activity.events {
        let Some(landed) = ev.landed_file.as_deref() else {
            continue;
        };
        let kind = ev.kind.as_deref().unwrap_or("");
        match kind {
            "notes" => {
                if !emitted_notes && let Some(md) = &note_md {
                    emitted_notes = true;
                    inline_cards.push(json!({
                        "kind": "notes",
                        "path": ev.path,
                        "at": ev.at,
                        "edited": ev.event == solstone_core_format::folder::FolderEventKind::Changed,
                        "markdown": md,
                    }));
                }
            }
            "documents" => {
                if !emitted_docs && let Some(md) = &doc_md {
                    emitted_docs = true;
                    inline_cards.push(json!({
                        "kind": "documents",
                        "path": ev.path,
                        "file_type": ev.file_type,
                        "at": ev.at,
                        "is_revision": ev.event == solstone_core_format::folder::FolderEventKind::Changed,
                        "markdown": md,
                    }));
                }
            }
            "transcripts" => {
                if !emitted_transcripts && !tr_lines.is_empty() {
                    emitted_transcripts = true;
                    inline_cards.push(json!({
                        "kind": "transcripts",
                        "path": ev.path,
                        "at": ev.at,
                        "is_revision": ev.event == solstone_core_format::folder::FolderEventKind::Changed,
                        "lines": tr_lines,
                    }));
                }
            }
            "screenshots" | "photos" => {
                inline_cards.push(json!({
                    "kind": kind,
                    "path": ev.path,
                    "at": ev.at,
                    "file": landed,
                }));
            }
            _ => {}
        }
    }

    let kinds: Vec<String> = ["notes", "documents", "transcripts", "screenshots", "photos"]
        .into_iter()
        .filter(|k| kind_set.contains(*k))
        .map(str::to_owned)
        .collect();

    Some(json!({
        "folder_id": activity.folder_id,
        "folder_name": activity.folder_name,
        "activity_only": activity_only,
        "facet": facet,
        "file_count": file_count,
        "content_count": content_count,
        "details_only_count": details_only_count,
        "kinds": kinds,
        "events": events_json,
        "inline_cards": inline_cards,
    }))
}

/// Preserve raw diagnostics from legacy or unrelated writers, including matching English.
fn depict_detection_error(
    locale: &str,
    record: Option<&Value>,
    row: &Map<String, Value>,
    error: &Value,
) -> Value {
    let mut displayed = error.clone();
    if locale != ENGLISH
        && record
            .and_then(|r| r.get("handler"))
            .and_then(Value::as_str)
            == Some(vocab::HANDLER_DEPICT)
        && error.get("reason_code").and_then(Value::as_str) == Some("rfdetr-unavailable")
        && let Some(
            id @ ("journal.cli.depict.detection.unavailable"
            | "journal.cli.depict.detection.unavailable.windows"),
        ) = row.get("detection_error_copy").and_then(Value::as_str)
        && let Some(detail) = error.get("detail").and_then(Value::as_str)
        && detail == solstone_core_cli_copy::render_for(ENGLISH, id, &[])
    {
        displayed["detail"] = Value::from(solstone_core_cli_copy::render_for(locale, id, &[]));
    }
    displayed
}

const GRANOLA_STREAM_PREFIX: &str = solstone_core_format::content::granola::STREAM_PREFIX;

/// What Granola wrote about a meeting, apart from its speech: the title, its
/// summary, the owner's own notes and who attended. None of it was spoken.
fn granola_context(dir: &Path) -> Option<Value> {
    let file = fs::File::open(dir.join("audio.jsonl")).ok()?;
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(file), &mut line).ok()?;
    let header: Value = serde_json::from_str(&line).ok()?;
    let note = header.get(solstone_core_format::content::granola::HEADER_KEY)?;
    let text = |keys: &[&str]| {
        keys.iter()
            .filter_map(|key| note.get(*key).and_then(Value::as_str))
            .map(str::trim)
            .find(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let attendees: Vec<String> = note
        .get("attendees")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|person| {
            ["name", "email"]
                .iter()
                .filter_map(|key| person.get(*key).and_then(Value::as_str))
                .map(str::trim)
                .find(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .collect();
    // Only a link back to Granola's own notes page is offered.
    let web_url = text(&["web_url"]).filter(|url| url.starts_with("https://notes.granola.ai/"));
    Some(json!({
        "title": text(&["title"]),
        "summary": text(&["summary_markdown", "summary_text"]),
        "private_notes": text(&["private_notes_markdown", "private_notes_text"]),
        "attendees": attendees,
        "calendar_title": note
            .get("calendar_event")
            .and_then(|event| event.get("event_title"))
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty()),
        "web_url": web_url,
    }))
}

/// An imported photo (`import.image`) carries its original image file,
/// optional `image_meta.json` provenance (capture time source, zone, owner
/// caption), and optional `image_transcript.md` / `screenshot_transcript.md`
/// model description. Emit one structured `image` chunk so the dashboard renders
/// the actual picture with clear owner-vs-model provenance instead of dumping
/// raw markdown metadata.
fn imported_image_entry(
    chunks: &mut Vec<Value>,
    media: &mut SegmentMedia,
    context: &SegmentContext<'_>,
    files: &[std::path::PathBuf],
) -> bool {
    let is_photo_stream = context.stream == "import.image"
        || context.stream == "photos"
        || solstone_core_segment::is_photos_stream(context.root, context.stream);
    if !is_photo_stream {
        return false;
    }
    let Some(raw) = files.iter().find(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_imported_image_original)
    }) else {
        return false;
    };
    let Some(raw_name) = raw.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if !media.register_imported_image(
        context.day,
        context.stream,
        context.key,
        context.dir,
        raw_name,
    ) {
        return false;
    }
    let dimensions = oriented_image_dimensions(raw);
    let is_screenshot = raw_name.starts_with("original.screenshot.");
    let observation = (!is_screenshot)
        .then(|| solstone_core_segment::photo_observation::read_photo_observation(context.dir))
        .flatten();
    let meta = read_image_meta(context.root, context.dir);

    let transcript_path = [
        context.dir.join("image_transcript.md"),
        context.dir.join("screenshot_transcript.md"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file());
    let parsed_transcript = transcript_path.as_deref().and_then(|path| {
        let text = fs::read_to_string(path).ok()?;
        Some(parse_imported_image_transcript(&text, context.root, path))
    });

    let caption = observation
        .as_ref()
        .and_then(|obs| obs.owner_caption.as_ref().map(|c| c.text.clone()))
        .or_else(|| {
            meta.as_ref()
                .and_then(|value| non_empty_str(value.get("caption")))
        })
        .or_else(|| {
            parsed_transcript
                .as_ref()
                .and_then(|parsed| parsed.caption.clone())
        });
    let time_source = observation
        .as_ref()
        .map(|obs| obs.time.time_source.clone())
        .or_else(|| {
            meta.as_ref()
                .and_then(|value| non_empty_str(value.get("time_source")))
        });
    let original_filename = observation
        .as_ref()
        .and_then(|obs| obs.source.original_filename.clone())
        .or_else(|| {
            meta.as_ref()
                .and_then(|value| non_empty_str(value.get("original_filename")))
        });
    let visible_text = observation
        .as_ref()
        .and_then(|obs| obs.visible_text.as_ref().map(|t| t.text.clone()));
    let belonging_class = observation
        .as_ref()
        .and_then(|obs| obs.belonging_class.map(|b| b.as_str()));
    let faces: Vec<Value> = observation
        .as_ref()
        .map(|obs| {
            obs.faces
                .iter()
                .map(|f| {
                    json!({
                        "entity_id": f.entity_id,
                        "face_idx": f.face_idx,
                        "similarity": f.similarity,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let (description, description_state) = if let Some(obs) = observation.as_ref() {
        (
            obs.description.as_ref().map(|d| d.text.clone()),
            obs.state.dashboard_state(),
        )
    } else {
        match parsed_transcript {
            Some(parsed) => (parsed.description, parsed.description_state),
            None => (
                None,
                if processing_set_up(context.root) {
                    "unavailable"
                } else {
                    "no_processing"
                },
            ),
        }
    };
    let is_library_photo = context.stream == "photos"
        || solstone_core_segment::is_photos_stream(context.root, context.stream);
    let device_name = is_library_photo
        .then(|| {
            meta.as_ref()
                .and_then(|value| non_empty_str(value.get("device_name")))
        })
        .flatten();
    let legacy_markdown = match (&caption, &description) {
        (Some(cap), Some(desc)) => format!("{cap}\n\n{desc}"),
        (Some(cap), None) => cap.clone(),
        (None, Some(desc)) => desc.clone(),
        (None, None) => String::new(),
    };
    let time = wall_time(context.key, 0.0);
    chunks.push(json!({
        "type": "image",
        "time": time,
        "timestamp": day_timestamp(&context.zone, context.day, &time, 0),
        "markdown": legacy_markdown,
        "source_ref": {
            "raw": raw_name,
            "media_kind": "image",
        },
        "photo": {
            "width": dimensions.map(|(width, _)| width),
            "height": dimensions.map(|(_, height)| height),
            "caption": caption,
            "description": description,
            "visible_text": visible_text,
            "belonging_class": belonging_class,
            "description_state": description_state,
            "time_source": time_source,
            "original_filename": original_filename,
            "is_screenshot": is_screenshot,
            "source_kind": if is_library_photo { "photos" } else { "import" },
            "device_name": device_name,
            "library_copy": is_library_photo,
            "faces": faces,
        },
    }));
    true
}

fn oriented_image_dimensions(path: &Path) -> Option<(u32, u32)> {
    use image::ImageDecoder;
    use image::metadata::Orientation;

    let reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let (width, height) = decoder.dimensions();
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    match orientation {
        Orientation::Rotate90
        | Orientation::Rotate270
        | Orientation::Rotate90FlipH
        | Orientation::Rotate270FlipH => Some((height, width)),
        _ => Some((width, height)),
    }
}

fn read_image_meta(journal_root: &Path, dir: &Path) -> Option<Value> {
    const MAX_IMAGE_META_BYTES: usize = 64 * 1024;
    let absolute_root = std::path::absolute(journal_root).ok()?;
    let root = solstone_core_journal_io::JournalRoot::open(&absolute_root).ok()?;
    let relative = dir
        .strip_prefix(journal_root)
        .ok()?
        .join(solstone_core_format::content::IMAGE_META_FILENAME);
    let observed = solstone_core_journal_io::read_relative_file_bounded(
        &root,
        &relative,
        MAX_IMAGE_META_BYTES,
    )
    .ok()??;
    serde_json::from_slice(&observed.bytes).ok()
}

fn non_empty_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

struct ParsedImageTranscript {
    caption: Option<String>,
    description: Option<String>,
    description_state: &'static str,
}

fn parse_imported_image_transcript(
    text: &str,
    journal_root: &Path,
    transcript_path: &Path,
) -> ParsedImageTranscript {
    let mut caption_lines = Vec::new();
    let mut in_caption = false;
    let mut model_lines = Vec::new();
    let mut in_model = false;

    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim() == "**Caption:**" {
            in_caption = true;
            in_model = false;
            continue;
        }
        if line.starts_with('>') {
            in_caption = false;
            let stripped = line
                .strip_prefix("> ")
                .or_else(|| line.strip_prefix('>'))
                .unwrap_or("");
            if !in_model && stripped.trim().starts_with("[image description") {
                in_model = true;
                continue;
            }
            if in_model {
                model_lines.push(stripped);
            }
            continue;
        }
        if in_caption {
            if line.trim() == "---" {
                in_caption = false;
            } else {
                caption_lines.push(line);
            }
        }
    }

    let caption = {
        let joined = caption_lines.join("\n").trim().to_owned();
        (!joined.is_empty()).then_some(joined)
    };
    let model_text = model_lines.join("\n").trim().to_owned();
    let is_unavailable =
        match solstone_core_format::stored_markdown::imported_image_description_is_unavailable(
            journal_root,
            transcript_path,
            text,
        ) {
            Some(unavailable) => unavailable || model_text.is_empty(),
            None => {
                model_text.is_empty()
                    || model_text.starts_with("unavailable —")
                    || model_text.starts_with("unavailable -")
                    || model_text == "unavailable"
            }
        };
    let (description, description_state) = if is_unavailable {
        (
            None,
            if processing_set_up(journal_root) {
                "unavailable"
            } else {
                "no_processing"
            },
        )
    } else {
        (Some(model_text), "described")
    };

    ParsedImageTranscript {
        caption,
        description,
        description_state,
    }
}

/// A screenshot is one entry at its second: the image at its own shape, its
/// text once searchable, and until then where its text stands. The `text`
/// word is `searchable`, `waiting` (on its way), `unreadable` (its text could
/// not be made searchable) or `no_processing` (nothing is set up to do it).
fn screenshot_entry(
    chunks: &mut Vec<Value>,
    media: &mut SegmentMedia,
    context: &SegmentContext<'_>,
    files: &[std::path::PathBuf],
    data_state: &BTreeMap<String, String>,
) {
    let Some(raw) = files.iter().find(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(solstone_core_format::screenshots::is_screen_still_file_name)
    }) else {
        return;
    };
    let Some(raw_name) = raw.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let dimensions = image::ImageReader::open(raw)
        .and_then(|reader| reader.with_guessed_format())
        .ok()
        .and_then(|reader| reader.into_dimensions().ok());
    let searchable = chunks.iter().any(|chunk| {
        chunk.get("type").and_then(Value::as_str) == Some("screen")
            && chunk
                .get("markdown")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty())
    });
    let state = data_state.get("screen").map(String::as_str);
    let text = if searchable {
        "searchable"
    } else if matches!(state, Some("failed_final" | "empty")) {
        "unreadable"
    } else if !processing_set_up(context.root) {
        "no_processing"
    } else {
        "waiting"
    };
    let screenshot = json!({
        "width": dimensions.map(|(width, _)| width),
        "height": dimensions.map(|(_, height)| height),
        "text": text,
    });
    let mut annotated = false;
    for chunk in chunks.iter_mut() {
        if chunk.get("type").and_then(Value::as_str) == Some("screen") {
            chunk["screenshot"] = screenshot.clone();
            annotated = true;
        }
    }
    if annotated {
        return;
    }
    let kind = media.register_screen(
        context.day,
        context.stream,
        context.key,
        context.dir,
        raw_name,
        "screenshot_screen.jsonl",
    );
    let time = wall_time(context.key, 0.0);
    chunks.push(json!({
        "type": "screen",
        "time": time,
        "timestamp": day_timestamp(&context.zone, context.day, &time, 0),
        "markdown": "",
        "source_ref": {
            "frame_id": 1,
            "filename": "screenshot_screen.jsonl",
            "raw": raw_name,
            "media_kind": kind,
            "monitor": "screenshot",
            "offset": 0.0,
        },
        "basic": false,
        "screenshot": screenshot,
    }));
}

/// Whether the owner has chosen a way to process what comes in.
fn processing_set_up(root: &Path) -> bool {
    solstone_core_journal_config::read_journal_config(root)
        .ok()
        .and_then(|read| read.config)
        .and_then(|config| {
            config
                .get("providers")?
                .get("active")?
                .get("provider")?
                .as_str()
                .map(|provider| !provider.trim().is_empty())
        })
        .unwrap_or(false)
}

fn extract_modality_reason_codes(
    records: &BTreeMap<String, Option<Value>>,
) -> Option<Map<String, Value>> {
    let mut map = Map::new();
    for (modality, record) in records {
        if let Some(record) = record
            && record.get("state").and_then(Value::as_str) == Some(vocab::STATE_FAILED)
            && let Some(reason_code) = record.get("reason_code").and_then(Value::as_str)
        {
            map.insert(modality.clone(), Value::String(reason_code.to_owned()));
        }
    }
    if map.is_empty() { None } else { Some(map) }
}

/// Why each recording or screen ended without a transcript, for the states an
/// owner sees as settled: `empty` (`no_decodable_audio`, `no_speech`, …),
/// `failed` and `failed_final` (`corrupt_input`, `no_audio_stream`, …). Keyed
/// by modality; a state the record gives no reason for is left out.
pub(crate) fn processing_reason_codes(
    records: &BTreeMap<String, Option<Value>>,
    data_state: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    records
        .iter()
        .filter(|(modality, _)| {
            data_state
                .get(*modality)
                .is_some_and(|state| reason_code_explains(state))
        })
        .filter_map(|(modality, record)| {
            Some((modality.clone(), record_reason_code(record.as_ref()?)?))
        })
        .collect()
}

/// Whether a modality's `data_state` is one whose processing record names why.
pub(crate) fn reason_code_explains(state: &str) -> bool {
    matches!(state, "empty" | "failed" | "failed_final")
}

/// A processing record's reason, unless it is the success code.
pub(crate) fn record_reason_code(record: &Value) -> Option<String> {
    record
        .get("reason_code")
        .and_then(Value::as_str)
        .filter(|code| !code.is_empty() && *code != vocab::REASON_OK)
        .map(str::to_owned)
}

fn imported_conversation_turns<'a>(
    context: &'a SegmentContext<'_>,
    file: &'a str,
    written: bool,
    entries: &'a [Map<String, Value>],
) -> impl Iterator<Item = Value> + 'a {
    let turns = entries.iter().filter(|row| row.contains_key("start"));
    let untimed = entries
        .first()
        .and_then(|header| header.get("untimed"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            // Older generic imports could file each untimed line at the same
            // offset. A source instant, a timed-speaker layout, or distinct
            // recorded offsets establishes timing independently of any clock value.
            let first_offset = turns.clone().next().and_then(|row| row.get("start"));
            context.stream == "import.text"
                && turns.clone().all(|row| {
                    row.get("speaker").and_then(Value::as_str).is_none()
                        && row.get("source_ms").and_then(Value::as_i64).is_none()
                        && !row.contains_key("sent_at")
                        && row.get("start") == first_offset
                })
        });
    let segment_start = Timestamp::from_millisecond(day_timestamp(
        &context.zone,
        context.day,
        &wall_time(context.key, 0.0),
        i64::MIN,
    ))
    .ok();

    turns.enumerate().filter_map(move |(index, row)| {
        let text = row.get("text")?.as_str().filter(|text| !text.is_empty())?;
        let speaker = row.get("speaker").and_then(Value::as_str);
        use solstone_core_format::content::conversation::{TurnTime, turn_time};
        let timestamp = match turn_time(Some(context.stream), row) {
            _ if untimed || row.contains_key("cue_start") => None,
            TurnTime::Unknown => None,
            TurnTime::Sent(instant) => Some(instant),
            TurnTime::Recorded => row
                .get("source_ms")
                .and_then(Value::as_i64)
                .and_then(|millis| Timestamp::from_millisecond(millis).ok())
                .or_else(|| {
                    let elapsed = row
                        .get("start")?
                        .as_str()?
                        .parse::<jiff::civil::Time>()
                        .ok()?;
                    segment_start?
                        .checked_add(elapsed.duration_since(jiff::civil::Time::MIN))
                        .ok()
                }),
        };
        let time = row
            .get("cue_start")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                timestamp.map(|instant| {
                    instant
                        .to_zoned_datetime(context.zone.clone())
                        .strftime("%H:%M:%S")
                        .to_string()
                })
            });
        let mut chunk = json!({
            "type": "transcript",
            "time": time,
            "timestamp": timestamp.map(|instant| instant.as_millisecond()),
            "speaker": speaker,
            "text": text,
        });
        if written {
            use solstone_core_format::content::conversation::{
                message_blocks_for_row, message_fields,
            };
            let mut message = message_fields(context.stream, file, index, row, untimed)?;
            message["blocks"] = json!(message_blocks_for_row(text, row));
            chunk["message"] = message;
        }
        Some(chunk)
    })
}

fn audio_chunks(
    chunks: &mut Vec<Value>,
    media: &mut SegmentMedia,
    speakers: &mut crate::segment_speakers::SpeakerJoin,
    context: &SegmentContext<'_>,
    name: &str,
    entries: &[Map<String, Value>],
    placement: Option<&AudioArchivePlacement>,
) {
    let source = name.trim_end_matches(".jsonl");
    let ids = match embedding_ids(context.dir, source) {
        Ok(ids) => Some(ids),
        Err(error) => {
            speakers.warnings.push(warning(
                "speaker_labels",
                &context.dir.join(format!("{source}.npz")),
                error.to_string(),
                context.now,
            ));
            None
        }
    };
    let voices = speakers.voices(
        context.root,
        context.day,
        context.stream,
        context.key,
        source,
    );
    let text = entries
        .iter()
        .map(|entry| serde_json::to_string(entry).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let ordinals = statement_ordinals(&text);
    let rel = format!(
        "{}/{name}",
        crate::segment_media::segment_rel(context.day, context.stream, context.key)
    );
    let produced = produce_raw_percept_chunks(RawPerceptFamily::Audio, &rel, &text);
    let header_raw = entries
        .iter()
        .find(|entry| !entry.contains_key("start") && entry.contains_key("raw"))
        .and_then(|entry| entry.get("raw"))
        .and_then(Value::as_str);
    let mut placement_media = None;
    if let Some(raw) = header_raw {
        media.register_audio(context.day, context.stream, context.key, context.dir, raw);
        let retained = if context.dir.join(raw).is_file() {
            None
        } else {
            solstone_core_import::audio_originals::retained_playback(context.root, context.dir, raw)
        };
        if retained.is_some() {
            media.has_raw_file.insert("audio".into(), true);
            media.has_raw_present.insert("audio".into(), true);
            media.audio_file = Some(crate::segment_media::audio_url(
                context.day,
                context.stream,
                context.key,
                raw,
            ));
        }
        let part = placement
            .and_then(|placement| placement.parts.iter().find(|part| part.stored_name == raw));
        if (retained.is_some() || part.is_some())
            && (retained.is_some() || context.dir.join(raw).is_file())
        {
            let start = segment_parse(context.key)
                .map(|start| {
                    day_timestamp(
                        &context.zone,
                        context.day,
                        &format!("{:02}:{:02}:{:02}", start.hour, start.minute, start.second),
                        0,
                    )
                })
                .unwrap_or(0);
            let origin_ms = start + part.map_or(0, |part| part.offset_ms)
                - retained.as_ref().map_or(0, |(_, offset_ms)| *offset_ms);
            placement_media = Some((
                crate::segment_media::audio_url(context.day, context.stream, context.key, raw),
                origin_ms,
            ));
        }
    }
    for (index, row) in entries.iter().enumerate() {
        let Some(start) = row.get("start").and_then(Value::as_str) else {
            continue;
        };
        let sid = ordinals.get(index).copied().flatten();
        let markdown = produced
            .chunks
            .iter()
            .find(|chunk| chunk.source.as_ref() == Some(row))
            .map(|chunk| chunk.content.clone())
            .unwrap_or_else(|| {
                row.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            });
        let mut markdown = strip_speaker_prefix(&markdown, row.get("speaker"));
        let possible_names =
            possible_name_marks(&markdown, row, name, sid, &context.possible_names);
        if row.get("turn_boundary_suspect").and_then(Value::as_bool) == Some(true) {
            markdown = format!("{markdown} [{POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE}]");
        }
        let has_embedding = ids
            .as_ref()
            .is_some_and(|ids| sid.is_some_and(|id| ids.contains(&id)));
        let labelled = speakers.state.source.as_deref() == Some(source);
        // The owner's own answer for this line, if one applies.
        let answer = sid
            .and_then(|id| speakers.owner_set.get(&id))
            .filter(|_| labelled);
        let actionable = speakers.state.present
            && speakers.state.loaded
            && labelled
            && has_embedding
            && answer.is_none()
            && !speakers.owner_unreadable;
        let mut chunk = json!({"type":"audio","time":start,"timestamp":produced.chunks.iter().find(|chunk| chunk.source.as_ref() == Some(row)).and_then(|chunk| chunk.occurrence_time_ms).map_or(0, |value| local_wall_instant(&context.zone, value.0)),"markdown":markdown,"sentence_id":sid,"speaker_source":source,"has_embedding":has_embedding,"speaker_actionable":actionable,"source_ref":{"start":start,"source":row.get("source"),"speaker":row.get("speaker")}});
        if let Some((ref audio_url, origin_ms)) = placement_media
            && let Some(obj) = chunk.as_object_mut()
        {
            obj.insert("audio_url".into(), json!(audio_url));
            obj.insert("audio_file".into(), json!(audio_url));
            obj.insert("audio_origin".into(), json!(origin_ms));
        }
        if !possible_names.is_empty()
            && let Some(obj) = chunk.as_object_mut()
        {
            obj.insert("possible_names".into(), Value::Array(possible_names));
        }
        // Granola names who spoke; the page shows its label as Granola's.
        if context.stream.starts_with(GRANOLA_STREAM_PREFIX)
            && let Some(label) = row.get("speaker").and_then(Value::as_str)
            && let Some(obj) = chunk.as_object_mut()
        {
            obj.insert("speaker_attribution".into(), json!(label));
        }
        if let Some(label) = sid
            .and_then(|id| speakers.labels.get(&id))
            .filter(|_| labelled)
        {
            chunk
                .as_object_mut()
                .unwrap()
                .insert("speaker_label".into(), serde_json::to_value(label).unwrap());
        } else if let Some(voice) = sid
            .and_then(|id| voices.get(&id))
            // A line the owner answered for, or a segment whose answers
            // can't be read, never takes a pool voice's name.
            .filter(|_| answer.is_none() && !speakers.owner_unreadable)
        {
            chunk
                .as_object_mut()
                .unwrap()
                .insert("speaker_voice".into(), serde_json::to_value(voice).unwrap());
        }
        if let Some(answer) = answer {
            chunk.as_object_mut().unwrap().insert(
                "owner_line_label".into(),
                json!({"speaker": answer.speaker, "set_at": answer.set_at}),
            );
        }
        chunks.push(chunk);
    }
}

fn screen_chunks(
    chunks: &mut Vec<Value>,
    media: &mut SegmentMedia,
    context: &SegmentContext<'_>,
    name: &str,
    entries: &[Map<String, Value>],
    locale: &str,
) {
    let raw = entries
        .iter()
        .find(|entry| !entry.contains_key("frame_id") && entry.contains_key("raw"))
        .and_then(|entry| entry.get("raw"))
        .and_then(Value::as_str);
    if let Some(raw) = raw {
        media.register_screen(
            context.day,
            context.stream,
            context.key,
            context.dir,
            raw,
            name,
        );
    }
    let describe = locale != ENGLISH
        && processing_record(entries)
            .as_ref()
            .and_then(|record| record["handler"].as_str())
            == Some(vocab::HANDLER_DESCRIBE);
    let mut enriched = entries.to_vec();
    for entry in &mut enriched {
        if !entry.contains_key("timestamp") {
            continue;
        }
        if describe {
            solstone_core_format::stored_markdown::project_screen_extractions(entry, locale);
        }
        let frame_raw = entry
            .get("raw")
            .and_then(Value::as_str)
            .or(raw)
            .map(str::to_owned);
        if let Some(frame_raw) = frame_raw
            && media.register_screen(
                context.day,
                context.stream,
                context.key,
                context.dir,
                &frame_raw,
                name,
            ) == Some("image")
        {
            entry.insert("raw".into(), Value::String(frame_raw.clone()));
            // A screenshot's entry shows its image itself, so its text carries
            // no file note under it.
            if solstone_core_format::screenshots::is_screen_still_file_name(&frame_raw) {
                continue;
            }
            let mut content = entry
                .get_mut("content")
                .and_then(Value::as_object_mut)
                .cloned()
                .unwrap_or_default();
            content
                .entry("media")
                .or_insert_with(|| json!({"photo_file":frame_raw}));
            entry.insert("content".into(), Value::Object(content));
        }
    }
    let text = enriched
        .iter()
        .map(|entry| serde_json::to_string(entry).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let rel = format!(
        "{}/{name}",
        crate::segment_media::segment_rel(context.day, context.stream, context.key)
    );
    let produced =
        produce_raw_percept_chunks_for_locale(RawPerceptFamily::RawScreen, &rel, &text, locale);
    let monitor = if name == "screen.jsonl" {
        ""
    } else {
        name.trim_end_matches("_screen.jsonl")
    };
    for chunk in produced.chunks {
        let source = chunk.source.unwrap_or_default();
        let offset = source
            .get("timestamp")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let source_raw = source.get("raw").and_then(Value::as_str).or(raw);
        let kind = source_raw.and_then(|value| {
            media.register_screen(
                context.day,
                context.stream,
                context.key,
                context.dir,
                value,
                name,
            )
        });
        let time = wall_time(context.key, offset);
        let participants = participants(source.get("content"));
        let mut chunk = json!({"type":"screen","time":time,"timestamp":day_timestamp(&context.zone, context.day, &time, chunk.occurrence_time_ms.map(|value| value.0).unwrap_or(0)),"markdown":chunk.content,"source_ref":{"frame_id":source.get("frame_id"),"filename":name,"raw":source_raw,"media_kind":kind,"monitor":monitor,"offset":source.get("timestamp"),"box_2d":source.get("box_2d"),"analysis":source.get("analysis"),"participants":if participants.is_empty(){Value::Null}else{json!(participants)}},"basic":source.get("analysis").is_none() && source.get("content").is_none_or(|value| value.is_null() || value.as_object().is_some_and(Map::is_empty))});
        if let Some(overlay) = source
            .get("detections")
            .and_then(|d| objects_overlay_for_locale(d, locale))
        {
            chunk["source_ref"]["objects"] = overlay;
        }
        chunks.push(chunk);
    }
}

fn browser_chunks(
    chunks: &mut Vec<Value>,
    zone: &TimeZone,
    name: &str,
    entries: &[Map<String, Value>],
    locale: &str,
) {
    let text = entries
        .iter()
        .map(|entry| serde_json::to_string(entry).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let produced = produce_chunks_for_locale(Family::Browser, name, &text, locale);
    let start = entries
        .iter()
        .find(|entry| entry.get("t").and_then(Value::as_str) == Some("segment_start"));
    let site = start
        .and_then(|entry| entry.get("site"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let title = start
        .and_then(|entry| entry.get("title"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let adapter = start
        .and_then(|entry| entry.get("adapter"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let site_name = if !adapter.trim().is_empty() {
        title_case(adapter)
    } else if !site.trim().is_empty() {
        site.into()
    } else {
        name.trim_start_matches("browser_")
            .trim_end_matches(".jsonl")
            .replace('-', ".")
    };
    for chunk in produced.chunks {
        let source = chunk.source.unwrap_or_default();
        let timestamp = chunk.occurrence_time_ms.map(|value| value.0).unwrap_or(0);
        chunks.push(json!({"type":"browser","time":local_time(zone, timestamp),"timestamp":timestamp,"markdown":chunk.content,"source_ref":{"site":site,"title":title,"adapter":adapter,"site_name":site_name,"file":name,"op":source.get("op").or_else(|| source.get("t"))}}));
    }
}

fn signals(dir: &Path, zone: &TimeZone, locale: &str) -> Value {
    let path = dir.join("signals.jsonl");
    let empty = || json!({"events":[],"counts":{},"calendar":{"total":0,"unique":0,"events":[]}});
    let Ok(ReadEntries { rows: entries, .. }) = read_entries(&path) else {
        return empty();
    };
    let mut events = Vec::new();
    let mut counts = BTreeMap::<String, usize>::new();
    let mut calendar = HashMap::<String, (Value, bool)>::new();
    let untitled = text(
        ENGLISH,
        "journal.window.transcripts.status.text.untitled.event",
    );
    for record in entries {
        let Some(kind) = record
            .get("event_type")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let payload = record
            .get("payload")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let value = record
            .get("timestamp")
            .or_else(|| payload.get("timestamp"))
            .or_else(|| payload.get("timeStamp"));
        let milliseconds = timestamp(value);
        let stamp = value.and_then(Value::as_str).unwrap_or_default();
        events.push(json!({"event_type":kind,"time":local_time(zone, milliseconds),"timestamp":stamp,"timestamp_ms":milliseconds,"payload":payload}));
        *counts.entry(kind.into()).or_default() += 1;
        if kind == "calendar_event" {
            let identity = format!(
                "{:?}|{:?}|{:?}|{:?}",
                payload.get("eventId"),
                payload.get("title"),
                payload.get("dtStart"),
                payload.get("dtEnd")
            );
            let (item, _) = calendar.entry(identity).or_insert_with(|| {
                let title = payload
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let item = json!({
                    "title": title.unwrap_or(&untitled),
                    "dtStart": payload.get("dtStart").and_then(Value::as_str).unwrap_or_default(),
                    "dtEnd": payload.get("dtEnd").and_then(Value::as_str).unwrap_or_default(),
                    "timezone": payload.get("timezone").and_then(Value::as_str).unwrap_or_default(),
                    "eventId": payload.get("eventId").and_then(Value::as_str).unwrap_or_default(),
                    "seen_count": 0,
                    "first_seen": stamp,
                    "last_seen": stamp,
                });
                (item, title.is_none())
            });
            item["seen_count"] = json!(item["seen_count"].as_u64().unwrap_or(0) + 1);
            if !stamp.is_empty() {
                item["last_seen"] = json!(stamp);
            }
        }
    }
    events.sort_by(|left, right| {
        left["timestamp_ms"]
            .as_i64()
            .cmp(&right["timestamp_ms"].as_i64())
            .then(
                left["event_type"]
                    .as_str()
                    .cmp(&right["event_type"].as_str()),
            )
    });
    let mut calendar = calendar.into_values().collect::<Vec<_>>();
    calendar.sort_by(|(left, _), (right, _)| {
        left["dtStart"]
            .as_str()
            .cmp(&right["dtStart"].as_str())
            .then(left["title"].as_str().cmp(&right["title"].as_str()))
    });
    let local_untitled = text(
        locale,
        "journal.window.transcripts.status.text.untitled.event",
    );
    let calendar: Vec<Value> = calendar
        .into_iter()
        .map(|(mut item, lacks_title)| {
            if lacks_title {
                item["title"] = json!(local_untitled);
            }
            item
        })
        .collect();
    json!({"events":events,"counts":counts,"calendar":{"total":counts.get("calendar_event").copied().unwrap_or(0),"unique":calendar.len(),"events":calendar}})
}

/// Every readable row of a JSONL file, and what damage kept other lines out.
///
/// Lines are read from raw bytes one at a time: a write cut short at the end,
/// a line of invalid UTF-8 or malformed JSON never hides the rows around it.
/// Nothing is changed on disk; setting damaged lines aside is the owner's call.
struct ReadEntries {
    rows: Vec<Map<String, Value>>,
    damage: Option<FileDamage>,
}

/// Damaged lines in one segment file. `file` is journal-relative, never a host path.
#[derive(Clone, Serialize)]
pub(crate) struct FileDamage {
    #[serde(rename = "type")]
    kind: String,
    file: String,
    /// How many non-blank valid JSON lines remain readable in the file.
    valid: usize,
    /// How many lines can't be read.
    damaged: usize,
    /// Whether the last line was cut short at the end of the file.
    truncated_tail: bool,
    /// Damaged lines other than a cut-short last line.
    interior: usize,
}

fn read_entries(path: &Path) -> Result<ReadEntries, std::io::Error> {
    let bytes = fs::read(path)?;
    let (valid, damaged) = solstone_core_journal_io::inspect_jsonl_bytes(&bytes);
    let rows = bytes
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line.trim_ascii()).ok())
        .filter_map(|value| match value {
            Value::Object(row) => Some(row),
            _ => None,
        })
        .collect();
    let damage = (!damaged.is_empty()).then(|| {
        let truncated_tail = damaged
            .iter()
            .any(|line| line.kind == solstone_core_journal_io::JsonlDamageKind::TruncatedTail);
        FileDamage {
            kind: String::new(),
            file: String::new(),
            valid,
            damaged: damaged.len(),
            truncated_tail,
            interior: damaged.len() - usize::from(truncated_tail),
        }
    });
    Ok(ReadEntries { rows, damage })
}

/// Read one segment file's rows, noting its damage under `kind`.
fn read_segment_entries(
    root: &Path,
    kind: &str,
    path: &Path,
    damage: &mut Vec<FileDamage>,
) -> Result<Vec<Map<String, Value>>, std::io::Error> {
    let read = read_entries(path)?;
    if let Some(mut found) = read.damage {
        found.kind = kind.to_owned();
        found.file = journal_relative_file(root, path);
        damage.push(found);
    }
    Ok(read.rows)
}

/// A path as the journal names it, `/`-separated from its root. Anything that
/// is not inside the journal is named by its last component alone, so no host
/// path ever reaches a response.
pub(crate) fn journal_relative_file(root: &Path, path: &Path) -> String {
    solstone_core_journal_io::journal_relative_path(root, path).unwrap_or_else(|| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    })
}

/// Name every warning's file the way the journal does: relative to its root.
/// Warning files that are not paths (a source id) are left as they are.
fn relative_warning_files(root: &Path, warnings: &mut [WarningDetail]) {
    for warning in warnings {
        let path = Path::new(&warning.file);
        if path.is_absolute() {
            warning.file = journal_relative_file(root, path);
        }
    }
}
fn processing_record(entries: &[Map<String, Value>]) -> Option<Value> {
    entries
        .iter()
        .find_map(|entry| entry.get("_solstone_processing").cloned())
        .filter(Value::is_object)
}
fn audio_duration(entries: &[Map<String, Value>], key: &str) -> f64 {
    for entry in entries {
        if entry.contains_key("start") {
            continue;
        }
        if let Some(value) = entry
            .get("duration")
            .and_then(Value::as_f64)
            .filter(|value| *value > 0.0)
        {
            return value;
        }
        if let Some(value) = entry
            .get("duration")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| *value > 0.0)
        {
            return value;
        }
    }
    segment_parse(key)
        .and_then(|_| key.split_once('_'))
        .and_then(|(_, length)| length.parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// The transcript page's speaker strings, by key.
const TRANSCRIPTS_COPY: &[(&str, &str)] = &[
    (
        "TR_SPEAKER_CHANGE_LABEL",
        "journal.window.transcripts.speaker.copy.change.label",
    ),
    (
        "TR_SPEAKER_ASSIGN_LABEL",
        "journal.window.transcripts.speaker.copy.assign.label",
    ),
    (
        "TR_SPEAKER_PICKER_TITLE",
        "journal.window.transcripts.speaker.copy.picker.title",
    ),
    (
        "TR_SPEAKER_PICKER_SEARCH_PLACEHOLDER",
        "journal.window.transcripts.speaker.copy.picker.search.placeholder",
    ),
    (
        "TR_SPEAKER_PICKER_OWNER",
        "journal.window.transcripts.speaker.copy.picker.owner",
    ),
    (
        "TR_SPEAKER_OWNER_NAME",
        "journal.window.transcripts.speaker.copy.owner.name",
    ),
    (
        "TR_SPEAKER_PICKER_EMPTY",
        "journal.window.transcripts.speaker.copy.picker.empty",
    ),
    (
        "TR_SPEAKER_SOMEONE_ELSE",
        "journal.window.transcripts.speaker.copy.someone.else",
    ),
    (
        "TR_SPEAKER_PICKER_NO_RESULTS",
        "journal.window.transcripts.speaker.copy.picker.no.results",
    ),
    (
        "TR_SPEAKER_UNKNOWN_CHIP",
        "journal.window.transcripts.speaker.copy.unknown.chip",
    ),
    (
        "TR_SPEAKER_VOICE_CHIP",
        "journal.window.transcripts.speaker.copy.voice.chip",
    ),
    (
        "TR_SPEAKER_VOICE_ASSISTIVE",
        "journal.window.transcripts.speaker.copy.voice.assistive",
    ),
    (
        "TR_SPEAKER_VOICE_NAMED_ASSISTIVE",
        "journal.window.transcripts.speaker.copy.voice.named.assistive",
    ),
    (
        "TR_SPEAKER_VOICE_SCOPE",
        "journal.window.transcripts.speaker.copy.voice.scope",
    ),
    (
        "TR_SPEAKER_VOICE_SCOPE_NOTE",
        "journal.window.transcripts.speaker.copy.voice.scope.note",
    ),
    (
        "TR_SPEAKER_VOICE_NAMED_COUNT",
        "journal.window.transcripts.speaker.copy.voice.named.count",
    ),
    (
        "TR_SPEAKER_VOICE_NAMED_ONE",
        "journal.window.transcripts.speaker.copy.voice.named.one",
    ),
    (
        "TR_SPEAKER_HEDGE_PROBABLE",
        "journal.window.transcripts.speaker.copy.hedge.probable",
    ),
    (
        "TR_SPEAKER_HEDGE_MAYBE",
        "journal.window.transcripts.speaker.copy.hedge.maybe",
    ),
    (
        "TR_SPEAKER_CONFIDENCE_HIGH",
        "journal.window.transcripts.speaker.copy.confidence.high",
    ),
    (
        "TR_SPEAKER_CONFIDENCE_UNKNOWN",
        "journal.window.transcripts.speaker.copy.confidence.unknown",
    ),
    (
        "TR_SPEAKER_MARGIN_OWNER",
        "journal.window.transcripts.speaker.copy.margin.owner",
    ),
    (
        "TR_SPEAKER_MARGIN_ACOUSTIC",
        "journal.window.transcripts.speaker.copy.margin.acoustic",
    ),
    (
        "TR_SPEAKER_ACTION_UNAVAILABLE",
        "journal.window.transcripts.speaker.copy.action.unavailable",
    ),
    (
        "TR_SPEAKER_NO_EMBEDDING",
        "journal.window.transcripts.speaker.copy.no.embedding",
    ),
    (
        "TR_SPEAKER_CORRECT_RETRY",
        "journal.window.transcripts.speaker.copy.correct.retry",
    ),
    (
        "TR_SPEAKER_CORRECT_BUSY",
        "journal.window.transcripts.speaker.copy.correct.busy",
    ),
    (
        "TR_SPEAKER_OWNER_TOO_CLOSE",
        "journal.window.transcripts.speaker.copy.owner.too.close",
    ),
    (
        "TR_SPEAKER_OWNER_IDENTITY_REQUIRED",
        "journal.window.transcripts.speaker.copy.owner.identity.required",
    ),
    (
        "TR_SPEAKER_ALREADY_CORRECT",
        "journal.window.transcripts.speaker.copy.already.correct",
    ),
    (
        "TR_SPEAKER_PROPAGATION_DISMISS",
        "journal.window.transcripts.speaker.copy.propagation.dismiss",
    ),
    (
        "TR_SPEAKER_PICKER_UNKNOWN",
        "journal.window.transcripts.speaker.copy.picker.unknown",
    ),
    (
        "TR_SPEAKER_PICKER_GUESS",
        "journal.window.transcripts.speaker.copy.picker.guess",
    ),
    (
        "TR_SPEAKER_LINE_ONLY",
        "journal.window.transcripts.speaker.copy.line.only",
    ),
    (
        "TR_SPEAKER_LINE_ONLY_BODY",
        "journal.window.transcripts.speaker.copy.line.only.body",
    ),
    (
        "TR_SPEAKER_LINE_ONLY_LINK",
        "journal.window.transcripts.speaker.copy.line.only.link",
    ),
    (
        "TR_SPEAKER_OWNER_SET",
        "journal.window.transcripts.speaker.copy.owner.set",
    ),
    (
        "TR_SPEAKER_OWNER_UNKNOWN_CHIP",
        "journal.window.transcripts.speaker.copy.owner.unknown.chip",
    ),
    (
        "TR_SPEAKER_LINE_REFUSED",
        "journal.window.transcripts.speaker.copy.line.refused",
    ),
    (
        "TR_SPEAKER_LINE_SOURCE",
        "journal.window.transcripts.speaker.copy.line.source",
    ),
    (
        "TR_SPEAKER_LINE_CHANGED",
        "journal.window.transcripts.speaker.copy.line.changed",
    ),
    (
        "TR_SPEAKER_LINE_OWNER",
        "journal.window.transcripts.speaker.copy.line.owner",
    ),
    (
        "TR_SPEAKER_LINE_PERSON",
        "journal.window.transcripts.speaker.copy.line.person",
    ),
    (
        "TR_SPEAKER_LINE_UNREADABLE",
        "journal.window.transcripts.speaker.copy.line.unreadable",
    ),
];

fn copy_payload(locale: &str) -> Value {
    Value::Object(
        TRANSCRIPTS_COPY
            .iter()
            .map(|(key, id)| ((*key).to_owned(), Value::String(text(locale, id))))
            .collect(),
    )
}
fn warning(
    kind: &str,
    path: &Path,
    error: impl std::fmt::Display,
    now: Timestamp,
) -> WarningDetail {
    WarningDetail {
        kind: kind.into(),
        file: path.display().to_string(),
        message: error.to_string(),
        ts: solstone_core_time::utc_z(now, Precision::Secs),
    }
}
fn hms_seconds(hms: &str) -> i64 {
    let mut parts = hms.split(':');
    let h = parts.next().and_then(|p| p.parse::<i64>().ok());
    let m = parts.next().and_then(|p| p.parse::<i64>().ok());
    let s = parts.next().and_then(|p| p.parse::<i64>().ok());
    if let (Some(h), Some(m), Some(s)) = (h, m, s)
        && parts.next().is_none()
        && h >= 0
        && m >= 0
        && s >= 0
    {
        return h * 3600 + m * 60 + s;
    }
    0
}
fn wall_time(key: &str, offset: f64) -> String {
    let Some(start) = segment_parse(key) else {
        return String::new();
    };
    let seconds =
        start.hour as i64 * 3600 + start.minute as i64 * 60 + start.second as i64 + offset as i64;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}
fn day_timestamp(zone: &TimeZone, day: &str, time: &str, fallback: i64) -> i64 {
    jiff::civil::DateTime::strptime("%Y%m%d %H:%M:%S", format!("{day} {time}"))
        .ok()
        .and_then(|value| solstone_core_time::from_local(zone, value).earliest())
        .map(|value| value.timestamp().as_millisecond())
        .unwrap_or(fallback)
}
/// The instant of a formatter occurrence time. The content formatter encodes a
/// segment's local wall time as if it were UTC; screen and image chunks here
/// are anchored in the owner zone, so audio is re-anchored the same way to
/// merge and sort on one clock.
fn local_wall_instant(zone: &TimeZone, wall_ms: i64) -> i64 {
    Timestamp::from_millisecond(wall_ms)
        .ok()
        .and_then(|wall| {
            solstone_core_time::from_local(zone, Offset::UTC.to_datetime(wall)).earliest()
        })
        .map_or(wall_ms, |local| local.timestamp().as_millisecond())
}
fn timestamp(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(number)) => number
            .as_f64()
            .map(|value| {
                if value > 10_000_000_000.0 {
                    value
                } else {
                    value * 1000.0
                }
            })
            .map(|value| value as i64)
            .unwrap_or_default(),
        Some(Value::String(value)) => solstone_core_time::parse_rfc3339(value)
            .map(|value| value.timestamp().as_millisecond())
            .unwrap_or(0),
        _ => 0,
    }
}
fn local_time(zone: &TimeZone, milliseconds: i64) -> String {
    if milliseconds <= 0 {
        return String::new();
    }
    Timestamp::from_millisecond(milliseconds)
        .ok()
        .map(|value| {
            value
                .to_zoned_datetime(zone.clone())
                .strftime("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}
fn participants(content: Option<&Value>) -> Vec<Value> {
    content.and_then(|value| value.get("meeting")).and_then(Value::as_object).and_then(|meeting| meeting.get("participants")).and_then(Value::as_array).into_iter().flatten().filter_map(|participant| { let box_2d = participant.get("box_2d").and_then(Value::as_array)?; (participant.get("video").and_then(Value::as_bool) == Some(true) && box_2d.len() == 4).then(|| json!({"name":participant.get("name").and_then(Value::as_str).unwrap_or("Unknown"),"status":participant.get("status").and_then(Value::as_str).unwrap_or("unknown"),"top":box_2d[0].as_f64().unwrap_or(0.0)/10.0,"left":box_2d[1].as_f64().unwrap_or(0.0)/10.0,"height":(box_2d[2].as_f64().unwrap_or(0.0)-box_2d[0].as_f64().unwrap_or(0.0))/10.0,"width":(box_2d[3].as_f64().unwrap_or(0.0)-box_2d[1].as_f64().unwrap_or(0.0))/10.0})) }).collect()
}
/// Where each possible name for this statement sits in its shown text: right
/// after the words it labels, as a UTF-16 offset the page slices at. The text
/// itself is never changed; the page shows the name beside it, as tentative.
fn possible_name_marks(
    markdown: &str,
    row: &Map<String, Value>,
    file: &str,
    sentence: Option<i64>,
    entries: &[solstone_core_possible_names::LiveEntry],
) -> Vec<Value> {
    let Some(sentence) = sentence.and_then(|sentence| u64::try_from(sentence).ok()) else {
        return Vec::new();
    };
    let mut labels = entries
        .iter()
        .filter(|entry| entry.file == file && entry.sentence == sentence)
        .filter(|entry| !solstone_core_possible_names::repeats_heard(entry))
        .collect::<Vec<_>>();
    if labels.is_empty() {
        return Vec::new();
    }
    let Some((_, words)) = solstone_core_transcripts::statement_text(row) else {
        return Vec::new();
    };
    let Some(start) = markdown.strip_suffix(words).map(str::len) else {
        return Vec::new();
    };
    labels.sort_by_key(|entry| entry.end_byte);
    labels
        .into_iter()
        .filter_map(|entry| {
            let at = start + entry.end_byte;
            markdown.is_char_boundary(at).then(|| {
                json!({
                    "at": markdown[..at].encode_utf16().count(),
                    "name": entry.name,
                })
            })
        })
        .collect()
}

fn strip_speaker_prefix(markdown: &str, speaker: Option<&Value>) -> String {
    let markdown = markdown
        .strip_prefix("[")
        .and_then(|value| value.split_once("] ").map(|(_, value)| value))
        .unwrap_or(markdown);
    // A row with a source renders as "(source) Speaker N: text"; keep the source.
    let (source, rest) = match markdown
        .strip_prefix('(')
        .and_then(|value| value.split_once(") "))
    {
        Some((source, rest)) => (Some(source), rest),
        None => (None, markdown),
    };
    let stripped = match speaker {
        Some(Value::Number(number)) => rest.strip_prefix(&format!("Speaker {number}: ")),
        Some(Value::String(speaker)) => rest.strip_prefix(&format!("{speaker}: ")),
        _ => None,
    };
    match (stripped, source) {
        (Some(text), Some(source)) => format!("({source}) {text}"),
        (Some(text), None) => text.into(),
        (None, _) => markdown.into(),
    }
}
fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}
pub(crate) fn valid_stream(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
pub(crate) fn valid_key(value: &str) -> bool {
    let Some((time, length)) = value.split_once('_') else {
        return false;
    };
    time.len() == 6
        && time.bytes().all(|byte| byte.is_ascii_digit())
        && !length.is_empty()
        && length.bytes().all(|byte| byte.is_ascii_digit())
}
fn invalid(locale: &str, detail: String) -> Response {
    legacy_error_response(
        "invalid_segment_or_stream",
        text(
            locale,
            "journal.window.transcripts.response.error.that.segment.or.stream.couldn.t",
        ),
        detail,
        StatusCode::NOT_FOUND,
    )
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;
    use jiff::tz::{Offset, TimeZone};
    use serde_json::json;
    use solstone_core_processing_record::vocab;

    use super::{
        day_timestamp, local_time, local_wall_instant, prepare_segment, strip_speaker_prefix,
        timestamp,
    };

    #[test]
    fn timestamps_accept_floats_and_non_positive_times_are_blank() {
        assert_eq!(timestamp(Some(&json!(1.5))), 1500);
        assert_eq!(timestamp(Some(&json!(10_000_000_001.5))), 10_000_000_001);
        assert_eq!(local_time(&TimeZone::UTC, 0), "");
        assert_eq!(local_time(&TimeZone::UTC, -1), "");
    }

    #[test]
    fn audio_wall_times_land_on_the_same_clock_as_screen_times() {
        let wall = Offset::UTC
            .to_timestamp(jiff::civil::date(2026, 7, 31).at(9, 0, 5, 0))
            .unwrap()
            .as_millisecond();
        let denver = &solstone_core_time::zone("America/Denver").unwrap();
        let screen = day_timestamp(denver, "20260731", "09:00:05", 0);
        // 09:00:05 in Denver (UTC-6 in July) is 15:00:05 UTC.
        assert_eq!(screen, wall + 6 * 3_600_000);
        assert_eq!(local_wall_instant(denver, wall), screen);
    }

    #[test]
    fn day_timestamps_use_the_owner_zone_at_that_days_offset() {
        let denver = &solstone_core_time::zone("America/Denver").unwrap();
        for (day, hours) in [("20260731", 6), ("20260115", 7)] {
            let wall = solstone_core_time::parse_day(day).unwrap().at(9, 0, 0, 0);
            assert_eq!(
                day_timestamp(denver, day, "09:00:00", 0),
                Offset::UTC.to_timestamp(wall).unwrap().as_millisecond() + hours * 3_600_000,
                "{day}"
            );
        }
        let instant = "2026-07-31T15:00:05Z".parse::<Timestamp>().unwrap();
        assert_eq!(local_time(denver, instant.as_millisecond()), "09:00:05");
    }

    #[test]
    fn possible_names_sit_beside_the_words_they_label_while_they_hold() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let entity = root.join("entities/friar");
        std::fs::create_dir_all(&entity).unwrap();
        std::fs::write(
            entity.join("entity.json"),
            json!({"id": "friar", "name": "Friar"}).to_string(),
        )
        .unwrap();
        let link = root.join("facets/work/entities/friar");
        std::fs::create_dir_all(&link).unwrap();
        std::fs::write(
            link.join("entity.json"),
            json!({"entity_id": "friar"}).to_string(),
        )
        .unwrap();
        let seg_dir = root.join("chronicle/20260101/default/120000_60");
        std::fs::create_dir_all(&seg_dir).unwrap();
        std::fs::write(
            seg_dir.join("audio.jsonl"),
            "{\"raw\":\"audio.flac\"}\n\
             {\"start\":\"00:00:01\",\"source\":\"mic\",\"speaker\":1,\"text\":\"café with Fryer\",\"emotion\":\"happy\"}\n\
             {\"start\":\"00:00:02\",\"speaker\":2,\"text\":\"nothing to label\"}\n",
        )
        .unwrap();
        let sidecar = solstone_core_possible_names::Sidecar::new(
            "test-1",
            solstone_core_possible_names::SidecarSource {
                file: "audio.jsonl".to_owned(),
                sha256: solstone_core_possible_names::source_sha256(&seg_dir, "audio.jsonl")
                    .unwrap(),
            },
            "rev",
            1,
            vec![solstone_core_possible_names::SidecarEntry {
                sentence: 1,
                start_byte: 11,
                end_byte: 16,
                heard: "Fryer".to_owned(),
                entity_id: "friar".to_owned(),
                name: "Friar".to_owned(),
            }],
        );
        solstone_core_possible_names::write(&seg_dir, &sidecar).unwrap();
        let audio = || {
            prepare_segment(root, "20260101", "default", "120000_60", Timestamp::now()).unwrap()["chunks"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|chunk| chunk["type"] == "audio")
                .cloned()
                .collect::<Vec<_>>()
        };
        let chunks = audio();
        // The words are what was said; the name sits after "Fryer", counted in
        // UTF-16 units of the shown text. An older version's mood label on the
        // line is never shown.
        assert_eq!(chunks[0]["markdown"], "(mic) café with Fryer");
        let at = "(mic) café with Fryer".encode_utf16().count();
        assert_eq!(
            chunks[0]["possible_names"],
            json!([{"at": at, "name": "Friar"}])
        );
        assert!(chunks[1].get("possible_names").is_none());
        // The file is not a talent output tab: its entries are only shown checked.
        let value =
            prepare_segment(root, "20260101", "default", "120000_60", Timestamp::now()).unwrap();
        assert!(value["md_files"].get("possible_names").is_none());

        // A renamed entity's suggestion is hidden until the talent runs again.
        std::fs::write(
            entity.join("entity.json"),
            json!({"id": "friar", "name": "Brother Friar"}).to_string(),
        )
        .unwrap();
        assert!(audio()[0].get("possible_names").is_none());
    }

    #[test]
    fn prepare_segment_browser_rich_and_legacy_page_context() {
        let expected = "context: background tab · media playing · solstone app paused · tab open at least 3h 5m · language en-US · safari 18.1";

        let temp_rich = tempfile::tempdir().unwrap();
        let root_rich = temp_rich.path();
        let seg_dir_rich = root_rich.join("chronicle/20260101/default/120000_60");
        std::fs::create_dir_all(&seg_dir_rich).unwrap();
        std::fs::write(
            seg_dir_rich.join("browser_events.jsonl"),
            r#"{"t":"segment_start","ts":1,"title":"Inbox","adapter":"gmail","site":"mail.google.com","ext":"EXT-SECRET-99","tab":{"id":"TAB-ID-77","front":false,"focused":true,"age_ms":11100000},"media":{"playing":true},"solstone_paused":true,"lang":"en-US","browser":{"vendor":"safari","version":"18.1"},"blocks":[{"type":"text","text":"Hello"}]}"#,
        )
        .unwrap();

        let value_rich = prepare_segment(
            root_rich,
            "20260101",
            "default",
            "120000_60",
            Timestamp::now(),
        )
        .unwrap();
        let browser_chunks_rich: Vec<&serde_json::Value> = value_rich["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|chunk| chunk["type"] == "browser")
            .collect();
        assert!(!browser_chunks_rich.is_empty());
        assert!(browser_chunks_rich.iter().any(|chunk| {
            chunk["markdown"]
                .as_str()
                .unwrap_or_default()
                .contains(expected)
        }));

        let temp_legacy = tempfile::tempdir().unwrap();
        let root_legacy = temp_legacy.path();
        let seg_dir_legacy = root_legacy.join("chronicle/20260101/default/120000_60");
        std::fs::create_dir_all(&seg_dir_legacy).unwrap();
        std::fs::write(
            seg_dir_legacy.join("browser_events.jsonl"),
            r#"{"t":"segment_start","ts":1,"title":"Inbox","blocks":[{"text":"Hello"}]}"#,
        )
        .unwrap();

        let value_legacy = prepare_segment(
            root_legacy,
            "20260101",
            "default",
            "120000_60",
            Timestamp::now(),
        )
        .unwrap();
        let browser_chunks_legacy: Vec<&serde_json::Value> = value_legacy["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|chunk| chunk["type"] == "browser")
            .collect();
        assert!(!browser_chunks_legacy.is_empty());
        assert!(!browser_chunks_legacy.iter().any(|chunk| {
            chunk["markdown"]
                .as_str()
                .unwrap_or_default()
                .contains("context:")
        }));
    }

    #[test]
    fn declared_dashboard_talent_rows_reach_the_segment() {
        use solstone_core_format::content::{Reader, file_spec};

        let temp = tempfile::tempdir().unwrap();
        let segment = "20260101/default/100000_60";
        for (name, key) in [
            ("summary.md", "summary"),
            ("details/note.md", "details/note"),
        ] {
            let relative = format!("{segment}/talents/{name}");
            let row = file_spec(&relative).unwrap();
            assert!(row.segment.is_none());
            assert!(row.offered_to.contains(Reader::Dashboard));
            let path = temp.path().join("chronicle").join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("projection witness {key}")).unwrap();
        }
        let value = prepare_segment(
            temp.path(),
            "20260101",
            "default",
            "100000_60",
            Timestamp::now(),
        )
        .unwrap();
        for key in ["summary", "details/note"] {
            assert_eq!(value["md_files"][key], format!("projection witness {key}"));
        }
    }

    #[test]
    fn md_files_includes_screen_text_and_neighbor_stem() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let seg_dir = root.join("chronicle/20260101/default/100000_60");
        let talents_dir = seg_dir.join("talents");
        std::fs::create_dir_all(&talents_dir).unwrap();
        std::fs::write(
            talents_dir.join("screen_text.md"),
            "## 00:12 · code\ncontent",
        )
        .unwrap();
        std::fs::write(talents_dir.join("summary.md"), "# Summary\nmeeting notes").unwrap();

        let val =
            prepare_segment(root, "20260101", "default", "100000_60", Timestamp::now()).unwrap();
        let md_files = val["md_files"].as_object().unwrap();
        assert!(md_files.contains_key("screen_text"));
        assert!(md_files.contains_key("summary"));
        // The group heading names its clock time: the segment's start plus 12 s.
        let screen_text = md_files["screen_text"].as_str().unwrap();
        assert!(screen_text.starts_with("## 10:00:12 · "), "{screen_text}");
    }

    #[test]
    fn a_calendar_segment_shows_each_note_as_plain_text() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("streams")).unwrap();
        std::fs::write(
            root.join("streams/phone_calendar.json"),
            r#"{"name":"phone_calendar","kind":"observer","host":null,"platform":null,"created_at":0,"last_day":null,"last_segment":null,"seq":1,"cid":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","source":"calendar"}"#,
        )
        .unwrap();
        let golden = include_str!("../../../fixtures/calendar_notes/golden.jsonl");
        let seg_dir = root.join("chronicle/20261020/phone_calendar/163000_300");
        std::fs::create_dir_all(&seg_dir).unwrap();
        let lines = golden.lines().collect::<Vec<_>>();
        std::fs::write(
            seg_dir.join(format!("calendar_{}.jsonl", "a".repeat(64))),
            format!("{}\n{}\n", lines[1], lines[3]),
        )
        .unwrap();
        let value = prepare_segment(
            root,
            "20261020",
            "phone_calendar",
            "163000_300",
            Timestamp::now(),
        )
        .unwrap();
        let notes = value["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|chunk| chunk["type"] == "calendar")
            .map(|chunk| chunk["text"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(notes.len(), 2);
        for (text, expected) in [
            (
                &notes[0],
                ["standup", "9:30", "Work", "every weekday", "changed"],
            ),
            (
                &notes[1],
                [
                    "Kate Bell's Birthday",
                    "2027",
                    "Birthdays",
                    "every year",
                    "added",
                ],
            ),
        ] {
            for word in expected {
                assert!(text.contains(word), "{word} in {text}");
            }
            assert!(!text.contains("null"), "{text}");
        }
    }

    #[test]
    fn a_granola_meeting_carries_its_notes_apart_from_speech_and_granolas_speaker_labels() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let stream = "import.granola.6e6f745f61626364656667686a6b6c6d6e";
        let seg_dir = root.join(format!("chronicle/20261111/{stream}/123448_164"));
        std::fs::create_dir_all(&seg_dir).unwrap();
        std::fs::write(
            seg_dir.join("audio.jsonl"),
            concat!(
                r#"{"granola":{"id":"not_abcdefghjklmn","title":"Capulet standup","web_url":"https://notes.granola.ai/d/d3dd409e-7aa8-4031-a790-8027d758930e","summary_text":"Beta status","summary_markdown":"","private_notes_text":"","private_notes_markdown":null,"attendees":[{"name":null,"email":"r@example.org"},{"name":"Robin","email":"robin@example.org"}],"calendar_event":null},"granola_start_ms":1794425688939}"#,
                "\n",
                r#"{"start":"12:34:48.939","source_ms":1794425688939,"end_ms":1794425693309,"speaker":"Granola: them","text":"Morning.","source":"granola","granola_speaker":{"source":"speaker","attribution":"them"}}"#,
                "\n"
            ),
        )
        .unwrap();
        let value =
            prepare_segment(root, "20261111", stream, "123448_164", Timestamp::now()).unwrap();
        let note = &value["granola"];
        assert_eq!(note["title"], "Capulet standup");
        // An empty markdown summary falls back to its text, and empty notes are absent.
        assert_eq!(note["summary"], "Beta status");
        assert!(note["private_notes"].is_null());
        assert_eq!(note["attendees"], json!(["r@example.org", "Robin"]));
        assert!(note["calendar_title"].is_null());
        assert!(
            note["web_url"]
                .as_str()
                .unwrap()
                .starts_with("https://notes.granola.ai/")
        );
        let speech = value["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|chunk| chunk["type"] == "audio")
            .collect::<Vec<_>>();
        assert_eq!(speech.len(), 1);
        assert_eq!(speech[0]["speaker_attribution"], "Granola: them");
        assert!(
            !speech[0]["markdown"]
                .as_str()
                .unwrap()
                .contains("Beta status")
        );
    }

    #[test]
    fn a_segment_from_another_source_has_no_granola_notes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let seg_dir = root.join("chronicle/20261111/import.audio/123448_164");
        std::fs::create_dir_all(&seg_dir).unwrap();
        std::fs::write(
            seg_dir.join("audio.jsonl"),
            "{\"granola\":{\"title\":\"not this\"}}\n{\"start\":\"12:34:48\",\"speaker\":1,\"text\":\"Hello.\"}\n",
        )
        .unwrap();
        let value = prepare_segment(
            root,
            "20261111",
            "import.audio",
            "123448_164",
            Timestamp::now(),
        )
        .unwrap();
        assert!(value.get("granola").is_none());
        assert!(value["chunks"][0].get("speaker_attribution").is_none());
    }

    #[test]
    fn a_speaker_number_is_stripped_after_a_source_prefix_too() {
        let speaker = json!(1);
        assert_eq!(strip_speaker_prefix("Speaker 1: hi", Some(&speaker)), "hi");
        assert_eq!(
            strip_speaker_prefix("(mic) Speaker 1: hi", Some(&speaker)),
            "(mic) hi"
        );
        assert_eq!(
            strip_speaker_prefix("[00:05] (mic) Speaker 1: hi", Some(&speaker)),
            "(mic) hi"
        );
        assert_eq!(
            strip_speaker_prefix("(aside) Speaker 2: hi", Some(&speaker)),
            "(aside) Speaker 2: hi"
        );
        assert_eq!(strip_speaker_prefix("(mic) hi", None), "(mic) hi");
    }

    #[test]
    fn unnamed_sentences_carry_their_pool_voice_and_the_owner_voice_stays_unshown() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for entity in [
            json!({"id":"owner","name":"Owner","type":"Person","is_principal":true}),
            json!({"id":"ryan","name":"Ryan","type":"Person"}),
        ] {
            let dir = root.join("entities").join(entity["id"].as_str().unwrap());
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("entity.json"), entity.to_string()).unwrap();
        }
        let segment_dir = root.join("chronicle/20260101/120000_60");
        std::fs::create_dir_all(segment_dir.join("talents")).unwrap();
        std::fs::write(
            segment_dir.join("audio.jsonl"),
            "{\"raw\":\"audio.flac\"}\n\
             {\"start\":\"00:00:01\",\"speaker\":1,\"text\":\"one\",\"sentence_id\":1}\n\
             {\"start\":\"00:00:02\",\"speaker\":2,\"text\":\"two\",\"sentence_id\":2}\n\
             {\"start\":\"00:00:03\",\"speaker\":1,\"text\":\"three\",\"sentence_id\":3}\n\
             {\"start\":\"00:00:04\",\"speaker\":2,\"text\":\"four\",\"sentence_id\":4}\n",
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("talents/speaker_labels.json"),
            json!({"labels":[{"sentence_id":3,"speaker":"ryan","confidence":"high","method":"acoustic"}]})
                .to_string(),
        )
        .unwrap();
        let source = |ids: &[i64]| json!({"day":"20260101","stream_layout":"direct","stream":"_default","segment_key":"120000_60","source":"audio","cluster_label":1,"sentence_ids":ids});
        std::fs::create_dir_all(root.join("awareness")).unwrap();
        std::fs::write(
            root.join("awareness/speaker_candidates.json"),
            json!({"next_id":20,"candidates":[
                {"cand_id":7,"centroid":[1.0],"status":"pending","confirmed_entity":null,"source_segments":[source(&[1,3])]},
                {"cand_id":8,"centroid":[1.0],"status":"confirmed","confirmed_entity":"owner","source_segments":[source(&[2])]},
                {"cand_id":9,"centroid":[1.0],"status":"confirmed","confirmed_entity":"ryan","source_segments":[source(&[4])]}
            ]})
            .to_string(),
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "_default",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();
        let chunks = value["chunks"].as_array().unwrap();
        let by_sentence = |id: i64| {
            chunks
                .iter()
                .find(|chunk| chunk["sentence_id"] == json!(id))
                .unwrap()
                .clone()
        };
        assert_eq!(
            by_sentence(1)["speaker_voice"],
            json!({"voice_id":7,"entity_id":null,"name":null})
        );
        assert!(
            by_sentence(2).get("speaker_voice").is_none(),
            "the owner's own voice is not tagged"
        );
        assert!(
            by_sentence(3).get("speaker_voice").is_none(),
            "a named sentence keeps its label"
        );
        assert_eq!(by_sentence(3)["speaker_label"]["entity_id"], json!("ryan"));
        assert_eq!(
            by_sentence(4)["speaker_voice"],
            json!({"voice_id":9,"entity_id":"ryan","name":"Ryan"})
        );
    }

    #[test]
    fn prepare_segment_emits_reason_code_for_failed_modality() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        let audio_jsonl = segment_dir.join("audio.jsonl");
        std::fs::write(
            audio_jsonl,
            "{\"_solstone_processing\":{\"handler\":\"transcribe\",\"state\":\"failed\",\"reason_code\":\"corrupt_input\"}}\n",
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "_default",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();

        assert_eq!(
            value.get("reason_code"),
            Some(&json!({
                "audio": "corrupt_input"
            }))
        );
    }

    #[test]
    fn prepare_segment_omits_reason_code_for_successful_and_empty_modalities() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        let audio_jsonl = segment_dir.join("audio.jsonl");
        std::fs::write(
            audio_jsonl,
            format!(
                "{{\"_solstone_processing\":{{\"handler\":\"{}\",\"state\":\"{}\",\"reason_code\":\"{}\"}}}}\n",
                vocab::HANDLER_TRANSCRIBE,
                vocab::STATE_EMPTY,
                vocab::REASON_NO_SPEECH
            ),
        )
        .unwrap();
        let screen_jsonl = segment_dir.join("screen.jsonl");
        std::fs::write(
            screen_jsonl,
            format!(
                "{{\"_solstone_processing\":{{\"handler\":\"{}\",\"state\":\"{}\"}}}}\n",
                vocab::HANDLER_DESCRIBE,
                vocab::STATE_ANALYZED
            ),
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "_default",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();

        assert_eq!(value.get("reason_code"), None);
    }

    #[test]
    fn prepare_segment_names_why_a_recording_settled_without_a_transcript() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(
            segment_dir.join("audio.jsonl"),
            format!(
                "{{\"_solstone_processing\":{{\"handler\":\"{}\",\"state\":\"{}\",\"reason_code\":\"{}\"}}}}\n",
                vocab::HANDLER_TRANSCRIBE,
                vocab::STATE_EMPTY,
                vocab::REASON_NO_SPEECH
            ),
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("screen.jsonl"),
            format!(
                "{{\"_solstone_processing\":{{\"handler\":\"{}\",\"state\":\"{}\",\"reason_code\":\"{}\",\"attempts\":1}}}}\n",
                vocab::HANDLER_DESCRIBE,
                vocab::STATE_FAILED,
                vocab::REASON_NO_DECODABLE_FRAMES
            ),
        )
        .unwrap();
        let value = super::prepare_segment(
            root,
            "20260101",
            "_default",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();
        assert_eq!(value["data_state"]["audio"], "empty");
        assert_eq!(
            value["processing_reason_code"],
            json!({"audio": "no_speech", "screen": "no_decodable_frames"})
        );
        // The failure-only field keeps its shape for the readers that have it.
        assert_eq!(
            value["reason_code"],
            json!({"screen": "no_decodable_frames"})
        );

        let day = crate::day::prepare_day(root, "20260101", jiff::Timestamp::now()).unwrap();
        let segment = serde_json::to_value(&day.segments).unwrap();
        assert_eq!(segment[0]["processing_reason_code"]["audio"], "no_speech");
    }

    #[test]
    fn prepare_segment_keeps_every_valid_turn_around_damaged_lines_and_names_no_host_path() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        let mut audio = Vec::new();
        audio.extend_from_slice(
            format!(
                "{{\"_solstone_processing\":{{\"handler\":\"{}\",\"state\":\"{}\"}}}}\n",
                vocab::HANDLER_TRANSCRIBE,
                vocab::STATE_ANALYZED
            )
            .as_bytes(),
        );
        audio.extend_from_slice(b"{\"start\":\"00:00:01\",\"text\":\"first turn\"}\n");
        audio.extend_from_slice(b"{\"start\":\"00:00:02\",\"text\":\"\xff\xfe\"}\n");
        audio.extend_from_slice(b"{\"start\":\"00:00:03\",\"text\":\"second turn\"}\n");
        audio.extend_from_slice(b"{\"start\":\"00:00:04\",\"te");
        std::fs::write(segment_dir.join("audio.jsonl"), &audio).unwrap();
        // An unreadable speaker file names its path in a warning.
        std::fs::create_dir_all(segment_dir.join("talents")).unwrap();
        std::fs::write(
            segment_dir.join("talents/speaker_labels.json"),
            b"{not json",
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "_default",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();
        let texts = value["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|chunk| chunk["type"] == "audio")
            .map(|chunk| chunk["markdown"].as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>();
        assert!(
            texts.iter().any(|text| text.contains("first turn")),
            "{texts:?}"
        );
        assert!(
            texts.iter().any(|text| text.contains("second turn")),
            "{texts:?}"
        );
        assert_eq!(
            value["damaged_lines"],
            json!([{
                "type": "audio",
                "file": "chronicle/20260101/120000_60/audio.jsonl",
                "valid": 3,
                "damaged": 2,
                "truncated_tail": true,
                "interior": 1,
            }])
        );
        assert_eq!(value["data_state"]["audio"], "analyzed");
        let serialized = value.to_string();
        assert!(
            !serialized.contains(&*root.to_string_lossy()),
            "the response names a host path"
        );
        // Reading never changes the owner's transcript.
        assert_eq!(
            std::fs::read(segment_dir.join("audio.jsonl")).unwrap(),
            audio
        );
    }

    #[test]
    fn prepare_segment_counts_a_wholly_damaged_recording_as_failed_not_pending() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(segment_dir.join("audio.jsonl"), b"\x00\x00\x00\x00").unwrap();
        std::fs::write(segment_dir.join("audio.flac"), b"raw").unwrap();
        let value = super::prepare_segment(
            root,
            "20260101",
            "_default",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();
        assert_eq!(value["damaged_lines"][0]["truncated_tail"], true);
        assert_eq!(value["data_state"]["audio"], "failed");
    }

    #[test]
    fn prepare_segment_media_removal_policy_audio() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(
            segment_dir.join("audio.jsonl"),
            "{\"raw\":\"raw.flac\"}\n{\"start\":\"0.0\",\"speaker\":\"1\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("events.jsonl"),
            "{\"tract\":\"retention\",\"event\":\"original_deleted\",\"name\":\"raw.flac\",\"class\":\"policy_raw_release\"}\n",
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "field",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();

        assert_eq!(value["media_purged"]["audio"], true);
        assert_eq!(
            value["media_removal"]["audio"],
            "you deleted this segment's original audio after your retention settings marked it"
        );
    }

    #[test]
    fn prepare_segment_names_the_zone_its_device_reported() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260929/phone/211400_300");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(
            segment_dir.join("audio.jsonl"),
            "{\"raw\":\"audio.flac\"}\n{\"start\":\"0.0\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        let receipt = serde_json::json!({
            "record_type": "device_ingest",
            "record_version": 1,
            "outcome": "accepted",
            "protocol_version": 3,
            "cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "source": "",
            "stream": "phone",
            "day": "20260929",
            "segment": "211400_300",
            "files": [],
            "meta": {"tz": "Asia/Tokyo", "utc_offset_seconds": 32400},
        });
        std::fs::write(segment_dir.join("events.jsonl"), format!("{receipt}\n")).unwrap();

        let value = super::prepare_segment(
            root,
            "20260929",
            "phone",
            "211400_300",
            jiff::Timestamp::now(),
        )
        .unwrap();

        assert_eq!(value["capture_zone"]["tz"], "Asia/Tokyo");
        assert_eq!(value["capture_zone"]["utc_offset_seconds"], 32400);
        assert_eq!(value["capture_zone"]["label"], "Tokyo");
    }

    #[test]
    fn capture_evidence_survives_reload_without_audio_or_transcripts() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let dir = root.join("chronicle/20260929/phone/211400_300");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("screen.mp4"), b"video").unwrap();
        let meta: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/audio-capture-v1.example.json"
        ))
        .unwrap();
        let receipt = serde_json::json!({"record_type": "device_ingest", "record_version": 1,
            "outcome": "accepted", "protocol_version": 3, "cid": "test", "source": "", "stream": "phone",
            "day": "20260929", "segment": "211400_300", "files": [], "meta": meta});
        std::fs::write(dir.join("events.jsonl"), format!("{receipt}\n")).unwrap();
        for _ in 0..2 {
            let value = super::prepare_segment(
                root,
                "20260929",
                "phone",
                "211400_300",
                jiff::Timestamp::now(),
            )
            .unwrap();
            assert_eq!(value["audio_capture"]["state"], "partial");
            let sources = value["audio_capture"]["sources"].as_array().unwrap();
            let system = sources
                .iter()
                .find(|row| row["source_id"] == "system")
                .unwrap();
            let mic = sources
                .iter()
                .find(|row| row["source_id"] == "mic")
                .unwrap();
            assert_eq!(system["statistics_available"], true);
            assert_eq!(system["statistics_complete"], true);
            assert_eq!(system["accepted_frames"], 960);
            assert_eq!(value["audio_capture"]["timeline_version"], 1);
            assert_eq!(system["timeline_origin_seconds"], 0.0);
            assert_eq!(system["generated_frames"], 1200);
            assert_eq!(system["gap_count"], 1);
            assert_eq!(mic["statistics_available"], false);
            assert_eq!(mic["statistics_complete"], false);
            assert_eq!(mic["writer_status"], "unknown");
            assert_eq!(value["warnings"], 2);
            assert_eq!(value["warning_details"][0]["type"], "audio_capture");
            assert!(value["audio_file"].is_null());
            assert!(value["chunks"].as_array().unwrap().is_empty());
            assert!(value["data_state"].get("audio").is_none());
        }
        std::fs::write(dir.join("events.jsonl"), format!("{receipt}\n{{torn\n")).unwrap();
        let value = super::prepare_segment(
            root,
            "20260929",
            "phone",
            "211400_300",
            jiff::Timestamp::now(),
        )
        .unwrap();
        assert_eq!(value["audio_capture"]["state"], "partial");
        assert_eq!(value["audio_capture"]["evidence_unavailable"], true);
    }

    #[test]
    fn prepare_segment_media_removal_offload_audio() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(
            segment_dir.join("audio.jsonl"),
            "{\"raw\":\"raw.flac\"}\n{\"start\":\"0.0\",\"speaker\":\"1\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("events.jsonl"),
            "{\"tract\":\"retention\",\"event\":\"original_deleted\",\"name\":\"raw.flac\",\"class\":\"offload_raw_release\"}\n",
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "field",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();

        assert_eq!(value["media_purged"]["audio"], true);
        assert_eq!(
            value["media_removal"]["audio"],
            "you deleted this segment's original audio after your backup copied it"
        );
    }

    #[test]
    fn prepare_segment_media_removal_owner_audio() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(
            segment_dir.join("audio.jsonl"),
            "{\"raw\":\"raw.flac\"}\n{\"start\":\"0.0\",\"speaker\":\"1\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("events.jsonl"),
            "{\"tract\":\"retention\",\"event\":\"original_deleted\",\"name\":\"raw.flac\",\"class\":\"owner_raw_release\"}\n",
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "field",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();

        assert_eq!(value["media_purged"]["audio"], true);
        assert_eq!(
            value["media_removal"]["audio"],
            "you deleted this segment's original audio"
        );
    }

    #[test]
    fn prepare_segment_media_removal_no_record_audio() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(
            segment_dir.join("audio.jsonl"),
            "{\"raw\":\"raw.flac\"}\n{\"start\":\"0.0\",\"speaker\":\"1\",\"text\":\"hello\"}\n",
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "field",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();

        assert_eq!(value["media_purged"]["audio"], true);
        assert_eq!(
            value["media_removal"]["audio"],
            "this segment's original audio is missing with no deletion record."
        );
        assert_eq!(value["media_removal"]["audio_missing_unrecorded"], true);
    }

    #[test]
    fn prepare_segment_media_removal_two_referenced_audio_mixed_classes_yields_no_record() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(
            segment_dir.join("audio.jsonl"),
            "{\"raw\":\"raw.flac\"}\n{\"start\":\"0.0\",\"speaker\":\"1\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("mic_audio.jsonl"),
            "{\"raw\":\"mic_raw.flac\"}\n{\"start\":\"0.0\",\"speaker\":\"1\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("events.jsonl"),
            "{\"tract\":\"retention\",\"event\":\"original_deleted\",\"name\":\"raw.flac\",\"class\":\"policy_raw_release\"}\n{\"tract\":\"retention\",\"event\":\"original_deleted\",\"name\":\"mic_raw.flac\",\"class\":\"offload_raw_release\"}\n",
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "field",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();

        assert_eq!(value["media_purged"]["audio"], true);
        assert_eq!(
            value["media_removal"]["audio"],
            "this segment's original audio is no longer in your journal"
        );
        assert!(
            value["media_removal"]
                .get("audio_missing_unrecorded")
                .is_none()
        );
    }

    #[test]
    fn prepare_segment_media_removal_policy_record_naming_stray_present_unreferenced_file_yields_no_record()
     {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(
            segment_dir.join("audio.jsonl"),
            "{\"raw\":\"raw.flac\"}\n{\"start\":\"0.0\",\"speaker\":\"1\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        std::fs::write(segment_dir.join("stray.flac"), b"stray audio content").unwrap();
        std::fs::write(
            segment_dir.join("events.jsonl"),
            "{\"tract\":\"retention\",\"event\":\"original_deleted\",\"name\":\"stray.flac\",\"class\":\"policy_raw_release\"}\n",
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "field",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();

        assert_eq!(value["media_purged"]["audio"], true);
        assert_eq!(
            value["media_removal"]["audio"],
            "this segment's original audio is missing with no deletion record."
        );
        assert_eq!(value["media_removal"]["audio_missing_unrecorded"], true);
    }

    #[test]
    fn prepare_segment_media_removal_two_referenced_files_only_one_recorded_yields_no_record() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(
            segment_dir.join("audio.jsonl"),
            "{\"raw\":\"raw1.flac\"}\n{\"start\":\"0.0\",\"speaker\":\"1\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("mic_audio.jsonl"),
            "{\"raw\":\"raw2.flac\"}\n{\"start\":\"0.0\",\"speaker\":\"1\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("events.jsonl"),
            "{\"tract\":\"retention\",\"event\":\"original_deleted\",\"name\":\"raw1.flac\",\"class\":\"policy_raw_release\"}\n",
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "field",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();

        assert_eq!(value["media_purged"]["audio"], true);
        assert_eq!(
            value["media_removal"]["audio"],
            "this segment's original audio is missing with no deletion record."
        );
        assert_eq!(value["media_removal"]["audio_missing_unrecorded"], true);
    }

    #[test]
    fn prepare_segment_media_removal_file_present_stale_record_is_not_purged() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(
            segment_dir.join("audio.jsonl"),
            "{\"raw\":\"raw.flac\"}\n{\"start\":\"0.0\",\"speaker\":\"1\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        std::fs::write(segment_dir.join("raw.flac"), b"present audio").unwrap();
        std::fs::write(
            segment_dir.join("events.jsonl"),
            "{\"tract\":\"retention\",\"event\":\"original_deleted\",\"name\":\"raw.flac\",\"class\":\"policy_raw_release\"}\n",
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "field",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();

        assert_eq!(value["media_purged"]["audio"], false);
        assert_eq!(value["media_removal"]["audio"], serde_json::Value::Null);
    }

    #[test]
    fn prepare_segment_media_removal_screen_policy() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(
            segment_dir.join("screen.jsonl"),
            "{\"raw\":\"screen.webm\"}\n{\"timestamp\":1700000000000,\"source\":\"screen.webm\"}\n",
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("events.jsonl"),
            "{\"tract\":\"retention\",\"event\":\"original_deleted\",\"name\":\"screen.webm\",\"class\":\"policy_raw_release\"}\n",
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "field",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();

        assert_eq!(value["media_purged"]["screen"], true);
        assert_eq!(
            value["media_removal"]["screen"],
            "you deleted this segment's original screen media after your retention settings marked it"
        );
    }

    #[test]
    fn prepare_segment_media_removal_policy_audio_and_missing_screen_no_record() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(
            segment_dir.join("audio.jsonl"),
            "{\"raw\":\"raw.flac\"}\n{\"start\":\"0.0\",\"speaker\":\"1\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("screen.jsonl"),
            "{\"raw\":\"screen.webm\"}\n{\"timestamp\":1700000000000,\"source\":\"screen.webm\"}\n",
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("events.jsonl"),
            "{\"tract\":\"retention\",\"event\":\"original_deleted\",\"name\":\"raw.flac\",\"class\":\"policy_raw_release\"}\n",
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "field",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();

        assert_eq!(value["media_purged"]["audio"], true);
        assert_eq!(value["media_purged"]["screen"], true);
        assert_eq!(
            value["media_removal"]["audio"],
            "you deleted this segment's original audio after your retention settings marked it"
        );
        assert_eq!(
            value["media_removal"]["screen"],
            "this segment's original screen media is missing with no deletion record."
        );
        assert_eq!(value["media_removal"]["screen_missing_unrecorded"], true);
    }

    #[test]
    fn prepare_segment_media_removal_referenced_subpath_with_basename_record_yields_no_record() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(
            segment_dir.join("audio.jsonl"),
            "{\"raw\":\"sub/mic_audio.flac\"}\n{\"start\":\"0.0\",\"speaker\":\"1\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("events.jsonl"),
            "{\"tract\":\"retention\",\"event\":\"original_deleted\",\"name\":\"mic_audio.flac\",\"class\":\"policy_raw_release\"}\n",
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "field",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();

        assert_eq!(value["media_purged"]["audio"], true);
        assert_eq!(
            value["media_removal"]["audio"],
            "this segment's original audio is no longer in your journal"
        );
    }

    #[test]
    fn prepare_segment_two_analyzed_stills() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(segment_dir.join("camera-1000-1.jpg"), b"dummy jpeg bytes 1").unwrap();
        std::fs::write(
            segment_dir.join("camera-1000-1.jsonl"),
            "{\"_solstone_processing\":{\"handler\":\"depict\",\"schema_version\":1,\"model\":\"mock\",\"device\":\"cpu\",\"state\":\"analyzed\"}}\n{\"start\":\"00:00:10\",\"text\":\"a desk with a laptop\",\"detection_error\":null}\n",
        )
        .unwrap();
        std::fs::write(segment_dir.join("camera-1000-2.jpg"), b"dummy jpeg bytes 2").unwrap();
        std::fs::write(
            segment_dir.join("camera-1000-2.jsonl"),
            "{\"_solstone_processing\":{\"handler\":\"depict\",\"schema_version\":1,\"model\":\"mock\",\"device\":\"cpu\",\"state\":\"analyzed\"}}\n{\"start\":\"00:00:20\",\"text\":\"a whiteboard with notes\"}\n",
        )
        .unwrap();

        let now = "2026-08-02T00:00:00Z".parse::<Timestamp>().unwrap();
        let value = super::prepare_segment(root, "20260101", "field", "120000_60", now).unwrap();

        assert_eq!(value["data_state"]["image"], "analyzed");
        assert!(value["data_state"].get("screen").is_none());
        assert!(value["media_sizes"].get("image").is_none());
        assert!(value["media_purged"].get("image").is_none());
        assert_eq!(
            value["image_files"]["camera-1000-1.jpg"],
            "/app/transcripts/api/serve_file/20260101/field/120000_60/camera-1000-1.jpg"
        );
        assert_eq!(
            value["image_files"]["camera-1000-2.jpg"],
            "/app/transcripts/api/serve_file/20260101/field/120000_60/camera-1000-2.jpg"
        );
        let chunks = value["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0]["type"], "image");
        assert_eq!(chunks[0]["markdown"], "a desk with a laptop");
        assert_eq!(chunks[0]["source_ref"]["raw"], "camera-1000-1.jpg");
        assert_eq!(chunks[0]["source_ref"]["media_kind"], "image");
        assert!(chunks[0]["source_ref"].get("detection_error").is_none());

        assert_eq!(chunks[1]["type"], "image");
        assert_eq!(chunks[1]["markdown"], "a whiteboard with notes");
        assert_eq!(chunks[1]["source_ref"]["raw"], "camera-1000-2.jpg");
        assert_eq!(chunks[1]["source_ref"]["media_kind"], "image");
        assert!(chunks[1]["source_ref"].get("detection_error").is_none());

        let (_, _, segments) = solstone_core_system_health::scan_day(
            &solstone_core_system_health::FilesystemSegmentSource,
            root,
            "20260101",
            now,
        )
        .unwrap();
        let segment = segments.iter().find(|s| s.key == "120000_60").unwrap();
        assert_eq!(segment.stream, "field");
        assert_eq!(
            value["data_state"]["image"].as_str(),
            segment.data_state.0.get("image").map(|s| s.as_str())
        );
    }

    #[test]
    fn prepare_segment_one_analyzed_one_bare_still() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(segment_dir.join("still-1.jpg"), b"dummy jpeg bytes 1").unwrap();
        std::fs::write(
            segment_dir.join("still-1.jsonl"),
            "{\"_solstone_processing\":{\"handler\":\"depict\",\"schema_version\":1,\"model\":\"mock\",\"device\":\"cpu\",\"state\":\"analyzed\"}}\n{\"start\":\"00:00:10\",\"text\":\"a desk with a laptop\"}\n",
        )
        .unwrap();
        std::fs::write(segment_dir.join("still-2.jpg"), b"dummy jpeg bytes 2").unwrap();

        let now = "2026-08-02T00:00:00Z".parse::<Timestamp>().unwrap();
        let value = super::prepare_segment(root, "20260101", "field", "120000_60", now).unwrap();

        assert_eq!(value["data_state"]["image"], "pending");
        let image_files = value["image_files"].as_object().unwrap();
        assert_eq!(image_files.len(), 1);
        assert_eq!(
            image_files["still-1.jpg"],
            "/app/transcripts/api/serve_file/20260101/field/120000_60/still-1.jpg"
        );
        assert!(!image_files.contains_key("still-2.jpg"));
        let chunks = value["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["source_ref"]["raw"], "still-1.jpg");

        let (_, _, segments) = solstone_core_system_health::scan_day(
            &solstone_core_system_health::FilesystemSegmentSource,
            root,
            "20260101",
            now,
        )
        .unwrap();
        let segment = segments.iter().find(|s| s.key == "120000_60").unwrap();
        assert_eq!(segment.stream, "field");
        assert_eq!(
            value["data_state"]["image"].as_str(),
            segment.data_state.0.get("image").map(|s| s.as_str())
        );
    }

    #[test]
    fn prepare_segment_bare_still_no_sidecar() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(segment_dir.join("bare.jpg"), b"dummy jpeg bytes").unwrap();

        let now = "2026-08-02T00:00:00Z".parse::<Timestamp>().unwrap();
        let value = super::prepare_segment(root, "20260101", "field", "120000_60", now).unwrap();

        assert_eq!(value["data_state"]["image"], "pending");
        assert!(value["image_files"].as_object().unwrap().is_empty());
        assert_eq!(value["chunks"].as_array().unwrap().len(), 0);

        let (_, _, segments) = solstone_core_system_health::scan_day(
            &solstone_core_system_health::FilesystemSegmentSource,
            root,
            "20260101",
            now,
        )
        .unwrap();
        let segment = segments.iter().find(|s| s.key == "120000_60").unwrap();
        assert_eq!(segment.stream, "field");
        assert_eq!(
            value["data_state"]["image"].as_str(),
            segment.data_state.0.get("image").map(|s| s.as_str())
        );
    }

    #[test]
    fn prepare_segment_depict_failed_sidecar_does_not_emit_chunks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(segment_dir.join("photo.jpg"), b"dummy jpeg bytes").unwrap();
        std::fs::write(
            segment_dir.join("photo.jsonl"),
            "{\"_solstone_processing\":{\"handler\":\"depict\",\"schema_version\":1,\"model\":\"mock\",\"device\":\"cpu\",\"state\":\"failed\"}}\n{\"start\":\"00:00:10\",\"text\":\"a desk with a laptop\"}\n",
        )
        .unwrap();

        let now = "2026-08-02T00:00:00Z".parse::<Timestamp>().unwrap();
        let value = super::prepare_segment(root, "20260101", "field", "120000_60", now).unwrap();

        assert_eq!(value["data_state"]["image"], "failed");
        assert!(value["image_files"].as_object().unwrap().is_empty());
        assert_eq!(value["chunks"].as_array().unwrap().len(), 0);

        let (_, _, segments) = solstone_core_system_health::scan_day(
            &solstone_core_system_health::FilesystemSegmentSource,
            root,
            "20260101",
            now,
        )
        .unwrap();
        let segment = segments.iter().find(|s| s.key == "120000_60").unwrap();
        assert_eq!(segment.stream, "field");
        assert_eq!(
            value["data_state"]["image"].as_str(),
            segment.data_state.0.get("image").map(|s| s.as_str())
        );
    }

    #[test]
    fn prepare_segment_depict_header_only_no_text_rows() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(segment_dir.join("photo.jpg"), b"dummy jpeg bytes").unwrap();
        std::fs::write(
            segment_dir.join("photo.jsonl"),
            "{\"_solstone_processing\":{\"handler\":\"depict\",\"schema_version\":1,\"model\":\"mock\",\"device\":\"cpu\",\"state\":\"analyzed\"}}\n",
        )
        .unwrap();

        let now = "2026-08-02T00:00:00Z".parse::<Timestamp>().unwrap();
        let value = super::prepare_segment(root, "20260101", "field", "120000_60", now).unwrap();

        assert_eq!(value["data_state"]["image"], "pending");
        assert!(value["image_files"].as_object().unwrap().is_empty());
        assert_eq!(value["chunks"].as_array().unwrap().len(), 0);

        let (_, _, segments) = solstone_core_system_health::scan_day(
            &solstone_core_system_health::FilesystemSegmentSource,
            root,
            "20260101",
            now,
        )
        .unwrap();
        let segment = segments.iter().find(|s| s.key == "120000_60").unwrap();
        assert_eq!(segment.stream, "field");
        assert_eq!(
            value["data_state"]["image"].as_str(),
            segment.data_state.0.get("image").map(|s| s.as_str())
        );
    }

    #[test]
    fn prepare_segment_depict_detection_error_included_in_source_ref() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(segment_dir.join("photo.jpg"), b"dummy jpeg bytes").unwrap();
        std::fs::write(
            segment_dir.join("photo.jsonl"),
            "{\"_solstone_processing\":{\"handler\":\"depict\",\"schema_version\":1,\"model\":\"mock\",\"device\":\"cpu\",\"state\":\"analyzed\"}}\n{\"start\":\"00:00:05\",\"text\":\"scene\",\"detection_error\":{\"reason_code\":\"rfdetr-unavailable\",\"detail\":\"detector did not run\"}}\n",
        )
        .unwrap();

        let now = "2026-08-02T00:00:00Z".parse::<Timestamp>().unwrap();
        let value = super::prepare_segment(root, "20260101", "field", "120000_60", now).unwrap();

        let chunks = value["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0];
        assert_eq!(
            chunk["source_ref"]["detection_error"],
            json!({"reason_code": "rfdetr-unavailable", "detail": "detector did not run"})
        );
    }

    #[test]
    fn prepare_segment_carries_qualified_objects_for_frames_and_stills() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(segment_dir.join("photo.jpg"), b"dummy jpeg bytes").unwrap();
        std::fs::write(
            segment_dir.join("photo.jsonl"),
            "{\"_solstone_processing\":{\"handler\":\"depict\",\"schema_version\":1,\"model\":\"mock\",\"device\":\"cpu\",\"state\":\"analyzed\"}}\n{\"start\":\"00:00:05\",\"text\":\"a picnic\",\"detections\":{\"source\":\"still\",\"image\":{\"width\":400,\"height\":300},\"objects\":[{\"class_name\":\"person\",\"score\":0.9,\"bbox\":[10,20,110,220]},{\"class_name\":\"kite\",\"score\":0.9,\"bbox\":[0,0,5,5]}]}}\n",
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("screen.jsonl"),
            "{\"frame_id\":1,\"timestamp\":2.0,\"analysis\":{\"primary\":\"media\",\"visual_description\":\"a stream\"},\"detections\":{\"source\":\"screen\",\"image\":{\"width\":1920,\"height\":1080},\"objects\":[{\"class_name\":\"person\",\"score\":0.8,\"bbox\":[1,2,3,4]},{\"class_name\":\"laptop\",\"score\":0.9,\"bbox\":[0,0,9,9]}]}}\n{\"frame_id\":2,\"timestamp\":3.0,\"analysis\":{\"primary\":\"code\",\"visual_description\":\"an editor\"}}\n",
        )
        .unwrap();

        let now = "2026-08-02T00:00:00Z".parse::<Timestamp>().unwrap();
        let value = super::prepare_segment(root, "20260101", "field", "120000_60", now).unwrap();
        let chunks = value["chunks"].as_array().unwrap();

        let still = chunks.iter().find(|c| c["type"] == "image").unwrap();
        assert_eq!(still["markdown"], "a picnic\n\n**Objects:** person");
        assert_eq!(
            still["source_ref"]["objects"],
            json!({"image":{"width":400.0,"height":300.0},"items":[{"label":"person","score":0.9,"bbox":[10.0,20.0,110.0,220.0]}]})
        );

        let screens = chunks
            .iter()
            .filter(|c| c["type"] == "screen")
            .collect::<Vec<_>>();
        assert_eq!(screens.len(), 2);
        assert!(
            screens[0]["markdown"]
                .as_str()
                .unwrap()
                .contains("**Objects:** person")
        );
        assert_eq!(
            screens[0]["source_ref"]["objects"]["items"],
            json!([{"label":"person","score":0.8,"bbox":[1.0,2.0,3.0,4.0]}])
        );
        assert!(screens[1]["source_ref"].get("objects").is_none());
    }

    #[test]
    fn prepare_segment_describe_claimed_still_differential() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        let png_bytes = b"dummy png media bytes";
        std::fs::write(segment_dir.join("screen.png"), png_bytes).unwrap();
        std::fs::write(
            segment_dir.join("screen.jsonl"),
            "{\"_solstone_processing\":{\"handler\":\"describe\",\"schema_version\":1,\"model\":\"mock\",\"device\":\"cpu\",\"state\":\"analyzed\"}}\n{\"timestamp\":1700000000000,\"source\":\"screen.png\",\"start\":\"00:00:00\",\"text\":\"screen content\"}\n",
        )
        .unwrap();

        let now = "2026-08-02T00:00:00Z".parse::<Timestamp>().unwrap();
        let value = super::prepare_segment(root, "20260101", "field", "120000_60", now).unwrap();

        assert!(value["data_state"].get("image").is_none());
        assert_eq!(value["data_state"]["screen"], "analyzed");
        assert_eq!(value["media_sizes"]["screen"], png_bytes.len());

        let (_, _, segments) = solstone_core_system_health::scan_day(
            &solstone_core_system_health::FilesystemSegmentSource,
            root,
            "20260101",
            now,
        )
        .unwrap();
        let segment = segments.iter().find(|s| s.key == "120000_60").unwrap();
        assert_eq!(
            value["data_state"]["screen"].as_str(),
            segment.data_state.0.get("screen").map(|s| s.as_str())
        );
        assert_eq!(segment.data_state.0.get("image"), None);
    }

    #[test]
    fn a_screenshot_entry_carries_its_text_and_no_file_note() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/phone_screenshots/091405_1");
        std::fs::create_dir_all(&segment_dir).unwrap();
        std::fs::write(
            segment_dir.join("screenshot_screen.png"),
            b"screenshot bytes",
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("screenshot_screen.jsonl"),
            concat!(
                "{\"raw\":\"screenshot_screen.png\",\"_solstone_processing\":{\"handler\":\"describe\",\"state\":\"analyzed\"}}\n",
                "{\"frame_id\":1,\"timestamp\":0.0,\"analysis\":{\"visual_description\":\"a boarding pass\",\"primary\":\"reading\"},",
                "\"content\":{\"reading\":\"Gate B14\"}}\n",
            ),
        )
        .unwrap();

        let now = "2026-08-02T00:00:00Z".parse::<Timestamp>().unwrap();
        let value =
            super::prepare_segment(root, "20260101", "phone_screenshots", "091405_1", now).unwrap();
        let chunks = value["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), 1, "{chunks:?}");
        let chunk = &chunks[0];
        assert_eq!(chunk["screenshot"]["text"], "searchable");
        let markdown = chunk["markdown"].as_str().unwrap();
        assert!(markdown.contains("Gate B14"), "{markdown}");
        assert!(!markdown.contains("photo_file"), "{markdown}");
        assert!(!markdown.contains("screenshot_screen.png"), "{markdown}");
        assert!(
            value["image_files"]
                .as_object()
                .is_some_and(|files| files.contains_key("screenshot_screen.png")),
            "{value}"
        );
    }

    #[test]
    fn prepare_segment_twin_fixture_negative_differential() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/convey_records_journal");
        let now = "2026-08-02T00:00:00Z".parse::<Timestamp>().unwrap();
        let value = super::prepare_segment(&root, "20260731", "field", "090000_300", now).unwrap();

        let (_, _, segments) = solstone_core_system_health::scan_day(
            &solstone_core_system_health::FilesystemSegmentSource,
            &root,
            "20260731",
            now,
        )
        .unwrap();
        let segment = segments.iter().find(|s| s.key == "090000_300").unwrap();

        assert!(value["data_state"].get("image").is_none());
        assert_eq!(segment.data_state.0.get("image"), None);
        assert_eq!(value["data_state"]["screen"], "analyzed");
        assert_eq!(value["media_sizes"]["screen"], 20);
        assert_eq!(value["media_sizes"]["audio"], 17);
        let chunks = value["chunks"].as_array().unwrap();
        let screen_chunks_count = chunks.iter().filter(|c| c["type"] == "screen").count();
        let image_chunks_count = chunks.iter().filter(|c| c["type"] == "image").count();
        assert_eq!(screen_chunks_count, 1);
        assert_eq!(image_chunks_count, 0);
    }

    const STILL_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    struct RefusingWire;

    impl solstone_core_depict::WireClient for RefusingWire {
        fn execute(
            &self,
            _: &solstone_core_generate::GenerateRequest,
        ) -> Result<solstone_core_generate::GenerateResponse, solstone_core_generate::ClientError>
        {
            Ok(solstone_core_generate::GenerateResponse::Refused(
                solstone_core_generate::RefusedResponse {
                    id: None,
                    reason: solstone_core_generate::RefusalReason::IncompleteText,
                    reason_code: Some(solstone_core_generate::ReasonCodeValue::Known(
                        solstone_core_generate::ReasonCode::new("incomplete_text_length")
                            .expect("known reason"),
                    )),
                    retryable: false,
                    blocking: false,
                    reset_at_ms: None,
                    provider: None,
                    detail: "wire detail".to_owned(),
                },
            ))
        }
    }

    struct SilentDetector;

    impl solstone_core_depict::Detector for SilentDetector {
        fn detect(&self, _: &[u8]) -> Result<Option<serde_json::Value>, String> {
            Ok(None)
        }
    }

    #[test]
    fn writer_failed_depict_sidecar_is_failed_for_health_and_transcripts() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root
            .join("chronicle")
            .join("20260101")
            .join("field")
            .join("120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        let image = segment_dir.join("photo.png");
        std::fs::write(&image, STILL_PNG).unwrap();
        let error =
            solstone_core_depict::run_with_clients(&image, false, &RefusingWire, &SilentDetector);
        assert!(error.is_err());
        assert_eq!(std::fs::read(&image).unwrap(), STILL_PNG);

        let now = "2026-08-02T00:00:00Z".parse::<Timestamp>().unwrap();
        let value = super::prepare_segment(root, "20260101", "field", "120000_60", now).unwrap();
        assert_eq!(value["data_state"]["image"], "failed");
        assert!(value["image_files"].as_object().unwrap().is_empty());
        assert_eq!(value["chunks"].as_array().unwrap().len(), 0);

        let (_, _, segments) = solstone_core_system_health::scan_day(
            &solstone_core_system_health::FilesystemSegmentSource,
            root,
            "20260101",
            now,
        )
        .unwrap();
        let segment = segments
            .iter()
            .find(|item| item.key == "120000_60")
            .unwrap();
        assert_eq!(
            segment.data_state.0.get("image").map(String::as_str),
            Some("failed")
        );

        let sidecar = std::fs::read_to_string(image.with_extension("jsonl")).unwrap();
        let lines = sidecar.lines().filter(|line| !line.is_empty()).count();
        assert_eq!(lines, 1);
        let header: serde_json::Value =
            serde_json::from_str(sidecar.lines().next().unwrap()).unwrap();
        let record = &header["_solstone_processing"];
        assert_eq!(record["state"], vocab::STATE_FAILED);
        assert_eq!(record["reason_code"], vocab::REASON_ANALYSIS_FAILED);
        assert_eq!(record["handler"], vocab::HANDLER_DEPICT);
        assert!(header.get("text").is_none());
    }

    #[test]
    fn prepare_segment_location_keys() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let now = jiff::Timestamp::now();
        let day = "20260101";

        // Segment 1: base (no location.jsonl)
        let base_dir = root
            .join("chronicle")
            .join(day)
            .join("phone")
            .join("120000_60");
        std::fs::create_dir_all(&base_dir).unwrap();
        std::fs::write(base_dir.join("stream.json"), "{}\n").unwrap();

        let base_val = super::prepare_segment(root, day, "phone", "120000_60", now).unwrap();
        assert!(base_val.get("holds_location").is_none());
        assert!(base_val.get("location_readings").is_none());
        let base_keys: Vec<String> = base_val.as_object().unwrap().keys().cloned().collect();

        // Segment 2: with readable location.jsonl (twin directory)
        let loc_dir = root
            .join("chronicle")
            .join(day)
            .join("watch")
            .join("120000_60");
        std::fs::create_dir_all(&loc_dir).unwrap();
        std::fs::write(loc_dir.join("stream.json"), "{}\n").unwrap();
        std::fs::write(
            loc_dir.join("location.jsonl"),
            "{\"record_type\":\"solstone.location.segment/1\"}\n{\"record_type\":\"solstone.location.fix/1\"}\n{\"record_type\":\"solstone.location.fix/1\"}\n",
        )
        .unwrap();

        let loc_val = super::prepare_segment(root, day, "watch", "120000_60", now).unwrap();
        assert_eq!(
            loc_val.get("holds_location"),
            Some(&serde_json::Value::Bool(true))
        );
        assert_eq!(
            loc_val.get("location_readings"),
            Some(&serde_json::json!(2))
        );

        let loc_keys: Vec<String> = loc_val.as_object().unwrap().keys().cloned().collect();
        assert_eq!(&loc_keys[..base_keys.len()], &base_keys[..]);
        assert_eq!(
            &loc_keys[base_keys.len()..],
            &["holds_location", "location_readings"]
        );

        // Segment 3: invalid UTF-8 location.jsonl
        let inv_dir = root
            .join("chronicle")
            .join(day)
            .join("car")
            .join("120000_60");
        std::fs::create_dir_all(&inv_dir).unwrap();
        std::fs::write(inv_dir.join("stream.json"), "{}\n").unwrap();
        std::fs::write(inv_dir.join("location.jsonl"), [0xff, 0xfe]).unwrap();

        let inv_val = super::prepare_segment(root, day, "car", "120000_60", now).unwrap();
        assert_eq!(
            inv_val.get("holds_location"),
            Some(&serde_json::Value::Bool(true))
        );
        assert!(inv_val.get("location_readings").is_none());
        let inv_keys: Vec<String> = inv_val.as_object().unwrap().keys().cloned().collect();
        assert_eq!(&inv_keys[..base_keys.len()], &base_keys[..]);
        assert_eq!(&inv_keys[base_keys.len()..], &["holds_location"]);
    }

    #[test]
    fn prepare_segment_protected_video_produces_markdown_chunks_without_warnings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();

        let row_a = r#"{"t":"protected_video","ts":1792027800000,"end_ts":1792030500000,"service":"Netflix","app":"Comet","title":"Netflix"}"#;
        let malformed = "{\n";
        let row_b = r#"{"t":"protected_video","ts":1000,"end_ts":91000,"service":"Apple TV"}"#;
        std::fs::write(
            segment_dir.join("120000_60_protected_video.jsonl"),
            format!("{row_a}\n{malformed}\n{row_b}\n"),
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20260101",
            "field",
            "120000_60",
            jiff::Timestamp::now(),
        )
        .unwrap();

        let chunks = value["chunks"].as_array().expect("chunks array");
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0]["type"], "markdown");
        assert_eq!(chunks[1]["type"], "markdown");
        assert_eq!(value["warnings"], 0);
        let warning_details = value["warning_details"]
            .as_array()
            .expect("warning details array");
        assert!(warning_details.is_empty());

        assert_eq!(chunks[0]["timestamp"], 1_000_i64);
        assert_eq!(chunks[1]["timestamp"], 1_792_027_800_000_i64);

        let zone = solstone_core_journal_config::owner_zone(root);
        assert_eq!(chunks[0]["time"], local_time(&zone, 1_000));
        assert_eq!(chunks[1]["time"], local_time(&zone, 1_792_027_800_000));
    }

    #[test]
    fn prepare_segment_audio_chunks_appends_warning_to_flagged_sentence() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let day = "20260101";
        let stream = "field";
        let key = "120000_60";
        let segment_dir = root.join("chronicle").join(day).join(stream).join(key);
        std::fs::create_dir_all(&segment_dir).unwrap();

        let header = json!({"raw": "audio.wav"});
        let flagged_row = json!({
            "start": "12:00:10",
            "text": "Is the gate open? It is open.",
            "speaker": 1,
            "sentence_id": 1,
            "turn_boundary_suspect": true,
            "speaker_turn_evidence": {
                "version": "window-local-turns-v1",
                "audio_duration_s": 20.0,
                "ranges": [{"start_s": 11.0, "end_s": 12.0}],
            },
        });
        let neighbor_row = json!({
            "start": "12:00:15",
            "text": "Neighbor speech without warning.",
            "speaker": 2,
            "sentence_id": 2,
        });

        std::fs::write(
            segment_dir.join("audio.jsonl"),
            format!(
                "{}\n{}\n{}\n",
                serde_json::to_string(&header).unwrap(),
                serde_json::to_string(&flagged_row).unwrap(),
                serde_json::to_string(&neighbor_row).unwrap(),
            ),
        )
        .unwrap();

        let now = "2026-01-02T03:04:05Z".parse::<Timestamp>().unwrap();
        let value = super::prepare_segment(root, day, stream, key, now).unwrap();

        let chunks = value["chunks"].as_array().expect("chunks array");
        assert_eq!(chunks.len(), 2);

        let flagged_chunk = &chunks[0];
        let flagged_markdown = flagged_chunk["markdown"].as_str().expect("markdown string");
        assert!(flagged_markdown.contains("Is the gate open? It is open."));
        assert!(flagged_markdown.contains(super::POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE));
        assert!(flagged_markdown.ends_with(&format!(
            "[{}]",
            super::POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE
        )));
        assert_eq!(flagged_chunk["source_ref"]["speaker"], 1);

        let neighbor_chunk = &chunks[1];
        let neighbor_markdown = neighbor_chunk["markdown"]
            .as_str()
            .expect("markdown string");
        assert_eq!(neighbor_markdown, "Neighbor speech without warning.");
        assert!(!neighbor_markdown.contains(super::POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE));
    }

    #[test]
    fn prepare_segment_and_day_desktop_activity() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let day = "20260910";
        let stream = "desk";
        let key = "120000_60";
        let segment_dir = root.join("chronicle").join(day).join(stream).join(key);
        std::fs::create_dir_all(&segment_dir).unwrap();

        let row_1 = r#"{"v":1,"t":"desktop_activity","kind":"foreground","method":"nsworkspace","ts":1000,"end_ts":2000,"app":"Finder"}"#;
        let row_unsupported = r#"{"v":999,"t":"desktop_activity","kind":"foreground","method":"nsworkspace","ts":1500,"end_ts":2500,"app":"DistinctiveSecretApp"}"#;
        let row_2 = r#"{"v":1,"t":"desktop_activity","kind":"terminal","method":"tmux","ts":3000,"end_ts":4000,"exe":"bash"}"#;
        std::fs::write(
            segment_dir.join("120000_60_desktop_activity.jsonl"),
            format!("{row_1}\n{row_unsupported}\n"),
        )
        .unwrap();
        std::fs::write(
            segment_dir.join("120000_60_a1_desktop_activity.jsonl"),
            format!("{row_2}\n"),
        )
        .unwrap();

        let now = jiff::Timestamp::now();

        let value = super::prepare_segment(root, day, stream, key, now).unwrap();
        let chunks = value["chunks"].as_array().expect("chunks array");
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0]["type"], "markdown");
        assert_eq!(chunks[1]["type"], "markdown");
        assert!(chunks[0]["markdown"].as_str().unwrap().contains("Finder"));
        assert!(chunks[1]["markdown"].as_str().unwrap().contains("bash"));

        assert!(value["warnings"].as_u64().unwrap_or(0) >= 1);
        let warning_details = value["warning_details"]
            .as_array()
            .expect("warning_details");
        let act_warning = warning_details
            .iter()
            .find(|d| {
                d["type"] == "desktop_activity"
                    && d["message"].as_str().is_some_and(|m| !m.is_empty())
            })
            .expect("desktop_activity warning");
        let msg = act_warning["message"].as_str().unwrap();
        assert!(!msg.contains("DistinctiveSecretApp"));

        let prepared = crate::day::prepare_day(root, day, now)
            .map_err(|e| e.to_string())
            .unwrap();
        let act_segments: Vec<_> = prepared
            .segments
            .iter()
            .filter(|s| s.types.contains(&"desktop_activity".to_string()))
            .collect();
        assert_eq!(act_segments.len(), 1);
        assert_eq!(act_segments[0].types, vec!["desktop_activity".to_string()]);
        assert_eq!(act_segments[0].key, key);
        assert_eq!(act_segments[0].stream, stream);
        assert_eq!(
            act_segments[0]
                .data_state
                .get("desktop_activity")
                .map(String::as_str),
            Some("analyzed")
        );
        assert_eq!(act_segments[0].think.as_deref(), Some("awaiting"));

        assert!(prepared.audio.is_empty());
        assert!(prepared.screen.is_empty());
    }

    mod who_spoke {
        use std::fs;
        use std::io::Write;
        use std::path::Path;

        use serde_json::{Value, json};
        use sha2::{Digest, Sha256};
        use zip::ZipWriter;

        const DAY: &str = "20260101";
        const STREAM: &str = "_default";
        const KEY: &str = "120000_60";
        const LINES: [&str; 3] = [
            r#"{"start":"00:00:01","speaker":1,"sentence_id":1,"text":"Let us begin"}"#,
            r#"{"start":"00:00:02","speaker":2,"sentence_id":2,"text":"I will send it"}"#,
            r#"{"start":"00:00:03","speaker":1,"sentence_id":3,"text":"Sounds good"}"#,
        ];

        fn npz(path: &Path, ids: &[i32]) {
            let mut zip = ZipWriter::new(fs::File::create(path).unwrap());
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("statement_ids.npy", options).unwrap();
            let payload = ids
                .iter()
                .flat_map(|id| id.to_le_bytes())
                .collect::<Vec<_>>();
            zip.write_all(&solstone_core_npy::write_npy(
                "<i4",
                &format!("({},)", ids.len()),
                &payload,
            ))
            .unwrap();
            zip.finish().unwrap();
        }

        /// The owner, Quinn and Pat; Quinn is heard by voice on lines 1 and 3, a
        /// pool voice confirmed as Quinn covers line 3, and the owner's voice is
        /// matched on line 2.
        fn attributed(root: &Path) -> std::path::PathBuf {
            for entity in [
                json!({"id":"owner","name":"Owner","type":"Person","is_principal":true}),
                json!({"id":"quinn","name":"Quinn","type":"Person"}),
                json!({"id":"pat","name":"Pat","type":"Person"}),
            ] {
                let dir = root.join("entities").join(entity["id"].as_str().unwrap());
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("entity.json"), entity.to_string()).unwrap();
            }
            let dir = root.join("chronicle").join(DAY).join(KEY);
            fs::create_dir_all(dir.join("talents")).unwrap();
            let mut jsonl = String::from("{\"raw\":\"audio.flac\"}\n");
            for line in LINES {
                jsonl.push_str(line);
                jsonl.push('\n');
            }
            fs::write(dir.join("audio.jsonl"), jsonl).unwrap();
            npz(&dir.join("audio.npz"), &[1, 2, 3]);
            fs::write(
                dir.join("talents/speaker_labels.json"),
                json!({"labels": [
                    {"sentence_id": 1, "speaker": "quinn", "method": "acoustic", "confidence": "high"},
                    {"sentence_id": 2, "speaker": "owner", "method": "owner_centroid", "confidence": "high"},
                    {"sentence_id": 3, "speaker": "quinn", "method": "acoustic", "confidence": "high"}
                ]})
                .to_string(),
            )
            .unwrap();
            fs::create_dir_all(root.join("awareness")).unwrap();
            fs::write(
                root.join("awareness/speaker_candidates.json"),
                json!({"next_id":20,"candidates":[
                    {"cand_id":7,"centroid":[1.0],"status":"confirmed","confirmed_entity":"quinn","source_segments":[
                        {"day":DAY,"stream_layout":"direct","stream":STREAM,"segment_key":KEY,"source":"audio","cluster_label":1,"sentence_ids":[3]}
                    ]}
                ]})
                .to_string(),
            )
            .unwrap();
            dir
        }

        /// The speaker part of the page: its label state, warning kinds, and
        /// each audio line's speaker fields.
        fn page(root: &Path) -> Value {
            let value =
                super::super::prepare_segment(root, DAY, STREAM, KEY, jiff::Timestamp::now())
                    .unwrap();
            let lines = value["chunks"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|chunk| chunk["type"] == "audio")
                .map(|chunk| {
                    let mut line = serde_json::Map::new();
                    for key in [
                        "sentence_id",
                        "speaker_label",
                        "speaker_voice",
                        "speaker_actionable",
                        "owner_line_label",
                    ] {
                        if let Some(field) = chunk.get(key) {
                            line.insert(key.to_owned(), field.clone());
                        }
                    }
                    Value::Object(line)
                })
                .collect::<Vec<_>>();
            let warnings = value["warning_details"]
                .as_array()
                .unwrap()
                .iter()
                .map(|warning| warning["type"].clone())
                .collect::<Vec<_>>();
            json!({"speaker_labels": value["speaker_labels"], "warnings": warnings, "lines": lines})
        }

        fn answer(dir: &Path, entries: &[(usize, Option<&str>)]) {
            let entries = entries
                .iter()
                .map(|(line, speaker)| {
                    let record: Value = serde_json::from_str(LINES[line - 1]).unwrap();
                    json!({
                        "source": "audio",
                        "sentence_id": line,
                        "start": record["start"],
                        "text_sha256": format!("{:x}", Sha256::digest(record["text"].as_str().unwrap())),
                        "speaker": speaker,
                        "set_at": "2026-10-31T12:00:00Z",
                    })
                })
                .collect::<Vec<_>>();
            fs::write(
                dir.join("talents/owner_line_labels.json"),
                json!({"version": 1, "entries": entries}).to_string(),
            )
            .unwrap();
        }

        /// The page without the who-spoke presence flag, which is new, and
        /// that flag.
        fn before_shape(mut page: Value) -> (Value, bool) {
            let present = page["speaker_labels"]
                .as_object_mut()
                .unwrap()
                .remove("who_spoke_present")
                .unwrap();
            (page, present.as_bool().unwrap())
        }

        fn label(name: &str, id: &str, method: &str, is_owner: bool) -> Value {
            json!({"name":name,"entity_id":id,"confidence":"high","confidence_state":"high","method":method,"owner_margin_declined":false,"acoustic_margin_declined":false,"is_owner":is_owner})
        }

        /// The attributed segment's speaker part before answers existed.
        fn attributed_before() -> Value {
            json!({"speaker_labels":{"present":true,"loaded":true,"source":"audio","ambiguous":false},"warnings":[],"lines":[
                {"sentence_id":1,"speaker_label":label("Quinn","quinn","acoustic",false),"speaker_actionable":true},
                {"sentence_id":2,"speaker_label":label("Owner","owner","owner_centroid",true),"speaker_actionable":true},
                {"sentence_id":3,"speaker_label":label("Quinn","quinn","acoustic",false),"speaker_actionable":true}
            ]})
        }

        /// The same segment without its labels file, before answers existed.
        fn unlabelled_before() -> Value {
            json!({"speaker_labels":{"present":false,"loaded":false,"source":null,"ambiguous":false},"warnings":[],"lines":[
                {"sentence_id":1,"speaker_actionable":false},
                {"sentence_id":2,"speaker_actionable":false},
                {"sentence_id":3,"speaker_voice":{"voice_id":7,"entity_id":"quinn","name":"Quinn"},"speaker_actionable":false}
            ]})
        }

        fn marker(speaker: Option<&str>) -> Value {
            json!({"speaker": speaker, "set_at": "2026-10-31T12:00:00Z"})
        }

        #[test]
        fn chips_follow_the_owners_answers() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let dir = attributed(root);
            assert_eq!(before_shape(page(root)), (attributed_before(), true));

            answer(&dir, &[(1, Some("owner")), (2, Some("pat")), (3, None)]);
            let shown = page(root);
            assert_eq!(shown["speaker_labels"]["who_spoke_present"], true);
            assert_eq!(
                shown["lines"],
                json!([
                    {"sentence_id":1,"speaker_label":label("Owner","owner","user_labeled",true),"speaker_actionable":false,"owner_line_label":marker(Some("owner"))},
                    {"sentence_id":2,"speaker_label":label("Pat","pat","user_labeled",false),"speaker_actionable":false,"owner_line_label":marker(Some("pat"))},
                    {"sentence_id":3,"speaker_actionable":false,"owner_line_label":marker(None)}
                ])
            );
            // The line the owner doesn't know never takes the pool voice's name.
            let full =
                super::super::prepare_segment(root, DAY, STREAM, KEY, jiff::Timestamp::now())
                    .unwrap();
            let unknown = full["chunks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|chunk| chunk["sentence_id"] == 3)
                .unwrap()
                .to_string()
                .to_lowercase();
            assert!(!unknown.contains("quinn"), "{unknown}");

            fs::remove_file(dir.join("talents/owner_line_labels.json")).unwrap();
            assert_eq!(before_shape(page(root)), (attributed_before(), true));
            answer(&dir, &[]);
            assert_eq!(before_shape(page(root)), (attributed_before(), true));
        }

        #[test]
        fn an_answer_whose_person_is_hidden_names_no_one_and_takes_no_pool_voice() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let dir = attributed(root);
            answer(&dir, &[(3, Some("pat"))]);
            fs::write(
                root.join("entities/pat/entity.json"),
                json!({"id":"pat","name":"Pat","type":"Person","blocked":true}).to_string(),
            )
            .unwrap();
            assert_eq!(
                page(root)["lines"][2],
                json!({"sentence_id":3,"speaker_actionable":false,"owner_line_label":marker(Some("pat"))})
            );
        }

        #[test]
        fn an_answer_shows_a_chip_on_a_segment_without_labels() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let dir = attributed(root);
            fs::remove_file(dir.join("talents/speaker_labels.json")).unwrap();
            assert_eq!(before_shape(page(root)), (unlabelled_before(), false));

            answer(&dir, &[(1, Some("pat"))]);
            let shown = page(root);
            assert_eq!(
                shown["speaker_labels"],
                json!({"present":false,"loaded":false,"source":"audio","ambiguous":false,"who_spoke_present":true})
            );
            assert_eq!(
                shown["lines"][0],
                json!({"sentence_id":1,"speaker_label":label("Pat","pat","user_labeled",false),"speaker_actionable":false,"owner_line_label":marker(Some("pat"))})
            );
            assert_eq!(
                shown["lines"].as_array().unwrap()[1..],
                unlabelled_before()["lines"].as_array().unwrap()[1..]
            );
        }

        #[test]
        fn unreadable_answers_name_no_one_and_warn_on_their_own() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let dir = attributed(root);
            fs::write(dir.join("talents/owner_line_labels.json"), "not a record").unwrap();
            let shown = page(root);
            assert_eq!(shown["warnings"], json!(["owner_line_labels"]));
            assert_eq!(shown["speaker_labels"]["who_spoke_present"], true);
            assert_eq!(
                shown["lines"],
                json!([
                    {"sentence_id":1,"speaker_actionable":false},
                    {"sentence_id":2,"speaker_actionable":false},
                    {"sentence_id":3,"speaker_actionable":false}
                ])
            );

            fs::remove_file(dir.join("talents/owner_line_labels.json")).unwrap();
            assert_eq!(before_shape(page(root)), (attributed_before(), true));
        }

        #[test]
        fn an_answer_for_a_changed_line_changes_no_chip() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let dir = attributed(root);
            answer(&dir, &[(1, Some("owner")), (2, Some("pat")), (3, None)]);
            let jsonl = fs::read_to_string(dir.join("audio.jsonl")).unwrap();
            fs::write(
                dir.join("audio.jsonl"),
                jsonl.replace("Let us begin", "Let us start"),
            )
            .unwrap();
            let answered = page(root);
            assert!(answered["lines"][0].get("owner_line_label").is_none());
            fs::remove_file(dir.join("talents/owner_line_labels.json")).unwrap();
            assert_eq!(page(root)["lines"][0], answered["lines"][0]);

            // On a segment without labels, an answer that applies nowhere
            // leaves the page as it was.
            fs::remove_file(dir.join("talents/speaker_labels.json")).unwrap();
            fs::write(dir.join("audio.jsonl"), &jsonl).unwrap();
            answer(&dir, &[(1, Some("pat"))]);
            fs::write(
                dir.join("audio.jsonl"),
                jsonl.replace("Let us begin", "Let us start"),
            )
            .unwrap();
            assert_eq!(before_shape(page(root)), (unlabelled_before(), false));
        }
    }

    #[test]
    fn imported_photo_segment_exposes_image_url_caption_description_and_capture_zone() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"timezone":"America/Denver","providers":{"active":{"provider":"openai"}}}"#,
        )
        .unwrap();
        let seg_dir = root.join("chronicle/20261112/import.image/083000_0");
        std::fs::create_dir_all(&seg_dir).unwrap();

        let mut jpeg = Vec::new();
        let pixels = image::ImageBuffer::from_pixel(16u32, 8u32, image::Rgb([100u8, 120, 140]));
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
            .encode_image(&image::DynamicImage::ImageRgb8(pixels))
            .unwrap();
        // Inject an APP1 EXIF Orientation=6 (90° CW) so upright dimensions swap to 8×16.
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II*\0");
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x0112u16.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&[6, 0, 0, 0]);
        tiff.extend_from_slice(&0u32.to_le_bytes());
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let len = u16::try_from(app1.len() + 2).unwrap();
        let mut rotated = jpeg[..2].to_vec();
        rotated.extend_from_slice(&[0xFF, 0xE1]);
        rotated.extend_from_slice(&len.to_be_bytes());
        rotated.extend_from_slice(&app1);
        rotated.extend_from_slice(&jpeg[2..]);
        std::fs::write(seg_dir.join("original.jpg"), &rotated).unwrap();

        std::fs::write(
            seg_dir.join("image_meta.json"),
            r#"{"schema":1,"time_source":"capture","utc_offset_seconds":32400,"caption":"morning ramen","original_filename":"IMG_4821.jpg"}"#,
        )
        .unwrap();
        std::fs::write(
            seg_dir.join("image_transcript.md"),
            "# IMG_4821\n\n**Type:** Image\n**Format:** JPEG\n**Dimensions:** 16×8\n**Date:** 2026-11-12\n\n---\n\n**Caption:**\nmorning ramen\n\n---\n\n> [image description — model-derived]\n> A steaming bowl of ramen on a wooden counter.\n",
        )
        .unwrap();

        let prepared_day =
            crate::day::prepare_day(root, "20261112", jiff::Timestamp::now()).unwrap();
        let day_seg = prepared_day
            .segments
            .iter()
            .find(|s| s.stream == "import.image" && s.key == "083000_0")
            .expect("import.image segment listed on day");
        assert_eq!(day_seg.types, vec!["image"]);
        assert_eq!(
            day_seg.data_state.get("image").map(String::as_str),
            Some("analyzed")
        );

        let value = super::prepare_segment(
            root,
            "20261112",
            "import.image",
            "083000_0",
            jiff::Timestamp::now(),
        )
        .unwrap();
        assert_eq!(
            value["image_files"]["original.jpg"],
            "/app/transcripts/api/serve_file/20261112/import.image/083000_0/original.jpg"
        );
        assert!(value["media_sizes"]["image"].as_u64().unwrap() > 0);
        assert_eq!(value["data_state"]["image"], "analyzed");
        assert_eq!(value["capture_zone"]["label"], "UTC+9");
        assert_eq!(value["capture_zone"]["differs"], true);
        let chunks = value["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["type"], "image");
        assert_eq!(chunks[0]["source_ref"]["raw"], "original.jpg");
        assert_eq!(chunks[0]["photo"]["width"], 8);
        assert_eq!(chunks[0]["photo"]["height"], 16);
        assert_eq!(chunks[0]["photo"]["caption"], "morning ramen");
        assert_eq!(
            chunks[0]["photo"]["description"],
            "A steaming bowl of ramen on a wooden counter."
        );
        assert_eq!(chunks[0]["photo"]["description_state"], "described");
        assert_eq!(chunks[0]["photo"]["time_source"], "capture");
    }

    #[test]
    fn imported_photo_without_thinking_or_capture_date_reports_fallback_and_no_processing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let seg_dir = root.join("chronicle/20261119/import.image/120000_0");
        std::fs::create_dir_all(&seg_dir).unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::ImageBuffer::from_pixel(
            6,
            4,
            image::Rgb([20, 30, 40]),
        ))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
        std::fs::write(seg_dir.join("original.png"), png.into_inner()).unwrap();
        std::fs::write(
            seg_dir.join("image_meta.json"),
            r#"{"schema":1,"time_source":"upload_fallback"}"#,
        )
        .unwrap();
        std::fs::write(
            seg_dir.join("image_transcript.md"),
            "# scan\n\n---\n\n> [image description — model-derived]\n> unavailable — No active provider configured\n",
        )
        .unwrap();

        let value = super::prepare_segment(
            root,
            "20261119",
            "import.image",
            "120000_0",
            jiff::Timestamp::now(),
        )
        .unwrap();
        let chunks = value["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["photo"]["description_state"], "no_processing");
        assert!(chunks[0]["photo"]["description"].is_null());
        assert_eq!(chunks[0]["photo"]["time_source"], "upload_fallback");
        assert_eq!(chunks[0]["photo"]["source_kind"], "import");
        assert_eq!(chunks[0]["photo"]["library_copy"], false);
    }

    #[test]
    fn library_photo_segment_reports_photos_source_device_pending_state_and_copy_flag() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let seg_dir = root.join("chronicle/20261119/photos/141200_0");
        std::fs::create_dir_all(&seg_dir).unwrap();
        let mut jpg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::ImageBuffer::from_pixel(
            512,
            384,
            image::Rgb([40, 80, 120]),
        ))
        .write_to(&mut jpg, image::ImageFormat::Jpeg)
        .unwrap();
        std::fs::write(seg_dir.join("original.jpg"), jpg.into_inner()).unwrap();
        std::fs::write(
            seg_dir.join("image_meta.json"),
            r#"{"schema":1,"time_source":"library_metadata","device_name":"Test Phone"}"#,
        )
        .unwrap();
        std::fs::write(
            seg_dir.join("image_transcript.md"),
            "# IMG_1001\n\n**Type:** Image\n**Format:** JPEG\n**Dimensions:** 512×384\n**Date:** 2026-11-19\n",
        )
        .unwrap();
        let obs = solstone_core_segment::photo_observation::PhotoObservation {
            schema: 1,
            observation_revision: 1,
            source: solstone_core_segment::photo_observation::PhotoObservationSource {
                day: "20261119".to_owned(),
                stream: "photos".to_owned(),
                segment: "141200_0".to_owned(),
                artifact_selector: "original.jpg".to_owned(),
                source_sha256: None,
                original_filename: Some("IMG_1001.HEIC".to_owned()),
                asset_id: Some("asset-1".to_owned()),
                import_id: None,
            },
            width: Some(512),
            height: Some(384),
            format: Some("jpeg".to_owned()),
            time: solstone_core_segment::photo_observation::PhotoObservationTime {
                captured_at: Some("2026-11-19T14:12:00-07:00".to_owned()),
                placed_at: "2026-11-19T14:12:00".to_owned(),
                utc_offset_seconds: Some(-25200),
                time_source: "capture".to_owned(),
                precision: solstone_core_segment::photo_observation::PhotoTimePrecision::Second,
                provenance:
                    solstone_core_segment::photo_observation::PhotoTimeProvenance::LibraryMetadata,
                tz: Some("America/Denver".to_owned()),
            },
            state: solstone_core_segment::photo_observation::PhotoObservationState::Pending,
            state_reason: None,
            retry_count: 0,
            owner_caption: None,
            description: None,
            visible_text: None,
            belonging_class: None,
            faces: Vec::new(),
            faces_checked: false,
            updated_at: "2026-11-19T21:12:05Z".to_owned(),
        };
        solstone_core_segment::photo_observation::write_photo_observation(&seg_dir, &obs).unwrap();

        let prepared_day =
            crate::day::prepare_day(root, "20261119", jiff::Timestamp::now()).unwrap();
        let day_seg = prepared_day
            .segments
            .iter()
            .find(|s| s.stream == "photos" && s.key == "141200_0")
            .expect("photos segment listed on day");
        assert_eq!(day_seg.types, vec!["image"]);
        assert_eq!(
            day_seg.data_state.get("image").map(String::as_str),
            Some("analyzed")
        );

        let value = super::prepare_segment(
            root,
            "20261119",
            "photos",
            "141200_0",
            jiff::Timestamp::now(),
        )
        .unwrap();
        let chunks = value["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["photo"]["source_kind"], "photos");
        assert_eq!(chunks[0]["photo"]["device_name"], "Test Phone");
        assert_eq!(chunks[0]["photo"]["library_copy"], true);
        assert_eq!(chunks[0]["photo"]["description_state"], "pending");
        assert_eq!(chunks[0]["photo"]["width"], 512);
        assert_eq!(chunks[0]["photo"]["height"], 384);
    }

    #[test]
    fn folder_segments_expose_day_summaries_and_structured_folder_activity_payload() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        solstone_core_facets::create_facet(root, "work", "Work", "", "#2E6F9E", "", None).unwrap();
        let stream = "laptop_folder_0123456789ab";
        let day = "20261127";

        // 1. Activity-only folder segment.
        let act_seg = root.join(format!("chronicle/{day}/{stream}/104000_300"));
        std::fs::create_dir_all(act_seg.join("talents")).unwrap();
        std::fs::write(act_seg.join("events.jsonl"), "").unwrap();
        std::fs::write(
            act_seg.join("talents/screen_text.md"),
            "## 02:00 · Folder: Downloads\n- Added installer\\.dmg\n",
        )
        .unwrap();
        std::fs::write(
            act_seg.join("talents/folder_activity.json"),
            json!({
                "v": 1,
                "folder_id": "dl-1",
                "folder_name": "Downloads",
                "owner_facet": "work",
                "events": [{
                    "key": "ev1",
                    "event": "added",
                    "path": "installer.dmg",
                    "at": "2026-11-27T10:42:00-07:00",
                    "at_ms": 1795714920000_i64,
                    "offset_ms": 120000,
                    "size": 58000000,
                    "origin_url": "https://mozilla.org/firefox?utm_source=test"
                }]
            })
            .to_string(),
        )
        .unwrap();

        // 2. Content-bearing folder segment with revised PDF + details-only rename.
        let doc_seg = root.join(format!("chronicle/{day}/{stream}/110000_300"));
        std::fs::create_dir_all(doc_seg.join("talents")).unwrap();
        std::fs::write(
            doc_seg.join("document_transcript.md"),
            "# report.pdf\n\nUpdated Q4 numbers.\n",
        )
        .unwrap();
        std::fs::write(
            doc_seg.join("talents/folder_activity.json"),
            json!({
                "v": 1,
                "folder_id": "dl-1",
                "folder_name": "Downloads",
                "events": [
                    {
                        "key": "ev2",
                        "event": "changed",
                        "path": "report.pdf",
                        "at": "2026-11-27T11:01:00-07:00",
                        "at_ms": 1795716060000_i64,
                        "offset_ms": 60000,
                        "file_type": "pdf",
                        "size": 420000,
                        "kind": "documents",
                        "landed_file": "document_transcript.md"
                    },
                    {
                        "key": "ev3",
                        "event": "renamed",
                        "path": "notes/new.txt",
                        "previous_path": "notes/old.txt",
                        "at": "2026-11-27T11:02:00-07:00",
                        "at_ms": 1795716120000_i64,
                        "offset_ms": 120000,
                        "size": 120
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();

        let prepared_day = crate::day::prepare_day(root, day, jiff::Timestamp::now()).unwrap();
        assert!(!prepared_day.screen.is_empty());
        let act_day = prepared_day
            .segments
            .iter()
            .find(|s| s.key == "104000_300")
            .expect("activity-only folder segment listed on day");
        assert_eq!(act_day.types, vec!["folder_activity"]);
        assert_eq!(act_day.think, None);
        let act_summary = act_day.folder.as_ref().expect("folder summary attached");
        assert!(act_summary.activity_only);
        assert_eq!(act_summary.file_count, 1);
        assert_eq!(act_summary.details_only_count, 1);
        assert_eq!(act_summary.facet.as_ref().unwrap().name, "work");

        let act_detail =
            super::prepare_segment(root, day, stream, "104000_300", jiff::Timestamp::now())
                .unwrap();
        let fa = &act_detail["folder_activity"];
        assert_eq!(fa["activity_only"], true);
        assert_eq!(fa["events"][0]["verb"], "added");
        assert_eq!(fa["events"][0]["origin_domain"], "mozilla.org");
        assert_eq!(fa["events"][0]["details_only"], true);
        assert_eq!(fa["inline_cards"].as_array().unwrap().len(), 0);

        let doc_detail =
            super::prepare_segment(root, day, stream, "110000_300", jiff::Timestamp::now())
                .unwrap();
        let dfa = &doc_detail["folder_activity"];
        assert_eq!(dfa["activity_only"], false);
        assert_eq!(dfa["events"][0]["verb"], "revised");
        assert_eq!(dfa["events"][1]["verb"], "renamed");
        assert_eq!(dfa["inline_cards"][0]["is_revision"], true);
    }
}
