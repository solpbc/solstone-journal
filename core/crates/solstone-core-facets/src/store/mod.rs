// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod activities;
mod declaration;
mod error;
mod identity;
mod logs;
mod map;
mod news;
mod observations;
mod paths;
mod repair;
mod todos;
mod write;

pub use activities::{read_activity_file, write_activity_file};
pub use declaration::{FacetDeclarationSnapshot, read_facet_declaration};
pub use error::{FacetRenameError, FacetStoreError, FacetWriteError};
pub use identity::{FacetEntityLinkSnapshot, read_facet_entity_link};
pub use logs::{read_log_file, write_log_file};
pub use map::list_facet_entity_directories;
pub use news::{read_news_file, write_news_file};
pub use observations::{read_facet_entity_observations, write_facet_entity_observations};
pub use repair::{
    FacetEntityLinkRepairBranch, FacetEntityLinkRepairError, FacetEntityLinkRepairReport,
    FacetEntityLinkReport, repair_facet_entity_links, repair_facet_entity_links_journal_wide,
};
pub use todos::{read_todo_file, write_todo_file};
pub use write::{
    FacetRenameResult, create_facet, rename_facet, save_facet_entity_link, set_facet_muted,
    update_facet,
};
