// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::Router;

use crate::Clock;

pub(crate) mod copy;
pub(crate) mod dates;
pub(crate) mod routes;
pub(crate) mod store;

pub(crate) fn routes(root: PathBuf, clock: Clock) -> Router {
    routes::routes(root, clock)
}
