// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable read and write access to journal facet state.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod action_log;
mod speculative_facets;
mod store;
mod trust_lock;

pub use action_log::append_action_log;
pub use solstone_core_journal_io::AppendError;
pub use speculative_facets::{
    FACET_CANDIDATE_MIN_SEGMENTS, FACET_CANDIDATE_WINDOW_DAYS, SpeculativeFacetCandidate,
    SpeculativeFacetSample, aggregate_speculative_facets,
};
pub use store::{
    ActivityIconMigrationReport, DetectedEntityInput, DetectionUpsertReport, EntityBlockReport,
    EntityDeleteGuardOutcome, EntityDeleteReport, EntityHistoryReference, EntityReferenceBreakdown,
    EventTopicMigrationReport, FacetDeclarationSnapshot, FacetEntityAttachResult,
    FacetEntityLifecycleError, FacetEntityLinkRepairBranch, FacetEntityLinkRepairError,
    FacetEntityLinkRepairReport, FacetEntityLinkReport, FacetEntityLinkSnapshot,
    FacetEntityMigrationError, FacetEntityMoveResult, FacetEntityWriteError,
    FacetRelationshipRecord, FacetRenameError, FacetRenameResult, FacetReviewCandidateError,
    FacetStoreError, FacetWriteError, LegacyFacetEntityMigrationReport,
    ObservationEntityResolution, ObservationLookup, ObservationLookupError,
    ObservationOperationCounts, ObservationWriteError, ScopedFacetEntity, SeedEntitiesError,
    SeedEntityBaseOutcome, SeedEntityInput, SeedEntityItemResult, SeedEntityOutcome,
    accept_candidate, add_activity, add_observation, block_journal_entity, count_observations,
    create_facet, delete_created_entity_if_unreferenced, delete_detected_entity, delete_facet,
    delete_facet_entity_link, delete_journal_entity, dismiss_candidate,
    enrich_relationship_with_journal, extract_spoken_names, facet_slug, is_speakable,
    list_facet_directories, list_facet_entity_directories, load_all_attached_entities,
    load_all_facet_relationships, load_all_facet_relationships_across_facets, load_candidates,
    load_detected_entities_recent, load_observations, load_observations_for_query,
    load_recent_entity_names, migrate_custom_activity_icons_to_emoji, migrate_event_topic_keys,
    migrate_legacy_facet_entities, observation_day_counts, read_activity_file,
    read_detected_entities, read_facet_declaration, read_facet_entity_link,
    read_facet_entity_observations, read_log_file, read_news_file, read_todo_file,
    record_facet_candidates, record_observation_ops, remove_activity, rename_facet,
    repair_facet_entity_links, repair_facet_entity_links_journal_wide,
    resolve_observation_entity_dir, save_detected_entity, save_facet_entity_link,
    save_observations, scan_facet_relationships, seed_entities, set_facet_entity_link_detached,
    set_facet_muted, update_activity, update_detected_entity, update_facet,
    upsert_detection_segment, write_activity_file, write_facet_entity_observations, write_log_file,
    write_news_file, write_todo_file,
};
pub use store::{
    add_entity_aka, attach_or_reactivate_entity, detach_facet_entity,
    iter_detected_entity_names_since, list_scoped_facet_entities,
    list_scoped_facet_entities_tolerant, move_facet_entity, update_facet_entity_description,
    update_facet_entity_identity,
};
pub use trust_lock::{
    FacetTrustLock, FacetTrustLockError, hold_facet_trust_lock, hold_facet_trust_lock_raw_for_test,
};

#[cfg(test)]
mod detected_entity_exclusion_fixture_tests;
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
mod review_candidate_tests;
#[cfg(test)]
mod speculative_facets_tests;
#[cfg(test)]
mod store_tests;
