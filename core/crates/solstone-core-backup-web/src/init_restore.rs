// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Restoring from a backup during setup: find the backups at a destination,
//! choose one, and restore it on the recorded path, so that a stop or a failed
//! check stays at setup with its retry.

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::response::Response;
use serde_json::{Map, Value, json};
use solstone_core_backup::{Destination, HostedBinding};
use solstone_core_backup_runtime::restore::{
    JournalBackup, MobileRestoreIdentity, MobileSelection, list_journal_backups,
};
use solstone_core_backup_runtime::restore_sweep::{abandon_init_restore, mark_init_restore};

use crate::first_restore::PausedRestore;
use crate::{BackupWebDeps, response, validation};

/// What encrypted backup needs to continue once the owner has chosen.
pub(crate) struct HostedChoice {
    pub binding: HostedBinding,
    pub recovery_key: String,
    pub backups: Vec<JournalBackup>,
}

pub(crate) type SharedHostedChoice = Arc<Mutex<Option<HostedChoice>>>;

/// Drains admitted work before the restore writes.
pub type Drain = Box<dyn FnMut() -> Result<Box<dyn PausedRestore>, String> + Send>;

/// One `/init/api/restore/<op>` request. `drain` is only used by `start` and
/// `hosted/choose`.
pub async fn api(deps: BackupWebDeps, op: String, body: Bytes, drain: Drain) -> Response {
    match op.as_str() {
        "find" => blocking(deps, move |deps| find(deps, &body)).await,
        "start" => blocking(deps, move |deps| start(deps, &body, drain)).await,
        "abandon" => blocking(deps, abandon).await,
        "status" => crate::status_response(&deps),
        "hosted/prepare" => {
            if let Err(error) = mark_init_restore(&deps.journal_root) {
                return failed(&error.to_string());
            }
            crate::restore_hosted_prepare_route(deps).await
        }
        "hosted/key" => crate::restore_hosted_key_route(deps, body).await,
        "hosted/arm" => crate::restore_hosted_arm_route(deps, body).await,
        "hosted/activate" => crate::restore_hosted_activate_route(deps, body).await,
        "hosted/cancel" => crate::restore_hosted_cancel_route(deps, body).await,
        "hosted/backups" => hosted_backups(&deps),
        "hosted/choose" => blocking(deps, move |deps| choose(deps, &body, drain)).await,
        _ => response::error(
            axum::http::StatusCode::NOT_FOUND,
            "that action isn't available.",
            "invalid_operation_for_state",
            "",
        ),
    }
}

async fn blocking(
    deps: BackupWebDeps,
    work: impl FnOnce(&BackupWebDeps) -> Response + Send + 'static,
) -> Response {
    match tokio::task::spawn_blocking(move || work(&deps)).await {
        Ok(response) => response,
        Err(error) => failed(&error.to_string()),
    }
}

fn failed(detail: &str) -> Response {
    refused("restore_failed", detail)
}

fn refused(reason: &str, detail: &str) -> Response {
    response::error(
        axum::http::StatusCode::CONFLICT,
        "the restore didn't complete",
        reason,
        detail,
    )
}

fn own_storage(body: &Bytes) -> Result<(Map<String, Value>, String), Response> {
    let payload = validation::object(body, response::missing)?;
    let key = validation::required_string(&payload, "recovery_key")?;
    validation::destination(&payload)?;
    Ok((payload, key))
}

fn find(deps: &BackupWebDeps, body: &Bytes) -> Response {
    let (payload, key) = match own_storage(body) {
        Ok(parsed) => parsed,
        Err(error) => return error,
    };
    let destination: Destination = crate::destination_from_payload(&payload);
    let tools = match crate::resolve_tools(deps) {
        Ok(tools) => tools,
        Err(error) => return refused(&error.to_string(), error.detail()),
    };
    let services = crate::services(deps, deps.runner.as_ref(), &tools);
    match list_journal_backups(&services, &destination, &key) {
        Ok(backups) => response::success(json!({ "backups": backups })),
        Err(reason) => refused(reason, ""),
    }
}

fn selection(payload: &Map<String, Value>) -> Result<MobileSelection, Response> {
    Ok(MobileSelection {
        snapshot_id: validation::required_string(payload, "snapshot_id")?,
        subtree: validation::required_string(payload, "subtree")?,
    })
}

fn outcome(result: Result<(), String>) -> Response {
    match result {
        Ok(()) => response::success(json!({ "accepted": true })),
        Err(reason) => refused(&reason, ""),
    }
}

fn start(deps: &BackupWebDeps, body: &Bytes, mut drain: Drain) -> Response {
    let (payload, key) = match own_storage(body) {
        Ok(parsed) => parsed,
        Err(error) => return error,
    };
    let chosen = match selection(&payload) {
        Ok(chosen) => chosen,
        Err(error) => return error,
    };
    if let Err(error) = mark_init_restore(&deps.journal_root) {
        return failed(&error.to_string());
    }
    outcome(crate::run_recorded(
        deps,
        crate::destination_from_payload(&payload),
        key,
        MobileRestoreIdentity::Byo,
        chosen,
        &mut drain,
        |terminal, _| terminal,
    ))
}

fn hosted_backups(deps: &BackupWebDeps) -> Response {
    match deps.init_hosted_choice.lock() {
        Ok(choice) => match choice.as_ref() {
            Some(choice) => response::success(json!({ "backups": choice.backups })),
            None => refused("invalid_operation_for_state", ""),
        },
        Err(_) => failed("the choice couldn't be read"),
    }
}

fn choose(deps: &BackupWebDeps, body: &Bytes, mut drain: Drain) -> Response {
    let payload = match validation::object(body, response::missing) {
        Ok(payload) => payload,
        Err(error) => return error,
    };
    let snapshot_id = match validation::required_string(&payload, "snapshot_id") {
        Ok(id) => id,
        Err(error) => return error,
    };
    let held = match deps.init_hosted_choice.lock() {
        Ok(mut choice) => choice.take(),
        Err(_) => return failed("the choice couldn't be read"),
    };
    let Some(held) = held else {
        return refused("invalid_operation_for_state", "");
    };
    let Some(subtree) = held
        .backups
        .iter()
        .find(|backup| backup.id == snapshot_id)
        .map(|backup| backup.subtree.clone())
    else {
        return refused("snapshot_catalog_invalid", "");
    };
    let credentials = match crate::fetch_hosted_credentials(
        deps.http.as_ref(),
        &held.binding,
        "maintenance",
        deps.version,
    ) {
        Ok(credentials) => credentials,
        Err(error) => return refused(error.reason_code, ""),
    };
    let destination = crate::operated_destination(&held.binding, &credentials);
    let binding = held.binding;
    outcome(crate::run_recorded(
        deps,
        destination,
        held.recovery_key,
        MobileRestoreIdentity::Hosted {
            account_id: binding.account_id,
            instance_id: binding.instance_id,
            bucket: binding.bucket,
            prefix: binding.prefix,
        },
        MobileSelection {
            snapshot_id,
            subtree,
        },
        &mut drain,
        |terminal, root| crate::first_restore::finish_hosted_mode(deps, root, terminal),
    ))
}

fn abandon(deps: &BackupWebDeps) -> Response {
    if let Ok(mut choice) = deps.init_hosted_choice.lock() {
        *choice = None;
    }
    match abandon_init_restore(&deps.journal_root) {
        Ok(()) => response::success(json!({ "abandoned": true })),
        Err(error) => failed(&error.to_string()),
    }
}
