// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read routes for the Search Convey surface.

use std::path::PathBuf;

use axum::{Router, routing::get};

mod search;
mod talent_outputs;

#[cfg(test)]
mod corpus;

/// Build the native Search route surface for one journal root.
pub fn api_router(journal_root: PathBuf) -> Router {
    Router::new()
        .merge(search::router(journal_root))
        .route("/app/search/", get(search::shell))
        .route("/app/search", get(search::root))
        .route("/app/search/workspace", get(search::workspace))
}
