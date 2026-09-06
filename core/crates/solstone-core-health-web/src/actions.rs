// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::brain;
use axum::{Json, http::StatusCode, response::IntoResponse};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_reprocess_cli::{DayOutcome, Flavor, reprocess_day_with};
use std::{path::Path, str::FromStr};

pub async fn retry_import(body: Option<Json<Value>>) -> axum::response::Response {
    let body = body.map(|v| v.0).unwrap_or_default();
    let Some(import_id) = body
        .get("import_id")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
    else {
        return missing("Missing import_id");
    };
    let _ = import_id;
    let message = match body
        .get("stage")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
    {
        Some(stage) => {
            format!("Import retry from stage {stage} will be available in a future update")
        }
        None => "Import retry will be available in a future update".to_owned(),
    };
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"status":"not_implemented","message":message})),
    )
        .into_response()
}

pub async fn check_brain(root: std::path::PathBuf) -> axum::response::Response {
    let ok = brain::refresh(&root);
    let mut response = serde_json::Map::new();
    response.insert("ok".to_owned(), Value::Bool(ok));
    response.insert("brain".to_owned(), brain::snapshot(&root));
    if !ok {
        response.insert(
            "error".to_owned(),
            Value::String("check_not_started".to_owned()),
        );
    }
    Json(Value::Object(response)).into_response()
}

pub async fn reprocess(
    root: std::path::PathBuf,
    body: Option<Json<Value>>,
) -> axum::response::Response {
    let body = body.map(|v| v.0).unwrap_or_default();
    let Some(day) = body
        .get("day")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
    else {
        return missing("Missing day");
    };
    let flavor = match body.get("flavor").and_then(Value::as_str) {
        Some("process-now") => Flavor::ProcessNow,
        Some("from-scratch") => Flavor::FromScratch,
        _ => return invalid("Unknown reprocess flavor"),
    };
    reprocess_with(&root, day, flavor, Utc::now(), local_zone(), |envelope| {
        send(&root, envelope)
    })
}

pub fn reprocess_with<F>(
    root: &Path,
    day: &str,
    flavor: Flavor,
    now: DateTime<Utc>,
    zone: Tz,
    transport: F,
) -> axum::response::Response
where
    F: FnMut(&solstone_core_callosum::CallosumEnvelope) -> bool,
{
    response(
        day,
        reprocess_day_with(root, day, flavor, now, zone, transport),
    )
}

pub fn response(day: &str, outcome: DayOutcome) -> axum::response::Response {
    match outcome {
        DayOutcome::Submitted(_) => Json(json!({"status":"queued","day":day})).into_response(),
        DayOutcome::AlreadyComplete => Json(json!({"status":"already_complete","day":day,"message":"this day's already done. want to redo it from scratch?","reason_code":"reprocess_already_complete"})).into_response(),
        DayOutcome::PastOnly => error_envelope("reprocess_past_only","you can only reprocess past days — today and future days aren't ready yet.","",StatusCode::BAD_REQUEST).into_response(),
        DayOutcome::Unreachable => error_envelope("reprocess_unreachable","your journal's background service isn't running. start it, then try again.","",StatusCode::SERVICE_UNAVAILABLE).into_response(),
        DayOutcome::Failed(cause) => error_envelope("reprocess_failed",format!("reprocess failed: {cause}"),"",StatusCode::INTERNAL_SERVER_ERROR).into_response(),
        DayOutcome::Malformed | DayOutcome::NoData => error_envelope("invalid_day","that day couldn't be used.","",StatusCode::BAD_REQUEST).into_response(),
    }
}

fn local_zone() -> Tz {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| Tz::from_str(&name).ok())
        .unwrap_or(chrono_tz::UTC)
}

fn send(root: &std::path::Path, envelope: &solstone_core_callosum::CallosumEnvelope) -> bool {
    let Ok(mut line) = serde_json::to_string(envelope) else {
        return false;
    };
    line.push('\n');
    solstone_core_callosum::CallosumOneShotSender::new(
        root.join("health/callosum.sock"),
        std::time::Duration::from_secs(1),
    )
    .send_line(&line)
    .is_ok()
}
fn missing(detail: &str) -> axum::response::Response {
    error_envelope(
        "missing_required_field",
        "a required field is missing.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}
fn invalid(detail: &str) -> axum::response::Response {
    error_envelope(
        "invalid_request_value",
        "one of those values couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}
