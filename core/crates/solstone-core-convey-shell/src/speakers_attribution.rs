// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native speaker-attribution writes and their owner-contamination admission gate.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_entity::{EncoderIdentity, normalize_embedding};
use solstone_core_journal_io::{DEFAULT_STREAM, SegmentLayout};
use solstone_core_speaker_resolve::OWNER_IDENTITY_INVALID_REASON;
use solstone_core_speaker_resolve::owner_admission::{OwnerAdmission, admitted_owner_id};
use solstone_core_speaker_resolve::owner_contamination_screen::{
    ContaminationProbe, ContaminationScreen, screen_owner_contamination,
};
use solstone_core_speaker_resolve::owner_provisional::OwnerTierReason;

use crate::JournalRoot;
use solstone_core_speaker_resolve::segment_catalog::{
    DirectSupport, SegmentLookup, UNSUPPORTED_LAYOUT_DETAIL, UNSUPPORTED_LAYOUT_MESSAGE,
    UNSUPPORTED_LAYOUT_REASON, catalog_journal, decode_stream_layout_value, lookup_segment,
};

const OWNER_TOO_CLOSE: (&str, &str, StatusCode) = (
    "speaker_owner_voice_too_close",
    "that voice couldn't be saved because it sounds too much like yours.",
    StatusCode::BAD_REQUEST,
);
const OWNER_NOT_ENOUGH: (&str, &str, StatusCode) = (
    "speaker_owner_centroid_required",
    "that speaker command can't run until your owner voice is set up.",
    StatusCode::CONFLICT,
);
const OWNER_DAMAGED: (&str, &str, StatusCode) = (
    "speaker_owner_voice_reference_invalid",
    "that voice couldn't be saved because your owner voice reference needs attention.",
    StatusCode::CONFLICT,
);
const OWNER_IDENTITY_INVALID: (&str, &str, StatusCode) = (
    OWNER_IDENTITY_INVALID_REASON,
    "that speaker command couldn't run because your configured owner identity needs attention.",
    StatusCode::BAD_REQUEST,
);

pub async fn assign(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = match request_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let fields = match assign_fields(&body) {
        Ok(fields) => fields,
        Err(response) => return response,
    };
    let segment = match lookup_mutation_segment(&root.0, &fields) {
        Ok(path) => path,
        Err(response) => return response,
    };
    let _trust = match solstone_core_entity::hold_entity_trust_lock(&root.0) {
        Ok(lock) => lock,
        Err(error) => return write_error(error.to_string(), true),
    };
    let labels = match labels(&segment) {
        Some(value) => value,
        None => return review_unavailable(),
    };
    let current = label(&labels, fields.sentence_id);
    if let Err(response) = entity_allowed(&root.0, &fields.speaker) {
        return response;
    }
    if current.is_some_and(|row| {
        row.get("speaker").and_then(Value::as_str) == Some(fields.speaker.as_str())
            && row.get("method").and_then(Value::as_str) == Some("user_assigned")
    }) {
        let principal = admitted_owner(&root.0);
        let mut response = json!({"success":true,"status":"already_assigned"});
        if principal.as_deref() == Some(fields.speaker.as_str()) {
            response["owner_bootstrap_outcome"] = json!("not_attempted");
        }
        return Json(response).into_response();
    }
    if current
        .and_then(|row| row.get("speaker").and_then(Value::as_str))
        .is_some()
    {
        return err(
            "speaker_attribution_state_invalid",
            "that change couldn't be applied because the sentence isn't in the right state.",
            "Pick a sentence without a speaker.",
            StatusCode::BAD_REQUEST,
        );
    }
    if !sentence_exists(&segment, &fields.source, fields.sentence_id) {
        return sentence_missing("Pick a different sentence with an embedding.");
    }
    let embedding = match sentence_embedding(&segment, &fields.source, fields.sentence_id) {
        Some(value) => value,
        None => return sentence_missing("Pick a different sentence with an embedding."),
    };
    if let Err(response) = contamination_allowed(&root.0, &fields, &embedding) {
        return response;
    }
    let old_method = current
        .and_then(|row| row.get("method").and_then(Value::as_str))
        .map(str::to_owned);
    if let Err(error) = write_voiceprint(&root.0, &fields, embedding) {
        return write_error(error, false);
    }
    if let Err(error) = patch(
        &segment,
        fields.sentence_id,
        &fields.speaker,
        "user_assigned",
        true,
    ) {
        return write_error(error, true);
    }
    if let Err(error) = correction(
        &segment,
        fields.sentence_id,
        None,
        &fields.speaker,
        old_method.as_deref(),
    ) {
        return write_error(error, true);
    }
    if let Err(error) = action(
        &root.0,
        "attribution_assign",
        json!({"day":fields.day,"stream_layout":layout_name(fields.layout),"stream":fields.stream,"segment_key":fields.segment_key,"source":fields.source,"sentence_id":fields.sentence_id,"speaker":fields.speaker}),
    ) {
        return write_error(error, true);
    }
    let principal = admitted_owner(&root.0);
    let mut response = json!({"success":true,"status":"assigned","speaker":fields.speaker});
    if principal.as_deref() == Some(fields.speaker.as_str()) {
        owner_bootstrap_response(
            &mut response,
            crate::speakers_owner_write::bootstrap_owner_from_manual_tags(&root.0),
        );
    }
    Json(response).into_response()
}

