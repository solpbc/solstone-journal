// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native backup Convey routes. Engine POSTs start restic/rclone work and
//! report a page-native operation on GET status.

#![allow(clippy::result_large_err)] // Route handlers return the exact HTTP refusal envelope on the Err path.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    body::Bytes,
    routing::{get, post},
};
use chrono::{Datelike, Utc};
use serde_json::{Map, Value, json};
use solstone_core_artifact_download::{ByteDownload, UreqByteDownload};
use solstone_core_backup::{
    Destination, HostedBinding, generate_daily_key, get_destination, get_keys, load_hosted_binding,
    save_hosted_binding, set_destination, set_enabled, set_mode,
};
use solstone_core_backup_runtime::hosted_runtime::fetch_hosted_credentials;
use solstone_core_backup_runtime::repo::RepoError;
use solstone_core_backup_runtime::{
    BackupServices, Clock, HttpTransport, JournalMaintenance, NativeJournalMaintenance,
    SystemToolRunner, ToolInstallDirs, ToolRunner, UreqHttpTransport, init_repository,
    operated_destination, reason_for_returncode, resolve_operational_tools, restore_journal,
    rotate_recovery_key, teardown_backup, validate_destination,
};
use solstone_core_offload::{restore_all_offload, restore_offload_day};

mod assets;
mod callosum;
mod config;
mod handoff_poll;
mod keys;
mod measurement;
mod operation;
mod response;
mod status;
mod validation;

use measurement::SharedMeasurementCache;
use operation::{SharedOperationSlot, Terminal};

#[derive(Clone)]
pub struct BackupWebDeps {
    pub journal_root: PathBuf,
    pub cache: SharedMeasurementCache,
    pub operations: SharedOperationSlot,
    pub runner: Arc<dyn ToolRunner + Send + Sync>,
    pub http: Arc<dyn HttpTransport + Send + Sync>,
    pub downloader: Arc<dyn ByteDownload + Send + Sync>,
    pub clock: Arc<dyn Clock + Send + Sync>,
    pub journal_maintenance: Arc<dyn JournalMaintenance + Send + Sync>,
    pub restic_install_dir: Option<PathBuf>,
    pub rclone_install_dir: Option<PathBuf>,
    pub portal_base: String,
    pub version: &'static str,
}

struct ProductionClock;

impl Clock for ProductionClock {
    fn now_unix(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }

    fn iso_week(&self) -> u8 {
        Utc::now().iso_week().week() as u8
    }
}

