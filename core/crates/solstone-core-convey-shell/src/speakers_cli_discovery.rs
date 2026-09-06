// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! CLI discovery identify routes and operation-ledger projections.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{Path as RoutePath, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;

use crate::JournalRoot;
use crate::speakers_discovery_write::{IdentifyPreflightError, preflight_identify_cluster};
use solstone_core_speaker_resolve::segment_catalog::{
    UNSUPPORTED_LAYOUT_DETAIL, UNSUPPORTED_LAYOUT_MESSAGE, UNSUPPORTED_LAYOUT_REASON,
};

pub async fn identify(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = match request_json(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(cluster_id) = body.get("cluster_id").and_then(Value::as_i64) else {
        return bad("missing_required_field", "cluster_id is required");
    };
    let name = body.get("name").and_then(Value::as_str).map(str::to_owned);
    let entity_id = body
        .get("entity_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if name.as_deref().is_none_or(str::is_empty) && entity_id.as_deref().is_none_or(str::is_empty) {
        return bad("missing_required_field", "name or entity_id is required");
    }
    match preflight_identify_cluster(&root.0, cluster_id) {
        Ok(()) => {}
        Err(IdentifyPreflightError::UnsupportedLayout) => {
            return err(
                UNSUPPORTED_LAYOUT_REASON,
                UNSUPPORTED_LAYOUT_MESSAGE,
                UNSUPPORTED_LAYOUT_DETAIL,
                StatusCode::BAD_REQUEST,
            );
        }
        Err(IdentifyPreflightError::Failed(detail)) => {
            return err(
                "speaker_command_failed",
                "that speaker command didn't finish.",
                &detail,
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    }
    let request_id = body
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("native-cli")
        .to_owned();
    let reviewed = body
        .get("reviewed_near_match_entity_ids")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let request = solstone_core_speaker_resolve::identify_cluster::IdentifyClusterRequest {
        journal_root: root.0.clone(),
        cluster_id,
        name,
        entity_id,
        resolve_only: body
            .get("resolve_only")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        create_new: body
            .get("create_new")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        entity_type: body
            .get("entity_type")
            .and_then(Value::as_str)
            .unwrap_or("Person")
            .to_owned(),
        request_id,
        reviewed_near_match_entity_ids: reviewed,
        caller: "apps.speakers.cli.identify".to_owned(),
        actor: None,
    };
    let encoder = encoder();
    match solstone_core_speaker_resolve::identify_cluster::identify_cluster(&request, &encoder) {
        Ok(value) => identify_response(value),
        Err(error) => err(
            "speaker_command_failed",
            "that speaker command didn't finish.",
            &error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

pub async fn operations(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    let path = root.0.join("speakers/identify-operations.jsonl");
    let rows = match solstone_core_speaker_resolve::identify_operations::load_operations(&path) {
        Ok(rows) => rows,
        Err(error) => {
            return err(
                "speaker_command_failed",
                "that speaker command didn't finish.",
                &error.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    match solstone_core_speaker_resolve::identify_operations::fold_all_operations(&rows) {
        Ok(states) => {
            let operations = states.iter().map(summary).collect::<Vec<_>>();
            Json(json!({"operations":operations,"total":operations.len()})).into_response()
        }
        Err(error) => err(
            "speaker_command_failed",
            "that speaker command didn't finish.",
            &error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

pub async fn operation(
    Extension(root): Extension<Arc<JournalRoot>>,
    RoutePath(operation_id): RoutePath<String>,
) -> Response {
    let path = root.0.join("speakers/identify-operations.jsonl");
    let rows = match solstone_core_speaker_resolve::identify_operations::load_operations(&path) {
        Ok(rows) => rows,
        Err(error) => {
            return err(
                "speaker_command_failed",
                "that speaker command didn't finish.",
                &error.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    match solstone_core_speaker_resolve::identify_operations::fold_operation(&rows, &operation_id) {
        Ok(Some(state)) => Json(json!({"operation":summary(&state)})).into_response(),
        Ok(None) => err(
            "speaker_identify_operation_not_found",
            "that speaker identify operation couldn't be found.",
            &format!("operation_id={operation_id}"),
            StatusCode::NOT_FOUND,
        ),
        Err(error) => err(
            "speaker_command_failed",
            "that speaker command didn't finish.",
            &error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

fn summary(state: &solstone_core_speaker_resolve::identify_operations::OperationState) -> Value {
    json!({"operation_id":state.operation_id,"request_id":state.request_id,"status":format!("{:?}",state.terminal_status).to_lowercase(),"target_entity_id":state.target_entity_id,"will_create":state.will_create,"entity_type":state.entity_type,"reviewed_near_match_entity_ids":state.reviewed_near_match_entity_ids,"cluster_member_count":state.cluster_member_set.len(),"completed_phases":state.completed_phases.iter().map(|phase| phase.as_str()).collect::<Vec<_>>(),"pending_phases":state.pending_phases,"checkpoints":{"forward":format!("{:?}",state.phase_checkpoints),"undo":format!("{:?}",state.undo_phase_checkpoints)},"result":state.result,"undo_report":state.undo_report,"repair":state.repair_required.as_ref().map(|event| event.to_json())})
}

fn identify_response(value: Value) -> Response {
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("error");
    match status {
        "identified" | "resolved" | "ambiguous" | "no_match" | "principal_match" | "undone"
        | "already_undone" => Json(value).into_response(),
        "recoverable" | "in_progress" | "undoing" => err(
            "speaker_identify_recoverable",
            "that speaker identify operation didn't finish, but it can be retried.",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "repair_required" | "undo_repair_required" => err(
            "speaker_identify_repair_required",
            "that speaker identify operation couldn't finish safely without repair.",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "conflict" | "operation_already_undone" => err(
            "speaker_identify_conflict",
            "that speaker identify operation couldn't run because it conflicts with existing state.",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "not_found" => err(
            "speaker_not_found",
            "that speaker couldn't be found.",
            &value.to_string(),
            StatusCode::NOT_FOUND,
        ),
        _ => err(
            "speaker_command_failed",
            "that speaker command didn't finish.",
            &value.to_string(),
            StatusCode::BAD_REQUEST,
        ),
    }
}

async fn request_json(request: Request) -> Result<Value, Response> {
    let bytes = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| bad("missing_request_body", "unable to read request body"))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| bad("invalid_json_request", "request body must be a JSON object"))
}
fn encoder() -> solstone_core_entity::EncoderIdentity {
    solstone_core_entity::EncoderIdentity {
        id: "unresolved".to_owned(),
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        width: 256,
    }
}
fn bad(code: &str, detail: &str) -> Response {
    err(
        code,
        "that request couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
}
fn err(code: &str, message: &str, detail: &str, status: StatusCode) -> Response {
    error_envelope(code, message, detail, status).into_response()
}
