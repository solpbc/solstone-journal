// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native backup Convey routes. Engine POSTs start restic/rclone work and
//! report a page-native operation on GET status.

#![allow(clippy::result_large_err)] // Route handlers return the exact HTTP refusal envelope on the Err path.

use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    body::Bytes,
    routing::{get, post},
};
use chrono::{Datelike, Utc};
use serde_json::{Map, Value, json};
use solstone_core_backup::{
    Destination, HostedBinding, generate_daily_key, get_destination, get_keys, load_hosted_binding,
    save_hosted_binding, set_destination, set_enabled, set_mode,
};
use solstone_core_backup_runtime::hosted_runtime::fetch_hosted_credentials;
use solstone_core_backup_runtime::repo::RepoError;
use solstone_core_backup_runtime::restore::{
    MobileRestoreIdentity, MobileSelection, restore_mobile_journal,
};
use solstone_core_backup_runtime::{
    BackupServices, Clock, ClosedToolError, HttpTransport, JournalMaintenance,
    NativeJournalMaintenance, NativeRestoreRecorder, ResolvedTools, RestoreDraft, RestoreOutcome,
    RestoreRecorder, SystemToolRunner, ToolRunner, UreqHttpTransport, init_repository,
    log_key_label_outcome, neutralize_key_labels, operated_destination, publish_restore_outcome,
    reason_for_returncode, resolve_operational_tools, rotate_recovery_key, validate_destination,
};

pub mod archive_operation;
pub mod archive_worker;
mod assets;
pub mod creator_recovery;
mod first_restore;
pub use first_restore::{EmbeddedRestoreLifecycle, PausedRestore};
mod callosum;
#[cfg(windows)]
mod cleanup;
mod config;
mod handoff_poll;
mod keys;
pub mod maintenance;
mod measurement;
mod offload_enable;
mod offload_report;
mod offload_support;
mod operation;
mod recovery_owner;
pub mod removal_report;
mod response;
mod restore_prepare;
pub mod restore_report;
mod status;
pub use status::home_backup_observation;
mod teardown_report;
mod validation;

pub mod route_lifetime;

pub use operation::{AdmitWrite, WriteGuard, admit_write};
pub(crate) use route_lifetime::LifetimeRunner;
pub use route_lifetime::{BackupRouteDrain, BackupRouteLifetime};
pub use validation::{HandoffFieldError, require_https_portal_url};

use measurement::SharedMeasurementCache;
use operation::{SharedOperationSlot, Terminal};
use restore_prepare::SharedRestorePrepare;

pub fn invalid_operation_response() -> axum::response::Response {
    response::error(
        axum::http::StatusCode::BAD_REQUEST,
        "that action isn't available in the current state.",
        "invalid_operation_for_state",
        "",
    )
}

/// The service is stopping: running archive children are killed and reaped
/// rather than left behind. False if some remain after `timeout`.
pub fn stop_archive_work(timeout: std::time::Duration) -> bool {
    solstone_core_backup::archive_admission::stop_archive_work();
    solstone_core_backup::archive_admission::wait_for_archive_children(timeout)
}

#[derive(Clone)]
pub struct BackupWebDeps {
    pub journal_root: PathBuf,
    pub(crate) root_owner: solstone_core_journal_io::whole_journal_restore::TargetUseOwner,
    pub cache: SharedMeasurementCache,
    pub operations: SharedOperationSlot,
    pub runner: Arc<dyn ToolRunner + Send + Sync>,
    pub http: Arc<dyn HttpTransport + Send + Sync>,
    pub clock: Arc<dyn Clock + Send + Sync>,
    pub journal_maintenance: Arc<dyn JournalMaintenance + Send + Sync>,
    pub restore_recorder: Arc<dyn RestoreRecorder + Send + Sync>,
    pub executable: PathBuf,
    pub portal_base: String,
    pub version: &'static str,
    pub handoff_poll_lease: Arc<AtomicBool>,
    /// Fixed native queue admission. An accepted reference is not a completion.
    pub verification_submitter: Option<VerificationSubmitter>,
    /// Current fixed-routine admission for this serving lifetime; no submission.
    pub offload_dispatch_probe: Option<OffloadDispatchProbe>,
    pub(crate) restore_prepare: SharedRestorePrepare,
    pub(crate) restore_host: first_restore::RestoreHost,
}

/// Host-owned admission for the single `backup:verify` routine.
pub type VerificationSubmitter = Arc<dyn Fn() -> Result<String, &'static str> + Send + Sync>;
pub type OffloadDispatchProbe = Arc<dyn Fn() -> bool + Send + Sync>;

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
    #[cfg(feature = "full-tests")]
    pub fn poison_operation_mutex_for_test(&self) {
        let _held = self.operations.lock().unwrap();
        panic!("injected operation-domain mutex poison");
    }

    pub fn new(journal_root: PathBuf) -> Self {
        let cache = measurement::new(&journal_root);
        Self::production(journal_root, cache)
    }

    /// Bind native backup work to the root captured by its host.
    pub fn with_root(
        root: Arc<solstone_core_journal_io::JournalRoot>,
    ) -> Result<Self, solstone_core_journal_io::JournalRootError> {
        root.revalidate_canonical_binding()?;
        let journal = root.canonical_path().to_path_buf();
        let mut deps = Self::production_with_owner(
            journal.clone(),
            measurement::new(&journal),
            solstone_core_journal_io::whole_journal_restore::TargetUseOwner::from_root(Arc::clone(
                &root,
            )),
        );
        deps.restore_host = first_restore::RestoreHost::Embedded(None);
        #[cfg(target_os = "ios")]
        let deps = {
            let mut deps = deps;
            deps.runner = Arc::new(
                solstone_core_backup_runtime::ios_callable::IosRootToolRunner::new(
                    root,
                    deps.runner,
                )?,
            );
            deps
        };
        Ok(deps)
    }

    /// Install before capturing the embedded router. Native construction
    /// refuses first restore until its host supplies this lifecycle.
    pub fn install_restore_lifecycle(&mut self, lifecycle: Arc<dyn EmbeddedRestoreLifecycle>) {
        self.restore_host = first_restore::RestoreHost::Embedded(Some(lifecycle));
    }

    pub fn retained_root(
        &self,
    ) -> Result<
        Arc<solstone_core_journal_io::JournalRoot>,
        solstone_core_journal_io::JournalRootError,
    > {
        use solstone_core_journal_io::JournalRootError;
        let root = self
            .root_owner
            .root()
            .map_err(|_| JournalRootError::Changed)?;
        // The public path may be relative or an alias at a desktop entry, but
        // it cannot redirect this dependency owner to another journal.
        let path =
            std::fs::canonicalize(&self.journal_root).map_err(|_| JournalRootError::Changed)?;
        if path != root.canonical_path() {
            return Err(JournalRootError::Changed);
        }
        Ok(root)
    }

    pub fn target_use_owner(
        &self,
    ) -> solstone_core_journal_io::whole_journal_restore::TargetUseOwner {
        self.root_owner.clone()
    }

    fn production(journal_root: PathBuf, cache: SharedMeasurementCache) -> Self {
        let root_owner =
            solstone_core_journal_io::whole_journal_restore::TargetUseOwner::new(&journal_root);
        Self::production_with_owner(journal_root, cache, root_owner)
    }

    fn production_with_owner(
        journal_root: PathBuf,
        cache: SharedMeasurementCache,
        root_owner: solstone_core_journal_io::whole_journal_restore::TargetUseOwner,
    ) -> Self {
        Self {
            journal_root,
            root_owner,
            cache,
            operations: operation::new_slot(),
            runner: Arc::new(SystemToolRunner),
            http: Arc::new(UreqHttpTransport),
            clock: Arc::new(ProductionClock),
            journal_maintenance: Arc::new(NativeJournalMaintenance),
            restore_recorder: Arc::new(NativeRestoreRecorder),
            executable: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("")),
            portal_base: "https://services.solstone.app".into(),
            version: env!("CARGO_PKG_VERSION"),
            handoff_poll_lease: Arc::new(AtomicBool::new(false)),
            verification_submitter: None,
            offload_dispatch_probe: None,
            restore_prepare: restore_prepare::new_shared(),
            restore_host: first_restore::RestoreHost::Desktop,
        }
    }

    pub fn lifetime(&self) -> BackupRouteLifetime {
        BackupRouteLifetime {
            operations: Arc::clone(&self.operations),
            restore_prepare: Arc::clone(&self.restore_prepare),
        }
    }
}

