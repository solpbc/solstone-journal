// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;

use crate::JournalRoot;
use crate::speakers_calendar::audio_embedding_sources;
use crate::speakers_known::intra_cosine_p25;
use crate::speakers_npz::{load_voiceprints, npz_row_count, owner_centroid_summary};
use crate::speakers_quality::{
    ManualOwnerTagStats, awareness_voiceprint, manual_owner_tag_stats, segment_overlap_fraction,
};
use solstone_core_speaker_resolve::segment_catalog::{CatalogBuildError, catalog_journal};

const OWNER_BOOTSTRAP_MIN_STATEMENTS: usize = 30;
const OWNER_REJECTION_COOLDOWN_DAYS: i64 = 14;
const OWNER_DETECT_CANDIDATE_GUIDANCE: &str =
    "Analyze available voice patterns to look for an owner voice candidate.";
const OWNER_REJECTION_COOLDOWN_GUIDANCE: &str = "Wait for the owner voice rejection cooldown before running detection again, \
or run solstone call speakers detect --force to look now.";

pub async fn status(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    match owner_status(&root.0) {
        Ok(status) => Json(status).into_response(),
        Err(OwnerStatusError::IdentityInvalid) => error_envelope(
            "speaker_owner_identity_invalid",
            "your owner voice couldn't be loaded because your configured owner identity needs attention.",
            "configured owner identity is not admitted",
            StatusCode::BAD_REQUEST,
        )
        .into_response(),
        Err(OwnerStatusError::Catalog(error)) => error_envelope(
            "speaker_command_failed",
            "that speaker command didn't finish.",
            error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
    }
}

enum OwnerStatusError {
    IdentityInvalid,
    Catalog(CatalogBuildError),
}

fn owner_status(root: &Path) -> Result<Value, OwnerStatusError> {
    let principal_id = match solstone_core_speaker_resolve::owner_admission::admitted_owner_id(root)
    {
        solstone_core_speaker_resolve::owner_admission::OwnerAdmission::Admitted(id) => id,
        solstone_core_speaker_resolve::owner_admission::OwnerAdmission::Invalid => {
            return Err(OwnerStatusError::IdentityInvalid);
        }
    };
    let voiceprint = awareness_voiceprint(root);
    let status = match voiceprint.get("status") {
        Some(Value::String(status)) => status.as_str(),
        Some(_) => "invalid",
        None => "none",
    };
    let manual_stats = manual_owner_tag_stats(root, &principal_id);
    let diagnostics = diagnostics(root, &manual_stats).map_err(OwnerStatusError::Catalog)?;

    Ok(match status {
        "confirmed" => confirmed_status(root, &principal_id, &manual_stats),
        "candidate" => json!({
            "status": "candidate",
            "cluster_size": voiceprint.get("cluster_size").cloned().unwrap_or(Value::Null),
            "samples": voiceprint.get("samples").cloned().unwrap_or_else(|| json!([])),
            "evidence_tier": voiceprint.get("evidence_tier").cloned().unwrap_or(Value::Null),
        }),
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
            if let Some(cooldown) = rejection_cooldown(&voiceprint) {
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

fn confirmed_status(root: &Path, principal_id: &str, manual_stats: &ManualOwnerTagStats) -> Value {
    let centroid = owner_centroid_summary(
        &root
            .join("entities")
            .join(principal_id)
            .join("owner_centroid.npz"),
    );
    let (streams, intra_cosine_p25) = if centroid.is_some() {
        load_voiceprints(
            &root
                .join("entities")
                .join(principal_id)
                .join("voiceprints.npz"),
        )
        .map(|voiceprints| {
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
        })
        .unwrap_or_default()
    } else {
        (Vec::new(), None)
    };
    json!({
        "status": "confirmed",
        "centroid_metadata": {
            "cluster_size": centroid.as_ref().map_or(0, |centroid| centroid.cluster_size),
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
    })
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
) -> Result<Diagnostics, CatalogBuildError> {
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

fn rejection_cooldown(voiceprint: &std::collections::BTreeMap<String, Value>) -> Option<i64> {
    let rejected_at = voiceprint.get("rejected_at")?.as_str()?;
    let days_since = parse_rejection_time(rejected_at)?;
    (days_since < OWNER_REJECTION_COOLDOWN_DAYS)
        .then_some(OWNER_REJECTION_COOLDOWN_DAYS - days_since)
}

fn parse_rejection_time(value: &str) -> Option<i64> {
    let elapsed = DateTime::parse_from_rfc3339(value)
        .map(|time| {
            Utc::now()
                .signed_duration_since(time.with_timezone(&Utc))
                .num_days()
        })
        .or_else(|_| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f").map(|time| {
                Utc::now()
                    .naive_utc()
                    .signed_duration_since(time)
                    .num_days()
            })
        })
        .ok()?;
    Some(elapsed)
}

fn owner_embedding_inventory(root: &Path) -> Result<(usize, usize), CatalogBuildError> {
    let mut segments_available = 0;
    let mut embeddings_available = 0;
    for segment in catalog_journal(root)? {
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
                npz_row_count(&segment.path.join(format!("{source}.npz")), "embeddings")
                    .unwrap_or_default();
        }
    }
    Ok((segments_available, embeddings_available))
}
