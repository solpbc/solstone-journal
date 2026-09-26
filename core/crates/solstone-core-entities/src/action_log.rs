// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deferred journal-entity deletion action-log records.

use std::path::Path;

use serde_json::{Value, json};

pub(crate) fn pending(
    journal_root: &Path,
    entity_id: &str,
    pending_id: &str,
) -> Result<(), solstone_core_facets::AppendError> {
    outcome(journal_root, entity_id, pending_id, "pending", json!({}))
}

/// One row for a phase of a delete: `pending`, then `committed`, `refused` or
/// `failed`, each with what the removal found.
pub(crate) fn outcome(
    journal_root: &Path,
    entity_id: &str,
    pending_id: &str,
    phase: &str,
    detail: Value,
) -> Result<(), solstone_core_facets::AppendError> {
    let mut params = serde_json::Map::new();
    params.insert("entity_id".into(), json!(entity_id));
    params.insert("pending_id".into(), json!(pending_id));
    params.insert("phase".into(), json!(phase));
    if let Value::Object(detail) = detail {
        params.extend(detail);
    }
    solstone_core_facets::append_action_log(
        journal_root,
        None,
        "app",
        "entities",
        "journal_entity_delete",
        Value::Object(params),
    )
}

pub(crate) fn cancelled(
    journal_root: &Path,
    pending_id: &str,
) -> Result<(), solstone_core_facets::AppendError> {
    solstone_core_facets::append_action_log(
        journal_root,
        None,
        "app",
        "entities",
        "journal_entity_delete",
        json!({"pending_id":pending_id,"phase":"cancelled"}),
    )
}