pub fn routes(journal_root: PathBuf) -> Router {
    routes_with_deps(BackupWebDeps::new(journal_root))
}

#[cfg(test)]
fn routes_with_cache(journal_root: PathBuf, cache: SharedMeasurementCache) -> Router {
    routes_with_deps(BackupWebDeps::production(journal_root, cache))
}

pub fn routes_with_deps(deps: BackupWebDeps) -> Router {
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
    let restore_hosted_prepare = deps.clone();
    let restore_hosted_key = deps.clone();
    let restore_hosted_arm = deps.clone();
    let restore_hosted_activate = deps.clone();
    let restore_hosted_cancel = deps.clone();
    let offload_restore = deps.clone();
    #[cfg(windows)]
    let cleanup_deps = deps.clone();
    #[cfg(windows)]
    let cleanup_operations = deps.operations.clone();
    let handoff = deps;
    let router = Router::new()
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
            "/app/backup/restore-hosted/prepare",
            post(move || restore_hosted_prepare_route(restore_hosted_prepare.clone())),
        )
        .route(
            "/app/backup/restore-hosted/key",
            post(move |body| restore_hosted_key_route(restore_hosted_key.clone(), body)),
        )
        .route(
            "/app/backup/restore-hosted/arm",
            post(move |body| restore_hosted_arm_route(restore_hosted_arm.clone(), body)),
        )
        .route(
            "/app/backup/restore-hosted/activate",
            post(move |body| restore_hosted_activate_route(restore_hosted_activate.clone(), body)),
        )
        .route(
            "/app/backup/restore-hosted/cancel",
            post(move |body| restore_hosted_cancel_route(restore_hosted_cancel.clone(), body)),
        )
        .route(
            "/app/backup/offload/restore",
            post(move |body| offload_restore_route(offload_restore.clone(), body)),
        )
        .route(
            "/app/backup/handoff",
            post(move |body| handoff_route(handoff.clone(), body)),
        );
    #[cfg(windows)]
    let router = router.route(
        "/app/backup/api/cleanup/retry",
        post(move || cleanup::retry(cleanup_deps.clone())),
    );
    #[cfg(windows)]
    let router = router.layer(axum::middleware::from_fn(move |request, next| {
        cleanup::admit_mutation(cleanup_operations.clone(), request, next)
    }));
    router
}

fn status_response(deps: &BackupWebDeps) -> axum::response::Response {
    restore_prepare::reconcile(&deps.restore_prepare, &deps.operations);
    if let Some(response) = archive_worker::teardown::response(deps) {
        return response;
    }
    if let Some(response) = archive_worker::restore::response(deps, None) {
        return response;
    }
    status::status(&deps.journal_root, &deps.operations)
        .map(response::success)
        .unwrap_or_else(|_| internal_error())
}

fn offload_response(deps: &BackupWebDeps) -> axum::response::Response {
    offload_value(deps)
        .map(response::success)
        .unwrap_or_else(|_| internal_error())
}

fn offload_value(deps: &BackupWebDeps) -> Result<Value, ()> {
    let root = deps.retained_root().map_err(|_| ())?;
    let _target_use =
        solstone_core_journal_io::whole_journal_restore::begin_target_use_bound(Arc::clone(&root))
            .map_err(|_| ())?;
    let mut value = status::offload(root.canonical_path(), &deps.cache, &deps.operations)?;
    let support = offload_support::check(deps, &root);
    value["enable_supported"] = Value::Bool(support.is_ok());
    if let Err(reason) = support {
        value["enable_unavailable_reason"] = json!(reason);
    }
    root.revalidate_canonical_binding().map_err(|_| ())?;
    Ok(value)
}

async fn get_status(deps: BackupWebDeps) -> axum::response::Response {
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::BackupStatus,
        move || status_response(&deps),
    )
    .await
}

async fn get_offload(deps: BackupWebDeps) -> axum::response::Response {
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::BackupOffloadStatus,
        move || offload_response(&deps),
    )
    .await
}

#[cfg(all(test, feature = "full-tests"))]
static GENERATE_KEYS_PAUSE_HOOK: std::sync::Mutex<
    Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
> = std::sync::Mutex::new(None);

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn arm_generate_keys_pause()
-> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *GENERATE_KEYS_PAUSE_HOOK.lock().unwrap() = Some((entered_tx, release_rx));
    (entered_rx, release_tx)
}

#[cfg(all(test, feature = "full-tests"))]
fn pause_generate_keys() {
    let pair = {
        let mut guard = GENERATE_KEYS_PAUSE_HOOK.lock().unwrap();
        guard.take()
    };
    if let Some((entered_tx, release_rx)) = pair {
        let _ = entered_tx.send(());
        let _ = release_rx.recv();
    }
}

