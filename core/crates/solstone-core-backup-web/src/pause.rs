// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pause and resume backup without changing any other setting.
//!
//! A pause stops work that reaches the destination (backup runs, verification,
//! prune and media offload); the admission paths read the same flag. Restore
//! and turn off & delete stay available.

use axum::http::StatusCode;
use serde_json::Value;

use crate::{AdmitWrite, BackupWebDeps, admit_write, config, internal_error, response};

pub(crate) const PAUSED_MESSAGE: &str = "backup is paused. resume it first.";

/// Whether the owner paused backup, from an already merged backup section.
pub(crate) fn paused(backup: &serde_json::Map<String, Value>) -> bool {
    solstone_core_backup::backup_paused_at(backup).is_some()
}

pub(crate) fn paused_response() -> axum::response::Response {
    response::error(
        StatusCode::CONFLICT,
        PAUSED_MESSAGE,
        "backup_paused",
        "backup is paused",
    )
}

pub(crate) async fn pause(deps: BackupWebDeps) -> axum::response::Response {
    change(deps, true).await
}

pub(crate) async fn resume(deps: BackupWebDeps) -> axum::response::Response {
    change(deps, false).await
}

async fn change(deps: BackupWebDeps, pause: bool) -> axum::response::Response {
    let _guard = match admit_write(&deps.operations, AdmitWrite::IdleOnly) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let Ok(backup) = config::backup(&deps.journal_root) else {
        return internal_error();
    };
    if backup.get("enabled") != Some(&Value::Bool(true)) {
        return response::error(
            StatusCode::BAD_REQUEST,
            "that action isn't available in the current state.",
            "invalid_operation_for_state",
            "backup is off",
        );
    }
    let paused_at = pause.then(|| deps.clock.now_unix());
    if solstone_core_backup::set_backup_paused(&deps.journal_root, paused_at).is_err() {
        return internal_error();
    }
    crate::get_status(deps).await
}