pub async fn confirm(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = match request_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let fields = match common_fields(&body, false) {
        Ok(fields) => fields,
        Err(response) => return response,
    };
    let segment = match lookup_mutation_segment(&root.0, &fields) {
        Ok(path) => path,
        Err(response) => return response,
    };
    let _trust = match solstone_core_entity::hold_entity_trust_lock(&root.0) {
        Ok(lock) => lock,
        Err(error) => return write_error(error.to_string(), true),
    };
    let labels = match labels(&segment) {
        Some(value) => value,
        None => return review_unavailable(),
    };
    let Some(current) = label(&labels, fields.sentence_id) else {
        return sentence_missing("Sentence not found in labels");
    };
    let Some(speaker) = current.get("speaker").and_then(Value::as_str) else {
        return err(
            "speaker_attribution_state_invalid",
            "that change couldn't be applied because the sentence isn't in the right state.",
            "sentence has no speaker assignment yet",
            StatusCode::BAD_REQUEST,
        );
    };
    let speaker = speaker.to_owned();
    if let Err(response) = entity_allowed(&root.0, &speaker) {
        return response;
    }
    if current.get("confidence").and_then(Value::as_str) == Some("high")
        && current.get("method").and_then(Value::as_str) == Some("user_confirmed")
    {
        return Json(json!({"success":true,"status":"already_confirmed"})).into_response();
    }
    if current.get("confidence").and_then(Value::as_str) != Some("medium") {
        return err(
            "speaker_attribution_state_invalid",
            "that change couldn't be applied because the sentence isn't in the right state.",
            "attribution is not medium confidence",
            StatusCode::BAD_REQUEST,
        );
    }
    let embedding = match sentence_embedding(&segment, &fields.source, fields.sentence_id) {
        Some(value) => value,
        None => return sentence_missing("Sentence embedding not found"),
    };
    let target = Fields {
        speaker,
        ..fields.clone()
    };
    if let Err(response) = contamination_allowed(&root.0, &target, &embedding) {
        return response;
    }
    if let Err(error) = write_voiceprint(&root.0, &target, embedding) {
        return write_error(error, false);
    }
    let old_method = current.get("method").and_then(Value::as_str);
    if let Err(error) = patch(
        &segment,
        target.sentence_id,
        &target.speaker,
        "user_confirmed",
        false,
    ) {
        return write_error(error, true);
    }
    if let Err(error) = correction(
        &segment,
        target.sentence_id,
        Some(&target.speaker),
        &target.speaker,
        old_method,
    ) {
        return write_error(error, true);
    }
    if let Err(error) = action(
        &root.0,
        "attribution_confirm",
        json!({"day":target.day,"stream_layout":layout_name(target.layout),"stream":target.stream,"segment_key":target.segment_key,"source":target.source,"sentence_id":target.sentence_id,"speaker":target.speaker}),
    ) {
        return write_error(error, true);
    }
    maybe_bootstrap_owner(&root.0, &target.speaker);
    Json(json!({"success":true,"status":"confirmed","speaker":target.speaker})).into_response()
}

pub async fn correct(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = match request_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let fields = match common_fields(&body, true) {
        Ok(fields) => fields,
        Err(response) => return response,
    };
    let segment = match lookup_mutation_segment(&root.0, &fields) {
        Ok(path) => path,
        Err(response) => return response,
    };
    let _trust = match solstone_core_entity::hold_entity_trust_lock(&root.0) {
        Ok(lock) => lock,
        Err(error) => return write_error(error.to_string(), true),
    };
    if let Err(response) = entity_allowed(&root.0, &fields.speaker) {
        return response;
    };
    let labels = match labels(&segment) {
        Some(value) => value,
        None => return review_unavailable(),
    };
    let Some(current) = label(&labels, fields.sentence_id) else {
        return sentence_missing("Sentence not found in labels");
    };
    let old_speaker = current
        .get("speaker")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let old_method = current
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if old_speaker.as_deref() == Some(fields.speaker.as_str()) {
        return Json(json!({"success":true,"status":"already_correct"})).into_response();
    }
    let embedding = match sentence_embedding(&segment, &fields.source, fields.sentence_id) {
        Some(value) => value,
        None => return sentence_missing("Sentence embedding not found"),
    };
    if let Err(response) = contamination_allowed(&root.0, &fields, &embedding) {
        return response;
    }
    let removal = if let Some(old) = old_speaker.as_deref() {
        let key = json!({"day":fields.day,"segment_key":fields.segment_key,"source":fields.source,"sentence_id":fields.sentence_id});
        let rendered_key = format!(
            "{}/{}/{}#{}",
            fields.day, fields.segment_key, fields.source, fields.sentence_id
        );
        match solstone_core_speaker_resolve::direct_voiceprints::remove_voiceprint(
            &root.0,
            old,
            key,
            &encoder(),
        ) {
            Ok(report) => json!({
                "outcome": if report.removed_count == 0 { "not_found" } else if report.file_removed { "unlinked" } else { "removed" },
                "entity_id": old,
                "keys_removed": if report.removed_count == 0 { Vec::new() } else { vec![rendered_key] },
                "file_deleted": report.file_removed,
                "path": format!("entities/{old}/voiceprints.npz"),
            }),
            Err(error) => return write_error(error.to_string(), false),
        }
    } else {
        json!({"outcome":"not_found","entity_id":"","keys_removed":[],"file_deleted":false,"path":Value::Null})
    };
    if let Err(error) = write_voiceprint(&root.0, &fields, embedding) {
        return write_error(error, false);
    }
    if let Err(error) = patch(
        &segment,
        fields.sentence_id,
        &fields.speaker,
        "user_corrected",
        false,
    ) {
        return write_error(error, true);
    }
    if let Err(error) = correction(
        &segment,
        fields.sentence_id,
        old_speaker.as_deref(),
        &fields.speaker,
        old_method.as_deref(),
    ) {
        return write_error(error, true);
    }
    if let Err(error) = action(
        &root.0,
        "attribution_correct",
        json!({"day":fields.day,"stream_layout":layout_name(fields.layout),"stream":fields.stream,"segment_key":fields.segment_key,"source":fields.source,"sentence_id":fields.sentence_id,"old_speaker":old_speaker,"new_speaker":fields.speaker,"voiceprint_removal":removal}),
    ) {
        return write_error(error, true);
    }
    maybe_bootstrap_owner(&root.0, &fields.speaker);
    let propagation_offer = propagation_offer(&root.0, old_speaker.as_deref(), &fields.speaker);
    Json(json!({"success":true,"status":"corrected","old_speaker":old_speaker,"new_speaker":fields.speaker,"voiceprint_removal":removal,"propagation_offer":propagation_offer})).into_response()
}