#[cfg(all(test, feature = "full-tests"))]
struct ConfirmKeyPause {
    journal: PathBuf,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(all(test, feature = "full-tests"))]
static CONFIRM_KEY_PAUSE_HOOK: std::sync::Mutex<Option<ConfirmKeyPause>> =
    std::sync::Mutex::new(None);

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn arm_confirm_key_pause(
    journal: &std::path::Path,
) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *CONFIRM_KEY_PAUSE_HOOK.lock().unwrap() = Some(ConfirmKeyPause {
        journal: journal.to_path_buf(),
        entered: entered_tx,
        release: release_rx,
    });
    (entered_rx, release_tx)
}

#[cfg(all(test, feature = "full-tests"))]
fn pause_confirm_key(journal: &std::path::Path) {
    let pair = {
        let mut guard = CONFIRM_KEY_PAUSE_HOOK.lock().unwrap();
        if guard.as_ref().is_some_and(|pause| pause.journal == journal) {
            guard.take()
        } else {
            None
        }
    };
    if let Some(pause) = pair {
        let _ = pause.entered.send(());
        let _ = pause.release.recv();
    }
}

async fn generate_keys(deps: BackupWebDeps) -> axum::response::Response {
    let _guard = match admit_write(&deps.operations, AdmitWrite::DuringOperation) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    #[cfg(all(test, feature = "full-tests"))]
    pause_generate_keys();
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
        "that request didn't finish.",
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
            "that action isn't available in the current state.",
            "invalid_operation_for_state",
            "no recovery key yet",
        ),
        Err(_) => internal_error(),
    }
}

async fn confirm_key(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    let _guard = match admit_write(&deps.operations, AdmitWrite::DuringOperation) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
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
                "that action isn't available in the current state.",
                "invalid_operation_for_state",
                "no recovery key yet",
            );
        }
        Err(_) => return internal_error(),
    };
    let parsed_entered = match keys::parse(&entered) {
        Ok(key) => key,
        Err(_) => {
            return response::error(
                axum::http::StatusCode::BAD_REQUEST,
                "that didn't match your recovery key.",
                "recovery_key_mismatch",
                "",
            );
        }
    };
    if parsed_entered != recovery {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "that didn't match your recovery key.",
            "recovery_key_mismatch",
            "",
        );
    }
    #[cfg(all(test, feature = "full-tests"))]
    pause_confirm_key(&deps.journal_root);

    let mut mismatch = false;
    if config::mutate(&deps.journal_root, |backup| {
        if backup.get("recovery_key") != Some(&Value::String(parsed_entered.clone())) {
            mismatch = true;
            return (false, ());
        }
        let changed = backup.get("confirmed_recovery_key") != Some(&Value::Bool(true));
        backup.insert("confirmed_recovery_key".to_owned(), Value::Bool(true));
        (changed, ())
    })
    .is_err()
    {
        return internal_error();
    }
    if mismatch {
        return response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "that didn't match your recovery key.",
            "recovery_key_mismatch",
            "",
        );
    }
    get_status(deps).await
}

async fn retention(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    let _guard = match admit_write(&deps.operations, AdmitWrite::DuringOperation) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
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
    let admitted = match offload_enable::admit_preferences(
        deps,
        offload_enable::PreferenceChange::Limits { budget, floor },
    ) {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    tokio::task::spawn_blocking(admitted)
        .await
        .unwrap_or_else(|_| internal_error())
}

async fn offload_enable(deps: BackupWebDeps) -> axum::response::Response {
    let admitted = match offload_enable::admit(deps) {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    // The admitted owner retains its original-root use and write guard while
    // queued and through publication, even if the HTTP caller disconnects.
    tokio::task::spawn_blocking(admitted)
        .await
        .unwrap_or_else(|_| internal_error())
}

async fn offload_disable(deps: BackupWebDeps) -> axum::response::Response {
    let admitted =
        match offload_enable::admit_preferences(deps, offload_enable::PreferenceChange::Disable) {
            Ok(admitted) => admitted,
            Err(response) => return response,
        };
    tokio::task::spawn_blocking(admitted)
        .await
        .unwrap_or_else(|_| internal_error())
}

async fn backup_now(deps: BackupWebDeps) -> axum::response::Response {
    if !callosum::request(&deps.journal_root, "backup:run") {
        return response::error(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "a backup can't start because your journal's background service isn't running. start it, then try again.",
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
            "that action isn't available in the current state.",
            "invalid_operation_for_state",
            "configure a destination first",
        ));
    }
    if backup.get("confirmed_recovery_key") != Some(&Value::Bool(true)) {
        return Err(response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "backup can't be turned on until you confirm your recovery key.",
            "backup_not_confirmed",
            "",
        ));
    }
    if !matches!(keys::keys(&backup), Ok(Some(_))) {
        return Err(response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "that action isn't available in the current state.",
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
            "backup can't be turned on until you confirm your recovery key.",
            "backup_not_confirmed",
            "",
        ));
    }
    if !matches!(keys::keys(&backup), Ok(Some(_))) {
        return Err(response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "that action isn't available in the current state.",
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
    restic_path: solstone_core_backup_runtime::ResticInvocation<'_>,
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

fn resolve_tools(deps: &BackupWebDeps) -> Result<ResolvedTools, ClosedToolError> {
    resolve_operational_tools(
        deps.runner.as_ref(),
        &deps.journal_root,
        false,
        &deps.executable,
    )
}

fn lifetime_runner<'a>(deps: &'a BackupWebDeps) -> LifetimeRunner<'a> {
    let guard = deps
        .operations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cancellation = guard
        .current
        .as_ref()
        .filter(|current| recovery_owner::owns_recovery(&current.view.kind))
        .and_then(|current| current.archive_cancellation.clone())
        .unwrap_or_else(|| guard.cancellation.clone());
    LifetimeRunner::new(deps.runner.as_ref(), cancellation)
}

fn services<'a>(
    deps: &'a BackupWebDeps,
    runner: &'a dyn ToolRunner,
    tools: &'a solstone_core_backup_runtime::ResolvedTools,
) -> BackupServices<'a> {
    BackupServices {
        runner,
        http: deps.http.as_ref(),
        clock: deps.clock.as_ref(),
        restic: Some(tools.restic.invocation()),
        rclone_path: tools.rclone_path.as_deref(),
        version: deps.version,
        journal_maintenance: deps.journal_maintenance.as_ref(),
    }
}

