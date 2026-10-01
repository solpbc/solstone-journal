// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as RoutePath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use serde_json::{Map, Value, json};
use solstone_core_format::content::{
    Family, RawPerceptFamily, produce_chunks, produce_raw_percept_chunks, talent_projection_map,
};
use solstone_core_format::segment::segment_parse;
use solstone_core_processing_record::vocab;
use solstone_core_system_health::{DataState, derive_modality_state};

use crate::day::valid_day;
use crate::segment_media::{SegmentMedia, discover, markdown_files, markdown_only};
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
    zone: Tz,
}

pub(crate) async fn segment_content(
    State(state): State<Arc<AppState>>,
    RoutePath((day, stream, key)): RoutePath<(String, String, String)>,
) -> Response {
    let now = state.clock.now();
    let journal_root = state.journal_root.clone();
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::TranscriptsSegment,
        move || match prepare_segment(&journal_root, &day, &stream, &key, now) {
            Ok(value) => Json(value).into_response(),
            Err(response) => response,
        },
    )
    .await
}

#[allow(clippy::result_large_err)]
fn prepare_segment(
    root: &Path,
    day: &str,
    stream: &str,
    key: &str,
    now: DateTime<Utc>,
) -> Result<Value, Response> {
    if !valid_day(day) {
        return Err(legacy_error_response(
            "invalid_day",
            "that day couldn't be used.",
            "Invalid day format",
            StatusCode::NOT_FOUND,
        ));
    }
    if !valid_stream(stream) {
        return Err(invalid("Invalid stream format"));
    }
    if !valid_key(key) {
        return Err(invalid("Invalid segment key format"));
    }
    let dir = root
        .join("chronicle")
        .join(crate::segment_media::segment_rel(day, stream, key));
    if !dir.is_dir() {
        return Err(invalid("Segment directory not found"));
    }
    let markdown_only = markdown_only(&dir, stream);
    let zone = solstone_core_journal_config::owner_zone(root);
    let context = SegmentContext {
        day,
        stream,
        key,
        dir: &dir,
        root,
        zone,
    };
    let unclaimed_images = solstone_core_system_health::unclaimed_image_state(&dir, stream, now)
        .map_err(|error| {
            legacy_error_response(
                "invalid_segment_or_stream",
                "that segment or stream couldn't be used.",
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
    let mut speakers = load(&dir, root, now);
    let mut warnings = std::mem::take(&mut speakers.warnings);
    let mut chunks = Vec::<Value>::new();
    let mut has_jsonl = BTreeMap::from([("audio".to_owned(), false), ("screen".to_owned(), false)]);
    let mut records: BTreeMap<String, Option<Value>> =
        BTreeMap::from([("audio".to_owned(), None), ("screen".to_owned(), None)]);
    let mut duration = 0.0_f64;
    let mut markdown_added = false;
    let mut files = fs::read_dir(&dir)
        .map_err(|error| {
            legacy_error_response(
                "invalid_segment_or_stream",
                "that segment or stream couldn't be used.",
                error.to_string(),
                StatusCode::NOT_FOUND,
            )
        })?
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    files.sort();
    for path in &files {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.ends_with("audio.jsonl") {
            has_jsonl.insert("audio".into(), true);
            match read_entries(path) {
                Ok(entries) => {
                    if records["audio"].is_none() {
                        records.insert("audio".into(), processing_record(&entries));
                    }
                    duration = duration.max(audio_duration(&entries, key));
                    audio_chunks(&mut chunks, &mut media, &speakers, &context, name, &entries);
                }
                Err(error) => warnings.push(warning("audio", path, error, now)),
            }
        } else if name.ends_with("screen.jsonl") {
            has_jsonl.insert("screen".into(), true);
            match read_entries(path) {
                Ok(entries) => {
                    if records["screen"].is_none() {
                        records.insert("screen".into(), processing_record(&entries));
                    }
                    screen_chunks(&mut chunks, &mut media, &context, name, &entries);
                }
                Err(error) => warnings.push(warning("screen", path, error, now)),
            }
        } else if name.starts_with("browser_") && name.ends_with(".jsonl") {
            match read_entries(path) {
                Ok(entries) => browser_chunks(&mut chunks, zone, name, &entries),
                Err(error) => warnings.push(warning("browser", path, error, now)),
            }
        }
    }
    for raw_path in &unclaimed_images.raw_paths {
        let Some(raw_name) = raw_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let jsonl_path = raw_path.with_extension("jsonl");
        if jsonl_path.is_file() {
            match read_entries(&jsonl_path) {
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
                            if let Some(text) = entry.get("text").and_then(Value::as_str) {
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
                                let timestamp = day_timestamp(zone, day, &time, 0);
                                let mut source_ref = json!({
                                    "raw": raw_name,
                                    "media_kind": "image",
                                });
                                if let Some(err) =
                                    entry.get("detection_error").filter(|v| v.is_object())
                                {
                                    source_ref["detection_error"] = err.clone();
                                }
                                chunks.push(json!({
                                    "type": "image",
                                    "time": time,
                                    "timestamp": timestamp,
                                    "markdown": text,
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
    if markdown_only {
        let time = wall_time(key, 0.0);
        let timestamp = day_timestamp(zone, day, &time, 0);
        for path in markdown_files(&dir) {
            match fs::read_to_string(&path) {
                Ok(markdown) if !markdown.trim().is_empty() => {
                    chunks.push(json!({"type":"markdown","time":time,"timestamp":timestamp,"markdown":markdown.trim(),"source_ref":{"filename":path.file_name().and_then(|name| name.to_str()).unwrap_or_default()}}));
                    markdown_added = true;
                }
                Ok(_) => {}
                Err(error) => warnings.push(warning("markdown", &path, error, now)),
            }
        }
    }
    chunks.sort_by_key(|chunk| chunk.get("timestamp").and_then(Value::as_i64).unwrap_or(0));
    let warning_types = warnings
        .iter()
        .filter_map(|warning| {
            matches!(warning.kind.as_str(), "audio" | "screen").then_some(warning.kind.as_str())
        })
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
    if markdown_added {
        data_state.insert("markdown".into(), DataState::Analyzed.as_str().into());
    }
    if chunks
        .iter()
        .any(|chunk| chunk.get("type").and_then(Value::as_str) == Some("browser"))
    {
        data_state.insert("browser".into(), DataState::Analyzed.as_str().into());
    }
    let mut md_files = talent_projection_map(&dir.join("talents"), "").unwrap_or_default();
    if data_state.contains_key("audio") {
        md_files.remove("audio");
    }
    if data_state.contains_key("screen") {
        md_files.remove("screen");
    }
    let reason_code = extract_modality_reason_codes(&records);
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
            solstone_core_callosum::read_reported_zone(&dir),
            &zone,
        ),
        "media_sizes": media.media_sizes,
        "media_purged": {
            "audio": media.purged("audio"),
            "screen": media.purged("screen"),
        },
        "media_removal": media.media_removal(&dir),
        "data_state": data_state,
        "signals": signals(&dir, zone),
        "transcripts_copy": copy_payload(),
        "speaker_labels": speakers.state,
        "warnings": warnings.len(),
        "warning_details": warnings,
    });
    if let Some(reason_code) = reason_code
        && let Some(obj) = payload.as_object_mut()
    {
        obj.insert("reason_code".to_owned(), Value::Object(reason_code));
    }
    Ok(payload)
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

fn audio_chunks(
    chunks: &mut Vec<Value>,
    media: &mut SegmentMedia,
    speakers: &crate::segment_speakers::SpeakerJoin,
    context: &SegmentContext<'_>,
    name: &str,
    entries: &[Map<String, Value>],
) {
    let source = name.trim_end_matches(".jsonl");
    let ids = embedding_ids(context.dir, source);
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
    if let Some(raw) = entries
        .iter()
        .find(|entry| !entry.contains_key("start") && entry.contains_key("raw"))
        .and_then(|entry| entry.get("raw"))
        .and_then(Value::as_str)
    {
        media.register_audio(context.day, context.stream, context.key, context.dir, raw);
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
        let mut chunk = json!({"type":"audio","time":start,"timestamp":produced.chunks.iter().find(|chunk| chunk.source.as_ref() == Some(row)).and_then(|chunk| chunk.occurrence_time_ms).map_or(0, |value| local_wall_instant(context.zone, value.0)),"markdown":strip_speaker_prefix(&markdown, row.get("speaker")),"sentence_id":sid,"speaker_source":source,"has_embedding":sid.is_some_and(|id| ids.contains(&id)),"speaker_actionable":speakers.state.present && speakers.state.loaded && speakers.state.source.as_deref() == Some(source) && sid.is_some_and(|id| ids.contains(&id)),"source_ref":{"start":start,"source":row.get("source"),"speaker":row.get("speaker")}});
        if let Some(label) = sid
            .and_then(|id| speakers.labels.get(&id))
            .filter(|_| speakers.state.source.as_deref() == Some(source))
        {
            chunk
                .as_object_mut()
                .unwrap()
                .insert("speaker_label".into(), serde_json::to_value(label).unwrap());
        } else if let Some(voice) = sid.and_then(|id| voices.get(&id)) {
            chunk
                .as_object_mut()
                .unwrap()
                .insert("speaker_voice".into(), serde_json::to_value(voice).unwrap());
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
    let mut enriched = entries.to_vec();
    for entry in &mut enriched {
        if !entry.contains_key("timestamp") {
            continue;
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
    let produced = produce_raw_percept_chunks(RawPerceptFamily::RawScreen, &rel, &text);
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
        chunks.push(json!({"type":"screen","time":time,"timestamp":day_timestamp(context.zone, context.day, &time, chunk.occurrence_time_ms.map(|value| value.0).unwrap_or(0)),"markdown":chunk.content,"source_ref":{"frame_id":source.get("frame_id"),"filename":name,"raw":source_raw,"media_kind":kind,"monitor":monitor,"offset":source.get("timestamp"),"box_2d":source.get("box_2d"),"analysis":source.get("analysis"),"participants":if participants.is_empty(){Value::Null}else{json!(participants)}},"basic":source.get("analysis").is_none() && source.get("content").is_none_or(|value| value.is_null() || value.as_object().is_some_and(Map::is_empty))}));
    }
}

fn browser_chunks(chunks: &mut Vec<Value>, zone: Tz, name: &str, entries: &[Map<String, Value>]) {
    let text = entries
        .iter()
        .map(|entry| serde_json::to_string(entry).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let produced = produce_chunks(Family::Browser, name, &text);
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

fn signals(dir: &Path, zone: Tz) -> Value {
    let path = dir.join("signals.jsonl");
    let empty = || json!({"events":[],"counts":{},"calendar":{"total":0,"unique":0,"events":[]}});
    let Ok(entries) = read_entries(&path) else {
        return empty();
    };
    let mut events = Vec::new();
    let mut counts = BTreeMap::<String, usize>::new();
    let mut calendar = HashMap::<String, Value>::new();
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
            let item = calendar.entry(identity).or_insert_with(|| json!({"title":payload.get("title").and_then(Value::as_str).filter(|value|!value.is_empty()).unwrap_or("Untitled event"),"dtStart":payload.get("dtStart").and_then(Value::as_str).unwrap_or_default(),"dtEnd":payload.get("dtEnd").and_then(Value::as_str).unwrap_or_default(),"timezone":payload.get("timezone").and_then(Value::as_str).unwrap_or_default(),"eventId":payload.get("eventId").and_then(Value::as_str).unwrap_or_default(),"seen_count":0,"first_seen":stamp,"last_seen":stamp}));
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
    calendar.sort_by(|left, right| {
        left["dtStart"]
            .as_str()
            .cmp(&right["dtStart"].as_str())
            .then(left["title"].as_str().cmp(&right["title"].as_str()))
    });
    json!({"events":events,"counts":counts,"calendar":{"total":counts.get("calendar_event").copied().unwrap_or(0),"unique":calendar.len(),"events":calendar}})
}

fn read_entries(path: &Path) -> Result<Vec<Map<String, Value>>, std::io::Error> {
    let text = fs::read_to_string(path)?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).map_err(std::io::Error::other))
        .map(|value| {
            value.and_then(|value| {
                value
                    .as_object()
                    .cloned()
                    .ok_or_else(|| std::io::Error::other("JSONL row is not an object"))
            })
        })
        .collect()
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

fn copy_payload() -> Value {
    json!({"TR_SPEAKER_CHANGE_LABEL":"change speaker","TR_SPEAKER_ASSIGN_LABEL":"add speaker","TR_SPEAKER_PICKER_TITLE":"choose speaker","TR_SPEAKER_PICKER_SEARCH_PLACEHOLDER":"find a person","TR_SPEAKER_PICKER_OWNER":"this is me","TR_SPEAKER_PICKER_EMPTY":"no known voices yet","TR_SPEAKER_SOMEONE_ELSE":"someone else…","TR_SPEAKER_PICKER_NO_RESULTS":"no matching people","TR_SPEAKER_UNKNOWN_CHIP":"unknown voice","TR_SPEAKER_VOICE_CHIP":"voice {number}","TR_SPEAKER_VOICE_ASSISTIVE":"one unnamed voice wherever this number appears","TR_SPEAKER_VOICE_NAMED_ASSISTIVE":"matches a voice you named","TR_SPEAKER_VOICE_SCOPE":"everywhere this voice appears","TR_SPEAKER_VOICE_SCOPE_NOTE":"there's no undo","TR_SPEAKER_VOICE_NAMED_COUNT":"{count} sentences now show this name","TR_SPEAKER_VOICE_NAMED_ONE":"1 sentence now shows this name","TR_SPEAKER_HEDGE_PROBABLE":"probably {name}","TR_SPEAKER_HEDGE_MAYBE":"maybe {name}?","TR_SPEAKER_CONFIDENCE_HIGH":"high confidence","TR_SPEAKER_CONFIDENCE_UNKNOWN":"confidence unavailable","TR_SPEAKER_MARGIN_OWNER":"close owner match","TR_SPEAKER_MARGIN_ACOUSTIC":"close voice match","TR_SPEAKER_ACTION_UNAVAILABLE":"speaker change unavailable","TR_SPEAKER_NO_EMBEDDING":"voice sample unavailable","TR_SPEAKER_CORRECT_RETRY":"retry speaker change","TR_SPEAKER_CORRECT_BUSY":"speaker files are busy","TR_SPEAKER_OWNER_TOO_CLOSE":"that voice is too close to yours to save there","TR_SPEAKER_OWNER_IDENTITY_REQUIRED":"set your identity before tagging yourself","TR_SPEAKER_ALREADY_CORRECT":"already set","TR_SPEAKER_PROPAGATION_OFFER":"{count} more sentences may need this change","TR_SPEAKER_PROPAGATION_APPLY":"apply changes","TR_SPEAKER_PROPAGATION_DISMISS":"dismiss","TR_SPEAKER_PROPAGATION_APPLIED":"changes applied"})
}
fn warning(
    kind: &str,
    path: &Path,
    error: impl std::fmt::Display,
    now: DateTime<Utc>,
) -> WarningDetail {
    WarningDetail {
        kind: kind.into(),
        file: path.display().to_string(),
        message: error.to_string(),
        ts: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
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
fn day_timestamp(zone: Tz, day: &str, time: &str, fallback: i64) -> i64 {
    chrono::NaiveDateTime::parse_from_str(&format!("{day} {time}"), "%Y%m%d %H:%M:%S")
        .ok()
        .and_then(|value| zone.from_local_datetime(&value).earliest())
        .map(|value| value.timestamp_millis())
        .unwrap_or(fallback)
}
/// The instant of a formatter occurrence time. The content formatter encodes a
/// segment's local wall time as if it were UTC; screen and image chunks here
/// are anchored in the owner zone, so audio is re-anchored the same way to
/// merge and sort on one clock.
fn local_wall_instant(zone: Tz, wall_ms: i64) -> i64 {
    chrono::DateTime::from_timestamp_millis(wall_ms)
        .and_then(|wall| zone.from_local_datetime(&wall.naive_utc()).earliest())
        .map_or(wall_ms, |local| local.timestamp_millis())
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
        Some(Value::String(value)) => chrono::DateTime::parse_from_rfc3339(value)
            .map(|value| value.timestamp_millis())
            .unwrap_or(0),
        _ => 0,
    }
}
fn local_time(zone: Tz, milliseconds: i64) -> String {
    if milliseconds <= 0 {
        return String::new();
    }
    zone.timestamp_millis_opt(milliseconds)
        .single()
        .map(|value| value.format("%H:%M:%S").to_string())
        .unwrap_or_default()
}
fn participants(content: Option<&Value>) -> Vec<Value> {
    content.and_then(|value| value.get("meeting")).and_then(Value::as_object).and_then(|meeting| meeting.get("participants")).and_then(Value::as_array).into_iter().flatten().filter_map(|participant| { let box_2d = participant.get("box_2d").and_then(Value::as_array)?; (participant.get("video").and_then(Value::as_bool) == Some(true) && box_2d.len() == 4).then(|| json!({"name":participant.get("name").and_then(Value::as_str).unwrap_or("Unknown"),"status":participant.get("status").and_then(Value::as_str).unwrap_or("unknown"),"top":box_2d[0].as_f64().unwrap_or(0.0)/10.0,"left":box_2d[1].as_f64().unwrap_or(0.0)/10.0,"height":(box_2d[2].as_f64().unwrap_or(0.0)-box_2d[0].as_f64().unwrap_or(0.0))/10.0,"width":(box_2d[3].as_f64().unwrap_or(0.0)-box_2d[1].as_f64().unwrap_or(0.0))/10.0})) }).collect()
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
fn valid_stream(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
fn valid_key(value: &str) -> bool {
    let Some((time, length)) = value.split_once('_') else {
        return false;
    };
    time.len() == 6
        && time.bytes().all(|byte| byte.is_ascii_digit())
        && !length.is_empty()
        && length.bytes().all(|byte| byte.is_ascii_digit())
}
fn invalid(detail: &str) -> Response {
    legacy_error_response(
        "invalid_segment_or_stream",
        "that segment or stream couldn't be used.",
        detail,
        StatusCode::NOT_FOUND,
    )
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDateTime, TimeZone, Utc};
    use chrono_tz::Tz;
    use serde_json::json;
    use solstone_core_processing_record::vocab;

    use super::{day_timestamp, local_time, local_wall_instant, strip_speaker_prefix, timestamp};

    #[test]
    fn timestamps_accept_floats_and_non_positive_times_are_blank() {
        assert_eq!(timestamp(Some(&json!(1.5))), 1500);
        assert_eq!(timestamp(Some(&json!(10_000_000_001.5))), 10_000_000_001);
        assert_eq!(local_time(Tz::UTC, 0), "");
        assert_eq!(local_time(Tz::UTC, -1), "");
    }

    #[test]
    fn audio_wall_times_land_on_the_same_clock_as_screen_times() {
        let wall = NaiveDateTime::parse_from_str("20260731 09:00:05", "%Y%m%d %H:%M:%S").unwrap();
        let denver = Tz::America__Denver;
        let screen = day_timestamp(denver, "20260731", "09:00:05", 0);
        // 09:00:05 in Denver (UTC-6 in July) is 15:00:05 UTC.
        assert_eq!(screen, wall.and_utc().timestamp_millis() + 6 * 3_600_000);
        assert_eq!(
            local_wall_instant(denver, wall.and_utc().timestamp_millis()),
            screen
        );
    }

    #[test]
    fn day_timestamps_use_the_owner_zone_at_that_days_offset() {
        let denver = Tz::America__Denver;
        for (day, hours) in [("20260731", 6), ("20260115", 7)] {
            let naive =
                NaiveDateTime::parse_from_str(&format!("{day} 09:00:00"), "%Y%m%d %H:%M:%S")
                    .unwrap();
            assert_eq!(
                day_timestamp(denver, day, "09:00:00", 0),
                naive.and_utc().timestamp_millis() + hours * 3_600_000,
                "{day}"
            );
        }
        let instant = Utc.with_ymd_and_hms(2026, 7, 31, 15, 0, 5).unwrap();
        assert_eq!(local_time(denver, instant.timestamp_millis()), "09:00:05");
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
            chrono::Utc::now(),
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
            chrono::Utc::now(),
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
            chrono::Utc::now(),
        )
        .unwrap();

        assert_eq!(value.get("reason_code"), None);
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

        let value =
            super::prepare_segment(root, "20260101", "field", "120000_60", chrono::Utc::now())
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

        let value =
            super::prepare_segment(root, "20260929", "phone", "211400_300", chrono::Utc::now())
                .unwrap();

        assert_eq!(value["capture_zone"]["tz"], "Asia/Tokyo");
        assert_eq!(value["capture_zone"]["utc_offset_seconds"], 32400);
        assert_eq!(value["capture_zone"]["label"], "Tokyo");
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

        let value =
            super::prepare_segment(root, "20260101", "field", "120000_60", chrono::Utc::now())
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

        let value =
            super::prepare_segment(root, "20260101", "field", "120000_60", chrono::Utc::now())
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

        let value =
            super::prepare_segment(root, "20260101", "field", "120000_60", chrono::Utc::now())
                .unwrap();

        assert_eq!(value["media_purged"]["audio"], true);
        assert_eq!(
            value["media_removal"]["audio"],
            "this segment's original audio is no longer in your journal"
        );
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

        let value =
            super::prepare_segment(root, "20260101", "field", "120000_60", chrono::Utc::now())
                .unwrap();

        assert_eq!(value["media_purged"]["audio"], true);
        assert_eq!(
            value["media_removal"]["audio"],
            "this segment's original audio is no longer in your journal"
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

        let value =
            super::prepare_segment(root, "20260101", "field", "120000_60", chrono::Utc::now())
                .unwrap();

        assert_eq!(value["media_purged"]["audio"], true);
        assert_eq!(
            value["media_removal"]["audio"],
            "this segment's original audio is no longer in your journal"
        );
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

        let value =
            super::prepare_segment(root, "20260101", "field", "120000_60", chrono::Utc::now())
                .unwrap();

        assert_eq!(value["media_purged"]["audio"], true);
        assert_eq!(
            value["media_removal"]["audio"],
            "this segment's original audio is no longer in your journal"
        );
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

        let value =
            super::prepare_segment(root, "20260101", "field", "120000_60", chrono::Utc::now())
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

        let value =
            super::prepare_segment(root, "20260101", "field", "120000_60", chrono::Utc::now())
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

        let value =
            super::prepare_segment(root, "20260101", "field", "120000_60", chrono::Utc::now())
                .unwrap();

        assert_eq!(value["media_purged"]["audio"], true);
        assert_eq!(value["media_purged"]["screen"], true);
        assert_eq!(
            value["media_removal"]["audio"],
            "you deleted this segment's original audio after your retention settings marked it"
        );
        assert_eq!(
            value["media_removal"]["screen"],
            "this segment's original screen media is no longer in your journal"
        );
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

        let value =
            super::prepare_segment(root, "20260101", "field", "120000_60", chrono::Utc::now())
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

        let now = Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap();
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

        let now = Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap();
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

        let now = Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap();
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

        let now = Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap();
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

        let now = Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap();
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

        let now = Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap();
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

        let now = Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap();
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
    fn prepare_segment_twin_fixture_negative_differential() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/convey_records_journal");
        let now = Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap();
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
        let segment_dir = root.join("chronicle/20260101/field/120000_60");
        std::fs::create_dir_all(&segment_dir).unwrap();
        let image = segment_dir.join("photo.png");
        std::fs::write(&image, STILL_PNG).unwrap();
        let error =
            solstone_core_depict::run_with_clients(&image, false, &RefusingWire, &SilentDetector);
        assert!(error.is_err());
        assert_eq!(std::fs::read(&image).unwrap(), STILL_PNG);

        let now = Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap();
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
}
