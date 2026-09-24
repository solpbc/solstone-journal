// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable read and write access to journal entity state.

mod ambiguity;
mod census;
mod create;
mod derived;
mod entity_paths;
mod error;
mod history;
mod identity;
mod journal_entities;
mod lifecycle;
mod map;
pub(crate) mod merge;
pub(crate) mod merge_payload;
mod merge_rollback;
#[cfg(all(test, unix))]
pub(crate) use merge_rollback::with_source_sync_failure;
pub mod observations;
mod paths;
mod reconcile;
mod repair;
pub(crate) mod review_candidates;
pub mod review_policy;
mod undo;
pub(crate) mod voiceprints;
mod write;

pub use ambiguity::{
    EntityAmbiguityRemovalReport, EntityAmbiguityRescopeError, EntityAmbiguityRescopeReport,
    load_resolved_ambiguity_choice, read_ambiguities, remove_entity_ambiguity_references,
    rescope_facet_ambiguities,
};
pub use census::{CensusEntity, IdentityCensus, scan_identity_census};
pub use create::create_journal_entity;
pub use derived::{
    DEFAULT_ACTIVITY_TS, entity_last_active_day, entity_last_active_ts,
    entity_matches_identity_name, is_valid_entity_type, last_active_day_for_ts,
};
pub use entity_paths::{entity_memory_path, entity_path};
pub use error::EntityStoreError;
pub use history::{
    HistoryEvent, PreparedHistoryEvent, guard_restore_does_not_cross_merge,
    guard_visible_event_collision, read_prepared_history, read_visible_history,
};
pub use identity::{IdentitySnapshot, entity_identity_destination_occupied, read_entity_identity};
pub use journal_entities::{JournalEntity, is_admissible_person, load_all_journal_entities};
pub use lifecycle::{
    EntityLifecycleError, delete_entity_directory, has_journal_principal, read_journal_principal,
    restore_journal_entity_version, unblock_journal_entity,
};
pub use map::{
    EntityIdentityGroupMap, EntityIdentityMap, IdentityMapLoser, IdentityMapLoserReason,
    read_identity_group_map, read_identity_map,
};
pub use merge::{
    EntityMergeError, EntityMergeOptions, EntityMergePreview, EntityMergeReport,
    commit_entity_merge, preview_entity_merge,
};
pub use observations::{
    HistoryEntry, IncomingObservationRow, ObservationChange, ObservationEntityResolution,
    ObservationErrorSource, ObservationLookup, ObservationLookupError, ObservationOperationCounts,
    ObservationPage, ObservationPageItem, ObservationParseSource, ObservationReadOrder,
    ObservationReadQuery, ObservationRow, ObservationStoreError, ObservationSummary,
    ObservationWriteError, ObservationWriteOutcome, ParsedObservations, PreparedObservationBatch,
    Retired, add_observation, apply_observation_change, apply_ops_to_parsed, count_observations,
    facet_entity_observations_path, list_facet_entity_directories, load_observations_for_query,
    normalize_observation_content, observation_day_counts, observation_summary,
    parse_observation_content, parse_observation_file, read_live_observations,
    record_observation_ops_strict, resolve_observation_entity_dir, retry_add_for_test,
    retry_record_for_test, serialize_observation_rows,
};
pub use reconcile::{PreparedHistoryOutcome, classify_prepared_history};
pub use repair::{
    EntityIdentityRepairError, EntityIdentityRepairGuard, EntityIdentityRepairRefusal,
    EntityIdentityRepairReport, EntityIdentityRepairSkip, EntityIdentityRepairSkipReason,
    repair_entity_identities,
};
pub use review_candidates::{
    EntityReviewCandidateError, PreparedMergeProposals, accept_merge_candidate,
    dismiss_merge_candidate, load_merge_candidates, prepare_merge_proposals,
    publish_merge_proposals, record_merge_candidate,
};
pub use review_policy::{
    AmbiguityGroupResolveRequest, ENTITY_REVIEW_POLICY_VERSION, PREFIX_CUTOFF,
    REVIEW_SWEEP_RECEIPT_RELATIVE_PATH, ReviewRestoreTarget, TYPO_FLOOR, ambiguity_group_revision,
    apply_ambiguity_review_policy, apply_merge_candidate_review_policy, is_placeholder_query,
    resolve_ambiguity_group, restore_review, sweep_entity_review_policy, validate_review_object,
};
pub use undo::{EntityUndoError, EntityUndoReport, undo_entity_merge};
pub use voiceprints::{
    CanonicalKeyField, EncoderIdentity, VoiceprintArchive, VoiceprintEnvelope, VoiceprintItem,
    VoiceprintKey, VoiceprintNpzError, VoiceprintOperationError, VoiceprintRemoval,
    VoiceprintRemovalReport, VoiceprintSkipReasons, load_entity_voiceprints_file,
    load_existing_voiceprint_keys, normalize_embedding, remove_voiceprints_by_key,
    rewrite_voiceprint_metadata, save_voiceprints_batch, try_load_entity_voiceprints_file,
    try_load_entity_voiceprints_in_dir,
};
pub use write::{
    AmbiguityChoiceEntity, AmbiguityChoiceRequest, AmbiguityObservation, EntityOperationContext,
    EntityOperationKind, EntitySaveResult, EntityWriteError, IdentityMapCacheLoad,
    PreparedIdentityChange, dismiss_ambiguity, prepare_identity_changes, publish_identity_change,
    record_ambiguity_choice, record_ambiguity_observation, refresh_identity_map_cache,
    rewrite_identity_map_cache, save_entity_identity,
};

#[cfg(test)]
pub(crate) use repair::set_repair_identity_write_failure_on_attempt;
#[cfg(test)]
pub(crate) use undo::undo_entity_merge_with_injector;
#[cfg(test)]
pub(crate) use write::{
    save_entity_identity_with_timeout, set_forced_history_apply_failure,
    set_forced_identity_write_failure, write_history_event_json_for_test,
};
