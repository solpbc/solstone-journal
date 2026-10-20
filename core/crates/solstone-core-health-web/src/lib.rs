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
mod brain;
mod brain_action;
mod host;
mod journal_data;
mod logs;
pub mod search_index;
mod talent_failures;
mod work_view;

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
    let media_root = journal_root.clone();
    let media_clock = clock.clone();
    let log_root = journal_root.clone();
    let info_root = journal_root.clone();
    let brain_root = journal_root.clone();
    let retry_root = journal_root.clone();
    let work_retry_root = journal_root.clone();
    let speech_root = journal_root.clone();
    let speech_clock = clock.clone();
    Router::new()
        .route("/app/health/", get(assets::shell))
        .route("/app/health/workspace", get(assets::workspace))
        .route("/app/health/static/{*name}", get(assets::static_asset))
        .route(
            "/app/health/api/state",
            get(move || state(state_root.clone(), state_clock.clone())),
        )
        .route(
            "/app/health/api/media-backlog",
            get(move || journal_data::media_backlog(media_root.clone(), media_clock.now())),
        )
        .route(
            "/app/health/api/log",
            get(move |query| logs::get(log_root.clone(), query)),
        )
        .route("/app/health/api/info", get(move || info(info_root.clone())))
        .route(
            "/app/health/api/speech-setup",
            get(move || speech_setup(speech_root.clone(), speech_clock.clone())),
        )
        .route(
            "/app/health/api/brain/check",
            post(move || actions::check_brain(brain_root.clone())),
        )
        .route("/app/health/api/retry-import", post(actions::retry_import))
        .route(
            "/app/health/api/reprocess",
            post(move |body| actions::reprocess(retry_root.clone(), body)),
        )
        .route(
            "/app/health/api/retry-work",
            post(move |body| actions::retry_work(work_retry_root.clone(), body)),
        )
        .merge(api_router(journal_root))
}

async fn speech_setup(root: PathBuf, clock: Clock) -> Response {
    spawn_blocking_response(OwnerReadRole::HealthSpeechSetup, move || {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute());
        let presentation = solstone_core_system::speech_setup::read_speech_setup(
            &root,
            std::env::consts::OS,
            std::env::consts::ARCH,
            home.as_deref(),
            clock.now(),
        );
        Json(presentation).into_response()
    })
    .await
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
            now.with_timezone(&solstone_core_journal_config::owner_zone(&root))
                .naive_local(),
        );
        let eval = match not_yet {
            Some(not_yet) => solstone_core_system_health::not_yet_evaluation(not_yet),
            None => solstone_core_system_health::evaluate_backlog_status(
                backlog.as_ref(),
                generated_at.as_deref(),
                now,
            ),
        };
        let mut search_index =
            search_index::evaluate_search_index(&root, &search_index::FsIndexMetadata, now);
        // Before the first night, search makes no claim unless it is failing.
        if search_index.state != "failing" {
            match not_yet {
                Some(solstone_core_system_health::NotYet::FirstNight) => {
                    search_index.text = solstone_core_system_health::NOT_YET_SEARCH.to_owned();
                }
                // Nothing catches search up until processing is set up; the
                // verdict above already says so.
                Some(solstone_core_system_health::NotYet::AwaitingEngine(_)) => {
                    search_index.text = String::new();
                }
                None => {}
            }
        }
        let work = solstone_core_system::work_inventory::read(&root);
        let policy = solstone_core_system::work_policy::WorkPolicy::read(&root);
        let work_error = work.as_ref().err().cloned();
        let (retry_records, retry_errors) = solstone_core_system::work_retry::list(&root);
        let (history_confirmed, confirmation_available) = policy
            .map(|p| {
                (
                    p.history_confirmed,
                    matches!(
                        p.lane,
                        solstone_core_system::work_policy::ThinkingLane::OwnKeys
                    ),
                )
            })
            .unwrap_or((false, false));
        let (items, ok) = talent_failures::today(&root);
        let count = items.len();
        Json(json!({
            "work": {
                "inventory": work.ok().flatten().map(work_view::summarize),
                "error": work_error,
                "retry_records": retry_records.into_iter().filter_map(|record| record.unit.id().ok().map(|id| json!({"id": id, "unit": record.unit, "reason": record.reason, "attempt": record.attempt}))).collect::<Vec<_>>(),
                "retry_errors": retry_errors,
                "history_confirmed": history_confirmed,
                "confirmation_available": confirmation_available,
            },
            "backlog": {
                "verdict": eval.verdict,
                "scope": "recent days",
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
            "search_index": {
                "state": search_index.state,
                "text": search_index.text,
                "coverage": search_index.coverage,
                "observed_failure": search_index.observed_failure,
                "index_activity_at_ms": search_index.index_activity_at_ms,
                "index": search_index.index,
                "classification": search_index.classification,
                "diagnostics": search_index.diagnostics,
            },
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