impl BackupWebDeps {
    fn production(journal_root: PathBuf, cache: SharedMeasurementCache) -> Self {
        Self {
            journal_root,
            cache,
            operations: operation::new_slot(),
            runner: Arc::new(SystemToolRunner),
            http: Arc::new(UreqHttpTransport),
            downloader: Arc::new(UreqByteDownload),
            clock: Arc::new(ProductionClock),
            journal_maintenance: Arc::new(NativeJournalMaintenance),
            restic_install_dir: None,
            rclone_install_dir: None,
            portal_base: "https://services.solstone.app".into(),
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn install_dirs(&self) -> ToolInstallDirs<'_> {
        ToolInstallDirs {
            restic: self.restic_install_dir.as_deref(),
            rclone: self.rclone_install_dir.as_deref(),
        }
    }
}

pub fn routes(journal_root: PathBuf) -> Router {
    let cache = measurement::new(&journal_root);
    routes_with_deps(BackupWebDeps::production(journal_root, cache))
}

#[cfg(test)]
fn routes_with_cache(journal_root: PathBuf, cache: SharedMeasurementCache) -> Router {
    routes_with_deps(BackupWebDeps::production(journal_root, cache))
}

pub(crate) fn routes_with_deps(deps: BackupWebDeps) -> Router {
    let status = deps.clone();
    let offload = deps.clone();
    let config = deps.clone();
    let enable_offload = deps.clone();
    let disable_offload = deps.clone();
    let keys = deps.clone();
    let reveal = deps.clone();
    let confirm = deps.clone();
    let retention_deps = deps.clone();
    let backup_now_deps = deps.clone();
    let enable = deps.clone();
    let enable_hosted = deps.clone();
    let destination = deps.clone();
    let rotate = deps.clone();
    let teardown = deps.clone();
    let restore = deps.clone();
    let restore_hosted = deps.clone();
    let offload_restore = deps.clone();
    let handoff = deps;
    Router::new()
        .route("/app/backup/", get(assets::shell))
        .route("/app/backup/workspace", get(assets::workspace))
        .route("/app/backup/background", get(assets::background))
        .route("/app/backup/static/{name}", get(assets::static_asset))
        .route(
            "/app/backup/status",
            get(move || get_status(status.clone())),
        )
        .route(
            "/app/backup/offload/status",
            get(move || get_offload(offload.clone())),
        )
        .route(
            "/app/backup/offload/config",
            post(move |body| offload_config(config.clone(), body)),
        )
        .route(
            "/app/backup/offload/enable",
            post(move || offload_enable(enable_offload.clone())),
        )
        .route(
            "/app/backup/offload/disable",
            post(move || offload_disable(disable_offload.clone())),
        )
        .route(
            "/app/backup/keys/generate",
            post(move || generate_keys(keys.clone())),
        )
        .route(
            "/app/backup/recovery-key/reveal",
            post(move || reveal_key(reveal.clone())),
        )
        .route(
            "/app/backup/confirm",
            post(move |body| confirm_key(confirm.clone(), body)),
        )
        .route(
            "/app/backup/retention",
            post(move |body| retention(retention_deps.clone(), body)),
        )
        .route(
            "/app/backup/backup-now",
            post(move || backup_now(backup_now_deps.clone())),
        )
        .route(
            "/app/backup/enable",
            post(move || enable_backup(enable.clone())),
        )
        .route(
            "/app/backup/enable-hosted",
            post(move || enable_hosted_backup(enable_hosted.clone())),
        )
        .route(
            "/app/backup/destination",
            post(move |body| set_backup_destination(destination.clone(), body)),
        )
        .route(
            "/app/backup/recovery-key/rotate",
            post(move || rotate_key(rotate.clone())),
        )
        .route(
            "/app/backup/teardown",
            post(move || teardown_route(teardown.clone())),
        )
        .route(
            "/app/backup/restore",
            post(move |body| restore_route(restore.clone(), body)),
        )
        .route(
            "/app/backup/restore-hosted",
            post(move |body| restore_hosted_route(restore_hosted.clone(), body)),
        )
        .route(
            "/app/backup/offload/restore",
            post(move |body| offload_restore_route(offload_restore.clone(), body)),
        )
        .route(
            "/app/backup/handoff",
            post(move |body| handoff_route(handoff.clone(), body)),
        )
}

fn status_response(deps: &BackupWebDeps) -> axum::response::Response {
    status::status(&deps.journal_root, &deps.operations)
        .map(response::success)
        .unwrap_or_else(|_| internal_error())
}

fn offload_response(deps: &BackupWebDeps) -> axum::response::Response {
    status::offload(&deps.journal_root, &deps.cache, &deps.operations)
        .map(response::success)
        .unwrap_or_else(|_| internal_error())
}

async fn get_status(deps: BackupWebDeps) -> axum::response::Response {
    status_response(&deps)
}

async fn get_offload(deps: BackupWebDeps) -> axum::response::Response {
    offload_response(&deps)
}

async fn generate_keys(deps: BackupWebDeps) -> axum::response::Response {
    let generated_daily = match keys::generated_key() {
        Ok(key) => key,
        Err(_) => return internal_error(),
    };
    let generated_recovery = match keys::generated_key() {
        Ok(key) => key,
        Err(_) => return internal_error(),
    };
    let outcome = config::mutate(&deps.journal_root, |backup| {
        let daily = backup.get("daily_key").cloned().unwrap_or(Value::Null);
        let recovery = backup.get("recovery_key").cloned().unwrap_or(Value::Null);
        let mut changed = false;
        let daily = if daily.is_null() {
            changed = true;
            Value::String(generated_daily)
        } else {
            daily
        };
        let recovery = if recovery.is_null() {
            changed = true;
            Value::String(generated_recovery)
        } else {
            recovery
        };
        backup.insert("daily_key".to_owned(), daily);
        backup.insert("recovery_key".to_owned(), recovery.clone());
        (changed, recovery.as_str().unwrap_or_default().to_owned())
    });
    match outcome.and_then(|recovery| keys::format(&recovery).map_err(|_| ())) {
        Ok(display) => response::success(json!({"success":true,"recovery_key_display":display})),
        Err(_) => internal_error(),
    }
}

fn internal_error() -> axum::response::Response {
    response::error(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "I couldn't complete that request.",
        "internal_error",
        "",
    )
}

async fn reveal_key(deps: BackupWebDeps) -> axum::response::Response {
    match config::backup(&deps.journal_root).and_then(|config| keys::keys(&config)) {
        Ok(Some((_, recovery))) => match keys::format(&recovery) {
            Ok(display) => {
                response::success(json!({"success":true,"recovery_key_display":display}))
            }
            Err(_) => internal_error(),
        },
        Ok(None) => response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't take that action in the current state.",
            "invalid_operation_for_state",
            "no recovery key yet",
        ),
        Err(_) => internal_error(),
    }
}

