// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read routes for the Import Convey surface.

use std::path::PathBuf;

use axum::{Router, routing::get};

mod assets;
mod content;
mod http;
mod imports;
mod journal_sources;

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
            "/app/import/api/{timestamp}/content/{item_id}",
            get(content::detail),
        )
        .route("/app/import/api/{timestamp}/content", get(content::list))
        .route("/app/import/api/{timestamp}", get(imports::detail))
        .route("/app/import/{timestamp}", get(assets::detail_shell))
        .with_state(AppState { root: journal_root })
}
