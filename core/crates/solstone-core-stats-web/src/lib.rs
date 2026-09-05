// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native stats workspace and its read-only token-usage APIs.

use axum::{Json, Router, extract::Query, http::StatusCode, response::IntoResponse, routing::get};
use chrono::Datelike;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::path::PathBuf;

mod assets;
mod clock;
mod tokens;
pub use clock::Clock;

pub fn routes(journal_root: PathBuf, clock: Clock) -> Router {
    let stats_root = journal_root.clone();
    let usage_root = journal_root.clone();
    let usage_clock = clock.clone();
    let index_root = journal_root.clone();
    let month_root = journal_root.clone();
    Router::new()
        .route("/app/stats/", get(assets::shell))
        .route("/app/stats/workspace", get(assets::workspace))
        .route("/app/stats/static/{*name}", get(assets::static_asset))
        .route("/app/stats/background", get(assets::background))
        .route(
            "/app/stats/api/stats",
            get(move || stats_data(stats_root.clone())),
        )
        .route(
            "/app/stats/api/usage",
            get(move |query| usage(usage_root.clone(), usage_clock.clone(), query)),
        )
        .route(
            "/app/stats/api/index",
            get(move || index(index_root.clone())),
        )
        .route(
            "/app/stats/api/stats/{month}",
            get(move |axum::extract::Path(month)| month_stats(month_root.clone(), month)),
        )
}

#[derive(Deserialize)]
struct UsageQuery {
    day: Option<String>,
}
async fn usage(root: PathBuf, clock: Clock, Query(query): Query<UsageQuery>) -> impl IntoResponse {
    let day = query.day.unwrap_or_else(|| {
        format!(
            "{:04}{:02}{:02}",
            clock.now().year(),
            clock.now().month(),
            clock.now().day()
        )
    });
    if !digits(&day, 8) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_day",
            "I couldn't use that day.",
            "Invalid day format",
        );
    }
    match tokens::aggregate(&root, &day) {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(_) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "file_read_failed",
            "I couldn't read that file.",
            "Failed to read token data",
        ),
    }
}
async fn index(root: PathBuf) -> impl IntoResponse {
    match tokens::usage_stats(&root, None) {
        Ok(days) => {
            let mut months = Map::new();
            for (day, tokens) in days.as_object().expect("usage rows") {
                let month = &day[..6];
                let total = months.get(month).and_then(Value::as_f64).unwrap_or(0.0)
                    + tokens.as_f64().unwrap_or(0.0);
                months.insert(month.to_owned(), json!(total));
            }
            let coverage = days
                .as_object()
                .and_then(|rows| rows.keys().min().zip(rows.keys().max()))
                .map(|(start, end)| json!({"start":start,"end":end}))
                .unwrap_or(Value::Null);
            (
                StatusCode::OK,
                Json(json!({"coverage":coverage,"months":months})),
            )
                .into_response()
        }
        Err(_) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "file_read_failed",
            "I couldn't read that file.",
            "Failed to read token data",
        ),
    }
}
async fn month_stats(root: PathBuf, month: String) -> impl IntoResponse {
    if !digits(&month, 6) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_month",
            "I couldn't use that month.",
            "Invalid month format, expected YYYYMM",
        );
    }
    match tokens::usage_stats(&root, Some(&month)) {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(_) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "file_read_failed",
            "I couldn't read that file.",
            "Failed to read token data",
        ),
    }
}
async fn stats_data(root: PathBuf) -> impl IntoResponse {
    let mut response = json!({"stats":{}});
    let path = match solstone_core_journal_io::resolve_journal_path(&root, "stats.json") {
        Ok(path) => path,
        Err(_) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "file_read_failed",
                "I couldn't read that file.",
                "Failed to read stats data",
            );
        }
    };
    if path.is_file() {
        let text = match solstone_core_journal_io::read_text(&path, String::new()) {
            Ok(text) => text,
            Err(_) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "file_read_failed",
                    "I couldn't read that file.",
                    "Failed to read stats data",
                );
            }
        };
        let stats = match serde_json::from_str::<Value>(&text) {
            Ok(value) => value,
            Err(_) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "file_read_failed",
                    "I couldn't read that file.",
                    "Failed to read stats data",
                );
            }
        };
        if let Ok(mtime) = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .and_then(|time| {
                time.duration_since(std::time::UNIX_EPOCH)
                    .map_err(std::io::Error::other)
            })
            .map(|value| value.as_secs_f64())
        {
            response["file_mtime"] = json!(mtime);
        }
        response["stats"] = stats;
    }
    let Some(package_root) = std::env::current_exe().ok().and_then(|executable| {
        executable
            .parent()
            .and_then(solstone_core_journal::resolve_installation_root_from_executable_dir)
    }) else {
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "file_read_failed",
            "I couldn't read that file.",
            "Failed to read stats data",
        );
    };
    let talent_root = package_root.join("solstone/talent");
    let apps_root = package_root.join("solstone/apps");
    let overrides = solstone_core_talent_config::read_talent_overrides(&root)
        .ok()
        .flatten();
    let configs = match solstone_core_talent_config::load_talent_configs(
        &talent_root,
        &apps_root,
        overrides.as_ref(),
        solstone_core_talent_config::TalentFilter {
            r#type: Some("generate"),
            schedule: None,
            include_disabled: false,
        },
    ) {
        Ok(configs) => configs,
        Err(_) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "file_read_failed",
                "I couldn't read that file.",
                "Failed to read stats data",
            );
        }
    };
    let generators = configs
        .into_iter()
        .map(|config| (config.key, Value::Object(config.metadata)))
        .collect::<Map<_, _>>();
    response["generators"] = Value::Object(generators);
    (StatusCode::OK, Json(response)).into_response()
}
fn digits(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|byte| byte.is_ascii_digit())
}
fn api_error(
    status: StatusCode,
    reason_code: &str,
    error: &str,
    detail: &str,
) -> axum::response::Response {
    (
        status,
        Json(json!({"error":error,"reason_code":reason_code,"detail":detail})),
    )
        .into_response()
}

#[cfg(test)]
mod corpus;
