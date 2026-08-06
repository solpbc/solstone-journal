// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable read and write access to journal facet state.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod store;
mod trust_lock;

pub use store::{
    EntityBlockReport, EntityDeleteGuardOutcome, EntityDeleteReport, EntityHistoryReference,
    EntityReferenceBreakdown, FacetDeclarationSnapshot, FacetEntityAttachResult,
    FacetEntityLifecycleError, FacetEntityLinkRepairBranch, FacetEntityLinkRepairError,
    FacetEntityLinkRepairReport, FacetEntityLinkReport, FacetEntityLinkSnapshot,
    FacetEntityMoveResult, FacetEntityWriteError, FacetRelationshipRecord, FacetRenameError,
    FacetRenameResult, FacetStoreError, FacetWriteError, ObservationEntityResolution,
    ObservationLookup, ObservationLookupError, ObservationOperationCounts, ObservationWriteError,
    ScopedFacetEntity, add_observation, block_journal_entity, count_observations, create_facet,
    delete_created_entity_if_unreferenced, delete_detected_entity, delete_facet_entity_link,
    delete_journal_entity, enrich_relationship_with_journal, list_facet_entity_directories,
    load_all_facet_relationships, load_all_facet_relationships_across_facets, load_observations,
    load_observations_for_query, observation_day_counts, read_activity_file,
    read_detected_entities, read_facet_declaration, read_facet_entity_link,
    read_facet_entity_observations, read_log_file, read_news_file, read_todo_file,
    record_observation_ops, rename_facet, repair_facet_entity_links,
    repair_facet_entity_links_journal_wide, resolve_observation_entity_dir, save_detected_entity,
    save_facet_entity_link, save_observations, scan_facet_relationships,
    set_facet_entity_link_detached, set_facet_muted, update_detected_entity, update_facet,
    write_activity_file, write_facet_entity_observations, write_log_file, write_news_file,
    write_todo_file,
};
pub use store::{
    add_entity_aka, attach_or_reactivate_entity, detach_facet_entity, list_scoped_facet_entities,
    move_facet_entity, update_facet_entity_description, update_facet_entity_identity,
};
pub use trust_lock::{FacetTrustLock, FacetTrustLockError, hold_facet_trust_lock};

#[cfg(test)]
mod detected_entity_tests;
#[cfg(test)]
mod facet_entity_fixture_tests;
#[cfg(test)]
mod facet_entity_move_tests;
#[cfg(test)]
mod facet_entity_tests;
#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod observation_tests;
#[cfg(test)]
mod relationship_scans_tests;
#[cfg(test)]
mod store_tests;
