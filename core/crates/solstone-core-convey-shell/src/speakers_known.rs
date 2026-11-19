// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::window_catalog;
use solstone_core_convey_http::coalesce::ResponseLocale;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde::Serialize;
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;

use crate::JournalRoot;
use crate::speakers_calendar::live_entities;
use crate::speakers_npz::load_voiceprints;
use crate::speakers_review::is_admissible_speaker_entity;
use solstone_core_speaker_resolve::owner_detection::intra_cosine_p25;

const SORT_RECENT: &str = "recent";
const SORT_MOST_SAMPLES: &str = "most samples";
const SORT_ALPHABETICAL: &str = "alphabetical";

#[derive(Deserialize)]
pub struct KnownQuery {
    sort: Option<String>,
}

pub async fn known(
    locale: Option<axum::Extension<ResponseLocale>>,
    Extension(root): Extension<Arc<JournalRoot>>,
    Query(query): Query<KnownQuery>,
) -> Response {
    let locale = window_catalog::response_locale(locale);
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
            window_catalog::text(
                locale,
                "journal.window.speakers.error.one.of.those.values.couldn.t.be.used",
            ),
            window_catalog::text(
                locale,
                "journal.window.speakers.error.invalid.sort.parameter",
            ),
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::SpeakersKnown,
        move || {
            let mut speakers = match known_speakers(&root.0) {
                Ok(speakers) => speakers,
                Err(error) => {
                    return error_envelope(
                        "speaker_command_failed",
                        window_catalog::text(
                            locale,
                            "journal.window.speakers.error.that.speaker.command.didn.t.finish",
                        ),
                        error.to_string(),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )
                    .into_response();
                }
            };
            sort_speakers(&mut speakers, &sort);
            Json(json!({"speakers": speakers, "total": speakers.len(), "sort": sort}))
                .into_response()
        },
    )
    .await
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

