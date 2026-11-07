// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;

use crate::JournalRoot;
use crate::speakers_calendar::audio_embedding_sources;
use crate::speakers_npz::{
    OwnerCentroidSummary, load_voiceprints, npz_row_count, owner_centroid_summary,
};
use crate::speakers_quality::{ManualOwnerTagStats, manual_owner_tag_stats, principal_folder};
use solstone_core_speaker_resolve::owner_detection::{
    CANDIDATE_EXPANSION_MAX_EMBEDDINGS, intra_cosine_p25, segment_overlap_fraction,
};
use solstone_core_speaker_resolve::owner_voice::{
    cooldown_days_remaining, next_look_after, relabel_progress, voiceprint_state,
};
use solstone_core_speaker_resolve::segment_catalog::{CatalogBuildError, catalog_journal};

const OWNER_BOOTSTRAP_MIN_STATEMENTS: usize = 30;
/// Owner status when the owner hasn't added their name yet.
pub(crate) const OWNER_IDENTITY_NEEDED_STATUS: &str = "identity_needed";

/// Whether a journal with no admitted owner is simply one whose owner hasn't
/// added their name. No setup path asks for it, so a new journal is like this
/// until the owner does; that is a state to show, not a failed read. A name
/// that is set but admits no one, or a config that can't be read, still needs
/// attention.
pub(crate) fn owner_identity_needed(root: &Path) -> bool {
    matches!(solstone_core_entity::owner_name_configured(root), Ok(false))
}
const OWNER_DETECT_CANDIDATE_GUIDANCE: &str =
    "Analyze available voice patterns to look for an owner voice candidate.";
const OWNER_REJECTION_COOLDOWN_GUIDANCE: &str = "Wait for the owner voice rejection cooldown before running detection again, \
or run solstone call speakers detect --force to look now.";

