// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read routes for the Chat and Search Convey surfaces.

use std::path::PathBuf;

use axum::{Router, routing::get};

mod chat;
pub mod chat_state;
mod journal_read;
mod search;
mod talent_outputs;

#[cfg(test)]
mod corpus;

/// Build the native Chat and Search route surface for one journal root.
pub fn api_router(journal_root: PathBuf) -> Router {
    Router::new()
        .merge(chat::router(journal_root.clone()))
        .merge(search::router(journal_root))
        .route("/app/search/", get(search::shell))
        .route("/app/search", get(search::root))
        .route("/app/search/workspace", get(search::workspace))
}
