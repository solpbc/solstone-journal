// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read routes for the Settings Convey surface.

use std::path::PathBuf;

use axum::{Router, routing::get};

mod activities;
mod assets;
mod chat;
mod config;
mod convey;
mod facets;
mod http;
mod icons;
mod keys;
mod logs;
mod observe;
mod processing;
mod sol_voice;
mod state;
mod storage;
mod sync;
mod transcribe;
mod vision;

pub fn routes(journal_root: PathBuf) -> Router {
    let config_root = journal_root.clone();
    let state_root = journal_root.clone();
    let convey_root = journal_root.clone();
    let observe_root = journal_root.clone();
    let transcribe_root = journal_root.clone();
    let processing_root = journal_root.clone();
    let sol_voice_root = journal_root.clone();
    let throttled_root = journal_root.clone();
    let chat_root = journal_root.clone();
    let vision_root = journal_root.clone();
    let facets_root = journal_root.clone();
    let muted_facets_root = journal_root.clone();
    let facet_root = journal_root.clone();
    let facet_activities_root = journal_root.clone();
    let logs_root = journal_root.clone();
    let facet_logs_root = journal_root.clone();
    let storage_root = journal_root.clone();
    let sync_root = journal_root;
    Router::new()
        .route("/app/settings/", get(assets::shell))
        .route("/app/settings/workspace", get(assets::workspace))
        .route("/app/settings/static/settings.js", get(assets::settings_js))
        .route("/app/settings/facets/{slug}", get(assets::shell))
        .route(
            "/app/settings/api/state",
            get(move || state::get(state_root.clone())),
        )
        .route(
            "/app/settings/api/config",
            get(move || config::get(config_root.clone())),
        )
        .route(
            "/app/settings/api/convey/status",
            get(move || convey::status(convey_root.clone())),
        )
        .route(
            "/app/settings/api/observe",
            get(move || observe::get(observe_root.clone())),
        )
        .route(
            "/app/settings/api/transcribe",
            get(move || transcribe::get(transcribe_root.clone())),
        )
        .route(
            "/app/settings/api/processing",
            get(move || processing::get(processing_root.clone())),
        )
        .route(
            "/app/settings/api/sol_voice",
            get(move || sol_voice::get(sol_voice_root.clone())),
        )
        .route(
            "/app/settings/api/sol_voice/throttled",
            get(move |query| sol_voice::throttled(throttled_root.clone(), query)),
        )
        .route(
            "/app/settings/api/chat",
            get(move || chat::get(chat_root.clone())),
        )
        .route("/app/settings/api/validate-keys", get(keys::get))
        .route(
            "/app/settings/api/vision",
            get(move || vision::get(vision_root.clone())),
        )
        .route(
            "/app/settings/api/facets",
            get(move |query| facets::list(facets_root.clone(), query)),
        )
        .route(
            "/app/settings/api/facets/muted",
            get(move || facets::muted(muted_facets_root.clone())),
        )
        .route(
            "/app/settings/api/facet/{facet_name}",
            get(move |path| facets::get_one(facet_root.clone(), path)),
        )
        .route(
            "/app/settings/api/activities/defaults",
            get(activities::defaults),
        )
        .route(
            "/app/settings/api/facet/{facet_name}/activities",
            get(move |path| activities::for_facet(facet_activities_root.clone(), path)),
        )
        .route(
            "/app/settings/api/logs",
            get(move |query| logs::journal(logs_root.clone(), query)),
        )
        .route(
            "/app/settings/api/facet/{facet_name}/logs",
            get(move |path, query| logs::facet(facet_logs_root.clone(), path, query)),
        )
        .route(
            "/app/settings/api/storage",
            get(move || storage::get(storage_root.clone())),
        )
        .route(
            "/app/settings/api/sync",
            get(move || sync::get(sync_root.clone())),
        )
        .route("/app/settings/api/icons", get(icons::search))
}

#[cfg(test)]
mod build_contract;
#[cfg(test)]
mod corpus;
#[cfg(test)]
mod router_contracts;
#[cfg(test)]
mod test_support;
