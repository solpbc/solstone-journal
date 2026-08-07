// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use crate::fixture::local_contract;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub path: String,
    pub reason: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.reason)
    }
}

impl std::error::Error for ValidationError {}

#[derive(Debug, Clone, PartialEq)]
pub struct BrainStateRecord {
    pub schema_version: u64,
    pub revision: u64,
    pub updated_at: DateTime<Utc>,
    pub aggregate_state: String,
    pub reason_code: Option<String>,
    pub active_lane: String,
    pub active_provider: Option<String>,
    pub active_model: Option<String>,
    pub fingerprint_sha256: Option<String>,
    pub evidence: BTreeMap<String, Option<EvidenceComponent>>,
    pub checking: Option<Checking>,
    pub runtime_failure_marker: Option<RuntimeFailureMarker>,
    pub diagnostic: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceComponent {
    pub status: String,
    pub observed_at: DateTime<Utc>,
    pub reason: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub diagnostic: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Checking {
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub checking_revision: u64,
    pub fingerprint_sha256: String,
    pub run_id: String,
    pub runtime_failure_marker_seen: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeFailureMarker {
    pub marker_id: String,
    pub revision: u64,
    pub recorded_at: DateTime<Utc>,
    pub reason: String,
}

pub fn validate_brain_state_record(
    value: &Value,
    now: DateTime<Utc>,
) -> Result<BrainStateRecord, ValidationError> {
    let object = match value.as_object() {
        Some(object) => object,
        None => match value.as_array() {
            Some(values) => {
                let mut unknown = values.iter().filter_map(Value::as_str).collect::<Vec<_>>();
                unknown.sort_unstable();
                return unknown
                    .first()
                    .map(|key| failure(key, "unknown top-level field"))
                    .map_or_else(|| Err(failure("record", "must be an object")), Err);
            }
            None => return Err(failure("record", "must be an object")),
        },
    };
    let vocabulary = &local_contract().brain_state;
    closed(object, &vocabulary.record_fields.top_level, "")?;

    let schema_version = required_u64(object, "schema_version")?;
    if schema_version != vocabulary.schema_version {
        return Err(failure("schema_version", "unsupported schema version"));
    }
    let revision = required_u64(object, "revision")?;
    let updated_at = required_time(object, "updated_at", now)?;
    let aggregate_state = required_known(object, "aggregate_state", &vocabulary.aggregate_states)?;
    let reason_code = optional_known(object, "reason_code", &vocabulary.reason_codes)?;
    let active_lane = required_known(object, "active_lane", &vocabulary.lanes)?;
    let active_provider = optional_string(object, "active_provider")?;
    let active_model = optional_string(object, "active_model")?;
    let fingerprint_sha256 = optional_string(object, "fingerprint_sha256")?;
    if let Some(fingerprint) = &fingerprint_sha256
        && !is_sha256(fingerprint)
    {
        return Err(failure(
            "fingerprint_sha256",
            "must be a SHA-256 hex digest",
        ));
    }
    let diagnostic = diagnostic(
        object.get("diagnostic"),
        "diagnostic",
        reason_code.as_deref(),
    )?;
    let evidence = parse_evidence(object.get("evidence"), now)?;
    let checking = match object.get("checking") {
        None | Some(Value::Null) => None,
        Some(value) => Some(parse_checking(value, now)?),
    };
    let runtime_failure_marker = match object.get("runtime_failure_marker") {
        None | Some(Value::Null) => None,
        Some(value) => Some(parse_runtime_failure_marker(value, now)?),
    };

    if aggregate_state == "ready" && reason_code.is_some() {
        return Err(failure(
            "reason_code",
            "must be null when aggregate_state is ready",
        ));
    }
    if aggregate_state != "ready" {
        let reason = reason_code.as_deref().ok_or_else(|| {
            failure(
                "reason_code",
                "must be set when aggregate_state is not ready",
            )
        })?;
        if vocabulary.reason_to_aggregate.get(reason) != Some(&aggregate_state) {
            return Err(failure("reason_code", "does not match aggregate_state"));
        }
    }
    if (aggregate_state == "checking") != checking.is_some() {
        return Err(failure(
            "checking",
            "checking aggregate requires matching checking marker",
        ));
    }
    if let Some(marker) = &runtime_failure_marker
        && vocabulary
            .projection_only_reason_codes
            .iter()
            .any(|reason| reason == &marker.reason)
    {
        return Err(failure(
            "runtime_failure_marker.reason_code",
            "must be evidence-recordable",
        ));
    }

    let record = BrainStateRecord {
        schema_version,
        revision,
        updated_at,
        aggregate_state,
        reason_code,
        active_lane,
        active_provider,
        active_model,
        fingerprint_sha256,
        evidence,
        checking,
        runtime_failure_marker,
        diagnostic,
    };
    validate_reduction(&record)?;
    Ok(record)
}

fn parse_evidence(
    value: Option<&Value>,
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, Option<EvidenceComponent>>, ValidationError> {
    let vocabulary = &local_contract().brain_state;
    let object = match value {
        None | Some(Value::Null) => return Ok(BTreeMap::new()),
        Some(value) => value
            .as_object()
            .ok_or_else(|| failure("evidence", "must be an object"))?,
    };
    closed(object, &vocabulary.record_fields.evidence, "evidence.")?;
    let mut result = BTreeMap::new();
    for component in &vocabulary.record_fields.evidence {
        let parsed = match object.get(component) {
            None | Some(Value::Null) => None,
            Some(value) => Some(parse_component(component, value, now)?),
        };
        result.insert(component.clone(), parsed);
    }
    let causal_reason = ["configuration", "lane_prerequisites"]
        .into_iter()
        .find_map(|name| {
            result
                .get(name)
                .and_then(Option::as_ref)
                .filter(|component| component.status != "ok")
                .and_then(|component| component.reason.as_deref())
        });
    for name in ["generate", "cogitate"] {
        if let Some(component) = result.get(name).and_then(Option::as_ref)
            && component.status == "not_attempted"
        {
            let reason = causal_reason.ok_or_else(|| {
                failure(
                    &format!("evidence.{name}.status"),
                    "not_attempted requires a non-ok prerequisite",
                )
            })?;
            if component.reason.as_deref() != Some(reason) {
                return Err(failure(
                    &format!("evidence.{name}.reason_code"),
                    "not_attempted reason must repeat causal prerequisite reason",
                ));
            }
        }
    }
    for name in ["configuration", "lane_prerequisites"] {
        if result
            .get(name)
            .and_then(Option::as_ref)
            .is_some_and(|component| component.status == "not_attempted")
        {
            return Err(failure(
                &format!("evidence.{name}.status"),
                "not_attempted is only valid for generate/cogitate",
            ));
        }
    }
    Ok(result)
}

fn parse_component(
    component: &str,
    value: &Value,
    now: DateTime<Utc>,
) -> Result<EvidenceComponent, ValidationError> {
    let vocabulary = &local_contract().brain_state;
    let path = format!("evidence.{component}");
    let object = value
        .as_object()
        .ok_or_else(|| failure(&path, "must be an object"))?;
    closed(
        object,
        &vocabulary.record_fields.component,
        &format!("{path}."),
    )?;
    let status = required_known(object, "status", &vocabulary.component_statuses)
        .map_err(|error| prefix_error(error, &path))?;
    let observed_at =
        required_time(object, "observed_at", now).map_err(|error| prefix_error(error, &path))?;
    let reason = optional_known(object, "reason_code", &vocabulary.reason_codes)
        .map_err(|error| prefix_error(error, &path))?;
    let expires_at =
        optional_time(object, "expires_at", now).map_err(|error| prefix_error(error, &path))?;
    if status == "ok" && reason.is_some() {
        return Err(failure(
            &format!("{path}.reason_code"),
            "ok evidence requires null reason",
        ));
    }
    if status != "ok" && reason.is_none() {
        return Err(failure(
            &format!("{path}.reason_code"),
            "non-ok evidence requires reason",
        ));
    }
    if status != "ok" && status != "not_attempted" {
        let allowed = vocabulary
            .evidence_reason_codes
            .get(component)
            .expect("fixture contains each evidence component");
        let reason = reason.as_deref().expect("checked above");
        if !allowed.iter().any(|candidate| candidate == reason) {
            return Err(failure(
                &format!("{path}.reason_code"),
                "reason not allowed for evidence component",
            ));
        }
        let expected = component_status_for_reason(reason)?;
        if status != expected {
            return Err(failure(
                &format!("{path}.status"),
                "component status does not match reason aggregate",
            ));
        }
    }
    let diagnostic = diagnostic(
        object.get("diagnostic"),
        &format!("{path}.diagnostic"),
        reason.as_deref(),
    )?;
    if status == "ok" && expires_at.is_none() {
        return Err(failure(
            &format!("{path}.expires_at"),
            "is required when status is ok",
        ));
    }
    Ok(EvidenceComponent {
        status,
        observed_at,
        reason,
        expires_at,
        diagnostic,
    })
}

fn parse_checking(value: &Value, now: DateTime<Utc>) -> Result<Checking, ValidationError> {
    let vocabulary = &local_contract().brain_state;
    let object = value
        .as_object()
        .ok_or_else(|| failure("checking", "must be an object"))?;
    closed(object, &vocabulary.record_fields.checking, "checking.")?;
    Ok(Checking {
        started_at: required_time(object, "started_at", now)
            .map_err(|error| prefix_error(error, "checking"))?,
        expires_at: required_time(object, "expires_at", now)
            .map_err(|error| prefix_error(error, "checking"))?,
        checking_revision: required_u64(object, "checking_revision")
            .map_err(|error| prefix_error(error, "checking"))?,
        fingerprint_sha256: required_string(object, "fingerprint_sha256")
            .map_err(|error| prefix_error(error, "checking"))?,
        run_id: required_string(object, "run_id")
            .map_err(|error| prefix_error(error, "checking"))?,
        runtime_failure_marker_seen: optional_string(object, "runtime_failure_marker_seen")
            .map_err(|error| prefix_error(error, "checking"))?,
    })
}

fn parse_runtime_failure_marker(
    value: &Value,
    now: DateTime<Utc>,
) -> Result<RuntimeFailureMarker, ValidationError> {
    let vocabulary = &local_contract().brain_state;
    let object = value
        .as_object()
        .ok_or_else(|| failure("runtime_failure_marker", "must be an object"))?;
    closed(
        object,
        &vocabulary.record_fields.runtime_failure_marker,
        "runtime_failure_marker.",
    )?;
    Ok(RuntimeFailureMarker {
        marker_id: required_string(object, "marker_id")
            .map_err(|error| prefix_error(error, "runtime_failure_marker"))?,
        revision: required_u64(object, "revision")
            .map_err(|error| prefix_error(error, "runtime_failure_marker"))?,
        recorded_at: required_time(object, "recorded_at", now)
            .map_err(|error| prefix_error(error, "runtime_failure_marker"))?,
        reason: required_known(
            object,
            "reason_code",
            &vocabulary
                .evidence_reason_codes
                .values()
                .flatten()
                .cloned()
                .collect::<Vec<_>>(),
        )
        .map_err(|error| prefix_error(error, "runtime_failure_marker"))?,
    })
}

fn validate_reduction(record: &BrainStateRecord) -> Result<(), ValidationError> {
    let (aggregate, reason) = reduce_evidence(record, record.updated_at, true);
    if reason.as_deref() == Some("brain_record_invalid") {
        return Err(failure(
            "evidence",
            "missing lane-applicable evidence without higher-priority cause",
        ));
    }
    if record.aggregate_state != aggregate || record.reason_code.as_deref() != reason.as_deref() {
        return Err(failure(
            "aggregate_state",
            "record aggregate/reason does not match evidence",
        ));
    }
    Ok(())
}

pub(crate) fn reduce_evidence(
    record: &BrainStateRecord,
    now: DateTime<Utc>,
    refresh_permit_active: bool,
) -> (String, Option<String>) {
    reduce_evidence_with_runtime(record, now, refresh_permit_active, None)
}

pub(crate) fn reduce_evidence_with_runtime(
    record: &BrainStateRecord,
    now: DateTime<Utc>,
    refresh_permit_active: bool,
    runtime_reason: Option<&str>,
) -> (String, Option<String>) {
    let vocabulary = &local_contract().brain_state;
    let mut candidates: Vec<(u8, usize, String)> = Vec::new();
    if runtime_failure_marker_active(record)
        && let Some(marker) = &record.runtime_failure_marker
    {
        candidates.push((0, 0, marker.reason.clone()));
    }
    if let Some(checking) = &record.checking {
        let (priority, reason) = if checking.expires_at > now && refresh_permit_active {
            (1, "brain_check_in_progress")
        } else {
            (4, "brain_check_interrupted")
        };
        candidates.push((priority, 0, reason.to_owned()));
    }
    let applicable = vocabulary
        .lane_components
        .get(&record.active_lane)
        .cloned()
        .unwrap_or_default();
    for (index, component) in vocabulary.component_order.iter().enumerate() {
        if !applicable.iter().any(|candidate| candidate == component) {
            continue;
        }
        if component == "lane_prerequisites"
            && let Some(reason) = runtime_reason
        {
            let priority = match vocabulary
                .reason_to_aggregate
                .get(reason)
                .map(String::as_str)
            {
                Some("unhealthy") => 2,
                Some("blocked") => 3,
                _ => 4,
            };
            candidates.push((priority, index, reason.to_owned()));
            continue;
        }
        let Some(component_record) = record.evidence.get(component).and_then(Option::as_ref) else {
            candidates.push((4, index, "brain_record_invalid".to_owned()));
            continue;
        };
        let reason = match component_record.status.as_str() {
            "ok" if component_record
                .expires_at
                .is_some_and(|expires| expires <= now) =>
            {
                Some((4, "brain_record_stale"))
            }
            "ok" => None,
            "not_attempted" => None,
            "failed" => component_record.reason.as_deref().map(|reason| (2, reason)),
            "blocked" => component_record.reason.as_deref().map(|reason| (3, reason)),
            _ => Some((4, "brain_record_invalid")),
        };
        if let Some((priority, reason)) = reason {
            candidates.push((priority, index, reason.to_owned()));
        }
    }
    match candidates
        .into_iter()
        .min_by_key(|candidate| (candidate.0, candidate.1))
    {
        None => ("ready".to_owned(), None),
        Some((_, _, reason)) => (
            vocabulary
                .reason_to_aggregate
                .get(&reason)
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned()),
            Some(reason),
        ),
    }
}

fn runtime_failure_marker_active(record: &BrainStateRecord) -> bool {
    let Some(marker) = &record.runtime_failure_marker else {
        return false;
    };
    let Some(checking) = &record.checking else {
        return marker.revision == record.revision;
    };
    if checking.runtime_failure_marker_seen.as_deref() == Some(&marker.marker_id) {
        return false;
    }
    marker.revision >= checking.checking_revision || marker.recorded_at >= checking.started_at
}

fn diagnostic(
    value: Option<&Value>,
    path: &str,
    reason_code: Option<&str>,
) -> Result<BTreeMap<String, String>, ValidationError> {
    let object = match value {
        None | Some(Value::Null) => return Ok(BTreeMap::new()),
        Some(value) => value
            .as_object()
            .ok_or_else(|| failure(path, "must be an object"))?,
    };
    let allowed = local_contract()
        .brain_state
        .diagnostic_metadata_schemas
        .get(reason_code.unwrap_or(""));
    for (key, value) in object {
        let allowed_values = allowed
            .and_then(|schema| schema.get(key))
            .ok_or_else(|| failure(&format!("{path}.{key}"), "diagnostic key not allowed"))?;
        let value = value.as_str().ok_or_else(|| {
            failure(
                &format!("{path}.{key}"),
                "diagnostic value must be an enum string",
            )
        })?;
        if !allowed_values.iter().any(|candidate| candidate == value) {
            return Err(failure(
                &format!("{path}.{key}"),
                "diagnostic enum value not allowed",
            ));
        }
    }
    Ok(object
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().expect("checked").to_owned()))
        .collect())
}

fn closed(
    object: &Map<String, Value>,
    allowed: &[String],
    prefix: &str,
) -> Result<(), ValidationError> {
    if let Some(key) = object
        .keys()
        .filter(|key| !allowed.iter().any(|candidate| candidate == *key))
        .min()
    {
        return Err(failure(&format!("{prefix}{key}"), "unknown field"));
    }
    Ok(())
}

fn component_status_for_reason(reason: &str) -> Result<String, ValidationError> {
    match local_contract()
        .brain_state
        .reason_to_aggregate
        .get(reason)
        .map(String::as_str)
    {
        Some("blocked") => Ok("blocked".to_owned()),
        Some("unhealthy") => Ok("failed".to_owned()),
        Some("unknown") => Ok("unknown".to_owned()),
        _ => Err(failure(
            "reason_code",
            "checking reason is not valid component evidence",
        )),
    }
}

fn required_string(object: &Map<String, Value>, key: &str) -> Result<String, ValidationError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| failure(key, "must be a string"))
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, ValidationError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| failure(key, "must be a string or null")),
    }
}