fn spawn_backup_worker<F>(deps: BackupWebDeps, generation: u64, work: F)
where
    F: FnOnce(BackupWebDeps) -> Terminal + Send + 'static,
{
    #[cfg(windows)]
    cleanup::spawn_worker(deps, generation, work);
    #[cfg(not(windows))]
    let _ = operation::spawn_worker(deps.operations.clone(), generation, move || work(deps));
}

fn map_restore(result: RestoreOutcome) -> Terminal {
    match result.status.as_str() {
        "ok" => Terminal::restore("done", None, result.recording_failure),
        "degraded" => Terminal::restore("degraded", result.reason_code, result.recording_failure),
        _ => Terminal::restore("error", result.reason_code, result.recording_failure),
    }
}

fn publish_restore_error(deps: &BackupWebDeps, reason_code: &str) -> Terminal {
    if let Some(reason) =
        solstone_core_backup_runtime::restore::restore_pending_rotation_refusal(&deps.journal_root)
    {
        return Terminal::error(reason);
    }
    map_restore(publish_restore_outcome(
        &deps.journal_root,
        deps.clock.as_ref(),
        deps.restore_recorder.as_ref(),
        RestoreDraft {
            status: "error".into(),
            reason_code: Some(reason_code.into()),
            integrity_ok: false,
            resumable: false,
            files_expected: None,
            files_restored: None,
            bytes_expected: None,
            bytes_restored: None,
        },
    ))
}

async fn enable_backup(deps: BackupWebDeps) -> axum::response::Response {
    if let Err(error) = enable_preconditions(&deps.journal_root) {
        return error;
    }
    let started = match operation::begin(&deps.operations, "enable", None, None, None) {
        Ok(started) => started,
        Err(error) => return error,
    };
    let generation = started.generation;
    spawn_backup_worker(deps.clone(), generation, move |worker| {
        let tools = match resolve_tools(&worker) {
            Ok(tools) => tools,
            Err(error) => return Terminal::tool_error(error),
        };
        let destination = match get_destination(&worker.journal_root) {
            Ok(Some(destination)) => destination,
            _ => return Terminal::error("invalid_operation_for_state"),
        };
        let keys = match get_keys(&worker.journal_root) {
            Ok(Some(keys)) => keys,
            _ => return Terminal::error("invalid_operation_for_state"),
        };
        let runner = lifetime_runner(&worker);
        if let Err(reason) = init_and_enable(
            &runner,
            &destination,
            &keys.daily_key,
            &keys.recovery_key,
            tools.restic.invocation(),
        ) {
            return Terminal::error(reason);
        }
        if set_enabled(&worker.journal_root, true).is_err() {
            return Terminal::error("failed");
        }
        Terminal::done()
    });
    status_response(&deps)
}

fn mint_portal(deps: &BackupWebDeps) -> Result<(String, String, String), axum::response::Response> {
    let committed = solstone_core_sol_link::committed::load_committed_identity(&deps.journal_root)
        .map_err(|_| internal_error())?;
    let nonce = solstone_core_handoff_nonce::mint_nonce().map_err(|_| internal_error())?;
    let wall_unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .map_err(|_| internal_error())?;
    let assertion = solstone_core_sol_link::home_reach::sign_service_enable_assertion(
        &committed,
        "spb",
        &nonce,
        wall_unix_seconds,
    )
    .map_err(|_| internal_error())?;
    let instance = committed.instance_id().to_owned();
    let url = operation::portal_url(
        &deps.portal_base,
        &nonce,
        &instance,
        &assertion.compact,
        &assertion.ca_pubkey_pem,
    );
    Ok((nonce, instance, url))
}

