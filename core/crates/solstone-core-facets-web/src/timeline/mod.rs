// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::{
    Router,
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};

use crate::{
    assets,
    clock::Clock,
    date_nav, http,
    segments::{
        DEFAULT_STREAM, day_segment_counts, is_day, is_exact_segment_key, is_month, iter_segments,
    },
};

mod browser;
mod day;
mod projection;
#[cfg(test)]
mod recovery_tests;
mod rollup;
mod segment;

pub fn routes(root: PathBuf, clock: Clock) -> Router {
    let index_root = root.clone();
    let index_clock = clock.clone();
    let overview_root = root.clone();
    let overview_clock = clock.clone();
    let grid_root = root.clone();
    let month_root = root.clone();
    let day_root = root.clone();
    let stats_root = root.clone();
    let segment_root = root.clone();
    let default_segment_root = root;
    Router::new()
        .route("/app/timeline/", get(move || index(index_clock.clone())))
        .route(
            "/app/timeline/workspace",
            get(|| async { assets::workspace() }),
        )
        .route(
            "/app/timeline/background",
            get(|| async { assets::background() }),
        )
        .route("/app/timeline/year", get(|| async { assets::shell() }))
        .route(
            "/app/timeline/{value}",
            get(|Path(value): Path<String>| async move {
                if is_day(&value) || is_month(&value) {
                    assets::shell()
                } else {
                    assets::empty_not_found()
                }
            }),
        )
        .route(
            "/app/timeline/static/{name}",
            get(|Path(name): Path<String>| async move { assets::static_asset(&name) }),
        )
        .route(
            "/app/timeline/api/overview",
            get(move || overview(overview_root.clone(), overview_clock.clone())),
        )
        .route(
            "/app/timeline/api/grid",
            get(move || grid(grid_root.clone())),
        )
        .route(
            "/app/timeline/api/index",
            get(move || index_api(index_root.clone())),
        )
        .route(
            "/app/timeline/api/stats/{ym}",
            get(move |Path(ym)| stats(stats_root.clone(), ym)),
        )
        .route(
            "/app/timeline/api/month/{ym}",
            get(move |Path(ym)| month(month_root.clone(), ym)),
        )
        .route(
            "/app/timeline/api/day/{day}",
            get(move |Path(value)| day(day_root.clone(), value)),
        )
        .route(
            "/app/timeline/api/segment/{day}/{stream}/{segment}",
            get(move |Path((day, stream, segment))| {
                segment_api(segment_root.clone(), day, stream, segment)
            }),
        )
        .route(
            "/app/timeline/api/segment/{day}/{segment}",
            get(move |Path((day, segment))| {
                segment_api(
                    default_segment_root.clone(),
                    day,
                    DEFAULT_STREAM.to_owned(),
                    segment,
                )
            }),
        )
}

async fn index(clock: Clock) -> Response {
    let location = format!("/app/timeline/{}", clock.now().format("%Y%m%d"));
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, &location)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(redirect_body(&location)))
        .expect("redirect builds")
}
// Deliberate duplicate of convey-shell's redirect body: a production dependency on convey-shell would create a Cargo cycle (the shell normally depends on this crate).
fn redirect_body(location: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=en>\n<title>Redirecting...</title>\n<h1>Redirecting...</h1>\n<p>You should be redirected automatically to the target URL: <a href=\"{location}\">{location}</a>. If not, click the link.\n"
    )
}
async fn overview(root: PathBuf, clock: Clock) -> Response {
    rollup::overview(&root, &clock)
        .map(json_response)
        .unwrap_or_else(|_| http::internal_error())
}
async fn grid(root: PathBuf) -> Response {
    let master = rollup::master(&root);
    let mut payload = date_nav::day_grid_payload(
        &day_segment_counts(&root, None),
        rollup::rollup_watermark(&master).as_deref(),
        None,
    );
    payload["timeline_status"] = Value::String(master.status.as_str().to_owned());
    payload["timeline_artifact_outcome"] = Value::String(master.outcome.as_str().to_owned());
    json_response(payload)
}
async fn index_api(root: PathBuf) -> Response {
    json_response(date_nav::date_nav_index(&day_segment_counts(&root, None)))
}
async fn stats(root: PathBuf, ym: String) -> Response {
    if !is_month(&ym) {
        // Python reaches this same 400 via the reason's default status; this port has no
        // reason-default table, so the observable status remains explicit.
        return http::error(
            "invalid_month",
            "I couldn't use that month.",
            "Invalid month format".to_owned(),
            StatusCode::BAD_REQUEST,
        );
    }
    json_response(
        serde_json::to_value(day_segment_counts(&root, Some(&ym))).unwrap_or_else(|_| json!({})),
    )
}
async fn month(root: PathBuf, ym: String) -> Response {
    if !is_month(&ym) {
        return http::error(
            "invalid_month",
            "I couldn't use that month.",
            "Invalid month format".to_owned(),
            StatusCode::BAD_REQUEST,
        );
    }
    rollup::month(&root, &ym)
        .map(json_response)
        .unwrap_or_else(|_| http::internal_error())
}
async fn day(root: PathBuf, value: String) -> Response {
    if !is_day(&value) {
        return http::error(
            "invalid_day",
            "I couldn't use that day.",
            "Invalid day format".to_owned(),
            StatusCode::BAD_REQUEST,
        );
    }
    let mut value_json = rollup::day_rollup(&root, &value);
    value_json["hours_avail"] =
        day::hours_avail(&value, iter_segments(&root.join("chronicle").join(&value)));
    json_response(value_json)
}
async fn segment_api(root: PathBuf, day: String, stream: String, segment: String) -> Response {
    if !is_day(&day)
        || !is_exact_segment_key(&segment)
        || (stream != DEFAULT_STREAM && stream.contains('/'))
    {
        return http::error(
            "invalid_path",
            "I couldn't use that path.",
            "Invalid segment path".to_owned(),
            StatusCode::BAD_REQUEST,
        );
    }
    json_response(segment::load(&root, &day, &stream, &segment))
}
fn json_response(value: Value) -> Response {
    axum::Json(value).into_response()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::routes;
    use crate::test_support::{fixed_clock, phase_root, write};

    #[tokio::test]
    async fn day_api_uses_its_own_artifact_when_master_state_is_stale() {
        let root = phase_root("populated");
        let state_path = root.path().join("health/timeline/state.json");
        let mut state: Value =
            serde_json::from_str(&fs::read_to_string(&state_path).unwrap()).expect("state JSON");
        state["artifacts"]["master"]["input_digest"] = json!("stale-master");
        write(
            &state_path,
            &serde_json::to_string(&state).expect("state JSON"),
        );

        let response = routes(root.path().to_path_buf(), fixed_clock())
            .oneshot(
                Request::get("/app/timeline/api/day/20260510")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let payload: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");

        assert_eq!(payload["status"], "current");
        assert_eq!(payload["day_top"][0]["binding"]["segment"], "100000_300");
    }
}
