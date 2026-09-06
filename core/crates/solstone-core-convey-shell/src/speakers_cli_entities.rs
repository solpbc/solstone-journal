// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! CLI journal-entity operations used by speakers.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;

use crate::JournalRoot;

pub async fn merge_names(
    Extension(root): Extension<Arc<JournalRoot>>,
    request: Request,
) -> Response {
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(alias) = body.get("alias").and_then(Value::as_str) else {
        return err(
            "missing_required_field",
            "alias is required",
            StatusCode::BAD_REQUEST,
        );
    };
    let Some(canonical) = body.get("canonical").and_then(Value::as_str) else {
        return err(
            "missing_required_field",
            "canonical is required",
            StatusCode::BAD_REQUEST,
        );
    };
    match solstone_core_speaker_resolve::bootstrap::merge_names(&root.0, alias, canonical) {
        Ok(solstone_core_speaker_resolve::bootstrap::MergeNamesOutcome::Ambiguous {
            field,
            ambiguity_id,
            candidates,
        }) => err_value(
            "speaker_command_failed",
            json!({"field":field,"ambiguity_id":ambiguity_id,"candidates":format!("{candidates:?}")}),
            StatusCode::BAD_REQUEST,
        ),
        Ok(solstone_core_speaker_resolve::bootstrap::MergeNamesOutcome::AliasNotFound) => {
            err("speaker_not_found", "alias", StatusCode::NOT_FOUND)
        }
        Ok(solstone_core_speaker_resolve::bootstrap::MergeNamesOutcome::CanonicalNotFound) => {
            err("speaker_not_found", "canonical", StatusCode::NOT_FOUND)
        }
        Ok(solstone_core_speaker_resolve::bootstrap::MergeNamesOutcome::SameEntity {
            entity_id,
        }) => err("invalid_request_value", &entity_id, StatusCode::BAD_REQUEST),
        Ok(solstone_core_speaker_resolve::bootstrap::MergeNamesOutcome::PrincipalEntity {
            entity_id,
        }) => err(
            "principal_entity_protected",
            &entity_id,
            StatusCode::BAD_REQUEST,
        ),
        Ok(solstone_core_speaker_resolve::bootstrap::MergeNamesOutcome::Ready {
            alias_entity_id,
            canonical_entity_id,
        }) => {
            let encoder = encoder();
            match solstone_core_entity::commit_entity_merge(
                &root.0,
                &alias_entity_id,
                &canonical_entity_id,
                solstone_core_entity::EntityMergeOptions {
                    keep_source_as_aka: true,
                },
                &encoder,
            ) {
                Ok(report) => {
                    let counts = &report.counts;
                    Json(json!({
                        "merged": true,
                        "alias": alias,
                        "alias_id": alias_entity_id,
                        "canonical_name": canonical,
                        "canonical_id": canonical_entity_id,
                        "akas_added": report.aliases_added,
                        "voiceprints_merged": counts.pointer("/voiceprints/added").and_then(Value::as_u64),
                        "voiceprints_total": counts.pointer("/voiceprints/target_total").and_then(Value::as_u64),
                        "facets_merged": counts.pointer("/facets/merged").and_then(Value::as_u64),
                        "facets_moved": counts.pointer("/facets/moved").and_then(Value::as_u64),
                        "segments_scanned": counts.pointer("/segments/files_scanned").and_then(Value::as_u64),
                        "labels_rewritten": counts.pointer("/segments/labels_rewritten").and_then(Value::as_u64),
                        "corrections_rewritten": counts.pointer("/segments/corrections_rewritten").and_then(Value::as_u64),
                        "errors": counts.pointer("/segments/errors").cloned().unwrap_or_else(|| json!([])),
                    })).into_response()
                }
                Err(error) => merge_error(error),
            }
        }
        Err(error) => err(
            "speaker_command_failed",
            &error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

pub async fn link_import(
    Extension(root): Extension<Arc<JournalRoot>>,
    request: Request,
) -> Response {
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(name) = body
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return err(
            "missing_required_field",
            "name is required",
            StatusCode::BAD_REQUEST,
        );
    };
    let Some(entity_id) = body
        .get("entity_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return err(
            "missing_required_field",
            "entity_id is required",
            StatusCode::BAD_REQUEST,
        );
    };
    let entities = match solstone_core_entity::load_all_journal_entities(&root.0) {
        Ok(entities) => entities,
        Err(error) => {
            return err(
                "entity_operation_failed",
                &error.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let Some(target) = entities.iter().find(|entity| entity.id == entity_id) else {
        return err("entity_not_found", entity_id, StatusCode::NOT_FOUND);
    };
    let candidates = entities
        .iter()
        .filter(|entity| entity.id != entity_id)
        .map(
            |entity| solstone_core_entity_matching::EntityNameCandidate {
                id: Some(entity.id.clone()),
                name: entity
                    .value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                aka: entity
                    .value
                    .get("aka")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                emails: entity
                    .value
                    .get("emails")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            },
        )
        .collect::<Vec<_>>();
    if matches!(
        solstone_core_entity_matching::find_matching_entity_detailed(name, &candidates, 90.0),
        solstone_core_entity_matching::EntityNameMatchOutcome::Matched { .. }
            | solstone_core_entity_matching::EntityNameMatchOutcome::Ambiguous { .. }
    ) {
        return err("entity_alias_conflict", name, StatusCode::CONFLICT);
    }
    let mut identity = target.value.clone();
    let object = identity
        .as_object_mut()
        .expect("journal identity is an object");
    let mut aka = object
        .get("aka")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let already_present = aka.iter().any(|value| value.as_str() == Some(name));
    if !already_present {
        aka.push(json!(name));
        aka.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
        object.insert("aka".to_owned(), Value::Array(aka));
        object.insert(
            "updated_at".to_owned(),
            json!(Utc::now().timestamp_millis()),
        );
        if let Err(error) =
            solstone_core_entity::save_entity_identity(&root.0, entity_id, &identity, None)
        {
            return err(
                "entity_operation_failed",
                &error.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    }
    Json(json!({"linked":true,"entity_id":entity_id,"name_added":name,"already_present":already_present})).into_response()
}

async fn json_body(request: Request) -> Result<Value, Response> {
    let bytes = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| {
            err(
                "missing_request_body",
                "unable to read request body",
                StatusCode::BAD_REQUEST,
            )
        })?;
    serde_json::from_slice(&bytes).map_err(|_| {
        err(
            "invalid_json_request",
            "request body must be a JSON object",
            StatusCode::BAD_REQUEST,
        )
    })
}
fn encoder() -> solstone_core_entity::EncoderIdentity {
    solstone_core_entity::EncoderIdentity {
        id: "unresolved".to_owned(),
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        width: 256,
    }
}
fn err(code: &str, detail: &str, status: StatusCode) -> Response {
    error_envelope(code, "that speaker command didn't finish.", detail, status).into_response()
}
fn err_value(code: &str, detail: Value, status: StatusCode) -> Response {
    err(code, &detail.to_string(), status)
}
fn merge_error(error: solstone_core_entity::EntityMergeError) -> Response {
    let detail = error.to_string();
    if detail.contains("lock") || detail.contains("busy") {
        err("entity_busy", &detail, StatusCode::SERVICE_UNAVAILABLE)
    } else if detail.contains("not found") {
        err("entity_not_found", &detail, StatusCode::NOT_FOUND)
    } else if matches!(
        error,
        solstone_core_entity::EntityMergeError::Refused(_)
            | solstone_core_entity::EntityMergeError::VoiceprintEncoderMismatch { .. }
    ) {
        err(
            "invalid_operation_for_state",
            &detail,
            StatusCode::BAD_REQUEST,
        )
    } else {
        err(
            "entity_operation_failed",
            &detail,
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    }
}
