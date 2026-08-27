// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native routes for the Health Convey surface.

use axum::{
    Json, Router,
    routing::{get, post},
};
use serde_json::json;
use std::path::PathBuf;

mod actions;
mod assets;
mod backlog;
mod backlog_reasons;
mod brain;
mod brain_action;
mod host;
mod journal_data;
mod logs;
mod talent_failures;

pub fn routes(journal_root: PathBuf) -> Router {
    let state_root = journal_root.clone();
    let log_root = journal_root.clone();
    let info_root = journal_root.clone();
    let brain_root = journal_root.clone();
    let retry_root = journal_root.clone();
    let restart_root = journal_root.clone();
    Router::new()
        .route("/app/health/", get(assets::shell))
        .route("/app/health/workspace", get(assets::workspace))
        .route("/app/health/static/{*name}", get(assets::static_asset))
        .route(
            "/app/health/api/state",
            get(move || state(state_root.clone())),
        )
        .route(
            "/app/health/api/log",
            get(move |query| logs::get(log_root.clone(), query)),
        )
        .route("/app/health/api/info", get(move || info(info_root.clone())))
        .route(
            "/app/health/api/brain/check",
            post(move || actions::check_brain(brain_root.clone())),
        )
        .route("/app/health/api/retry-import", post(actions::retry_import))
        .route(
            "/app/health/api/restart-capture",
            post(move |body| actions::restart_capture(restart_root.clone(), body)),
        )
        .route(
            "/app/health/api/reprocess",
            post(move |body| actions::reprocess(retry_root.clone(), body)),
        )
        .merge(api_router(journal_root))
}

/// Read-only journal-data health routes used by `solstone call health`.
pub fn api_router(journal_root: PathBuf) -> Router {
    journal_data::api_router(journal_root)
}

async fn state(root: PathBuf) -> Json<serde_json::Value> {
    let backlog = backlog::load(&root);
    let (items, ok) = talent_failures::today(&root);
    let count = items.len();
    Json(
        json!({"backlog":{"verdict":backlog::verdict(backlog.as_ref()),"stuck_rows":backlog::stuck_rows(backlog.as_ref()),"copy":backlog::copy()},"agent_errors":{"items":items,"ok":ok,"count":count,"label":errors_today_label(count,ok)}}),
    )
}

fn errors_today_label(count: usize, ok: bool) -> &'static str {
    if ok && count == 1 {
        "error today"
    } else {
        "errors today"
    }
}
async fn info(root: PathBuf) -> Json<serde_json::Value> {
    Json(json!({"hostname":host::hostname(),"brain":brain::snapshot(&root)}))
}

#[cfg(test)]
mod corpus;
#[cfg(test)]
mod test_support;
