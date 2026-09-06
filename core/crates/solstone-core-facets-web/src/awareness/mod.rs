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

use crate::{Clock, http};

const IMPORTS_BUSY: &str = "imports are busy; try again";

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
                        "that part of your journal couldn't be found.",
                        format!("no awareness section named '{section}'"),
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
    let day = query_value(query.as_deref(), "day")
        .unwrap_or_else(|| clock.now().format("%Y%m%d").to_string());
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
            "provide exactly one of record/declined/nudge".to_owned()
        } else {
            format!(
                "provide exactly one of record/declined/nudge; got {}",
                active.join(", ")
            )
        };
        return invalid(detail);
    }
    let now = clock.now();
    let iso = now.format("%Y%m%dT%H:%M:%S").to_string();
    let day = now.format("%Y%m%d").to_string();
    let timestamp_ms = now.and_utc().timestamp_millis();
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
            "a required field is missing.",
            "kind is required".to_owned(),
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
        &now.format("%Y%m%d").to_string(),
        now.and_utc().timestamp_millis(),
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
            "that request had no data in it.",
            "no request body".to_owned(),
            StatusCode::BAD_REQUEST,
        )));
    }
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| {
            Box::new(http::error(
                "invalid_json_request",
                "that JSON request couldn't be read.",
                "request body must be a JSON object".to_owned(),
                StatusCode::BAD_REQUEST,
            ))
        })
}

fn invalid(detail: String) -> Response {
    http::error(
        "invalid_request_value",
        "one of those values couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
}
fn busy() -> Response {
    http::error(
        "awareness_busy",
        "The awareness operation is busy.",
        IMPORTS_BUSY.to_owned(),
        StatusCode::SERVICE_UNAVAILABLE,
    )
}
fn internal(detail: String) -> Response {
    http::error(
        "internal_error",
        "that request didn't finish.",
        detail,
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
                "your settings couldn't be read.",
                format!("your settings file at {} couldn't be read. your settings were not changed. repair the file or restore config/journal.json from a backup, then try again.", path.display()),
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
}
