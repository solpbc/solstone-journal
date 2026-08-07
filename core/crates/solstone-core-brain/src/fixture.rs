// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

const LOCAL_CONTRACT: &str = include_str!("../../../fixtures/local_contract.json");
#[cfg(test)]
const BRAIN_PROJECTION: &str = include_str!("../../../fixtures/brain_projection.json");

static CONTRACT: OnceLock<LocalContract> = OnceLock::new();
#[cfg(test)]
static BRAIN_STATE_KEYS: OnceLock<Vec<String>> = OnceLock::new();
#[cfg(test)]
static PROJECTION: OnceLock<BrainProjectionFixture> = OnceLock::new();

pub(crate) fn local_contract() -> &'static LocalContract {
    CONTRACT.get_or_init(|| {
        serde_json::from_str(LOCAL_CONTRACT)
            .expect("core/fixtures/local_contract.json must be valid")
    })
}

#[cfg(test)]
pub(crate) fn brain_state_keys() -> &'static [String] {
    BRAIN_STATE_KEYS.get_or_init(|| {
        #[derive(Deserialize)]
        struct Keys {
            brain_state: BTreeMap<String, Value>,
        }
        serde_json::from_str::<Keys>(LOCAL_CONTRACT)
            .expect("core/fixtures/local_contract.json must be valid")
            .brain_state
            .into_keys()
            .collect()
    })
}

#[cfg(test)]
pub(crate) fn projection_fixture() -> &'static BrainProjectionFixture {
    PROJECTION.get_or_init(|| {
        serde_json::from_str(BRAIN_PROJECTION)
            .expect("core/fixtures/brain_projection.json must be valid")
    })
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct LocalContract {
    pub brain_state: BrainStateVocabulary,
    pub canonical_fingerprint: CanonicalFingerprintFixture,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct BrainStateVocabulary {
    pub schema_version: u64,
    pub checking_ttl_seconds: u64,
    pub fingerprint_schema_version: u64,
    pub fingerprint_key_bytes: usize,
    pub paths: BrainStatePaths,
    pub lanes: Vec<String>,
    pub lane_components: BTreeMap<String, Vec<String>>,
    pub component_order: Vec<String>,
    pub aggregate_states: Vec<String>,
    pub component_statuses: Vec<String>,
    pub reason_codes: Vec<String>,
    pub reason_to_aggregate: BTreeMap<String, String>,
    pub runtime_failure_aggregates: Vec<String>,
    pub evidence_reason_codes: BTreeMap<String, Vec<String>>,
    pub projection_only_reason_codes: Vec<String>,
    pub runtime_phases: Vec<String>,
    pub runtime_reason_codes: Vec<String>,
    pub runtime_phase_to_reason: BTreeMap<String, Value>,
    pub runtime_reason_to_brain_reason: BTreeMap<String, String>,
    pub incoherent_runtime_phase_reason_codes: Vec<Vec<String>>,
    pub runtime_transition_phases: Vec<String>,
    pub config_diagnostic_fields: Vec<String>,
    pub diagnostic_metadata_schemas: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    pub record_fields: RecordFields,
    pub cloud_byo_providers: Vec<String>,
    pub provider_env_by_name: BTreeMap<String, String>,
    pub runtime_failure_components: Vec<String>,
    pub runtime_failure_rejected_reasons: Vec<String>,
    pub prerequisite_renewal_statuses: Vec<String>,
    pub inspection_statuses: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BrainStatePaths {
    pub record: String,
    pub fingerprint_key: String,
    pub refresh_lease: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RecordFields {
    #[serde(rename = "top_level")]
    pub top_level: Vec<String>,
    pub checking: Vec<String>,
    pub evidence: Vec<String>,
    #[serde(rename = "evidence_component")]
    pub component: Vec<String>,
    #[serde(rename = "runtime_failure_marker")]
    pub runtime_failure_marker: Vec<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct CanonicalFingerprintFixture {
    pub algorithm: Value,
    pub vectors: Vec<CanonicalVector>,
    pub canonical_digest_vectors: Vec<CanonicalDigestVector>,
    pub vector_hmac_key_hex: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct CanonicalVector {
    pub name: String,
    pub input_json: Option<String>,
    pub input_repr: Option<String>,
    pub canonical_json: String,
    pub sha256: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct CanonicalDigestVector {
    pub name: String,
    pub input_json: String,
    pub wrapped_canonical_json: String,
    pub hmac_sha256: String,
}

#[cfg(test)]
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct BrainProjectionFixture {
    pub now: String,
    pub hmac_key_hex: String,
    pub bundled_runtime_fingerprint_sha256: String,
    pub unrelated_fingerprint: String,
    pub configs: BTreeMap<String, Value>,
    pub records: BTreeMap<String, Value>,
    pub malformed_records: BTreeMap<String, Value>,
    pub runtime_health: BTreeMap<String, Value>,
    pub projection: Vec<ProjectionCase>,
    pub validation: Vec<ValidationCase>,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectionCase {
    pub record: String,
    pub config: String,
    pub runtime_health: String,
    pub refresh_permit_active: bool,
    pub hmac_key_present: bool,
    pub projection: ProjectionValue,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectionValue {
    pub aggregate_state: String,
    pub reason_code: Option<String>,
    pub active_lane: Option<String>,
    pub active_provider: Option<String>,
    pub active_model: Option<String>,
    pub fingerprint_sha256: Option<String>,
    pub runtime_transition_in_progress: bool,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
pub(crate) struct ValidationCase {
    pub name: String,
    pub accepted: bool,
    pub error: Option<String>,
}
