// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Segment speaker matching and the sentence-review read surface.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path as RoutePath};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::error_envelope;

use crate::JournalRoot;
use crate::speakers_calendar::{
    is_day, is_segment_key, journal_principal_id, load_all_journal_entities, load_segment_speakers,
    load_speaker_labels, parse_segment, speaker_sentence_needs_review, value_truthy,
};
use crate::speakers_npz::{SegmentEmbeddings, load_segment_embeddings};

const AUDIO_FORMATS: [(&str, &str); 6] = [
    (".flac", "audio/flac"),
    (".opus", "audio/opus"),
    (".ogg", "audio/ogg"),
    (".m4a", "audio/mp4"),
    (".mp3", "audio/mpeg"),
    (".wav", "audio/wav"),
];

pub async fn segment_speakers(
    Extension(root): Extension<Arc<JournalRoot>>,
    RoutePath((day, stream, segment_key)): RoutePath<(String, String, String)>,
) -> Response {
    if !is_day(&day) {
        return bad_request(
            "invalid_day",
            "I couldn't use that day.",
            "Invalid day format",
        );
    }
    if !is_segment_key(&segment_key) {
        return bad_request(
            "invalid_segment_or_stream",
            "I couldn't use that segment or stream.",
            "Invalid segment key",
        );
    }

    let speakers = load_segment_speakers(&segment_path(&root.0, &day, &stream, &segment_key));
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
) -> Response {
    if !is_day(&day) {
        return bad_request(
            "invalid_day",
            "I couldn't use that day.",
            "Invalid day format",
        );
    }
    if !is_segment_key(&segment_key) {
        return bad_request(
            "invalid_segment_or_stream",
            "I couldn't use that segment or stream.",
            "Invalid segment key",
        );
    }

    let segment_dir = segment_path(&root.0, &day, &stream, &segment_key);
    let (sentences, embeddings) = load_sentences(&segment_dir, &segment_key, &source);
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
        let needs_review = speaker_sentence_needs_review(label.copied(), labels_data.as_ref());
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
        .filter(|(_, entity)| !entity.get("blocked").is_some_and(value_truthy))
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

    let (audio_file, audio_mimetype) =
        audio_info(&segment_dir, &day, &stream, &segment_key, &source);
    let (start, end) = parse_segment(&segment_key)
        .map(|(start, end, _)| (start, end))
        .unwrap_or_default();
    Json(json!({
        "segment": {"key": segment_key, "start": start, "end": end},
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

fn segment_path(root: &Path, day: &str, stream: &str, segment_key: &str) -> PathBuf {
    root.join("chronicle")
        .join(day)
        .join(stream)
        .join(segment_key)
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
        let Some(absolute_start) = entry
            .get("start")
            .and_then(Value::as_str)
            .and_then(time_to_seconds)
        else {
            continue;
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
    if let Some(entity_id) = entity_id.filter(|entity_id| !entity_id.is_empty()) {
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

fn audio_info(
    segment_dir: &Path,
    day: &str,
    stream: &str,
    segment_key: &str,
    source: &str,
) -> (Option<String>, Option<String>) {
    for (extension, mimetype) in AUDIO_FORMATS {
        let filename = format!("{source}{extension}");
        if segment_dir.join(&filename).is_file() {
            return (
                Some(format!(
                    "/app/speakers/api/serve_audio/{day}/{stream}/{segment_key}/{filename}"
                )),
                Some(mimetype.to_owned()),
            );
        }
    }
    (None, None)
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

/// Match entity names using Python's tiers 1–7.
///
/// Tier 8 fuzzy matching is intentionally not ported: its `rapidfuzz`
/// `token_sort_ratio` semantics need a faithful implementation, not a merely
/// similar Rust string-distance crate. A future wave adding fuzzy support must
/// port those semantics rather than extending this fallback.
fn find_matching_entity<'a>(
    detected_name: &str,
    entities: &'a [(String, Value)],
) -> Option<&'a Value> {
    if detected_name.is_empty() || entities.is_empty() {
        return None;
    }
    let detected_lower = detected_name.to_lowercase();
    let candidates = entities
        .iter()
        .filter_map(|(_, entity)| {
            entity
                .get("name")
                .and_then(Value::as_str)
                .map(|name| (entity, name))
        })
        .collect::<Vec<_>>();

    if let Some(entity) = candidates.iter().rev().find_map(|(entity, name)| {
        entity_strings(entity, name)
            .contains(&detected_name)
            .then_some(*entity)
    }) {
        return Some(entity);
    }
    if let Some(entity) = candidates.iter().rev().find_map(|(entity, name)| {
        entity_strings(entity, name)
            .iter()
            .any(|value| value.to_lowercase() == detected_lower)
            .then_some(*entity)
    }) {
        return Some(entity);
    }
    if detected_name.contains('@')
        && let Some(entity) = candidates.iter().rev().find_map(|(entity, _)| {
            entity
                .get("emails")?
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .any(|email| email.to_lowercase() == detected_lower)
                .then_some(*entity)
        })
    {
        return Some(entity);
    }
    let detected_slug = entity_slug(detected_name);
    if !detected_slug.is_empty()
        && let Some(entity) = candidates.iter().rev().find_map(|(entity, name)| {
            entity_matches_slug(entity, name, &detected_slug).then_some(*entity)
        })
    {
        return Some(entity);
    }
    if detected_name.len() >= 3 {
        let matches = candidates
            .iter()
            .filter(|(_, name)| first_word(name) == detected_lower)
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            return Some(matches[0].0);
        }
        let detected_first = detected_name
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_lowercase();
        if detected_first != detected_lower && detected_first.len() >= 3 {
            let matches = candidates
                .iter()
                .filter(|(_, name)| first_word(name) == detected_first)
                .collect::<Vec<_>>();
            if matches.len() == 1 && matches[0].1.split_whitespace().count() == 1 {
                return Some(matches[0].0);
            }
        }
    }
    let subset_matches = candidates
        .iter()
        .filter(|(_, name)| token_subset_match(&detected_lower, &name.to_lowercase()))
        .collect::<Vec<_>>();
    if subset_matches.len() == 1 {
        return Some(subset_matches[0].0);
    }
    let prefix_matches = candidates
        .iter()
        .filter(|(_, name)| prefix_token_match(&detected_lower, &name.to_lowercase()))
        .collect::<Vec<_>>();
    if prefix_matches.len() == 1 {
        Some(prefix_matches[0].0)
    } else {
        None
    }
}

fn entity_strings<'a>(entity: &'a Value, name: &'a str) -> Vec<&'a str> {
    std::iter::once(name)
        .chain(entity.get("id").and_then(Value::as_str))
        .chain(
            entity
                .get("aka")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        )
        .collect()
}

fn entity_matches_slug(entity: &Value, name: &str, slug: &str) -> bool {
    match entity
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    {
        Some(entity_id) => entity_id == slug,
        None => entity_slug(name) == slug,
    }
}

fn entity_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_separator = true;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            previous_separator = false;
        } else if !previous_separator {
            slug.push('_');
            previous_separator = true;
        }
    }
    slug.trim_end_matches('_').to_owned()
}

fn first_word(value: &str) -> String {
    let word = value
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    if word.len() >= 3 { word } else { String::new() }
}

fn token_subset_match(left: &str, right: &str) -> bool {
    let left = left.split_whitespace().collect::<BTreeSet<_>>();
    let right = right.split_whitespace().collect::<BTreeSet<_>>();
    let (shorter, longer) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    shorter.len() >= 2 && shorter.is_subset(&longer)
}

fn prefix_token_match(left: &str, right: &str) -> bool {
    let mut left = left.split_whitespace().collect::<Vec<_>>();
    let mut right = right.split_whitespace().collect::<Vec<_>>();
    left.sort_unstable();
    right.sort_unstable();
    left.len() == right.len()
        && left.into_iter().zip(right).all(|(left, right)| {
            left == right
                || (left.len() >= 4 && right.starts_with(left))
                || (right.len() >= 4 && left.starts_with(right))
        })
}
