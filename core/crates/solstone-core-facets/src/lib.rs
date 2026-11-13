// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable read and write access to journal facet state.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod action_log;
mod activity_calendar;
mod activity_placement;
mod body_facet;
pub mod entity_trash;
pub mod facet_trash;
mod moment_copies;
mod speculative_facets;
mod store;
mod trust_lock;

pub use action_log::{
    ActionLogScan, append_action_log, append_action_log_checked, append_action_log_for_day,
    append_journal_action_log_at_root_checked, read_journal_action_rows,
};
pub use activity_calendar::{InvitedPerson, ScheduledEvent, calendar_beside, calendar_on_day};
pub use activity_placement::{
    ActivityCourse, PlacedRecord, ResolvedSegment, activity_span, place_activity_records,
};
pub use body_facet::{
    BodyFacetAsk, BodyFacetOfferRead, BodyFacetReadError, apply_body_facet_answer, body_facet_ask,
    body_facet_offer, default_for_unattended,
};
pub use moment_copies::{Moment, merge_moment_copies, moment_key};
pub use solstone_core_entity::facet_links;
pub use solstone_core_entity::hidden_names;
pub use solstone_core_entity::{EntityIdentityMap, read_identity_map};
pub use solstone_core_journal_io::AppendError;
pub use speculative_facets::{
    FACET_CANDIDATE_MIN_SEGMENTS, FACET_CANDIDATE_WINDOW_DAYS, SpeculativeFacetCandidate,
    SpeculativeFacetSample, aggregate_speculative_facets,
};
pub use store::stretch;
pub use store::{
    ActivityIconMigrationReport, ActivityRecord, ActivityRecordStoreError, AppendOutcome,
    AwarenessStoreError, BODY_RECORD_SOURCE, BackfillReport, BodyAccountRefresh, BulkBlockReport,
    BulkJournalError, BulkUnblockReport, ConnectionsHorizon, DEFAULT_FACET, DeclaredFacetInventory,
    DestinationObservation, DetectedEntityInput, DetectedLinkRef, DetectedRecent,
    DetectionUpsertReport, EDIT_BEFORE, EDIT_LINES, EDIT_REMOVED, EDIT_SET, EDIT_SPEAKER,
    EditRefusal, EntityBlockReport, EntityDeleteGuardOutcome, EntityDeleteReport,
    EntityHistoryReference, EntityReferenceBreakdown, EntityUnblockReport,
    EventTopicMigrationReport, FacetDeclarationSnapshot, FacetEntityAttachResult,
    FacetEntityCreateOutcome, FacetEntityLifecycleError, FacetEntityLinkSnapshot,
    FacetEntityMoveResult, FacetEntityWriteError, FacetIdError, FacetIdResolveError,
    FacetIdentityError, FacetRelationshipPlan, FacetRelationshipRecord, FacetReviewCandidateError,
    FacetStoreError, FacetWriteError, FailedEntity, HistoryEntry, IncomingObservationRow,
    OWNER_LINES_ACTOR, ObservationChange, ObservationEntityResolution, ObservationErrorSource,
    ObservationLookup, ObservationLookupError, ObservationOperationCounts, ObservationPage,
    ObservationPageItem, ObservationParseSource, ObservationReadOrder, ObservationReadQuery,
    ObservationRow, ObservationStoreError, ObservationSummary, ObservationWriteError,
    ObservationWriteOutcome, ParsedObservations, PlannedFacetRelationship,
    PreparedObservationBatch, PreparedReviewAttachment, PreparedReviewPromotion,
    RETIRED_FACETS_FILE, RestoredEntity, Retired, RetiredFacet, RetiredFacetState, RetiredFacets,
    RetiredFilesSnapshot, ScopedFacetEntity, SeedEntitiesError, SeedEntityBaseOutcome,
    SeedEntityInput, SeedEntityItemResult, SeedEntityOutcome, StoppedReport,
    UnreadableEntityRecord, accept_candidate, activity_field_held, activity_is_available,
    activity_value_or_empty, activity_value_string, activity_value_truthy, add_activity,
    add_observation, add_observation_for_entity, admit_activity_destination,
    adopt_moved_activity_rows, allocate_facet_id, allocate_facet_id_locked,
    append_activity_lines_edit, append_activity_record, append_activity_record_checked,
    append_edit, append_log, apply_observation_change, assign_new_facet_id,
    assign_new_facet_id_locked, backfill_facet_ids, block_journal_entities, block_journal_entity,
    count_observations, create_facet, delete_created_entity_if_unreferenced, delete_facet,
    delete_facet_entity_link, delete_journal_entity, dismiss_candidate, edit_removable,
    edit_removed, enrich_relationship_with_journal, ensure_daily_facet_id, ensure_default_facet,
    extract_spoken_names, facet_declaration_entry, facet_declaration_file, facet_name_is_free,
    facet_slug, facet_write_identity, first_free_facet_name, get_activity_record,
    hold_activity_enrichment, humanize_facet_title, is_body_record, is_retired_plan, is_speakable,
    is_well_formed_facet_id, list_activity_day_stems, list_declared_facet_names,
    list_facet_directories, list_facet_entity_directories, load_activity_records,
    load_activity_records_readable, load_all_attached_entities, load_all_facet_relationships,
    load_all_facet_relationships_across_facets, load_candidates, load_current,
    load_detected_entities_recent, load_detected_entities_recent_at, load_detected_link_days,
    load_imports, load_observations_for_query, load_recent_entity_names,
    migrate_custom_activity_icons_to_emoji, migrate_event_topic_keys,
    normalize_observation_content, observation_day_counts, observation_summary,
    observe_declared_facet_inventory, observe_facet_declaration, observe_facet_destination,
    observe_facet_write_identity, parse_activity_records_strict, parse_observation_file,
    parse_retired_facets, plan_facet_entity_relationships, prepare_observation_batch,
    prepare_review_promotion, publish_observation_batch, publish_observation_batch_checked,
    publish_review_aliases, publish_review_attachment, queue_body_entries_once,
    read_activity_definitions, read_activity_file, read_activity_records_readable,
    read_activity_records_strict, read_connections_horizon, read_detected_entities,
    read_detected_entities_strict, read_detected_entity_names_strict, read_facet_declaration,
    read_facet_entity_link, read_facet_entity_observations, read_live_observations, read_log,
    read_log_file, read_news_file, read_retired_facets, record_facet_candidates,
    record_facet_candidates_checked, record_import, record_import_nudge,
    record_import_offer_declined, record_observation_ops_strict, record_retired_facet,
    refresh_body_account, refresh_connections_horizon, remove_activity, remove_activity_edit,
    require_declared_facet, require_facet_write_identity, require_observed_facet_write_identity,
    resolve_facet_id, resolve_observation_entity_dir, restore_retired_files, retired_facet_entry,
    retired_facets_path, retitle_facet, review_promotion_snapshot, save_detected_entity,
    save_facet_entity_link, scan_facet_relationships, seed_entities, serialize_observation_rows,
    set_activity_hidden, set_activity_hidden_checked, set_facet_entity_link_detached,
    set_facet_muted, settle_imported_facet_id, snapshot_retired_files, strip_incoming_facet_id,
    unblock_journal_entity, unblock_journal_operation, update_activity, update_activity_record,
    update_activity_record_checked, update_activity_record_with, update_detected_entity,
    update_facet, upsert_detection_segment, validate_observation_operations, write_activity_file,
    write_facet_entity_observations, write_log_file,
};
pub use store::{
    BodyMoveOutcome, BodyRecordPlace, LineRemoval, OpenBodyMove, body_record_key,
    complete_body_move, move_body_record, open_body_moves, remove_activity_line_if,
    write_body_record_with,
};
pub use store::{
    HiddenNameWrite, HiddenNamesError, check_hidden_names_readable, hide_detected_name,
    hide_detected_name_at, merge_hidden_names_locked, queue_every_facet_detections,
    queue_facet_detections, queue_linked_facet_detections, unhide_detected_name,
};
pub use store::{StravaConvertedLine, append_strava_converted, read_strava_converted};
pub use store::{
    add_entity_aka, attach_or_reactivate_entity, attach_or_reactivate_entity_for_owner,
    create_facet_entity_if_absent, detach_facet_entity, iter_detected_entity_names_since,
    iter_detected_entity_names_since_strict, list_scoped_facet_entities,
    list_scoped_facet_entities_tolerant, list_scoped_facet_entities_tolerant_with_resolved_ids,
    list_scoped_facet_entities_with_resolved_ids, move_facet_entity,
    update_facet_entity_description, update_facet_entity_identity,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use store::{
    block_journal_entity_with_hook, create_facet_entity_if_absent_with_hook,
    delete_created_entity_if_unreferenced_with_hook, delete_journal_entity_with_hook,
    reset_undone_operation_for_test, test_hooks, unblock_journal_entity_with_hook,
};
pub use store::{strava_lines_may_be_folded, superseded_record_ids};
pub use trust_lock::{
    FacetTrustLock, FacetTrustLockError, hold_facet_trust_lock, hold_facet_trust_lock_raw_for_test,
};

#[cfg(test)]
mod body_facet_tests;
#[cfg(test)]
mod bulk_hide_tests;
#[cfg(all(test, feature = "full-tests"))]
mod connections_horizon_tests;
#[cfg(test)]
mod create_if_absent_tests;
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
#[cfg(test)]
mod hidden_names_tests;
#[cfg(test)]
mod hide_unhide_tests;
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
