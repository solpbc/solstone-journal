// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read routes for the activities, curation, and awareness Convey surfaces.

use std::path::PathBuf;

use axum::Router;

mod activities;
mod assets;
mod awareness;
pub mod clock;
mod curation;
mod date_nav;
mod facet_name;
mod http;
pub mod segments;

pub use clock::Clock;
pub use date_nav::date_nav_index;

/// Activities per day across the facets that aren't muted, as the activities
/// calendar counts them.
pub fn activity_day_counts(
    journal_root: &std::path::Path,
) -> std::collections::BTreeMap<String, usize> {
    activities::activity_day_counts(journal_root)
}
pub use facet_name::valid_facet;

pub fn routes(journal_root: PathBuf, clock: Clock) -> Router {
    activities::routes(journal_root.clone(), clock.clone())
        .merge(curation::routes(journal_root.clone()))
        .merge(awareness::routes(journal_root, clock))
}

#[cfg(test)]
mod corpus;
#[cfg(test)]
mod test_support;
