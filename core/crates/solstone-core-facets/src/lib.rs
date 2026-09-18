// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable read and write access to journal facet state.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod action_log;
mod speculative_facets;
mod store;
mod trust_lock;

pub use action_log::{append_action_log, append_action_log_for_day};
pub use solstone_core_journal_io::AppendError;
pub use speculative_facets::{
    FACET_CANDIDATE_MIN_SEGMENTS, FACET_CANDIDATE_WINDOW_DAYS, SpeculativeFacetCandidate,
    SpeculativeFacetSample, aggregate_speculative_facets,
};
pub use store::{
    ActivityIconMigrationReport, ActivityRecord, ActivityRecordStoreError, AppendOutcome,
    AwarenessStoreError, BackfillReport, ConnectionsHorizon, DetectedEntityInput,
    DetectionUpsertReport, EntityBlockReport, EntityDeleteGuardOutcome, EntityDeleteReport,
    EntityHistoryReference, EntityReferenceBreakdown, EventTopicMigrationReport,
    FacetDeclarationSnapshot, FacetEntityAttachResult, FacetEntityLifecycleError,
    FacetEntityLinkRepairBranch, FacetEntityLinkRepairError, FacetEntityLinkRepairReport,
    FacetEntityLinkReport, FacetEntityLinkSnapshot, FacetEntityMigrationError,
    FacetEntityMoveResult, FacetEntityWriteError, FacetIdError, FacetIdResolveError,
    FacetIdentityError, FacetRelationshipRecord, FacetRenameError, FacetRenameResult,
    FacetReviewCandidateError, FacetStoreError, FacetWriteError, HistoryEntry,
    IncomingObservationRow, LegacyFacetEntityMigrationReport, ObservationChange,
    ObservationEntityResolution, ObservationErrorSource, ObservationLookup, ObservationLookupError,
    ObservationOperationCounts, ObservationPage, ObservationPageItem, ObservationParseSource,
    ObservationReadOrder, ObservationReadQuery, ObservationRow, ObservationStoreError,
    ObservationSummary, ObservationWriteError, ObservationWriteOutcome, ParsedObservations,
    PreparedAnticipationBatch, PreparedNewsReplacement, PreparedObservationBatch,
    PreparedReviewAttachment, PreparedReviewPromotion, Retired, ScopedFacetEntity,
    SeedEntitiesError, SeedEntityBaseOutcome, SeedEntityInput, SeedEntityItemResult,
    SeedEntityOutcome, accept_candidate, activity_is_available, activity_value_or_empty,
    activity_value_string, activity_value_truthy, add_activity, add_observation, allocate_facet_id,
    allocate_facet_id_locked, append_activity_record, append_edit, append_log,
    apply_observation_change, assign_new_facet_id, assign_new_facet_id_locked, backfill_facet_ids,
    block_journal_entity, count_observations, create_facet, delete_created_entity_if_unreferenced,
    delete_detected_entity, delete_facet, delete_facet_entity_link, delete_journal_entity,
    dismiss_candidate, enrich_relationship_with_journal, ensure_daily_facet_id,
    extract_spoken_names, facet_slug, facet_write_identity, get_activity_record,
    humanize_facet_title, is_speakable, is_well_formed_facet_id, list_declared_facet_names,
    list_facet_directories, list_facet_entity_directories, load_activity_records,
    load_all_attached_entities, load_all_facet_relationships,
    load_all_facet_relationships_across_facets, load_candidates, load_current,
    load_detected_entities_recent, load_imports, load_observations_for_query,
    load_recent_entity_names, migrate_custom_activity_icons_to_emoji, migrate_event_topic_keys,
    migrate_legacy_facet_entities, normalize_observation_content, observation_day_counts,
    observation_summary, parse_observation_file, prepare_anticipation_batch,
    prepare_news_replacement, prepare_observation_batch, prepare_review_promotion,
    publish_anticipation_batch, publish_news_replacement, publish_observation_batch,
    publish_review_aliases, publish_review_attachment, read_activity_file, read_detected_entities,
    read_detected_entities_strict, read_detected_entity_names_strict, read_facet_declaration,
    read_facet_entity_link, read_facet_entity_observations, read_live_observations, read_log,
    read_log_file, read_news_file, record_facet_candidates, record_import, record_import_nudge,
    record_import_offer_declined, record_observation_ops_strict, refresh_connections_horizon,
    remove_activity, rename_facet, repair_facet_entity_links,
    repair_facet_entity_links_journal_wide, require_facet_write_identity, resolve_facet_id,
    resolve_observation_entity_dir, review_promotion_snapshot, save_detected_entity,
    save_facet_entity_link, scan_facet_relationships, seed_entities, serialize_observation_rows,
    set_activity_hidden, set_facet_entity_link_detached, set_facet_muted, strip_incoming_facet_id,
    update_activity, update_activity_record, update_detected_entity, update_facet,
    upsert_detection_segment, validate_observation_operations, write_activity_file,
    write_facet_entity_observations, write_log_file, write_news_file,
};
pub use store::{
    add_entity_aka, attach_or_reactivate_entity, detach_facet_entity,
    iter_detected_entity_names_since, iter_detected_entity_names_since_strict,
    list_scoped_facet_entities, list_scoped_facet_entities_tolerant, move_facet_entity,
    update_facet_entity_description, update_facet_entity_identity,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use store::{
    block_journal_entity_with_hook, delete_created_entity_if_unreferenced_with_hook,
    delete_journal_entity_with_hook,
};
pub use trust_lock::{
    FacetTrustLock, FacetTrustLockError, hold_facet_trust_lock, hold_facet_trust_lock_raw_for_test,
};

#[cfg(all(test, feature = "full-tests"))]
mod connections_horizon_tests;
#[cfg(all(test, feature = "full-tests"))]
mod detected_entity_exclusion_fixture_tests;
#[cfg(all(test, feature = "full-tests"))]
mod detected_entity_tests;
#[cfg(all(test, feature = "full-tests"))]
mod facet_entity_fixture_tests;
#[cfg(all(test, feature = "full-tests"))]
mod facet_entity_move_tests;
#[cfg(all(test, feature = "full-tests"))]
mod facet_entity_tests;
#[cfg(all(test, feature = "full-tests"))]
mod fixture_tests;
#[cfg(all(test, feature = "full-tests"))]
mod lifecycle_tests;
#[cfg(all(test, feature = "full-tests"))]
mod observation_tests;
#[cfg(all(test, feature = "full-tests"))]
mod relationship_scans_tests;
#[cfg(all(test, feature = "full-tests"))]
mod review_candidate_tests;
#[cfg(all(test, feature = "full-tests"))]
mod speculative_facets_tests;
#[cfg(all(test, feature = "full-tests"))]
mod store_tests;
#[cfg(all(test, not(feature = "full-tests")))]
mod unit_tests;
