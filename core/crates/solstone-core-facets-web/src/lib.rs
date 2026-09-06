// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read routes for the activities, news, curation, awareness, and PDF Convey surfaces.

use std::path::PathBuf;

use axum::Router;

mod activities;
mod assets;
mod awareness;
pub mod clock;
mod curation;
mod date_nav;
mod http;
mod news;
mod pdf;
pub mod segments;

pub use clock::Clock;
pub use date_nav::date_nav_index;

pub fn routes(journal_root: PathBuf, clock: Clock) -> Router {
    activities::routes(journal_root.clone(), clock.clone())
        .merge(news::routes(journal_root.clone(), clock.clone()))
        .merge(curation::routes(journal_root.clone()))
        .merge(awareness::routes(journal_root, clock))
}

#[cfg(test)]
mod corpus;
#[cfg(test)]
mod test_support;
