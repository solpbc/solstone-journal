// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::fcntl::{Flock, FlockArg};
use serde_json::{Map, Value};

use crate::fingerprint::{
    LaneResolution, StoredCredentialBinding, build_active_brain_fingerprint,
    classify_stored_binding, derive_active_brain_lane, endpoint_binding_parts,
    endpoint_credential_binding_hmac, managed_credential_binding_hmac,
};
use crate::fixture::local_contract;
use crate::record::{BrainStateRecord, reduce_evidence_with_runtime, validate_brain_state_record};
use crate::runtime_health::{RuntimeRecordInspection, inspect_runtime_health};

pub(crate) const FINGERPRINT_KEY_BYTES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectionStatus {
    Ok,
    Corrupt,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BrainProjection {
    pub aggregate_state: String,
    pub reason_code: Option<String>,
    pub active_lane: Option<String>,
    pub active_provider: Option<String>,
    pub active_model: Option<String>,
    pub fingerprint_sha256: Option<String>,
    pub runtime_transition_in_progress: bool,
}

/// Read-only bundled-runtime prerequisite facts for owner orchestration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundledRuntimePrerequisiteAssessment {
    pub reason_code: Option<String>,
    pub desired_fingerprint_sha256: Option<String>,
    pub phase: Option<String>,
    pub runtime_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BrainInspection {
    pub status: InspectionStatus,
    pub projection: BrainProjection,
    pub error: Option<String>,
    /// The record exactly as it was read, or `None` when there is nothing valid
    /// to carry. Deliberately the raw parsed value rather than a re-serialized
    /// view of the typed record: this is what is on disk, so it cannot drift
    /// from it, and a durable format's null-versus-absent distinctions are not
    /// re-decided by a `Serialize` impl.
    pub record: Option<Value>,
}

pub fn brain_state_path(journal_path: &Path) -> PathBuf {
    journal_path.join(&local_contract().brain_state.paths.record)
}

pub fn brain_fingerprint_key_path(journal_path: &Path) -> PathBuf {
    journal_path.join(&local_contract().brain_state.paths.fingerprint_key)
}

pub fn brain_refresh_lease_path(journal_path: &Path) -> PathBuf {
    journal_path.join(&local_contract().brain_state.paths.refresh_lease)
}

pub fn load_existing_fingerprint_key(journal_path: &Path) -> Option<[u8; FINGERPRINT_KEY_BYTES]> {
    let bytes = fs::read(brain_fingerprint_key_path(journal_path)).ok()?;
    bytes.try_into().ok()
}

/// Whether `health/brain-fingerprint.key` can bind a credential.
///
/// `NotFound` is [`FingerprintKeyMaterial::Absent`]. Any other length or IO
/// error is [`FingerprintKeyMaterial::Unavailable`] and must not be replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FingerprintKeyMaterial {
    Absent,
    Ready([u8; FINGERPRINT_KEY_BYTES]),
    Unavailable,
}

pub fn read_fingerprint_key_material(journal_path: &Path) -> FingerprintKeyMaterial {
    match fs::read(brain_fingerprint_key_path(journal_path)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            FingerprintKeyMaterial::Absent
        }
        Ok(bytes) => match <[u8; FINGERPRINT_KEY_BYTES]>::try_from(bytes) {
            Ok(key) => FingerprintKeyMaterial::Ready(key),
            Err(_) => FingerprintKeyMaterial::Unavailable,
        },
        Err(_) => FingerprintKeyMaterial::Unavailable,
    }
}

#[allow(dead_code)]
pub(crate) fn map_lease_probe(
    probe: solstone_core_journal_io::lease::LeaseProbe,
) -> std::io::Result<bool> {
    match probe {
        solstone_core_journal_io::lease::LeaseProbe::Active => Ok(true),
        solstone_core_journal_io::lease::LeaseProbe::Released => Ok(false),
        solstone_core_journal_io::lease::LeaseProbe::Indeterminate => {
            Err(std::io::Error::other("file lease state indeterminate"))
        }
    }
}

pub fn probe_file_lease_held(path: &Path) -> std::io::Result<bool> {
    #[cfg(not(unix))]
    {
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(false)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        map_lease_probe(solstone_core_journal_io::lease::probe_file_lease(&file))
    }
    #[cfg(unix)]
    {
        let file = match OpenOptions::new().read(true).write(true).open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(lock) => {
                drop(lock);
                Ok(false)
            }
            Err((_file, error))
                if error == Errno::EACCES
                    || error == Errno::EAGAIN
                    || error == Errno::EWOULDBLOCK =>
            {
                Ok(true)
            }
            Err((_file, error)) => Err(std::io::Error::from_raw_os_error(error as i32)),
        }
    }
}

pub fn inspect_brain_state(
    journal_path: &Path,
    config: &Map<String, Value>,
    now: Timestamp,
) -> BrainInspection {
    inspect_brain_state_with_clock(journal_path, config, || now)
}

