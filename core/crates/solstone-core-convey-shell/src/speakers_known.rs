// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cmp::Ordering;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use ndarray::Array2;
use serde::Deserialize;
use serde::Serialize;
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;

use crate::JournalRoot;
use crate::speakers_npz::load_voiceprints;
use crate::speakers_review::is_admissible_speaker_entity;

const SORT_RECENT: &str = "recent";
const SORT_MOST_SAMPLES: &str = "most samples";
const SORT_ALPHABETICAL: &str = "alphabetical";

#[derive(Deserialize)]
pub struct KnownQuery {
    sort: Option<String>,
}

pub async fn known(
    Extension(root): Extension<Arc<JournalRoot>>,
    Query(query): Query<KnownQuery>,
) -> Response {
    let sort = query
        .sort
        .filter(|sort| !sort.is_empty())
        .unwrap_or_else(|| SORT_RECENT.to_owned())
        .replace('_', " ");
    if !matches!(
        sort.as_str(),
        SORT_RECENT | SORT_MOST_SAMPLES | SORT_ALPHABETICAL
    ) {
        return error_envelope(
            "invalid_request_value",
            "one of those values couldn't be used.",
            "Invalid sort parameter",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }
    let mut speakers = known_speakers(&root.0);
    sort_speakers(&mut speakers, &sort);
    Json(json!({"speakers": speakers, "total": speakers.len(), "sort": sort})).into_response()
}

#[derive(Serialize)]
struct Speaker {
    entity_id: String,
    name: String,
    embedding_count: usize,
    segment_count: usize,
    streams: Vec<String>,
    last_seen_ts: Option<i64>,
    intra_cosine_p25: Option<f64>,
}

fn known_speakers(root: &Path) -> Vec<Speaker> {
    let mut entities = fs::read_dir(root.join("entities"))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect::<Vec<_>>();
    entities.sort_by_key(|entry| entry.file_name());
    entities
        .into_iter()
        .filter_map(|entry| {
            let entity_id = entry.file_name().to_string_lossy().into_owned();
            let entity = load_entity(&entry.path())?;
            if !is_admissible_speaker_entity(&entity) {
                return None;
            }
            let voiceprints = load_voiceprints(&entry.path().join("voiceprints.npz"))?;
            let mut streams = Vec::new();
            let mut segments = Vec::new();
            let mut last_seen = Vec::new();
            for metadata in &voiceprints.metadata {
                if let Some(stream) = metadata.get("stream").and_then(Value::as_str)
                    && !stream.is_empty()
                    && !streams.iter().any(|known| known == stream)
                {
                    streams.push(stream.to_owned());
                }
                let day = metadata
                    .get("day")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let segment_key = metadata
                    .get("segment_key")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let segment = (day.to_owned(), segment_key.to_owned());
                if !segments.contains(&segment) {
                    segments.push(segment);
                }
                if let Some(last_seen_ts) = metadata.get("last_seen_ts").and_then(Value::as_i64) {
                    last_seen.push(last_seen_ts);
                }
            }
            streams.sort();
            let name = entity_name(&entity, &entity_id);
            Some(Speaker {
                entity_id,
                name,
                embedding_count: voiceprints.embeddings.len(),
                segment_count: segments.len(),
                streams,
                last_seen_ts: last_seen.into_iter().max(),
                intra_cosine_p25: intra_cosine_p25(&voiceprints.embeddings),
            })
        })
        .collect()
}

fn load_entity(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path.join("entity.json")).ok()?).ok()
}

fn entity_name(entity: &Value, entity_id: &str) -> String {
    entity
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| entity_id.to_owned())
}

fn sort_speakers(speakers: &mut [Speaker], sort: &str) {
    speakers.sort_by(|left, right| match sort {
        SORT_MOST_SAMPLES => right
            .embedding_count
            .cmp(&left.embedding_count)
            .then_with(|| name_key(left).cmp(&name_key(right)))
            .then_with(|| left.entity_id.cmp(&right.entity_id)),
        SORT_ALPHABETICAL => name_key(left)
            .cmp(&name_key(right))
            .then_with(|| left.entity_id.cmp(&right.entity_id)),
        _ => left
            .last_seen_ts
            .is_none()
            .cmp(&right.last_seen_ts.is_none())
            .then_with(|| {
                right
                    .last_seen_ts
                    .unwrap_or_default()
                    .cmp(&left.last_seen_ts.unwrap_or_default())
            })
            .then_with(|| name_key(left).cmp(&name_key(right)))
            .then_with(|| left.entity_id.cmp(&right.entity_id)),
    });
}

