// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native awareness API routes.

use std::path::PathBuf;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{RawQuery, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Map, Value, json};
use solstone_core_time::day_key;

use crate::{Clock, http};

pub fn routes(root: PathBuf, clock: Clock) -> Router {
    Router::new()
        .route("/app/awareness/api/state", get(state))
        .route(
            "/app/awareness/api/imports",
            get(imports).post(update_imports),
        )
        .route("/app/awareness/api/log", get(log).post(create_log))
        .with_state((root, clock))
}

async fn state(State((root, _)): State<(PathBuf, Clock)>, RawQuery(query): RawQuery) -> Response {
    if let Some(response) = session_response(&root) {
        return response;
    }
    match solstone_core_facets::load_current(&root) {
        Ok(value) => match query_value(query.as_deref(), "section") {
            None => Json(value).into_response(),
            Some(section) => value
                .get(&section)
                .cloned()
                .map(Json)
                .map(IntoResponse::into_response)
                .unwrap_or_else(|| {
                    http::error(
                        "awareness_section_not_found",
                        "journal.window.facets.awareness.response.error.message.that.part.of.your.journal.couldn",
                        http::Detail::with_value("journal.window.facets.awareness.response.error.detail.no.awareness.section.named.value", section.to_string()),
                        StatusCode::NOT_FOUND,
                    )
                }),
        },
        Err(error) => internal(error.to_string()),
    }
}

async fn imports(State((root, _)): State<(PathBuf, Clock)>) -> Response {
    if let Some(response) = session_response(&root) {
        return response;
    }
    solstone_core_facets::load_imports(&root)
        .map(|value| Json(value).into_response())
        .unwrap_or_else(|error| internal(error.to_string()))
}

async fn log(State((root, clock)): State<(PathBuf, Clock)>, RawQuery(query): RawQuery) -> Response {
    if let Some(response) = session_response(&root) {
        return response;
    }
    let day = query_value(query.as_deref(), "day").unwrap_or_else(|| day_key(clock.now().date()));
    let kind = query_value(query.as_deref(), "kind");
    let (limit, offset) = pagination(query.as_deref());
    match solstone_core_facets::read_log(&root, &day) {
        Ok(mut entries) => {
            if let Some(kind) = kind {
                entries.retain(|entry| {
                    entry.get("kind").and_then(Value::as_str) == Some(kind.as_str())
                });
            }
            let total = entries.len();
            let page = entries
                .into_iter()
                .skip(offset)
                .take(limit)
                .collect::<Vec<_>>();
            Json(json!({"items":page,"total":total})).into_response()
        }
        Err(error) => internal(error.to_string()),
    }
}

async fn update_imports(State((root, clock)): State<(PathBuf, Clock)>, body: Bytes) -> Response {
    if let Some(response) = session_response(&root) {
        return response;
    }
    let body = match awareness_body(&body) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    let active = [
        body.get("record")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|_| "record"),
        body.get("declined")
            .filter(|value| **value == Value::Bool(true))
            .map(|_| "declined"),
        body.get("nudge")
            .filter(|value| **value == Value::Bool(true))
            .map(|_| "nudge"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if active.len() != 1 {
        let detail = if active.is_empty() {
            http::Detail::catalog("journal.window.facets.awareness.response.one.import.answer")
        } else {
            http::Detail::with_value(
                "journal.window.facets.awareness.response.too.many.import.answers",
                active.join(", "),
            )
        };
        return invalid(detail);
    }
    let now = clock.now();
    // The imports block keeps its local wall-time form; the log row keeps the instant.
    let iso = now.strftime("%Y%m%dT%H:%M:%S").to_string();
    let day = day_key(now.date());
    let timestamp_ms = now.timestamp().as_millisecond();
    let outcome = match active[0] {
        "record" => solstone_core_facets::record_import(
            &root,
            body["record"].as_str().expect("active record"),
            None,
            0,
            &iso,
            &day,
            timestamp_ms,
        ),
        "declined" => {
            solstone_core_facets::record_import_offer_declined(&root, &iso, &day, timestamp_ms)
        }
        _ => solstone_core_facets::record_import_nudge(&root, &iso, &day, timestamp_ms),
    };
    outcome
        .map(|value| Json(value).into_response())
        .unwrap_or_else(|error| {
            if error.to_string().contains("timed out") {
                busy()
            } else {
                internal(error.to_string())
            }
        })
}

async fn create_log(State((root, clock)): State<(PathBuf, Clock)>, body: Bytes) -> Response {
    if let Some(response) = session_response(&root) {
        return response;
    }
    let body = match awareness_body(&body) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    let Some(kind) = body
        .get("kind")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return http::error(
            "missing_required_field",
            "journal.window.facets.awareness.response.error.message.a.required.field.is.missing",
            http::Detail::catalog(
                "journal.window.facets.awareness.response.error.detail.kind.is.required",
            ),
            StatusCode::BAD_REQUEST,
        );
    };
    let data = body.get("data").and_then(Value::as_object);
    // The HTTP contract accepts only the documented fields; framework-owned
    // fields such as `ts` must not be supplied by a caller.
    let extra = Map::new();
    let now = clock.now();
    match solstone_core_facets::append_log(
        &root,
        kind,
        body.get("key").and_then(Value::as_str),
        body.get("message").and_then(Value::as_str),
        data,
        &day_key(now.date()),
        now.timestamp().as_millisecond(),
        &extra,
    ) {
        Ok(value) => (StatusCode::CREATED, Json(value)).into_response(),
        Err(error) => internal(error.to_string()),
    }
}

