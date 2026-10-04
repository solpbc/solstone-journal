// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native profile-surface endpoints and ledger routing logic.

use std::path::PathBuf;

use axum::{
    Router,
    routing::{get, post},
};

pub mod types;
pub use ledger_fold::read_owner_open_loops;

pub(crate) mod cadence;
pub(crate) mod error;
pub(crate) mod ledger_fold;
pub(crate) mod pagination;
pub(crate) mod profile;
pub(crate) mod relationships;
pub(crate) mod resolution;
pub(crate) mod routes;

#[cfg(test)]
pub(crate) mod test_support;

/// Native HTTP routes for `solstone call profile`.
pub fn routes(journal_root: PathBuf) -> Router {
    Router::new()
        .route("/api/profile/{name}", get(routes::full))
        .route("/api/profile/{name}/brief", get(routes::brief))
        .route("/api/profile/{name}/cadence", get(routes::cadence))
        .route("/api/profiles/active", get(routes::active))
        .route("/api/ledger/{item_id}", get(routes::item))
        .route("/api/ledger/{item_id}/close", post(routes::close))
        .with_state(routes::RouteState { journal_root })
}
