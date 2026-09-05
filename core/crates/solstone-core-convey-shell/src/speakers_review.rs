// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Segment speaker matching and the sentence-review read surface.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path as RoutePath, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_entity_matching::{
    EntityNameCandidate, entity_slug, find_matching_entity as match_entity_name,
};
use solstone_core_journal_io::SegmentLayout;
use solstone_core_speaker_resolve::audio_sample::audio_info;

use crate::JournalRoot;
use crate::speakers_calendar::{
    is_day, journal_principal_id, load_all_journal_entities, load_segment_speakers,
    load_speaker_labels, parse_segment, speaker_sentence_needs_review, value_truthy,
};
use crate::speakers_npz::{SegmentEmbeddings, load_segment_embeddings};
use solstone_core_speaker_resolve::segment_catalog::{
    DirectSupport, SegmentLookup, decode_stream_layout, lookup_segment,
};

#[derive(Debug, Default, Deserialize)]
pub struct StreamLayoutQuery {
    stream_layout: Option<String>,
}

pub async fn segment_speakers(
    Extension(root): Extension<Arc<JournalRoot>>,
    RoutePath((day, stream, segment_key)): RoutePath<(String, String, String)>,
    Query(query): Query<StreamLayoutQuery>,
) -> Response {
    if !is_day(&day) {
        return bad_request(
            "invalid_day",
            "I couldn't use that day.",
            "Invalid day format",
        );
    }
    let segment_dir = match lookup_read_segment(
        &root.0,
        &day,
        &stream,
        &segment_key,
        query.stream_layout.as_deref(),
    ) {
        Ok(Some(path)) => path,
        Ok(None) => {
            return Json(json!({"matched": [], "unmatched": []})).into_response();
        }
        Err(response) => return response,
    };

    let speakers = load_segment_speakers(&segment_dir);
    if speakers.is_empty() {
        return Json(json!({"matched": [], "unmatched": []})).into_response();
    }

    let entities = load_all_journal_entities(&root.0)
        .into_iter()
        .filter(|(_, entity)| !entity.get("blocked").is_some_and(value_truthy))
        .collect::<Vec<_>>();
    let mut matched = Vec::new();
    let mut unmatched = Vec::new();
    for speaker in speakers {
        if let Some(entity) = find_matching_entity(&speaker, &entities) {
            matched.push(json!({
                "detected_name": speaker,
                "entity_name": entity.get("name").cloned().unwrap_or(Value::Null),
                "entity_type": entity.get("type").cloned().unwrap_or(Value::Null),
            }));
        } else {
            unmatched.push(Value::String(speaker));
        }
    }
    Json(json!({"matched": matched, "unmatched": unmatched})).into_response()
}