fn name_key(speaker: &Speaker) -> String {
    speaker.name.to_lowercase()
}

/// Compute p25 with a float32 GEMM, matching Python's algorithm.
///
/// This is the fourth permanent narrow frozen-oracle deviation, alongside the
/// ETag value, unregistered-extension 500→refusal, and chat-bar placeholder:
/// BLAS backends may reduce float32 products in different non-associative
/// orders. The resulting last-bit variance is numerical, not behavioral.
pub(crate) fn intra_cosine_p25(embeddings: &[Vec<f32>]) -> Option<f64> {
    if embeddings.len() < 2 {
        return None;
    }
    let normalized = embeddings
        .iter()
        .map(|embedding| {
            let norm = embedding
                .iter()
                .map(|value| value * value)
                .sum::<f32>()
                .sqrt();
            let norm = if norm < 1e-9 { 1.0 } else { norm };
            embedding
                .iter()
                .map(|value| value / norm)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let columns = normalized.first()?.len();
    if columns == 0
        || normalized
            .iter()
            .any(|embedding| embedding.len() != columns)
    {
        return None;
    }
    let rows = normalized.len();
    let values = normalized.into_iter().flatten().collect::<Vec<_>>();
    let matrix = Array2::from_shape_vec((rows, columns), values).ok()?;
    // NumPy uses a float32 matrix multiply (`e_norm @ e_norm.T`) before
    // percentile interpolation, so use a GEMM rather than a scalar dot loop.
    let similarities = matrix.dot(&matrix.t());
    let mut cosines = Vec::new();
    for row in 0..rows {
        cosines.extend((row + 1..rows).map(|column| similarities[(row, column)]));
    }
    cosines.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    let rank = (cosines.len() - 1) as f64 * 0.25;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    let fraction = rank - lower as f64;
    Some(f64::from(cosines[lower]) + fraction * f64::from(cosines[upper] - cosines[lower]))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::{intra_cosine_p25, known_speakers};

    #[test]
    fn p25_is_none_for_one_voiceprint() {
        assert_eq!(intra_cosine_p25(&[vec![1.0, 0.0]]), None);
    }

    #[test]
    fn frozen_voiceprint_batches_stay_close_to_python_p25() {
        let voices: std::collections::BTreeMap<String, Vec<Vec<f32>>> = serde_json::from_str(
            include_str!("../../../fixtures/populated_journal_voiceprints.json"),
        )
        .expect("fixture parses");
        for (name, expected) in [
            ("grace_hopper", -0.041_334_249_079_227_45),
            ("alan_turing", 0.074_594_385_921_955_11),
        ] {
            let actual = intra_cosine_p25(&voices[name]).expect("multiple voiceprints have p25");
            assert!(
                (actual - expected).abs() < 1e-4,
                "{name}: {actual} vs {expected}"
            );
        }
    }

    #[test]
    fn known_speakers_excludes_non_person_and_blocked_voiceprints() {
        let root = std::env::temp_dir().join(format!(
            "solstone-known-speaker-admission-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        for (id, entity) in [
            ("person", json!({"id":"person","type":"Person"})),
            ("tool", json!({"id":"tool","type":"Tool"})),
            (
                "blocked",
                json!({"id":"blocked","type":"Person","blocked":true}),
            ),
        ] {
            let entity_dir = root.join("entities").join(id);
            fs::create_dir_all(&entity_dir).expect("entity directory");
            fs::write(
                entity_dir.join("entity.json"),
                serde_json::to_vec(&entity).expect("entity json"),
            )
            .expect("entity writes");
            solstone_core_speaker_resolve::direct_voiceprints::write_voiceprint(
                &root,
                id,
                vec![1.0; 256],
                json!({"day":"20260808","stream":"main","segment_key":"120000_1","source":"audio","sentence_id":1}),
                &solstone_core_entity::EncoderIdentity {
                    id: "test".to_owned(),
                    sha256: "0".repeat(64),
                    width: 256,
                },
            )
            .expect("voiceprint writes");
        }

        let ids = known_speakers(&root)
            .into_iter()
            .map(|speaker| speaker.entity_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["person"]);
        let _ = fs::remove_dir_all(root);
    }
}
