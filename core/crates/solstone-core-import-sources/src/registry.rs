// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Ordered source registry; real detector signatures are defined by a later wave.
//!
//! The module is named `archive`, while the owner-facing registry name is
//! `journal_archive`; the latter intentionally matches the fixture contract.

use crate::ImportSourcesError;

/// Return the first ordered source claimed by the injected predicate.
pub fn first_claimed<'a>(
    order: &'a [&'a str],
    mut claims: impl FnMut(&str) -> bool,
) -> Option<&'a str> {
    order.iter().copied().find(|name| claims(name))
}

/// Reserved registry seam; its real operation signature is defined by a later wave.
pub fn reserved_seam() -> Result<(), ImportSourcesError> {
    Err(ImportSourcesError::Unimplemented { module: "registry" })
}