pub(crate) fn mint_restore_portal(
    deps: &BackupWebDeps,
) -> Result<(String, String), axum::response::Response> {
    let nonce = solstone_core_handoff_nonce::mint_nonce().map_err(|_| internal_error())?;
    let url = operation::restore_portal_url(&deps.portal_base, &nonce);
    Ok((nonce, url))
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
    // An optional S3 signing region is kept for clients that cannot discover it.
    let optional = if backend == "s3" {
        ["region"].as_slice()
    } else {
        [].as_slice()
    };
    let credentials = required
        .iter()
        .chain(optional)
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
    let _guard = match admit_write(&deps.operations, AdmitWrite::IdleOnly) {
        Ok(guard) => guard,
        Err(response) => return response,
    };
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
        Err(error) => {
            return destination_status_response(
                &deps,
                false,
                false,
                &error.to_string(),
                error.guidance(),
                Some(error.detail()),
                Some(error.guidance()),
            );
        }
    };
    let runner = lifetime_runner(&deps);
    match validate_destination(
        &runner,
        &destination,
        &password,
        tools.restic.invocation(),
        None,
    ) {
        Ok(status) => destination_status_response(
            &deps,
            status.reachable,
            status.repo_exists,
            status.reason_code,
            status.message,
            None,
            None,
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
    detail: Option<&str>,
    guidance: Option<&str>,
) -> axum::response::Response {
    match status::status(&deps.journal_root, &deps.operations) {
        Ok(mut value) => {
            let mut status_obj = json!({
                "reachable": reachable,
                "repo_exists": repo_exists,
                "reason_code": reason_code,
                "message": message,
            });
            if let Some(detail) = detail {
                status_obj["detail"] = json!(detail);
            }
            if let Some(guidance) = guidance {
                status_obj["guidance"] = json!(guidance);
            }
            value["destination_status"] = status_obj;
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
    spawn_backup_worker(deps.clone(), started.generation, move |worker| {
        let tools = match resolve_tools(&worker) {
            Ok(tools) => tools,
            Err(error) => return Terminal::tool_error(error),
        };
        let runner = lifetime_runner(&worker);
        let result = rotate_recovery_key(&worker.journal_root, &services(&worker, &runner, &tools));
        match result.status.as_str() {
            "ok" => Terminal::done(),
            "skipped" => Terminal::error("invalid_operation_for_state"),
            _ => Terminal::phase(
                "error",
                result.reason_code.or_else(|| Some("failed".into())),
            ),
        }
    });
    status_response(&deps)
}

async fn teardown_route(deps: BackupWebDeps) -> axum::response::Response {
    match archive_worker::teardown::spawn(&deps) {
        Ok(_) => {}
        Err(error) => return error,
    }
    status_response(&deps)
}

fn restore_pending_response(deps: &BackupWebDeps) -> Option<axum::response::Response> {
    solstone_core_backup_runtime::restore::restore_pending_rotation_refusal(&deps.journal_root).map(
        |reason| {
            response::error(
                axum::http::StatusCode::CONFLICT,
                "restore couldn't start.",
                reason,
                "",
            )
        },
    )
}

/// Continue the recorded restore through the retained caller lifetime.
pub fn recover_recorded_journal_with(
    deps: &BackupWebDeps,
    body: &[u8],
    drain: &mut dyn FnMut() -> Result<Box<dyn PausedRestore>, String>,
) -> Result<(), String> {
    use solstone_core_journal_io::whole_journal_restore::{
        WholeJournalRestoreReadiness, read_whole_journal_restore,
    };
    let record = match read_whole_journal_restore(&deps.journal_root) {
        WholeJournalRestoreReadiness::Pending(record) => *record,
        _ => return Err("whole-journal restore record is unavailable".into()),
    };
    let payload = serde_json::from_slice::<Map<String, Value>>(body)
        .map_err(|_| "malformed json request".to_owned())?;
    let recovery_key = required_text(&payload, "recovery_key")?;
    if let Some(snapshot_id) = optional_text(&payload, "snapshot_id")
        && snapshot_id != record.snapshot_id
    {
        return Err("restore_source_mismatch".into());
    }
    if let Some(subtree) = optional_text(&payload, "subtree")
        && subtree != record.subtree
    {
        return Err("restore_source_mismatch".into());
    }
    let (identity, destination) = retry_destination(&deps.journal_root, &record, &payload)?;
    let selection = MobileSelection {
        snapshot_id: record.snapshot_id,
        subtree: record.subtree,
    };
    let frame = recovery_owner::RecoveryFrame::begin(&deps.operations)?;
    let tools = resolve_tools(deps).map_err(|error| error.to_string())?;
    let runner = LifetimeRunner::new(deps.runner.as_ref(), frame.cancellation());
    let mut paused = None;
    let outcome = restore_mobile_journal(
        deps.retained_root().map_err(|error| error.to_string())?,
        &services(deps, &runner, &tools),
        deps.restore_recorder.as_ref(),
        destination,
        recovery_key,
        identity,
        selection,
        &mut || {
            if paused.is_some() {
                return Err("restore attempt has already drained".into());
            }
            paused = Some(drain()?);
            Ok(())
        },
    );
    let mut result = match outcome.status.as_str() {
        "ok" | "degraded" => Ok(()),
        _ => Err(outcome
            .reason_code
            .clone()
            .unwrap_or_else(|| "restore_failed".into())),
    };
    let mut terminal = map_restore(outcome);
    if result.is_ok() {
        result = if runner
            .cancellation()
            .is_some_and(|stop| stop.is_cancelled())
        {
            Err("restore_failed".into())
        } else {
            match paused.take() {
                Some(attempt) => attempt.resume(&|| {
                    runner
                        .cancellation()
                        .is_some_and(|stop| stop.is_cancelled())
                }),
                None => Err("restore couldn't confirm your journal was paused".into()),
            }
        };
        if result.is_err() {
            terminal = Terminal::error("destination_admission_failed");
        }
    }
    drop(paused);
    frame.finish(terminal);
    result
}

/// Cancel the active initial or retry generation and wait for its actual frame.
pub fn stop_recorded_journal(
    deps: &BackupWebDeps,
    executor: &tokio::runtime::Handle,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let control = operation::admit_write(&deps.operations, operation::AdmitWrite::RecoveryControl)
        .map_err(|_| "restore controls are unavailable".to_owned())?;
    recovery_owner::stop(&deps.operations)?;
    match deps.lifetime().drain_recovery(executor, deadline, &control) {
        BackupRouteDrain::Drained => Ok(()),
        BackupRouteDrain::Pending => Err("restore is still stopping".into()),
        BackupRouteDrain::Failure => Err("couldn't confirm that restore stopped".into()),
    }
}

pub fn recorded_journal_recovery_active(
    deps: &BackupWebDeps,
    executor: &tokio::runtime::Handle,
) -> Result<bool, String> {
    let control = operation::admit_write(&deps.operations, operation::AdmitWrite::RecoveryControl)
        .map_err(|_| "restore controls are unavailable".to_owned())?;
    if recovery_owner::active(&deps.operations)? {
        return Ok(true);
    }
    let recovery = deps
        .operations
        .lock()
        .map_err(|_| "restore controls are unavailable")?
        .current
        .as_ref()
        .is_some_and(|current| recovery_owner::owns_recovery(&current.view.kind));
    if !recovery {
        return Ok(false);
    }
    match deps
        .lifetime()
        .drain_recovery(executor, std::time::Instant::now(), &control)
    {
        BackupRouteDrain::Drained => Ok(false),
        BackupRouteDrain::Pending => Ok(true),
        BackupRouteDrain::Failure => Err("couldn't confirm that restore stopped".into()),
    }
}

fn optional_text<'a>(payload: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn required_text<'a>(payload: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    optional_text(payload, key).ok_or_else(|| format!("{key} required"))
}

fn payload_supplies_destination(payload: &Map<String, Value>) -> bool {
    optional_text(payload, "repository").is_some() || optional_text(payload, "backend").is_some()
}

fn retry_destination(
    journal: &Path,
    record: &solstone_core_journal_io::whole_journal_restore::WholeJournalRestore,
    payload: &Map<String, Value>,
) -> Result<(MobileRestoreIdentity, Destination), String> {
    use solstone_core_journal_io::whole_journal_restore::WholeJournalRestoreSource;
    let mismatch = || Err("restore_source_mismatch".to_owned());
    match &record.source {
        WholeJournalRestoreSource::Hosted {
            account_id,
            instance_id,
            bucket,
            prefix,
        } => {
            if let Some(kind) = optional_text(payload, "kind")
                && kind != "hosted"
            {
                return mismatch();
            }
            for (key, expected) in [
                ("account_id", account_id.as_str()),
                ("instance_id", instance_id.as_str()),
                ("bucket", bucket.as_str()),
                ("prefix", prefix.as_str()),
            ] {
                if let Some(value) = optional_text(payload, key)
                    && value != expected
                {
                    return mismatch();
                }
            }
            if payload_supplies_destination(payload) {
                return mismatch();
            }
            let identity = MobileRestoreIdentity::Hosted {
                account_id: account_id.clone(),
                instance_id: instance_id.clone(),
                bucket: bucket.clone(),
                prefix: prefix.clone(),
            };
            let binding = load_hosted_binding(journal);
            if let Some(binding) = binding.as_ref()
                && (binding.account_id != *account_id
                    || binding.instance_id != *instance_id
                    || binding.bucket != *bucket
                    || binding.prefix != *prefix)
            {
                return mismatch();
            }
            if let Some(endpoint) = optional_text(payload, "broker_endpoint") {
                let Some(mut binding) = binding else {
                    return mismatch();
                };
                binding.broker_endpoint = endpoint.to_owned();
                return hosted_destination_from_binding(identity, &binding);
            }
            match get_destination(journal) {
                Ok(Some(destination)) => Ok((identity, destination)),
                Ok(None) => mismatch(),
                Err(_) => Err("config_unreadable".into()),
            }
        }
        WholeJournalRestoreSource::Byo {
            repository,
            backend,
        } => {
            if let Some(kind) = optional_text(payload, "kind")
                && kind != "byo"
            {
                return mismatch();
            }
            let identity = MobileRestoreIdentity::Byo;
            if payload_supplies_destination(payload) {
                let destination = destination_from_payload(payload);
                if destination.repository != *repository || destination.backend != *backend {
                    return mismatch();
                }
                validation::destination(payload)
                    .map_err(|_| "invalid_restore_credentials".to_owned())?;
                return Ok((identity, destination));
            }
            match get_destination(journal) {
                Ok(Some(destination))
                    if destination.repository == *repository && destination.backend == *backend =>
                {
                    Ok((identity, destination))
                }
                Ok(_) => mismatch(),
                Err(_) => Err("config_unreadable".into()),
            }
        }
    }
}

fn hosted_destination_from_binding(
    identity: MobileRestoreIdentity,
    binding: &HostedBinding,
) -> Result<(MobileRestoreIdentity, Destination), String> {
    let deps_http = UreqHttpTransport;
    let credentials = fetch_hosted_credentials(
        &deps_http,
        binding,
        "maintenance",
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(|error| error.reason_code.to_owned())?;
    Ok((identity, operated_destination(binding, &credentials)))
}

async fn restore_route(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    if let Some(refusal) = restore_pending_response(&deps) {
        return refusal;
    }
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
    spawn_backup_worker(deps.clone(), started.generation, move |worker| {
        panic::catch_unwind(AssertUnwindSafe(|| {
            if let Some(reason) =
                solstone_core_backup_runtime::restore::restore_pending_rotation_refusal(
                    &worker.journal_root,
                )
            {
                return Terminal::error(reason);
            }
            let tools = match resolve_tools(&worker) {
                Ok(tools) => tools,
                Err(error) => {
                    let mut terminal = publish_restore_error(&worker, &error.to_string());
                    terminal.detail = Some(error.detail().to_owned());
                    terminal.guidance = Some(error.guidance().to_owned());
                    return terminal;
                }
            };
            let runner = lifetime_runner(&worker);
            first_restore::run(
                &worker,
                &services(&worker, &runner, &tools),
                destination,
                &recovery_key,
                MobileRestoreIdentity::Byo,
            )
        }))
        // Restore owns this boundary so spawn_worker's generic fallback is unreachable here.
        .unwrap_or_else(|_| {
            publish_restore_error(
                &worker,
                solstone_core_backup_runtime::restore::RESTORE_REASON_RESTORE_FAILED,
            )
        })
    });
    status_response(&deps)
}

async fn restore_hosted_route(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    if let Some(refusal) = restore_pending_response(&deps) {
        return refusal;
    }
    let Some(binding) = load_hosted_binding(&deps.journal_root) else {
        return response::error(
            axum::http::StatusCode::CONFLICT,
            "This journal must start its hosted restore handoff first.",
            "restore_hosted_prepare_required",
            "use the hosted restore prepare flow",
        );
    };
    let payload = match validation::object(&body, response::missing) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let recovery_key = match validation::required_string(&payload, "recovery_key") {
        Ok(value) => value,
        Err(error) => return error,
    };
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
    spawn_backup_worker(deps.clone(), started.generation, move |worker| {
        bound_restore_hosted(&worker, binding, &recovery_key)
    });
    status_response(&deps)
}

async fn restore_hosted_prepare_route(deps: BackupWebDeps) -> axum::response::Response {
    if let Some(refusal) = restore_pending_response(&deps) {
        return refusal;
    }
    if load_hosted_binding(&deps.journal_root).is_some() {
        return response::error(
            axum::http::StatusCode::CONFLICT,
            "This journal already has a hosted backup binding.",
            "restore_hosted_prepare_unbound_only",
            "",
        );
    }
    match restore_prepare::prepare(&deps.restore_prepare, &deps.operations) {
        Ok(prepared) => response::success(json!({
            "capability": prepared.capability,
            "expires_in_seconds": restore_prepare::RESTORE_PREPARE_CONSENT_WINDOW.as_secs(),
        })),
        Err(error) => error,
    }
}

async fn restore_hosted_key_route(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    if let Some(refusal) = restore_pending_response(&deps) {
        return refusal;
    }
    let payload = match validation::object(&body, response::missing) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let capability = match validation::required_string(&payload, "capability") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let recovery_key = match validation::required_string(&payload, "recovery_key") {
        Ok(value) => value,
        Err(error) => return error,
    };
    match restore_prepare::key(
        &deps.restore_prepare,
        &deps.operations,
        &capability,
        recovery_key,
        || mint_restore_portal(&deps),
    ) {
        Ok(keyed) => response::success(json!({"portal_url": keyed.portal_url})),
        Err(error) => error,
    }
}

async fn restore_hosted_arm_route(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    if let Some(refusal) = restore_pending_response(&deps) {
        return refusal;
    }
    let capability = match restore_prepare_capability(&body) {
        Ok(value) => value,
        Err(error) => return error,
    };
    match restore_prepare::arm(&deps.restore_prepare, &deps.operations, &capability) {
        Ok(()) => status_response(&deps),
        Err(error) => error,
    }
}

async fn restore_hosted_activate_route(
    deps: BackupWebDeps,
    body: Bytes,
) -> axum::response::Response {
    if let Some(refusal) = restore_pending_response(&deps) {
        return refusal;
    }
    let capability = match restore_prepare_capability(&body) {
        Ok(value) => value,
        Err(error) => return error,
    };
    match restore_prepare::activate(&deps.restore_prepare, &deps.operations, &capability) {
        Ok(restore_prepare::Activation::Spawn { nonce, generation }) => {
            handoff_poll::spawn(deps.clone(), nonce, generation);
            status_response(&deps)
        }
        Ok(restore_prepare::Activation::AlreadyActivated) => status_response(&deps),
        Err(error) => error,
    }
}

async fn restore_hosted_cancel_route(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    let capability = match restore_prepare_capability(&body) {
        Ok(value) => value,
        Err(error) => return error,
    };
    match restore_prepare::cancel(&deps.restore_prepare, &deps.operations, &capability) {
        Ok(()) => status_response(&deps),
        Err(error) => error,
    }
}

fn restore_prepare_capability(body: &Bytes) -> Result<String, axum::response::Response> {
    let payload = validation::object(body, response::missing)?;
    validation::required_string(&payload, "capability")
}

fn bound_restore_hosted(
    deps: &BackupWebDeps,
    binding: HostedBinding,
    recovery_key: &str,
) -> Terminal {
    panic::catch_unwind(AssertUnwindSafe(|| {
        bound_restore_hosted_inner(deps, binding, recovery_key)
    }))
    // Restore owns this boundary so spawn_worker's generic fallback is unreachable here.
    .unwrap_or_else(|_| {
        publish_restore_error(
            deps,
            solstone_core_backup_runtime::restore::RESTORE_REASON_RESTORE_FAILED,
        )
    })
}

fn bound_restore_hosted_inner(
    deps: &BackupWebDeps,
    binding: HostedBinding,
    recovery_key: &str,
) -> Terminal {
    if let Some(reason) =
        solstone_core_backup_runtime::restore::restore_pending_rotation_refusal(&deps.journal_root)
    {
        return Terminal::error(reason);
    }
    let tools = match resolve_tools(deps) {
        Ok(tools) => tools,
        Err(error) => {
            let mut terminal = publish_restore_error(deps, &error.to_string());
            terminal.detail = Some(error.detail().to_owned());
            terminal.guidance = Some(error.guidance().to_owned());
            return terminal;
        }
    };
    let credentials =
        match fetch_hosted_credentials(deps.http.as_ref(), &binding, "maintenance", deps.version) {
            Ok(credentials) => credentials,
            Err(error) => return publish_restore_error(deps, error.reason_code),
        };
    let destination = operated_destination(&binding, &credentials);
    let runner = lifetime_runner(deps);
    first_restore::run(
        deps,
        &services(deps, &runner, &tools),
        destination,
        recovery_key,
        MobileRestoreIdentity::Hosted {
            account_id: binding.account_id,
            instance_id: binding.instance_id,
            bucket: binding.bucket,
            prefix: binding.prefix,
        },
    )
}

async fn offload_restore_route(deps: BackupWebDeps, body: Bytes) -> axum::response::Response {
    let payload = match validation::object(&body, response::missing) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Err(error) = validation::restore_day(&payload) {
        return error;
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "android",
        target_os = "ios"
    )))]
    {
        let _ = deps;
        response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "that action isn't available in the current state.",
            "restore_held",
            "",
        )
    }
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "android",
        target_os = "ios"
    ))]
    {
        let day = payload
            .get("day")
            .and_then(Value::as_str)
            .map(str::to_owned);
        solstone_core_convey_http::owner_read::spawn_blocking_response(
            solstone_core_convey_http::owner_read::OwnerReadRole::BackupOffloadStatus,
            move || match archive_worker::restore::start(&deps, day) {
                Ok(generation) => archive_worker::restore::response(&deps, Some(generation))
                    .unwrap_or_else(internal_error),
                Err(response) => response,
            },
        )
        .await
    }
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
    if payload.get("status").and_then(Value::as_str) == Some("refused")
        || payload.contains_key("reason_code")
    {
        let matched = match operation::match_handoff(&deps.operations, &nonce, None) {
            Ok(matched) => matched,
            Err(operation::HandoffError::Expired) => {
                return response::error(
                    axum::http::StatusCode::BAD_REQUEST,
                    "that action isn't available in the current state.",
                    "expired",
                    "",
                );
            }
            Err(operation::HandoffError::Invalid) => {
                return response::error(
                    axum::http::StatusCode::BAD_REQUEST,
                    "that action isn't available in the current state.",
                    "invalid_operation_for_state",
                    "",
                );
            }
        };
        let generation = matched.generation;
        let mut refused_payload = payload.clone();
        let _ = refused_payload.remove("nonce");
        let reason = match validation::refused_reason(&refused_payload) {
            Ok(reason) => reason,
            Err(error) => {
                operation::finish(
                    &deps.operations,
                    generation,
                    "error",
                    Some("failed".into()),
                    None,
                );
                return handoff_field_response(error);
            }
        };
        operation::mark_refused(&deps.operations, generation, reason);
        return status_response(&deps);
    }
    let matched = match operation::match_handoff(&deps.operations, &nonce, None) {
        Ok(matched) => matched,
        Err(operation::HandoffError::Expired) => {
            return response::error(
                axum::http::StatusCode::BAD_REQUEST,
                "that action isn't available in the current state.",
                "expired",
                "",
            );
        }
        Err(operation::HandoffError::Invalid) => {
            return response::error(
                axum::http::StatusCode::BAD_REQUEST,
                "that action isn't available in the current state.",
                "invalid_operation_for_state",
                "",
            );
        }
    };
    let generation = matched.generation;
    if payload.get("needs_subscription") == Some(&Value::Bool(true)) {
        if let Err(error) = validate_needs_subscription(&payload, &deps.portal_base) {
            operation::finish(
                &deps.operations,
                generation,
                "error",
                Some("failed".into()),
                None,
            );
            return error;
        }
        operation::mark_needs_subscription(&deps.operations, generation);
        return status_response(&deps);
    }
    let binding = match hosted_binding_from_payload(&payload, &deps.portal_base) {
        Ok(binding) => binding,
        Err(error) => {
            operation::finish(
                &deps.operations,
                generation,
                "error",
                Some("failed".into()),
                None,
            );
            return error;
        }
    };
    persist_and_consume_hosted(
        &deps,
        generation,
        matched.kind,
        binding,
        matched.restore_key,
    );
    status_response(&deps)
}