pub async fn status(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::SpeakersOwnerStatus,
        move || match owner_status(&root.0) {
            Ok(status) => Json(status).into_response(),
            Err(OwnerStatusError::IdentityInvalid) => error_envelope(
                "speaker_owner_identity_invalid",
                "your owner voice couldn't be loaded because your configured owner identity needs attention.",
                "configured owner identity is not admitted",
                StatusCode::CONFLICT,
            )
            .into_response(),
            Err(OwnerStatusError::Catalog(error)) => error_envelope(
                "speaker_command_failed",
                "that speaker command didn't finish.",
                error.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response(),
            Err(OwnerStatusError::Failed(error)) => error_envelope(
                "speaker_command_failed",
                "that speaker command didn't finish.",
                error,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response(),
        },
    )
    .await
}

#[derive(Debug)]
enum OwnerStatusError {
    IdentityInvalid,
    Catalog(CatalogBuildError),
    Failed(String),
}

fn owner_status(root: &Path) -> Result<Value, OwnerStatusError> {
    let mut status = owner_setup_status(root)?;
    if let Some(relabel) = relabel_progress(root) {
        status["relabel"] = relabel;
    }
    Ok(status)
}

fn owner_setup_status(root: &Path) -> Result<Value, OwnerStatusError> {
    let principal_id = match solstone_core_speaker_resolve::owner_admission::admitted_owner_id(root)
    {
        solstone_core_speaker_resolve::owner_admission::OwnerAdmission::Admitted(id) => id,
        solstone_core_speaker_resolve::owner_admission::OwnerAdmission::Invalid => {
            return if owner_identity_needed(root) {
                Ok(json!({"status": OWNER_IDENTITY_NEEDED_STATUS}))
            } else {
                Err(OwnerStatusError::IdentityInvalid)
            };
        }
    };
    let lock = solstone_core_speaker_resolve::owner_candidate::hold_owner_candidate_lock(root)
        .map_err(|e| OwnerStatusError::Failed(e.to_string()))?;
    let voiceprint = voiceprint_state(root);
    let status = match voiceprint.get("status") {
        Some(Value::String(status)) => status.as_str(),
        Some(_) => "invalid",
        None => "none",
    };
    // Snapshot the status with the candidate or saved voice it describes.
    // Recording inventory and manual-tag validation can take much longer.
    let candidate_opt = if status == "candidate" {
        solstone_core_speaker_resolve::owner_candidate::load_owner_candidate(root)
            .map_err(|e| OwnerStatusError::Failed(e.to_string()))?
    } else {
        None
    };
    let centroid = if status == "confirmed" {
        match solstone_core_speaker_resolve::owner_centroid::owner_centroid_file(
            root,
            &principal_id,
        ) {
            Ok(path) => owner_centroid_summary(&path)
                .map_err(|error| OwnerStatusError::Failed(error.to_string()))?,
            Err(_) => None,
        }
    } else {
        None
    };
    drop(lock);
    let manual_stats = manual_owner_tag_stats(root, &principal_id)
        .map_err(|error| OwnerStatusError::Failed(error.to_string()))?;
    if status == "confirmed" {
        return confirmed_status(root, &principal_id, &manual_stats, centroid);
    }
    let diagnostics = diagnostics(root, &manual_stats)?;

    Ok(match status {
        "candidate" => {
            let detected_at = voiceprint.get("detected_at").and_then(Value::as_str);

            match (candidate_opt, detected_at) {
                (Some(candidate), Some(ver)) if candidate.version == ver => {
                    let raw_samples = voiceprint
                        .get("samples")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let hydrated = solstone_core_speaker_resolve::owner_candidate::hydrate_owner_candidate_samples(
                        root,
                        &candidate,
                        &raw_samples,
                    );
                    json!({
                        "status": "candidate",
                        "version": candidate.version,
                        "cluster_size": voiceprint.get("cluster_size").cloned().unwrap_or_else(|| json!(candidate.cluster_size)),
                        "samples": hydrated,
                        "evidence_tier": candidate.evidence_tier,
                        "manual_tags_count": diagnostics.manual_tags_count,
                        "segments_available": diagnostics.segments_available,
                        "embeddings_available": diagnostics.embeddings_available,
                        "streams_represented": diagnostics.streams_represented,
                        "can_build_from_tags": diagnostics.can_build_from_tags,
                        "segments_with_embeddings": diagnostics.segments_with_embeddings,
                    })
                }
                _ => json!({
                    "status": "candidate",
                    "review": "incomplete",
                    "version": voiceprint.get("detected_at").cloned().unwrap_or(Value::Null),
                    "cluster_size": voiceprint.get("cluster_size").cloned().unwrap_or(Value::Null),
                    "samples": [],
                    "evidence_tier": voiceprint.get("evidence_tier").cloned().unwrap_or(Value::Null),
                    "manual_tags_count": diagnostics.manual_tags_count,
                    "segments_available": diagnostics.segments_available,
                    "embeddings_available": diagnostics.embeddings_available,
                    "streams_represented": diagnostics.streams_represented,
                    "can_build_from_tags": diagnostics.can_build_from_tags,
                    "segments_with_embeddings": diagnostics.segments_with_embeddings,
                }),
            }
        }
        "low_quality" => {
            let guidance = manual_guidance(&manual_stats);
            json!({
                "status": "low_quality",
                "source": voiceprint.get("source").cloned().unwrap_or_else(|| json!("candidate_pool")),
                "low_quality_reason": voiceprint.get("low_quality_reason").cloned().unwrap_or_else(|| json!("")),
                "observed_value": voiceprint.get("observed_value").cloned().unwrap_or_else(|| json!(0.0)),
                "threshold_value": voiceprint.get("threshold_value").cloned().unwrap_or_else(|| json!(0.0)),
                "evidence_tier": voiceprint.get("evidence_tier").cloned().unwrap_or(Value::Null),
                "intra_cosine_p25_bound": voiceprint.get("intra_cosine_p25_bound").cloned().unwrap_or(Value::Null),
                "manual_tags_count": diagnostics.manual_tags_count,
                "segments_available": diagnostics.segments_available,
                "embeddings_available": diagnostics.embeddings_available,
                "streams_represented": diagnostics.streams_represented,
                "can_build_from_tags": diagnostics.can_build_from_tags,
                "segments_with_embeddings": diagnostics.segments_with_embeddings,
                "next_step": guidance.next_step,
                "guidance": guidance.guidance,
            })
        }
        "no_cluster" => {
            let guidance = manual_guidance(&manual_stats);
            json!({
                "status": "no_cluster",
                "manual_tags_count": diagnostics.manual_tags_count,
                "segments_available": diagnostics.segments_available,
                "embeddings_available": diagnostics.embeddings_available,
                "streams_represented": diagnostics.streams_represented,
                "can_build_from_tags": diagnostics.can_build_from_tags,
                "segments_with_embeddings": diagnostics.segments_with_embeddings,
                "next_step": guidance.next_step,
                "guidance": guidance.guidance,
            })
        }
        "none" | "rejected" => {
            if let Some(cooldown) = cooldown_days_remaining(&voiceprint) {
                return Ok(json!({
                    "status": "none",
                    "manual_tags_count": diagnostics.manual_tags_count,
                    "segments_available": diagnostics.segments_available,
                    "embeddings_available": diagnostics.embeddings_available,
                    "streams_represented": diagnostics.streams_represented,
                    "can_build_from_tags": diagnostics.can_build_from_tags,
                    "segments_with_embeddings": diagnostics.segments_with_embeddings,
                    "reason": "cooldown",
                    "days_remaining": cooldown,
                    "next_look_after": next_look_after(&voiceprint).map(|time| time.to_rfc3339()),
                    "next_step": "wait_for_cooldown",
                    "guidance": OWNER_REJECTION_COOLDOWN_GUIDANCE,
                }));
            }
            if diagnostics.segments_available > 0 {
                return Ok(json!({
                    "status": "needs_detection",
                    "manual_tags_count": diagnostics.manual_tags_count,
                    "segments_available": diagnostics.segments_available,
                    "embeddings_available": diagnostics.embeddings_available,
                    "streams_represented": diagnostics.streams_represented,
                    "can_build_from_tags": diagnostics.can_build_from_tags,
                    "segments_with_embeddings": diagnostics.segments_with_embeddings,
                    "next_step": "detect_candidate",
                    "guidance": OWNER_DETECT_CANDIDATE_GUIDANCE,
                }));
            }
            manual_fallback(&diagnostics, &manual_stats)
        }
        _ => manual_fallback(&diagnostics, &manual_stats),
    })
}

fn confirmed_status(
    root: &Path,
    principal_id: &str,
    manual_stats: &ManualOwnerTagStats,
    centroid: Option<OwnerCentroidSummary>,
) -> Result<Value, OwnerStatusError> {
    let folder = principal_folder(root, principal_id);
    let (streams, intra_cosine_p25) = if centroid.is_some() {
        match solstone_core_entity::entity_voiceprints_path(root, &folder) {
            Ok(path) => match load_voiceprints(&path) {
                Ok(Some(voiceprints)) => {
                    let mut streams = voiceprints
                        .metadata
                        .iter()
                        .filter_map(|row| row.get("stream").and_then(Value::as_str))
                        .filter(|stream| !stream.is_empty())
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    streams.sort();
                    streams.dedup();
                    (streams, intra_cosine_p25(&voiceprints.embeddings))
                }
                Ok(None) => (Vec::new(), None),
                Err(error) => return Err(OwnerStatusError::Failed(error.to_string())),
            },
            Err(_) => (Vec::new(), None),
        }
    } else {
        (Vec::new(), None)
    };
    Ok(json!({
        "status": "confirmed",
        "centroid_metadata": {
            "cluster_size": centroid.as_ref().map_or(0, |centroid| centroid.cluster_size),
            "cluster_size_capped": centroid.as_ref().is_some_and(|centroid| {
                centroid.cluster_size
                    >= CANDIDATE_EXPANSION_MAX_EMBEDDINGS as i32
            }),
            "streams": streams,
            "created_at": centroid.as_ref().and_then(|centroid| centroid.created_at.clone()),
            "last_refreshed_at": centroid.as_ref().map_or("", |centroid| &centroid.last_refreshed_at),
            "threshold": centroid.as_ref().map(|centroid| centroid.threshold),
            "margin": centroid.as_ref().and_then(|centroid| centroid.margin),
            "intra_cosine_p25": intra_cosine_p25,
            "evidence_hash": centroid.as_ref().and_then(|centroid| centroid.evidence_hash.clone()),
            "evidence_intra_cosine_p25": centroid.as_ref().and_then(|centroid| centroid.evidence_intra_cosine_p25),
            "evidence_tier": centroid.as_ref().map(|centroid| centroid.evidence_tier.clone()),
        },
        "manual_tags_count": manual_stats.manual_tags_count,
    }))
}

struct Diagnostics {
    manual_tags_count: usize,
    segments_available: usize,
    embeddings_available: usize,
    streams_represented: usize,
    can_build_from_tags: bool,
    segments_with_embeddings: usize,
}

fn diagnostics(
    root: &Path,
    manual_stats: &ManualOwnerTagStats,
) -> Result<Diagnostics, OwnerStatusError> {
    let (segments_available, embeddings_available) = owner_embedding_inventory(root)?;
    Ok(Diagnostics {
        manual_tags_count: manual_stats.manual_tags_count,
        segments_available,
        embeddings_available,
        streams_represented: manual_stats.streams_represented,
        can_build_from_tags: manual_stats.manual_tags_count >= OWNER_BOOTSTRAP_MIN_STATEMENTS,
        segments_with_embeddings: segments_available,
    })
}

struct ManualGuidance {
    next_step: &'static str,
    guidance: String,
}

fn manual_guidance(stats: &ManualOwnerTagStats) -> ManualGuidance {
    if stats.manual_tags_count >= OWNER_BOOTSTRAP_MIN_STATEMENTS {
        return ManualGuidance {
            next_step: "build_from_tags",
            guidance: format!(
                "You have {} validated owner tags (minimum {}). Run solstone call speakers build-from-tags to save your owner voice; add more with solstone call speakers tag-owner <day> <stream> <segment> <source> <sentence-id> if needed.",
                stats.manual_tags_count, OWNER_BOOTSTRAP_MIN_STATEMENTS,
            ),
        };
    }
    ManualGuidance {
        next_step: "seed_manual_tags",
        guidance: format!(
            "Use solstone call speakers tag-owner <day> <stream> <segment> <source> <sentence-id> on owner sentences in raw media until you have {OWNER_BOOTSTRAP_MIN_STATEMENTS} validated owner tags; {} more needed. Then run solstone call speakers build-from-tags.",
            OWNER_BOOTSTRAP_MIN_STATEMENTS - stats.manual_tags_count,
        ),
    }
}

fn manual_fallback(diagnostics: &Diagnostics, manual_stats: &ManualOwnerTagStats) -> Value {
    let guidance = manual_guidance(manual_stats);
    json!({
        "status": "none",
        "manual_tags_count": diagnostics.manual_tags_count,
        "segments_available": diagnostics.segments_available,
        "embeddings_available": diagnostics.embeddings_available,
        "streams_represented": diagnostics.streams_represented,
        "can_build_from_tags": diagnostics.can_build_from_tags,
        "segments_with_embeddings": diagnostics.segments_with_embeddings,
        "next_step": guidance.next_step,
        "guidance": guidance.guidance,
    })
}

fn owner_embedding_inventory(root: &Path) -> Result<(usize, usize), OwnerStatusError> {
    let mut segments_available = 0;
    let mut embeddings_available = 0;
    for segment in catalog_journal(root).map_err(OwnerStatusError::Catalog)? {
        let sources = audio_embedding_sources(&segment.path);
        if sources.is_empty() {
            continue;
        }
        segments_available += 1;
        for source in sources {
            if segment_overlap_fraction(&segment.path.join(format!("{source}.jsonl"))) > 0.10 {
                continue;
            }
            embeddings_available +=
                match npz_row_count(&segment.path.join(format!("{source}.npz")), "embeddings") {
                    Ok(count) => count.unwrap_or_default(),
                    Err(error) => return Err(OwnerStatusError::Failed(error.to_string())),
                };
        }
    }
    Ok((segments_available, embeddings_available))
}

// Owner identity states read only a temporary journal, so they run in the
// routine unit lane.
#[cfg(test)]
mod identity_tests {
    use std::fs;

    use serde_json::json;

    use super::*;

    #[test]
    fn owner_status_reports_a_journal_with_no_admitted_owner_as_a_state() {
        // No first-run path collects a name, so a new journal has no principal
        // until the owner adds one, and its two entries here are not it.
        let temp = tempfile::tempdir().expect("tempdir");
        for (id, value) in [
            ("someone", json!({"id":"someone","type":"Person"})),
            (
                "tool",
                json!({"id":"tool","type":"Tool","is_principal":true}),
            ),
        ] {
            fs::create_dir_all(temp.path().join("entities").join(id)).expect("entity dir");
            fs::write(
                temp.path().join("entities").join(id).join("entity.json"),
                value.to_string(),
            )
            .expect("entity writes");
        }
        assert_eq!(
            owner_status(temp.path()).expect("owner status reads"),
            json!({"status": OWNER_IDENTITY_NEEDED_STATUS})
        );
    }

    #[test]
    fn a_configured_name_that_admits_no_owner_is_not_the_needed_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("config")).expect("config dir");
        fs::write(
            temp.path().join("config/journal.json"),
            json!({"identity":{"name":"Field"}}).to_string(),
        )
        .expect("config writes");
        assert!(matches!(
            owner_status(temp.path()),
            Err(OwnerStatusError::IdentityInvalid)
        ));
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;

    #[test]
    fn confirmed_owner_cache_reflects_voiceprint_mutations() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let principal_id = "owner_person";

        let manual_stats = ManualOwnerTagStats {
            manual_tags_count: 5,
            streams_represented: 1,
            lookup_error_count: 0,
        };

        // 1. centroid: None -> intra_cosine_p25 is null and streams is empty
        let res_no_centroid = confirmed_status(root, principal_id, &manual_stats, None)
            .expect("readable voiceprints");
        assert_eq!(res_no_centroid["status"], "confirmed");
        assert!(res_no_centroid["centroid_metadata"]["intra_cosine_p25"].is_null());
        assert_eq!(res_no_centroid["centroid_metadata"]["streams"], json!([]));

        // Create entity and voiceprints
        let entity_dir = root.join("entities").join(principal_id);
        fs::create_dir_all(&entity_dir).expect("entity dir");
        fs::write(
            entity_dir.join("entity.json"),
            serde_json::to_vec(&json!({"id": principal_id, "name": "The Owner", "type": "Person"}))
                .unwrap(),
        )
        .unwrap();

        let encoder = solstone_core_entity::EncoderIdentity {
            id: "test".to_owned(),
            sha256: "0".repeat(64),
            width: 256,
        };
        solstone_core_entity::save_voiceprints_batch(
            root,
            principal_id,
            &[
                solstone_core_entity::VoiceprintItem {
                    embedding: vec![1.0; 256],
                    metadata: json!({"day":"20261015","stream":"mic","segment_key":"120000_1","source":"audio","sentence_id":1}),
                },
                solstone_core_entity::VoiceprintItem {
                    embedding: vec![0.5; 256],
                    metadata: json!({"day":"20261015","stream":"mic","segment_key":"120000_1","source":"audio","sentence_id":2}),
                },
            ],
            &encoder,
        )
        .unwrap();

        let centroid_summary = OwnerCentroidSummary {
            cluster_size: 2,
            threshold: 0.5,
            margin: Some(0.1),
            last_refreshed_at: "2026-10-15T12:00:00Z".to_owned(),
            created_at: Some("2026-10-15T12:00:00Z".to_owned()),
            evidence_tier: "standard".to_owned(),
            evidence_hash: Some("abc".to_owned()),
            evidence_intra_cosine_p25: Some(0.4),
        };

        // 2. centroid: Some(...) with real voiceprint -> number and streams from file
        let res_with_centroid =
            confirmed_status(root, principal_id, &manual_stats, Some(centroid_summary))
                .expect("readable voiceprints");
        assert_eq!(res_with_centroid["status"], "confirmed");
        assert!(res_with_centroid["centroid_metadata"]["intra_cosine_p25"].is_number());
        assert_eq!(
            res_with_centroid["centroid_metadata"]["streams"],
            json!(["mic"])
        );

        // 3. Corrupt voiceprint file -> null p25 and empty streams
        fs::write(entity_dir.join("voiceprints.npz"), b"corrupted bytes").unwrap();
        let centroid_summary2 = OwnerCentroidSummary {
            cluster_size: 2,
            threshold: 0.5,
            margin: Some(0.1),
            last_refreshed_at: "2026-10-15T12:00:00Z".to_owned(),
            created_at: Some("2026-10-15T12:00:00Z".to_owned()),
            evidence_tier: "standard".to_owned(),
            evidence_hash: Some("abc".to_owned()),
            evidence_intra_cosine_p25: Some(0.4),
        };
        let res_corrupted =
            confirmed_status(root, principal_id, &manual_stats, Some(centroid_summary2))
                .expect("readable voiceprints");
        assert!(res_corrupted["centroid_metadata"]["intra_cosine_p25"].is_null());
        assert_eq!(res_corrupted["centroid_metadata"]["streams"], json!([]));

        // 4. Delete voiceprint file -> null p25 and empty streams
        fs::remove_file(entity_dir.join("voiceprints.npz")).unwrap();
        let centroid_summary3 = OwnerCentroidSummary {
            cluster_size: 2,
            threshold: 0.5,
            margin: Some(0.1),
            last_refreshed_at: "2026-10-15T12:00:00Z".to_owned(),
            created_at: Some("2026-10-15T12:00:00Z".to_owned()),
            evidence_tier: "standard".to_owned(),
            evidence_hash: Some("abc".to_owned()),
            evidence_intra_cosine_p25: Some(0.4),
        };
        let res_deleted =
            confirmed_status(root, principal_id, &manual_stats, Some(centroid_summary3))
                .expect("readable voiceprints");
        assert!(res_deleted["centroid_metadata"]["intra_cosine_p25"].is_null());
        assert_eq!(res_deleted["centroid_metadata"]["streams"], json!([]));
    }
}