pub fn inspect_brain_state_with_clock(
    journal_path: &Path,
    config: &Map<String, Value>,
    clock: impl FnOnce() -> Timestamp,
) -> BrainInspection {
    let resolution = derive_active_brain_lane(config);
    let credential;
    let endpoint;
    let target = match resolution.lane.as_deref() {
        Some("byo-cloud")
            if matches!(
                resolution.provider.as_str(),
                "openai" | "anthropic" | "google"
            ) =>
        {
            credential = managed_config_credential(config, &resolution.provider);
            credential
                .as_ref()
                .map(|credential| crate::CredentialHoldTarget::Managed {
                    provider: &resolution.provider,
                    credential,
                })
        }
        Some("byo-endpoint") if resolution.provider == "local" => {
            endpoint = endpoint_binding_parts(config);
            Some(crate::CredentialHoldTarget::Endpoint {
                base_url: &endpoint.0,
                credential: &endpoint.1,
            })
        }
        _ => None,
    };
    // Retain the original root and health directory before either read. An
    // independent later open must not lend its authority to an earlier Ready
    // record from a replaced directory.
    let inputs = target.as_ref().map(|_| {
        solstone_core_journal_io::JournalRoot::open(journal_path)
            .map_err(|error| crate::WriterError::Io(error.to_string()))
            .and_then(|root| {
                crate::writer::read_brain_inspection_inputs_bound(
                    std::sync::Arc::new(root),
                    journal_path,
                )
            })
    });
    let mut inspection =
        inspect_durable_brain_state_with_clock(journal_path, config, clock, inputs.as_ref());
    let Some(target) = target else {
        return inspection;
    };
    let binding = match inputs.as_ref().expect("applicable credential inputs") {
        Ok(inputs) => crate::writer::inspect_credential_binding_bound(
            inputs,
            target,
            validation_entry(config, &resolution.provider),
        ),
        Err(error) => Err(crate::WriterError::Io(error.to_string())),
    };
    match binding {
        Ok(StoredCredentialBinding::Matches | StoredCredentialBinding::Corrupt) => {
            inspection.projection.aggregate_state = aggregate_for_reason("provider_key_invalid");
            inspection.projection.reason_code = Some("provider_key_invalid".to_owned());
            inspection.projection.active_lane = resolution.lane;
            inspection.projection.active_provider = Some(resolution.provider);
            inspection.projection.active_model = resolution.model;
            inspection.projection.runtime_transition_in_progress = false;
        }
        Err(_) if inspection.projection.aggregate_state == "ready" => {
            inspection.projection.aggregate_state = "unknown".to_owned();
            inspection.projection.reason_code = Some("brain_record_unavailable".to_owned());
        }
        _ => {}
    }
    inspection
}

fn inspect_durable_brain_state_with_clock(
    journal_path: &Path,
    config: &Map<String, Value>,
    clock: impl FnOnce() -> Timestamp,
    bound: Option<&Result<crate::writer::BoundBrainInspection, crate::WriterError>>,
) -> BrainInspection {
    // Sample after the read: a concurrent renewal may be newer than the time
    // at which a caller started its health-check battery.
    let state_path = brain_state_path(journal_path);
    let bytes = match bound {
        Some(Ok(inputs)) => inputs.record.clone().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "Brain record is absent")
        }),
        Some(Err(error)) => Err(std::io::Error::other(error.to_string())),
        None => fs::read(&state_path),
    };
    let now = clock();
    let (record, raw, status, record_reason, error) = match bytes {
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && (bound.is_some_and(Result::is_ok)
                    || state_path
                        .parent()
                        .is_some_and(|parent| match fs::metadata(parent) {
                            Ok(metadata) => metadata.is_dir(),
                            Err(parent_error) => {
                                parent_error.kind() == std::io::ErrorKind::NotFound
                            }
                        })) =>
        {
            (
                None,
                None,
                InspectionStatus::Unavailable,
                Some("brain_record_missing"),
                None,
            )
        }
        Err(error) => (
            None,
            None,
            InspectionStatus::Unavailable,
            Some("brain_record_unavailable"),
            Some(error.to_string()),
        ),
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes)
            .map_err(|error| error.to_string())
            .and_then(|value| {
                if !value.is_object() {
                    return Err("record: brain state must be an object".to_owned());
                }
                validate_brain_state_record(&value, now)
                    .map(|record| (record, value))
                    .map_err(|error| error.to_string())
            }) {
            Ok((record, value)) => (Some(record), Some(value), InspectionStatus::Ok, None, None),
            // A record that does not validate is not carried: half a record is
            // the state this contract exists to make unrepresentable.
            Err(error) => (
                None,
                None,
                InspectionStatus::Corrupt,
                Some("brain_record_invalid"),
                Some(error),
            ),
        },
    };
    let existing_key = || match bound {
        Some(Ok(inputs)) => match inputs.material {
            FingerprintKeyMaterial::Ready(key) => Some(key),
            _ => None,
        },
        Some(Err(_)) => None,
        None => load_existing_fingerprint_key(journal_path),
    };
    let resolution = derive_active_brain_lane(config);
    if resolution.lane.is_none() {
        return inspection(
            status,
            project_brain_state(record.as_ref(), config, false, None, None, now),
            error,
            raw,
        );
    }
    let Some(record) = record else {
        let key = existing_key();
        return inspection(
            status,
            projection_without_record(
                config,
                &resolution,
                key.as_ref(),
                record_reason.expect("record outcome reason"),
            ),
            error,
            raw,
        );
    };
    if resolution.lane.as_deref() == Some("none") {
        return inspection(
            status,
            project_brain_state(Some(&record), config, false, None, None, now),
            error,
            raw,
        );
    }
    let key = match existing_key() {
        Some(key) => key,
        None => {
            if credential_overlay(config, &resolution, None) == CredentialOverlay::Rejected {
                return inspection(
                    status,
                    credential_rejection_projection(&record, &resolution),
                    error,
                    raw,
                );
            }
            return inspection(
                status,
                record_projection(&record, "unknown", Some("fingerprint_key_unavailable")),
                error,
                raw,
            );
        }
    };
    let runtime = (resolution.lane.as_deref() == Some("bundled"))
        .then(|| inspect_runtime_health(journal_path));
    let refresh_permit_active = if record.checking.is_some() {
        match probe_file_lease_held(&brain_refresh_lease_path(journal_path)) {
            Ok(held) => held,
            Err(error) => {
                return inspection(
                    status,
                    record_projection(&record, "unknown", Some("brain_check_interrupted")),
                    Some(error.to_string()),
                    raw,
                );
            }
        }
    } else {
        false
    };
    let mut projection = project_brain_state_with_journal(
        Some(&record),
        config,
        refresh_permit_active,
        Some(&key),
        runtime.as_ref(),
        now,
        Some(journal_path),
    );
    // The runtime record only notices a model download on its next pass. The
    // install record knows at once, so a local model the owner just chose
    // reads as being set up from the first second, not as a fault.
    // A check recorded before the owner chose local is out of date in the
    // same second; until the queued check lands, the install is the answer.
    if projection.active_lane.as_deref() == Some("bundled")
        && matches!(
            projection.reason_code.as_deref(),
            Some("local_artifact_not_ready" | "local_runtime_not_ready" | "brain_config_changed")
        )
        && crate::runtime_health::local_install_in_flight(journal_path)
    {
        if projection.reason_code.as_deref() == Some("brain_config_changed") {
            projection.aggregate_state = aggregate_for_reason("local_artifact_not_ready");
            projection.reason_code = Some("local_artifact_not_ready".to_owned());
        }
        projection.runtime_transition_in_progress = true;
    }
    inspection(InspectionStatus::Ok, projection, None, raw)
}