async fn confirm_key(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::missing) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let entered = match validation::required_string(&payload, "recovery_key") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let recovery = match config::backup(&deps.journal_root).and_then(|config| keys::keys(&config)) {
        Ok(Some((_, key))) => key,
        Ok(None) => {
            return response::error(
                axum::http::StatusCode::BAD_REQUEST,
                "I couldn't take that action in the current state.",
                "invalid_operation_for_state",
                "no recovery key yet",
            );
        }
        Err(_) => return internal_error(),
    };
    if keys::parse(&entered).ok().as_deref() != Some(&recovery) {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't confirm that — it didn't match your recovery key.",
            "recovery_key_mismatch",
            "",
        );
    }
    if config::mutate(&deps.journal_root, |backup| {
        let changed = backup.get("confirmed_recovery_key") != Some(&Value::Bool(true));
        backup.insert("confirmed_recovery_key".to_owned(), Value::Bool(true));
        (changed, ())
    })
    .is_err()
    {
        return internal_error();
    }
    get_status(deps).await
}

async fn retention(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::invalid_config) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let next = match validation::retention(&payload) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if config::mutate(&deps.journal_root, |backup| {
        let value = Value::Object(next);
        let changed = backup.get("retention") != Some(&value);
        backup.insert("retention".to_owned(), value);
        (changed, ())
    })
    .is_err()
    {
        return internal_error();
    }
    get_status(deps).await
}

async fn offload_config(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::invalid_config) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let (budget, floor) = match validation::offload(&payload) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if config::mutate(&deps.journal_root, |backup| {
        let enabled = backup
            .get("offload")
            .and_then(Value::as_object)
            .and_then(|value| value.get("enabled"))
            .cloned()
            .unwrap_or(Value::Bool(false));
        let value = json!({"enabled":enabled,"budget_bytes":budget,"floor_bytes":floor});
        let changed = backup.get("offload") != Some(&value);
        backup.insert("offload".to_owned(), value);
        (changed, ())
    })
    .is_err()
    {
        return internal_error();
    }
    measurement::invalidate(&deps.cache);
    get_offload(deps).await
}

async fn offload_enable(deps: BackupWebDeps) -> axum::response::Response {
    let backup = match config::backup(&deps.journal_root) {
        Ok(value) => value,
        Err(_) => return internal_error(),
    };
    if backup.get("enabled") != Some(&Value::Bool(true)) {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't take that action in the current state.",
            "invalid_operation_for_state",
            "backup is disabled",
        );
    }
    if backup.get("confirmed_recovery_key") != Some(&Value::Bool(true)) {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't turn on backup until you confirm your recovery key.",
            "backup_not_confirmed",
            "",
        );
    }
    if !matches!(keys::keys(&backup), Ok(Some(_))) {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't take that action in the current state.",
            "invalid_operation_for_state",
            "backup keys are missing",
        );
    }
    let defaults = measurement::snapshot(&deps.cache);
    if config::mutate(&deps.journal_root, |backup| { let old = backup.get("offload").and_then(Value::as_object); let value = json!({"enabled":true,"budget_bytes":old.and_then(|item| item.get("budget_bytes")).cloned().filter(|value| !value.is_null()).unwrap_or_else(|| defaults["suggested_defaults"]["budget_bytes"].clone()),"floor_bytes":old.and_then(|item| item.get("floor_bytes")).cloned().filter(|value| !value.is_null()).unwrap_or_else(|| defaults["suggested_defaults"]["floor_bytes"].clone())}); let changed = backup.get("offload") != Some(&value); backup.insert("offload".to_owned(), value); (changed, ()) }).is_err() { return response::invalid_config(""); }
    if backup["last_verification"]["status"].is_null() {
        let _ = callosum::request(&deps.journal_root, "backup:verify");
    }
    measurement::invalidate(&deps.cache);
    get_offload(deps).await
}

