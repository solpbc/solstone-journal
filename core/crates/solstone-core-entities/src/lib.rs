// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native HTTP route surface for entities and facet curation.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod action_log;
mod deferred_delete;
mod model;
mod router;

pub use model::{ATTENDANCE_KINDS, ENTITIES_COPY, compose_connections_horizon_note};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use router::router_with_delete_window;
pub use router::{api_router, api_router_with_delete_window};
#[cfg(test)]
pub(crate) use router::{router, router_with_delete_window_and_registry};

#[cfg(test)]
mod router_tests;
