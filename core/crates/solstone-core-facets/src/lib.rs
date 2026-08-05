// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable read and write access to journal facet state.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod store;
mod trust_lock;

pub use store::{
    FacetDeclarationSnapshot, FacetEntityLinkRepairBranch, FacetEntityLinkRepairError,
    FacetEntityLinkRepairReport, FacetEntityLinkReport, FacetEntityLinkSnapshot, FacetRenameError,
    FacetRenameResult, FacetStoreError, FacetWriteError, create_facet,
    list_facet_entity_directories, read_activity_file, read_facet_declaration,
    read_facet_entity_link, read_facet_entity_observations, read_log_file, read_news_file,
    read_todo_file, rename_facet, repair_facet_entity_links,
    repair_facet_entity_links_journal_wide, save_facet_entity_link, set_facet_muted, update_facet,
    write_activity_file, write_facet_entity_observations, write_log_file, write_news_file,
    write_todo_file,
};
pub use trust_lock::{FacetTrustLock, FacetTrustLockError, hold_facet_trust_lock};

#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod store_tests;