async fn offload_disable(deps: BackupWebDeps) -> axum::response::Response {
    if config::mutate(&deps.journal_root, |backup| { let old = backup.get("offload").and_then(Value::as_object); let value = json!({"enabled":false,"budget_bytes":old.and_then(|item| item.get("budget_bytes")).cloned().unwrap_or(Value::Null),"floor_bytes":old.and_then(|item| item.get("floor_bytes")).cloned().unwrap_or(Value::Null)}); let changed = backup.get("offload") != Some(&value); backup.insert("offload".to_owned(), value); (changed, ()) }).is_err() { return response::invalid_config(""); }
    measurement::invalidate(&deps.cache);
    get_offload(deps).await
}

async fn backup_now(deps: BackupWebDeps) -> axum::response::Response {
    if !callosum::request(&deps.journal_root, "backup:run") {
        return response::error(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "I couldn't start a backup because your journal's background service isn't running. Start it, then try again.",
            "backup_unavailable",
            "",
        );
    }
    get_status(deps).await
}

fn enable_preconditions(root: &Path) -> Result<(), axum::response::Response> {
    let backup = config::backup(root).map_err(|_| internal_error())?;
    let destination = backup.get("destination").and_then(Value::as_object);
    if destination
        .and_then(|value| value.get("repository"))
        .is_none()
    {
        return Err(response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't take that action in the current state.",
            "invalid_operation_for_state",
            "configure a destination first",
        ));
    }
    if backup.get("confirmed_recovery_key") != Some(&Value::Bool(true)) {
        return Err(response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't turn on backup until you confirm your recovery key.",
            "backup_not_confirmed",
            "",
        ));
    }
    if !matches!(keys::keys(&backup), Ok(Some(_))) {
        return Err(response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't take that action in the current state.",
            "invalid_operation_for_state",
            "no recovery key yet",
        ));
    }
    Ok(())
}

fn hosted_preconditions(root: &Path) -> Result<(), axum::response::Response> {
    let backup = config::backup(root).map_err(|_| internal_error())?;
    if backup.get("confirmed_recovery_key") != Some(&Value::Bool(true)) {
        return Err(response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't turn on backup until you confirm your recovery key.",
            "backup_not_confirmed",
            "",
        ));
    }
    if !matches!(keys::keys(&backup), Ok(Some(_))) {
        return Err(response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't take that action in the current state.",
            "invalid_operation_for_state",
            "no recovery key yet",
        ));
    }
    Ok(())
}

fn init_and_enable(
    runner: &dyn ToolRunner,
    destination: &Destination,
    daily_key: &str,
    recovery_key: &str,
    restic_path: &Path,
) -> Result<(), String> {
    init_repository(
        runner,
        destination,
        daily_key,
        recovery_key,
        restic_path,
        None,
    )
    .map_err(|error| match error {
        RepoError::Key(key) => reason_for_returncode(key.returncode).to_owned(),
        _ => "failed".to_owned(),
    })
}

fn resolve_tools(
    deps: &BackupWebDeps,
) -> Result<solstone_core_backup_runtime::ResolvedTools, String> {
    resolve_operational_tools(
        deps.runner.as_ref(),
        deps.downloader.as_ref(),
        &deps.journal_root,
        false,
        deps.install_dirs(),
    )
}

fn services<'a>(
    deps: &'a BackupWebDeps,
    tools: &'a solstone_core_backup_runtime::ResolvedTools,
) -> BackupServices<'a> {
    BackupServices {
        runner: deps.runner.as_ref(),
        http: deps.http.as_ref(),
        clock: deps.clock.as_ref(),
        restic_path: Some(&tools.restic_path),
        rclone_path: tools.rclone_path.as_deref(),
        version: deps.version,
        journal_maintenance: deps.journal_maintenance.as_ref(),
    }
}

