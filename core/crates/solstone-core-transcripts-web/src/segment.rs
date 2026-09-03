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
use chrono::{DateTime, Local, TimeZone, Utc};
use serde::Serialize;
use serde_json::{Map, Value, json};
use solstone_core_format::content::{
    Family, RawPerceptFamily, produce_chunks, produce_raw_percept_chunks, talent_projection_map,
};
use solstone_core_format::segment::segment_parse;
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
}

pub(crate) async fn segment_content(
    State(state): State<Arc<AppState>>,
    RoutePath((day, stream, key)): RoutePath<(String, String, String)>,
) -> Response {
    let now = state.clock.now();
    match prepare_segment(&state.journal_root, &day, &stream, &key, now) {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
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
            "I couldn't use that day.",
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
    let dir = root.join("chronicle").join(day).join(stream).join(key);
    if !dir.is_dir() {
        return Err(invalid("Segment directory not found"));
    }
    let markdown_only = markdown_only(&dir, stream);
    let context = SegmentContext {
        day,
        stream,
        key,
        dir: &dir,
    };
    let mut media = discover(&dir, markdown_only);
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
                "I couldn't use that segment or stream.",
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
                Ok(entries) => browser_chunks(&mut chunks, name, &entries),
                Err(error) => warnings.push(warning("browser", path, error, now)),
            }
        }
    }
    if markdown_only {
        let time = wall_time(key, 0.0);
        let timestamp = day_timestamp(day, &time, 0);
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
    Ok(
        json!({"chunks":chunks,"audio_file":media.audio_file,"duration":duration,"video_files":media.video_files,"image_files":media.image_files,"md_files":md_files,"segment_key":key,"cost":read_cost(root, day, key),"media_sizes":media.media_sizes,"media_purged":{"audio":media.purged("audio"),"screen":media.purged("screen")},"data_state":data_state,"signals":signals(&dir),"transcripts_copy":copy_payload(),"speaker_labels":speakers.state,"warnings":warnings.len(),"warning_details":warnings}),
    )
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
    let text = entries
        .iter()
        .map(|entry| serde_json::to_string(entry).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let ordinals = statement_ordinals(&text);
    let rel = format!(
        "{}/{}/{}/{}",
        context.day, context.stream, context.key, name
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
        let mut chunk = json!({"type":"audio","time":start,"timestamp":produced.chunks.iter().find(|chunk| chunk.source.as_ref() == Some(row)).and_then(|chunk| chunk.occurrence_time_ms).map(|value| value.0).unwrap_or(0),"markdown":strip_speaker_prefix(&markdown, row.get("speaker")),"sentence_id":sid,"speaker_source":source,"has_embedding":sid.is_some_and(|id| ids.contains(&id)),"speaker_actionable":speakers.state.present && speakers.state.loaded && speakers.state.source.as_deref() == Some(source) && sid.is_some_and(|id| ids.contains(&id)),"source_ref":{"start":start,"source":row.get("source"),"speaker":row.get("speaker")}});
        if let Some(label) = sid
            .and_then(|id| speakers.labels.get(&id))
            .filter(|_| speakers.state.source.as_deref() == Some(source))
        {
            chunk
                .as_object_mut()
                .unwrap()
                .insert("speaker_label".into(), serde_json::to_value(label).unwrap());
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
        "{}/{}/{}/{}",
        context.day, context.stream, context.key, name
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
        chunks.push(json!({"type":"screen","time":time,"timestamp":day_timestamp(context.day, &time, chunk.occurrence_time_ms.map(|value| value.0).unwrap_or(0)),"markdown":chunk.content,"source_ref":{"frame_id":source.get("frame_id"),"filename":name,"raw":source_raw,"media_kind":kind,"monitor":monitor,"offset":source.get("timestamp"),"box_2d":source.get("box_2d"),"analysis":source.get("analysis"),"participants":if participants.is_empty(){Value::Null}else{json!(participants)}},"basic":source.get("analysis").is_none() && source.get("content").is_none_or(|value| value.is_null() || value.as_object().is_some_and(Map::is_empty))}));
    }
}

fn browser_chunks(chunks: &mut Vec<Value>, name: &str, entries: &[Map<String, Value>]) {
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
        chunks.push(json!({"type":"browser","time":local_time(timestamp),"timestamp":timestamp,"markdown":chunk.content,"source_ref":{"site":site,"title":title,"adapter":adapter,"site_name":site_name,"file":name,"op":source.get("op").or_else(|| source.get("t"))}}));
    }
}

fn signals(dir: &Path) -> Value {
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
        events.push(json!({"event_type":kind,"time":local_time(milliseconds),"timestamp":stamp,"timestamp_ms":milliseconds,"payload":payload}));
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
fn read_cost(root: &Path, day: &str, key: &str) -> f64 {
    fs::read_to_string(root.join("tokens").join(format!("{day}.jsonl")))
        .ok()
        .into_iter()
        .flat_map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .collect::<Vec<_>>()
        })
        .filter(|row| {
            row.get("segment").and_then(Value::as_str) == Some(key)
                && provider(row.get("model").and_then(Value::as_str).unwrap_or_default())
                    != "unknown"
        })
        .count() as f64
        * 0.0
}
fn provider(model: &str) -> &'static str {
    if model == "qwen3.5:9b"
        || model == "gemma-4-26b-a4b-it-mlx-4bit"
        || model.starts_with("local/")
    {
        "local"
    } else if model.starts_with("gpt") {
        "openai"
    } else if model.starts_with("gemini") {
        "google"
    } else if model.starts_with("claude") {
        "anthropic"
    } else {
        "unknown"
    }
}
fn copy_payload() -> Value {
    json!({"TR_SPEAKER_CHANGE_LABEL":"change speaker","TR_SPEAKER_ASSIGN_LABEL":"add speaker","TR_SPEAKER_PICKER_TITLE":"choose speaker","TR_SPEAKER_PICKER_SEARCH_PLACEHOLDER":"find a person","TR_SPEAKER_PICKER_OWNER":"this is me","TR_SPEAKER_PICKER_EMPTY":"no known voices yet","TR_SPEAKER_SOMEONE_ELSE":"someone else…","TR_SPEAKER_PICKER_NO_RESULTS":"no matching people","TR_SPEAKER_UNKNOWN_CHIP":"unknown voice","TR_SPEAKER_HEDGE_PROBABLE":"probably {name}","TR_SPEAKER_HEDGE_MAYBE":"maybe {name}?","TR_SPEAKER_CONFIDENCE_HIGH":"high confidence","TR_SPEAKER_CONFIDENCE_UNKNOWN":"confidence unavailable","TR_SPEAKER_MARGIN_OWNER":"close owner match","TR_SPEAKER_MARGIN_ACOUSTIC":"close voice match","TR_SPEAKER_ACTION_UNAVAILABLE":"speaker change unavailable","TR_SPEAKER_NO_EMBEDDING":"voice sample unavailable","TR_SPEAKER_CORRECT_RETRY":"retry speaker change","TR_SPEAKER_CORRECT_BUSY":"speaker files are busy","TR_SPEAKER_OWNER_TOO_CLOSE":"that voice is too close to yours to save there","TR_SPEAKER_OWNER_IDENTITY_REQUIRED":"set your identity before tagging yourself","TR_SPEAKER_ALREADY_CORRECT":"already set","TR_SPEAKER_PROPAGATION_OFFER":"{count} more statements may need this change","TR_SPEAKER_PROPAGATION_APPLY":"apply changes","TR_SPEAKER_PROPAGATION_DISMISS":"dismiss","TR_SPEAKER_PROPAGATION_APPLIED":"changes applied"})
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
fn day_timestamp(day: &str, time: &str, fallback: i64) -> i64 {
    chrono::NaiveDateTime::parse_from_str(&format!("{day} {time}"), "%Y%m%d %H:%M:%S")
        .ok()
        .and_then(|value| Local.from_local_datetime(&value).earliest())
        .map(|value| value.timestamp_millis())
        .unwrap_or(fallback)
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
fn local_time(milliseconds: i64) -> String {
    if milliseconds <= 0 {
        return String::new();
    }
    Local
        .timestamp_millis_opt(milliseconds)
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
    match speaker {
        Some(Value::Number(number)) => {
            let prefix = format!("Speaker {number}: ");
            markdown.strip_prefix(&prefix).unwrap_or(markdown).into()
        }
        Some(Value::String(speaker)) => markdown
            .strip_prefix(&format!("{speaker}: "))
            .unwrap_or(markdown)
            .into(),
        _ => markdown.into(),
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
        "I couldn't use that segment or stream.",
        detail,
        StatusCode::NOT_FOUND,
    )
}

#[cfg(test)]
mod tests {
    use chrono::{Local, NaiveDateTime, TimeZone};
    use serde_json::json;

    use super::{day_timestamp, local_time, timestamp};

    #[test]
    fn timestamps_accept_floats_and_non_positive_times_are_blank() {
        assert_eq!(timestamp(Some(&json!(1.5))), 1500);
        assert_eq!(timestamp(Some(&json!(10_000_000_001.5))), 10_000_000_001);
        assert_eq!(local_time(0), "");
        assert_eq!(local_time(-1), "");
    }

    #[test]
    fn day_timestamps_use_the_local_timezone() {
        let naive = NaiveDateTime::parse_from_str("20260731 09:00:00", "%Y%m%d %H:%M:%S").unwrap();
        let expected = Local
            .from_local_datetime(&naive)
            .earliest()
            .unwrap()
            .timestamp_millis();

        assert_eq!(day_timestamp("20260731", "09:00:00", 0), expected);
    }
}
