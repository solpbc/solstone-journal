// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::fcntl::{Flock, FlockArg};
use serde_json::{Map, Value};

use crate::fingerprint::{build_active_brain_fingerprint, derive_active_brain_lane};
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

pub fn probe_file_lease_held(path: &Path) -> std::io::Result<bool> {
    #[cfg(not(unix))]
    {
        let _ = path;
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "file lease probing is unavailable on this platform",
        ));
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
    now: DateTime<Utc>,
) -> BrainInspection {
    inspect_brain_state_with_clock(journal_path, config, || now)
}

pub fn inspect_brain_state_with_clock(
    journal_path: &Path,
    config: &Map<String, Value>,
    clock: impl FnOnce() -> DateTime<Utc>,
) -> BrainInspection {
    // Sample after the read: a concurrent renewal may be newer than the time
    // at which a caller started its health-check battery.
    let bytes = fs::read(brain_state_path(journal_path));
    let now = clock();
    let (record, raw, status, record_reason, error) = match bytes {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            None,
            None,
            InspectionStatus::Unavailable,
            Some("brain_record_missing"),
            None,
        ),
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
        return inspection(
            status,
            record_outcome_projection(record_reason.expect("record outcome reason"), &resolution),
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
    let key = match load_existing_fingerprint_key(journal_path) {
        Some(key) => key,
        None => {
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
    inspection(
        InspectionStatus::Ok,
        project_brain_state(
            Some(&record),
            config,
            refresh_permit_active,
            Some(&key),
            runtime.as_ref(),
            now,
        ),
        None,
        raw,
    )
}

pub fn project_brain_state(
    record: Option<&BrainStateRecord>,
    config: &Map<String, Value>,
    refresh_permit_active: bool,
    hmac_key: Option<&[u8; 32]>,
    runtime_health: Option<&RuntimeRecordInspection>,
    now: DateTime<Utc>,
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
        return record_outcome_projection("brain_record_missing", &resolution);
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
    if hmac_key.is_none() {
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
            match build_active_brain_fingerprint(config, key, Some(Value::String(desired))) {
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
        match build_active_brain_fingerprint(config, key, None) {
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
    let runtime_transition = reason_code.as_deref() != Some("brain_config_changed")
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

fn record_timestamp_invalid(record: &BrainStateRecord, now: DateTime<Utc>) -> bool {
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;

    struct TempJournal(PathBuf);

    impl TempJournal {
        fn new() -> Self {
            let path = PathBuf::from("/var/tmp").join(format!(
                "solstone-brain-inspect-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos(),
            ));
            fs::create_dir_all(&path).expect("create temporary journal");
            Self(path)
        }

        fn write_runtime(&self, value: Value) {
            let path = self.0.join("health/providers/runtime");
            fs::create_dir_all(&path).expect("create runtime directory");
            fs::write(path.join("local.json"), value.to_string()).expect("write runtime health");
        }
    }

    impl Drop for TempJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn bundled_runtime_prerequisite_covers_missing_invalid_phases_and_fingerprint() {
        let journal = TempJournal::new();
        let missing = assess_bundled_runtime_prerequisite(&journal.0, None);
        assert_eq!(
            missing.reason_code.as_deref(),
            Some("local_runtime_not_ready")
        );
        assert_eq!(missing.phase.as_deref(), Some("stopped"));

        journal.write_runtime(json!({"phase":"not-a-real-phase"}));
        let invalid = assess_bundled_runtime_prerequisite(&journal.0, None);
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
            let assessment = assess_bundled_runtime_prerequisite(&journal.0, None);
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
        let matched = assess_bundled_runtime_prerequisite(&journal.0, Some(&"a".repeat(64)));
        assert_eq!(matched.reason_code, None);
        let mismatched = assess_bundled_runtime_prerequisite(&journal.0, Some(&"b".repeat(64)));
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
        let now = DateTime::parse_from_rfc3339(&fixture.now)
            .unwrap()
            .with_timezone(&Utc);
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
            assess_bundled_runtime_prerequisite(&journal.0, Some(&desired)).reason_code,
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
            assess_bundled_runtime_prerequisite(&journal.0, Some(&vulkan))
                .reason_code
                .as_deref(),
            Some("local_runtime_fingerprint_mismatch")
        );
    }
}