fn map_restore(result: solstone_core_backup_runtime::RestoreResult) -> Terminal {
    match result.status.as_str() {
        "ok" => Terminal {
            phase: "done".into(),
            reason_code: None,
        },
        "degraded" => Terminal {
            phase: "degraded".into(),
            reason_code: result.reason_code,
        },
        _ => Terminal {
            phase: "error".into(),
            reason_code: result.reason_code.or_else(|| Some("failed".into())),
        },
    }
}

fn map_offload_restore(result: solstone_core_offload::RestoreResult) -> Terminal {
    match result.status.as_str() {
        "ok" | "no_op" => Terminal {
            phase: "done".into(),
            reason_code: result.reason,
        },
        "refused" => Terminal {
            phase: "refused".into(),
            reason_code: result.reason,
        },
        "degraded" => Terminal {
            phase: "degraded".into(),
            reason_code: result.reason,
        },
        _ => Terminal {
            phase: "error".into(),
            reason_code: result.reason.or_else(|| Some("failed".into())),
        },
    }
}

async fn enable_backup(deps: BackupWebDeps) -> axum::response::Response {
    if let Err(error) = enable_preconditions(&deps.journal_root) {
        return error;
    }
    let started = match operation::begin(&deps.operations, "enable", None, None, None) {
        Ok(started) => started,
        Err(error) => return error,
    };
    let worker = deps.clone();
    let generation = started.generation;
    operation::spawn_worker(deps.operations.clone(), generation, move || {
        let tools = match resolve_tools(&worker) {
            Ok(tools) => tools,
            Err(reason) => {
                return Terminal {
                    phase: "error".into(),
                    reason_code: Some(reason),
                };
            }
        };
        let destination = match get_destination(&worker.journal_root) {
            Ok(Some(destination)) => destination,
            _ => {
                return Terminal {
                    phase: "error".into(),
                    reason_code: Some("invalid_operation_for_state".into()),
                };
            }
        };
        let keys = match get_keys(&worker.journal_root) {
            Ok(Some(keys)) => keys,
            _ => {
                return Terminal {
                    phase: "error".into(),
                    reason_code: Some("invalid_operation_for_state".into()),
                };
            }
        };
        if let Err(reason) = init_and_enable(
            worker.runner.as_ref(),
            &destination,
            &keys.daily_key,
            &keys.recovery_key,
            &tools.restic_path,
        ) {
            return Terminal {
                phase: "error".into(),
                reason_code: Some(reason),
            };
        }
        if set_enabled(&worker.journal_root, true).is_err() {
            return Terminal {
                phase: "error".into(),
                reason_code: Some("failed".into()),
            };
        }
        Terminal {
            phase: "done".into(),
            reason_code: None,
        }
    });
    status_response(&deps)
}

fn mint_portal(deps: &BackupWebDeps) -> Result<(String, String, String), axum::response::Response> {
    let nonce = solstone_core_handoff_nonce::mint_nonce().map_err(|_| internal_error())?;
    let instance = operation::mint_hex().map_err(|_| internal_error())?;
    let url = operation::portal_url(&deps.portal_base, &nonce, &instance);
    Ok((nonce, instance, url))
}

async fn enable_hosted_backup(deps: BackupWebDeps) -> axum::response::Response {
    if let Err(error) = hosted_preconditions(&deps.journal_root) {
        return error;
    }
    let (nonce, _instance, url) = match mint_portal(&deps) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let started = match operation::begin(
        &deps.operations,
        "enable_hosted",
        Some(url),
        Some(nonce.clone()),
        None,
    ) {
        Ok(started) => started,
        Err(error) => return error,
    };
    handoff_poll::spawn(deps.clone(), nonce, started.generation);
    status_response(&deps)
}

fn destination_from_payload(payload: &Map<String, Value>) -> Destination {
    let repository = payload
        .get("repository")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    let backend = payload
        .get("backend")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    let source = payload
        .get("credentials")
        .and_then(Value::as_object)
        .unwrap_or(payload);
    let required = if backend == "s3" {
        ["access_key_id", "secret_access_key"].as_slice()
    } else {
        ["account_id", "account_key"].as_slice()
    };
    let credentials = required
        .iter()
        .filter_map(|key| {
            source
                .get(*key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| ((*key).to_owned(), Value::String(value.to_owned())))
        })
        .collect();
    Destination {
        repository,
        backend,
        credentials,
    }
}

