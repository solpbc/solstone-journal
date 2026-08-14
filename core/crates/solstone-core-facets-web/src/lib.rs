// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read routes for the Timeline Convey surface.

use std::path::PathBuf;

use axum::Router;

mod assets;
mod awareness;
pub mod clock;
mod curation;
mod date_nav;
mod http;
mod news;
mod pdf;
pub mod segments;
mod timeline;

pub use clock::Clock;

pub fn routes(journal_root: PathBuf, clock: Clock) -> Router {
    timeline::routes(journal_root.clone(), clock.clone())
        .merge(news::routes(journal_root.clone(), clock.clone()))
        .merge(curation::routes(journal_root.clone()))
        .merge(awareness::routes(journal_root, clock))
}

#[cfg(test)]
mod corpus;
#[cfg(test)]
mod test_support;
