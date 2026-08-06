// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-level action-log records (`config/actions/<day>.jsonl`).
//!
//! Mirrors the Python reference's `solstone.think.facets._write_action_log`
//! journal-level (`facet=None`) branch: a durable audit trail of user- and
//! agent-initiated actions that are not scoped to any single facet.

use std::path::Path;

use chrono::Utc;
use serde_json::{Value, json};
use solstone_core_journal_io::AppendError;

/// Append a journal-level action-log record to `config/actions/<day>.jsonl`.
pub fn append_journal_action_log(
    journal_root: &Path,
    source: &str,
    actor: &str,
    action: &str,
    params: Value,
) -> Result<(), AppendError> {
    let day = Utc::now().format("%Y%m%d").to_string();
    solstone_core_journal_io::append_jsonl(
        journal_root
            .join("config/actions")
            .join(format!("{day}.jsonl")),
        &json!({
            "timestamp": Utc::now().to_rfc3339(),
            "source": source,
            "actor": actor,
            "action": action,
            "params": params,
        }),
    )
}
