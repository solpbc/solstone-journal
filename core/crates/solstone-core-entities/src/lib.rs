// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native HTTP route surface for entities and facet curation.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod action_log;
mod deferred_delete;
mod model;
mod router;

pub use router::{router, router_with_delete_window};

#[cfg(test)]
mod router_tests;