async fn set_backup_destination(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::missing) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Err(error) = validation::destination(&payload) {
        return error;
    }
    // Destination is synchronous (page reads destination_status on the POST) so
    // it must not occupy the operation slot, but it still refuses backup_busy.
    if operation::is_busy(&deps.operations) {
        return operation::busy_response();
    }
    let destination = destination_from_payload(&payload);
    if set_destination(&deps.journal_root, &destination).is_err() {
        return internal_error();
    }
    let password = match get_keys(&deps.journal_root) {
        Ok(Some(keys)) => keys.daily_key,
        Ok(None) => match generate_daily_key() {
            Ok(key) => key,
            Err(_) => return internal_error(),
        },
        Err(_) => return internal_error(),
    };
    let tools = match resolve_tools(&deps) {
        Ok(tools) => tools,
        Err(reason) => {
            return destination_status_response(
                &deps,
                false,
                false,
                &reason,
                "could not prepare the backup tool",
            );
        }
    };
    match validate_destination(
        deps.runner.as_ref(),
        &destination,
        &password,
        &tools.restic_path,
        None,
    ) {
        Ok(status) => destination_status_response(
            &deps,
            status.reachable,
            status.repo_exists,
            status.reason_code,
            status.message,
        ),
        Err(_) => internal_error(),
    }
}

fn destination_status_response(
    deps: &BackupWebDeps,
    reachable: bool,
    repo_exists: bool,
    reason_code: &str,
    message: &str,
) -> axum::response::Response {
    match status::status(&deps.journal_root, &deps.operations) {
        Ok(mut value) => {
            value["destination_status"] = json!({
                "reachable": reachable,
                "repo_exists": repo_exists,
                "reason_code": reason_code,
                "message": message,
            });
            response::success(value)
        }
        Err(_) => internal_error(),
    }
}

async fn rotate_key(deps: BackupWebDeps) -> axum::response::Response {
    let started = match operation::begin(&deps.operations, "rotate", None, None, None) {
        Ok(started) => started,
        Err(error) => return error,
    };
    let worker = deps.clone();
    operation::spawn_worker(deps.operations.clone(), started.generation, move || {
        let tools = match resolve_tools(&worker) {
            Ok(tools) => tools,
            Err(reason) => {
                return Terminal {
                    phase: "error".into(),
                    reason_code: Some(reason),
                };
            }
        };
        let result = rotate_recovery_key(&worker.journal_root, &services(&worker, &tools));
        match result.status.as_str() {
            "ok" => Terminal {
                phase: "done".into(),
                reason_code: None,
            },
            "skipped" => Terminal {
                phase: "error".into(),
                reason_code: Some("invalid_operation_for_state".into()),
            },
            _ => Terminal {
                phase: "error".into(),
                reason_code: result.reason_code.or_else(|| Some("failed".into())),
            },
        }
    });
    status_response(&deps)
}

async fn teardown_route(deps: BackupWebDeps) -> axum::response::Response {
    let started = match operation::begin(&deps.operations, "teardown", None, None, None) {
        Ok(started) => started,
        Err(error) => return error,
    };
    let worker = deps.clone();
    operation::spawn_worker(deps.operations.clone(), started.generation, move || {
        let tools = match resolve_tools(&worker) {
            Ok(tools) => tools,
            Err(reason) => {
                return Terminal {
                    phase: "error".into(),
                    reason_code: Some(reason),
                };
            }
        };
        let result = teardown_backup(&worker.journal_root, &services(&worker, &tools));
        match result.status.as_str() {
            "ok" | "skipped" => Terminal {
                phase: "done".into(),
                reason_code: None,
            },
            _ => Terminal {
                phase: "error".into(),
                reason_code: result.reason_code.or_else(|| Some("failed".into())),
            },
        }
    });
    status_response(&deps)
}