pub async fn review(
    Extension(root): Extension<Arc<JournalRoot>>,
    RoutePath((day, stream, segment_key, source)): RoutePath<(String, String, String, String)>,
    Query(query): Query<StreamLayoutQuery>,
) -> Response {
    if !is_day(&day) {
        return bad_request(
            "invalid_day",
            "I couldn't use that day.",
            "Invalid day format",
        );
    }
    let layout = decode_stream_layout(query.stream_layout.as_deref());
    let segment_dir = match lookup_read_segment(
        &root.0,
        &day,
        &stream,
        &segment_key,
        query.stream_layout.as_deref(),
    ) {
        Ok(Some(path)) => path,
        Ok(None) => {
            return error_envelope(
                "speaker_review_unavailable",
                "I couldn't load that speaker review.",
                "No transcript found",
                StatusCode::NOT_FOUND,
            )
            .into_response();
        }
        Err(response) => return response,
    };
    let time_key = parsed_time_key(&segment_key).unwrap_or_else(|| segment_key.clone());
    let (sentences, embeddings) = load_sentences(&segment_dir, &time_key, &source);
    if sentences.is_empty() {
        return error_envelope(
            "speaker_review_unavailable",
            "I couldn't load that speaker review.",
            "No transcript found",
            StatusCode::NOT_FOUND,
        )
        .into_response();
    }

    let labels_data = load_speaker_labels(&segment_dir);
    let label_map = sentence_map(labels_data.as_ref(), "labels");
    let corrections_data = load_speaker_corrections(&segment_dir);
    let correction_map = sentence_map(corrections_data.as_ref(), "corrections");
    let duration_map = embeddings.as_ref().map(duration_map).unwrap_or_default();
    let principal_id = journal_principal_id(&root.0);
    let entities = load_all_journal_entities(&root.0);
    let entity_map = entities
        .iter()
        .map(|(entity_id, entity)| (entity_id.clone(), entity))
        .collect::<BTreeMap<_, _>>();
    let admitted_speaker_ids = entities
        .iter()
        .filter(|(_, entity)| is_admissible_speaker_entity(entity))
        .map(|(entity_id, _)| entity_id.clone())
        .collect::<BTreeSet<_>>();

    let mut review_sentences = sentences
        .into_iter()
        .filter(|sentence| sentence.get("has_embedding") == Some(&Value::Bool(true)))
        .collect::<Vec<_>>();
    let mut needs_review_count = 0;
    let mut corrections_count = 0;
    for sentence in &mut review_sentences {
        let sentence_id = sentence_id(sentence);
        let sentence_object = sentence
            .as_object_mut()
            .expect("review sentence is an object");
        sentence_object.insert(
            "duration_s".to_owned(),
            json!(duration_map.get(&sentence_id).copied().unwrap_or(0.0)),
        );

        let label = label_map.get(&sentence_id);
        if let Some(label) = label {
            add_label_fields(sentence_object, label, &entity_map, principal_id.as_deref());
        } else {
            add_empty_label_fields(sentence_object);
        }
        let needs_review = speaker_sentence_needs_review(
            label.copied(),
            labels_data.as_ref(),
            &admitted_speaker_ids,
        );
        sentence_object.insert("needs_review".to_owned(), json!(needs_review));

        let is_correction = sentence_object
            .get("method")
            .and_then(Value::as_str)
            .is_some_and(|method| matches!(method, "user_corrected" | "user_assigned"));
        sentence_object.insert("is_correction".to_owned(), json!(is_correction));
        if let Some(correction) = correction_map.get(&sentence_id).filter(|_| is_correction) {
            add_original_speaker_fields(
                sentence_object,
                correction,
                &entity_map,
                principal_id.as_deref(),
            );
            corrections_count += 1;
        } else {
            sentence_object.insert("original_speaker_entity_id".to_owned(), Value::Null);
            sentence_object.insert("original_speaker_name".to_owned(), Value::Null);
        }
        if needs_review {
            needs_review_count += 1;
        }
    }

    let mut all_entities = entities
        .iter()
        .filter(|(_, entity)| is_admissible_speaker_entity(entity))
        .map(|(entity_id, entity)| {
            json!({
                "entity_id": entity_id,
                "name": entity.get("name").cloned().unwrap_or_else(|| json!(entity_id)),
                "is_principal": entity.get("is_principal") == Some(&Value::Bool(true)),
            })
        })
        .collect::<Vec<_>>();
    if !entities
        .iter()
        .any(|(_, entity)| entity.get("is_principal") == Some(&Value::Bool(true)))
        && let Some((entity_id, name)) = configured_principal_identity(&root.0)
        && !entities.iter().any(|(id, _)| id == &entity_id)
    {
        all_entities.push(json!({
            "entity_id": entity_id,
            "name": name,
            "is_principal": true,
        }));
    }
    all_entities.sort_by(|left, right| {
        let left_principal = left["is_principal"].as_bool().unwrap_or(false);
        let right_principal = right["is_principal"].as_bool().unwrap_or(false);
        (
            !left_principal,
            left["name"].as_str().unwrap_or_default().to_lowercase(),
        )
            .cmp(&(
                !right_principal,
                right["name"].as_str().unwrap_or_default().to_lowercase(),
            ))
    });

    let (audio_file, audio_mimetype) = audio_info(
        &segment_dir,
        &day,
        &stream,
        &segment_key,
        &source,
        layout.unwrap_or(SegmentLayout::Named),
    );
    let (start, end) = parse_segment(&time_key)
        .map(|(start, end, _)| (start, end))
        .unwrap_or_default();
    Json(json!({
        "segment": {"key": segment_key, "time_key": time_key, "stream": stream, "stream_layout": layout_name(layout.unwrap_or(SegmentLayout::Named)), "start": start, "end": end},
        "source": source,
        "sentences": review_sentences,
        "all_entities": all_entities,
        "audio_file": audio_file,
        "audio_mimetype": audio_mimetype,
        "has_labels": labels_data.is_some(),
        "summary": {
            "total": review_sentences.len(),
            "needs_review": needs_review_count,
            "corrections": corrections_count,
        },
    }))
    .into_response()
}

fn bad_request(reason_code: &str, message: &str, detail: &str) -> Response {
    error_envelope(reason_code, message, detail, StatusCode::BAD_REQUEST).into_response()
}

