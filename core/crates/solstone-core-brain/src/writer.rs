// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The single native write authority for `health/brain.json`.
//!
//! Records are deliberately composed as JSON values.  The read model parses
//! timestamps into `DateTime<Utc>`, but write paths must preserve caller and
//! carried-forward timestamp spellings byte-for-byte (`Z` versus `+00:00`).

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use getrandom::fill as fill_random;
use serde_json::{Map, Value, json};
use solstone_core_journal_io::{
    AtomicWriteOptions, FileLease, JsonWriteOptions, LeaseOptions, LockOptions, acquire_file_lease,
    atomic_replace, hold_lock, write_json,
};

use crate::fingerprint::{
    LaneResolution, build_active_brain_fingerprint, derive_active_brain_lane,
};
use crate::fixture::local_contract;
use crate::inspect::{
    FINGERPRINT_KEY_BYTES, brain_fingerprint_key_path, brain_refresh_lease_path, brain_state_path,
    load_existing_fingerprint_key,
};
use crate::record::{
    BrainStateRecord, ValidationError, component_status_for_reason, parse_component,
    parse_evidence, parse_runtime_failure_marker, reduce_evidence_with_runtime,
    validate_brain_state_record,
};

const BRAIN_FILE_MODE: u32 = 0o600;

/// Terminal writer-parity cases from the frozen projection corpus.
///
/// Intermediate checking, absent/foreign fingerprint, marker, none-lane, and
/// configuration-invalid records are composition-only. The remaining 28 cases
/// are owned by `finish_refresh` and retained as a reviewable selector for AC13.
pub const REACHABLE_WRITE_CASES: [&str; 28] = [
    "lane_bundled/cogitate_failed_generate_ok",
    "lane_bundled/evidence_expired",
    "lane_bundled/generate_failed",
    "lane_bundled/prerequisites_blocked",
    "lane_bundled/ready",
    "lane_bundled/ready_expiring_within_the_hour",
    "lane_bundled/updated_at_in_the_future",
    "lane_byo_cloud/cogitate_failed_generate_ok",
    "lane_byo_cloud/evidence_expired",
    "lane_byo_cloud/generate_failed",
    "lane_byo_cloud/prerequisites_blocked",
    "lane_byo_cloud/ready",
    "lane_byo_cloud/ready_expiring_within_the_hour",
    "lane_byo_cloud/updated_at_in_the_future",
    "lane_byo_endpoint/cogitate_failed_generate_ok",
    "lane_byo_endpoint/evidence_expired",
    "lane_byo_endpoint/generate_failed",
    "lane_byo_endpoint/prerequisites_blocked",
    "lane_byo_endpoint/ready",
    "lane_byo_endpoint/ready_expiring_within_the_hour",
    "lane_byo_endpoint/updated_at_in_the_future",
    "lane_spp/cogitate_failed_generate_ok",
    "lane_spp/evidence_expired",
    "lane_spp/generate_failed",
    "lane_spp/prerequisites_blocked",
    "lane_spp/ready",
    "lane_spp/ready_expiring_within_the_hour",
    "lane_spp/updated_at_in_the_future",
];

/// A refresh/renewal permit whose lease remains held until it is consumed.
#[derive(Debug)]
pub struct BrainRefreshPermit {
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub fingerprint_sha256: String,
    pub checking_revision: u64,
    pub runtime_failure_marker_seen: Option<String>,
    _lease: FileLease,
}

/// A stale expected active-fingerprint assertion or another begin failure.
#[derive(Debug)]
pub enum BeginRefreshError {
    ExpectedFingerprintStale(String),
    InvalidArgument(String),
    Writer(WriterError),
}

impl fmt::Display for BeginRefreshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExpectedFingerprintStale(message) | Self::InvalidArgument(message) => {
                formatter.write_str(message)
            }
            Self::Writer(error) => error.fmt(formatter),
        }
    }
}

impl Error for BeginRefreshError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Writer(error) => Some(error),
            Self::ExpectedFingerprintStale(_) | Self::InvalidArgument(_) => None,
        }
    }
}

/// The deliberately distinct prerequisite-renewal begin result.
#[derive(Debug)]
pub enum BeginPrerequisiteRenewal {
    Started(BrainRefreshPermit),
    Busy { reason: String },
    Unsafe { reason: String },
}

/// A runtime-failure attempt, including the source-compatible rejection name.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeFailureResult {
    pub accepted: bool,
    pub record: Option<Value>,
    pub rejected_reason: Option<String>,
    pub error: Option<String>,
}

/// Writer failure outside runtime-failure's returned rejection taxonomy.
#[derive(Debug)]
pub enum WriterError {
    Io(String),
    Config(String),
    Validation(ValidationError),
    Conflict(String),
    Fingerprint(String),
    Random,
}

impl fmt::Display for WriterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message)
            | Self::Config(message)
            | Self::Conflict(message)
            | Self::Fingerprint(message) => formatter.write_str(message),
            Self::Validation(error) => error.fmt(formatter),
            Self::Random => formatter.write_str("could not generate secure random bytes"),
        }
    }
}

impl Error for WriterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Validation(error) => Some(error),
            Self::Io(_)
            | Self::Config(_)
            | Self::Conflict(_)
            | Self::Fingerprint(_)
            | Self::Random => None,
        }
    }
}

impl From<ValidationError> for WriterError {
    fn from(error: ValidationError) -> Self {
        Self::Validation(error)
    }
}

/// Load or generate the protected 32-byte fingerprint key.
pub fn generate_fingerprint_key(
    journal_path: &Path,
) -> Result<[u8; FINGERPRINT_KEY_BYTES], WriterError> {
    let path = brain_fingerprint_key_path(journal_path);
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(BRAIN_FILE_MODE),
            ..LockOptions::default()
        },
    )
    .map_err(lock_error)?;
    let key = match load_existing_fingerprint_key(journal_path) {
        Some(key) => key,
        None => secure_key()?,
    };
    atomic_replace(
        &path,
        &key,
        AtomicWriteOptions {
            mode: Some(BRAIN_FILE_MODE),
        },
    )
    .map_err(atomic_error)?;
    Ok(key)
}