fn known_speakers(
    root: &Path,
) -> Result<Vec<Speaker>, solstone_core_journal_io::zip2_name::Zip2NameFailure> {
    live_entities(root)
        .into_iter()
        .filter_map(|(dir, entity)| {
            let entity_id = entity.id;
            let entity = entity.value;
            // The owner's own voice is the "your voice" section, from the
            // owner profile. Samples on the owner's entity are not someone
            // else's voice, so they never list here as a known speaker.
            if !is_admissible_speaker_entity(&entity)
                || solstone_core_entity::identity_is_principal(&entity)
            {
                return None;
            }
            let voiceprints = match load_voiceprints(
                &solstone_core_entity::entity_voiceprints_path(root, &dir).ok()?,
            ) {
                Ok(Some(voiceprints)) => voiceprints,
                Ok(None) => return None,
                Err(error) => return Some(Err(error)),
            };
            let mut streams = BTreeSet::new();
            let mut segments = BTreeSet::new();
            let mut last_seen = None;
            for metadata in &voiceprints.metadata {
                if let Some(stream) = metadata.get("stream").and_then(Value::as_str)
                    && !stream.is_empty()
                {
                    streams.insert(stream.to_owned());
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
                segments.insert(segment);
                if let Some(last_seen_ts) = metadata.get("last_seen_ts").and_then(Value::as_i64) {
                    last_seen = Some(
                        last_seen.map_or(last_seen_ts, |previous: i64| previous.max(last_seen_ts)),
                    );
                }
            }
            let name = entity_name(&entity, &entity_id);
            Some(Ok(Speaker {
                entity_id,
                name,
                embedding_count: voiceprints.embeddings.len(),
                segment_count: segments.len(),
                streams: streams.into_iter().collect(),
                last_seen_ts: last_seen,
                intra_cosine_p25: intra_cosine_p25(&voiceprints.embeddings),
            }))
        })
        .collect()
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
#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::known_speakers;

    #[test]
    fn known_speakers_reports_unproved_names_instead_of_omitting_the_person() {
        let root = tempfile::tempdir().unwrap();
        let entity = root.path().join("entities/person");
        fs::create_dir_all(&entity).unwrap();
        fs::write(
            entity.join("entity.json"),
            br#"{"id":"person","type":"Person"}"#,
        )
        .unwrap();
        assert!(known_speakers(root.path()).unwrap().is_empty());
        fs::write(
            entity.join("voiceprints.npz"),
            crate::speakers_npz::tests::colliding("embeddings.npy", b"x"),
        )
        .unwrap();
        assert!(known_speakers(root.path()).is_err());
    }

    #[test]
    fn known_speakers_excludes_the_owner_non_person_and_blocked_voiceprints() {
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
            (
                "owner",
                json!({"id":"owner","type":"Person","is_principal":true}),
            ),
        ] {
            let entity_dir = root.join("entities").join(id);
            fs::create_dir_all(&entity_dir).expect("entity directory");
            fs::write(
                entity_dir.join("entity.json"),
                serde_json::to_vec(&entity).expect("entity json"),
            )
            .expect("entity writes");
            solstone_core_entity::save_voiceprints_batch(
                &root,
                id,
                &[solstone_core_entity::VoiceprintItem {
                    embedding: vec![1.0; 256],
                    metadata: json!({"day":"20260808","stream":"main","segment_key":"120000_1","source":"audio","sentence_id":1}),
                }],
                &solstone_core_entity::EncoderIdentity {
                    id: "test".to_owned(),
                    sha256: "0".repeat(64),
                    width: 256,
                },
            )
            .expect("voiceprint writes");
        }

        let ids = known_speakers(&root)
            .expect("speaker archive names are proved")
            .into_iter()
            .map(|speaker| speaker.entity_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["person"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn known_speakers_are_keyed_by_id_with_the_voice_from_the_folder_holding_them() {
        let root = std::env::temp_dir().join(format!(
            "solstone-known-speaker-folders-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        for (folder, entity) in [
            // Its file names another id than its folder.
            ("a", json!({"id":"bee","name":"Bee","type":"Person"})),
            // No id written: the folder is its id.
            ("cee", json!({"name":"Cee","type":"Person"})),
            // Claims `bee` too, and loses to `a`, the first folder in order.
            ("zz", json!({"id":"bee","name":"Impostor","type":"Person"})),
        ] {
            let entity_dir = root.join("entities").join(folder);
            fs::create_dir_all(&entity_dir).expect("entity directory");
            fs::write(
                entity_dir.join("entity.json"),
                serde_json::to_vec(&entity).expect("entity json"),
            )
            .expect("entity writes");
        }
        for id in ["bee", "cee"] {
            solstone_core_entity::save_voiceprints_batch(
                &root,
                id,
                &[solstone_core_entity::VoiceprintItem {
                    embedding: vec![1.0; 256],
                    metadata: json!({"day":"20260808","stream":"main","segment_key":"120000_1","source":"audio","sentence_id":1}),
                }],
                &solstone_core_entity::EncoderIdentity {
                    id: "test".to_owned(),
                    sha256: "0".repeat(64),
                    width: 256,
                },
            )
            .expect("voiceprint writes");
        }
        // The loser has a voice too, so only the one-per-id rule leaves it out.
        fs::copy(
            root.join("entities/a/voiceprints.npz"),
            root.join("entities/zz/voiceprints.npz"),
        )
        .expect("loser voice");

        let speakers = known_speakers(&root)
            .expect("speaker archive names are proved")
            .into_iter()
            .map(|speaker| (speaker.entity_id, speaker.name))
            .collect::<Vec<_>>();
        assert_eq!(
            speakers,
            vec![
                ("bee".to_owned(), "Bee".to_owned()),
                ("cee".to_owned(), "Cee".to_owned())
            ]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(all(test, feature = "full-tests"))]
    fn known_voice_cache_live_reads_reflect_entity_and_voiceprint_mutations() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();

        let encoder = solstone_core_entity::EncoderIdentity {
            id: "test".to_owned(),
            sha256: "0".repeat(64),
            width: 256,
        };

        for (id, name) in [("alice", "Alice"), ("bob", "Bob")] {
            let entity_dir = root.join("entities").join(id);
            fs::create_dir_all(&entity_dir).expect("entity dir");
            fs::write(
                entity_dir.join("entity.json"),
                serde_json::to_vec(&json!({"id": id, "name": name, "type": "Person"})).unwrap(),
            )
            .unwrap();
            solstone_core_entity::save_voiceprints_batch(
                root,
                id,
                &[
                    solstone_core_entity::VoiceprintItem {
                        embedding: vec![1.0; 256],
                        metadata: json!({"day":"20261015","stream":"main","segment_key":"120000_1","source":"audio","sentence_id":1}),
                    },
                    solstone_core_entity::VoiceprintItem {
                        embedding: vec![0.5; 256],
                        metadata: json!({"day":"20261015","stream":"main","segment_key":"120000_1","source":"audio","sentence_id":2}),
                    },
                ],
                &encoder,
            )
            .unwrap();
        }

        // 1. Initial read: both present
        let initial = known_speakers(root).expect("speaker archive names are proved");
        assert_eq!(initial.len(), 2);
        let alice_p25 = initial
            .iter()
            .find(|s| s.entity_id == "alice")
            .unwrap()
            .intra_cosine_p25;
        assert!(alice_p25.is_some());

        // 2. Name-only edit of entity.json shows new name, same p25
        let alice_dir = root.join("entities").join("alice");
        fs::write(
            alice_dir.join("entity.json"),
            serde_json::to_vec(&json!({"id": "alice", "name": "Alice Wonder", "type": "Person"}))
                .unwrap(),
        )
        .unwrap();
        let after_rename = known_speakers(root).expect("speaker archive names are proved");
        let alice_after = after_rename
            .iter()
            .find(|s| s.entity_id == "alice")
            .unwrap();
        assert_eq!(alice_after.name, "Alice Wonder");
        assert_eq!(alice_after.intra_cosine_p25, alice_p25);

        // 3. Block bob
        let bob_dir = root.join("entities").join("bob");
        fs::write(
            bob_dir.join("entity.json"),
            serde_json::to_vec(
                &json!({"id": "bob", "name": "Bob", "type": "Person", "blocked": true}),
            )
            .unwrap(),
        )
        .unwrap();
        let after_block = known_speakers(root).expect("speaker archive names are proved");
        assert_eq!(after_block.len(), 1);
        assert_eq!(after_block[0].entity_id, "alice");

        // Unblock bob
        fs::write(
            bob_dir.join("entity.json"),
            serde_json::to_vec(&json!({"id": "bob", "name": "Bob", "type": "Person"})).unwrap(),
        )
        .unwrap();
        assert_eq!(
            known_speakers(root)
                .expect("speaker archive names are proved")
                .len(),
            2
        );

        // 4. Change bob to non-person
        fs::write(
            bob_dir.join("entity.json"),
            serde_json::to_vec(&json!({"id": "bob", "name": "Bob", "type": "Tool"})).unwrap(),
        )
        .unwrap();
        let after_type = known_speakers(root).expect("speaker archive names are proved");
        assert_eq!(after_type.len(), 1);
        assert_eq!(after_type[0].entity_id, "alice");

        // Restore bob type
        fs::write(
            bob_dir.join("entity.json"),
            serde_json::to_vec(&json!({"id": "bob", "name": "Bob", "type": "Person"})).unwrap(),
        )
        .unwrap();
        assert_eq!(
            known_speakers(root)
                .expect("speaker archive names are proved")
                .len(),
            2
        );

        // 5. Corrupt bob's voiceprint file
        fs::write(bob_dir.join("voiceprints.npz"), b"not an npz file").unwrap();
        let after_corrupt = known_speakers(root).expect("speaker archive names are proved");
        assert_eq!(after_corrupt.len(), 1);
        assert_eq!(after_corrupt[0].entity_id, "alice");

        // 6. Delete bob entity directory entirely
        let _ = fs::remove_dir_all(&bob_dir);
        let after_delete = known_speakers(root).expect("speaker archive names are proved");
        assert_eq!(after_delete.len(), 1);
        assert_eq!(after_delete[0].entity_id, "alice");
    }
}
