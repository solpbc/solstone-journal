// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::{
    Router,
    extract::{Path, Query},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{Datelike, Local, NaiveDate};
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;

use crate::chat_state;

const SHELL: &[u8] = include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
const WORKSPACE: &str = include_str!("../assets/chat/workspace.html");

pub fn router(journal_root: PathBuf) -> Router {
    let index_root = journal_root.clone();
    let day_root = journal_root.clone();
    let state_root = journal_root.clone();
    let index_api_root = journal_root.clone();
    let stats_root = journal_root;
    Router::new()
        .route("/app/chat/", get(move || index(index_root.clone())))
        .route(
            "/app/chat/{day}",
            get(move |day: Path<String>| day_page(day_root.clone(), day)),
        )
        .route("/app/chat/workspace", get(workspace))
        .route(
            "/app/chat/api/state",
            get(move |query: Query<StateQuery>| state(state_root.clone(), query)),
        )
        .route(
            "/app/chat/api/index",
            get(move || index_api(index_api_root.clone())),
        )
        .route(
            "/app/chat/api/stats/{month}",
            get(move |month: Path<String>| stats(stats_root.clone(), month)),
        )
}

async fn index(_journal_root: PathBuf) -> Response {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, format!("/app/chat/{}", today_day()))
        .body(axum::body::Body::empty())
        .expect("chat redirect response")
}

async fn day_page(_journal_root: PathBuf, Path(day): Path<String>) -> Response {
    if !valid_day(&day) {
        return StatusCode::NOT_FOUND.into_response();
    }
    shell()
}

async fn workspace() -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(WORKSPACE))
        .expect("embedded chat workspace response")
}

#[derive(Deserialize)]
struct StateQuery {
    day: Option<String>,
}

async fn state(journal_root: PathBuf, Query(query): Query<StateQuery>) -> Response {
    let Some(day) = query.day.filter(|day| valid_day(day)) else {
        return invalid_day(StatusCode::NOT_FOUND, "Day not found");
    };
    let today = today_day();
    let events = chat_state::read_events(&journal_root, &day).unwrap_or_default();
    let config = solstone_core_settings_web::chat::load_chat_config(&journal_root);
    let thinking_surfaces = config
        .get("thinking_surfaces")
        .cloned()
        .unwrap_or_else(|| json!(solstone_core_settings_web::chat::DEFAULT_THINKING_SURFACES));
    let (owner_name, agent_name) = identity(&journal_root);
    axum::Json(json!({
        "events": events,
        "sol_message_origins": chat_state::message_origins(&events),
        "owner_name": owner_name,
        "agent_name": agent_name,
        "thinking_surfaces": thinking_surfaces,
        "today_day": today,
        "sol_open_request_id": chat_state::sol_open_request_id(&events, &day, &today),
    }))
    .into_response()
}

async fn index_api(journal_root: PathBuf) -> axum::Json<Value> {
    let counts = chat_state::day_counts(&journal_root);
    let days = counts
        .iter()
        .filter(|(_, count)| **count > 0)
        .map(|(day, _)| day.clone())
        .collect::<Vec<_>>();
    let mut months = serde_json::Map::new();
    for (day, count) in counts.into_iter().filter(|(_, count)| *count > 0) {
        let month = day[..6].to_owned();
        let total = months.get(&month).and_then(Value::as_u64).unwrap_or(0) + count as u64;
        months.insert(month, json!(total));
    }
    axum::Json(json!({
        "coverage": days.first().zip(days.last()).map(|(start, end)| json!({"start": start, "end": end})),
        "months": months,
    }))
}

async fn stats(journal_root: PathBuf, Path(month): Path<String>) -> Response {
    let Some((year, month_number)) = parse_month(&month) else {
        return invalid_month();
    };
    let mut result = serde_json::Map::new();
    for day_number in 1..=days_in_month(year, month_number) {
        let day = format!("{month}{day_number:02}");
        let count = chat_state::read_events(&journal_root, &day).map_or(0, |events| events.len());
        if count > 0 {
            result.insert(day, json!(count));
        }
    }
    axum::Json(Value::Object(result)).into_response()
}

fn today_day() -> String {
    Local::now().format("%Y%m%d").to_string()
}

fn valid_day(day: &str) -> bool {
    day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit())
}

fn parse_month(month: &str) -> Option<(i32, u32)> {
    if month.len() != 6 || !month.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year = month[..4].parse().ok()?;
    let number = month[4..].parse().ok()?;
    NaiveDate::from_ymd_opt(year, number, 1)?;
    Some((year, number))
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .expect("validated month has a following month");
    (next - chrono::Days::new(1)).day()
}

fn identity(journal_root: &std::path::Path) -> (String, String) {
    let config = std::fs::read_to_string(journal_root.join("config/journal.json"))
        .ok()
        .and_then(|source| serde_json::from_str::<Value>(&source).ok())
        .unwrap_or(Value::Null);
    let owner = config["identity"]["preferred"]
        .as_str()
        .or_else(|| config["identity"]["name"].as_str())
        .unwrap_or("Owner")
        .trim();
    let agent = config["agent"]["name"].as_str().unwrap_or("sol").trim();
    (
        if owner.is_empty() { "Owner" } else { owner }.to_owned(),
        if agent.is_empty() { "sol" } else { agent }.to_owned(),
    )
}

fn shell() -> Response {
    asset(SHELL)
}

fn asset(bytes: &'static [u8]) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(bytes))
        .expect("embedded chat asset response")
}

fn invalid_day(status: StatusCode, detail: &str) -> Response {
    error_envelope("invalid_day", "I couldn't use that day.", detail, status).into_response()
}

fn invalid_month() -> Response {
    error_envelope(
        "invalid_month",
        "I couldn't use that month.",
        "Invalid month format, expected YYYYMM",
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}
