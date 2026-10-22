// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Link-owned HTTP routes for the Rust convey substrate.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, Json, State};
use axum::http::{HeaderValue, StatusCode, header::LOCATION};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use solstone_core_check::{CheckReport, Severity, build_check_report, gather_host_inputs};
use solstone_core_convey_http::envelope::{error_envelope, not_found_fallback};
use solstone_core_convey_http::gate::require_access;
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_journal_config::load_mutation_base;
use solstone_core_journal_config_write::{
    ConfigMutationError, JournalConfigMutation, mutate_journal_config,
};
use solstone_core_thinking_copy::{
    CONFIDENTIAL_LANE_DETAIL, ConfidentialLaneDetail, LANES, Lane, SetupCatalogFacts,
    thinking_copy_payload,
};

use crate::ca::jid_from_spki;
use crate::establish::{self, EstablishError};
use crate::ledger::{AuthorizedClientsRead, read_authorized_clients};
use crate::mark::mark_from_jid;

const INIT_HTML: &str = include_str!("../assets/init.html");
const INIT_LOCAL_ONLY_DETAIL: &str = "setup routes require a localhost connection";
const LOCAL_PROVIDER_SELECTION: &str = r#"{"provider":"local","model":"local/qwen3.5-4b"}"#;

fn local_provider_val() -> Value {
    serde_json::from_str(LOCAL_PROVIDER_SELECTION).expect("valid local provider json")
}

pub use crate::setup_progress::{
    SetupDownloadError, SetupDownloadTask, SetupDownloads, SetupProgress, SetupProgressError,
    SetupProgressThinking, SetupRetryError, packaged_speech_retry, read_setup_downloads,
    read_setup_progress,
};

pub trait SetupHost: Send + Sync {
    fn start_local(&self, journal_root: &Path) -> Result<(), String>;
    fn cancel_local(&self, journal_root: &Path, attempt_id: &str) -> Result<(), String>;
    fn retry_speech(&self, journal_root: &Path) -> Result<(), SetupRetryError>;
    fn project_downloads(&self, journal_root: &Path) -> Result<SetupDownloads, SetupProgressError>;
    fn thinking_download_bytes(&self, journal: &Path) -> Option<u64>;
    fn catalog_facts(&self) -> SetupCatalogFacts;
    /// Ask for a fresh thinking check after setup changes the active lane, so
    /// home and Thinking read the new choice instead of the record from before it.
    fn refresh_brain(&self, _journal_root: &Path) {}
}

pub struct NoopSetupHost;

impl SetupHost for NoopSetupHost {
    fn start_local(&self, _journal_root: &Path) -> Result<(), String> {
        Err("start_local unavailable in noop host".into())
    }

    fn cancel_local(&self, _journal_root: &Path, _attempt_id: &str) -> Result<(), String> {
        Err("cancel_local unavailable in noop host".into())
    }

    fn retry_speech(&self, _journal_root: &Path) -> Result<(), SetupRetryError> {
        Err(SetupRetryError {
            code: "retry_unavailable",
            message: "retry unavailable",
            detail: "speech retry is not available.".into(),
        })
    }

    fn project_downloads(
        &self,
        _journal_root: &Path,
    ) -> Result<SetupDownloads, SetupProgressError> {
        Ok(SetupDownloads {
            speech: SetupDownloadTask {
                state: "idle".into(),
                attempt_id: None,
                bytes_done: 0,
                bytes_total: None,
                error: None,
            },
            thinking: SetupDownloadTask {
                state: "idle".into(),
                attempt_id: None,
                bytes_done: 0,
                bytes_total: None,
                error: None,
            },
        })
    }

    fn thinking_download_bytes(&self, _journal: &Path) -> Option<u64> {
        self.catalog_facts().thinking_download_bytes
    }

    fn catalog_facts(&self) -> SetupCatalogFacts {
        solstone_core_thinking_copy::current_setup_facts()
    }
}

#[derive(Clone)]
pub struct LinkHttpState {
    pub journal_root: PathBuf,
    pub host: Arc<dyn SetupHost>,
}

#[derive(Serialize)]
pub struct SetupMarkState {
    pub render: Option<crate::mark::MarkRenderSpec>,
    pub state: &'static str,
    pub proposal_id: Option<String>,
}

#[derive(Serialize)]
pub struct SetupThinkingState {
    pub choice: Option<String>,
    pub saved: bool,
    pub capability: &'static str,
    pub download_bytes: Option<u64>,
}

#[derive(Serialize)]
struct DeviceResponse {
    fingerprint: String,
    device_label: String,
    paired_at: String,
    instance_id: String,
    role: String,
}

#[derive(Serialize)]
struct DevicesResponse {
    devices: Vec<DeviceResponse>,
}

#[derive(Serialize)]
struct IdentityResponse {
    committed: bool,
    instance_id: Option<String>,
    mark: Option<crate::mark::MarkRenderSpec>,
}

#[derive(Serialize)]
struct InitStateResponse {
    schema: &'static str,
    journal_ready: bool,
    version: &'static str,
    journal_path: String,
    identity_name: String,
    identity_preferred: String,
    retention_mode: String,
    retention_days: Value,
    lanes: [Lane; 3],
    confidential: ConfidentialResponse,
    mark: SetupMarkState,
    thinking: SetupThinkingState,
    downloads: SetupDownloads,
    remaining_required_steps: Vec<&'static str>,
    done: bool,
    redirect: &'static str,
    copy: Value,
}

#[derive(Serialize)]
struct ConfidentialResponse {
    lane_detail: ConfidentialLaneDetail,
}

