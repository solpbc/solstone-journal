// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only projection of journal/health/brain.json.

pub use solstone_core_callosum::PrivateProofReceive;

mod credential_hold;
mod fingerprint;
mod fixture;
mod inspect;
mod observation;
mod presentation;
mod record;
mod runtime_health;
mod transcription_verification;
mod writer;

#[cfg(test)]
mod corpus_tests;

pub use credential_hold::{
    CredentialHoldError, CredentialHoldSession, CredentialHoldTarget, VolatileCredentialWitness,
};
pub use observation::{
    BrainObservation, BrainObservationSink, ObservationCaller, ObservationFailure,
    ObservationOutcome, ObservationPhase,
};

pub use fingerprint::{
    BundledRuntimeDesired, CREDENTIAL_BINDING_VERSION, CanonicalInput, FingerprintError,
    LaneResolution, StoredCredentialBinding, attach_credential_binding,
    build_active_brain_fingerprint, bundled_runtime_desired_fingerprint, canonical_fingerprint,
    canonical_fingerprint_preserving_array_order, canonical_json,
    canonical_json_preserving_array_order, classify_stored_binding, derive_active_brain_lane,
    endpoint_credential_binding_hmac, fingerprint_sha256, is_cloud_byo_provider,
    managed_credential_binding_hmac, strip_credential_binding,
};
pub use inspect::{
    BrainInspection, BrainProjection, BundledRuntimePrerequisiteAssessment, FingerprintKeyMaterial,
    InspectionStatus, assess_bundled_runtime_prerequisite, brain_fingerprint_key_path,
    brain_refresh_lease_path, brain_state_path, inspect_brain_state,
    inspect_brain_state_with_clock, load_existing_fingerprint_key, probe_file_lease_held,
    project_brain_state, read_fingerprint_key_material,
};
pub use presentation::{
    BrainEvidencePresentation, BrainPresentation, brain_reason_text, lane_label,
    present_brain_inspection, processing_headline_for_reason,
};
pub use record::{
    BrainStateRecord, CredentialRejectionHold, ValidationError, evidence_component_for_reason,
    is_valid_evidence_reason, spp_reason_diagnostic, valid_spp_reason, validate_brain_state_record,
    validate_refresh_probe_outcome,
};
pub use runtime_health::{
    RuntimeRecordInspection, RuntimeRetryError, RuntimeRetryRecord, RuntimeRetryTransaction,
    hold_runtime_operation, inspect_runtime_health, inspect_runtime_retry_token,
    request_runtime_retry, runtime_retry_token_path,
};
pub use transcription_verification::{
    TranscriptionVerification, TranscriptionVerificationError, clear_transcription_verification,
    read_transcription_verification, record_transcription_verification,
    transcription_verification_path,
};
pub use writer::{
    BeginPrerequisiteRenewal, BeginRefreshError, BrainRefreshPermit, CredentialRejectionCarrier,
    CredentialRejectionFailure, CredentialRejectionReceipt, CredentialValidationWitness,
    REACHABLE_WRITE_CASES, RuntimeFailureResult, RuntimeRefusalCaller, WriterError,
    abandon_prerequisite_renewal, abandon_prerequisite_renewal_checked, abandon_refresh,
    abandon_refresh_checked, begin_prerequisite_renewal, begin_prerequisite_renewal_checked,
    begin_refresh, begin_refresh_checked, capture_credential_validation_bound,
    classify_credential_admission_bound, finish_prerequisite_renewal,
    finish_prerequisite_renewal_checked, finish_refresh, finish_refresh_checked,
    finish_refresh_rejected, finish_refresh_rejected_checked, generate_fingerprint_key,
    generate_fingerprint_key_at_root_checked, generate_fingerprint_key_checked, hold_record_lock,
    read_fingerprint_key_material_bound, record_confidential_attestation_refusal,
    record_confidential_attestation_refusal_observed, record_credential_hold_bound,
    record_runtime_failure, retain_credential_rejection_with_config_bound,
    retain_current_credential_rejection_bound,
};

pub use solstone_core_journal_config::read_journal_config;
pub use solstone_core_journal_io::resolve_configured_journal;
