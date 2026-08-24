// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Immutable owner-intent links.
//!
//! A link is create-only and never overwritten. Links are keyed by exact
//! intent serial inside a per-operation directory, so a later-dirty successor
//! records its own link without rewriting the link of the transition it
//! follows.
//!
//! Admission absence therefore means the operation's link set is exactly
//! empty, which is the condition that holds on a fresh `begin` before any
//! allocation. The per-operation directory is created by linkage itself, so
//! its **existence** is the durable evidence that the operation has entered
//! linkage: it cannot appear before an intent exists, and a crash between
//! creating the directory and creating the link is exactly the
//! intent-without-link state that resumes by creating the link at the live
//! serial. Presence is deliberately a directory probe and not a listing, so
//! admission performs no scan.

use crate::error::ConvergenceError;
use crate::layout::{LINKS, operation_links_dir};
use crate::registry::RegistrySection;
use crate::walk::open_dir;

/// Read: whether the operation has entered linkage. Never creates.
pub(crate) fn operation_link_present(
    section: &RegistrySection<'_>,
    operation_id: &str,
) -> Result<bool, ConvergenceError> {
    let Some(links) = open_dir(section.registry(), LINKS)? else {
        return Ok(false);
    };
    Ok(open_dir(&links, &operation_links_dir(operation_id))?.is_some())
}