fn required_u64(object: &Map<String, Value>, key: &str) -> Result<u64, ValidationError> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| failure(key, "must be a non-negative integer"))
}

fn required_known(
    object: &Map<String, Value>,
    key: &str,
    known: &[String],
) -> Result<String, ValidationError> {
    let value = required_string(object, key)?;
    if known.iter().any(|candidate| candidate == &value) {
        Ok(value)
    } else {
        Err(failure(key, "unknown value"))
    }
}

fn optional_known(
    object: &Map<String, Value>,
    key: &str,
    known: &[String],
) -> Result<Option<String>, ValidationError> {
    let Some(value) = optional_string(object, key)? else {
        return Ok(None);
    };
    if known.iter().any(|candidate| candidate == &value) {
        Ok(Some(value))
    } else {
        Err(failure(key, "unknown value"))
    }
}

fn required_time(
    object: &Map<String, Value>,
    key: &str,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, ValidationError> {
    let value = required_string(object, key)?;
    parse_time(&value, key, now)
}

fn optional_time(
    object: &Map<String, Value>,
    key: &str,
    _now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, ValidationError> {
    match optional_string(object, key)? {
        None => Ok(None),
        Some(value) => DateTime::parse_from_rfc3339(&value)
            .map(|value| Some(value.with_timezone(&Utc)))
            .map_err(|_| failure(key, "must be an ISO-8601 timestamp")),
    }
}

fn parse_time(
    value: &str,
    path: &str,
    _now: DateTime<Utc>,
) -> Result<DateTime<Utc>, ValidationError> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| failure(path, "must be an ISO-8601 timestamp"))?;
    Ok(parsed)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn prefix_error(error: ValidationError, prefix: &str) -> ValidationError {
    ValidationError {
        path: format!("{prefix}.{}", error.path),
        reason: error.reason,
    }
}

fn failure(path: &str, reason: &str) -> ValidationError {
    ValidationError {
        path: path.to_owned(),
        reason: reason.to_owned(),
    }
}
