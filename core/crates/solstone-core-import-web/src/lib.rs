// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read routes for the Import Convey surface.

use std::path::PathBuf;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};

mod assets;
mod callosum;
mod content;
mod http;
mod imports;
mod ingest;
mod journal_sources;
mod lifecycle;
mod multipart;

#[cfg(test)]
mod corpus;
#[cfg(test)]
mod test_support;

#[derive(Clone)]
pub(crate) struct AppState {
    root: PathBuf,
}

pub fn routes(journal_root: PathBuf) -> Router {
    Router::new()
        .route("/app/import/workspace", get(assets::workspace))
        .route("/app/import/background", get(assets::background_not_found))
        .route("/app/import/static/{*path}", get(assets::static_asset))
        .route("/app/import/api/sources", get(imports::sources))
        .route("/app/import/api/list", get(imports::list))
        .route("/app/import/api/guide/{source}", get(assets::guide))
        .route("/app/import/api/save", post(lifecycle::save))
        .route("/app/import/api/save-path", post(lifecycle::save_path))
        .route("/app/import/api/meta", post(lifecycle::meta))
        .route("/app/import/api/start", post(lifecycle::start))
        .route(
            "/app/import/journal/{prefix}/ingest/segments",
            post(ingest::segments),
        )
        .route(
            "/app/import/journal/{prefix}/ingest/entities",
            post(ingest::entities),
        )
        .route(
            "/app/import/journal/{prefix}/ingest/imports",
            post(ingest::imports),
        )
        .route(
            "/app/import/journal/{prefix}/ingest/config",
            post(ingest::config),
        )
        .route(
            "/app/import/api/journal-sources/list",
            get(journal_sources::list),
        )
        .route(
            "/app/import/api/journal-sources/{name}/status",
            get(journal_sources::status),
        )
        .route(
            "/app/import/api/journal-sources/{name}/staged",
            get(journal_sources::staged),
        )
        .route(
            "/app/import/api/journal-sources/create",
            post(journal_sources::create),
        )
        .route(
            "/app/import/api/journal-sources/{name}/revoke",
            post(journal_sources::revoke),
        )
        .route(
            "/app/import/journal/{key_prefix}/manifest/{area}",
            get(journal_sources::manifest),
        )
        .route(
            "/app/import/api/{timestamp}/content/{item_id}",
            get(content::detail),
        )
        .route("/app/import/api/{timestamp}/content", get(content::list))
        .route("/app/import/api/{timestamp}", get(imports::detail))
        .route("/app/import/{timestamp}", get(assets::detail_shell))
        .layer(DefaultBodyLimit::max(multipart::MAX_BODY_BYTES))
        .with_state(AppState { root: journal_root })
}
