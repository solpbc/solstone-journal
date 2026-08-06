// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod activities;
mod declaration;
mod detected_entities;
mod error;
mod facet_entities;
mod facet_entity_move;
mod identity;
mod lifecycle;
mod logs;
mod map;
mod news;
mod observations;
mod paths;
pub(crate) mod reference_scan;
mod repair;
mod todos;
mod write;

pub use activities::{read_activity_file, write_activity_file};
pub use declaration::{FacetDeclarationSnapshot, read_facet_declaration};
pub use detected_entities::{
    delete_detected_entity, read_detected_entities, save_detected_entity, update_detected_entity,
};
pub use error::{FacetEntityWriteError, FacetRenameError, FacetStoreError, FacetWriteError};
pub use facet_entities::{
    FacetEntityAttachResult, ScopedFacetEntity, add_entity_aka, attach_or_reactivate_entity,
    detach_facet_entity, list_scoped_facet_entities, update_facet_entity_description,
    update_facet_entity_identity,
};
pub use facet_entity_move::{FacetEntityMoveResult, move_facet_entity};
pub use identity::{FacetEntityLinkSnapshot, read_facet_entity_link};
pub use lifecycle::{
    EntityBlockReport, EntityDeleteReport, FacetEntityLifecycleError, block_journal_entity,
    delete_journal_entity,
};
pub use logs::{read_log_file, write_log_file};
pub use map::list_facet_entity_directories;
pub use news::{read_news_file, write_news_file};
pub use observations::{read_facet_entity_observations, write_facet_entity_observations};
pub use reference_scan::EntityReferenceBreakdown;
pub use repair::{
    FacetEntityLinkRepairBranch, FacetEntityLinkRepairError, FacetEntityLinkRepairReport,
    FacetEntityLinkReport, repair_facet_entity_links, repair_facet_entity_links_journal_wide,
};
pub use todos::{read_todo_file, write_todo_file};
pub use write::{
    FacetRenameResult, create_facet, delete_facet_entity_link, rename_facet,
    save_facet_entity_link, set_facet_entity_link_detached, set_facet_muted, update_facet,
};