#[allow(clippy::result_large_err)]
fn lookup_read_segment(
    root: &Path,
    day: &str,
    stream: &str,
    segment_key: &str,
    stream_layout: Option<&str>,
) -> Result<Option<PathBuf>, Response> {
    match lookup_segment(
        root,
        day,
        stream,
        segment_key,
        decode_stream_layout(stream_layout),
        DirectSupport::Allow,
    ) {
        SegmentLookup::Present(path) => Ok(Some(path)),
        SegmentLookup::Absent => Ok(None),
        SegmentLookup::MalformedLayout => Err(bad_request(
            "invalid_segment_or_stream",
            "I couldn't use that segment or stream.",
            "Invalid segment key",
        )),
        SegmentLookup::Failed(error) => Err(error_envelope(
            "speaker_review_unavailable",
            "I couldn't load that speaker review.",
            error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response()),
        SegmentLookup::UnsupportedLayout => Err(error_envelope(
            "speaker_review_unavailable",
            "I couldn't load that speaker review.",
            "segment layout is not readable",
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response()),
    }
}

fn load_sentences(
    segment_dir: &Path,
    segment_key: &str,
    source: &str,
) -> (Vec<Value>, Option<SegmentEmbeddings>) {
    let Ok(contents) = fs::read_to_string(segment_dir.join(format!("{source}.jsonl"))) else {
        return (Vec::new(), None);
    };
    let lines = contents.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return (Vec::new(), None);
    }
    let segment_start = segment_start_seconds(segment_key).unwrap_or(0);
    let mut sentences = Vec::new();
    for (index, line) in lines.into_iter().skip(1).enumerate() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let absolute_start = match entry.get("start") {
            Some(start) => match start.as_str().and_then(time_to_seconds) {
                Some(seconds) => seconds,
                None => continue,
            },
            None => 0,
        };
        let text = entry.get("text").cloned().unwrap_or_else(|| json!(""));
        sentences.push(json!({
            "id": index + 1,
            "offset": absolute_start - segment_start,
            "text": text,
        }));
    }
    let embeddings = load_segment_embeddings(&segment_dir.join(format!("{source}.npz")));
    if let Some(embeddings) = &embeddings {
        let statement_ids = embeddings
            .statement_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        for sentence in &mut sentences {
            let id = sentence_id(sentence);
            sentence
                .as_object_mut()
                .expect("review sentence is an object")
                .insert(
                    "has_embedding".to_owned(),
                    json!(statement_ids.contains(&id)),
                );
        }
    }
    (sentences, embeddings)
}

fn sentence_map<'a>(payload: Option<&'a Value>, key: &str) -> BTreeMap<i32, &'a Value> {
    payload
        .and_then(|payload| payload.get(key))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .get("sentence_id")
                .and_then(Value::as_i64)
                .and_then(|id| i32::try_from(id).ok())
                .map(|id| (id, entry))
        })
        .collect()
}

fn load_speaker_corrections(segment_dir: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(segment_dir.join("talents/speaker_corrections.json")).ok()?)
        .ok()
}

fn duration_map(embeddings: &SegmentEmbeddings) -> BTreeMap<i32, f32> {
    embeddings
        .statement_ids
        .iter()
        .enumerate()
        .map(|(index, statement_id)| {
            (
                *statement_id,
                embeddings
                    .durations_s
                    .as_ref()
                    .and_then(|durations| durations.get(index))
                    .copied()
                    .unwrap_or(0.0),
            )
        })
        .collect()
}

fn add_label_fields(
    sentence: &mut Map<String, Value>,
    label: &Value,
    entities: &BTreeMap<String, &Value>,
    principal_id: Option<&str>,
) {
    let entity_id = label.get("speaker").and_then(Value::as_str);
    if let Some(entity_id) = entity_id.filter(|entity_id| {
        !entity_id.is_empty()
            && entities
                .get(*entity_id)
                .is_some_and(|entity| is_admissible_speaker_entity(entity))
    }) {
        let (name, is_owner) = entity_display(entity_id, entities, principal_id);
        sentence.insert("speaker_entity_id".to_owned(), json!(entity_id));
        sentence.insert("speaker_name".to_owned(), json!(name));
        sentence.insert("is_owner".to_owned(), json!(is_owner));
    } else {
        sentence.insert("speaker_entity_id".to_owned(), Value::Null);
        sentence.insert("speaker_name".to_owned(), Value::Null);
        sentence.insert("is_owner".to_owned(), json!(false));
    }
    sentence.insert(
        "confidence".to_owned(),
        label.get("confidence").cloned().unwrap_or(Value::Null),
    );
    sentence.insert(
        "method".to_owned(),
        label.get("method").cloned().unwrap_or(Value::Null),
    );
}