fn propagation_offer(
    root: &std::path::Path,
    old_speaker: Option<&str>,
    new_speaker: &str,
) -> Value {
    let Some(old_speaker) = old_speaker else {
        return json!({
            "available":false,
            "reason":"no_old_speaker",
            "statement_count":0,
            "segment_count":0,
        });
    };
    let result = match propagate_speaker_correction(root, old_speaker, new_speaker, false) {
        Ok(result) => result,
        Err(PropagationError::OwnerIdentityInvalid) => {
            return json!({
                "available":false,
                "reason":OWNER_IDENTITY_INVALID_REASON,
                "statement_count":0,
                "segment_count":0,
            });
        }
        Err(PropagationError::UnsupportedLayout | PropagationError::Failed(_)) => {
            return json!({
                "available":false,
                "reason":"preview_failed",
                "statement_count":0,
                "segment_count":0,
            });
        }
    };
    let statement_count = result["statement_count"].as_u64().unwrap_or(0);
    let segment_count = result["segment_count"].as_u64().unwrap_or(0);
    if statement_count == 0 {
        return json!({
            "available":false,
            "reason":"no_changes",
            "statement_count":0,
            "segment_count":0,
        });
    }
    json!({
        "available":true,
        "statement_count":statement_count,
        "segment_count":segment_count,
        "route":"/app/speakers/api/propagate-correction",
        "request":{"old_speaker":old_speaker,"new_speaker":new_speaker,"commit":false},
    })
}

fn maybe_bootstrap_owner(root: &std::path::Path, speaker: &str) {
    if admitted_owner(root).as_deref() == Some(speaker) {
        let _ = crate::speakers_owner_write::bootstrap_owner_from_manual_tags(root);
    }
}

fn admitted_owner(root: &std::path::Path) -> Option<String> {
    match admitted_owner_id(root) {
        OwnerAdmission::Admitted(id) => Some(id),
        OwnerAdmission::Invalid => None,
    }
}

fn owner_bootstrap_response(response: &mut Value, result: Result<Value, String>) {
    let fields = match result {
        Ok(value)
            if value["status"] == "confirmed"
                && value.as_object().is_some_and(|object| {
                    object.len() == 4
                        && object.contains_key("status")
                        && object.contains_key("principal_id")
                        && object.contains_key("cluster_size")
                        && object.contains_key("evidence_tier")
                }) =>
        {
            json!({"owner_bootstrap_outcome":"built"})
        }
        Ok(value) if value["status"] == "confirmed" && value["next_step"] == "rebuild_owner" => {
            json!({"owner_bootstrap_outcome":"already_built"})
        }
        Ok(value) if value["status"] == "low_quality" => {
            let mut fields = json!({"owner_bootstrap_outcome":"refused"});
            if let Some(guidance) = value.get("guidance").and_then(Value::as_str) {
                fields["owner_bootstrap_guidance"] = json!(guidance);
            }
            fields
        }
        Ok(value) if value["error_kind"] == "voiceprint_busy" => {
            json!({"owner_bootstrap_outcome":"busy"})
        }
        Ok(value) if value["reason_code"] == "speaker_owner_identity_invalid" => {
            json!({"owner_bootstrap_outcome":"identity_invalid","owner_bootstrap_reason_code":"speaker_owner_identity_invalid"})
        }
        _ => json!({"owner_bootstrap_outcome":"failed"}),
    };
    response
        .as_object_mut()
        .expect("attribution response is an object")
        .extend(
            fields
                .as_object()
                .expect("bootstrap fields are an object")
                .clone(),
        );
}

/// Native propagation keeps the existing resolver and accumulation primitive; the full
/// segment-reprocessing policy is deliberately kept here rather than reopening the Python path.
pub async fn propagate(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = match request_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(old_speaker) = body
        .get("old_speaker")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return required("Missing required fields");
    };
    let Some(new_speaker) = body
        .get("new_speaker")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return required("Missing required fields");
    };
    if old_speaker == new_speaker {
        return err(
            "invalid_request_value",
            "one of those values couldn't be used.",
            "Choose two different speakers.",
            StatusCode::BAD_REQUEST,
        );
    }
    if let Err(response) = entity_allowed(&root.0, new_speaker) {
        return response;
    }
    let commit = body.get("commit").and_then(Value::as_bool).unwrap_or(false);
    let _trust = if commit {
        match solstone_core_entity::hold_entity_trust_lock(&root.0) {
            Ok(lock) => Some(lock),
            Err(error) => return write_error(error.to_string(), true),
        }
    } else {
        None
    };
    if admitted_owner(&root.0).is_none() {
        return owner_refusal(
            OWNER_IDENTITY_INVALID,
            "configured owner identity is not admitted",
        );
    }
    let result = match propagate_speaker_correction(&root.0, old_speaker, new_speaker, commit) {
        Ok(result) => result,
        Err(PropagationError::OwnerIdentityInvalid) => {
            return owner_refusal(
                OWNER_IDENTITY_INVALID,
                "configured owner identity is not admitted",
            );
        }
        Err(PropagationError::UnsupportedLayout) => {
            return err(
                UNSUPPORTED_LAYOUT_REASON,
                UNSUPPORTED_LAYOUT_MESSAGE,
                UNSUPPORTED_LAYOUT_DETAIL,
                StatusCode::BAD_REQUEST,
            );
        }
        Err(PropagationError::Failed(error)) => return write_error(error, true),
    };
    let statement_count = result["statement_count"].as_u64().unwrap_or(0);
    if commit
        && statement_count > 0
        && let Err(error) = action(
            &root.0,
            "attribution_propagate_correction",
            json!({"old_speaker":old_speaker,"new_speaker":new_speaker,"statement_count":statement_count,"segment_count":result["segment_count"]}),
        )
    {
        return write_error(error, true);
    }
    Json(result).into_response()
}

