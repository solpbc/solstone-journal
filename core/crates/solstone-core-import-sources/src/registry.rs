// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Ordered source registry; real detector signatures are defined by a later wave.

use crate::ImportSourcesError;

/// Ordered names for file importer routing.
///
/// The module is named `archive`, while the owner-facing registry name is
/// `journal_archive`; the latter intentionally matches the fixture contract.
pub const ORDERED_FILE_IMPORTER_NAMES: &[&str] = &[
    "ics",
    "obsidian",
    "claude",
    "chatgpt",
    "kindle",
    "gemini",
    "document",
    "image",
    "journal_archive",
    "apple_health",
    "oura",
];

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