async fn restore_route(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::missing) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let recovery_key = match validation::required_string(&payload, "recovery_key") {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Err(error) = validation::destination(&payload) {
        return error;
    }
    let destination = destination_from_payload(&payload);
    let started = match operation::begin(&deps.operations, "restore", None, None, None) {
        Ok(started) => started,
        Err(error) => return error,
    };
    let worker = deps.clone();
    operation::spawn_worker(deps.operations.clone(), started.generation, move || {
        let tools = match resolve_tools(&worker) {
            Ok(tools) => tools,
            Err(reason) => {
                return Terminal {
                    phase: "error".into(),
                    reason_code: Some(reason),
                };
            }
        };
        map_restore(restore_journal(
            &worker.journal_root,
            &services(&worker, &tools),
            destination,
            &recovery_key,
        ))
    });
    status_response(&deps)
}

async fn restore_hosted_route(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::missing) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let recovery_key = match validation::required_string(&payload, "recovery_key") {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Some(binding) = load_hosted_binding(&deps.journal_root) {
        let started = match operation::begin(
            &deps.operations,
            "restore_hosted",
            None,
            None,
            Some(recovery_key.clone()),
        ) {
            Ok(started) => started,
            Err(error) => return error,
        };
        let worker = deps.clone();
        operation::spawn_worker(deps.operations.clone(), started.generation, move || {
            bound_restore_hosted(&worker, binding, &recovery_key)
        });
        return status_response(&deps);
    }
    let (nonce, _instance, url) = match mint_portal(&deps) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let started = match operation::begin(
        &deps.operations,
        "restore_hosted",
        Some(url),
        Some(nonce.clone()),
        Some(recovery_key),
    ) {
        Ok(started) => started,
        Err(error) => return error,
    };
    handoff_poll::spawn(deps.clone(), nonce, started.generation);
    status_response(&deps)
}

fn bound_restore_hosted(
    deps: &BackupWebDeps,
    binding: HostedBinding,
    recovery_key: &str,
) -> Terminal {
    let tools = match resolve_tools(deps) {
        Ok(tools) => tools,
        Err(reason) => {
            return Terminal {
                phase: "error".into(),
                reason_code: Some(reason),
            };
        }
    };
    let credentials =
        match fetch_hosted_credentials(deps.http.as_ref(), &binding, "maintenance", deps.version) {
            Ok(credentials) => credentials,
            Err(error) => {
                return Terminal {
                    phase: "error".into(),
                    reason_code: Some(error.reason_code.to_owned()),
                };
            }
        };
    let destination = operated_destination(&binding, &credentials);
    map_restore(restore_journal(
        &deps.journal_root,
        &services(deps, &tools),
        destination,
        recovery_key,
    ))
}

async fn offload_restore_route(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::missing) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Err(error) = validation::restore_day(&payload) {
        return error;
    }
    let all = payload.get("all") == Some(&Value::Bool(true));
    let day = payload
        .get("day")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let started = match operation::begin(&deps.operations, "offload_restore", None, None, None) {
        Ok(started) => started,
        Err(error) => return error,
    };
    let worker = deps.clone();
    operation::spawn_worker(deps.operations.clone(), started.generation, move || {
        let tools = match resolve_tools(&worker) {
            Ok(tools) => tools,
            Err(reason) => {
                return Terminal {
                    phase: "error".into(),
                    reason_code: Some(reason),
                };
            }
        };
        let result = if all {
            restore_all_offload(&worker.journal_root, &services(&worker, &tools))
        } else {
            restore_offload_day(
                &worker.journal_root,
                &services(&worker, &tools),
                day.as_deref().unwrap_or_default(),
            )
        };
        map_offload_restore(result)
    });
    status_response(&deps)
}

