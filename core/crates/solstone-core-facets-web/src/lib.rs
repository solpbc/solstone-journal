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
pub use curation::to_review_count;
pub use date_nav::date_nav_index;

/// Activities per day across the facets that aren't muted, as the activities
/// calendar counts them.
pub fn activity_day_counts(
    journal_root: &std::path::Path,
) -> std::collections::BTreeMap<String, usize> {
    activities::activity_day_counts(journal_root)
}

/// [`activity_day_counts`] for the days from `from` to `to`, reading only
/// those days and the few around them a body group's members can be filed on.
pub fn activity_day_counts_between(
    journal_root: &std::path::Path,
    from: &str,
    to: &str,
) -> std::collections::BTreeMap<String, usize> {
    activities::activity_day_counts_between(journal_root, from, to)
}

/// The years with a day on or before `through` that [`activity_day_counts`]
/// counts, each year read only until a day in it has a count.
pub fn activity_years(
    journal_root: &std::path::Path,
    through: &str,
) -> std::collections::BTreeSet<String> {
    activities::activity_years(journal_root, through)
}

/// Non-hidden Experiences per anchor day across non-muted facets.
pub fn experience_day_counts(
    journal_root: &std::path::Path,
) -> std::collections::BTreeMap<String, usize> {
    activities::experience_day_counts(journal_root)
}

/// [`experience_day_counts`] restricted to anchor days from `from` to `to`.
pub fn experience_day_counts_between(
    journal_root: &std::path::Path,
    from: &str,
    to: &str,
) -> std::collections::BTreeMap<String, usize> {
    activities::experience_day_counts_between(journal_root, from, to)
}

/// Years on or before `through` that hold at least one non-hidden Experience.
pub fn experience_years(
    journal_root: &std::path::Path,
    through: &str,
) -> std::collections::BTreeSet<String> {
    activities::experience_years(journal_root, through)
}
pub use facet_name::valid_facet;

pub fn routes(journal_root: PathBuf, clock: Clock) -> Router {
    activities::routes(journal_root.clone(), clock.clone())
        .merge(curation::routes(journal_root.clone()))
        .merge(awareness::routes(journal_root, clock))
        .layer(axum::middleware::from_fn(http::localize))
}

#[cfg(test)]
mod corpus;
#[cfg(test)]
mod test_support;