pub fn project_brain_state(
    record: Option<&BrainStateRecord>,
    config: &Map<String, Value>,
    refresh_permit_active: bool,
    hmac_key: Option<&[u8; 32]>,
    runtime_health: Option<&RuntimeRecordInspection>,
    now: Timestamp,
) -> BrainProjection {
    project_brain_state_with_journal(
        record,
        config,
        refresh_permit_active,
        hmac_key,
        runtime_health,
        now,
        None,
    )
}

pub fn project_brain_state_with_journal(
    record: Option<&BrainStateRecord>,
    config: &Map<String, Value>,
    refresh_permit_active: bool,
    hmac_key: Option<&[u8; 32]>,
    runtime_health: Option<&RuntimeRecordInspection>,
    now: Timestamp,
    journal: Option<&Path>,
) -> BrainProjection {
    let resolution = derive_active_brain_lane(config);
    if resolution.lane.is_none() {
        return BrainProjection {
            aggregate_state: "unknown".to_owned(),
            reason_code: Some("configuration_invalid".to_owned()),
            active_lane: None,
            active_provider: Some(resolution.provider),
            active_model: resolution.model,
            fingerprint_sha256: record.and_then(|record| record.fingerprint_sha256.clone()),
            runtime_transition_in_progress: false,
        };
    }
    let lane = resolution.lane.as_deref().expect("checked lane resolution");
    let Some(record) = record else {
        return projection_without_record(config, &resolution, hmac_key, "brain_record_missing");
    };
    let runtime_inputs = (lane == "bundled").then(|| bundled_runtime_inputs(runtime_health));
    let runtime_reason = runtime_inputs.as_ref().and_then(|inputs| inputs.0.clone());
    let (mut aggregate_state, mut reason_code) = reduce_evidence_with_runtime(
        record,
        now,
        refresh_permit_active,
        runtime_reason.as_deref(),
    );
    let fingerprint_sha256 = record.fingerprint_sha256.clone();
    if lane == "none" && record.active_lane == "none" {
        return record_projection(record, &aggregate_state, reason_code.as_deref());
    }
    let mut config_changed = false;
    let overlay = credential_overlay(config, &resolution, hmac_key);
    if hmac_key.is_none() {
        if overlay == CredentialOverlay::Rejected {
            return credential_rejection_projection(record, &resolution);
        }
        return BrainProjection {
            aggregate_state: "unknown".to_owned(),
            reason_code: Some("fingerprint_key_unavailable".to_owned()),
            active_lane: Some(record.active_lane.clone()),
            active_provider: record.active_provider.clone(),
            active_model: record.active_model.clone(),
            fingerprint_sha256,
            runtime_transition_in_progress: false,
        };
    }
    if overlay == CredentialOverlay::Rejected {
        return credential_rejection_projection(record, &resolution);
    }
    if record_timestamp_invalid(record, now) {
        return BrainProjection {
            aggregate_state: "unknown".to_owned(),
            reason_code: Some("brain_record_invalid".to_owned()),
            active_lane: Some(record.active_lane.clone()),
            active_provider: record.active_provider.clone(),
            active_model: record.active_model.clone(),
            fingerprint_sha256,
            runtime_transition_in_progress: false,
        };
    }
    if lane == "bundled" {
        let desired = runtime_inputs.and_then(|inputs| inputs.1);
        if desired.is_none() {
            return BrainProjection {
                aggregate_state,
                reason_code,
                active_lane: Some(record.active_lane.clone()),
                active_provider: record.active_provider.clone(),
                active_model: record.active_model.clone(),
                fingerprint_sha256,
                runtime_transition_in_progress: runtime_transition_in_progress(
                    lane,
                    runtime_health,
                ),
            };
        }
        if let (Some(key), Some(desired)) = (hmac_key, desired) {
            match build_active_brain_fingerprint(config, key, Some(Value::String(desired)), journal)
            {
                Ok(fingerprint) if fingerprint == record.fingerprint_sha256 => {}
                Ok(_) => {
                    aggregate_state = "unknown".to_owned();
                    reason_code = Some("brain_config_changed".to_owned());
                    config_changed = true;
                }
                Err(error) => {
                    aggregate_state = "unknown".to_owned();
                    reason_code = Some(error.0);
                }
            }
        }
    } else if let Some(key) = hmac_key {
        match build_active_brain_fingerprint(config, key, None, journal) {
            Ok(fingerprint) if fingerprint == record.fingerprint_sha256 => {}
            Ok(_) => {
                aggregate_state = "unknown".to_owned();
                reason_code = Some("brain_config_changed".to_owned());
                config_changed = true;
            }
            Err(error) => {
                aggregate_state = "unknown".to_owned();
                reason_code = Some(error.0);
            }
        }
    }
    let missing_key = overlay == CredentialOverlay::Missing
        && (aggregate_state == "ready"
            || reason_code.is_none()
            || reason_code.as_deref() == Some("brain_config_changed"));
    if missing_key {
        aggregate_state = aggregate_for_reason("provider_key_missing");
        reason_code = Some("provider_key_missing".to_owned());
        config_changed = true;
    }
    let runtime_transition = reason_code.as_deref() != Some("brain_config_changed")
        && reason_code.as_deref() != Some("provider_key_missing")
        && reason_code.as_deref() != Some("provider_key_invalid")
        && runtime_transition_in_progress(lane, runtime_health);
    let (active_lane, active_provider, active_model) = if config_changed {
        (
            resolution.lane.clone(),
            Some(resolution.provider.clone()),
            resolution.model.clone(),
        )
    } else {
        (
            Some(record.active_lane.clone()),
            record.active_provider.clone(),
            record.active_model.clone(),
        )
    };
    BrainProjection {
        aggregate_state,
        reason_code,
        active_lane,
        active_provider,
        active_model,
        fingerprint_sha256,
        runtime_transition_in_progress: runtime_transition,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialOverlay {
    None,
    Rejected,
    Missing,
}

fn credential_overlay(
    config: &Map<String, Value>,
    resolution: &LaneResolution,
    hmac_key: Option<&[u8; 32]>,
) -> CredentialOverlay {
    let lane = resolution.lane.as_deref();
    let provider = resolution.provider.as_str();
    if lane == Some("spp") || provider == "chatgpt" {
        return CredentialOverlay::None;
    }
    let managed =
        lane == Some("byo-cloud") && matches!(provider, "openai" | "anthropic" | "google");
    let byo_endpoint = lane == Some("byo-endpoint") && provider == "local";
    if !managed && !byo_endpoint {
        return CredentialOverlay::None;
    }
    let (expected, credential_present) = if managed {
        match managed_config_credential(config, provider) {
            Some(secret) => (
                hmac_key.map(|key| managed_credential_binding_hmac(key, provider, &secret)),
                true,
            ),
            None => (None, false),
        }
    } else {
        let (base_url, credential) = endpoint_binding_parts(config);
        (
            hmac_key.map(|key| endpoint_credential_binding_hmac(key, &base_url, &credential)),
            !credential.is_empty(),
        )
    };
    let binding = classify_stored_binding(
        validation_entry(config, provider),
        hmac_key,
        expected.as_deref(),
    );
    if matches!(
        binding,
        StoredCredentialBinding::Matches | StoredCredentialBinding::Corrupt
    ) {
        return CredentialOverlay::Rejected;
    }
    if managed && !credential_present {
        CredentialOverlay::Missing
    } else {
        CredentialOverlay::None
    }
}

fn projection_without_record(
    config: &Map<String, Value>,
    resolution: &LaneResolution,
    hmac_key: Option<&[u8; 32]>,
    record_reason: &str,
) -> BrainProjection {
    match credential_overlay(config, resolution, hmac_key) {
        CredentialOverlay::Rejected => BrainProjection {
            aggregate_state: aggregate_for_reason("provider_key_invalid"),
            reason_code: Some("provider_key_invalid".to_owned()),
            active_lane: resolution.lane.clone(),
            active_provider: Some(resolution.provider.clone()),
            active_model: resolution.model.clone(),
            fingerprint_sha256: None,
            runtime_transition_in_progress: false,
        },
        CredentialOverlay::Missing => BrainProjection {
            aggregate_state: aggregate_for_reason("provider_key_missing"),
            reason_code: Some("provider_key_missing".to_owned()),
            active_lane: resolution.lane.clone(),
            active_provider: Some(resolution.provider.clone()),
            active_model: resolution.model.clone(),
            fingerprint_sha256: None,
            runtime_transition_in_progress: false,
        },
        CredentialOverlay::None => record_outcome_projection(record_reason, resolution),
    }
}

fn credential_rejection_projection(
    record: &BrainStateRecord,
    resolution: &LaneResolution,
) -> BrainProjection {
    BrainProjection {
        aggregate_state: aggregate_for_reason("provider_key_invalid"),
        reason_code: Some("provider_key_invalid".to_owned()),
        active_lane: resolution.lane.clone(),
        active_provider: Some(resolution.provider.clone()),
        active_model: resolution.model.clone(),
        fingerprint_sha256: record.fingerprint_sha256.clone(),
        runtime_transition_in_progress: false,
    }
}

fn aggregate_for_reason(reason: &str) -> String {
    local_contract()
        .brain_state
        .reason_to_aggregate
        .get(reason)
        .cloned()
        .unwrap_or_else(|| "unknown".to_owned())
}

fn managed_config_credential(config: &Map<String, Value>, provider: &str) -> Option<String> {
    let env_name = local_contract()
        .brain_state
        .provider_env_by_name
        .get(provider)?;
    config
        .get("env")
        .and_then(Value::as_object)
        .and_then(|env| env.get(env_name))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn validation_entry<'a>(config: &'a Map<String, Value>, provider: &str) -> Option<&'a Value> {
    config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("key_validation"))
        .and_then(Value::as_object)
        .and_then(|entries| entries.get(provider))
}

