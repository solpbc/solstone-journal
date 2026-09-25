// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native routes for the Health Convey surface.

use axum::{
    Json, Router,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;
use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};

mod actions;
mod assets;
mod backlog;
mod backlog_reasons;
mod brain;
mod brain_action;
mod host;
mod journal_data;
mod logs;
pub mod search_freshness;
mod talent_failures;

#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>);

impl Clock {
    pub fn real() -> Self {
        Self(Arc::new(Utc::now))
    }
    pub fn fixed(dt: DateTime<Utc>) -> Self {
        Self(Arc::new(move || dt))
    }
    pub fn new(now: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        Self(Arc::new(now))
    }
    pub fn now(&self) -> DateTime<Utc> {
        (self.0)()
    }
}

pub fn routes(journal_root: PathBuf) -> Router {
    routes_with_clock(journal_root, Clock::real())
}

pub fn routes_with_clock(journal_root: PathBuf, clock: Clock) -> Router {
    let state_root = journal_root.clone();
    let state_clock = clock.clone();
    let log_root = journal_root.clone();
    let info_root = journal_root.clone();
    let brain_root = journal_root.clone();
    let retry_root = journal_root.clone();
    Router::new()
        .route("/app/health/", get(assets::shell))
        .route("/app/health/workspace", get(assets::workspace))
        .route("/app/health/static/{*name}", get(assets::static_asset))
        .route(
            "/app/health/api/state",
            get(move || state(state_root.clone(), state_clock.clone())),
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
            "/app/health/api/reprocess",
            post(move |body| actions::reprocess(retry_root.clone(), body)),
        )
        .merge(api_router(journal_root))
}

/// Read-only journal-data health routes used by `solstone call health`.
pub fn api_router(journal_root: PathBuf) -> Router {
    journal_data::api_router(journal_root)
}

async fn state(root: PathBuf, clock: Clock) -> Response {
    spawn_blocking_response(OwnerReadRole::HealthState, move || {
        let now = clock.now();
        let (generated_at, backlog) = backlog::load(&root);
        // One rule with home and stats: before the nightly run's first chance,
        // a summary that doesn't exist yet is calm, not unclear.
        let not_yet = solstone_core_system_health::summary_not_yet(
            &root,
            now.with_timezone(&chrono::Local).naive_local(),
        );
        let eval = match not_yet {
            Some(not_yet) => solstone_core_system_health::not_yet_evaluation(not_yet),
            None => solstone_core_system_health::evaluate_backlog_status(
                backlog.as_ref(),
                generated_at.as_deref(),
                now,
            ),
        };
        let indexer_phase = backlog
            .as_ref()
            .and_then(|b| b.get("indexer_phase"))
            .and_then(solstone_core_system_health::IndexerPhase::from_json_value);
        let search_summary_freshness = if backlog
            .as_ref()
            .is_none_or(|b| b.get("degraded") == Some(&serde_json::Value::Bool(true)))
        {
            solstone_core_system_health::SummaryFreshness::Unknown
        } else {
            eval.freshness
        };
        let mut search_index = search_freshness::evaluate_search_freshness(
            &root,
            &search_freshness::FsIndexMetadata,
            indexer_phase.as_ref(),
            search_summary_freshness,
            now,
        );
        if search_index.text == search_freshness::SEARCH_TEXT_UNCLEAR {
            match not_yet {
                Some(solstone_core_system_health::NotYet::FirstNight) => {
                    search_index.text = solstone_core_system_health::NOT_YET_SEARCH.to_owned();
                }
                // Nothing catches search up until processing is set up; the
                // verdict above already says so.
                Some(solstone_core_system_health::NotYet::AwaitingEngine) => {
                    search_index.text = String::new();
                }
                None => {}
            }
        }
        let (items, ok) = talent_failures::today(&root);
        let count = items.len();
        Json(json!({
            "backlog": {
                "verdict": eval.verdict,
                "not_yet": not_yet.map(solstone_core_system_health::NotYet::as_str),
                "pending_days": eval.pending_days,
                "oldest_pending_day": eval.oldest_pending_day,
                "freshness": {
                    "state": eval.freshness.as_str(),
                    "generated_at": generated_at,
                },
                "unfinished_activities": eval.unfinished_activities,
                "stuck_rows": backlog::stuck_rows(backlog.as_ref()),
                "copy": backlog::copy(),
            },
            "search_index": search_index,
            "agent_errors": {
                "items": items,
                "ok": ok,
                "count": count,
                "label": errors_today_label(count, ok),
            },
        }))
        .into_response()
    })
    .await
}

fn errors_today_label(count: usize, ok: bool) -> &'static str {
    if ok && count == 1 {
        "error today"
    } else {
        "errors today"
    }
}
async fn info(root: PathBuf) -> Response {
    spawn_blocking_response(OwnerReadRole::HealthInfo, move || {
        Json(json!({"hostname":host::hostname(),"brain":brain::snapshot(&root)})).into_response()
    })
    .await
}

#[cfg(test)]
mod acceptance_tests;
#[cfg(test)]
mod corpus;
#[cfg(test)]
mod test_support;
