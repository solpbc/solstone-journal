// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only discovery-cache access and identify-plan input normalization.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::identify_operations::MemberProvenance;

/// Return the discovery cluster cache path without creating its parent directory.
#[must_use]
pub fn discovery_cache_path(journal_root: &Path) -> PathBuf {
    journal_root.join("awareness/discovery_clusters.json")
}

/// Load the discovery cache if it is present and structurally valid.
///
/// This intentionally treats a missing, unreadable, malformed, or incomplete cache
/// as unavailable rather than as an error. Publication is owned by `discovery_scan`.
#[must_use]
pub fn load_discovery_cache(journal_root: &Path) -> Option<Value> {
    let path = discovery_cache_path(journal_root);
    let data: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let object = data.as_object()?;
    object.get("clusters")?.as_object()?;
    Some(data)
}

/// Failure to normalize a discovery-cache cluster member.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DiscoveryMemberError {
    #[error("discovery cluster member must be an object")]
    NotObject,
    #[error("discovery cluster member is missing or has invalid {field}")]
    InvalidField { field: &'static str },
}

/// Convert one cache member into its canonical provenance tuple representation.
pub fn member_tuple(member: &Value) -> Result<MemberProvenance, DiscoveryMemberError> {
    let object = member.as_object().ok_or(DiscoveryMemberError::NotObject)?;
    Ok(MemberProvenance {
        day: required_member_string(object, "day")?,
        stream: required_member_string(object, "stream")?,
        segment_key: required_member_string(object, "segment_key")?,
        source: required_member_string(object, "source")?,
        sentence_id: object.get("sentence_id").and_then(Value::as_i64).ok_or(
            DiscoveryMemberError::InvalidField {
                field: "sentence_id",
            },
        )?,
    })
}

/// Canonicalize raw cache members using the durable provenance tuple order.
pub fn canonical_members(
    cluster_members: &[Value],
) -> Result<Vec<MemberProvenance>, DiscoveryMemberError> {
    let mut members = cluster_members
        .iter()
        .map(member_tuple)
        .collect::<Result<Vec<_>, _>>()?;
    members.sort();
    Ok(members)
}

/// Validation failure for `reviewed_near_match_entity_ids`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReviewedNearMatchIdsError {
    #[error("reviewed_near_match_entity_ids must be a list")]
    NotList,
    #[error("reviewed_near_match_entity_ids must contain strings")]
    InvalidItem,
    #[error("reviewed_near_match_entity_ids must be unique")]
    Duplicate { entity_id: String },
}

impl ReviewedNearMatchIdsError {
    /// Return the Python-compatible invalid-request response shape.
    #[must_use]
    pub fn invalid_request_response(&self) -> Value {
        match self {
            Self::NotList => json!({
                "status": "invalid_request",
                "error": "reviewed_near_match_entity_ids must be a list",
            }),
            Self::InvalidItem => json!({
                "status": "invalid_request",
                "error": "reviewed_near_match_entity_ids must contain strings",
            }),
            Self::Duplicate { entity_id } => json!({
                "status": "invalid_request",
                "error": "reviewed_near_match_entity_ids must be unique",
                "invalid_reviewed_near_match_entity_ids": [
                    {"entity_id": entity_id, "reason": "duplicate"}
                ],
            }),
        }
    }
}

/// Validate, trim, and preserve the caller's reviewed near-match IDs.
pub fn normalize_reviewed_near_match_ids(
    value: Option<&Value>,
) -> Result<Vec<String>, ReviewedNearMatchIdsError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let items = value.as_array().ok_or(ReviewedNearMatchIdsError::NotList)?;
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let entity_id = item
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(ReviewedNearMatchIdsError::InvalidItem)?;
        if result.iter().any(|existing| existing == entity_id) {
            return Err(ReviewedNearMatchIdsError::Duplicate {
                entity_id: entity_id.to_owned(),
            });
        }
        result.push(entity_id.to_owned());
    }
    Ok(result)
}

fn required_member_string(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<String, DiscoveryMemberError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(DiscoveryMemberError::InvalidField { field })
}