fn record_outcome_projection(
    reason: &str,
    resolution: &crate::fingerprint::LaneResolution,
) -> BrainProjection {
    BrainProjection {
        aggregate_state: "unknown".to_owned(),
        reason_code: Some(reason.to_owned()),
        active_lane: resolution.lane.clone(),
        active_provider: Some(resolution.provider.clone()),
        active_model: resolution.model.clone(),
        fingerprint_sha256: None,
        runtime_transition_in_progress: false,
    }
}

fn record_projection(
    record: &BrainStateRecord,
    aggregate_state: &str,
    reason_code: Option<&str>,
) -> BrainProjection {
    BrainProjection {
        aggregate_state: aggregate_state.to_owned(),
        reason_code: reason_code.map(str::to_owned),
        active_lane: Some(record.active_lane.clone()),
        active_provider: record.active_provider.clone(),
        active_model: record.active_model.clone(),
        fingerprint_sha256: record.fingerprint_sha256.clone(),
        runtime_transition_in_progress: false,
    }
}

/// Read the bundled runtime health record for a prerequisite probe.
///
/// The result deliberately preserves the runtime's desired fingerprint so an
/// owner can compare it with the fingerprint it is about to record.  This is
/// the same decision table used by the brain projection.
pub fn assess_bundled_runtime_prerequisite(
    journal_path: &Path,
    expected_desired_fingerprint: Option<&str>,
) -> BundledRuntimePrerequisiteAssessment {
    let runtime = inspect_runtime_health(journal_path);
    let (mut reason_code, desired_fingerprint_sha256) = bundled_runtime_inputs(Some(&runtime));
    let record = runtime.record.as_ref().and_then(Value::as_object);
    let vocabulary = &local_contract().brain_state;
    let phase = record
        .and_then(|record| record.get("phase"))
        .and_then(Value::as_str)
        .filter(|phase| {
            vocabulary
                .runtime_phases
                .iter()
                .any(|candidate| candidate == phase)
        })
        .map(str::to_owned);
    let runtime_reason = record
        .and_then(|record| record.get("reason_code"))
        .and_then(Value::as_str)
        .filter(|reason| {
            vocabulary
                .runtime_reason_codes
                .iter()
                .any(|candidate| candidate == reason)
        })
        .map(str::to_owned);
    if reason_code.is_none()
        && expected_desired_fingerprint.is_some()
        && desired_fingerprint_sha256.as_deref() != expected_desired_fingerprint
    {
        reason_code = Some("local_runtime_fingerprint_mismatch".to_owned());
    }
    BundledRuntimePrerequisiteAssessment {
        reason_code,
        desired_fingerprint_sha256,
        phase,
        runtime_reason,
    }
}