fn hosted_binding_from_payload(
    payload: &Map<String, Value>,
    portal_base: &str,
) -> Result<HostedBinding, axum::response::Response> {
    let binding = validation::hosted_binding(payload).map_err(handoff_field_response)?;
    validation::require_portal_origin(&binding.broker_endpoint, portal_base)
        .map_err(handoff_field_response)?;
    Ok(binding)
}

fn validate_needs_subscription(
    payload: &Map<String, Value>,
    portal_base: &str,
) -> Result<(), axum::response::Response> {
    let binding = validation::hosted_binding(payload).map_err(handoff_field_response)?;
    validation::require_portal_origin(&binding.broker_endpoint, portal_base)
        .map_err(handoff_field_response)?;
    let url =
        validation::nonempty_string(payload, "subscribe_url").map_err(handoff_field_response)?;
    validation::require_https_portal_url(&url, portal_base).map_err(handoff_field_response)?;
    Ok(())
}

fn handoff_field_response(error: validation::HandoffFieldError) -> axum::response::Response {
    match error {
        validation::HandoffFieldError::Missing(key) => response::missing(&format!("missing {key}")),
        validation::HandoffFieldError::InvalidValue => response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "one of those values couldn't be used.",
            "invalid_request_value",
            "",
        ),
    }
}