/// Begin a fenced refresh.  Lease contention deliberately returns no permit.
pub fn begin_refresh(
    journal_path: &Path,
    now: DateTime<Utc>,
    run_id: Option<String>,
    expected_active_fingerprint_sha256: Option<&str>,
    expect_active_fingerprint_absent: bool,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> Result<Option<BrainRefreshPermit>, BeginRefreshError> {
    validate_expected(
        expected_active_fingerprint_sha256,
        expect_active_fingerprint_absent,
    )?;
    let expected_contract =
        expected_active_fingerprint_sha256.is_some() || expect_active_fingerprint_absent;
    let config = match config(journal_path) {
        Ok(config) => config,
        Err(_) => return Ok(None),
    };
    let initial = derive_active_brain_lane(&config);
    if initial.lane.is_none() && !expected_contract {
        return Ok(None);
    }
    if initial.lane.as_deref() == Some("none") && !expected_contract {
        begin_none_lane(journal_path, now, &initial).map_err(BeginRefreshError::Writer)?;
        return Ok(None);
    }

    let Some(lease) = acquire_file_lease(
        brain_refresh_lease_path(journal_path),
        LeaseOptions::default(),
    )
    .map_err(|error| BeginRefreshError::Writer(lease_error(error)))?
    else {
        return Ok(None);
    };
    begin_refresh_under_lease(
        journal_path,
        now,
        run_id,
        expected_active_fingerprint_sha256,
        expect_active_fingerprint_absent,
        bundled_runtime_fingerprint_sha256,
        lease,
    )
}

/// Begin the narrow SPP prerequisite-renewal lane.
pub fn begin_prerequisite_renewal(
    journal_path: &Path,
    now: DateTime<Utc>,
    run_id: Option<String>,
    expected_fingerprint_sha256: Option<&str>,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> BeginPrerequisiteRenewal {
    if expected_fingerprint_sha256.is_some_and(|value| !is_sha256(value)) {
        return BeginPrerequisiteRenewal::Unsafe {
            reason: "fingerprint_mismatch".to_owned(),
        };
    }
    let lease = match acquire_file_lease(
        brain_refresh_lease_path(journal_path),
        LeaseOptions::default(),
    ) {
        Ok(Some(lease)) => lease,
        Ok(None) => {
            return BeginPrerequisiteRenewal::Busy {
                reason: "lease_held".to_owned(),
            };
        }
        Err(error) => {
            return BeginPrerequisiteRenewal::Unsafe {
                reason: error.to_string(),
            };
        }
    };
    let loaded = match fingerprint_for_write(journal_path, bundled_runtime_fingerprint_sha256) {
        Ok(Some(loaded)) => loaded,
        Ok(None) => {
            return BeginPrerequisiteRenewal::Unsafe {
                reason: "fingerprint_not_available".to_owned(),
            };
        }
        Err(error) => {
            return BeginPrerequisiteRenewal::Unsafe {
                reason: error.to_string(),
            };
        }
    };
    if loaded.resolution.lane.as_deref() != Some("spp") {
        return BeginPrerequisiteRenewal::Unsafe {
            reason: "non_spp_lane".to_owned(),
        };
    }
    if expected_fingerprint_sha256.is_some_and(|expected| expected != loaded.sha256) {
        return BeginPrerequisiteRenewal::Unsafe {
            reason: "fingerprint_mismatch".to_owned(),
        };
    }
    let run_id = match run_id {
        Some(run_id) => run_id,
        None => match random_id() {
            Ok(run_id) => run_id,
            Err(error) => {
                return BeginPrerequisiteRenewal::Unsafe {
                    reason: error.to_string(),
                };
            }
        },
    };
    let expires_at = now + checking_ttl();
    let path = brain_state_path(journal_path);
    let result: Result<BeginPrerequisiteRenewal, WriterError> = (|| {
        let _lock = hold_record_lock(&path)?;
        let Some((raw, current)) = read_current(&path, now)? else {
            return Ok(BeginPrerequisiteRenewal::Unsafe {
                reason: "brain_record_missing".to_owned(),
            });
        };
        if current.fingerprint_sha256.as_deref() != Some(&loaded.sha256) {
            return Ok(BeginPrerequisiteRenewal::Unsafe {
                reason: "brain_record_missing".to_owned(),
            });
        }
        let Some(evidence) = safe_prerequisite_evidence(&raw, &current, now) else {
            return Ok(BeginPrerequisiteRenewal::Unsafe {
                reason: "unsafe_evidence".to_owned(),
            });
        };
        let revision = next_revision(Some(&current));
        let marker_seen = marker_id(Some(&raw));
        let record = compose_checking_record(CheckingRecord {
            revision,
            now,
            expires_at,
            run_id: &run_id,
            loaded: &loaded,
            evidence,
            marker: raw
                .get("runtime_failure_marker")
                .cloned()
                .unwrap_or(Value::Null),
            marker_seen: marker_seen.clone(),
        });
        write_record(&path, &record, now)?;
        Ok(BeginPrerequisiteRenewal::Started(BrainRefreshPermit {
            run_id,
            started_at: now,
            expires_at,
            fingerprint_sha256: loaded.sha256.clone(),
            checking_revision: revision,
            runtime_failure_marker_seen: marker_seen,
            _lease: lease,
        }))
    })();
    result.unwrap_or_else(|error| BeginPrerequisiteRenewal::Unsafe {
        reason: error.to_string(),
    })
}

/// Finish a refresh from the caller's four-component probe outcome.
pub fn finish_refresh(
    journal_path: &Path,
    permit: BrainRefreshPermit,
    outcome: Value,
    now: DateTime<Utc>,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> Result<Value, WriterError> {
    parse_evidence(Some(&outcome), now)?;
    let loaded = fingerprint_for_write(journal_path, bundled_runtime_fingerprint_sha256)?
        .ok_or_else(|| WriterError::Conflict("brain fingerprint key is unavailable".to_owned()))?;
    let path = brain_state_path(journal_path);
    let _lock = hold_record_lock(&path)?;
    let Some((_, current)) = read_current(&path, now)? else {
        return Err(WriterError::Conflict(
            "brain refresh checking marker is absent".to_owned(),
        ));
    };
    assert_finish_allowed(&permit, &current, now)?;
    if loaded.sha256 != permit.fingerprint_sha256 {
        return Err(WriterError::Conflict(
            "brain fingerprint changed".to_owned(),
        ));
    }
    let record = compose_record_from_evidence(
        outcome,
        &loaded,
        next_revision(Some(&current)),
        now,
        Value::Null,
        Value::Null,
        Value::Object(Map::new()),
    )?;
    write_record(&path, &record, now)?;
    Ok(record)
}

/// Abandon a refresh with one evidence-recordable reason.
pub fn abandon_refresh(
    journal_path: &Path,
    permit: BrainRefreshPermit,
    reason_code: &str,
    diagnostic: Map<String, Value>,
    now: DateTime<Utc>,
) -> Result<Value, WriterError> {
    let component = target_component(reason_code).ok_or_else(|| {
        WriterError::Conflict("brain abandon reason is not recordable evidence".to_owned())
    })?;
    let path = brain_state_path(journal_path);
    let _lock = hold_record_lock(&path)?;
    let Some((raw, current)) = read_current(&path, now)? else {
        return Err(WriterError::Conflict(
            "brain refresh checking marker is absent".to_owned(),
        ));
    };
    assert_finish_allowed(&permit, &current, now)?;
    let mut evidence = object_field(&raw, "evidence")?.clone();
    evidence.insert(
        component.to_owned(),
        component_for_reason(reason_code, diagnostic.clone(), now)?,
    );
    let revision = next_revision(Some(&current));
    let record = compose_direct_record(DirectRecord {
        revision,
        reason_code: reason_code.to_owned(),
        active_lane: current.active_lane.clone(),
        active_provider: current.active_provider.clone(),
        active_model: current.active_model.clone(),
        fingerprint_sha256: current.fingerprint_sha256.clone(),
        evidence: Value::Object(evidence),
        checking: Value::Null,
        marker: raw
            .get("runtime_failure_marker")
            .cloned()
            .unwrap_or(Value::Null),
        diagnostic: Value::Object(diagnostic),
        now,
    });
    write_record(&path, &record, now)?;
    Ok(record)
}

/// Finish an SPP prerequisite renewal.
pub fn finish_prerequisite_renewal(
    journal_path: &Path,
    permit: BrainRefreshPermit,
    lane_prerequisites: Value,
    now: DateTime<Utc>,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> Result<Value, WriterError> {
    let component = parse_component("lane_prerequisites", &lane_prerequisites, now)?;
    if component.status == "not_attempted" {
        return Err(WriterError::Validation(ValidationError {
            path: "lane_prerequisites.status".to_owned(),
            reason: "prerequisite renewal requires ok or declared failure".to_owned(),
        }));
    }
    let loaded = fingerprint_for_write(journal_path, bundled_runtime_fingerprint_sha256)?
        .ok_or_else(|| WriterError::Conflict("brain fingerprint key is unavailable".to_owned()))?;
    let path = brain_state_path(journal_path);
    let _lock = hold_record_lock(&path)?;
    let Some((raw, current)) = read_current(&path, now)? else {
        return Err(WriterError::Conflict(
            "brain refresh checking marker is absent".to_owned(),
        ));
    };
    assert_finish_allowed(&permit, &current, now)?;
    if loaded.sha256 != permit.fingerprint_sha256 {
        return Err(WriterError::Conflict(
            "brain fingerprint changed".to_owned(),
        ));
    }
    if loaded.resolution.lane.as_deref() != Some("spp") {
        return Err(WriterError::Conflict("brain lane changed".to_owned()));
    }
    let Some(mut evidence) = safe_prerequisite_evidence(&raw, &current, now) else {
        return Err(WriterError::Conflict(
            "brain prerequisite evidence is unsafe".to_owned(),
        ));
    };
    evidence.insert("lane_prerequisites".to_owned(), lane_prerequisites);
    let record = compose_record_from_evidence(
        Value::Object(evidence),
        &loaded,
        next_revision(Some(&current)),
        now,
        Value::Null,
        Value::Null,
        Value::Object(Map::new()),
    )?;
    write_record(&path, &record, now)?;
    Ok(record)
}

/// Abandon an SPP prerequisite renewal by delegating to its finish path.
pub fn abandon_prerequisite_renewal(
    journal_path: &Path,
    permit: BrainRefreshPermit,
    reason_code: &str,
    diagnostic: Map<String, Value>,
    now: DateTime<Utc>,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> Result<Value, WriterError> {
    finish_prerequisite_renewal(
        journal_path,
        permit,
        component_for_reason(reason_code, diagnostic, now)?,
        now,
        bundled_runtime_fingerprint_sha256,
    )
}

/// Record an independent runtime failure without acquiring the refresh lease.
pub fn record_runtime_failure(
    journal_path: &Path,
    reason_code: &str,
    component: &str,
    expected_fingerprint_sha256: &str,
    diagnostic: Map<String, Value>,
    now: DateTime<Utc>,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> RuntimeFailureResult {
    let vocabulary = &local_contract().brain_state;
    if !vocabulary
        .reason_codes
        .iter()
        .any(|reason| reason == reason_code)
        || vocabulary
            .projection_only_reason_codes
            .iter()
            .any(|reason| reason == reason_code)
        || !vocabulary
            .reason_to_aggregate
            .get(reason_code)
            .is_some_and(|aggregate| vocabulary.runtime_failure_aggregates.contains(aggregate))
    {
        return rejected("reason_not_recordable", None);
    }
    if !vocabulary
        .runtime_failure_components
        .iter()
        .any(|candidate| candidate == component)
        || !vocabulary
            .evidence_reason_codes
            .get(component)
            .is_some_and(|reasons| reasons.iter().any(|reason| reason == reason_code))
    {
        return rejected("component_reason_not_allowed", None);
    }
    if component_for_reason_in(component, reason_code, diagnostic.clone(), now).is_err() {
        return rejected("reason_not_recordable", None);
    }
    if !is_sha256(expected_fingerprint_sha256) {
        return rejected("fingerprint_mismatch", None);
    }
    let path = brain_state_path(journal_path);
    let result = (|| {
        let _lock = hold_record_lock(&path)?;
        let (current_raw, current, current_readable) = match read_current(&path, now) {
            Ok(Some((raw, record))) => (Some(raw), Some(record), true),
            Ok(None) => (None, None, true),
            Err(WriterError::Io(error)) => {
                return Err(RuntimeFailureInternal::Rejected(
                    "state_unavailable",
                    Some(error),
                ));
            }
            Err(_) => (None, None, false),
        };
        let loaded = match fingerprint_for_write(journal_path, bundled_runtime_fingerprint_sha256) {
            Ok(Some(loaded)) => loaded,
            Ok(None) => {
                return Err(RuntimeFailureInternal::Rejected(
                    "fingerprint_not_available",
                    None,
                ));
            }
            Err(error) => {
                return Err(RuntimeFailureInternal::Rejected(
                    "fingerprint_not_available",
                    Some(error.to_string()),
                ));
            }
        };
        if loaded.sha256 != expected_fingerprint_sha256 {
            return Err(RuntimeFailureInternal::Rejected(
                "fingerprint_mismatch",
                None,
            ));
        }
        let revision = if current_readable {
            next_revision(current.as_ref())
        } else {
            1
        };
        let mut evidence = current_raw
            .as_ref()
            .filter(|raw| {
                raw.get("fingerprint_sha256").and_then(Value::as_str)
                    == Some(loaded.sha256.as_str())
            })
            .and_then(|raw| raw.get("evidence").and_then(Value::as_object).cloned())
            .unwrap_or_else(empty_evidence);
        evidence.insert(
            component.to_owned(),
            component_for_reason_in(component, reason_code, diagnostic.clone(), now)?,
        );
        let marker = json!({
            "marker_id": random_id()?,
            "revision": revision,
            "recorded_at": iso(now),
            "reason_code": reason_code,
        });
        let record = compose_record_from_evidence(
            Value::Object(evidence),
            &loaded,
            revision,
            now,
            Value::Null,
            marker,
            Value::Object(diagnostic),
        )?;
        write_record(&path, &record, now)?;
        Ok(record)
    })();
    match result {
        Ok(record) => RuntimeFailureResult {
            accepted: true,
            record: Some(record),
            rejected_reason: None,
            error: None,
        },
        Err(RuntimeFailureInternal::Rejected(reason, error)) => rejected(reason, error),
        Err(RuntimeFailureInternal::Writer(error)) => {
            rejected("state_unavailable", Some(error.to_string()))
        }
    }
}

enum RuntimeFailureInternal {
    Rejected(&'static str, Option<String>),
    Writer(WriterError),
}

impl From<WriterError> for RuntimeFailureInternal {
    fn from(error: WriterError) -> Self {
        Self::Writer(error)
    }
}

fn begin_refresh_under_lease(
    journal_path: &Path,
    now: DateTime<Utc>,
    run_id: Option<String>,
    expected: Option<&str>,
    expect_absent: bool,
    bundled_runtime: Option<String>,
    lease: FileLease,
) -> Result<Option<BrainRefreshPermit>, BeginRefreshError> {
    let config = match config(journal_path) {
        Ok(config) => config,
        Err(_) => return Ok(None),
    };
    let resolution = derive_active_brain_lane(&config);
    if resolution.lane.is_none() {
        return if expected.is_some() || expect_absent {
            Err(BeginRefreshError::ExpectedFingerprintStale(
                "active brain fingerprint is unavailable".to_owned(),
            ))
        } else {
            Ok(None)
        };
    }
    if resolution.lane.as_deref() == Some("none") {
        if expected.is_some() || expect_absent {
            return Err(BeginRefreshError::ExpectedFingerprintStale(
                "active brain fingerprint is unavailable".to_owned(),
            ));
        }
        begin_none_lane(journal_path, now, &resolution).map_err(BeginRefreshError::Writer)?;
        return Ok(None);
    }
    let key = if expect_absent {
        if load_existing_fingerprint_key(journal_path).is_some() {
            return Err(BeginRefreshError::ExpectedFingerprintStale(
                "active brain fingerprint is present".to_owned(),
            ));
        }
        generate_fingerprint_key(journal_path).map_err(BeginRefreshError::Writer)?
    } else if expected.is_some() {
        load_existing_fingerprint_key(journal_path).ok_or_else(|| {
            BeginRefreshError::ExpectedFingerprintStale(
                "active brain fingerprint is absent".to_owned(),
            )
        })?
    } else {
        generate_fingerprint_key(journal_path).map_err(BeginRefreshError::Writer)?
    };
    let sha256 =
        match build_active_brain_fingerprint(&config, &key, bundled_runtime.map(Value::String)) {
            Ok(Some(sha256)) => sha256,
            // Python compares the expected non-null fingerprint to the failed
            // build's `None` first, so this observable error wins over unavailable.
            Ok(None) | Err(_) if expected.is_some() => {
                return Err(BeginRefreshError::ExpectedFingerprintStale(
                    "active brain fingerprint changed".to_owned(),
                ));
            }
            Ok(None) | Err(_) if expect_absent => {
                return Err(BeginRefreshError::ExpectedFingerprintStale(
                    "active brain fingerprint is unavailable".to_owned(),
                ));
            }
            Ok(None) | Err(_) => return Ok(None),
        };
    if expected.is_some_and(|expected| expected != sha256) {
        return Err(BeginRefreshError::ExpectedFingerprintStale(
            "active brain fingerprint changed".to_owned(),
        ));
    }
    let run_id = run_id
        .map_or_else(random_id, Ok)
        .map_err(BeginRefreshError::Writer)?;
    let expires_at = now + checking_ttl();
    let path = brain_state_path(journal_path);
    let _lock = hold_record_lock(&path).map_err(BeginRefreshError::Writer)?;
    let current = read_current(&path, now).map_err(BeginRefreshError::Writer)?;
    let revision = next_revision(current.as_ref().map(|(_, record)| record));
    let marker_seen = current.as_ref().and_then(|(raw, _)| marker_id(Some(raw)));
    let marker = current
        .as_ref()
        .and_then(|(raw, _)| raw.get("runtime_failure_marker").cloned())
        .unwrap_or(Value::Null);
    let loaded = LoadedFingerprint {
        resolution,
        sha256: sha256.clone(),
    };
    let record = compose_checking_record(CheckingRecord {
        revision,
        now,
        expires_at,
        run_id: &run_id,
        loaded: &loaded,
        evidence: empty_evidence(),
        marker,
        marker_seen: marker_seen.clone(),
    });
    write_record(&path, &record, now).map_err(BeginRefreshError::Writer)?;
    Ok(Some(BrainRefreshPermit {
        run_id,
        started_at: now,
        expires_at,
        fingerprint_sha256: sha256,
        checking_revision: revision,
        runtime_failure_marker_seen: marker_seen,
        _lease: lease,
    }))
}

struct LoadedFingerprint {
    resolution: LaneResolution,
    sha256: String,
}

fn fingerprint_for_write(
    journal_path: &Path,
    bundled_runtime: Option<String>,
) -> Result<Option<LoadedFingerprint>, WriterError> {
    let config = config(journal_path)?;
    let Some(key) = load_existing_fingerprint_key(journal_path) else {
        return Ok(None);
    };
    let resolution = derive_active_brain_lane(&config);
    let sha256 = build_active_brain_fingerprint(&config, &key, bundled_runtime.map(Value::String))
        .map_err(|error| WriterError::Fingerprint(error.to_string()))?
        .ok_or_else(|| {
            WriterError::Fingerprint("active brain fingerprint is unavailable".to_owned())
        })?;
    Ok(Some(LoadedFingerprint { resolution, sha256 }))
}

fn begin_none_lane(
    journal_path: &Path,
    now: DateTime<Utc>,
    resolution: &LaneResolution,
) -> Result<(), WriterError> {
    let path = brain_state_path(journal_path);
    let _lock = hold_record_lock(&path)?;
    let current = match read_current(&path, now) {
        Ok(current) => current,
        Err(WriterError::Validation(_)) => None,
        Err(error) => return Err(error),
    };
    let record = compose_direct_record(DirectRecord {
        revision: next_revision(current.as_ref().map(|(_, record)| record)),
        reason_code: "thinking_engine_not_chosen".to_owned(),
        active_lane: "none".to_owned(),
        active_provider: Some(resolution.provider.clone()),
        active_model: resolution.model.clone(),
        fingerprint_sha256: None,
        evidence: Value::Object({
            let mut evidence = empty_evidence();
            evidence.insert(
                "configuration".to_owned(),
                component_for_reason("thinking_engine_not_chosen", Map::new(), now)?,
            );
            evidence
        }),
        checking: Value::Null,
        marker: Value::Null,
        diagnostic: Value::Object(Map::new()),
        now,
    });
    write_record(&path, &record, now)
}

struct CheckingRecord<'a> {
    revision: u64,
    now: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    run_id: &'a str,
    loaded: &'a LoadedFingerprint,
    evidence: Map<String, Value>,
    marker: Value,
    marker_seen: Option<String>,
}

fn compose_checking_record(input: CheckingRecord<'_>) -> Value {
    json!({
        "schema_version": local_contract().brain_state.schema_version,
        "revision": input.revision,
        "aggregate_state": "checking",
        "reason_code": "brain_check_in_progress",
        "active_lane": input.loaded.resolution.lane.clone(),
        "active_provider": input.loaded.resolution.provider.clone(),
        "active_model": input.loaded.resolution.model.clone(),
        "fingerprint_sha256": input.loaded.sha256,
        "checking": {
            "run_id": input.run_id,
            "started_at": iso(input.now),
            "expires_at": iso(input.expires_at),
            "fingerprint_sha256": input.loaded.sha256,
            "checking_revision": input.revision,
            "runtime_failure_marker_seen": input.marker_seen,
        },
        "evidence": input.evidence,
        "runtime_failure_marker": input.marker,
        "diagnostic": {},
        "updated_at": iso(input.now),
    })
}

fn compose_record_from_evidence(
    evidence_value: Value,
    loaded: &LoadedFingerprint,
    revision: u64,
    now: DateTime<Utc>,
    checking: Value,
    marker: Value,
    diagnostic: Value,
) -> Result<Value, WriterError> {
    let evidence = parse_evidence(Some(&evidence_value), now)?;
    let marker_typed = (!marker.is_null())
        .then(|| parse_runtime_failure_marker(&marker, now))
        .transpose()?;
    let provisional = BrainStateRecord {
        schema_version: local_contract().brain_state.schema_version,
        revision,
        updated_at: now,
        aggregate_state: "ready".to_owned(),
        reason_code: None,
        active_lane: loaded.resolution.lane.clone().unwrap_or_default(),
        active_provider: Some(loaded.resolution.provider.clone()),
        active_model: loaded.resolution.model.clone(),
        fingerprint_sha256: Some(loaded.sha256.clone()),
        evidence,
        checking: None,
        runtime_failure_marker: marker_typed,
        diagnostic: BTreeMap::new(),
    };
    let runtime_reason = provisional
        .runtime_failure_marker
        .as_ref()
        .and_then(|marker| {
            (u64::try_from(marker.revision).ok() == Some(revision))
                .then_some(marker.reason.as_str())
        });
    let (aggregate, reason) = reduce_evidence_with_runtime(&provisional, now, true, runtime_reason);
    Ok(compose_direct_record(DirectRecord {
        revision,
        reason_code: reason.unwrap_or_default(),
        active_lane: provisional.active_lane,
        active_provider: provisional.active_provider,
        active_model: provisional.active_model,
        fingerprint_sha256: provisional.fingerprint_sha256,
        evidence: evidence_value,
        checking,
        marker,
        diagnostic,
        now,
    })
    .tap_aggregate(aggregate))
}

trait WithAggregate {
    fn tap_aggregate(self, aggregate: String) -> Self;
}

impl WithAggregate for Value {
    fn tap_aggregate(mut self, aggregate: String) -> Self {
        self.as_object_mut()
            .expect("composed record is an object")
            .insert("aggregate_state".to_owned(), Value::String(aggregate));
        self
    }
}

struct DirectRecord {
    revision: u64,
    reason_code: String,
    active_lane: String,
    active_provider: Option<String>,
    active_model: Option<String>,
    fingerprint_sha256: Option<String>,
    evidence: Value,
    checking: Value,
    marker: Value,
    diagnostic: Value,
    now: DateTime<Utc>,
}

fn compose_direct_record(input: DirectRecord) -> Value {
    let aggregate = local_contract()
        .brain_state
        .reason_to_aggregate
        .get(&input.reason_code)
        .cloned()
        .unwrap_or_else(|| "ready".to_owned());
    json!({
        "schema_version": local_contract().brain_state.schema_version,
        "revision": input.revision,
        "aggregate_state": aggregate,
        "reason_code": if input.reason_code.is_empty() { Value::Null } else { Value::String(input.reason_code) },
        "active_lane": input.active_lane,
        "active_provider": input.active_provider,
        "active_model": input.active_model,
        "fingerprint_sha256": input.fingerprint_sha256,
        "checking": input.checking,
        "evidence": input.evidence,
        "runtime_failure_marker": input.marker,
        "diagnostic": input.diagnostic,
        "updated_at": iso(input.now),
    })
}

fn raw_evidence_value(value: &Value) -> Result<Map<String, Value>, WriterError> {
    object_field(value, "evidence").cloned()
}

fn safe_prerequisite_evidence(
    raw: &Value,
    current: &BrainStateRecord,
    now: DateTime<Utc>,
) -> Option<Map<String, Value>> {
    if current.active_lane != "spp" || record_timestamp_invalid(current, now) {
        return None;
    }
    for name in ["configuration", "generate", "cogitate"] {
        let component = current.evidence.get(name).and_then(Option::as_ref)?;
        if component.status != "ok" || component.expires_at.is_none_or(|expires| now >= expires) {
            return None;
        }
    }
    raw_evidence_value(raw).ok()
}

fn record_timestamp_invalid(record: &BrainStateRecord, now: DateTime<Utc>) -> bool {
    if record.updated_at > now
        || record
            .checking
            .as_ref()
            .is_some_and(|checking| checking.started_at > now)
        || record
            .runtime_failure_marker
            .as_ref()
            .is_some_and(|marker| marker.recorded_at > now)
    {
        return true;
    }
    record.evidence.values().flatten().any(|component| {
        component.observed_at > now
            || component
                .expires_at
                .is_none_or(|expires| expires < component.observed_at)
    })
}

fn assert_finish_allowed(
    permit: &BrainRefreshPermit,
    current: &BrainStateRecord,
    now: DateTime<Utc>,
) -> Result<(), WriterError> {
    // Ownership is represented by the unforgeable, still-live FileLease field.
    if now >= permit.expires_at {
        return Err(WriterError::Conflict(
            "brain refresh permit expired".to_owned(),
        ));
    }
    let checking = current.checking.as_ref().ok_or_else(|| {
        WriterError::Conflict("brain refresh checking marker is absent".to_owned())
    })?;
    if current.revision != permit.checking_revision {
        return Err(WriterError::Conflict(
            "brain refresh record revision changed".to_owned(),
        ));
    }
    if checking.run_id != permit.run_id {
        return Err(WriterError::Conflict(
            "brain refresh run id changed".to_owned(),
        ));
    }
    if u64::try_from(checking.checking_revision).ok() != Some(permit.checking_revision) {
        return Err(WriterError::Conflict(
            "brain refresh revision changed".to_owned(),
        ));
    }
    if checking.runtime_failure_marker_seen != permit.runtime_failure_marker_seen {
        return Err(WriterError::Conflict(
            "brain runtime failure marker changed".to_owned(),
        ));
    }
    Ok(())
}

fn target_component(reason_code: &str) -> Option<&str> {
    let vocabulary = &local_contract().brain_state;
    vocabulary.component_order.iter().find_map(|component| {
        vocabulary
            .evidence_reason_codes
            .get(component)
            .is_some_and(|reasons| reasons.iter().any(|reason| reason == reason_code))
            .then_some(component.as_str())
    })
}

fn component_for_reason(
    reason_code: &str,
    diagnostic: Map<String, Value>,
    now: DateTime<Utc>,
) -> Result<Value, WriterError> {
    let component = target_component(reason_code)
        .ok_or_else(|| WriterError::Conflict("reason is not recordable evidence".to_owned()))?;
    component_for_reason_in(component, reason_code, diagnostic, now)
}

fn component_for_reason_in(
    component: &str,
    reason_code: &str,
    diagnostic: Map<String, Value>,
    now: DateTime<Utc>,
) -> Result<Value, WriterError> {
    let status = component_status_for_reason(reason_code)?;
    let value = json!({
        "status": status,
        "observed_at": iso(now),
        "reason_code": reason_code,
        "diagnostic": diagnostic,
    });
    parse_component(component, &value, now)?;
    Ok(value)
}

fn empty_evidence() -> Map<String, Value> {
    local_contract()
        .brain_state
        .record_fields
        .evidence
        .iter()
        .map(|component| (component.clone(), Value::Null))
        .collect()
}

fn write_record(path: &Path, record: &Value, now: DateTime<Utc>) -> Result<(), WriterError> {
    validate_brain_state_record(record, now)?;
    write_json(
        path,
        record,
        JsonWriteOptions {
            mode: Some(BRAIN_FILE_MODE),
            indent: Some(2),
            sort_keys: true,
        },
    )
    .map_err(atomic_error)
}

fn read_current(
    path: &Path,
    now: DateTime<Utc>,
) -> Result<Option<(Value, BrainStateRecord)>, WriterError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(WriterError::Io(error.to_string())),
    };
    let value = serde_json::from_slice(&bytes).map_err(|error| {
        WriterError::Validation(ValidationError {
            path: "record".to_owned(),
            reason: error.to_string(),
        })
    })?;
    let parsed = validate_brain_state_record(&value, now)?;
    Ok(Some((value, parsed)))
}

fn config(journal_path: &Path) -> Result<Map<String, Value>, WriterError> {
    crate::read_journal_config(journal_path)
        .map_err(|error| WriterError::Config(error.to_string()))
        .map(|read| read.config.unwrap_or_default())
}

/// Exclusive flock on the brain record sidecar. Hidden so writer component
/// tests contend on the same path and mode as production.
#[doc(hidden)]
pub fn hold_record_lock(path: &Path) -> Result<solstone_core_journal_io::FileLock, WriterError> {
    hold_lock(
        path,
        LockOptions {
            mode: Some(BRAIN_FILE_MODE),
            ..LockOptions::default()
        },
    )
    .map_err(lock_error)
}

fn next_revision(record: Option<&BrainStateRecord>) -> u64 {
    record.map_or(1, |record| record.revision + 1)
}

fn marker_id(record: Option<&Value>) -> Option<String> {
    record
        .and_then(|record| record.get("runtime_failure_marker"))
        .and_then(Value::as_object)
        .and_then(|marker| marker.get("marker_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn checking_ttl() -> Duration {
    Duration::seconds(local_contract().brain_state.checking_ttl_seconds as i64)
}

fn iso(now: DateTime<Utc>) -> String {
    now.to_rfc3339()
}

fn validate_expected(expected: Option<&str>, expect_absent: bool) -> Result<(), BeginRefreshError> {
    if expected.is_some_and(|value| !is_sha256(value)) {
        return Err(BeginRefreshError::ExpectedFingerprintStale(
            "expected_active_fingerprint_sha256: expected SHA-256 hex string".to_owned(),
        ));
    }
    if expected.is_some() && expect_absent {
        return Err(BeginRefreshError::InvalidArgument(
            "expected active fingerprint and expected absence are mutually exclusive".to_owned(),
        ));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

// ⚠ These two are the crate's ONLY use of a cryptographic RNG, and the
// dependency that served them pulled a C build step into a crate the iOS canary
// compiles. `getrandom` is the OS entropy syscall and nothing else: no C, no
// linkage, no native-dependency release proof owed. `docs/PORTING.md` § native
// dependency release proof is the rule that makes the distinction load-bearing.
fn secure_key() -> Result<[u8; FINGERPRINT_KEY_BYTES], WriterError> {
    let mut key = [0_u8; FINGERPRINT_KEY_BYTES];
    fill_random(&mut key).map_err(|_| WriterError::Random)?;
    Ok(key)
}

fn random_id() -> Result<String, WriterError> {
    let mut bytes = [0_u8; 16];
    fill_random(&mut bytes).map_err(|_| WriterError::Random)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn object_field<'a>(value: &'a Value, field: &str) -> Result<&'a Map<String, Value>, WriterError> {
    value
        .get(field)
        .and_then(Value::as_object)
        .ok_or_else(|| WriterError::Conflict(format!("{field} must be an object")))
}

fn rejected(reason: &str, error: Option<String>) -> RuntimeFailureResult {
    RuntimeFailureResult {
        accepted: false,
        record: None,
        rejected_reason: Some(reason.to_owned()),
        error,
    }
}

fn lock_error(error: solstone_core_journal_io::LockError) -> WriterError {
    WriterError::Io(error.to_string())
}

fn lease_error(error: solstone_core_journal_io::LeaseError) -> WriterError {
    WriterError::Io(error.to_string())
}

fn atomic_error(error: solstone_core_journal_io::AtomicWriteError) -> WriterError {
    WriterError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use chrono::DateTime;

    use super::*;
    use crate::fixture::projection_fixture;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TestJournal(PathBuf);

    impl TestJournal {
        fn new() -> Self {
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "solstone-core-brain-writer-{}-{}-{sequence}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("time after epoch")
                    .as_nanos()
            ));
            fs::create_dir_all(&path).expect("journal directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write_config(&self, name: &str) {
            let config = projection_fixture()
                .configs
                .get(name)
                .expect("fixture config");
            let path = self.path().join("config/journal.json");
            fs::create_dir_all(path.parent().expect("config parent")).unwrap();
            fs::write(path, serde_json::to_vec(config).unwrap()).unwrap();
        }

        fn write_fixture_key(&self) {
            let hex = &projection_fixture().hmac_key_hex;
            let key = (0..hex.len())
                .step_by(2)
                .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
                .collect::<Vec<_>>();
            let path = brain_fingerprint_key_path(self.path());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, key).unwrap();
        }

        fn seed_record(&self, name: &str) {
            let record = projection_fixture()
                .records
                .get(name)
                .expect("fixture record");
            let path = brain_state_path(self.path());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, serde_json::to_vec(record).unwrap()).unwrap();
        }

        fn seed_value(&self, record: &Value) {
            let path = brain_state_path(self.path());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, serde_json::to_vec(record).unwrap()).unwrap();
        }
    }

    impl Drop for TestJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(&projection_fixture().now)
            .expect("fixture now")
            .with_timezone(&Utc)
    }

    fn ready_outcome() -> Value {
        projection_fixture()
            .records
            .get("lane_byo_cloud/ready")
            .expect("ready record")
            .get("evidence")
            .expect("evidence")
            .clone()
    }

    fn begin_cloud(journal: &TestJournal) -> BrainRefreshPermit {
        journal.write_config("lane_byo_cloud");
        begin_refresh(
            journal.path(),
            fixture_now(),
            Some("run".to_owned()),
            None,
            false,
            None,
        )
        .unwrap()
        .expect("refresh permit")
    }

    fn record_bytes(journal: &TestJournal) -> Vec<u8> {
        fs::read(brain_state_path(journal.path())).unwrap()
    }

    #[test]
    fn generated_timestamps_use_python_isoformat_utc_style() {
        assert!(iso(fixture_now()).ends_with("+00:00"));
        assert!(!iso(fixture_now()).ends_with('Z'));
    }

    #[test]
    fn begin_refresh_prefers_expected_fingerprint_changed_over_unavailable() {
        let journal = TestJournal::new();
        journal.write_config("lane_bundled");
        generate_fingerprint_key(journal.path()).unwrap();
        let expected = "a".repeat(64);
        let error = begin_refresh(
            journal.path(),
            fixture_now(),
            None,
            Some(&expected),
            false,
            None,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "active brain fingerprint changed");
    }

    #[test]
    fn begin_refresh_absent_contract_reports_unavailable_for_failed_build() {
        let journal = TestJournal::new();
        journal.write_config("lane_bundled");
        let error =
            begin_refresh(journal.path(), fixture_now(), None, None, true, None).unwrap_err();
        assert_eq!(error.to_string(), "active brain fingerprint is unavailable");
    }

    #[test]
    fn reachable_writer_selector_has_the_documented_28_cases() {
        assert_eq!(REACHABLE_WRITE_CASES.len(), 28);
        assert!(REACHABLE_WRITE_CASES.iter().all(|name| {
            name.starts_with("lane_bundled/")
                || name.starts_with("lane_byo_cloud/")
                || name.starts_with("lane_byo_endpoint/")
                || name.starts_with("lane_spp/")
        }));
        assert!(REACHABLE_WRITE_CASES.iter().all(|name| {
            !name.contains("fingerprint_")
                && !name.contains("marker_")
                && projection_fixture()
                    .records
                    .get(*name)
                    .is_some_and(|record| record["checking"].is_null())
        }));
    }

    #[test]
    fn reachable_writer_cases_are_emitted_by_finish_refresh() {
        for name in REACHABLE_WRITE_CASES {
            let fixture = projection_fixture();
            let target = fixture.records.get(name).expect("reachable fixture record");
            let (lane, _) = name.split_once('/').expect("fixture case name");
            let now = DateTime::parse_from_rfc3339(
                target["updated_at"].as_str().expect("fixture updated_at"),
            )
            .expect("fixture timestamp")
            .with_timezone(&Utc);
            let journal = TestJournal::new();
            journal.write_config(lane);
            journal.write_fixture_key();

            let mut prior = fixture.records[&format!("{lane}/ready")].clone();
            prior["revision"] = json!(1);
            journal.seed_value(&prior);

            let bundled_runtime = (lane == "lane_bundled")
                .then(|| fixture.bundled_runtime_fingerprint_sha256.clone());
            let permit = begin_refresh(
                journal.path(),
                now,
                Some("writer-parity".to_owned()),
                None,
                false,
                bundled_runtime.clone(),
            )
            .expect("begin refresh")
            .expect("refresh permit");
            let written = finish_refresh(
                journal.path(),
                permit,
                target["evidence"].clone(),
                now,
                bundled_runtime,
            )
            .expect("finish refresh");
            assert_eq!(written, *target, "writer parity mismatch: {name}");
            let disk: Value = serde_json::from_slice(&record_bytes(&journal)).expect("record JSON");
            assert_eq!(disk, *target, "writer parity disk mismatch: {name}");
        }
    }

    #[test]
    fn composition_matches_every_projection_fixture_record() {
        for (name, record) in &projection_fixture().records {
            let now = DateTime::parse_from_rfc3339(
                record["updated_at"].as_str().expect("fixture updated_at"),
            )
            .unwrap()
            .with_timezone(&Utc);
            let composed = compose_direct_record(DirectRecord {
                revision: record["revision"].as_u64().expect("fixture revision"),
                reason_code: record["reason_code"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                active_lane: record["active_lane"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                active_provider: record["active_provider"].as_str().map(str::to_owned),
                active_model: record["active_model"].as_str().map(str::to_owned),
                fingerprint_sha256: record["fingerprint_sha256"].as_str().map(str::to_owned),
                evidence: record["evidence"].clone(),
                checking: record["checking"].clone(),
                marker: record["runtime_failure_marker"].clone(),
                diagnostic: record["diagnostic"].clone(),
                now,
            });
            assert_eq!(&composed, record, "fixture composition mismatch: {name}");
        }
    }

    #[test]
    fn none_lane_write_needs_no_refresh_lease_and_validates() {
        let journal = TestJournal::new();
        journal.write_config("lane_none");
        let permit = begin_refresh(
            journal.path(),
            fixture_now(),
            Some("run".to_owned()),
            None,
            false,
            None,
        )
        .unwrap();
        assert!(permit.is_none());
        assert!(!brain_refresh_lease_path(journal.path()).exists());
        let record = fs::read(brain_state_path(journal.path())).unwrap();
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(brain_state_path(journal.path()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let value: Value = serde_json::from_slice(&record).unwrap();
        validate_brain_state_record(&value, fixture_now()).unwrap();
        assert_eq!(value["reason_code"], "thinking_engine_not_chosen");
        assert!(value["runtime_failure_marker"].is_null());
        let evidence = value["evidence"].as_object().expect("evidence object");
        assert_eq!(evidence.len(), 4);
        assert!(evidence["configuration"].is_object());
        for component in ["lane_prerequisites", "generate", "cogitate"] {
            assert!(evidence[component].is_null(), "{component} must be null");
        }
    }

    #[test]
    fn checking_record_validates_without_a_live_lease() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        drop(permit);
        let record: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
        validate_brain_state_record(&record, fixture_now()).unwrap();
        assert_eq!(record["reason_code"], "brain_check_in_progress");
    }

    fn assert_finish_refusal_preserves_bytes(
        mutator: impl FnOnce(&mut Value),
        finish_now: DateTime<Utc>,
    ) {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let path = brain_state_path(journal.path());
        let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        mutator(&mut record);
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(finish_refresh(journal.path(), permit, ready_outcome(), finish_now, None).is_err());
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn finish_fence_expiry_refuses_without_writing() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let before = record_bytes(&journal);
        assert!(
            finish_refresh(
                journal.path(),
                permit,
                ready_outcome(),
                fixture_now() + checking_ttl(),
                None
            )
            .is_err()
        );
        assert_eq!(record_bytes(&journal), before);
    }

    #[test]
    fn finish_fence_missing_checking_refuses_without_writing() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        journal.seed_record("lane_byo_cloud/ready");
        let before = record_bytes(&journal);
        assert!(
            finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None,).is_err()
        );
        assert_eq!(record_bytes(&journal), before);
    }

    #[test]
    fn finish_fence_revision_refuses_without_writing() {
        assert_finish_refusal_preserves_bytes(
            |record| record["revision"] = json!(record["revision"].as_u64().unwrap() + 1),
            fixture_now(),
        );
    }

    #[test]
    fn finish_fence_run_id_refuses_without_writing() {
        assert_finish_refusal_preserves_bytes(
            |record| record["checking"]["run_id"] = json!("other-run"),
            fixture_now(),
        );
    }

    #[test]
    fn finish_fence_checking_revision_refuses_without_writing() {
        assert_finish_refusal_preserves_bytes(
            |record| {
                record["checking"]["checking_revision"] =
                    json!(record["checking"]["checking_revision"].as_u64().unwrap() + 1)
            },
            fixture_now(),
        );
    }

    #[test]
    fn finish_fence_marker_seen_refuses_without_writing() {
        assert_finish_refusal_preserves_bytes(
            |record| record["checking"]["runtime_failure_marker_seen"] = json!("new-marker"),
            fixture_now(),
        );
    }

    #[test]
    fn permit_lease_ownership_fence_is_type_enforced() {
        fn consumes_permit(_permit: BrainRefreshPermit) {}
        let journal = TestJournal::new();
        consumes_permit(begin_cloud(&journal));
        // BrainRefreshPermit has no Clone implementation and owns the private,
        // non-Clone FileLease. A caller cannot retain a usable permit after
        // releasing its lease, so Python's leading ownership assertion is
        // enforced by the Rust representation rather than a forgeable flag.
    }

    fn begin_spp_renewal(journal: &TestJournal) -> BrainRefreshPermit {
        journal.write_config("lane_spp");
        journal.write_fixture_key();
        journal.seed_record("lane_spp/ready");
        match begin_prerequisite_renewal(
            journal.path(),
            fixture_now(),
            Some("renew".to_owned()),
            None,
            None,
        ) {
            BeginPrerequisiteRenewal::Started(permit) => permit,
            result => panic!("expected renewal permit, got {result:?}"),
        }
    }

    #[test]
    fn accepted_paths_increment_revision_once() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        assert_eq!(permit.checking_revision, 1);
        let finished =
            finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None).unwrap();
        assert_eq!(finished["revision"], 2);

        let journal = TestJournal::new();
        let first = begin_cloud(&journal);
        finish_refresh(journal.path(), first, ready_outcome(), fixture_now(), None).unwrap();
        let permit = begin_cloud(&journal);
        let abandoned = abandon_refresh(
            journal.path(),
            permit,
            "provider_unavailable",
            Map::new(),
            fixture_now(),
        )
        .unwrap();
        assert_eq!(abandoned["revision"], 4);

        let journal = TestJournal::new();
        let permit = begin_spp_renewal(&journal);
        assert_eq!(permit.checking_revision, 4);
        let component =
            projection_fixture().records["lane_spp/ready"]["evidence"]["lane_prerequisites"]
                .clone();
        let finished =
            finish_prerequisite_renewal(journal.path(), permit, component, fixture_now(), None)
                .unwrap();
        assert_eq!(finished["revision"], 5);

        let journal = TestJournal::new();
        let permit = begin_spp_renewal(&journal);
        let abandoned = abandon_prerequisite_renewal(
            journal.path(),
            permit,
            "provider_key_missing",
            Map::new(),
            fixture_now(),
            None,
        )
        .unwrap();
        assert_eq!(abandoned["revision"], 5);

        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        let permit = begin_cloud(&journal);
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "generate",
            &permit.fingerprint_sha256,
            Map::new(),
            fixture_now(),
            None,
        );
        assert!(result.accepted, "{result:?}");
        assert_eq!(result.record.unwrap()["revision"], 2);
        drop(permit);

        let journal = TestJournal::new();
        journal.write_config("lane_none");
        begin_refresh(journal.path(), fixture_now(), None, None, false, None).unwrap();
        let record: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
        assert_eq!(record["revision"], 1);
    }

    #[test]
    fn abandon_and_runtime_failure_refusals_preserve_record_bytes() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let before = record_bytes(&journal);
        assert!(
            abandon_refresh(
                journal.path(),
                permit,
                "not-a-reason",
                Map::new(),
                fixture_now(),
            )
            .is_err()
        );
        assert_eq!(record_bytes(&journal), before);

        let journal = TestJournal::new();
        let permit = begin_spp_renewal(&journal);
        let before = record_bytes(&journal);
        assert!(
            finish_prerequisite_renewal(
                journal.path(),
                permit,
                json!({"status": "not_attempted", "observed_at": iso(fixture_now())}),
                fixture_now(),
                None,
            )
            .is_err()
        );
        assert_eq!(record_bytes(&journal), before);

        let journal = TestJournal::new();
        let permit = begin_spp_renewal(&journal);
        let before = record_bytes(&journal);
        assert!(
            abandon_prerequisite_renewal(
                journal.path(),
                permit,
                "provider_unavailable",
                Map::new(),
                fixture_now(),
                None,
            )
            .is_err()
        );
        assert_eq!(record_bytes(&journal), before);

        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let before = record_bytes(&journal);
        let rejected = record_runtime_failure(
            journal.path(),
            "not-a-reason",
            "generate",
            &permit.fingerprint_sha256,
            Map::new(),
            fixture_now(),
            None,
        );
        assert_eq!(
            rejected.rejected_reason.as_deref(),
            Some("reason_not_recordable")
        );
        assert_eq!(record_bytes(&journal), before);
        drop(permit);
    }

    #[test]
    fn abandon_refresh_preserves_caller_diagnostic_at_record_top_level() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let diagnostic = Map::from_iter([
            ("phase".to_owned(), Value::String("failed".to_owned())),
            (
                "runtime_reason".to_owned(),
                Value::String("gpu-unavailable".to_owned()),
            ),
        ]);
        let record = abandon_refresh(
            journal.path(),
            permit,
            "local_server_unhealthy",
            diagnostic.clone(),
            fixture_now(),
        )
        .expect("refresh abandon");
        assert_eq!(record["diagnostic"], Value::Object(diagnostic.clone()));
        let disk: Value = serde_json::from_slice(&record_bytes(&journal)).expect("record JSON");
        assert_eq!(disk["diagnostic"], Value::Object(diagnostic));
    }

    #[test]
    fn finish_preserves_caller_evidence_timestamp_spelling() {
        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        let now = fixture_now();
        let permit = begin_refresh(
            journal.path(),
            now,
            Some("run".to_owned()),
            None,
            false,
            None,
        )
        .unwrap()
        .expect("refresh permit");
        let mut outcome = projection_fixture()
            .records
            .get("lane_byo_cloud/ready")
            .expect("ready record")
            .get("evidence")
            .expect("evidence")
            .clone();
        let components = outcome.as_object_mut().unwrap();
        for (index, component) in components
            .values_mut()
            .filter_map(Value::as_object_mut)
            .enumerate()
        {
            let observed = component["observed_at"].as_str().unwrap();
            component.insert(
                "observed_at".to_owned(),
                Value::String(if index == 0 {
                    observed.replace("+00:00", "Z")
                } else {
                    observed.replace('Z', "+00:00")
                }),
            );
        }
        finish_refresh(journal.path(), permit, outcome, now, None).unwrap();
        let text = String::from_utf8(fs::read(brain_state_path(journal.path())).unwrap()).unwrap();
        assert!(text.contains("\"observed_at\": \"2026-08-06T11:59:00Z\""));
        assert!(text.contains("\"observed_at\": \"2026-08-06T11:59:00+00:00\""));
        assert!(text.contains("\"updated_at\": \"2026-08-06T12:00:00+00:00\""));
    }

    #[test]
    fn fingerprint_key_is_reused_and_mode_is_private() {
        let journal = TestJournal::new();
        let first = generate_fingerprint_key(journal.path()).unwrap();
        let second = generate_fingerprint_key(journal.path()).unwrap();
        assert_eq!(first, second);
        let path = brain_fingerprint_key_path(journal.path());
        assert_eq!(fs::read(path).unwrap(), first);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(brain_fingerprint_key_path(journal.path()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn runtime_failure_publishes_a_valid_marker_record() {
        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        let now = fixture_now();
        let permit = begin_refresh(
            journal.path(),
            now,
            Some("run".to_owned()),
            None,
            false,
            None,
        )
        .unwrap()
        .expect("refresh permit");
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "generate",
            &permit.fingerprint_sha256,
            Map::new(),
            now,
            None,
        );
        assert!(result.accepted, "{result:?}");
        let record = result.record.expect("published record");
        validate_brain_state_record(&record, now).unwrap();
        assert!(record["checking"].is_null());
        assert_eq!(
            record["runtime_failure_marker"]["reason_code"],
            "provider_unavailable"
        );
        drop(permit);
    }

    #[test]
    fn runtime_failure_precedence_prefers_reason_over_component() {
        // There is no (1, 2) pair test: the native API receives `DateTime<Utc>`,
        // so Python's fallible now-normalization has already completed before this
        // function can run. Invalid timestamps are structurally unrepresentable here.
        let journal = TestJournal::new();
        let result = record_runtime_failure(
            journal.path(),
            "not-a-reason",
            "not-a-component",
            "not-a-sha256",
            Map::new(),
            fixture_now(),
            None,
        );
        assert_eq!(
            result.rejected_reason.as_deref(),
            Some("reason_not_recordable")
        );
    }

    #[test]
    fn runtime_failure_precedence_prefers_component_over_fingerprint() {
        let journal = TestJournal::new();
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "configuration",
            "not-a-sha256",
            Map::new(),
            fixture_now(),
            None,
        );
        assert_eq!(
            result.rejected_reason.as_deref(),
            Some("component_reason_not_allowed")
        );
    }

    #[test]
    fn runtime_failure_precedence_prefers_component_over_invalid_diagnostic() {
        let journal = TestJournal::new();
        let mut diagnostic = Map::new();
        diagnostic.insert("unexpected".to_owned(), json!("value"));
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "configuration",
            &"a".repeat(64),
            diagnostic,
            fixture_now(),
            None,
        );
        assert_eq!(
            result.rejected_reason.as_deref(),
            Some("component_reason_not_allowed")
        );
    }

    #[test]
    fn runtime_failure_precedence_prefers_invalid_diagnostic_over_bad_fingerprint() {
        let journal = TestJournal::new();
        let mut diagnostic = Map::new();
        diagnostic.insert("unexpected".to_owned(), json!("value"));
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "generate",
            "not-a-sha256",
            diagnostic,
            fixture_now(),
            None,
        );
        assert_eq!(
            result.rejected_reason.as_deref(),
            Some("reason_not_recordable")
        );
    }

    #[test]
    fn runtime_failure_precedence_prefers_bad_fingerprint_over_unreadable_record() {
        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        fs::create_dir_all(brain_state_path(journal.path())).unwrap();
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "generate",
            "not-a-sha256",
            Map::new(),
            fixture_now(),
            None,
        );
        assert_eq!(
            result.rejected_reason.as_deref(),
            Some("fingerprint_mismatch")
        );
    }

    #[test]
    fn runtime_failure_precedence_prefers_unreadable_record_over_missing_fingerprint() {
        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        fs::create_dir_all(brain_state_path(journal.path())).unwrap();
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "generate",
            &"a".repeat(64),
            Map::new(),
            fixture_now(),
            None,
        );
        assert_eq!(result.rejected_reason.as_deref(), Some("state_unavailable"));
    }

    #[test]
    fn runtime_failure_precedence_prefers_missing_fingerprint_over_mismatch() {
        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "generate",
            &"a".repeat(64),
            Map::new(),
            fixture_now(),
            None,
        );
        assert_eq!(
            result.rejected_reason.as_deref(),
            Some("fingerprint_not_available")
        );
    }
}
