// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Link-owned HTTP routes for the Rust convey substrate.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, Json, State};
use axum::http::{HeaderValue, StatusCode, header::LOCATION};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::{error_envelope, not_found_fallback};
use solstone_core_convey_http::gate::require_access;
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_journal_config::load_mutation_base;
use solstone_core_journal_config_write::{
    ConfigMutationError, JournalConfigMutation, mutate_journal_config,
};
use solstone_core_thinking_copy::{CONFIDENTIAL_LANE_DETAIL, ConfidentialLaneDetail, LANES, Lane};

use crate::establish::{self, EstablishError};
use crate::ledger::{AuthorizedClientsRead, read_authorized_clients};
use crate::mark::mark_from_jid;

const INIT_HTML: &str = include_str!("../assets/init.html");
const INIT_LOCAL_ONLY_DETAIL: &str = "setup routes require a localhost connection";

#[derive(Clone)]
struct LinkHttpState {
    journal_root: PathBuf,
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
    version: &'static str,
    journal_path: String,
    identity_name: String,
    identity_preferred: String,
    retention_mode: String,
    retention_days: Value,
    lanes: [Lane; 3],
    confidential: ConfidentialResponse,
}

#[derive(Serialize)]
struct ConfidentialResponse {
    lane_detail: ConfidentialLaneDetail,
}

#[derive(Serialize)]
struct MarkResponse {
    mark: crate::mark::MarkRenderSpec,
    locked: bool,
}

#[derive(Serialize)]
struct LocalCapabilityResponse {
    overall: &'static str,
    checks: [Value; 0],
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
    let state = LinkHttpState {
        journal_root: journal_root.as_ref().to_path_buf(),
    };
    Router::new()
        .route("/init/api/state", get(init_state))
        .route("/init/api/local-capability", get(init_local_capability))
        .route("/init", get(init))
        .route("/init/mark", get(init_mark))
        .route("/init/mark/regenerate", post(init_mark_regenerate))
        .route("/init/mark/lock", post(init_mark_lock))
        .route("/init/finalize", post(init_finalize))
        .with_state(state)
}

/// Build the complete link HTTP surface. The caller remains responsible for serving it.
pub fn router(journal_root: impl AsRef<Path>) -> Router {
    let journal_root = journal_root.as_ref();
    let state = LinkHttpState {
        journal_root: journal_root.to_path_buf(),
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
    let retention_mode = nested_string(&config, "retention", "raw_media");
    Json(InitStateResponse {
        version: env!("CARGO_PKG_VERSION"),
        journal_path: state.journal_root.display().to_string(),
        identity_name: nested_string(&config, "identity", "name"),
        identity_preferred: nested_string(&config, "identity", "preferred"),
        retention_mode: if retention_mode.is_empty() {
            "keep".to_owned()
        } else {
            retention_mode
        },
        retention_days: nested_value(&config, "retention", "raw_media_days").unwrap_or(Value::Null),
        lanes: LANES,
        confidential: ConfidentialResponse {
            lane_detail: CONFIDENTIAL_LANE_DETAIL,
        },
    })
    .into_response()
}

async fn init_local_capability(Extension(basis): Extension<AccessBasis>) -> Response {
    if !is_local(&basis) {
        return init_local_only();
    }
    Json(LocalCapabilityResponse {
        overall: "unknown",
        checks: [],
    })
    .into_response()
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
    Html(INIT_HTML).into_response()
}

async fn init_mark(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<LinkHttpState>,
) -> Response {
    if !is_local(&basis) {
        return init_local_only();
    }
    match establish::load_committed(&state.journal_root) {
        Ok(Some(link_state)) => match mark_from_jid(&link_state.instance_id) {
            Ok(mark) => Json(MarkResponse {
                mark: mark.to_render_spec(),
                locked: true,
            })
            .into_response(),
            Err(error) => establish_error(error.into()),
        },
        Ok(None) => match establish::current_candidate(&state.journal_root)
            .and_then(|candidate| establish::candidate_mark(&candidate))
        {
            Ok(mark) => Json(MarkResponse {
                mark: mark.to_render_spec(),
                locked: false,
            })
            .into_response(),
            Err(error) => establish_error(error),
        },
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
    match establish::load_committed(&state.journal_root) {
        Ok(Some(_)) => error_envelope(
            "invalid_operation_for_state",
            "Bad Request",
            "journal id already locked",
            StatusCode::BAD_REQUEST,
        )
        .into_response(),
        Ok(None) => match establish::regenerate_candidate(&state.journal_root)
            .and_then(|candidate| establish::candidate_mark(&candidate))
        {
            Ok(mark) => Json(MarkResponse {
                mark: mark.to_render_spec(),
                locked: false,
            })
            .into_response(),
            Err(error) => establish_error(error),
        },
        Err(error) => establish_error(error),
    }
}

async fn init_mark_lock(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<LinkHttpState>,
) -> Response {
    if !is_local(&basis) {
        return init_local_only();
    }
    match establish::lock_in(&state.journal_root, None) {
        Ok(link_state) => match mark_from_jid(&link_state.instance_id) {
            Ok(mark) => Json(MarkResponse {
                mark: mark.to_render_spec(),
                locked: true,
            })
            .into_response(),
            Err(error) => establish_error(error.into()),
        },
        Err(EstablishError::NoCandidate) => error_envelope(
            "invalid_operation_for_state",
            "Bad Request",
            "no journal id candidate to lock in — request a preview first",
            StatusCode::BAD_REQUEST,
        )
        .into_response(),
        Err(error) => establish_error(error),
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

    let request = serde_json::from_slice::<FinalizeRequest>(&body).unwrap_or_default();
    let redirect = match finalize_redirect(request.lane.as_ref()) {
        Some(redirect) => redirect,
        None => return invalid_lane(),
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

    // Deliberately does not seed config/convey.json — see
    // solstone/convey/config.py:seed_default_app_navigation; out of scope this wave.
    let result = mutate_journal_config(
        &state.journal_root,
        solstone_core_journal_config_write::LockOptions::default(),
        |config| {
            if !finalize_config_sections_are_objects(config) {
                return JournalConfigMutation {
                    changed: false,
                    value: false,
                };
            }
            let mut changed = false;
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
                if value.is_some_and(|value| value.as_str().is_some_and(|text| !text.is_empty()))
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
                value: true,
            }
        },
    );
    match result {
        Ok(transaction) if !transaction.value => return corrupt_config(),
        Ok(_) => {}
        Err(error) => return config_error(error),
    }
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
    ["convey", "identity", "setup", "retention"]
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

fn finalize_redirect(lane: Option<&Value>) -> Option<String> {
    let Some(lane) = lane else {
        return Some("/app/thinking".to_owned());
    };
    let lane = lane.as_str()?;
    if lane.is_empty() {
        return Some("/app/thinking".to_owned());
    }
    if LANES.iter().any(|candidate| candidate.id == lane) {
        return Some(format!("/app/thinking#{lane}-setup"));
    }
    None
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
            "settings are busy; try again",
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
