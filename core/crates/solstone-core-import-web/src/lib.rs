// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read routes for the Import Convey surface.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};

mod assets;
mod audio_archive;
mod callosum;
mod content;
mod embedded_route;
mod http;
mod imports;
mod journal_archive;
mod lifecycle;
#[cfg(unix)]
pub use lifecycle::register_admitted_folder;
pub mod mobile_import_contract;
mod multipart;
mod preview;
mod progress;
mod save_stream;
pub(crate) mod start_lock;

pub use embedded_route::{
    CapturedImport, ImportAdmission, ImportRouteRequest, ImportRouteSlot, RetainedImportRoute,
};

#[cfg(all(test, unix))]
mod native_folder_tests;

#[cfg(test)]
mod corpus;
#[cfg(test)]
mod test_support;

#[derive(Clone)]
pub(crate) struct AppState {
    root: PathBuf,
    route: Option<Arc<ImportRouteSlot>>,
}

pub fn routes(journal_root: PathBuf) -> Router {
    router(journal_root, None)
}

/// The same import paths as [`routes`], with a retained route installed.
pub fn routes_with_embedded(journal_root: PathBuf, route: Arc<ImportRouteSlot>) -> Router {
    router(journal_root, Some(route))
}

fn router(journal_root: PathBuf, route: Option<Arc<ImportRouteSlot>>) -> Router {
    lifecycle::sweep_abandoned_strava_uploads(&journal_root);
    Router::new()
        .route("/app/import/", get(assets::shell))
        .route("/app/import/workspace", get(assets::workspace))
        .route("/app/import/background", get(assets::background_not_found))
        .route("/app/import/static/{*path}", get(assets::static_asset))
        .route("/app/import/api/sources", get(imports::sources))
        .route("/app/import/api/list", get(imports::list))
        .route("/app/import/api/guide/{source}", get(assets::guide))
        .route(
            "/app/import/api/journal-archive/export",
            get(journal_archive::export),
        )
        .route(
            "/app/import/api/journal-archive/preview",
            post(journal_archive::preview),
        )
        .route(
            mobile_import_contract::SAVE_PATH,
            post(lifecycle::save).layer(DefaultBodyLimit::max(
                solstone_core_convey_http::serve::REQUEST_BODY_LIMIT,
            )),
        )
        .route("/app/import/api/save-path", post(lifecycle::save_path))
        .route("/app/import/api/meta", post(lifecycle::meta))
        .route("/app/import/api/preview", post(preview::preview))
        .route(mobile_import_contract::START_PATH, post(lifecycle::start))
        .route(
            "/app/import/api/{timestamp}/content/{item_id}",
            get(content::detail),
        )
        .route("/app/import/api/{timestamp}/content", get(content::list))
        .route("/app/import/api/{timestamp}", get(imports::detail))
        .route("/app/import/{timestamp}", get(assets::shell))
        .layer(DefaultBodyLimit::max(multipart::MAX_BODY_BYTES))
        .with_state(AppState {
            root: journal_root,
            route,
        })
}
