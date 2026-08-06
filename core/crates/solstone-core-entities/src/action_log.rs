// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deferred journal-entity deletion action-log records.

use std::path::Path;

use serde_json::json;

pub(crate) fn pending(
    journal_root: &Path,
    entity_id: &str,
    pending_id: &str,
) -> Result<(), solstone_core_facets::AppendError> {
    solstone_core_facets::append_journal_action_log(
        journal_root,
        "app",
        "entities",
        "journal_entity_delete",
        json!({"entity_id":entity_id,"pending_id":pending_id,"phase":"pending"}),
    )
}

pub(crate) fn committed(
    journal_root: &Path,
    entity_id: &str,
    pending_id: &str,
    facets_deleted: &[String],
) -> Result<(), solstone_core_facets::AppendError> {
    solstone_core_facets::append_journal_action_log(
        journal_root,
        "app",
        "entities",
        "journal_entity_delete",
        json!({
            "entity_id":entity_id,
            "facets_deleted":facets_deleted,
            "pending_id":pending_id,
            "phase":"committed",
        }),
    )
}

pub(crate) fn cancelled(
    journal_root: &Path,
    pending_id: &str,
) -> Result<(), solstone_core_facets::AppendError> {
    solstone_core_facets::append_journal_action_log(
        journal_root,
        "app",
        "entities",
        "journal_entity_delete",
        json!({"pending_id":pending_id,"phase":"cancelled"}),
    )
}
