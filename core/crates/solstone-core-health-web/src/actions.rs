// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::brain;
use axum::{Json, http::StatusCode, response::IntoResponse};
use chrono::{DateTime, Local, TimeZone, Utc};
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

pub async fn restart_observer(
    root: std::path::PathBuf,
    body: Option<Json<Value>>,
) -> axum::response::Response {
    let body = body.map(|v| v.0).unwrap_or_default();
    let Some(service) = body
        .get("service")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
    else {
        return missing("Missing service");
    };
    if service != "sense" {
        return invalid("Unknown observer service");
    }
    restart_observer_with(service, |envelope| send(&root, envelope))
}

pub fn restart_observer_with<F>(service: &str, mut transport: F) -> axum::response::Response
where
    F: FnMut(&solstone_core_callosum::CallosumEnvelope) -> bool,
{
    let mut extra = serde_json::Map::new();
    extra.insert("service".to_owned(), Value::String(service.to_owned()));
    let envelope = solstone_core_callosum::CallosumEnvelope {
        tract: "supervisor".to_owned(),
        event: "restart".to_owned(),
        ts: None,
        extra,
    };
    let sent = transport(&envelope);
    if !sent {
        return error_envelope(
            "observer_restart_failed",
            "i couldn't restart sol's processing.",
            "Could not reach the supervisor",
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .into_response();
    }
    Json(json!({"status":"restart_requested","service":service})).into_response()
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
        DayOutcome::Held(epoch) => Json(json!({"status":"held_by_backoff","day":day,"message":format!("sol's not retrying this day until {}. to start it over right now, use redo from scratch.",format_retry_when(epoch)),"reason_code":"reprocess_held_by_backoff"})).into_response(),
        DayOutcome::PastOnly => error_envelope("reprocess_past_only","you can only reprocess past days — today and future days aren't ready yet.","",StatusCode::BAD_REQUEST).into_response(),
        DayOutcome::Unreachable => error_envelope("reprocess_unreachable","your journal's background service isn't running. start it, then try again.","",StatusCode::SERVICE_UNAVAILABLE).into_response(),
        DayOutcome::Failed(cause) => error_envelope("reprocess_failed",format!("reprocess failed: {cause}"),"",StatusCode::INTERNAL_SERVER_ERROR).into_response(),
        DayOutcome::Malformed | DayOutcome::NoData => error_envelope("invalid_day","I couldn't use that day.","",StatusCode::BAD_REQUEST).into_response(),
    }
}

pub fn format_retry_when(epoch: f64) -> String {
    format_retry_when_at(epoch, Local::now())
}
pub fn format_retry_when_at(epoch: f64, now: DateTime<Local>) -> String {
    let Some(then) = Local.timestamp_opt(epoch as i64, 0).single() else {
        return "an unknown time".to_owned();
    };
    let days = then
        .date_naive()
        .signed_duration_since(now.date_naive())
        .num_days();
    let when = match days {
        0 => "today".to_owned(),
        1 => "tomorrow".to_owned(),
        _ => then.format("%b %-d").to_string().to_ascii_lowercase(),
    };
    format!("{when} at {}", format_retry_clock(then))
}
pub fn format_retry_clock(then: DateTime<Local>) -> String {
    then.format("%I:%M%p")
        .to_string()
        .trim_start_matches('0')
        .to_ascii_lowercase()
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
        "I couldn't find a required field.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}
fn invalid(detail: &str) -> axum::response::Response {
    error_envelope(
        "invalid_request_value",
        "I couldn't use one of those values.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}