async fn handoff_route(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::missing) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let nonce = match validation::required_string(&payload, "nonce") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let matched = match operation::match_handoff(&deps.operations, &nonce) {
        Ok(matched) => matched,
        Err(operation::HandoffError::Expired) => {
            if let Some(generation) = operation::generation_of(&deps.operations) {
                operation::mark_expired(&deps.operations, generation);
            }
            return response::error(
                axum::http::StatusCode::BAD_REQUEST,
                "I couldn't take that action in the current state.",
                "expired",
                "",
            );
        }
        Err(operation::HandoffError::Invalid) => {
            return response::error(
                axum::http::StatusCode::BAD_REQUEST,
                "I couldn't take that action in the current state.",
                "invalid_operation_for_state",
                "",
            );
        }
    };
    let generation = operation::generation_of(&deps.operations).unwrap_or(0);
    if payload.get("needs_subscription") == Some(&Value::Bool(true)) {
        operation::mark_needs_subscription(&deps.operations, generation);
        return status_response(&deps);
    }
    let binding = match hosted_binding_from_payload(&payload) {
        Ok(binding) => binding,
        Err(error) => {
            operation::finish(&deps.operations, generation, "error", Some("failed".into()));
            return error;
        }
    };
    if persist_and_consume_hosted(
        &deps,
        generation,
        matched.kind,
        binding,
        matched.restore_key,
    )
    .is_err()
    {
        return internal_error();
    }
    status_response(&deps)
}

fn hosted_binding_from_payload(
    payload: &Map<String, Value>,
) -> Result<HostedBinding, axum::response::Response> {
    let field = |name: &'static str| validation::required_string(payload, name);
    Ok(HostedBinding {
        broker_endpoint: field("broker_endpoint")?,
        account_id: field("account_id")?,
        instance_id: field("instance_id")?,
        bucket: field("bucket")?,
        prefix: field("prefix")?,
        broker_token: field("broker_token")?,
    })
}

pub(crate) fn persist_and_consume_hosted(
    deps: &BackupWebDeps,
    generation: u64,
    kind: String,
    binding: HostedBinding,
    restore_key: Option<String>,
) -> Result<(), ()> {
    if save_hosted_binding(&deps.journal_root, &binding).is_err() {
        operation::finish(&deps.operations, generation, "error", Some("failed".into()));
        return Err(());
    }
    let worker = deps.clone();
    operation::spawn_worker(deps.operations.clone(), generation, move || {
        consume_hosted(&worker, kind, binding, restore_key)
    });
    Ok(())
}

pub(crate) fn consume_hosted(
    deps: &BackupWebDeps,
    kind: String,
    binding: HostedBinding,
    restore_key: Option<String>,
) -> Terminal {
    let tools = match resolve_tools(deps) {
        Ok(tools) => tools,
        Err(reason) => {
            return Terminal {
                phase: "error".into(),
                reason_code: Some(reason),
            };
        }
    };
    let credentials = match fetch_hosted_credentials(
        deps.http.as_ref(),
        &binding,
        if kind == "enable_hosted" {
            "operated"
        } else {
            "maintenance"
        },
        deps.version,
    ) {
        Ok(credentials) => credentials,
        Err(error) => {
            return Terminal {
                phase: "error".into(),
                reason_code: Some(error.reason_code.to_owned()),
            };
        }
    };
    let destination = operated_destination(&binding, &credentials);
    if kind == "enable_hosted" {
        let keys = match get_keys(&deps.journal_root) {
            Ok(Some(keys)) => keys,
            _ => {
                return Terminal {
                    phase: "error".into(),
                    reason_code: Some("invalid_operation_for_state".into()),
                };
            }
        };
        if let Err(reason) = init_and_enable(
            deps.runner.as_ref(),
            &destination,
            &keys.daily_key,
            &keys.recovery_key,
            &tools.restic_path,
        ) {
            return Terminal {
                phase: "error".into(),
                reason_code: Some(reason),
            };
        }
        if set_mode(&deps.journal_root, "operated").is_err()
            || set_enabled(&deps.journal_root, true).is_err()
        {
            return Terminal {
                phase: "error".into(),
                reason_code: Some("failed".into()),
            };
        }
        return Terminal {
            phase: "done".into(),
            reason_code: None,
        };
    }
    let Some(recovery_key) = restore_key else {
        return Terminal {
            phase: "error".into(),
            reason_code: Some("invalid_operation_for_state".into()),
        };
    };
    let terminal = map_restore(restore_journal(
        &deps.journal_root,
        &services(deps, &tools),
        destination,
        &recovery_key,
    ));
    if matches!(terminal.phase.as_str(), "done" | "degraded") {
        let _ = set_mode(&deps.journal_root, "operated");
    }
    terminal
}

#[cfg(test)]
mod corpus;
#[cfg(test)]
mod test_support;