fn bundled_runtime_inputs(
    runtime: Option<&RuntimeRecordInspection>,
) -> (Option<String>, Option<String>) {
    let Some(runtime) = runtime else {
        return (Some("local_runtime_state_unavailable".to_owned()), None);
    };
    if runtime.status == "corrupt" {
        return (Some("local_runtime_state_invalid".to_owned()), None);
    }
    if runtime.status != "ok" {
        return (Some("local_runtime_state_unavailable".to_owned()), None);
    }
    let Some(record) = runtime.record.as_ref().and_then(Value::as_object) else {
        return (Some("local_runtime_state_unavailable".to_owned()), None);
    };
    let Some(phase) = record.get("phase").and_then(Value::as_str) else {
        return (Some("local_runtime_state_invalid".to_owned()), None);
    };
    let vocabulary = &local_contract().brain_state;
    if !vocabulary
        .runtime_phases
        .iter()
        .any(|candidate| candidate == phase)
    {
        return (Some("local_runtime_state_invalid".to_owned()), None);
    }
    let runtime_reason = record.get("reason_code").and_then(Value::as_str);
    if let Some(reason) = runtime_reason
        && vocabulary
            .incoherent_runtime_phase_reason_codes
            .iter()
            .any(|pair| {
                pair.first().is_some_and(|candidate| candidate == phase)
                    && pair.get(1).is_some_and(|candidate| candidate == reason)
            })
    {
        return (Some("local_runtime_state_invalid".to_owned()), None);
    }
    let desired = record
        .get("desired_fingerprint_sha256")
        .and_then(Value::as_str)
        .filter(|fingerprint| {
            fingerprint.len() == 64 && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .map(str::to_owned);
    if let Some(reason) = runtime_reason
        && let Some(mapped) = vocabulary.runtime_reason_to_brain_reason.get(reason)
    {
        return (Some(mapped.clone()), desired);
    }
    if let Some(mapped) = vocabulary.runtime_phase_to_reason.get(phase)
        && let Some(mapped) = mapped.as_str()
    {
        return (Some(mapped.to_owned()), desired);
    }
    if phase == "ready" && desired.is_none() {
        return (Some("local_runtime_state_invalid".to_owned()), None);
    }
    (None, desired)
}

fn runtime_transition_in_progress(lane: &str, runtime: Option<&RuntimeRecordInspection>) -> bool {
    if lane != "bundled" || runtime.is_none_or(|runtime| runtime.status != "ok") {
        return false;
    }
    let Some(record) = runtime
        .and_then(|runtime| runtime.record.as_ref())
        .and_then(Value::as_object)
    else {
        return false;
    };
    let phase = record.get("phase").and_then(Value::as_str);
    let reason = record.get("reason_code").and_then(Value::as_str);
    phase.is_some_and(|phase| {
        local_contract()
            .brain_state
            .runtime_transition_phases
            .iter()
            .any(|candidate| candidate == phase)
    }) || reason == Some("install-in-progress")
}

fn record_timestamp_invalid(record: &BrainStateRecord, now: Timestamp) -> bool {
    record.updated_at > now
        || record
            .checking
            .as_ref()
            .is_some_and(|checking| checking.started_at > now)
        || record
            .runtime_failure_marker
            .as_ref()
            .is_some_and(|marker| marker.recorded_at > now)
        || record
            .evidence
            .values()
            .flatten()
            .any(|component| component.observed_at > now)
}

fn inspection(
    status: InspectionStatus,
    projection: BrainProjection,
    error: Option<String>,
    record: Option<Value>,
) -> BrainInspection {
    BrainInspection {
        status,
        projection,
        error,
        record,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    struct TempJournal(tempfile::TempDir);

    impl TempJournal {
        fn new() -> Self {
            Self(tempfile::tempdir().expect("create temporary journal"))
        }

        fn write_runtime(&self, value: Value) {
            let path = self.0.path().join("health/providers/runtime");
            fs::create_dir_all(&path).expect("create runtime directory");
            fs::write(path.join("local.json"), value.to_string()).expect("write runtime health");
        }
    }

    #[test]
    fn bundled_runtime_prerequisite_covers_missing_invalid_phases_and_fingerprint() {
        let journal = TempJournal::new();
        let missing = assess_bundled_runtime_prerequisite(journal.0.path(), None);
        assert_eq!(
            missing.reason_code.as_deref(),
            Some("local_runtime_not_ready")
        );
        assert_eq!(missing.phase.as_deref(), Some("stopped"));

        journal.write_runtime(json!({"phase":"not-a-real-phase"}));
        let invalid = assess_bundled_runtime_prerequisite(journal.0.path(), None);
        assert_eq!(
            invalid.reason_code.as_deref(),
            Some("local_runtime_state_invalid")
        );
        assert_eq!(invalid.phase, None);

        for phase in &local_contract().brain_state.runtime_phases {
            journal.write_runtime(json!({
                "phase": phase,
                "desired_fingerprint_sha256": "a".repeat(64),
            }));
            let assessment = assess_bundled_runtime_prerequisite(journal.0.path(), None);
            let expected = local_contract()
                .brain_state
                .runtime_phase_to_reason
                .get(phase)
                .and_then(Value::as_str);
            assert_eq!(assessment.reason_code.as_deref(), expected, "{phase}");
            assert_eq!(assessment.phase.as_deref(), Some(phase.as_str()), "{phase}");
        }

        journal.write_runtime(json!({
            "phase": "ready",
            "desired_fingerprint_sha256": "a".repeat(64),
        }));
        let matched = assess_bundled_runtime_prerequisite(journal.0.path(), Some(&"a".repeat(64)));
        assert_eq!(matched.reason_code, None);
        let mismatched =
            assess_bundled_runtime_prerequisite(journal.0.path(), Some(&"b".repeat(64)));
        assert_eq!(
            mismatched.reason_code.as_deref(),
            Some("local_runtime_fingerprint_mismatch")
        );
    }

    fn unified(backend: &str, model_id: &str) -> String {
        crate::fingerprint::bundled_runtime_desired_fingerprint(
            backend,
            model_id,
            &"c".repeat(64),
            Some("/var/tmp/journal/cache/providers/local/bin/x86_64-unknown-linux-gnu/b10068/llama-server"),
            "/var/tmp/journal/cache/providers/local/models/local__qwen3.5-4b/Qwen3.5-4B-Q4_K_M.gguf",
            Some("/var/tmp/journal/cache/providers/local/models/local__qwen3.5-4b/mmproj-F16.gguf"),
        )
        .unwrap()
        .sha256
    }

    fn project(write_hash: &str, desired: &str) -> BrainProjection {
        let fixture = crate::fixture::projection_fixture();
        let now = solstone_core_time::parse_rfc3339(&fixture.now)
            .unwrap()
            .timestamp();
        let key: [u8; 32] = (0..fixture.hmac_key_hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&fixture.hmac_key_hex[i..i + 2], 16).unwrap())
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let config = fixture.configs["lane_bundled"].as_object().unwrap();
        let brain = build_active_brain_fingerprint(
            config,
            &key,
            Some(Value::String(write_hash.to_owned())),
            None,
        )
        .unwrap()
        .unwrap();
        let mut raw = fixture.records["lane_bundled/ready"].clone();
        raw["fingerprint_sha256"] = json!(brain);
        let mut runtime = fixture.runtime_health["ok_ready_fingerprint_b"].clone();
        runtime["record"]["desired_fingerprint_sha256"] = json!(desired);
        project_brain_state(
            Some(&validate_brain_state_record(&raw, now).unwrap()),
            config,
            false,
            Some(&key),
            Some(&crate::runtime_health::inspection_from_fixture(&runtime)),
            now,
        )
    }

    #[test]
    fn unified_formula_matches_projection_and_historical_six_field_is_unknown() {
        let desired = unified("vulkan", "local/qwen3.5-4b");
        let matched = project(&desired, &desired);
        assert_ne!(matched.aggregate_state, "unknown");
        assert_ne!(matched.reason_code.as_deref(), Some("brain_config_changed"));
        let six = crate::fingerprint::canonical_fingerprint(
            &crate::fingerprint::CanonicalInput::Json(json!({
                "provider":"local","runtime":"llama.cpp","backend":"vulkan",
                "backend_reason":"no NVIDIA GPU detected",
                "runtime_pin":{"unit":"llama-server-vulkan","artifact_key":"x86_64-unknown-linux-gnu","release_tag":"b10068","filename":"llama-b10068-bin-ubuntu-vulkan-x64.tar.gz","sha256":"713641920dce6c8efb953ebc9ffa309977e200cec5e182e6ad0e8b086203cdc3","binary_name":"llama-server"},
                "model_pin":{"unit":"local-model","model_id":"local/qwen3.5-4b"}
            })),
        )
        .unwrap();
        let stale = project(&six, &desired);
        assert_eq!(stale.aggregate_state, "unknown");
        assert_eq!(stale.reason_code.as_deref(), Some("brain_config_changed"));
        let journal = TempJournal::new();
        journal.write_runtime(json!({"phase":"ready","desired_fingerprint_sha256":desired}));
        assert_eq!(
            assess_bundled_runtime_prerequisite(journal.0.path(), Some(&desired)).reason_code,
            None
        );
    }

    #[test]
    fn unified_formula_still_detects_real_input_drift() {
        let vulkan = unified("vulkan", "local/qwen3.5-4b");
        let cuda = unified("cuda", "local/qwen3.5-4b");
        let drifted = project(&vulkan, &cuda);
        assert_eq!(drifted.aggregate_state, "unknown");
        assert_eq!(drifted.reason_code.as_deref(), Some("brain_config_changed"));
        let journal = TempJournal::new();
        journal.write_runtime(json!({"phase":"ready","desired_fingerprint_sha256":cuda}));
        assert_eq!(
            assess_bundled_runtime_prerequisite(journal.0.path(), Some(&vulkan))
                .reason_code
                .as_deref(),
            Some("local_runtime_fingerprint_mismatch")
        );
    }

    fn fixture_now() -> Timestamp {
        solstone_core_time::parse_rfc3339(&crate::fixture::projection_fixture().now)
            .expect("fixture now")
            .timestamp()
    }

    fn fixture_key() -> [u8; 32] {
        let hex = &crate::fixture::projection_fixture().hmac_key_hex;
        (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect::<Vec<_>>()
            .try_into()
            .unwrap()
    }

    fn config_named(name: &str) -> Map<String, Value> {
        crate::fixture::projection_fixture().configs[name]
            .as_object()
            .expect("fixture config")
            .clone()
    }

    fn record_named(name: &str) -> BrainStateRecord {
        validate_brain_state_record(
            &crate::fixture::projection_fixture().records[name],
            fixture_now(),
        )
        .expect("fixture record")
    }

    fn project_at(
        record: &str,
        config: &Map<String, Value>,
        key: Option<&[u8; 32]>,
    ) -> BrainProjection {
        project_brain_state_with_journal(
            Some(&record_named(record)),
            config,
            false,
            key,
            None,
            fixture_now(),
            None,
        )
    }

    fn plant_rejection(config: &mut Map<String, Value>, provider: &str, entry: Value) {
        let providers = config
            .entry("providers".to_owned())
            .or_insert_with(|| json!({}));
        let providers = providers.as_object_mut().expect("providers");
        let validations = providers
            .entry("key_validation".to_owned())
            .or_insert_with(|| json!({}));
        validations
            .as_object_mut()
            .expect("key_validation")
            .insert(provider.to_owned(), entry);
    }

    fn matching_entry(provider: &str, credential: &str) -> Value {
        let hmac = managed_credential_binding_hmac(&fixture_key(), provider, credential);
        let mut entry = Map::new();
        entry.insert("valid".to_owned(), Value::Bool(false));
        entry.insert(
            "reason_code".to_owned(),
            Value::String("provider_key_invalid".to_owned()),
        );
        crate::attach_credential_binding(&mut entry, &hmac);
        Value::Object(entry)
    }

    #[test]
    fn matching_rejection_overlays_model_only_fingerprint_change() {
        let key = fixture_key();
        let mut changed = config_named("lane_byo_cloud");
        changed
            .get_mut("providers")
            .and_then(Value::as_object_mut)
            .and_then(|providers| providers.get_mut("active"))
            .and_then(Value::as_object_mut)
            .expect("active")
            .insert("model".to_owned(), json!("claude-other"));
        let drifted = project_at("lane_byo_cloud/ready", &changed, Some(&key));
        assert_eq!(drifted.reason_code.as_deref(), Some("brain_config_changed"));
        plant_rejection(
            &mut changed,
            "anthropic",
            matching_entry("anthropic", "sk-test"),
        );
        let overlaid = project_at("lane_byo_cloud/ready", &changed, Some(&key));
        assert_eq!(
            overlaid.reason_code.as_deref(),
            Some("provider_key_invalid")
        );
        assert_eq!(overlaid.aggregate_state, "unhealthy");
        assert_eq!(
            overlaid.fingerprint_sha256.as_deref(),
            record_named("lane_byo_cloud/ready")
                .fingerprint_sha256
                .as_deref()
        );

        let mut corrupt = config_named("lane_byo_cloud");
        plant_rejection(
            &mut corrupt,
            "anthropic",
            json!({"valid":false,"reason_code":"provider_key_invalid","binding_version":1}),
        );
        let missing_key = project_at("lane_byo_cloud/ready", &corrupt, None);
        assert_eq!(
            missing_key.reason_code.as_deref(),
            Some("provider_key_invalid")
        );
        assert_eq!(missing_key.aggregate_state, "unhealthy");

        let journal = TempJournal::new();
        let path = brain_state_path(journal.0.path());
        fs::create_dir_all(path.parent().expect("health")).expect("health directory");
        fs::write(
            path,
            serde_json::to_vec(
                &crate::fixture::projection_fixture().records["lane_byo_cloud/ready"],
            )
            .expect("record bytes"),
        )
        .expect("record writes");
        let inspection = inspect_brain_state(journal.0.path(), &corrupt, fixture_now());
        assert_eq!(
            inspection.projection.reason_code.as_deref(),
            Some("provider_key_invalid")
        );
        assert_eq!(inspection.projection.aggregate_state, "unhealthy");
    }

    #[test]
    fn validation_unavailable_does_not_clear_matching_rejection() {
        let key = fixture_key();
        let mut config = config_named("lane_byo_cloud");
        plant_rejection(
            &mut config,
            "anthropic",
            matching_entry("anthropic", "sk-test"),
        );
        let rejected = project_at("lane_byo_cloud/ready", &config, Some(&key));
        assert_eq!(
            rejected.reason_code.as_deref(),
            Some("provider_key_invalid")
        );
        assert_eq!(rejected.aggregate_state, "unhealthy");

        let hmac = managed_credential_binding_hmac(&key, "anthropic", "sk-test");
        let mut unavailable = Map::new();
        unavailable.insert("valid".to_owned(), Value::Bool(false));
        unavailable.insert(
            "reason_code".to_owned(),
            Value::String("validation_unavailable".to_owned()),
        );
        crate::attach_credential_binding(&mut unavailable, &hmac);
        plant_rejection(&mut config, "anthropic", Value::Object(unavailable));
        let left = project_at("lane_byo_cloud/ready", &config, Some(&key));
        assert_ne!(left.reason_code.as_deref(), Some("provider_key_invalid"));
        assert_eq!(left.aggregate_state, "ready");
    }

    #[test]
    fn missing_config_key_overlays_provider_key_missing() {
        let key = fixture_key();
        let mut config = config_named("lane_byo_cloud");
        config
            .get_mut("env")
            .and_then(Value::as_object_mut)
            .expect("env")
            .remove("ANTHROPIC_API_KEY");
        let missing = project_at("lane_byo_cloud/ready", &config, Some(&key));
        assert_eq!(missing.reason_code.as_deref(), Some("provider_key_missing"));
        assert_eq!(missing.aggregate_state, "blocked");

        config
            .get_mut("providers")
            .and_then(Value::as_object_mut)
            .and_then(|providers| providers.get_mut("active"))
            .and_then(Value::as_object_mut)
            .expect("active")
            .insert("model".to_owned(), json!("claude-other"));
        let drifted = project_at("lane_byo_cloud/ready", &config, Some(&key));
        assert_eq!(drifted.reason_code.as_deref(), Some("provider_key_missing"));
        assert_eq!(drifted.aggregate_state, "blocked");

        let unavailable = project_at("lane_byo_cloud/ready", &config, None);
        assert_eq!(
            unavailable.reason_code.as_deref(),
            Some("fingerprint_key_unavailable")
        );
    }

    #[test]
    fn legacy_unbound_does_not_overlay() {
        let key = fixture_key();
        let plain = config_named("lane_byo_cloud");
        let mut unbound = plain.clone();
        plant_rejection(
            &mut unbound,
            "anthropic",
            json!({"valid":false,"reason_code":"provider_key_invalid","error":"old","timestamp":"2026-08-06T12:00:00Z"}),
        );
        assert_eq!(
            project_at("lane_byo_cloud/ready", &plain, Some(&key)),
            project_at("lane_byo_cloud/ready", &unbound, Some(&key))
        );
        assert_ne!(
            project_at("lane_byo_cloud/ready", &unbound, Some(&key))
                .reason_code
                .as_deref(),
            Some("provider_key_invalid")
        );
    }

    #[test]
    fn spp_and_chatgpt_records_stay_unchanged() {
        let key = fixture_key();
        let spp = config_named("lane_spp");
        let mut spp_rejected = spp.clone();
        plant_rejection(
            &mut spp_rejected,
            "local",
            matching_entry("local", "endpoint-credential"),
        );
        let spp_plain = project_at("lane_spp/ready", &spp, Some(&key));
        let spp_planted = project_at("lane_spp/ready", &spp_rejected, Some(&key));
        assert_eq!(spp_plain, spp_planted);
        assert_ne!(
            spp_plain.reason_code.as_deref(),
            Some("provider_key_invalid")
        );

        let mut chatgpt = config_named("lane_byo_cloud");
        chatgpt
            .get_mut("providers")
            .and_then(Value::as_object_mut)
            .and_then(|providers| providers.get_mut("active"))
            .and_then(Value::as_object_mut)
            .expect("active")
            .insert("provider".to_owned(), json!("chatgpt"));
        let mut chatgpt_rejected = chatgpt.clone();
        plant_rejection(
            &mut chatgpt_rejected,
            "chatgpt",
            matching_entry("chatgpt", "sk-test"),
        );
        let chatgpt_plain = project_at("lane_byo_cloud/ready", &chatgpt, Some(&key));
        let chatgpt_planted = project_at("lane_byo_cloud/ready", &chatgpt_rejected, Some(&key));
        assert_eq!(chatgpt_plain, chatgpt_planted);
        assert_ne!(
            chatgpt_plain.reason_code.as_deref(),
            Some("provider_key_invalid")
        );
    }

    fn assert_record_free_overlay(record_bytes: Option<&[u8]>) {
        let journal = TempJournal::new();
        if let Some(bytes) = record_bytes {
            let path = brain_state_path(journal.0.path());
            fs::create_dir_all(path.parent().expect("health")).expect("health");
            fs::write(path, bytes).expect("record");
        }
        let key = fixture_key();
        fs::create_dir_all(
            brain_fingerprint_key_path(journal.0.path())
                .parent()
                .expect("health"),
        )
        .expect("health");
        fs::write(brain_fingerprint_key_path(journal.0.path()), key).expect("key");
        let mut rejected = config_named("lane_byo_cloud");
        plant_rejection(
            &mut rejected,
            "anthropic",
            matching_entry("anthropic", "sk-test"),
        );
        let inspection = inspect_brain_state(journal.0.path(), &rejected, fixture_now());
        assert_eq!(
            inspection.projection.reason_code.as_deref(),
            Some("provider_key_invalid")
        );
        assert_eq!(inspection.projection.aggregate_state, "unhealthy");
        assert!(inspection.projection.fingerprint_sha256.is_none());
        assert_eq!(
            inspection.projection.active_provider.as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            inspection.projection.active_lane.as_deref(),
            Some("byo-cloud")
        );

        let mut missing = config_named("lane_byo_cloud");
        missing
            .get_mut("env")
            .and_then(Value::as_object_mut)
            .expect("env")
            .remove("ANTHROPIC_API_KEY");
        let inspection = inspect_brain_state(journal.0.path(), &missing, fixture_now());
        assert_eq!(
            inspection.projection.reason_code.as_deref(),
            Some("provider_key_missing")
        );
        assert_eq!(inspection.projection.aggregate_state, "blocked");

        let spp = inspect_brain_state(journal.0.path(), &config_named("lane_spp"), fixture_now());
        let expected = if record_bytes.is_some() {
            "brain_record_invalid"
        } else {
            "brain_record_missing"
        };
        assert_eq!(spp.projection.reason_code.as_deref(), Some(expected));
        assert_ne!(spp.projection.aggregate_state, "ready");

        let mut chatgpt = config_named("lane_byo_cloud");
        chatgpt
            .get_mut("providers")
            .and_then(Value::as_object_mut)
            .and_then(|providers| providers.get_mut("active"))
            .and_then(Value::as_object_mut)
            .expect("active")
            .insert("provider".to_owned(), json!("chatgpt"));
        let chatgpt = inspect_brain_state(journal.0.path(), &chatgpt, fixture_now());
        assert_eq!(chatgpt.projection.reason_code.as_deref(), Some(expected));
        assert_ne!(chatgpt.projection.aggregate_state, "ready");
    }

    #[test]
    fn missing_brain_record_overlays_bound_rejection_and_missing_key() {
        assert_record_free_overlay(None);
    }

    #[test]
    fn corrupt_brain_record_overlays_bound_rejection_and_missing_key() {
        assert_record_free_overlay(Some(b"{"));
    }
}
