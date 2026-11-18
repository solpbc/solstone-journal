// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::brain;
use axum::{Json, http::StatusCode, response::IntoResponse};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_reprocess_cli::{DayOutcome, Flavor, reprocess_day_with};
use std::path::Path;

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
    let zone = solstone_core_journal_config::owner_zone(&root);
    reprocess_with(&root, day, flavor, Timestamp::now(), &zone, |envelope| {
        send(&root, envelope)
    })
}

pub fn reprocess_with<F>(
    root: &Path,
    day: &str,
    flavor: Flavor,
    now: Timestamp,
    zone: &TimeZone,
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
        DayOutcome::NoThinkingEngine => error_envelope("reprocess_no_thinking_engine","no model is chosen yet. choose one in thinking, then try again.","",StatusCode::BAD_REQUEST).into_response(),
        DayOutcome::PastOnly => error_envelope("reprocess_past_only","you can only reprocess past days — today and future days aren't ready yet.","",StatusCode::BAD_REQUEST).into_response(),
        DayOutcome::WaitingForSupervisor => Json(json!({"status":"saved","day":day,"message":"your request is saved. it will run when your journal's background service starts."})).into_response(),
        DayOutcome::Failed(cause) => error_envelope("reprocess_failed",format!("reprocess failed: {cause}"),"",StatusCode::INTERNAL_SERVER_ERROR).into_response(),
        DayOutcome::Malformed | DayOutcome::NoData => error_envelope("invalid_day","that day couldn't be used.","",StatusCode::BAD_REQUEST).into_response(),
    }
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

/// Save only a source-validated unit intent. Queue delivery is a wakeup, not a
/// claim that the unit has started or that a process safety hold was cleared.
pub async fn retry_work(
    root: std::path::PathBuf,
    body: Option<Json<Value>>,
) -> axum::response::Response {
    match tokio::task::spawn_blocking(move || retry_work_blocking(root, body)).await {
        Ok(response) => response,
        Err(error) => error_envelope(
            "retry_unavailable",
            "the retry could not be checked.",
            error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
    }
}

fn retry_work_blocking(
    root: std::path::PathBuf,
    body: Option<Json<Value>>,
) -> axum::response::Response {
    use solstone_core_system::work_retry::{self, Unit};
    let body = body.map(|v| v.0).unwrap_or_default();
    if body.get("safety") == Some(&Value::Bool(true)) {
        let sent = send(
            &root,
            &solstone_core_callosum::CallosumEnvelope {
                tract: "supervisor".into(),
                event: "drain".into(),
                ts: None,
                extra: Default::default(),
            },
        );
        return Json(json!({"status": "held", "message": if sent {
            "the safety hold is still in place. checking the journal's status again."
        } else { "the background service could not be reached. the safety hold is still in place." }})).into_response();
    }
    let Some(id) = body.get("unit").and_then(Value::as_str) else {
        return missing("Missing unit");
    };
    let result = (|| -> Result<Option<String>, String> {
        let record = work_retry::read(&root, id)?.ok_or("that work is no longer set aside")?;
        let input = match &record.unit {
            Unit::Raw { output, .. } => {
                let output = record.unit.segment_directory(&root)?.join(output);
                let Some(evidence) = work_retry::raw::inspect_output(&root, &output)? else {
                    return Ok(None);
                };
                work_retry::raw::observe(&root, &evidence)?;
                if matches!(
                    evidence.reason.as_str(),
                    "corrupt_input" | "no_audio_stream"
                ) && evidence.input == record.input
                {
                    return Err("the source still cannot be read as usable media. replace it before trying again".into());
                }
                evidence.input
            }
            Unit::Daily { .. } => {
                let Some(input) = work_retry::daily::observe(&root, &record.unit)? else {
                    return Ok(None);
                };
                input
            }
            Unit::Segment { .. } | Unit::Activity { .. } => {
                work_retry::input::current(&root, &record.unit)?
            }
        };
        work_retry::request(&root, &record.unit)?;
        let current = work_retry::read(&root, id)?.ok_or("saved request could not be read")?;
        if current
            .permission(&input, work_retry::DECLARED_FIXES)
            .is_none()
        {
            return Err("this work is still held after checking its source".into());
        }
        let _ = send(
            &root,
            &solstone_core_callosum::CallosumEnvelope {
                tract: "supervisor".into(),
                event: "drain".into(),
                ts: None,
                extra: serde_json::Map::from_iter([("day".into(), json!(record.unit.day()))]),
            },
        );
        Ok(Some(
            "your retry request is saved. the journal will check this work before retrying it."
                .into(),
        ))
    })();
    match result {
        Ok(Some(message)) => Json(json!({"status":"saved", "message":message})).into_response(),
        Ok(None) => Json(json!({"status":"changed", "message":"that work has changed since this list was checked. refreshing its status."})).into_response(),
        Err(error) => (StatusCode::CONFLICT, Json(json!({"status":"held", "message":error}))).into_response(),
    }
}