enum PropagationError {
    UnsupportedLayout,
    OwnerIdentityInvalid,
    Failed(String),
}

fn propagate_speaker_correction(
    root: &std::path::Path,
    old_speaker: &str,
    new_speaker: &str,
    commit: bool,
) -> Result<Value, PropagationError> {
    struct PreflightTarget {
        segment: solstone_core_speaker_resolve::segment_catalog::CatalogedSegment,
        current: Vec<Value>,
    }

    struct Target {
        segment: solstone_core_speaker_resolve::segment_catalog::CatalogedSegment,
        current: Vec<Value>,
        outcome: solstone_core_speaker_resolve::resolve::ResolveOutcome,
    }

    let catalog =
        catalog_journal(root).map_err(|error| PropagationError::Failed(error.to_string()))?;
    let mut preflight_targets = Vec::new();
    for segment in catalog {
        let labels_path = segment.path.join("talents/speaker_labels.json");
        let bytes = match fs::read(&labels_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(PropagationError::Failed(format!(
                    "failed to read {}: {error}",
                    labels_path.display()
                )));
            }
        };
        let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            PropagationError::Failed(format!("invalid labels {}: {error}", labels_path.display()))
        })?;
        let current = value
            .get("labels")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| {
                PropagationError::Failed(format!(
                    "invalid labels {}: labels must be an array",
                    labels_path.display()
                ))
            })?;
        if !current.iter().any(|label| {
            label.get("speaker").and_then(Value::as_str) == Some(old_speaker)
                || label.get("speaker").and_then(Value::as_str) == Some(new_speaker)
        }) {
            continue;
        }
        if segment.layout == SegmentLayout::Direct {
            return Err(PropagationError::UnsupportedLayout);
        }
        preflight_targets.push(PreflightTarget { segment, current });
    }

    let mut targets = Vec::with_capacity(preflight_targets.len());
    for PreflightTarget { segment, current } in preflight_targets {
        let outcome = solstone_core_speaker_resolve::resolve::resolve(
            root,
            &segment.day,
            &segment.stream,
            &segment.name,
            true,
            Utc::now().timestamp_millis(),
        )
        .map_err(|error| PropagationError::Failed(error.to_string()))?;
        match &outcome {
            solstone_core_speaker_resolve::resolve::ResolveOutcome::IdentityInvalid => {
                return Err(PropagationError::OwnerIdentityInvalid);
            }
            solstone_core_speaker_resolve::resolve::ResolveOutcome::NoOwnerCentroid if commit => {
                return Err(PropagationError::Failed(
                    "owner centroid unavailable".to_owned(),
                ));
            }
            solstone_core_speaker_resolve::resolve::ResolveOutcome::SegmentMissing if commit => {
                return Err(PropagationError::Failed(
                    "speaker propagation segment disappeared".to_owned(),
                ));
            }
            solstone_core_speaker_resolve::resolve::ResolveOutcome::Resolved(_)
            | solstone_core_speaker_resolve::resolve::ResolveOutcome::Empty { .. }
            | solstone_core_speaker_resolve::resolve::ResolveOutcome::NoOwnerCentroid
            | solstone_core_speaker_resolve::resolve::ResolveOutcome::SegmentMissing => {}
        }
        targets.push(Target {
            segment,
            current,
            outcome,
        });
    }

    let mut results = Vec::new();
    let mut changes = Vec::new();
    let mut errors = Vec::new();
    for Target {
        segment,
        current,
        outcome,
    } in targets
    {
        match outcome {
            solstone_core_speaker_resolve::resolve::ResolveOutcome::Resolved(output) => {
                let updated = crate::speakers_cli_maintenance::labels(&output);
                let segment_changes = propagation_changes(
                    &current,
                    &updated,
                    &segment.day,
                    segment.layout,
                    &segment.stream,
                    &segment.name,
                    output.source.as_deref(),
                );
                let accumulated = if commit && !segment_changes.is_empty() {
                    let metadata = crate::speakers_cli_maintenance::metadata(&output);
                    solstone_core_speaker_id::labels::write_full_labels(
                        &segment.path,
                        updated,
                        &metadata,
                    )
                    .map_err(|error| PropagationError::Failed(error.to_string()))?;
                    crate::speakers_cli_maintenance::accumulate(
                        root,
                        &segment.path,
                        &segment.day,
                        &segment.stream,
                        &segment.name,
                        &output,
                        Utc::now().timestamp_millis(),
                    )
                    .map_err(|error| PropagationError::Failed(error.to_string()))?
                } else {
                    json!({})
                };
                let changed_count = segment_changes.len();
                changes.extend(segment_changes.clone());
                results.push(json!({
                    "status": if changed_count > 0 { "changed" } else { "unchanged" },
                    "day":segment.day,
                    "stream_layout":"named",
                    "stream":segment.stream,
                    "segment_key":segment.name,
                    "source":output.source,
                    "changes":segment_changes,
                    "changed_count":changed_count,
                    "accumulated":accumulated,
                    "error":Value::Null,
                }));
            }
            solstone_core_speaker_resolve::resolve::ResolveOutcome::SegmentMissing => {
                results.push(json!({"status":"skipped","day":segment.day,"stream_layout":"named","stream":segment.stream,"segment_key":segment.name,"source":Value::Null,"changes":[],"changed_count":0,"accumulated":{},"error":Value::Null,"skip_reason":"segment_missing"}));
            }
            solstone_core_speaker_resolve::resolve::ResolveOutcome::Empty { source } => {
                results.push(json!({"status":"skipped","day":segment.day,"stream_layout":"named","stream":segment.stream,"segment_key":segment.name,"source":source,"changes":[],"changed_count":0,"accumulated":{},"error":Value::Null,"skip_reason":"no_embeddings"}));
            }
            solstone_core_speaker_resolve::resolve::ResolveOutcome::IdentityInvalid => {
                return Err(PropagationError::OwnerIdentityInvalid);
            }
            solstone_core_speaker_resolve::resolve::ResolveOutcome::NoOwnerCentroid => {
                let error = "owner centroid unavailable".to_owned();
                errors.push(format!(
                    "{}/{}/{}: {error}",
                    segment.day, segment.stream, segment.name
                ));
                results.push(json!({"status":"error","day":segment.day,"stream_layout":"named","stream":segment.stream,"segment_key":segment.name,"source":Value::Null,"changes":[],"changed_count":0,"accumulated":{},"error":error}));
            }
        }
    }
    let statement_count = changes.len();
    let segment_count = changes
        .iter()
        .filter_map(|change| {
            Some((
                change.get("day")?.as_str()?,
                change.get("stream_layout")?.as_str()?,
                change.get("stream")?.as_str()?,
                change.get("segment_key")?.as_str()?,
            ))
        })
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    Ok(json!({
        "status":if commit { "applied" } else { "preview" },
        "commit":commit,
        "old_speaker":old_speaker,
        "new_speaker":new_speaker,
        "segments_scanned":results.len(),
        "segments_considered":results.len(),
        "segment_count":segment_count,
        "statement_count":statement_count,
        "changes":changes,
        "segments":results,
        "errors":errors,
    }))
}