pub(crate) fn persist_and_consume_hosted(
    deps: &BackupWebDeps,
    generation: u64,
    kind: String,
    binding: HostedBinding,
    restore_key: Option<String>,
) {
    spawn_backup_worker(deps.clone(), generation, move |worker| {
        if kind == "restore_hosted"
            && let Some(reason) =
                solstone_core_backup_runtime::restore::restore_pending_rotation_refusal(
                    &worker.journal_root,
                )
        {
            return Terminal::error(reason);
        }
        if save_hosted_binding(&worker.journal_root, &binding).is_err() {
            return Terminal::error("failed");
        }
        consume_hosted(&worker, kind, binding, restore_key)
    });
}

pub(crate) fn consume_hosted(
    deps: &BackupWebDeps,
    kind: String,
    binding: HostedBinding,
    restore_key: Option<String>,
) -> Terminal {
    if kind != "enable_hosted"
        && let Some(reason) =
            solstone_core_backup_runtime::restore::restore_pending_rotation_refusal(
                &deps.journal_root,
            )
    {
        return Terminal::error(reason);
    }
    let restore_root = if kind == "enable_hosted" {
        None
    } else {
        match deps.retained_root() {
            Ok(root) => Some(root),
            Err(_) => return Terminal::error("destination_admission_failed"),
        }
    };
    let tools = match resolve_tools(deps) {
        Ok(tools) => tools,
        Err(error) => return Terminal::tool_error(error),
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
        Err(error) => return Terminal::error(error.reason_code),
    };
    let destination = operated_destination(&binding, &credentials);
    if kind == "enable_hosted" {
        let keys = match get_keys(&deps.journal_root) {
            Ok(Some(keys)) => keys,
            _ => return Terminal::error("invalid_operation_for_state"),
        };
        let runner = lifetime_runner(deps);
        if let Err(reason) = init_and_enable(
            &runner,
            &destination,
            &keys.daily_key,
            &keys.recovery_key,
            tools.restic.invocation(),
        ) {
            return Terminal::error(reason);
        }
        // `restic init` labels its key file with this machine's names. The repository
        // already opens with both secrets, so a miss here waits for the daily prune.
        log_key_label_outcome(neutralize_key_labels(
            &runner,
            &destination,
            &keys.daily_key,
            &keys.recovery_key,
            tools.restic.invocation(),
            None,
        ));
        if set_mode(&deps.journal_root, "operated").is_err()
            || set_enabled(&deps.journal_root, true).is_err()
        {
            return Terminal::error("failed");
        }
        return Terminal::done();
    }
    let Some(recovery_key) = restore_key else {
        return Terminal::error("invalid_operation_for_state");
    };
    let runner = lifetime_runner(deps);
    let terminal = first_restore::run(
        deps,
        &services(deps, &runner, &tools),
        destination,
        &recovery_key,
        MobileRestoreIdentity::Hosted {
            account_id: binding.account_id,
            instance_id: binding.instance_id,
            bucket: binding.bucket,
            prefix: binding.prefix,
        },
    );
    if matches!(terminal.phase.as_str(), "done" | "degraded") {
        return first_restore::finish_hosted_mode(
            deps,
            restore_root.expect("restore root captured before work"),
            terminal,
        );
    }
    terminal
}

