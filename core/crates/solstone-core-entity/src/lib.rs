// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity identity, matching primitives, durable-store access, and mutation-support plumbing.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod ambiguity;
mod archive_dedupe;
mod resolution;
mod store;
mod trust_lock;

pub use ambiguity::ambiguity_id;
pub use archive_dedupe::{archive_dedupe_akas, archive_dedupe_emails, archive_dedupe_observations};
pub use resolution::{
    EntityResolution, EntityResolutionEntity, EntityResolutionError, EntityResolutionOutcome,
    ResolutionCandidate, record_entity_resolution, record_entity_resolution_from_name_evidence,
};
pub use solstone_core_journal_io::FileLock;
pub use solstone_core_journal_io::LockError;
pub use solstone_core_journal_io::LockTimeout;
pub use solstone_core_journal_io::MalformedPolicy;
pub use store::{
    AmbiguityChoiceEntity, AmbiguityChoiceRequest, AmbiguityObservation, CanonicalKeyField,
    DEFAULT_ACTIVITY_TS, EncoderIdentity, EntityAmbiguityRemovalReport,
    EntityAmbiguityRescopeError, EntityAmbiguityRescopeReport, EntityIdentityGroupMap,
    EntityIdentityMap, EntityIdentityRepairError, EntityIdentityRepairGuard,
    EntityIdentityRepairRefusal, EntityIdentityRepairReport, EntityIdentityRepairSkip,
    EntityIdentityRepairSkipReason, EntityLifecycleError, EntityMergeError, EntityMergeOptions,
    EntityMergePreview, EntityMergeReport, EntityOperationContext, EntityOperationKind,
    EntityReviewCandidateError, EntitySaveResult, EntityStoreError, EntityUndoError,
    EntityUndoReport, EntityWriteError, HistoryEvent, IdentityMapCacheLoad, IdentityMapLoser,
    IdentityMapLoserReason, IdentitySnapshot, JournalEntity, PreparedHistoryEvent,
    PreparedHistoryOutcome, VoiceprintArchive, VoiceprintEnvelope, VoiceprintItem, VoiceprintKey,
    VoiceprintNpzError, VoiceprintOperationError, VoiceprintRemoval, VoiceprintRemovalReport,
    VoiceprintSkipReasons, accept_merge_candidate, classify_prepared_history, commit_entity_merge,
    create_journal_entity, delete_entity_directory, dismiss_merge_candidate,
    entity_identity_destination_occupied, entity_last_active_day, entity_last_active_ts,
    entity_matches_identity_name, entity_memory_path, entity_path,
    guard_restore_does_not_cross_merge, guard_visible_event_collision, has_journal_principal,
    is_admissible_person, is_valid_entity_type, last_active_day_for_ts, load_all_journal_entities,
    load_entity_voiceprints_file, load_existing_voiceprint_keys, load_merge_candidates,
    load_resolved_ambiguity_choice, normalize_embedding, preview_entity_merge, read_ambiguities,
    read_entity_identity, read_identity_group_map, read_identity_map, read_journal_principal,
    read_prepared_history, read_visible_history, record_ambiguity_choice,
    record_ambiguity_observation, record_merge_candidate, refresh_identity_map_cache,
    remove_entity_ambiguity_references, remove_voiceprints_by_key, repair_entity_identities,
    rescope_facet_ambiguities, restore_journal_entity_version, rewrite_identity_map_cache,
    rewrite_voiceprint_metadata, save_entity_identity, save_voiceprints_batch,
    try_load_entity_voiceprints_file, try_load_entity_voiceprints_in_dir, unblock_journal_entity,
    undo_entity_merge,
};
pub use trust_lock::{
    EntityTrustLock, EntityTrustLockError, hold_entity_trust_lock,
    hold_entity_trust_lock_raw_for_test,
};

#[cfg(test)]
pub(crate) use store::{
    save_entity_identity_with_timeout, set_forced_identity_write_failure,
    set_repair_identity_write_failure_on_attempt, write_history_event_json_for_test,
};

#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod merge_payload_tests;
#[cfg(test)]
mod merge_tests;
#[cfg(test)]
mod resolution_tests;
#[cfg(test)]
mod review_candidate_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod trust_lock_tests;
#[cfg(test)]
mod undo_tests;
#[cfg(test)]
mod voiceprint_tests;

#[cfg(feature = "test-hooks")]
pub type MergeFailureInjectorForTest = dyn Fn(&str, usize) -> bool;

/// Exercise merge interruption boundaries from the component test harness.
#[cfg(feature = "test-hooks")]
pub fn commit_entity_merge_with_injector_for_test(
    journal: &std::path::Path,
    source_id: &str,
    target_id: &str,
    options: EntityMergeOptions,
    fallback_encoder: &EncoderIdentity,
    injector: Option<&MergeFailureInjectorForTest>,
) -> Result<EntityMergeReport, EntityMergeError> {
    store::merge::commit_entity_merge_with_injector(
        journal,
        source_id,
        target_id,
        options,
        fallback_encoder,
        injector,
    )
}