fn add_empty_label_fields(sentence: &mut Map<String, Value>) {
    for key in ["speaker_entity_id", "speaker_name", "confidence", "method"] {
        sentence.insert(key.to_owned(), Value::Null);
    }
    sentence.insert("is_owner".to_owned(), json!(false));
}

fn add_original_speaker_fields(
    sentence: &mut Map<String, Value>,
    correction: &Value,
    entities: &BTreeMap<String, &Value>,
    principal_id: Option<&str>,
) {
    if let Some(entity_id) = correction
        .get("original_speaker")
        .and_then(Value::as_str)
        .filter(|entity_id| !entity_id.is_empty())
    {
        let (name, _) = entity_display(entity_id, entities, principal_id);
        sentence.insert("original_speaker_entity_id".to_owned(), json!(entity_id));
        sentence.insert("original_speaker_name".to_owned(), json!(name));
    } else {
        sentence.insert("original_speaker_entity_id".to_owned(), Value::Null);
        sentence.insert("original_speaker_name".to_owned(), Value::Null);
    }
}

fn entity_display(
    entity_id: &str,
    entities: &BTreeMap<String, &Value>,
    principal_id: Option<&str>,
) -> (String, bool) {
    let name = entities
        .get(entity_id)
        .and_then(|entity| entity.get("name"))
        .and_then(Value::as_str)
        .unwrap_or(entity_id)
        .to_owned();
    (name, principal_id == Some(entity_id))
}

fn configured_principal_identity(root: &Path) -> Option<(String, String)> {
    let config: Value =
        serde_json::from_slice(&fs::read(root.join("config/journal.json")).ok()?).ok()?;
    let identity = config.get("identity")?.as_object()?;
    let names = identity
        .get("preferred")
        .and_then(Value::as_str)
        .into_iter()
        .chain(identity.get("name").and_then(Value::as_str))
        .chain(
            identity
                .get("aliases")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        );
    for name in names {
        let name = name.trim();
        if !name.is_empty() {
            let entity_id = entity_slug(name);
            if !entity_id.is_empty() {
                return Some((entity_id, name.to_owned()));
            }
        }
    }
    None
}

fn sentence_id(sentence: &Value) -> i32 {
    sentence["id"]
        .as_i64()
        .and_then(|id| i32::try_from(id).ok())
        .expect("review sentence has an i32 ID")
}

fn segment_start_seconds(key: &str) -> Option<i64> {
    let time = key.split_once('_')?.0;
    Some(
        time[0..2].parse::<i64>().ok()? * 3_600
            + time[2..4].parse::<i64>().ok()? * 60
            + time[4..6].parse::<i64>().ok()?,
    )
}

fn parsed_time_key(name: &str) -> Option<String> {
    let (time, rest) = name.split_once('_')?;
    let duration = rest.split('_').next()?;
    (time.len() == 6
        && time.bytes().all(|byte| byte.is_ascii_digit())
        && !duration.is_empty()
        && duration.bytes().all(|byte| byte.is_ascii_digit()))
    .then(|| format!("{time}_{duration}"))
}

fn layout_name(layout: SegmentLayout) -> &'static str {
    match layout {
        SegmentLayout::Direct => "direct",
        SegmentLayout::Named => "named",
    }
}

fn time_to_seconds(time: &str) -> Option<i64> {
    let mut parts = time.split(':');
    let hours = parts.next()?.parse::<i64>().ok()?;
    let minutes = parts.next()?.parse::<i64>().ok()?;
    let seconds = parts.next()?.parse::<i64>().ok()?;
    parts
        .next()
        .is_none()
        .then_some(hours * 3_600 + minutes * 60 + seconds)
}

