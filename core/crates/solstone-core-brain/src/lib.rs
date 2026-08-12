// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only projection of journal/health/brain.json.

mod fingerprint;
mod fixture;
mod inspect;
mod presentation;
mod record;
mod runtime_health;
mod writer;

#[cfg(test)]
mod corpus_tests;

pub use fingerprint::{
    CanonicalInput, FingerprintError, LaneResolution, build_active_brain_fingerprint,
    canonical_fingerprint, canonical_json, derive_active_brain_lane, fingerprint_sha256,
};
pub use inspect::{
    BrainInspection, BrainProjection, InspectionStatus, brain_fingerprint_key_path,
    brain_refresh_lease_path, brain_state_path, inspect_brain_state, load_existing_fingerprint_key,
    probe_file_lease_held, project_brain_state,
};
pub use presentation::{BrainEvidencePresentation, BrainPresentation, present_brain_inspection};
pub use record::{
    BrainStateRecord, ValidationError, validate_brain_state_record, validate_refresh_probe_outcome,
};
pub use runtime_health::{RuntimeRecordInspection, inspect_runtime_health};
pub use writer::{
    BeginPrerequisiteRenewal, BeginRefreshError, BrainRefreshPermit, REACHABLE_WRITE_CASES,
    RuntimeFailureResult, WriterError, abandon_prerequisite_renewal, abandon_refresh,
    begin_prerequisite_renewal, begin_refresh, finish_prerequisite_renewal, finish_refresh,
    generate_fingerprint_key, record_runtime_failure,
};

pub use solstone_core_journal_config::read_journal_config;
pub use solstone_core_journal_io::resolve_configured_journal;
