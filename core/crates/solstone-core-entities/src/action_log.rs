// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deferred journal-entity deletion action-log records.

use std::path::Path;

use chrono::Utc;
use serde_json::json;

fn append(journal_root: &Path, params: serde_json::Value) -> Result<(), solstone_core_journal_io::AppendError> {
    let day = Utc::now().format("%Y%m%d").to_string();
    solstone_core_journal_io::append_jsonl(
        journal_root.join("config/actions").join(format!("{day}.jsonl")),
        &json!({
            "timestamp": Utc::now().to_rfc3339(),
            "source": "app",
            "actor": "entities",
            "action": "journal_entity_delete",
            "params": params,
        }),
    )
}

pub(crate) fn pending(
    journal_root: &Path,
    entity_id: &str,
    pending_id: &str,
) -> Result<(), solstone_core_journal_io::AppendError> {
    append(
        journal_root,
        json!({"entity_id":entity_id,"pending_id":pending_id,"phase":"pending"}),
    )
}

pub(crate) fn committed(
    journal_root: &Path,
    entity_id: &str,
    pending_id: &str,
    facets_deleted: &[String],
) -> Result<(), solstone_core_journal_io::AppendError> {
    append(
        journal_root,
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
) -> Result<(), solstone_core_journal_io::AppendError> {
    append(journal_root, json!({"pending_id":pending_id,"phase":"cancelled"}))
}
