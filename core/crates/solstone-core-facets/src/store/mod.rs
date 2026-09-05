// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod activities;
mod activity_records;
mod awareness;
mod connections_horizon;
mod declaration;
mod detected_entities;
mod detected_entity_activity;
mod error;
mod event_topic_migration;
mod facet_entities;
mod facet_entity_move;
mod identity;
mod legacy_entity_migration;
mod lifecycle;
mod logs;
mod map;
mod news;
mod observations;
mod paths;
mod recent_names;
pub(crate) mod reference_scan;
mod relationship_scans;
mod repair;
mod review_candidates;
mod seeding;
mod write;

pub use activities::{
    ActivityIconMigrationReport, add_activity, migrate_custom_activity_icons_to_emoji,
    read_activity_file, remove_activity, update_activity, write_activity_file,
};
pub use activity_records::{
    ActivityRecord, ActivityRecordStoreError, AppendOutcome, activity_is_available,
    activity_value_or_empty, activity_value_string, activity_value_truthy, append_activity_record,
    append_edit, get_activity_record, load_activity_records, set_activity_hidden,
    update_activity_record,
};
pub use awareness::{
    AwarenessStoreError, append_log, load_current, load_imports, read_log, record_import,
    record_import_nudge, record_import_offer_declined,
};
pub use connections_horizon::{ConnectionsHorizon, refresh_connections_horizon};
pub use declaration::{FacetDeclarationSnapshot, read_facet_declaration};
pub use detected_entities::{
    DetectedEntityInput, DetectionUpsertReport, delete_detected_entity, read_detected_entities,
    save_detected_entity, update_detected_entity, upsert_detection_segment,
};
#[cfg(all(test, feature = "full-tests"))]
pub(crate) use detected_entity_activity::{cutoff_day, exclusion_tier};
pub use detected_entity_activity::{
    iter_detected_entity_names_since, iter_detected_entity_names_since_strict,
    load_detected_entities_recent, read_detected_entity_names_strict,
};
pub use error::{
    FacetEntityWriteError, FacetRenameError, FacetStoreError, FacetWriteError,
    ObservationLookupError, ObservationWriteError,
};
pub use event_topic_migration::{EventTopicMigrationReport, migrate_event_topic_keys};
pub use facet_entities::{
    FacetEntityAttachResult, ScopedFacetEntity, add_entity_aka, attach_or_reactivate_entity,
    detach_facet_entity, list_scoped_facet_entities, list_scoped_facet_entities_tolerant,
    update_facet_entity_description, update_facet_entity_identity,
};
pub use facet_entity_move::{FacetEntityMoveResult, move_facet_entity};
pub use identity::{FacetEntityLinkSnapshot, read_facet_entity_link};
pub use legacy_entity_migration::{
    FacetEntityMigrationError, LegacyFacetEntityMigrationReport, migrate_legacy_facet_entities,
};
pub use lifecycle::{
    EntityBlockReport, EntityDeleteGuardOutcome, EntityDeleteReport, EntityHistoryReference,
    FacetEntityLifecycleError, block_journal_entity, delete_created_entity_if_unreferenced,
    delete_journal_entity,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use lifecycle::{
    block_journal_entity_with_hook, delete_created_entity_if_unreferenced_with_hook,
    delete_journal_entity_with_hook,
};
pub use logs::{read_log_file, write_log_file};
pub use map::{list_declared_facet_names, list_facet_directories, list_facet_entity_directories};
pub use news::{read_news_file, write_news_file};
pub use observations::{
    ObservationEntityResolution, ObservationLookup, ObservationOperationCounts, add_observation,
    count_observations, load_observations, load_observations_for_query, observation_day_counts,
    read_facet_entity_observations, record_observation_ops, resolve_observation_entity_dir,
    save_observations, write_facet_entity_observations,
};
#[cfg(all(test, feature = "full-tests"))]
pub(crate) use observations::{retry_add_for_test, retry_record_for_test};
pub use recent_names::{
    extract_spoken_names, is_speakable, load_all_attached_entities, load_recent_entity_names,
};
pub use reference_scan::EntityReferenceBreakdown;
pub use relationship_scans::{
    FacetRelationshipRecord, enrich_relationship_with_journal, load_all_facet_relationships,
    load_all_facet_relationships_across_facets, scan_facet_relationships,
};
pub use repair::{
    FacetEntityLinkRepairBranch, FacetEntityLinkRepairError, FacetEntityLinkRepairReport,
    FacetEntityLinkReport, repair_facet_entity_links, repair_facet_entity_links_journal_wide,
};
pub use review_candidates::{
    FacetReviewCandidateError, accept_candidate, dismiss_candidate, facet_slug,
    humanize_facet_title, load_candidates, record_facet_candidates,
};
pub use seeding::{
    SeedEntitiesError, SeedEntityBaseOutcome, SeedEntityInput, SeedEntityItemResult,
    SeedEntityOutcome, seed_entities,
};
pub use write::{
    FacetRenameResult, create_facet, delete_facet, delete_facet_entity_link, rename_facet,
    save_facet_entity_link, set_facet_entity_link_detached, set_facet_muted, update_facet,
};
