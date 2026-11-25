// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only discovery-cache access and identify-plan input normalization.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use solstone_core_entity::hold_entity_trust_lock;
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};
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
    let stream_layout = crate::segment_catalog::decode_stream_layout_value(
        object.get("stream_layout"),
    )
    .map_err(|_| DiscoveryMemberError::InvalidField {
        field: "stream_layout",
    })?;
    Ok(MemberProvenance {
        day: required_member_string(object, "day")?,
        stream_layout,
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

/// Remove stale references to `entity_id` from the discovery cluster caches
/// (`awareness/discovery_clusters.json` and `awareness/discovery_clusters.resolved.json`).
///
/// Leaves absent, unreadable, or unaffected caches untouched.
pub fn prune_discovery_clusters_for_entity(
    journal_root: &Path,
    entity_id: &str,
) -> Result<bool, String> {
    let _trust = hold_entity_trust_lock(journal_root).map_err(|error| error.to_string())?;
    let mut changed = false;

    let resolved_path = journal_root.join("awareness/discovery_clusters.resolved.json");
    if let Ok(bytes) = fs::read(&resolved_path)
        && let Ok(Value::Object(mut map)) = serde_json::from_slice::<Value>(&bytes)
    {
        let before = map.len();
        map.retain(|_, value| value.get("entity_id").and_then(Value::as_str) != Some(entity_id));
        if map.len() != before {
            let encoded = serde_json::to_vec_pretty(&Value::Object(map))
                .map_err(|error| error.to_string())?;
            atomic_replace(&resolved_path, &encoded, AtomicWriteOptions::default())
                .map_err(|error| error.to_string())?;
            changed = true;
        }
    }

    let cache_path = discovery_cache_path(journal_root);
    if let Ok(bytes) = fs::read(&cache_path)
        && let Ok(mut cache) = serde_json::from_slice::<Value>(&bytes)
    {
        let mut cache_changed = false;
        if let Some(root_obj) = cache.as_object_mut() {
            if let Some(resolved) = root_obj.get_mut("resolved").and_then(Value::as_object_mut) {
                let before = resolved.len();
                resolved.retain(|_, value| {
                    value.get("entity_id").and_then(Value::as_str) != Some(entity_id)
                });
                if resolved.len() != before {
                    cache_changed = true;
                }
            }
            if let Some(clusters) = root_obj.get_mut("clusters").and_then(Value::as_object_mut) {
                for cluster in clusters.values_mut() {
                    if prune_cluster_value_for_entity(cluster, entity_id) {
                        cache_changed = true;
                    }
                }
            }
        }
        if cache_changed {
            let encoded = serde_json::to_vec_pretty(&cache).map_err(|error| error.to_string())?;
            atomic_replace(&cache_path, &encoded, AtomicWriteOptions::default())
                .map_err(|error| error.to_string())?;
            changed = true;
        }
    }

    Ok(changed)
}

fn prune_cluster_value_for_entity(cluster: &mut Value, entity_id: &str) -> bool {
    const ENTITY_FIELDS: [&str; 4] = [
        "entity_id",
        "matched_entity_id",
        "candidate_entity_id",
        "suggested_entity_id",
    ];
    let mut changed = false;
    match cluster {
        Value::Object(map) => {
            for field in ENTITY_FIELDS {
                if map.get(field).and_then(Value::as_str) == Some(entity_id) {
                    map.remove(field);
                    changed = true;
                }
            }
            for list_field in ["near_matches", "candidates"] {
                if let Some(items) = map.get_mut(list_field).and_then(Value::as_array_mut) {
                    let before = items.len();
                    items.retain(|item| {
                        !ENTITY_FIELDS
                            .iter()
                            .any(|field| item.get(field).and_then(Value::as_str) == Some(entity_id))
                    });
                    if items.len() != before {
                        changed = true;
                    }
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                if let Some(obj) = item.as_object_mut() {
                    for field in ENTITY_FIELDS {
                        if obj.get(field).and_then(Value::as_str) == Some(entity_id) {
                            obj.remove(field);
                            changed = true;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_discovery_clusters_clears_stale_entity_references() {
        let root = tempfile::tempdir().unwrap();
        assert!(!prune_discovery_clusters_for_entity(root.path(), "alice").unwrap());

        let awareness = root.path().join("awareness");
        fs::create_dir_all(&awareness).unwrap();
        fs::write(
            awareness.join("discovery_clusters.resolved.json"),
            serde_json::to_vec_pretty(&json!({
                "0": {"entity_id": "alice", "label": "Alice", "ts": "2026-01-01T00:00:00Z"},
                "1": {"entity_id": "bob", "label": "Bob", "ts": "2026-01-01T00:00:00Z"}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            awareness.join("discovery_clusters.json"),
            serde_json::to_vec_pretty(&json!({
                "version": "2026-01-01T00:00:00Z",
                "clusters": {
                    "0": [
                        {"day": "20260101", "stream_layout": "direct", "stream": "_default", "segment_key": "100000_300", "source": "audio", "sentence_id": 1, "entity_id": "alice"},
                        {"day": "20260101", "stream_layout": "direct", "stream": "_default", "segment_key": "100000_300", "source": "audio", "sentence_id": 2}
                    ],
                    "2": {
                        "entity_id": "alice",
                        "near_matches": [
                            {"entity_id": "alice", "score": 0.9},
                            {"entity_id": "bob", "score": 0.8}
                        ]
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        assert!(prune_discovery_clusters_for_entity(root.path(), "alice").unwrap());

        let resolved: Value = serde_json::from_slice(
            &fs::read(awareness.join("discovery_clusters.resolved.json")).unwrap(),
        )
        .unwrap();
        assert!(resolved.get("0").is_none());
        assert_eq!(resolved["1"]["entity_id"], "bob");

        let cache = load_discovery_cache(root.path()).unwrap();
        assert!(cache["clusters"]["0"][0].get("entity_id").is_none());
        assert_eq!(cache["clusters"]["0"][0]["sentence_id"], 1);
        assert!(cache["clusters"]["2"].get("entity_id").is_none());
        assert_eq!(
            cache["clusters"]["2"]["near_matches"],
            json!([{"entity_id": "bob", "score": 0.8}])
        );

        assert!(!prune_discovery_clusters_for_entity(root.path(), "alice").unwrap());
    }
}
