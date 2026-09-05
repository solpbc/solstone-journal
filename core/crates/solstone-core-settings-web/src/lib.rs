// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read routes for the Settings Convey surface.

use std::path::PathBuf;

use axum::{
    Router,
    routing::{get, post, put},
};
use solstone_core_journal_io::LockOptions;

mod activities;
mod assets;
mod config;
mod convey;
mod facets;
mod http;
mod icons;
mod keys;
mod logs;
mod observe;
mod processing;
mod request_body;
mod retention;
mod retention_executor;
mod state;
mod storage;
mod sync;
mod transcribe;
mod vision;

pub fn routes(journal_root: PathBuf) -> Router {
    routes_with_lock_options(journal_root, LockOptions::default())
}

pub fn routes_with_lock_options(journal_root: PathBuf, config_lock_options: LockOptions) -> Router {
    let config_get_root = journal_root.clone();
    let config_put_root = journal_root.clone();
    let config_post_root = journal_root.clone();
    let state_root = journal_root.clone();
    let convey_root = journal_root.clone();
    let observe_get_root = journal_root.clone();
    let observe_put_root = journal_root.clone();
    let observe_post_root = journal_root.clone();
    let transcribe_root = journal_root.clone();
    let processing_root = journal_root.clone();
    let keys_root = journal_root.clone();
    let vision_get_root = journal_root.clone();
    let vision_put_root = journal_root.clone();
    let facets_root = journal_root.clone();
    let muted_facets_root = journal_root.clone();
    let facet_get_root = journal_root.clone();
    let facet_put_root = journal_root.clone();
    let facet_delete_root = journal_root.clone();
    let facet_create_root = journal_root.clone();
    let facet_rename_root = journal_root.clone();
    let facet_activities_get_root = journal_root.clone();
    let facet_activities_add_root = journal_root.clone();
    let facet_activities_update_root = journal_root.clone();
    let facet_activities_delete_root = journal_root.clone();
    let logs_root = journal_root.clone();
    let facet_logs_root = journal_root.clone();
    let storage_root = journal_root.clone();
    let storage_put_root = journal_root.clone();
    let purge_root = journal_root.clone();
    let prune_logs_root = journal_root.clone();
    let sync_get_root = journal_root.clone();
    let sync_put_root = journal_root;
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
            get(move || config::get(config_get_root.clone()))
                .put(move |body| config::update(config_put_root.clone(), config_lock_options, body))
                .post(move |body| {
                    config::update(config_post_root.clone(), config_lock_options, body)
                }),
        )
        .route(
            "/app/settings/api/convey/status",
            get(move || convey::status(convey_root.clone())),
        )
        .route(
            "/app/settings/api/observe",
            get(move || observe::get(observe_get_root.clone()))
                .put(move |body| {
                    observe::update(observe_put_root.clone(), config_lock_options, body)
                })
                .post(move |body| {
                    observe::update(observe_post_root.clone(), config_lock_options, body)
                }),
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
            "/app/settings/api/validate-keys",
            get(keys::get)
                .post(move |body| keys::post(keys_root.clone(), config_lock_options, body)),
        )
        .route(
            "/app/settings/api/vision",
            get(move || vision::get(vision_get_root.clone())).put(move |body| {
                vision::update(vision_put_root.clone(), config_lock_options, body)
            }),
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
            get(move |path| facets::get_one(facet_get_root.clone(), path))
                .put(move |path, body| facets::update(facet_put_root.clone(), path, body))
                .delete(move |path, body| facets::delete(facet_delete_root.clone(), path, body)),
        )
        .route(
            "/app/settings/api/facet",
            post(move |body| facets::create(facet_create_root.clone(), body)),
        )
        .route(
            "/app/settings/api/facet/{facet_name}/rename",
            post(move |path, body| facets::rename(facet_rename_root.clone(), path, body)),
        )
        .route(
            "/app/settings/api/activities/defaults",
            get(activities::defaults),
        )
        .route(
            "/app/settings/api/facet/{facet_name}/activities",
            get(move |path| activities::for_facet(facet_activities_get_root.clone(), path)).post(
                move |path, body| activities::add(facet_activities_add_root.clone(), path, body),
            ),
        )
        .route(
            "/app/settings/api/facet/{facet_name}/activities/{activity_id}",
            put(move |path, body| {
                activities::update(facet_activities_update_root.clone(), path, body)
            })
            .delete(move |path, body| {
                activities::remove(facet_activities_delete_root.clone(), path, body)
            }),
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
            get(move || storage::get(storage_root.clone())).put(move |body| {
                retention::update(storage_put_root.clone(), config_lock_options, body)
            }),
        )
        .route(
            "/app/settings/api/storage/list",
            post(move |body| retention::purge(purge_root.clone(), body)),
        )
        .route(
            "/app/settings/api/storage/prune-logs",
            post(move |body| retention::prune_logs(prune_logs_root.clone(), body)),
        )
        .route(
            "/app/settings/api/sync",
            get(move || sync::get(sync_get_root.clone()))
                .put(move |body| sync::update(sync_put_root.clone(), body)),
        )
        .route("/app/settings/api/icons", get(icons::search))
}

#[cfg(test)]
mod corpus;
#[cfg(test)]
mod mutations;
#[cfg(test)]
mod retention_tests;
#[cfg(test)]
mod router_contracts;
#[cfg(test)]
mod settings_corpus_divergence;
#[cfg(test)]
mod test_support;