/// Resolve names through the shared Python-compatible eight-tier matcher,
/// including its rapidfuzz-based fuzzy tier.
pub(crate) fn find_matching_entity<'a>(
    detected_name: &str,
    entities: &'a [(String, Value)],
) -> Option<&'a Value> {
    let admitted_entities = entities
        .iter()
        .filter(|(_, entity)| is_admissible_speaker_entity(entity))
        .collect::<Vec<_>>();
    let candidates = admitted_entities
        .iter()
        .map(|(_, entity)| EntityNameCandidate {
            id: entity
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned),
            name: entity
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            aka: string_values(entity, "aka"),
            emails: string_values(entity, "emails"),
        })
        .collect::<Vec<_>>();
    let matched = match_entity_name(detected_name, &candidates, 90.0)?;
    let (matched_directory_id, matched_entity) = *admitted_entities.get(matched.candidate_index)?;
    let emitted_id = effective_entity_id(matched_directory_id, matched_entity);
    let requires_independent_resolution = matched_directory_id.as_str() != emitted_id;
    let mut resolved_entities = entities.iter().filter(|(directory_id, entity)| {
        effective_entity_id(directory_id, entity) == emitted_id
            && (!requires_independent_resolution || directory_id != matched_directory_id)
    });
    let (_, resolved_entity) = resolved_entities.next()?;
    if !is_admissible_speaker_entity(resolved_entity)
        || resolved_entities.any(|(_, entity)| !is_admissible_speaker_entity(entity))
    {
        return None;
    }
    Some(matched_entity)
}

fn effective_entity_id<'a>(directory_id: &'a str, entity: &'a Value) -> &'a str {
    entity
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or(directory_id)
}

/// Raw entity JSON admission for speaker read surfaces.
pub(crate) fn is_admissible_speaker_entity(entity: &Value) -> bool {
    entity.get("type").and_then(Value::as_str) == Some("Person")
        && !entity.get("blocked").is_some_and(value_truthy)
}

fn string_values(entity: &Value, field: &str) -> Vec<String> {
    entity
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{add_label_fields, find_matching_entity};

    #[test]
    fn matching_excludes_non_person_and_blocked_entities() {
        let entities = vec![
            (
                "tool".to_owned(),
                json!({"id":"tool","name":"Deploy Bot","type":"Tool"}),
            ),
            (
                "blocked".to_owned(),
                json!({"id":"blocked","name":"Blocked Ada","type":"Person","blocked":true}),
            ),
            (
                "person".to_owned(),
                json!({"id":"person","name":"Ada Lovelace","type":"Person"}),
            ),
        ];

        assert!(find_matching_entity("Deploy Bot", &entities).is_none());
        assert!(find_matching_entity("Blocked Ada", &entities).is_none());
        assert_eq!(
            find_matching_entity("Ada Lovelace", &entities)
                .and_then(|entity| entity.get("id"))
                .and_then(serde_json::Value::as_str),
            Some("person")
        );
    }

    #[test]
    fn matching_revalidates_mismatched_entity_ids() {
        let mismatched = vec![(
            "mismatched".to_owned(),
            json!({"id":"other","name":"Ada Lovelace","type":"Person"}),
        )];
        assert!(find_matching_entity("Ada Lovelace", &mismatched).is_none());

        let mismatched_with_tool_target = vec![
            (
                "mismatched".to_owned(),
                json!({"id":"other","name":"Ada Lovelace","type":"Person"}),
            ),
            (
                "other".to_owned(),
                json!({"id":"other","name":"Deploy Bot","type":"Tool"}),
            ),
        ];
        assert!(find_matching_entity("Ada Lovelace", &mismatched_with_tool_target).is_none());

        let matching_id = vec![(
            "person".to_owned(),
            json!({"id":"person","name":"Ada Lovelace","type":"Person"}),
        )];
        assert!(find_matching_entity("Ada Lovelace", &matching_id).is_some());

        let directory_id_fallback = vec![(
            "person".to_owned(),
            json!({"name":"Ada Lovelace","type":"Person"}),
        )];
        assert!(find_matching_entity("Ada Lovelace", &directory_id_fallback).is_some());
    }

    #[test]
    fn active_label_fields_exclude_ineligible_speakers() {
        let tool = json!({"id":"tool","name":"Deploy Bot","type":"Tool"});
        let person = json!({"id":"person","name":"Ada Lovelace","type":"Person"});
        let entities = BTreeMap::from([("tool".to_owned(), &tool), ("person".to_owned(), &person)]);

        let mut ineligible = serde_json::Map::new();
        add_label_fields(
            &mut ineligible,
            &json!({"speaker":"tool","confidence":"high","method":"legacy"}),
            &entities,
            None,
        );
        assert_eq!(ineligible["speaker_entity_id"], serde_json::Value::Null);
        assert_eq!(ineligible["speaker_name"], serde_json::Value::Null);

        let mut admitted = serde_json::Map::new();
        add_label_fields(
            &mut admitted,
            &json!({"speaker":"person","confidence":"high"}),
            &entities,
            None,
        );
        assert_eq!(admitted["speaker_entity_id"], "person");
        assert_eq!(admitted["speaker_name"], "Ada Lovelace");
    }
}