#[derive(Serialize)]
struct MarkResponse {
    mark: Option<crate::mark::MarkRenderSpec>,
    locked: bool,
    availability: &'static str,
    proposal_id: Option<String>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct LocalCapabilityResponse {
    pub overall: &'static str,
    pub local_admitted: bool,
    pub checks: Vec<LocalCapabilityCheck>,
}

impl LocalCapabilityResponse {
    pub fn admits_local(&self) -> bool {
        self.overall == "ok" || (self.overall == "unknown" && self.local_admitted)
    }
}

#[derive(Debug, PartialEq, Serialize)]
pub struct LocalCapabilityCheck {
    pub name: &'static str,
    pub severity: &'static str,
    pub reason_code: &'static str,
    pub detail: String,
}

#[derive(Default, Deserialize)]
struct FinalizeRequest {
    lane: Option<Value>,
    retention_mode: Option<Value>,
    retention_days: Option<Value>,
    name: Option<Value>,
    preferred: Option<Value>,
    timezone: Option<Value>,
}

#[derive(Serialize)]
struct FinalizeResponse {
    success: bool,
    redirect: String,
    warnings: [String; 0],
}

/// First-run setup routes only. Fallback-free so a convey-shell merge keeps the HTML 404.
pub fn init_router(journal_root: impl AsRef<Path>) -> Router {
    init_router_with_host(journal_root, Arc::new(NoopSetupHost))
}

pub fn init_router_with_host(journal_root: impl AsRef<Path>, host: Arc<dyn SetupHost>) -> Router {
    let state = LinkHttpState {
        journal_root: journal_root.as_ref().to_path_buf(),
        host,
    };
    Router::new()
        .route("/init/api/state", get(init_state))
        .route("/init/api/local-capability", get(init_local_capability))
        .route("/init", get(init))
        .route("/init/mark", get(init_mark))
        .route("/init/mark/regenerate", post(init_mark_regenerate))
        .route("/init/mark/lock", post(init_mark_lock))
        .route("/init/api/action", post(init_action))
        .route("/init/finalize", post(init_finalize))
        .with_state(state)
}

/// Build the complete link HTTP surface. The caller remains responsible for serving it.
pub fn router(journal_root: impl AsRef<Path>) -> Router {
    let journal_root = journal_root.as_ref();
    let state = LinkHttpState {
        journal_root: journal_root.to_path_buf(),
        host: Arc::new(NoopSetupHost),
    };
    init_router(journal_root)
        .merge(
            Router::new()
                .route("/app/network/api/devices", get(devices))
                .route("/app/network/api/identity", get(identity))
                .with_state(state),
        )
        .fallback(not_found_fallback)
}

async fn devices(State(state): State<LinkHttpState>) -> Response {
    match read_authorized_clients(
        &state
            .journal_root
            .join("link")
            .join("authorized_clients.json"),
    ) {
        AuthorizedClientsRead::Present(entries) => Json(DevicesResponse {
            devices: entries
                .into_iter()
                .map(|entry| DeviceResponse {
                    fingerprint: entry.fingerprint,
                    device_label: entry.device_label,
                    paired_at: entry.paired_at,
                    instance_id: entry.instance_id,
                    role: entry.role.as_wire().to_owned(),
                })
                .collect(),
        })
        .into_response(),
        AuthorizedClientsRead::Missing => Json(DevicesResponse {
            devices: Vec::new(),
        })
        .into_response(),
        AuthorizedClientsRead::Unreadable => error_envelope(
            "authorization_ledger_unreadable",
            "Service Unavailable",
            "authorized-client ledger could not be read",
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .into_response(),
        AuthorizedClientsRead::Malformed => error_envelope(
            "authorization_ledger_malformed",
            "Service Unavailable",
            "authorized-client ledger is invalid",
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .into_response(),
        AuthorizedClientsRead::DuplicateCid => error_envelope(
            "authorization_ledger_duplicate_cid",
            "Service Unavailable",
            "authorized-client ledger contains a duplicate client identifier",
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .into_response(),
    }
}

async fn identity(State(state): State<LinkHttpState>) -> Response {
    let neutral = || {
        Json(IdentityResponse {
            committed: false,
            instance_id: None,
            mark: None,
        })
        .into_response()
    };
    let Ok(Some(link_state)) = establish::load_committed(&state.journal_root) else {
        return neutral();
    };
    let Ok(mark) = mark_from_jid(&link_state.instance_id) else {
        return neutral();
    };
    Json(IdentityResponse {
        committed: true,
        instance_id: Some(link_state.instance_id),
        mark: Some(mark.to_render_spec()),
    })
    .into_response()
}

async fn init_state(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<LinkHttpState>,
) -> Response {
    if !is_local(&basis) {
        return init_local_only();
    }
    let config = match load_mutation_base(&state.journal_root) {
        Ok(base) => base.config,
        Err(_) => return corrupt_config(),
    };
    match build_setup_state(&state, &config).await {
        Ok(resp) => Json(resp).into_response(),
        Err(err_resp) => err_resp,
    }
}

pub async fn compute_local_capability(journal_root: &Path) -> LocalCapabilityResponse {
    let journal_root = journal_root.to_path_buf();
    #[cfg(all(test, feature = "full-tests"))]
    let checkpoint_root = journal_root.clone();
    let report = tokio::task::spawn_blocking(move || {
        build_check_report(&gather_host_inputs(
            &journal_root,
            env!("CARGO_PKG_VERSION"),
        ))
    })
    .await;
    let capability = match report {
        Ok(report) => local_capability(&report),
        Err(error) => {
            log::warn!("local capability check did not finish: {error}");
            LocalCapabilityResponse {
                overall: "unknown",
                local_admitted: false,
                checks: Vec::new(),
            }
        }
    };
    #[cfg(all(test, feature = "full-tests"))]
    if let Some(paused) = setup_probe_test::after_probe(&checkpoint_root) {
        return paused;
    }
    capability
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) mod setup_probe_test {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::{Mutex, OnceLock, mpsc};

    struct Pause {
        reached: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
    }

    static PAUSES: OnceLock<Mutex<BTreeMap<PathBuf, Pause>>> = OnceLock::new();
    static CAPABILITIES: OnceLock<Mutex<BTreeMap<PathBuf, (&'static str, bool)>>> = OnceLock::new();

    pub(crate) fn capability(root: &Path, overall: &'static str) {
        capability_admitted(root, overall, false);
    }

    pub(crate) fn capability_admitted(root: &Path, overall: &'static str, local_admitted: bool) {
        CAPABILITIES
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(root.to_owned(), (overall, local_admitted));
    }

    pub(crate) fn clear(root: &Path) {
        PAUSES
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(root);
        CAPABILITIES
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(root);
    }

    pub(crate) fn pause(root: &Path) -> (mpsc::Receiver<()>, mpsc::SyncSender<()>) {
        let (reached_tx, reached_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        PAUSES.get_or_init(Default::default).lock().unwrap().insert(
            root.to_owned(),
            Pause {
                reached: reached_tx,
                release: release_rx,
            },
        );
        (reached_rx, release_tx)
    }

    pub(super) fn after_probe(root: &Path) -> Option<LocalCapabilityResponse> {
        let pause = PAUSES
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(root);
        let (overall, local_admitted) = if let Some(pause) = pause {
            pause.reached.send(()).unwrap();
            pause
                .release
                .recv_timeout(std::time::Duration::from_secs(60))
                .unwrap();
            ("ok", true)
        } else {
            *CAPABILITIES
                .get_or_init(Default::default)
                .lock()
                .unwrap()
                .get(root)?
        };
        Some(LocalCapabilityResponse {
            overall,
            local_admitted,
            checks: Vec::new(),
        })
    }
}

async fn build_setup_state(
    state: &LinkHttpState,
    config: &Map<String, Value>,
) -> Result<InitStateResponse, Response> {
    let mark_snapshot = match establish::read_mark_snapshot(&state.journal_root) {
        Ok(snapshot) => snapshot,
        Err(error) => return Err(establish_error(error)),
    };
    let mark_is_locked = mark_snapshot.state == establish::MarkState::Locked;
    let mark = SetupMarkState {
        render: mark_snapshot.render,
        state: mark_snapshot.state.as_str(),
        proposal_id: mark_snapshot.proposal_id,
    };

    let thinking_choice_raw = nested_string(config, "setup", "thinking_choice");
    let (choice, saved) = match thinking_choice_raw.as_str() {
        "local" | "byo" | "spp" | "later" => (Some(thinking_choice_raw), true),
        _ => (None, false),
    };
    let capability = compute_local_capability(&state.journal_root).await.overall;

    let host = state.host.clone();
    let journal = state.journal_root.clone();
    let (downloads, download_bytes) = match tokio::task::spawn_blocking(move || {
        let downloads = host.project_downloads(&journal)?;
        let thinking_bytes = if downloads.thinking.state == "downloading" {
            downloads
                .thinking
                .bytes_total
                .or_else(|| host.thinking_download_bytes(&journal))
        } else {
            host.thinking_download_bytes(&journal)
        };
        Ok::<_, SetupProgressError>((downloads, thinking_bytes))
    })
    .await
    {
        Ok(Ok(pair)) => pair,
        Ok(Err(err)) => {
            return Err(error_envelope(
                err.code,
                "Bad Request",
                &err.detail,
                StatusCode::BAD_REQUEST,
            )
            .into_response());
        }
        Err(err) => {
            return Err(error_envelope(
                "spawn_blocking_failed",
                "Internal Server Error",
                err.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response());
        }
    };

    let thinking = SetupThinkingState {
        choice,
        saved,
        capability,
        download_bytes,
    };
    let catalog_facts = state.host.catalog_facts();

    let completed = setup_is_complete(config);
    let mut remaining_required_steps = Vec::new();
    if !completed {
        if !mark_is_locked {
            remaining_required_steps.push("mark");
        }
        if !saved {
            remaining_required_steps.push("thinking");
        }
    }

    let done = completed;

    let redirect = "/app/home";

    let copy = solstone_core_thinking_copy::setup_copy(&catalog_facts);
    let retention_mode = nested_string(config, "retention", "raw_media");

    Ok(InitStateResponse {
        schema: "solstone.setup-state.v1",
        journal_ready: true,
        version: env!("CARGO_PKG_VERSION"),
        journal_path: state.journal_root.display().to_string(),
        identity_name: nested_string(config, "identity", "name"),
        identity_preferred: nested_string(config, "identity", "preferred"),
        retention_mode: if retention_mode.is_empty() {
            "keep".to_owned()
        } else {
            retention_mode
        },
        retention_days: nested_value(config, "retention", "raw_media_days").unwrap_or(Value::Null),
        lanes: LANES,
        confidential: ConfidentialResponse {
            lane_detail: CONFIDENTIAL_LANE_DETAIL,
        },
        mark,
        thinking,
        downloads,
        remaining_required_steps,
        done,
        redirect,
        copy,
    })
}

async fn init_local_capability(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<LinkHttpState>,
) -> Response {
    if !is_local(&basis) {
        return init_local_only();
    }
    Json(compute_local_capability(&state.journal_root).await).into_response()
}

/// Answer "can this computer run local thinking" from the same platform, GPU and
/// Mac memory rows `journal setup` reads before it chooses local. Those rows use
/// the same memory bars the local model install refuses below.
pub(crate) fn local_capability(report: &CheckReport) -> LocalCapabilityResponse {
    let rows = || {
        report
            .checks
            .iter()
            .filter(|check| matches!(check.name, "platform" | "gpu" | "memory"))
    };
    if let Some(row) = rows().find(|check| check.severity == Severity::Blocked) {
        return LocalCapabilityResponse {
            overall: "blocked",
            local_admitted: false,
            checks: vec![LocalCapabilityCheck {
                name: "local",
                severity: "blocked",
                reason_code: row.name,
                detail: local_unavailable_reason(row.name),
            }],
        };
    }
    if rows().any(|check| check.severity == Severity::Unknown) {
        let checks =
            if rows().any(|check| check.name == "gpu" && check.severity == Severity::Unknown) {
                vec![LocalCapabilityCheck {
                    name: "local",
                    severity: "unknown",
                    reason_code: "gpu",
                    detail: gpu_memory_unknown_reason(),
                }]
            } else {
                Vec::new()
            };
        let local_admitted = matches!(report.platform.os.as_str(), "Darwin" | "Linux");
        return LocalCapabilityResponse {
            overall: "unknown",
            local_admitted,
            checks,
        };
    }
    LocalCapabilityResponse {
        overall: "ok",
        local_admitted: true,
        checks: Vec::new(),
    }
}

fn thinking_copy_pointer_string(pointer: &str) -> String {
    serde_json::to_value(thinking_copy_payload())
        .ok()
        .and_then(|copy| {
            copy.pointer(pointer)
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// The thinking page's own owner copy for the refusal this row predicts.
fn local_unavailable_reason(row: &str) -> String {
    let pointer = match row {
        "gpu" => "/local_recovery/states/gpu_memory_insufficient/reason",
        "memory" => "/local_install/memory_failed",
        _ => "/local_recovery/states/unsupported/reason",
    };
    thinking_copy_pointer_string(pointer)
}

fn gpu_memory_unknown_reason() -> String {
    thinking_copy_pointer_string("/local_install/gpu_memory_unknown")
}

pub(crate) fn injected_init_html() -> String {
    let copy = solstone_core_thinking_copy::setup_copy(
        &solstone_core_thinking_copy::current_setup_facts(),
    );
    let serialized = serde_json::to_string(&copy)
        .unwrap_or_default()
        .replace('<', "\\u003c");
    const TARGET: &str = "<script type=\"application/json\" id=\"setup-copy-bootstrap\"></script>";
    let replacement = format!(
        "<script type=\"application/json\" id=\"setup-copy-bootstrap\">{serialized}</script>"
    );
    INIT_HTML.replace(TARGET, &replacement)
}

async fn init(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<LinkHttpState>,
) -> Response {
    if !is_local(&basis) {
        return init_local_only();
    }
    let config = match load_mutation_base(&state.journal_root) {
        Ok(base) => base.config,
        Err(_) => return corrupt_config(),
    };
    if setup_is_complete(&config) {
        let mut response = StatusCode::FOUND.into_response();
        response
            .headers_mut()
            .insert(LOCATION, HeaderValue::from_static("/"));
        return response;
    }
    Html(injected_init_html()).into_response()
}

async fn init_mark(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<LinkHttpState>,
) -> Response {
    if !is_local(&basis) {
        return init_local_only();
    }
    match establish::ensure_mark_snapshot(&state.journal_root) {
        Ok(snapshot) => Json(MarkResponse {
            mark: snapshot.render,
            locked: snapshot.state == establish::MarkState::Locked,
            availability: if snapshot.state == establish::MarkState::Locked {
                "ready"
            } else {
                "candidate"
            },
            proposal_id: snapshot.proposal_id,
        })
        .into_response(),
        Err(error) => establish_error(error),
    }
}

async fn init_mark_regenerate(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<LinkHttpState>,
) -> Response {
    if !is_local(&basis) {
        return init_local_only();
    }
    match establish::regenerate_proposal(&state.journal_root) {
        Ok(candidate) => match jid_from_spki(candidate.spki_der()) {
            Ok(instance_id) => match mark_from_jid(&instance_id) {
                Ok(mark) => Json(MarkResponse {
                    mark: Some(mark.to_render_spec()),
                    locked: false,
                    availability: "candidate",
                    proposal_id: Some(instance_id),
                })
                .into_response(),
                Err(error) => establish_error(EstablishError::Mark(error)),
            },
            Err(error) => establish_error(EstablishError::Ca(error)),
        },
        Err(EstablishError::AlreadyLocked) => error_envelope(
            "invalid_operation_for_state",
            "Bad Request",
            "journal id already locked",
            StatusCode::BAD_REQUEST,
        )
        .into_response(),
        Err(error) => establish_error(error),
    }
}

#[derive(Deserialize)]
struct MarkLockRequest {
    proposal_id: Option<String>,
}

async fn init_mark_lock(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<LinkHttpState>,
    body: Bytes,
) -> Response {
    if !is_local(&basis) {
        return init_local_only();
    }
    let req: MarkLockRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(_) => {
            return error_envelope(
                "invalid_request",
                "Bad Request",
                "malformed lock request",
                StatusCode::BAD_REQUEST,
            )
            .into_response();
        }
    };
    let Some(proposal_id) = req.proposal_id.filter(|s| !s.trim().is_empty()) else {
        return error_envelope(
            "proposal_required",
            "Bad Request",
            "proposal_id is required",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    };
    match establish::commit_proposal(&state.journal_root, &proposal_id) {
        Ok(link_state) => match mark_from_jid(&link_state.instance_id) {
            Ok(mark) => Json(MarkResponse {
                mark: Some(mark.to_render_spec()),
                locked: true,
                availability: "ready",
                proposal_id: Some(link_state.instance_id),
            })
            .into_response(),
            Err(_) => Json(MarkResponse {
                mark: None,
                locked: true,
                availability: "unavailable",
                proposal_id: Some(link_state.instance_id),
            })
            .into_response(),
        },
        Err(EstablishError::ProposalConflict) => error_envelope(
            "proposal_conflict",
            "Conflict",
            "identity proposal conflict",
            StatusCode::CONFLICT,
        )
        .into_response(),
        Err(error) => establish_error(error),
    }
}

#[derive(Deserialize)]
struct SetupActionRequest {
    action: Option<String>,
    proposal_id: Option<String>,
    choice: Option<String>,
    attempt_id: Option<String>,
    timezone: Option<String>,
}

fn resolve_local_provider_update(
    config: &Map<String, Value>,
) -> Result<(), (&'static str, &'static str, StatusCode)> {
    let endpoint_configured = config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|p| p.get("local"))
        .and_then(Value::as_object)
        .is_some_and(|local| {
            local
                .get("endpoint_url")
                .and_then(Value::as_str)
                .is_some_and(|v| !v.trim().is_empty())
                && local
                    .get("served_model_id")
                    .and_then(Value::as_str)
                    .is_some_and(|v| !v.trim().is_empty())
        });
    let confidential_provisioned = config
        .get("services")
        .and_then(Value::as_object)
        .and_then(|s| s.get("confidential"))
        .is_some_and(Value::is_object);
    if confidential_provisioned {
        return Err((
            "invalid_operation_for_state",
            "turn off confidential processing first, then switch to the bundled local model.",
            StatusCode::BAD_REQUEST,
        ));
    }
    if endpoint_configured {
        return Err((
            "invalid_operation_for_state",
            "clear your own endpoint first to run the bundled local model.",
            StatusCode::BAD_REQUEST,
        ));
    }
    Ok(())
}

type ChoiceError = (&'static str, &'static str, StatusCode);

fn choice_error(error: ChoiceError) -> Response {
    error_envelope(
        error.0,
        error.2.canonical_reason().unwrap_or("Error"),
        error.1,
        error.2,
    )
    .into_response()
}

/// Called only while the journal config authority is held. An unchanged choice
/// is a replay, not permission to restart a cancelled job or replace settings.
fn transition_thinking_choice(
    config: &mut Map<String, Value>,
    choice: &str,
    local_available: bool,
) -> Result<(bool, bool), ChoiceError> {
    if !finalize_config_sections_are_objects(config) {
        return Err((
            "corrupt_config",
            "your journal settings could not be read",
            StatusCode::INTERNAL_SERVER_ERROR,
        ));
    }
    if setup_is_complete(config) {
        return Ok((false, false));
    }
    let previous = nested_string(config, "setup", "thinking_choice");
    if choice == "local" {
        if previous == choice {
            return Ok((false, false));
        }
        if !local_available {
            return Err((
                "local_unavailable",
                "local thinking is not available on your journal's computer",
                StatusCode::BAD_REQUEST,
            ));
        }
        resolve_local_provider_update(config)?;
        let selected = local_provider_val();
        object_mut(config, "providers").insert("active".into(), selected.clone());
        let setup = object_mut(config, "setup");
        setup.insert("thinking_choice".into(), json!(choice));
        setup.insert("thinking_selection".into(), selected);
        return Ok((true, true));
    }
    let mut changed = previous != choice;
    let active = nested_value(config, "providers", "active");
    let selection = nested_value(config, "setup", "thinking_selection");
    if selection.is_some() && active == selection {
        object_mut(config, "providers").remove("active");
        changed = true;
    }
    let setup = object_mut(config, "setup");
    changed |= setup.remove("thinking_selection").is_some();
    if previous != choice {
        setup.insert("thinking_choice".into(), json!(choice));
    }
    Ok((changed, false))
}

/// Retry may repair the unfinished setup selection, but never reactivate an
/// intention superseded while the capability probe was in flight.
fn admit_thinking_retry(config: &mut Map<String, Value>) -> Result<bool, ChoiceError> {
    if !finalize_config_sections_are_objects(config) {
        return Err((
            "corrupt_config",
            "your journal settings could not be read",
            StatusCode::INTERNAL_SERVER_ERROR,
        ));
    }
    if nested_string(config, "setup", "thinking_choice") != "local" {
        return Err((
            "thinking_choice_changed",
            "your thinking choice changed; review it before trying again",
            StatusCode::CONFLICT,
        ));
    }
    let local = local_provider_val();
    let active = nested_value(config, "providers", "active");
    let selected = nested_value(config, "setup", "thinking_selection");
    if active.as_ref() == Some(&local) && selected.as_ref() == Some(&local) {
        resolve_local_provider_update(config)?;
        return Ok(false);
    }
    if setup_is_complete(config) || active.is_some() || selected.is_some() {
        return Err((
            "thinking_choice_changed",
            "your thinking settings changed; open thinking to choose what to use",
            StatusCode::CONFLICT,
        ));
    }
    resolve_local_provider_update(config)?;
    object_mut(config, "providers").insert("active".into(), local.clone());
    object_mut(config, "setup").insert("thinking_selection".into(), local);
    Ok(true)
}

async fn init_action(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<LinkHttpState>,
    body: Bytes,
) -> Response {
    if !is_local(&basis) {
        return init_local_only();
    }
    let req: SetupActionRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(_) => {
            return error_envelope(
                "invalid_request",
                "Bad Request",
                "malformed json request",
                StatusCode::BAD_REQUEST,
            )
            .into_response();
        }
    };
    let Some(action) = req.action.as_deref() else {
        return error_envelope(
            "invalid_operation_for_state",
            "Bad Request",
            "action is required",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    };

    match action {
        "commit_mark" => {
            let Some(proposal_id) = req.proposal_id.filter(|s| !s.trim().is_empty()) else {
                return error_envelope(
                    "proposal_required",
                    "Bad Request",
                    "proposal_id is required",
                    StatusCode::BAD_REQUEST,
                )
                .into_response();
            };
            match establish::commit_proposal(&state.journal_root, &proposal_id) {
                Ok(_) => {}
                Err(EstablishError::ProposalConflict) => {
                    return error_envelope(
                        "proposal_conflict",
                        "Conflict",
                        "identity proposal conflict",
                        StatusCode::CONFLICT,
                    )
                    .into_response();
                }
                Err(error) => return establish_error(error),
            }
        }
        "regenerate_mark" => match establish::regenerate_proposal(&state.journal_root) {
            Ok(_) => {}
            Err(EstablishError::AlreadyLocked) => {
                return error_envelope(
                    "invalid_operation_for_state",
                    "Bad Request",
                    "journal id already locked",
                    StatusCode::BAD_REQUEST,
                )
                .into_response();
            }
            Err(error) => return establish_error(error),
        },
        "save_thinking" => {
            let Some(choice) = req.choice.as_deref() else {
                return error_envelope(
                    "invalid_config_value",
                    "Bad Request",
                    "choice is required",
                    StatusCode::BAD_REQUEST,
                )
                .into_response();
            };
            if !matches!(choice, "local" | "byo" | "spp" | "later") {
                return error_envelope(
                    "invalid_config_value",
                    "Bad Request",
                    "unknown thinking choice",
                    StatusCode::BAD_REQUEST,
                )
                .into_response();
            }

            let local_available = choice != "local"
                || compute_local_capability(&state.journal_root)
                    .await
                    .admits_local();
            let mutation = mutate_journal_config(
                &state.journal_root,
                solstone_core_journal_config_write::LockOptions::default(),
                |config| match transition_thinking_choice(config, choice, local_available) {
                    Ok((changed, start)) => JournalConfigMutation {
                        changed,
                        value: Ok(start),
                    },
                    Err(error) => JournalConfigMutation {
                        changed: false,
                        value: Err(error),
                    },
                },
            );
            let (changed, start) = match mutation {
                Ok(transaction) => match transaction.value {
                    Ok(start) => (transaction.changed, start),
                    Err(error) => return choice_error(error),
                },
                Err(error) => return config_error(error),
            };
            if start {
                let journal = state.journal_root.clone();
                let host = state.host.clone();
                match tokio::task::spawn_blocking(move || host.start_local(&journal)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        return error_envelope(
                            "start_failed",
                            "thinking download could not start",
                            &error,
                            StatusCode::SERVICE_UNAVAILABLE,
                        )
                        .into_response();
                    }
                    Err(error) => {
                        return error_envelope(
                            "start_failed",
                            "thinking download could not start",
                            error.to_string(),
                            StatusCode::SERVICE_UNAVAILABLE,
                        )
                        .into_response();
                    }
                }
            }
            // After the download has started, so the check sees it under way.
            if changed {
                let journal = state.journal_root.clone();
                let host = state.host.clone();
                let _ = tokio::task::spawn_blocking(move || host.refresh_brain(&journal)).await;
            }
        }
        "start_thinking_download" => {
            let base = match load_mutation_base(&state.journal_root) {
                Ok(base) => base.config,
                Err(_) => return corrupt_config(),
            };
            if nested_string(&base, "setup", "thinking_choice") != "local" {
                return choice_error((
                    "thinking_choice_required",
                    "choose local thinking before starting its download",
                    StatusCode::BAD_REQUEST,
                ));
            }
            if !compute_local_capability(&state.journal_root)
                .await
                .admits_local()
            {
                return choice_error((
                    "local_unavailable",
                    "local thinking is not available on your journal's computer",
                    StatusCode::BAD_REQUEST,
                ));
            }
            let mutation = mutate_journal_config(
                &state.journal_root,
                solstone_core_journal_config_write::LockOptions::default(),
                |config| match admit_thinking_retry(config) {
                    Ok(changed) => JournalConfigMutation {
                        changed,
                        value: Ok(()),
                    },
                    Err(error) => JournalConfigMutation {
                        changed: false,
                        value: Err(error),
                    },
                },
            );
            match mutation {
                Ok(transaction) => {
                    if let Err(error) = transaction.value {
                        return choice_error(error);
                    }
                }
                Err(error) => return config_error(error),
            }
            let journal = state.journal_root.clone();
            let host = state.host.clone();
            match tokio::task::spawn_blocking(move || host.start_local(&journal)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    return error_envelope(
                        "start_failed",
                        "thinking download could not start",
                        &error,
                        StatusCode::SERVICE_UNAVAILABLE,
                    )
                    .into_response();
                }
                Err(error) => {
                    return error_envelope(
                        "start_failed",
                        "thinking download could not start",
                        error.to_string(),
                        StatusCode::SERVICE_UNAVAILABLE,
                    )
                    .into_response();
                }
            }
        }
        "cancel_thinking_download" => {
            let Some(attempt_id) = req.attempt_id.filter(|s| !s.trim().is_empty()) else {
                return error_envelope(
                    "attempt_required",
                    "Bad Request",
                    "attempt_id is required",
                    StatusCode::BAD_REQUEST,
                )
                .into_response();
            };
            let journal = state.journal_root.clone();
            let host = state.host.clone();
            let attempt = attempt_id.clone();
            let cancel_res =
                tokio::task::spawn_blocking(move || host.cancel_local(&journal, &attempt)).await;
            match cancel_res {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    return error_envelope(
                        "cancel_failed",
                        "Bad Request",
                        &err,
                        StatusCode::BAD_REQUEST,
                    )
                    .into_response();
                }
                Err(err) => {
                    return error_envelope(
                        "cancel_failed",
                        "Internal Server Error",
                        err.to_string(),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )
                    .into_response();
                }
            }
        }
        "retry_speech_download" => {
            let journal = state.journal_root.clone();
            let host = state.host.clone();
            let retry_res = tokio::task::spawn_blocking(move || host.retry_speech(&journal)).await;
            match retry_res {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    let status = match err.code {
                        "speech_package_unavailable" => StatusCode::CONFLICT,
                        "speech_admission_pending" => StatusCode::SERVICE_UNAVAILABLE,
                        _ => StatusCode::BAD_REQUEST,
                    };
                    return error_envelope(err.code, err.message, &err.detail, status)
                        .into_response();
                }
                Err(err) => {
                    return error_envelope(
                        "retry_failed",
                        "retry failed",
                        err.to_string(),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )
                    .into_response();
                }
            }
        }
        "finalize" => {
            let mut valid_tz = None;
            if let Some(tz) = req.timezone.filter(|s| !s.trim().is_empty()) {
                if solstone_core_journal_config::parse_zone(&tz).is_none() {
                    return error_envelope(
                        "invalid_config_value",
                        "Bad Request",
                        "invalid timezone",
                        StatusCode::BAD_REQUEST,
                    )
                    .into_response();
                }
                valid_tz = Some(tz);
            }

            match establish::load_committed(&state.journal_root) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return error_envelope(
                        "identity_not_locked",
                        "Bad Request",
                        "journal id must be locked before setup can finish",
                        StatusCode::BAD_REQUEST,
                    )
                    .into_response();
                }
                Err(error) => return establish_error(error),
            }

            let base_config = match load_mutation_base(&state.journal_root) {
                Ok(base) => base.config,
                Err(_) => return corrupt_config(),
            };
            if !setup_is_complete(&base_config) {
                let saved_choice = nested_string(&base_config, "setup", "thinking_choice");
                if !matches!(saved_choice.as_str(), "local" | "byo" | "spp" | "later") {
                    return error_envelope(
                        "thinking_choice_required",
                        "Bad Request",
                        "thinking choice is required before setup can finish",
                        StatusCode::BAD_REQUEST,
                    )
                    .into_response();
                }
            }

            let mutation = mutate_journal_config(
                &state.journal_root,
                solstone_core_journal_config_write::LockOptions::default(),
                |config| {
                    if !finalize_config_sections_are_objects(config) {
                        return JournalConfigMutation {
                            changed: false,
                            value: Err((
                                "corrupt_config",
                                "your journal settings could not be read",
                                StatusCode::INTERNAL_SERVER_ERROR,
                            )),
                        };
                    }
                    if setup_is_complete(config) {
                        return JournalConfigMutation {
                            changed: false,
                            value: Ok(false),
                        };
                    }
                    let choice = nested_string(config, "setup", "thinking_choice");
                    if !matches!(choice.as_str(), "local" | "byo" | "spp" | "later") {
                        return JournalConfigMutation {
                            changed: false,
                            value: Err((
                                "thinking_choice_required",
                                "choose thinking before finishing setup",
                                StatusCode::BAD_REQUEST,
                            )),
                        };
                    }
                    object_mut(config, "setup").insert("completed_at".into(), json!(now_ms()));
                    if let Some(tz) = valid_tz.as_ref() {
                        object_mut(config, "identity").insert("timezone".into(), json!(tz));
                    }
                    JournalConfigMutation {
                        changed: true,
                        value: Ok(true),
                    }
                },
            );
            match mutation {
                Ok(transaction) => {
                    if let Err(error) = transaction.value {
                        return choice_error(error);
                    }
                }
                Err(error) => return config_error(error),
            }
        }
        _ => {
            return error_envelope(
                "invalid_operation_for_state",
                "Bad Request",
                "unknown action",
                StatusCode::BAD_REQUEST,
            )
            .into_response();
        }
    }

    let config = match load_mutation_base(&state.journal_root) {
        Ok(base) => base.config,
        Err(_) => return corrupt_config(),
    };
    match build_setup_state(&state, &config).await {
        Ok(state_resp) => Json(state_resp).into_response(),
        Err(err_resp) => err_resp,
    }
}

async fn init_finalize(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<LinkHttpState>,
    body: Bytes,
) -> Response {
    if !is_local(&basis) {
        return init_local_only();
    }
    match establish::load_committed(&state.journal_root) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error_envelope(
                "identity_not_locked",
                "Bad Request",
                "journal id must be locked before setup can finish",
                StatusCode::BAD_REQUEST,
            )
            .into_response();
        }
        Err(error) => return establish_error(error),
    }

    let request = match serde_json::from_slice::<FinalizeRequest>(&body) {
        Ok(req) => req,
        Err(_) => {
            return error_envelope(
                "invalid_request",
                "Bad Request",
                "malformed json request",
                StatusCode::BAD_REQUEST,
            )
            .into_response();
        }
    };

    let lane_str = request
        .lane
        .as_ref()
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let choice = match lane_str {
        "" | "defer" | "later" => "later",
        "local" => "local",
        "byo" => "byo",
        "confidential" => "spp",
        _ => return invalid_lane(),
    };

    let retention_mode = request
        .retention_mode
        .as_ref()
        .and_then(Value::as_str)
        .unwrap_or("keep")
        .to_owned();
    let retention_days = request.retention_days.as_ref().and_then(Value::as_i64);
    if retention_mode == "days" && !retention_days.is_some_and(|days| days > 0) {
        return error_envelope(
            "invalid_config_value",
            "Bad Request",
            "retention_days must be a positive integer",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }

    let config = match materialize_config(&state.journal_root) {
        Ok(config) => config,
        Err(error) => return config_error(error),
    };
    if let Some(response) = invalid_finalize_config_sections(&config) {
        return response;
    }

    let completed_at_existing = setup_is_complete(&config);

    let local_available = if !completed_at_existing && choice == "local" {
        compute_local_capability(&state.journal_root)
            .await
            .admits_local()
    } else {
        false
    };

    let result = mutate_journal_config(
        &state.journal_root,
        solstone_core_journal_config_write::LockOptions::default(),
        |config| {
            if !finalize_config_sections_are_objects(config) {
                return JournalConfigMutation {
                    changed: false,
                    value: Err((
                        "corrupt_config",
                        "your journal settings could not be read",
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )),
                };
            }
            if setup_is_complete(config) {
                return JournalConfigMutation {
                    changed: false,
                    value: Ok((false, false)),
                };
            }
            let (mut changed, start_local) =
                match transition_thinking_choice(config, choice, local_available) {
                    Ok(transition) => transition,
                    Err(error) => {
                        return JournalConfigMutation {
                            changed: false,
                            value: Err(error),
                        };
                    }
                };
            let convey = object_mut(config, "convey");
            if convey.remove("allow_network_access").is_some() {
                changed = true;
            }
            let identity = object_mut(config, "identity");
            for (key, value) in [
                ("name", request.name.as_ref()),
                ("preferred", request.preferred.as_ref()),
                ("timezone", request.timezone.as_ref()),
            ] {
                let usable = |text: &str| {
                    !text.is_empty()
                        && (key != "timezone"
                            || solstone_core_journal_config::parse_zone(text).is_some())
                };
                if value.is_some_and(|value| value.as_str().is_some_and(usable))
                    && identity.get(key) != value
                {
                    identity.insert(key.to_owned(), value.unwrap().clone());
                    changed = true;
                }
            }
            let setup = object_mut(config, "setup");
            let completed_at = now_ms();
            if setup.get("completed_at") != Some(&json!(completed_at)) {
                setup.insert("completed_at".to_owned(), json!(completed_at));
                changed = true;
            }
            let retention = object_mut(config, "retention");
            for (key, value) in [
                ("raw_media", Value::String(retention_mode)),
                (
                    "raw_media_days",
                    retention_days.map_or(Value::Null, |days| json!(days)),
                ),
            ] {
                if retention.get(key) != Some(&value) {
                    retention.insert(key.to_owned(), value);
                    changed = true;
                }
            }
            JournalConfigMutation {
                changed,
                value: Ok((true, start_local)),
            }
        },
    );
    let (wrote_first_completion, should_start_local) = match result {
        Ok(transaction) => match transaction.value {
            Ok(first) => first,
            Err(error) => return choice_error(error),
        },
        Err(error) => return config_error(error),
    };

    if wrote_first_completion && should_start_local {
        let journal = state.journal_root.clone();
        let host = state.host.clone();
        let start_res = tokio::task::spawn_blocking(move || host.start_local(&journal)).await;
        if let Err(err) = start_res {
            return error_envelope(
                "start_failed",
                "Internal Server Error",
                err.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response();
        }
    }

    let redirect = "/app/home".to_owned();

    Json(FinalizeResponse {
        success: true,
        redirect,
        warnings: [],
    })
    .into_response()
}

fn is_local(basis: &AccessBasis) -> bool {
    require_access(basis) && matches!(basis, AccessBasis::Localhost)
}

fn init_local_only() -> Response {
    error_envelope(
        "init_local_only",
        "Forbidden",
        INIT_LOCAL_ONLY_DETAIL,
        StatusCode::FORBIDDEN,
    )
    .into_response()
}

fn materialize_config(journal_root: &Path) -> Result<Map<String, Value>, ConfigMutationError> {
    mutate_journal_config(
        journal_root,
        solstone_core_journal_config_write::LockOptions::default(),
        |config| JournalConfigMutation {
            changed: false,
            value: config.clone(),
        },
    )
    .map(|transaction| transaction.value)
}

fn invalid_finalize_config_sections(config: &Map<String, Value>) -> Option<Response> {
    (!finalize_config_sections_are_objects(config)).then(corrupt_config)
}

fn finalize_config_sections_are_objects(config: &Map<String, Value>) -> bool {
    ["convey", "identity", "setup", "retention", "providers"]
        .into_iter()
        .all(|key| config.get(key).is_none_or(Value::is_object))
}

fn nested_string(config: &Map<String, Value>, section: &str, key: &str) -> String {
    nested_value(config, section, key)
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn nested_value(config: &Map<String, Value>, section: &str, key: &str) -> Option<Value> {
    config
        .get(section)
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .cloned()
}

fn object_mut<'a>(config: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let value = config
        .entry(key.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    value
        .as_object_mut()
        .expect("finalize config sections are validated before mutation")
}

fn setup_is_complete(config: &Map<String, Value>) -> bool {
    nested_value(config, "setup", "completed_at")
        .and_then(|value| value.as_f64())
        .is_some_and(|completed_at| completed_at > 0.0)
}

fn invalid_lane() -> Response {
    error_envelope(
        "invalid_request_value",
        "Bad Request",
        "lane must be one of: byo, confidential, local",
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn config_error(error: ConfigMutationError) -> Response {
    match error {
        ConfigMutationError::Lock(_) => error_envelope(
            "config_busy",
            "your settings are busy. try again.",
            "settings lock unavailable",
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .into_response(),
        ConfigMutationError::Load(_) => corrupt_config(),
        ConfigMutationError::Write(_) => error_envelope(
            "config_write_failed",
            "your settings couldn't be saved.",
            "settings write failed",
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
    }
}

fn corrupt_config() -> Response {
    error_envelope(
        "corrupt_config",
        "your settings couldn't be read.",
        "settings read failed",
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}

fn establish_error(error: EstablishError) -> Response {
    error_envelope(
        "link_identity_error",
        "your journal's identity couldn't be set up.".to_string(),
        error.to_string(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(all(test, not(feature = "full-tests")))]
mod access_tests {
    use super::is_local;
    use solstone_core_convey_http::identity::{AccessBasis, Carrier};

    #[test]
    fn local_gate_accepts_localhost_and_refuses_pairing_peers() {
        assert!(is_local(&AccessBasis::Localhost));
        assert!(!is_local(&AccessBasis::PairingPeer {
            carrier: Carrier::Direct,
        }));
    }
}

#[cfg(test)]
mod local_capability_tests {
    use super::local_capability;
    use solstone_core_check::{
        CheckInputs, DiskInput, MemoryInput, NvidiaInput, PlatformInput, RfdetrCheckInput,
        VulkanInput, build_check_report,
    };

    const GIB: u64 = 1024 * 1024 * 1024;

    fn inputs(os: &str, arch: &str, memory_bytes: Option<u64>) -> CheckInputs {
        CheckInputs {
            platform: PlatformInput {
                os: os.into(),
                os_version: "x".into(),
                arch: arch.into(),
            },
            memory: MemoryInput {
                total_bytes: memory_bytes,
                available_bytes: memory_bytes,
            },
            disk: DiskInput::Ok {
                free_bytes: 100 * GIB,
            },
            journal_path: "/journal".into(),
            nvidia: NvidiaInput {
                detected: false,
                vram_mib: None,
                tiering_memory_mib: None,
                memory_source: "unavailable".into(),
            },
            vulkan: VulkanInput {
                probe_ok: true,
                devices: vec![],
            },
            render_nodes_present_but_inaccessible: false,
            gpu_evaluation_error: None,
            version: "x".into(),
            ced: None,
            rfdetr: RfdetrCheckInput::Omit,
        }
    }

    fn linux_with_nvidia(vram_mib: Option<u64>) -> CheckInputs {
        let mut inputs = inputs("Linux", "x86_64", Some(32 * GIB));
        inputs.nvidia = NvidiaInput {
            detected: true,
            vram_mib,
            tiering_memory_mib: vram_mib,
            memory_source: "nvidia_vram".into(),
        };
        inputs
    }

    #[test]
    fn a_mac_below_the_memory_bar_is_refused_with_a_reason() {
        let capability = local_capability(&build_check_report(&inputs(
            "Darwin",
            "arm64",
            Some(8 * GIB),
        )));
        assert_eq!(capability.overall, "blocked");
        assert!(!capability.local_admitted);
        assert_eq!(capability.checks.len(), 1);
        assert_eq!(capability.checks[0].name, "local");
        assert_eq!(capability.checks[0].severity, "blocked");
        assert_eq!(capability.checks[0].reason_code, "memory");
        assert!(!capability.checks[0].detail.is_empty());
        let ok_report = local_capability(&build_check_report(&inputs(
            "Darwin",
            "arm64",
            Some(32 * GIB),
        )));
        assert_eq!(ok_report.overall, "ok");
        assert!(ok_report.local_admitted);
    }

    #[test]
    fn a_gpu_below_the_memory_bar_is_refused_with_a_reason() {
        let capability = local_capability(&build_check_report(&linux_with_nvidia(Some(4096))));
        assert_eq!(capability.overall, "blocked");
        assert!(!capability.local_admitted);
        assert_eq!(capability.checks.len(), 1);
        assert_eq!(capability.checks[0].name, "local");
        assert_eq!(capability.checks[0].severity, "blocked");
        assert_eq!(capability.checks[0].reason_code, "gpu");
        assert!(!capability.checks[0].detail.is_empty());
        let ok_report = local_capability(&build_check_report(&linux_with_nvidia(Some(12 * 1024))));
        assert_eq!(ok_report.overall, "ok");
        assert!(ok_report.local_admitted);
    }

    #[test]
    fn a_host_without_a_hardware_gpu_is_refused() {
        let capability = local_capability(&build_check_report(&inputs(
            "Linux",
            "x86_64",
            Some(32 * GIB),
        )));
        assert_eq!(capability.overall, "blocked");
        assert!(!capability.local_admitted);
        assert_eq!(capability.checks.len(), 1);
        assert_eq!(capability.checks[0].name, "local");
        assert_eq!(capability.checks[0].severity, "blocked");
        assert_eq!(capability.checks[0].reason_code, "gpu");
        assert!(!capability.checks[0].detail.is_empty());
    }

    #[test]
    fn an_unsupported_platform_is_refused_with_a_reason() {
        let capability = local_capability(&build_check_report(&inputs(
            "Darwin",
            "x86_64",
            Some(32 * GIB),
        )));
        assert_eq!(capability.overall, "blocked");
        assert!(!capability.local_admitted);
        assert_eq!(capability.checks.len(), 1);
        assert_eq!(capability.checks[0].name, "local");
        assert_eq!(capability.checks[0].severity, "blocked");
        assert_eq!(capability.checks[0].reason_code, "platform");
        assert!(!capability.checks[0].detail.is_empty());
    }

    #[test]
    fn unreadable_hardware_stays_unknown() {
        let linux_report = build_check_report(&linux_with_nvidia(None));
        let raw_detail = linux_report
            .checks
            .iter()
            .find(|c| c.name == "gpu")
            .map(|c| c.detail.as_str())
            .unwrap_or("");
        let linux_unknown = local_capability(&linux_report);
        assert_eq!(linux_unknown.overall, "unknown");
        assert!(linux_unknown.local_admitted);
        assert_eq!(linux_unknown.checks.len(), 1);
        assert!(!linux_unknown.checks[0].detail.is_empty());
        assert_ne!(linux_unknown.checks[0].detail, raw_detail);
        let darwin_unknown =
            local_capability(&build_check_report(&inputs("Darwin", "arm64", None)));
        assert_eq!(darwin_unknown.overall, "unknown");
        assert!(darwin_unknown.local_admitted);
        assert!(darwin_unknown.checks.is_empty());

        let mut win_inputs = inputs("Windows", "x86_64", Some(32 * GIB));
        win_inputs.vulkan.probe_ok = false;
        let win_unknown = local_capability(&build_check_report(&win_inputs));
        assert_eq!(win_unknown.overall, "unknown");
        assert!(!win_unknown.local_admitted);
    }
}