fn query_value(query: Option<&str>, wanted: &str) -> Option<String> {
    query.unwrap_or_default().split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == wanted).then(|| value.to_owned())
    })
}

fn pagination(query: Option<&str>) -> (usize, usize) {
    let limit = query_value(query, "limit")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(20)
        .clamp(1, 100) as usize;
    let offset = query_value(query, "offset")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0) as usize;
    (limit, offset)
}

type AwarenessBodyResult = Result<Value, Box<Response>>;

fn awareness_body(bytes: &[u8]) -> AwarenessBodyResult {
    if bytes.is_empty() {
        return Err(Box::new(http::error(
            "missing_request_body",
            "journal.window.facets.awareness.response.error.message.that.request.had.no.data.in",
            http::Detail::catalog(
                "journal.window.facets.awareness.response.error.detail.no.request.body",
            ),
            StatusCode::BAD_REQUEST,
        )));
    }
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| {
            Box::new(http::error(
                "invalid_json_request",
                "journal.window.facets.awareness.response.error.message.that.json.request.couldn.t.be",
                http::Detail::catalog("journal.window.facets.awareness.response.error.detail.request.body.must.be.a.json"),
                StatusCode::BAD_REQUEST,
            ))
        })
}

fn invalid(detail: http::Detail) -> Response {
    http::error(
        "invalid_request_value",
        "journal.window.facets.awareness.response.error.message.one.of.those.values.couldn.t",
        detail,
        StatusCode::BAD_REQUEST,
    )
}
fn busy() -> Response {
    http::error(
        "awareness_busy",
        "journal.window.facets.awareness.response.error.message.the.awareness.operation.is.busy",
        http::Detail::catalog("journal.window.facets.awareness.response.imports.busy.detail"),
        StatusCode::SERVICE_UNAVAILABLE,
    )
}
fn internal(detail: String) -> Response {
    http::error(
        "internal_error",
        "journal.window.facets.awareness.response.error.message.that.request.didn.t.finish",
        detail.into(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
}

fn session_response(root: &std::path::Path) -> Option<Response> {
    let path = root.join("config/journal.json");
    if !path.exists() {
        return Some(
            axum::http::Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, "/init")
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(axum::body::Body::from("<!doctype html>\n<html lang=en>\n<title>Redirecting...</title>\n<h1>Redirecting...</h1>\n<p>You should be redirected automatically to the target URL: <a href=\"/init\">/init</a>. If not, click the link.\n"))
                .expect("redirect response"),
        );
    }
    std::fs::read_to_string(&path)
        .ok()
        .filter(|contents| serde_json::from_str::<Value>(contents).is_err())
        .map(|_| {
            http::error(
                "corrupt_config",
                "journal.window.facets.awareness.response.error.message.your.settings.couldn.t.be.read",
                http::Detail::with_value("journal.window.facets.awareness.response.error.detail.your.settings.file.at.value.couldn", path.display().to_string()),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::*;
    #[test]
    fn pagination_clamps_like_flask() {
        assert_eq!(pagination(Some("limit=nope")), (20, 0));
        assert_eq!(pagination(Some("limit=0")), (1, 0));
        assert_eq!(pagination(Some("limit=999")), (100, 0));
        assert_eq!(pagination(Some("offset=-5")), (20, 0));
    }

    #[tokio::test]
    async fn awareness_body_rejections_match_python() {
        let root = crate::test_support::phase_root("established_empty");
        let cases = [
            (Body::empty(), "missing_request_body"),
            (Body::from("not json"), "invalid_json_request"),
            (Body::from("[]"), "invalid_json_request"),
        ];
        for (body, reason) in cases {
            let response = routes(
                root.path().to_path_buf(),
                crate::test_support::fixed_clock(),
            )
            .oneshot(
                Request::post("/app/awareness/api/log")
                    .body(body)
                    .expect("request"),
            )
            .await
            .expect("response");
            let body: Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body"),
            )
            .expect("json");
            assert_eq!(body["reason_code"], reason);
        }
    }

    #[tokio::test]
    async fn create_log_does_not_allow_a_request_to_override_the_timestamp() {
        let root = crate::test_support::phase_root("established_empty");
        let response = routes(
            root.path().to_path_buf(),
            crate::test_support::fixed_clock(),
        )
        .oneshot(
            Request::post("/app/awareness/api/log")
                .body(Body::from(r#"{"kind":"state","key":"test","ts":0}"#))
                .expect("request"),
        )
        .await
        .expect("response");
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        assert_eq!(body["ts"], 1_778_846_400_000_i64);
    }

    #[tokio::test]
    async fn an_evening_log_row_keeps_the_instant_and_files_under_the_local_day() {
        let root = crate::test_support::phase_root("established_empty");
        // 18:00 at UTC-6 on May 10 is midnight UTC on May 11.
        let evening = Clock::new(|| {
            solstone_core_time::parse_rfc3339("2026-05-10T18:00:00-06:00").expect("clock")
        });
        let response = routes(root.path().to_path_buf(), evening)
            .oneshot(
                Request::post("/app/awareness/api/log")
                    .body(Body::from(r#"{"kind":"state","key":"test"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        let instant = solstone_core_time::parse_timestamp("2026-05-11T00:00:00Z").unwrap();
        assert_eq!(body["ts"], instant.as_millisecond());
        assert!(root.path().join("awareness/20260510.jsonl").is_file());
    }
}
