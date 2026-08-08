// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::whitespace::strip_python_whitespace;
use crate::{
    BodyHashError, BodyObject, BodyString, BodyValue, FieldState, IdentityField, ValueState,
    canonicalize,
};

/// Stable identity inputs for one source health record.
pub struct HealthRecordIdentity {
    pub source_family: BodyString,
    pub record_type: BodyString,
    pub start_time: BodyString,
    pub end_time: FieldState<BodyString>,
    pub source_record_id: FieldState<BodyString>,
    pub source_name: FieldState<BodyString>,
    pub unit: FieldState<BodyString>,
    pub metadata: FieldState<BodyObject>,
    pub value: ValueState,
}

/// Hashes a health record's value payload without source identity fields.
pub fn health_value_hash(
    unit: &FieldState<BodyString>,
    metadata: &FieldState<BodyObject>,
    value: &ValueState,
) -> Result<String, BodyHashError> {
    let payload = BodyValue::Object(BTreeMap::from([
        (body_string("metadata"), metadata_value(metadata)),
        (
            body_string("unit"),
            BodyValue::String(optional_string(unit)),
        ),
        (body_string("value"), value_value(value)),
    ]));
    let canonical = canonicalize(&payload).map_err(|_| BodyHashError::ValueTooDeep)?;
    Ok(health_hash(&["health-value", &canonical]))
}

/// Returns the importer-owned dedupe key for a health record.
pub fn health_record_dedupe_key(identity: &HealthRecordIdentity) -> Result<String, BodyHashError> {
    let source_family = strip_python_whitespace(&identity.source_family);
    if source_family.code_points().is_empty()
        || source_family
            .code_points()
            .iter()
            .any(|code_point| *code_point > 0x7f)
    {
        return Err(BodyHashError::InvalidIdentity(IdentityField::SourceFamily));
    }
    let source_family = ascii_lowercase(&source_family);

    let record_type = strip_python_whitespace(&identity.record_type);
    if record_type.code_points().is_empty() {
        return Err(BodyHashError::InvalidIdentity(IdentityField::RecordType));
    }

    let start_time = strip_python_whitespace(&identity.start_time);
    if start_time.code_points().is_empty() {
        return Err(BodyHashError::InvalidIdentity(IdentityField::StartTime));
    }

    let source_record_id = strip_optional_string(&identity.source_record_id);
    if !source_record_id.code_points().is_empty() {
        return canonical_hash(
            "health-record/source-id",
            BodyValue::Object(BTreeMap::from([
                (body_string("record_type"), BodyValue::String(record_type)),
                (
                    body_string("source_family"),
                    BodyValue::String(source_family),
                ),
                (
                    body_string("source_record_id"),
                    BodyValue::String(source_record_id),
                ),
            ])),
        );
    }

    let value_hash = health_value_hash(&identity.unit, &identity.metadata, &identity.value)?;
    let end_time = fallback_string(&identity.end_time, &start_time);
    let source_name = optional_string(&identity.source_name);
    canonical_hash(
        "health-record/composite",
        BodyValue::Object(BTreeMap::from([
            (body_string("end_time"), BodyValue::String(end_time)),
            (body_string("record_type"), BodyValue::String(record_type)),
            (
                body_string("source_family"),
                BodyValue::String(source_family),
            ),
            (body_string("source_name"), BodyValue::String(source_name)),
            (body_string("start_time"), BodyValue::String(start_time)),
            (
                body_string("value_hash"),
                BodyValue::String(body_string(&value_hash)),
            ),
        ])),
    )
}

fn canonical_hash(namespace: &str, value: BodyValue) -> Result<String, BodyHashError> {
    let canonical = canonicalize(&value).map_err(|_| BodyHashError::ValueTooDeep)?;
    Ok(health_hash(&[namespace, &canonical]))
}

fn health_hash(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.as_bytes());
        digest.update([0x1f]);
    }
    format!("sha256:{:x}", digest.finalize())
}

fn body_string(value: &str) -> BodyString {
    BodyString::from_code_points(value.bytes().map(u32::from).collect())
        .expect("ASCII text is a valid body string")
}

fn ascii_lowercase(value: &BodyString) -> BodyString {
    BodyString::from_code_points(
        value
            .code_points()
            .iter()
            .map(|code_point| match *code_point {
                0x41..=0x5a => *code_point + 32,
                _ => *code_point,
            })
            .collect(),
    )
    .expect("ASCII lowercasing preserves valid body-string code points")
}

fn optional_string(value: &FieldState<BodyString>) -> BodyString {
    match value {
        FieldState::Present(value) if !value.code_points().is_empty() => value.clone(),
        FieldState::Absent | FieldState::Null | FieldState::Present(_) => body_string(""),
    }
}

fn strip_optional_string(value: &FieldState<BodyString>) -> BodyString {
    match value {
        FieldState::Present(value) => strip_python_whitespace(value),
        FieldState::Absent | FieldState::Null => body_string(""),
    }
}

fn fallback_string(value: &FieldState<BodyString>, fallback: &BodyString) -> BodyString {
    match value {
        FieldState::Present(value) if !value.code_points().is_empty() => value.clone(),
        FieldState::Absent | FieldState::Null | FieldState::Present(_) => fallback.clone(),
    }
}

fn metadata_value(value: &FieldState<BodyObject>) -> BodyValue {
    match value {
        FieldState::Present(value) if !value.is_empty() => BodyValue::Object(value.clone()),
        FieldState::Absent | FieldState::Null | FieldState::Present(_) => {
            BodyValue::Object(BTreeMap::new())
        }
    }
}

fn value_value(value: &ValueState) -> BodyValue {
    match value {
        ValueState::Absent => BodyValue::Null,
        ValueState::Present(value) => value.clone(),
    }
}
