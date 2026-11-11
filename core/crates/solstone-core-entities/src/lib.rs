// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native HTTP route surface for entities and facet curation.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod id_shape;
mod model;
mod router;
mod tidy;
mod trash_delete;

pub use model::{ATTENDANCE_KINDS, ENTITIES_COPY, compose_connections_horizon_note, edge_aliases};
pub use router::api_router;
#[cfg(test)]
pub(crate) use router::router;

#[cfg(test)]
mod router_tests;