fn propagation_changes(
    current: &[Value],
    updated: &[Value],
    day: &str,
    layout: SegmentLayout,
    stream: &str,
    segment_key: &str,
    source: Option<&str>,
) -> Vec<Value> {
    let current = labels_by_sentence(current);
    let updated = labels_by_sentence(updated);
    current
        .keys()
        .chain(updated.keys())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|sentence_id| {
            let before = current.get(&sentence_id);
            let after = updated.get(&sentence_id);
            let (from_speaker, from_method, from_confidence) = label_fields(before.copied());
            let (to_speaker, to_method, to_confidence) = label_fields(after.copied());
            ((from_speaker, from_method, from_confidence) != (to_speaker, to_method, to_confidence))
                .then(|| {
                    json!({
                        "day":day,
                        "stream_layout":if layout == SegmentLayout::Direct { "direct" } else { "named" },
                        "stream":stream,
                        "segment_key":segment_key,
                        "source":source,
                        "sentence_id":sentence_id,
                        "from_speaker":from_speaker,
                        "to_speaker":to_speaker,
                        "from_method":from_method,
                        "to_method":to_method,
                        "from_confidence":from_confidence,
                        "to_confidence":to_confidence,
                    })
                })
        })
        .collect()
}

fn labels_by_sentence(labels: &[Value]) -> BTreeMap<i64, &Value> {
    labels
        .iter()
        .filter_map(|label| {
            label
                .get("sentence_id")
                .and_then(Value::as_i64)
                .map(|sentence_id| (sentence_id, label))
        })
        .collect()
}

fn label_fields(label: Option<&Value>) -> (Option<&str>, Option<&str>, Option<&str>) {
    label.map_or((None, None, None), |label| {
        (
            label.get("speaker").and_then(Value::as_str),
            label.get("method").and_then(Value::as_str),
            label.get("confidence").and_then(Value::as_str),
        )
    })
}

#[derive(Clone)]
struct Fields {
    day: String,
    layout: SegmentLayout,
    stream: String,
    segment_key: String,
    source: String,
    sentence_id: i64,
    speaker: String,
}