#[cfg(test)]
mod corpus;
#[cfg(test)]
mod resolution_tests;
#[cfg(test)]
mod test_support;

#[cfg(test)]
mod offload_report_tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "pure restore DTO forwarding controls"
    )]

    use super::*;
    use solstone_core_offload::restore::{RestoreResult, RestoreSegmentResult};

    #[test]
    fn summary_keeps_partial_reports_and_exact_json_integers() {
        let bytes = (1_u64 << 53) + 17;
        let first = RestoreSegmentResult {
            status: "ok".into(),
            reason: None,
            day: "20260101".into(),
            stream: "_default".into(),
            segment: "010000_001".into(),
            snapshot_id: Some("snapshot".into()),
            files_expected: 1,
            files_restored: 1,
            bytes_expected: bytes,
            bytes_restored: bytes,
        };
        let mut later = first.clone();
        later.status = "error".into();
        later.reason = Some("verification_failed".into());
        later.files_restored = 0;
        later.bytes_expected = 100;
        later.bytes_restored = 0;
        for status in ["ok", "no_op", "refused", "degraded", "error", "unknown"] {
            let result = RestoreResult {
                status: status.into(),
                reason: Some("verification_failed".into()),
                scope: "all".into(),
                day: None,
                segments_selected: 2,
                segments_restored: 1,
                files_expected: 2,
                files_restored: 1,
                bytes_expected: bytes + 100,
                bytes_restored: bytes,
                reason_detail: Some("segment identity changed".into()),
                details: vec![first.clone(), later.clone()],
            };
            let encoded = serde_json::to_string(&restore_report::summary(&result)).unwrap();
            let report: Value = serde_json::from_str(&encoded).unwrap();
            assert_eq!(report["status"], status);
            assert_eq!(report["scope"], "all");
            assert!(report["day"].is_null());
            assert_eq!(report["reason"], "verification_failed");
            assert_eq!(report["reason_detail"], "segment identity changed");
            assert_eq!(report["segments_selected"], 2);
            assert_eq!(report["segments_restored"], 1);
            assert_eq!(report["files_expected"], 2);
            assert_eq!(report["files_restored"], 1);
            assert_eq!(report["bytes_expected"].as_u64(), Some(bytes + 100));
            assert_eq!(report["bytes_restored"].as_u64(), Some(bytes));
            assert_eq!(report["details"].as_array().unwrap().len(), 2);
            assert_eq!(
                report["details"][0]["segment"],
                report["details"][1]["segment"]
            );
            assert_eq!(report["details"][0]["status"], "ok");
            assert_eq!(report["details"][0]["bytes_restored"].as_u64(), Some(bytes));
            assert_eq!(report["details"][1]["status"], "error");
            assert_eq!(report["details"][1]["reason"], "verification_failed");
            assert_eq!(report["details"][1]["bytes_restored"], 0);
        }
    }
}