#[allow(clippy::result_large_err)]
fn assign_fields(value: &Value) -> Result<Fields, Response> {
    let object = value.as_object().ok_or_else(missing_body)?;
    let day = object
        .get("day")
        .ok_or_else(|| required("Missing required fields"))?;
    let stream = object
        .get("stream")
        .ok_or_else(|| required("Missing required fields"))?;
    let segment_key = object
        .get("segment_key")
        .ok_or_else(|| required("Missing required fields"))?;
    let source = object
        .get("source")
        .ok_or_else(|| required("Missing required fields"))?;
    let sentence_id = object
        .get("sentence_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| required("Missing required fields"))?;
    let speaker = object
        .get("speaker")
        .map(ToString::to_string)
        .map(|value| value.trim_matches('"').to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| required("Missing required fields"))?;
    let Some(day) = day.as_str() else {
        return Err(invalid_day(
            "Use a valid day, stream, and segment, then pick a sentence.",
        ));
    };
    let Some(segment_key) = segment_key.as_str() else {
        return Err(invalid_segment_or_stream(
            "Use a valid day, stream, and segment, then pick a sentence.",
        ));
    };
    let Some(stream) = stream.as_str() else {
        return Err(invalid_segment_or_stream(
            "Use a valid day, stream, and segment, then pick a sentence.",
        ));
    };
    let Some(source) = source.as_str() else {
        return Err(err(
            "internal_error",
            "that request didn't finish.",
            "source was not a string",
            StatusCode::INTERNAL_SERVER_ERROR,
        ));
    };
    if !valid_day(day) {
        return Err(invalid_day(
            "Use a valid day, stream, and segment, then pick a sentence.",
        ));
    }
    if stream != DEFAULT_STREAM && !valid_stream(stream) {
        return Err(invalid_segment_or_stream(
            "Use a valid day, stream, and segment, then pick a sentence.",
        ));
    }
    let layout = decode_stream_layout_value(object.get("stream_layout")).map_err(|_| {
        invalid_segment_or_stream("Use a valid day, stream, and segment, then pick a sentence.")
    })?;
    Ok(Fields {
        day: day.to_owned(),
        layout,
        stream: stream.to_owned(),
        segment_key: segment_key.to_owned(),
        source: source.to_owned(),
        sentence_id,
        speaker,
    })
}
#[allow(clippy::result_large_err)]
fn common_fields(value: &Value, correction: bool) -> Result<Fields, Response> {
    let object = value.as_object().ok_or_else(missing_body)?;
    let day = object
        .get("day")
        .cloned()
        .ok_or_else(|| required("Missing required fields"))?;
    let stream = object
        .get("stream")
        .cloned()
        .ok_or_else(|| required("Missing required fields"))?;
    let segment_key = object
        .get("segment_key")
        .cloned()
        .ok_or_else(|| required("Missing required fields"))?;
    let source = object
        .get("source")
        .cloned()
        .ok_or_else(|| required("Missing required fields"))?;
    let sentence_id = object
        .get("sentence_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| required("Missing required fields"))?;
    // Python's confirm/correct regex calls on non-strings raise; retain the 500 class rather than
    // converting those malformed values into a normal validation refusal.
    let (Some(day), Some(stream), Some(segment_key), Some(source)) = (
        day.as_str(),
        stream.as_str(),
        segment_key.as_str(),
        source.as_str(),
    ) else {
        return Err(err(
            "internal_error",
            "that request didn't finish.",
            "regex input was not a string",
            StatusCode::INTERNAL_SERVER_ERROR,
        ));
    };
    if !valid_day(day) {
        return Err(invalid_day("Invalid day format"));
    }
    if stream != DEFAULT_STREAM && !valid_stream(stream) {
        return Err(invalid_segment_or_stream("Invalid stream"));
    }
    let layout = decode_stream_layout_value(object.get("stream_layout"))
        .map_err(|_| invalid_segment_or_stream("Invalid stream layout"))?;
    let speaker = if correction {
        object
            .get("new_speaker")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| required("Missing required fields"))?
            .to_owned()
    } else {
        String::new()
    };
    Ok(Fields {
        day: day.to_owned(),
        layout,
        stream: stream.to_owned(),
        segment_key: segment_key.to_owned(),
        source: source.to_owned(),
        sentence_id,
        speaker,
    })
}

#[allow(clippy::result_large_err)]
fn contamination_allowed(
    root: &std::path::Path,
    fields: &Fields,
    embedding: &[f32],
) -> Result<(), Response> {
    let owner = admitted_owner(root);
    if owner.as_deref() == Some(fields.speaker.as_str()) {
        return Ok(());
    }
    if normalize_embedding(embedding).is_none() {
        return Err(sentence_missing("Sentence embedding not found"));
    }
    let probe = ContaminationProbe {
        day: fields.day.clone(),
        stream: fields.stream.clone(),
        segment_key: fields.segment_key.clone(),
        source: fields.source.clone(),
        sentence_id: fields.sentence_id,
    };
    match screen_owner_contamination(root, &probe, &encoder()) {
        Ok(ContaminationScreen::Clear { .. }) => Ok(()),
        Ok(ContaminationScreen::Contaminated { .. }) => Err(owner_refusal(
            OWNER_TOO_CLOSE,
            "Embedding too similar to owner voice; cannot save",
        )),
        Ok(ContaminationScreen::Indeterminate { reason }) => {
            match classify_indeterminate(&reason) {
                Ok(class) => Err(owner_refusal(class, &reason)),
                Err(response) => Err(response),
            }
        }
        Err(error) => Err(owner_refusal(OWNER_DAMAGED, &error.to_string())),
    }
}

#[allow(clippy::result_large_err)]
fn classify_indeterminate(
    reason: &str,
) -> Result<(&'static str, &'static str, StatusCode), Response> {
    if reason == "speaker_owner_identity_invalid" {
        return Ok(OWNER_IDENTITY_INVALID);
    }
    if let Some(tier_reason) = OwnerTierReason::ALL
        .iter()
        .copied()
        .find(|tier_reason| tier_reason.wire_str() == reason)
    {
        return Ok(classify_owner_tier(tier_reason));
    }
    match reason {
        // The sentence lookup and normalization checks run before the screen, so these
        // responses should be unreachable unless the sidecar changed underneath us.
        "probe_not_found" | "probe_zero_norm" => Ok(OWNER_DAMAGED),
        _ => Err(err(
            "internal_error",
            "that request didn't finish.",
            &format!("unknown owner-contamination reason: {reason}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

fn classify_owner_tier(reason: OwnerTierReason) -> (&'static str, &'static str, StatusCode) {
    match reason {
        OwnerTierReason::ConfirmedAbsent
        | OwnerTierReason::VoiceprintsAbsent
        | OwnerTierReason::BelowRowFloor
        | OwnerTierReason::BelowEmbeddingFloor => OWNER_NOT_ENOUGH,
        OwnerTierReason::ConfirmedUnreadable
        | OwnerTierReason::ConfirmedIncomplete
        | OwnerTierReason::ConfirmedZeroNorm
        | OwnerTierReason::VoiceprintsUnreadable
        | OwnerTierReason::ProvisionalZeroNorm => OWNER_DAMAGED,
    }
}

fn write_voiceprint(
    root: &std::path::Path,
    fields: &Fields,
    embedding: Vec<f32>,
) -> Result<(), String> {
    solstone_core_speaker_resolve::direct_voiceprints::write_voiceprint(root, &fields.speaker, embedding, json!({"day":fields.day,"stream_layout":layout_name(fields.layout),"stream":fields.stream,"segment_key":fields.segment_key,"source":fields.source,"sentence_id":fields.sentence_id}), &encoder()).map_err(|error| error.to_string())
}
fn patch(
    segment: &std::path::Path,
    sentence_id: i64,
    speaker: &str,
    method: &str,
    allow_insert: bool,
) -> Result<(), String> {
    let mut patch = Map::new();
    patch.insert("speaker".to_owned(), json!(speaker));
    patch.insert("confidence".to_owned(), json!("high"));
    patch.insert("method".to_owned(), json!(method));
    solstone_core_speaker_id::labels::patch_labels(segment, &[(sentence_id, patch)], allow_insert)
        .map_err(|error| error.to_string())
}
fn correction(
    segment: &std::path::Path,
    sentence_id: i64,
    original_speaker: Option<&str>,
    corrected_speaker: &str,
    original_method: Option<&str>,
) -> Result<(), String> {
    solstone_core_speaker_id::corrections::append_correction(segment, json!({"sentence_id":sentence_id,"original_speaker":original_speaker,"corrected_speaker":corrected_speaker,"original_method":original_method,"timestamp":Utc::now().timestamp_millis()}).as_object().expect("correction is object").clone()).map_err(|error| error.to_string())
}
#[allow(clippy::result_large_err)]
pub(crate) fn entity_allowed(root: &std::path::Path, speaker: &str) -> Result<(), Response> {
    let entity = solstone_core_entity::load_all_journal_entities(root)
        .ok()
        .into_iter()
        .flatten()
        .find(|entity| entity.id == speaker);
    match entity {
        Some(entity) if entity.is_blocked() => Err(err(
            "entity_blocked",
            "that speaker couldn't be used because it's blocked.",
            &format!("Entity '{speaker}' is blocked"),
            StatusCode::BAD_REQUEST,
        )),
        Some(entity) if !solstone_core_entity::is_admissible_person(&entity) => Err(err(
            "speaker_not_person",
            "that speaker couldn't be used because it isn't a Person.",
            &format!("Entity '{speaker}' is not a Person. Select an existing, unblocked Person."),
            StatusCode::BAD_REQUEST,
        )),
        Some(_) => Ok(()),
        None => Err(err(
            "speaker_not_found",
            "that speaker couldn't be found.",
            &format!("Entity '{speaker}' not found"),
            StatusCode::NOT_FOUND,
        )),
    }
}
fn labels(segment: &std::path::Path) -> Option<Vec<Value>> {
    fs::read(segment.join("talents/speaker_labels.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.get("labels").and_then(Value::as_array).cloned())
}
fn label(rows: &[Value], sentence_id: i64) -> Option<&Value> {
    rows.iter()
        .find(|row| row.get("sentence_id").and_then(Value::as_i64) == Some(sentence_id))
}
fn sentence_exists(segment: &std::path::Path, source: &str, sentence_id: i64) -> bool {
    fs::read_to_string(segment.join(format!("{source}.jsonl")))
        .ok()
        .is_some_and(|body| {
            body.lines().any(|line| {
                serde_json::from_str::<Value>(line)
                    .ok()
                    .and_then(|value| value.get("id").and_then(Value::as_i64))
                    .is_some_and(|id| id == sentence_id)
            })
        })
}
fn sentence_embedding(
    segment: &std::path::Path,
    source: &str,
    sentence_id: i64,
) -> Option<Vec<f32>> {
    solstone_core_speaker_id::embeddings::load_embeddings_file(
        &segment.join(format!("{source}.npz")),
    )
    .ok()
    .flatten()
    .and_then(|file| {
        file.statements
            .into_iter()
            .find_map(|(id, embedding)| (id == sentence_id).then_some(embedding))
    })
    .filter(|embedding| normalize_embedding(embedding).is_some())
}
#[allow(clippy::result_large_err)]
fn lookup_mutation_segment(
    root: &std::path::Path,
    fields: &Fields,
) -> Result<std::path::PathBuf, Response> {
    match lookup_segment(
        root,
        &fields.day,
        &fields.stream,
        &fields.segment_key,
        Ok(fields.layout),
        DirectSupport::Refuse,
    ) {
        SegmentLookup::Present(path) => Ok(path),
        SegmentLookup::UnsupportedLayout => Err(err(
            UNSUPPORTED_LAYOUT_REASON,
            UNSUPPORTED_LAYOUT_MESSAGE,
            UNSUPPORTED_LAYOUT_DETAIL,
            StatusCode::BAD_REQUEST,
        )),
        SegmentLookup::MalformedLayout => {
            Err(invalid_segment_or_stream("Invalid segment key or stream"))
        }
        SegmentLookup::Absent => Err(review_unavailable()),
        SegmentLookup::Failed(error) => Err(write_error(error.to_string(), true)),
    }
}
fn layout_name(layout: SegmentLayout) -> &'static str {
    match layout {
        SegmentLayout::Direct => "direct",
        SegmentLayout::Named => "named",
    }
}
fn valid_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}
fn valid_stream(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}
fn invalid_day(detail: &str) -> Response {
    err(
        "invalid_day",
        "that day couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
}
fn invalid_segment_or_stream(detail: &str) -> Response {
    err(
        "invalid_segment_or_stream",
        "that segment or stream couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
}
fn encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "unresolved".to_owned(),
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        width: 256,
    }
}
pub(crate) fn action(root: &std::path::Path, action: &str, params: Value) -> Result<(), String> {
    solstone_core_facets::append_action_log(root, None, "app", "speakers", action, params)
        .map_err(|error| error.to_string())
}
async fn request_json(request: Request) -> Result<Value, Response> {
    let bytes = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| missing_body())?;
    if bytes.is_empty() {
        return Err(missing_body());
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        err(
            "invalid_json_request",
            "that JSON request couldn't be read.",
            "request body must be a JSON object",
            StatusCode::BAD_REQUEST,
        )
    })?;
    if value.is_null() {
        Err(missing_body())
    } else {
        Ok(value)
    }
}
fn owner_refusal(reason: (&str, &str, StatusCode), detail: &str) -> Response {
    err(reason.0, reason.1, detail, reason.2)
}
fn sentence_missing(detail: &str) -> Response {
    err(
        "speaker_sentence_missing",
        "that sentence couldn't be found. try refreshing the page.",
        detail,
        StatusCode::NOT_FOUND,
    )
}
fn review_unavailable() -> Response {
    err(
        "speaker_review_unavailable",
        "that speaker review couldn't be loaded.",
        "No speaker labels found",
        StatusCode::NOT_FOUND,
    )
}
fn missing_body() -> Response {
    err(
        "missing_request_body",
        "that request had no data in it.",
        "No data provided",
        StatusCode::BAD_REQUEST,
    )
}
fn required(detail: &str) -> Response {
    err(
        "missing_required_field",
        "a required field is missing.",
        detail,
        StatusCode::BAD_REQUEST,
    )
}
fn write_error(detail: String, labels: bool) -> Response {
    if detail.contains("busy") || detail.contains("lock") {
        let (code, message) = if labels {
            (
                "speaker_labels_busy",
                "those speaker attributions couldn't be updated right now because they were busy. try again in a moment.",
            )
        } else {
            (
                "speaker_voiceprint_busy",
                "that voice couldn't be updated right now because it was busy. try again in a moment.",
            )
        };
        err(code, message, &detail, StatusCode::SERVICE_UNAVAILABLE)
    } else {
        err(
            "speaker_command_failed",
            "that speaker command didn't finish.",
            &detail,
            StatusCode::BAD_REQUEST,
        )
    }
}
fn err(code: &str, message: &str, detail: &str, status: StatusCode) -> Response {
    error_envelope(code, message, detail, status).into_response()
}

#[cfg(test)]
mod tests {
    use super::{
        Fields, OWNER_DAMAGED, OWNER_IDENTITY_INVALID, OWNER_NOT_ENOUGH, classify_indeterminate,
        classify_owner_tier, contamination_allowed, propagation_offer,
    };
    use axum::body::to_bytes;
    use serde_json::{Value, json};
    use solstone_core_journal_io::SegmentLayout;
    use solstone_core_speaker_resolve::OWNER_IDENTITY_INVALID_REASON;
    use solstone_core_speaker_resolve::owner_provisional::OwnerTierReason;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestJournal(PathBuf);

    impl TestJournal {
        fn owner_identity_invalid() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "solstone-propagation-offer-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(root.join("entities/owner")).expect("owner directory");
            fs::create_dir_all(root.join("entities/new")).expect("new directory");
            fs::create_dir_all(root.join("chronicle/20260808/main/120000_1/talents"))
                .expect("segment directory");
            fs::write(
                root.join("entities/owner/entity.json"),
                json!({"id":"owner","name":"Owner","is_principal":true}).to_string(),
            )
            .expect("invalid owner entity");
            fs::write(
                root.join("entities/new/entity.json"),
                json!({"id":"new","name":"New","type":"Person"}).to_string(),
            )
            .expect("new entity");
            fs::write(
                root.join("chronicle/20260808/main/120000_1/talents/speaker_labels.json"),
                json!({"labels":[{"sentence_id":1,"speaker":"old"}]}).to_string(),
            )
            .expect("labels");
            Self(root)
        }
    }

    impl Drop for TestJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn snapshot_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn collect(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(directory).expect("journal directory") {
                let path = entry.expect("journal entry").path();
                if path.is_dir() {
                    collect(root, &path, files);
                } else if path.is_file() {
                    files.insert(
                        path.strip_prefix(root)
                            .expect("journal-relative file")
                            .to_path_buf(),
                        fs::read(path).expect("journal file"),
                    );
                }
            }
        }

        let mut files = BTreeMap::new();
        collect(root, root, &mut files);
        files
    }

    #[test]
    fn every_indeterminate_owner_tier_is_refused_by_an_exhaustive_mapping() {
        let expected = [
            OWNER_NOT_ENOUGH,
            OWNER_DAMAGED,
            OWNER_DAMAGED,
            OWNER_DAMAGED,
            OWNER_NOT_ENOUGH,
            OWNER_DAMAGED,
            OWNER_NOT_ENOUGH,
            OWNER_NOT_ENOUGH,
            OWNER_DAMAGED,
        ];
        assert_eq!(
            OwnerTierReason::ALL.map(classify_owner_tier),
            expected,
            "a new tier reason must be deliberately assigned to a refusal class"
        );
        for reason in [
            "confirmed_absent",
            "confirmed_unreadable",
            "confirmed_incomplete",
            "confirmed_zero_norm",
            "voiceprints_absent",
            "voiceprints_unreadable",
            "below_row_floor",
            "below_embedding_floor",
            "provisional_zero_norm",
        ] {
            let class = classify_indeterminate(reason).expect("known reason maps");
            assert!(class == OWNER_NOT_ENOUGH || class == OWNER_DAMAGED);
            assert_ne!(class.0, "speaker_owner_voice_too_close");
        }
        assert!(matches!(
            classify_indeterminate("speaker_owner_identity_invalid"),
            Ok(class) if class == OWNER_IDENTITY_INVALID
        ));
        assert!(classify_indeterminate("unknown_reason").is_err());
    }

    #[tokio::test]
    async fn propagation_offer_preserves_an_owner_identity_refusal() {
        // `correct` refuses at contamination_allowed before it asks for this offer, so the
        // helper is the only reachable level that can prove this distinct offer reason.
        let journal = TestJournal::owner_identity_invalid();
        let before = snapshot_files(&journal.0);

        let offer = propagation_offer(&journal.0, Some("old"), "new");
        assert_eq!(offer["available"], false, "{offer}");
        assert_eq!(offer["reason"], OWNER_IDENTITY_INVALID_REASON, "{offer}");
        assert_eq!(snapshot_files(&journal.0), before);

        let fields = Fields {
            day: "20260808".to_owned(),
            layout: SegmentLayout::Named,
            stream: "main".to_owned(),
            segment_key: "120000_1".to_owned(),
            source: "audio".to_owned(),
            sentence_id: 1,
            speaker: "new".to_owned(),
        };
        let response = contamination_allowed(&journal.0, &fields, &[1.0; 256])
            .expect_err("the direct correction gate remains independently refused");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("refusal body"),
        )
        .expect("refusal json");
        assert_eq!(body["reason_code"], OWNER_IDENTITY_INVALID_REASON);
        assert_eq!(snapshot_files(&journal.0), before);
    }
}
