// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native network read routes and direct device-pairing handlers.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_sol_link::ledger::{
    AuthorizationLedger, AuthorizedClientsLoadError, AuthorizedClientsMutationError,
    AuthorizedClientsRead, ClientActivity, DeviceActivityRead, read_authorized_clients,
    read_device_activity,
};
use solstone_core_sol_link::pairing::addresses::{
    PairingSnapshot, SystemInterfaceSource, SystemRouteIpv4Source, snapshot_from_sources,
};
use solstone_core_sol_link::pairing::nonces::{NonceStore, relay_pairing_nonce_open};
use solstone_core_sol_link::pairing::{
    CeremonyRequest, MintRequest, PairingError, complete_pairing, mint_pairing, pair_response_json,
};

use crate::door::PairingAdmission;
use crate::network_status::{identity, local_endpoints, private_link, status};
use crate::network_writes;
use crate::pair_window_manager::PairWindowManager;
use solstone_core_journal_config::{direct_door_port_from_config, read_direct_door_port};
use solstone_core_thinking::confidential::OperationRegistry;

use crate::{JournalRoot, asset_response, assets};

/// Exact network-device response vocabulary mirrored from
/// `solstone/apps/network/routes.py::_entry_to_json`.
pub(crate) const NETWORK_DEVICE_FIELDS: [&str; 10] = [
    "fingerprint",
    "fingerprint_short",
    "device_label",
    "display_label",
    "client_label",
    "paired_at",
    "last_seen_at",
    "role",
    "network",
    "kind",
];

#[derive(Deserialize)]
pub(crate) struct NonceQuery {
    nonce: String,
}

#[derive(Deserialize)]
pub(crate) struct PairTokenQuery {
    token: Option<String>,
}

pub(crate) const NETWORK_ROUTE_PREFIXES: &[&str] = &["/app/network", "/app/link"];

pub(crate) fn direct_routes(prefix: &str, pair_windows: Arc<PairWindowManager>) -> Router {
    Router::new()
        .route(&format!("{prefix}/pair-start"), post(pair_start))
        .route(
            &format!("{prefix}/api/pair/nonce-status"),
            get(nonce_status),
        )
        .route(&format!("{prefix}/pair"), post(pair))
        .route(&format!("{prefix}/api/devices"), get(devices))
        .layer(Extension(pair_windows))
}

pub fn router(
    journal: Arc<JournalRoot>,
    prefix: &str,
    registry: Arc<OperationRegistry>,
    pair_windows: Arc<PairWindowManager>,
) -> Router {
    Router::new()
        .route(&format!("{prefix}/"), get(shell))
        .route(&format!("{prefix}/workspace"), get(workspace))
        .route(&format!("{prefix}/static/network.js"), get(script))
        .route(&format!("{prefix}/api/state"), get(state))
        .route(&format!("{prefix}/api/status"), get(status))
        .route(&format!("{prefix}/api/identity"), get(identity))
        .route(&format!("{prefix}/api/private-link"), get(private_link))
        .route(&format!("{prefix}/local-endpoints"), get(local_endpoints))
        .route(&format!("{prefix}/unpair"), post(unpair))
        .merge(network_writes::router(prefix))
        .layer(Extension(journal))
        .layer(Extension(registry))
        .layer(Extension(pair_windows))
}

async fn shell() -> Response {
    asset_response("/static/shell.html")
}

async fn workspace() -> Response {
    asset_response("/app/network/workspace")
}

async fn script() -> Response {
    asset_response("/app/network/static/network.js")
}

async fn state(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let link_copy = serde_json::from_str::<Value>(assets::network_copy_json())
        .expect("generated network copy JSON parses");
    Json(json!({
        "link_copy": link_copy,
        "posture": read_posture(&journal.0),
    }))
    .into_response()
}

pub(crate) fn read_posture(journal_root: &std::path::Path) -> &'static str {
    let posture = std::fs::read(journal_root.join("config/journal.json"))
        .ok()
        .and_then(|contents| serde_json::from_slice::<Value>(&contents).ok())
        .and_then(|config| {
            config
                .get("link")?
                .get("posture")?
                .as_str()
                .map(str::to_owned)
        });
    if posture.as_deref() == Some("spl") {
        "spl"
    } else {
        "direct"
    }
}

/// Pair-start accepts only the local owner; a linked device or pairing peer has
/// no authority to mint additional enrollment windows.
pub(crate) async fn pair_start(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    pair_windows: Option<Extension<Arc<PairWindowManager>>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !require_local_owner(&basis) {
        return refusal(
            "pairing_request_invalid",
            "local owner access is required",
            StatusCode::FORBIDDEN,
        );
    }
    let Some(object) = body.as_object() else {
        return refusal(
            "pairing_request_invalid",
            "pairing request must be an object",
            StatusCode::BAD_REQUEST,
        );
    };
    // Python treats a missing or blank label as empty and still mints. The
    // owner workspace posts `{}`; requiring the field made that path a 400.
    let device_label = object
        .get("device_label")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let role = object
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let same_machine = match object.get("same_machine") {
        None => Some(false),
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => None,
    };
    let request = MintRequest {
        device_label: device_label.to_owned(),
        role: role.to_owned(),
        same_machine,
        hardened_loopback: hardened_loopback(&basis, &headers),
        configured_home: configured_home(&root.0),
    };
    let minted = if uses_relay_pairing(&root.0, &request) {
        let Some(Extension(pair_windows)) = pair_windows else {
            return pairing_refusal(PairingError::RelayPairingUnavailable);
        };
        pair_windows
            .mint_and_register(&root.0, &request, now())
            .await
    } else {
        mint_pairing(&root.0, &request, now())
    };
    match minted {
        Ok(response) => Json(json!({
            "nonce": response.nonce,
            "pair_link": response.pair_link,
            "expires_in": response.expires_in,
            "device_label": response.device_label,
            "ca_fingerprint": response.ca_fingerprint,
        }))
        .into_response(),
        Err(error) => pairing_refusal(error),
    }
}

pub(crate) async fn nonce_status(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    Query(query): Query<NonceQuery>,
) -> Response {
    if !require_local_owner(&basis) {
        return refusal(
            "pairing_request_invalid",
            "local owner access is required",
            StatusCode::FORBIDDEN,
        );
    }
    let nonce = NonceStore::new(&root.0).peek(&query.nonce);
    Json(json!({"present": nonce.is_some(), "used": nonce.is_some_and(|entry| entry.used)}))
        .into_response()
}

pub(crate) async fn devices(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    let authorized_path = root.0.join("link/authorized_clients.json");
    let entries = match read_authorized_clients(&authorized_path) {
        AuthorizedClientsRead::Present(entries) => entries,
        AuthorizedClientsRead::Missing => Vec::new(),
        AuthorizedClientsRead::Unreadable => {
            log::warn!(
                "network devices could not read the authorization ledger: authorization_ledger_unreadable"
            );
            return refusal(
                "authorization_ledger_unreadable",
                "authorized-client ledger could not be read",
                StatusCode::SERVICE_UNAVAILABLE,
            );
        }
        AuthorizedClientsRead::Malformed => {
            log::warn!(
                "network devices could not read the authorization ledger: authorization_ledger_malformed"
            );
            return refusal(
                "authorization_ledger_malformed",
                "authorized-client ledger is invalid",
                StatusCode::SERVICE_UNAVAILABLE,
            );
        }
        AuthorizedClientsRead::DuplicateCid => {
            log::warn!(
                "network devices could not read the authorization ledger: authorization_ledger_duplicate_cid"
            );
            return refusal(
                "authorization_ledger_duplicate_cid",
                "authorized-client ledger contains a duplicate client identifier",
                StatusCode::SERVICE_UNAVAILABLE,
            );
        }
    };
    let activity = match read_device_activity(&root.0.join("link/devices.json")) {
        DeviceActivityRead::Present(activity) => Some(activity),
        DeviceActivityRead::Missing => None,
        DeviceActivityRead::Unreadable | DeviceActivityRead::Malformed => {
            log::warn!("network devices could not read device activity metadata");
            None
        }
    };
    let devices = entries
        .iter()
        .map(|entry| network_device_json(entry, activity.as_ref()))
        .collect::<Vec<_>>();
    Json(json!({"devices": devices})).into_response()
}

pub(crate) async fn unpair(Extension(root): Extension<Arc<JournalRoot>>, body: Bytes) -> Response {
    let object = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let fingerprint = object
        .get("fingerprint")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let device_label = object
        .get("device_label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let target = match (fingerprint, device_label) {
        (Some(fingerprint), _) => fingerprint.to_owned(),
        (None, Some(label)) => match resolve_unpair_label(&root.0, label) {
            Ok(Some(fingerprint)) => fingerprint,
            Ok(None) => {
                return refusal(
                    "paired_device_not_found",
                    "paired device not found",
                    StatusCode::BAD_REQUEST,
                );
            }
            Err(error) => return unpair_label_refusal(error),
        },
        (None, None) => {
            return refusal(
                "missing_required_field",
                "fingerprint or device_label is required",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    match AuthorizationLedger::new(&root.0).remove(&target) {
        Ok(outcome) if outcome.authorized_removed => {
            Json(json!({"unpaired": target})).into_response()
        }
        Ok(_) => refusal(
            "paired_device_not_found",
            "paired device not found",
            StatusCode::BAD_REQUEST,
        ),
        Err(error) => unpair_mutation_refusal(error),
    }
}

enum UnpairLabelError {
    Unreadable,
    Malformed,
    DuplicateCid,
    Ambiguous,
}

fn resolve_unpair_label(
    journal_root: &std::path::Path,
    label: &str,
) -> Result<Option<String>, UnpairLabelError> {
    match read_authorized_clients(&journal_root.join("link/authorized_clients.json")) {
        AuthorizedClientsRead::Present(entries) => {
            let matches = entries
                .into_iter()
                .filter(|entry| entry.display_label() == label)
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [] => Ok(None),
                [entry] => Ok(Some(entry.fingerprint.clone())),
                _ => Err(UnpairLabelError::Ambiguous),
            }
        }
        AuthorizedClientsRead::Missing => Ok(None),
        AuthorizedClientsRead::Unreadable => Err(UnpairLabelError::Unreadable),
        AuthorizedClientsRead::Malformed => Err(UnpairLabelError::Malformed),
        AuthorizedClientsRead::DuplicateCid => Err(UnpairLabelError::DuplicateCid),
    }
}

fn unpair_label_refusal(error: UnpairLabelError) -> Response {
    match error {
        UnpairLabelError::Unreadable => refusal(
            "authorization_ledger_unreadable",
            "authorized-client ledger could not be read",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        UnpairLabelError::Malformed => refusal(
            "authorization_ledger_malformed",
            "authorized-client ledger is invalid",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        UnpairLabelError::DuplicateCid => refusal(
            "authorization_ledger_duplicate_cid",
            "authorized-client ledger contains a duplicate client identifier",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        UnpairLabelError::Ambiguous => refusal(
            "invalid_operation_for_state",
            "device label matches more than one paired device",
            StatusCode::BAD_REQUEST,
        ),
    }
}

pub(crate) fn unpair_mutation_refusal(error: AuthorizedClientsMutationError) -> Response {
    match error {
        AuthorizedClientsMutationError::Load(AuthorizedClientsLoadError::Unreadable { .. })
        | AuthorizedClientsMutationError::Lock(_) => refusal(
            "authorization_ledger_unreadable",
            "authorized-client ledger could not be read",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        AuthorizedClientsMutationError::Load(AuthorizedClientsLoadError::Malformed { .. }) => {
            refusal(
                "authorization_ledger_malformed",
                "authorized-client ledger is invalid",
                StatusCode::SERVICE_UNAVAILABLE,
            )
        }
        AuthorizedClientsMutationError::Load(AuthorizedClientsLoadError::DuplicateCid {
            ..
        }) => refusal(
            "authorization_ledger_duplicate_cid",
            "authorized-client ledger contains a duplicate client identifier",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        AuthorizedClientsMutationError::Write(_)
        | AuthorizedClientsMutationError::Device(_)
        | AuthorizedClientsMutationError::InvalidLabel(_)
        | AuthorizedClientsMutationError::InvalidLastSeenAt
        | AuthorizedClientsMutationError::InvalidActivityTimestamp(_) => refusal(
            "internal_error",
            "couldn't unpair this device",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

fn network_device_json(
    entry: &solstone_core_sol_link::ledger::ClientEntry,
    activity: Option<&std::collections::BTreeMap<String, ClientActivity>>,
) -> Value {
    let last_seen_at = activity
        .and_then(|devices| devices.get(&entry.fingerprint))
        .map(|device| Value::from(device.last_seen_at.clone()))
        .unwrap_or(Value::Null);
    let value = Map::from_iter([
        ("fingerprint".to_owned(), json!(entry.fingerprint)),
        (
            "fingerprint_short".to_owned(),
            json!(
                entry
                    .fingerprint
                    .strip_prefix("sha256:")
                    .unwrap_or(&entry.fingerprint)
                    .chars()
                    .take(16)
                    .collect::<String>()
            ),
        ),
        ("device_label".to_owned(), json!(entry.device_label)),
        ("display_label".to_owned(), json!(entry.display_label())),
        ("client_label".to_owned(), json!(entry.client_label)),
        ("paired_at".to_owned(), json!(entry.paired_at)),
        ("last_seen_at".to_owned(), last_seen_at),
        ("role".to_owned(), json!(entry.role.as_wire())),
        ("network".to_owned(), json!(entry.network)),
        ("kind".to_owned(), json!(entry.kind)),
    ]);
    debug_assert_eq!(value.len(), NETWORK_DEVICE_FIELDS.len());
    Value::Object(value)
}

pub(crate) async fn pair(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    pairing_admission: Option<Extension<PairingAdmission>>,
    pair_windows: Option<Extension<Arc<PairWindowManager>>>,
    snapshot: Option<Extension<PairingSnapshot>>,
    Query(query): Query<PairTokenQuery>,
    Json(request): Json<spl_core::PairRequest>,
) -> Response {
    // The owner cannot drive the ceremony through loopback: a direct pairing
    // request is accepted only from the cert-less door carrier.
    if !matches!(basis, AccessBasis::PairingPeer { .. }) {
        return refusal(
            "pairing_request_invalid",
            "pairing requires a pairing carrier",
            StatusCode::FORBIDDEN,
        );
    }
    let sender_instance_id = request
        .additional_fields
        .get("sender_instance_id")
        .map(|value| value.as_str().unwrap_or_default());
    let Some(nonce) = query.token.as_deref().or_else(|| {
        request
            .additional_fields
            .get("nonce")
            .and_then(Value::as_str)
    }) else {
        return refusal(
            "missing_required_field",
            "nonce is required",
            StatusCode::BAD_REQUEST,
        );
    };
    let submitted_relay_nonce = relay_pairing_nonce_open(&NonceStore::new(&root.0), nonce, now());
    let relay_nonce_mismatch = match pairing_admission.as_ref() {
        Some(Extension(PairingAdmission::Relay(identity))) => !identity.matches(nonce),
        Some(Extension(PairingAdmission::Direct)) | None => submitted_relay_nonce,
    };
    if relay_nonce_mismatch {
        return refusal(
            "pairing_request_invalid",
            "pairing nonce does not match the pairing carrier",
            StatusCode::FORBIDDEN,
        );
    }
    let snapshot = match snapshot {
        Some(Extension(snapshot)) => snapshot,
        None => match snapshot_from_sources(&SystemInterfaceSource, &SystemRouteIpv4Source) {
            Ok(snapshot) => snapshot,
            Err(error) => return pairing_refusal(PairingError::Address(error)),
        },
    };
    match complete_pairing(
        &root.0,
        CeremonyRequest {
            request: &request,
            nonce,
            sender_instance_id,
            local_endpoints: response_local_endpoints(
                &snapshot,
                match read_direct_door_port(&root.0) {
                    Ok(port) => port,
                    Err(_) => {
                        return refusal(
                            "internal_error",
                            "couldn't read journal config",
                            StatusCode::INTERNAL_SERVER_ERROR,
                        );
                    }
                },
            ),
        },
        now(),
    ) {
        Ok(response) => {
            if let Some(Extension(pair_windows)) = pair_windows {
                let _ = pair_windows.retire(&root.0, nonce, now()).await;
            }
            match pair_response_json(&response) {
                Ok(value) => {
                    emit_pair_complete(&root.0, &response.fingerprint);
                    Json(value).into_response()
                }
                Err(error) => pairing_refusal(error),
            }
        }
        Err(error) => pairing_refusal(error),
    }
}

pub(crate) fn uses_relay_pairing(journal_root: &std::path::Path, request: &MintRequest) -> bool {
    read_posture(journal_root) == "spl" && request.same_machine == Some(false)
}

fn response_local_endpoints(snapshot: &PairingSnapshot, port: u16) -> Option<Value> {
    (!snapshot.endpoints.is_empty()).then(|| {
        Value::Array(
            snapshot
                .endpoints
                .iter()
                .map(|endpoint| {
                    json!({
                        "ip": endpoint.ip.to_string(),
                        "port": port,
                        "scope": endpoint.scope,
                    })
                })
                .collect(),
        )
    })
}

/// Pair-complete is an owner-facing notification only: losing the local
/// Callosum socket must never undo an already persisted ceremony.
fn emit_pair_complete(journal_root: &std::path::Path, fingerprint: &str) {
    let mut ledger = AuthorizationLedger::new(journal_root);
    let Some(entry) = ledger.get(fingerprint) else {
        log::debug!("paired-device entry was absent while emitting pair completion");
        return;
    };
    let mut extra = Map::new();
    extra.insert("device_label".to_owned(), json!(entry.display_label()));
    extra.insert("fingerprint".to_owned(), json!(entry.fingerprint));
    extra.insert(
        "fingerprint_short".to_owned(),
        json!(fingerprint.strip_prefix("sha256:").unwrap_or(fingerprint)[..16]),
    );
    extra.insert("paired_at".to_owned(), json!(entry.paired_at));
    extra.insert(
        "network".to_owned(),
        json!(entry.network.as_deref().unwrap_or("network")),
    );
    let envelope = CallosumEnvelope {
        tract: "link".to_owned(),
        event: "pair_complete".to_owned(),
        ts: None,
        extra,
    };
    let Ok(mut line) = serde_json::to_string(&envelope) else {
        return;
    };
    line.push('\n');
    let sender = CallosumOneShotSender::new(
        journal_root.join("health/callosum.sock"),
        Duration::from_secs(1),
    );
    if sender.send_line(&line).is_err() {
        log::debug!("paired-device Callosum pair-complete notification unavailable");
    }
}

fn require_local_owner(basis: &AccessBasis) -> bool {
    matches!(basis, AccessBasis::Localhost)
}

pub(crate) fn hardened_loopback(basis: &AccessBasis, headers: &HeaderMap) -> bool {
    matches!(basis, AccessBasis::Localhost)
        && ["x-forwarded-for", "x-real-ip", "x-forwarded-host"]
            .iter()
            .all(|name| !headers.contains_key(*name))
}

fn configured_home(journal_root: &std::path::Path) -> Option<Ipv4Addr> {
    let read = solstone_core_journal_config::read_journal_config(journal_root).ok()?;
    let config = read.config.as_ref()?;
    let address = config.get("pairing")?.get("home_address")?.as_str()?;
    let (host, port) = address.rsplit_once(':')?;
    let expected = direct_door_port_from_config(config).ok()?;
    (port.parse::<u16>().ok() == Some(expected))
        .then(|| host.parse().ok())
        .flatten()
}

fn pairing_refusal(error: PairingError) -> Response {
    let (reason, status) = match &error {
        PairingError::Certificate(_) => ("pairing_key_invalid", StatusCode::BAD_REQUEST),
        _ => (
            error.reason(),
            StatusCode::from_u16(error.status()).expect("pairing status is valid"),
        ),
    };
    let detail = error
        .detail()
        .unwrap_or("pairing request could not be completed");
    refusal(reason, detail, status)
}

pub(crate) fn refusal(reason_code: &str, detail: &str, status: StatusCode) -> Response {
    (
        status,
        Json(json!({
            "reason_code": reason_code,
            "reason": reason_code,
            "error": detail,
            "detail": detail,
        })),
    )
        .into_response()
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs()
        .try_into()
        .expect("Unix timestamp fits i64")
}

#[cfg(test)]
#[path = "pairing_contract_vectors.rs"]
mod pairing_contract_vectors;

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crate::authorization_gate::DoorRouter;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, header};
    use axum::routing::get;
    use axum::{Extension, Router};
    use solstone_core_convey_http::identity::{AccessBasis, Carrier};
    use solstone_core_sol_link::ca::{generate_ca, jid_from_spki};
    use tower::ServiceExt;

    use super::*;

    struct TempDir(tempfile::TempDir);

    impl TempDir {
        fn new() -> Self {
            Self(tempfile::TempDir::new_in("/var/tmp").expect("temporary root"))
        }

        fn path(&self) -> &Path {
            self.0.path()
        }
    }

    fn committed_identity(root: &Path) {
        let ca = generate_ca().expect("CA");
        fs::create_dir_all(root.join("link/ca")).expect("CA directory");
        fs::write(root.join("link/ca/cert.pem"), ca.certificate_pem()).expect("CA certificate");
        fs::write(root.join("link/ca/private.pem"), ca.private_key_pem()).expect("CA key");
        fs::write(
            root.join("link/state.json"),
            json!({"instance_id": jid_from_spki(ca.spki_der()).expect("JID"), "home_label": "Home"}).to_string(),
        ).expect("state");
        fs::create_dir_all(root.join("config")).expect("config directory");
        fs::write(
            root.join("config/journal.json"),
            r#"{"pairing":{"home_address":"10.0.0.2:7657"}}"#,
        )
        .expect("config");
    }

    fn established_journal(root: &Path) {
        fs::create_dir_all(root.join("config")).expect("config directory");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .expect("established config");
    }

    async fn get_response(app: Router, path: &str) -> Response {
        app.oneshot(
            Request::get(path)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds")
    }

    #[tokio::test]
    async fn nonce_status_reports_missing_live_and_used_values_without_mutation() {
        let temporary = TempDir::new();
        let root = Arc::new(JournalRoot(temporary.path().to_path_buf()));
        let app = Router::new()
            .route("/status", get(nonce_status))
            .layer(Extension(AccessBasis::Localhost))
            .layer(Extension(root.clone()));
        async fn status(app: Router, nonce: &str) -> Value {
            let response = app
                .oneshot(
                    Request::get(format!("/status?nonce={nonce}"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            serde_json::from_slice(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body"),
            )
            .expect("JSON")
        }
        assert_eq!(
            status(app.clone(), "nonce").await,
            json!({"present":false,"used":false})
        );
        let store = NonceStore::new(temporary.path());
        store
            .add("nonce".into(), "phone".into(), "".into(), false, now())
            .expect("mint");
        assert_eq!(
            status(app.clone(), "nonce").await,
            json!({"present":true,"used":false})
        );
        store.consume("nonce", now()).expect("consume");
        assert_eq!(
            status(app, "nonce").await,
            json!({"present":true,"used":true})
        );
    }

    #[tokio::test]
    async fn owner_routes_refuse_pairing_peers_while_pair_route_refuses_the_owner() {
        let temporary = TempDir::new();
        let root = Arc::new(JournalRoot(temporary.path().to_path_buf()));
        let owner = Router::new()
            .route("/status", get(nonce_status))
            .layer(Extension(AccessBasis::PairingPeer {
                carrier: Carrier::Direct,
            }))
            .layer(Extension(root.clone()));
        let response = owner
            .oneshot(
                Request::get("/status?nonce=x")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let pair_route = Router::new()
            .route("/pair", axum::routing::post(pair))
            .layer(Extension(AccessBasis::Localhost))
            .layer(Extension(root));
        let response = pair_route
            .oneshot(
                Request::post("/pair?token=x")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"csr":"x","device_label":"phone"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn unconfined_pair_route_rejects_an_untyped_live_relay_nonce() {
        let temporary = TempDir::new();
        committed_identity(temporary.path());
        let store = NonceStore::new(temporary.path());
        let nonce = "relay-a";
        store
            .add_relay(nonce.into(), "phone".into(), "".into(), now())
            .expect("relay window");
        let app = DoorRouter::unconfined(
            Router::new()
                .route("/pair", axum::routing::post(pair))
                .layer(Extension(Arc::new(JournalRoot(
                    temporary.path().to_path_buf(),
                )))),
        )
        .into_inner();
        let mut request = Request::post(format!("/pair?token={nonce}"))
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"csr":"not reached","device_label":"phone"}"#,
            ))
            .expect("request");
        request.extensions_mut().insert(AccessBasis::PairingPeer {
            carrier: Carrier::ViaSpl,
        });

        let response = app.oneshot(request).await.expect("response");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(
            relay_pairing_nonce_open(&store, nonce, now()),
            "untyped rejection does not consume the live relay nonce"
        );
        assert!(
            AuthorizationLedger::new(temporary.path())
                .snapshot()
                .is_empty(),
            "untyped rejection does not issue a certificate or mutate the ledger"
        );
    }

    #[tokio::test]
    async fn no_usable_pairing_address_uses_the_declared_wire_reason_code() {
        let response = pairing_refusal(PairingError::PairingRequestInvalid(
            "no usable local address is available for pairing",
        ));
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("refusal body");
        let value: Value = serde_json::from_slice(&body).expect("refusal JSON");
        assert_eq!(value["reason_code"], "pairing_request_invalid");
    }

    #[tokio::test]
    async fn same_machine_mint_rejects_each_forwarded_header_without_persisting_then_succeeds() {
        let temporary = TempDir::new();
        committed_identity(temporary.path());
        let root = Arc::new(JournalRoot(temporary.path().to_path_buf()));
        let app = Router::new()
            .route("/start", axum::routing::post(pair_start))
            .layer(Extension(AccessBasis::Localhost))
            .layer(Extension(root));
        let payload = br#"{"device_label":"phone","same_machine":true}"#;
        let non_loopback = Router::new()
            .route("/start", axum::routing::post(pair_start))
            .layer(Extension(AccessBasis::PairingPeer {
                carrier: Carrier::Direct,
            }))
            .layer(Extension(Arc::new(JournalRoot(
                temporary.path().to_path_buf(),
            ))));
        let response = non_loopback
            .oneshot(
                Request::post("/start")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.as_slice()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!temporary.path().join("link/nonces.json").exists());
        for invalid in [
            br#"{"device_label":"phone","role":"unknown"}"#.as_slice(),
            br#"{"device_label":"phone","same_machine":"true"}"#.as_slice(),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::post("/start")
                        .header("content-type", "application/json")
                        .body(Body::from(invalid))
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert!(!temporary.path().join("link/nonces.json").exists());
        }
        fs::write(
            temporary.path().join("config/journal.json"),
            r#"{"link":{"posture":"spl"},"pairing":{"home_address":"10.0.0.2:7657"}}"#,
        )
        .expect("SPL posture");
        let response = app
            .clone()
            .oneshot(
                Request::post("/start")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        br#"{"device_label":"phone","same_machine":false}"#.as_slice(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let refusal: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("posture refusal body"),
        )
        .expect("posture refusal JSON");
        assert_eq!(refusal["reason_code"], "relay_pairing_unavailable");
        assert!(!temporary.path().join("link/nonces.json").exists());
        fs::write(
            temporary.path().join("config/journal.json"),
            r#"{"pairing":{"home_address":"10.0.0.2:7657"}}"#,
        )
        .expect("direct posture");
        for header in ["x-forwarded-for", "x-real-ip", "x-forwarded-host"] {
            let response = app
                .clone()
                .oneshot(
                    Request::post("/start")
                        .header("content-type", "application/json")
                        .header(header, "spoofed")
                        .body(Body::from(payload.as_slice()))
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{header}");
            assert!(
                !temporary.path().join("link/nonces.json").exists(),
                "{header} persisted no nonce"
            );
        }
        let response = app
            .clone()
            .oneshot(
                Request::post("/start")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.as_slice()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let omitted = app
            .oneshot(
                Request::post("/start")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(omitted.status(), StatusCode::OK);
        let omitted: Value = serde_json::from_slice(
            &to_bytes(omitted.into_body(), usize::MAX)
                .await
                .expect("omitted-label body"),
        )
        .expect("omitted-label JSON");
        assert!(
            omitted["nonce"]
                .as_str()
                .is_some_and(|nonce| !nonce.is_empty()),
            "omitted device_label must still mint: {omitted}"
        );
        assert_eq!(omitted["device_label"], "");
        let value: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(
            value
                .as_object()
                .expect("mint object")
                .keys()
                .collect::<Vec<_>>(),
            [
                "nonce",
                "pair_link",
                "expires_in",
                "device_label",
                "ca_fingerprint"
            ]
        );
        assert_eq!(value["expires_in"], 300);
        assert_eq!(value["device_label"], "phone");
        assert!(
            NonceStore::new(temporary.path())
                .peek(value["nonce"].as_str().expect("nonce"))
                .expect("stored nonce")
                .same_machine
        );
    }

    #[test]
    fn relay_mode_is_only_spl_and_off_machine() {
        let temporary = TempDir::new();
        fs::create_dir_all(temporary.path().join("config")).expect("config directory");
        let mut request = MintRequest {
            device_label: "phone".to_owned(),
            role: "observer".to_owned(),
            same_machine: Some(false),
            hardened_loopback: false,
            configured_home: None,
        };

        fs::write(
            temporary.path().join("config/journal.json"),
            r#"{"link":{"posture":"spl"}}"#,
        )
        .expect("SPL posture");
        assert!(uses_relay_pairing(temporary.path(), &request));
        request.same_machine = Some(true);
        assert!(!uses_relay_pairing(temporary.path(), &request));

        request.same_machine = Some(false);
        fs::write(
            temporary.path().join("config/journal.json"),
            r#"{"link":{"posture":"direct"}}"#,
        )
        .expect("direct posture");
        assert!(!uses_relay_pairing(temporary.path(), &request));
    }

    #[test]
    fn configured_home_requires_the_journal_direct_port() {
        let temporary = TempDir::new();
        fs::create_dir_all(temporary.path().join("config")).expect("config directory");
        fs::write(
            temporary.path().join("config/journal.json"),
            r#"{"pairing":{"home_address":"10.0.0.2:9000","direct_port":9000}}"#,
        )
        .expect("custom port config");
        assert_eq!(
            configured_home(temporary.path()),
            Some(Ipv4Addr::new(10, 0, 0, 2))
        );
        fs::write(
            temporary.path().join("config/journal.json"),
            r#"{"pairing":{"home_address":"10.0.0.2:7657","direct_port":9000}}"#,
        )
        .expect("mismatched port config");
        assert_eq!(configured_home(temporary.path()), None);
    }

    #[tokio::test]
    async fn devices_route_projects_the_exact_network_device_vocabulary() {
        let temporary = TempDir::new();
        fs::create_dir_all(temporary.path().join("config")).expect("config directory");
        fs::write(
            temporary.path().join("config/journal.json"),
            r#"{"setup":{"completed_at":1}}"#,
        )
        .expect("established journal");
        fs::create_dir_all(temporary.path().join("link")).expect("link directory");
        fs::write(
            temporary.path().join("link/authorized_clients.json"),
            json!([{
                "fingerprint": "sha256:0123456789abcdef0123456789abcdef",
                "device_label": "phone",
                "paired_at": "2026-08-13T00:00:00Z",
                "instance_id": "device-instance",
                "role": "owner",
                "network": "home",
                "client_label": "Phone",
                "kind": "cert",
            }])
            .to_string(),
        )
        .expect("authorization ledger");
        fs::write(
            temporary.path().join("link/devices.json"),
            json!({
                "sha256:0123456789abcdef0123456789abcdef": {
                    "last_seen_at": "2026-08-13T00:01:00Z"
                }
            })
            .to_string(),
        )
        .expect("activity metadata");

        for prefix in NETWORK_ROUTE_PREFIXES {
            let response = crate::router(temporary.path().to_path_buf())
                .oneshot(
                    Request::get(format!("{prefix}/api/devices"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::OK, "{prefix}");
            let body: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body"),
            )
            .expect("JSON");
            let device = &body["devices"][0];
            let actual = device
                .as_object()
                .expect("device object")
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>();
            let expected = NETWORK_DEVICE_FIELDS
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(
                actual, expected,
                "{prefix}: the projection neither widens nor narrows"
            );
            assert_eq!(device["last_seen_at"], "2026-08-13T00:01:00Z", "{prefix}");
            assert!(
                device.get("observer_handle").is_none(),
                "{prefix}: legacy handles are no longer exposed"
            );
        }
    }

    #[tokio::test]
    async fn devices_unavailable_ledgers_are_service_unavailable_not_empty_lists() {
        let temporary = TempDir::new();
        established_journal(temporary.path());
        fs::create_dir_all(temporary.path().join("link")).expect("link directory");
        let app = crate::router(temporary.path().to_path_buf());

        fs::create_dir_all(temporary.path().join("link/authorized_clients.json"))
            .expect("unreadable ledger");
        for prefix in NETWORK_ROUTE_PREFIXES {
            let (status, body) = request_json(app.clone(), &format!("{prefix}/api/devices")).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{prefix}");
            assert_eq!(body["reason_code"], "authorization_ledger_unreadable");
            assert!(body.get("devices").is_none(), "{prefix}");
        }

        fs::remove_dir_all(temporary.path().join("link/authorized_clients.json"))
            .expect("remove unreadable");
        fs::write(temporary.path().join("link/authorized_clients.json"), "{")
            .expect("malformed ledger");
        for prefix in NETWORK_ROUTE_PREFIXES {
            let (status, body) = request_json(app.clone(), &format!("{prefix}/api/devices")).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{prefix}");
            assert_eq!(body["reason_code"], "authorization_ledger_malformed");
            assert!(body.get("devices").is_none(), "{prefix}");
        }

        fs::write(
            temporary.path().join("link/authorized_clients.json"),
            json!([
                {"fingerprint":"a","device_label":"one","paired_at":"1","instance_id":"i"},
                {"fingerprint":"a","device_label":"two","paired_at":"2","instance_id":"i"}
            ])
            .to_string(),
        )
        .expect("duplicate ledger");
        for prefix in NETWORK_ROUTE_PREFIXES {
            let (status, body) = request_json(app.clone(), &format!("{prefix}/api/devices")).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{prefix}");
            assert_eq!(body["reason_code"], "authorization_ledger_duplicate_cid");
            assert!(body.get("devices").is_none(), "{prefix}");
        }
    }

    #[tokio::test]
    async fn devices_projection_ignores_pairing_identity_reader_states() {
        let cid = "sha256:0123456789abcdef0123456789abcdef";
        let expected_keys = NETWORK_DEVICE_FIELDS
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        for (client_label, expected) in [
            (None, json!("")),
            (Some(json!("")), json!("")),
            (Some(json!("Phone")), json!("Phone")),
            (Some(json!(1)), json!("")),
        ] {
            let temporary = TempDir::new();
            established_journal(temporary.path());
            fs::create_dir_all(temporary.path().join("link")).expect("link directory");
            let mut entry = json!({
                "fingerprint": cid,
                "device_label": "phone",
                "paired_at": "2026-08-13T00:00:00Z",
                "instance_id": "device-instance",
                "role": "owner",
                "kind": "cert",
                "platform": "linux",
            });
            match client_label {
                None => {}
                Some(value) => {
                    entry
                        .as_object_mut()
                        .expect("object")
                        .insert("client_label".to_owned(), value);
                }
            }
            fs::write(
                temporary.path().join("link/authorized_clients.json"),
                json!([entry]).to_string(),
            )
            .expect("ledger");
            let (status, body) = request_json(
                crate::router(temporary.path().to_path_buf()),
                "/app/network/api/devices",
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let row = body["devices"][0].as_object().expect("device row");
            assert_eq!(
                row.keys()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>(),
                expected_keys
            );
            assert_eq!(row["client_label"], expected);
            assert!(!row.contains_key("platform"));
        }
    }

    async fn request_json(app: Router, path: &str) -> (StatusCode, Value) {
        let response = app
            .oneshot(
                Request::get(path)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn network_native_read_routes_serve_established_assets_and_state() {
        let temporary = TempDir::new();
        established_journal(temporary.path());
        let app = crate::router(temporary.path().to_path_buf());
        for prefix in NETWORK_ROUTE_PREFIXES {
            for (suffix, content_type) in [
                ("/", "text/html; charset=utf-8"),
                ("/workspace", "text/html; charset=utf-8"),
                ("/static/network.js", "text/javascript; charset=utf-8"),
                ("/api/state", "application/json"),
            ] {
                let path = format!("{prefix}{suffix}");
                let response = get_response(app.clone(), &path).await;
                assert_eq!(response.status(), StatusCode::OK, "{path}");
                assert_eq!(
                    response.headers()[header::CONTENT_TYPE],
                    content_type,
                    "{path}"
                );
                if suffix == "/api/state" {
                    let body: Value = serde_json::from_slice(
                        &to_bytes(response.into_body(), usize::MAX)
                            .await
                            .expect("state body reads"),
                    )
                    .expect("state is JSON");
                    assert_eq!(body["posture"], "direct");
                    assert!(
                        body["link_copy"]
                            .as_object()
                            .is_some_and(|copy| !copy.is_empty()),
                        "state has the generated copy payload"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn network_state_reports_spl_posture_from_established_journal() {
        let temporary = TempDir::new();
        established_journal(temporary.path());
        fs::write(
            temporary.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1},"link":{"posture":"spl"}}"#,
        )
        .expect("SPL established config");

        let response = get_response(
            crate::router(temporary.path().to_path_buf()),
            "/app/link/api/state",
        )
        .await;
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("state body reads"),
        )
        .expect("state is JSON");

        assert_eq!(body["posture"], "spl");
    }

    #[tokio::test]
    async fn network_workspace_and_script_obey_the_session_gate_in_all_phases() {
        for phase in ["unestablished", "established", "corrupt"] {
            let temporary = TempDir::new();
            match phase {
                "unestablished" => {}
                "established" => established_journal(temporary.path()),
                "corrupt" => {
                    fs::create_dir_all(temporary.path().join("config")).expect("config directory");
                    fs::write(
                        temporary.path().join("config/journal.json"),
                        br#"{"setup":{"completed_at":1"#,
                    )
                    .expect("corrupt config");
                }
                _ => unreachable!("known phase"),
            }
            for path in [
                "/app/network/workspace",
                "/app/network/static/network.js",
                "/app/link/workspace",
                "/app/link/static/network.js",
            ] {
                let response =
                    get_response(crate::router(temporary.path().to_path_buf()), path).await;
                match phase {
                    "unestablished" => {
                        assert_eq!(response.status(), StatusCode::FOUND, "{path}");
                        assert_eq!(response.headers()[header::LOCATION], "/init", "{path}");
                    }
                    "established" => assert_eq!(response.status(), StatusCode::OK, "{path}"),
                    "corrupt" => {
                        assert_eq!(
                            response.status(),
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "{path}"
                        )
                    }
                    _ => unreachable!("known phase"),
                }
            }
        }
    }

    #[test]
    fn network_router_is_fallback_free_and_shell_constructs() {
        let temporary = TempDir::new();
        let root = Arc::new(JournalRoot(temporary.path().to_path_buf()));
        let _merged = Router::new()
            .route("/x", get(|| async { StatusCode::OK }))
            .fallback(|| async { StatusCode::NOT_FOUND })
            .merge(super::router(
                root,
                "/app/network",
                Arc::new(OperationRegistry::default()),
                Arc::new(PairWindowManager::new(Arc::new(
                    crate::relay_admission::RelayAdmissionRegistry::new(),
                ))),
            ));
        let _shell = crate::router(temporary.path().to_path_buf());
    }

    async fn post_json(app: Router, path: &str, body: Value) -> (StatusCode, Value) {
        let response = app
            .oneshot(
                Request::post(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        let status = response.status();
        let parsed = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .unwrap_or(Value::Null);
        (status, parsed)
    }

    fn write_authorized_clients(root: &Path, entries: Value) {
        fs::create_dir_all(root.join("link")).expect("link directory");
        fs::write(
            root.join("link/authorized_clients.json"),
            entries.to_string(),
        )
        .expect("authorization ledger");
    }

    fn one_client() -> Value {
        json!([{
            "fingerprint": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "device_label": "phone",
            "paired_at": "2026-08-13T00:00:00Z",
            "instance_id": "device-instance",
            "role": "peer",
            "kind": "cert",
        }])
    }

    #[tokio::test]
    async fn aliased_network_routes_are_registered_on_both_prefixes() {
        let temporary = TempDir::new();
        established_journal(temporary.path());
        write_authorized_clients(temporary.path(), one_client());
        committed_identity(temporary.path());
        let app = crate::router(temporary.path().to_path_buf());
        for prefix in NETWORK_ROUTE_PREFIXES {
            for path in [
                format!("{prefix}/"),
                format!("{prefix}/workspace"),
                format!("{prefix}/static/network.js"),
                format!("{prefix}/api/state"),
                format!("{prefix}/api/status"),
                format!("{prefix}/api/identity"),
                format!("{prefix}/api/private-link"),
                format!("{prefix}/api/devices"),
                format!("{prefix}/api/clients"),
            ] {
                let response = get_response(app.clone(), &path).await;
                assert_ne!(response.status(), StatusCode::NOT_FOUND, "{path}");
            }
            let mut local = Request::get(format!("{prefix}/local-endpoints"))
                .body(Body::empty())
                .expect("request");
            local.extensions_mut().insert(AccessBasis::Localhost);
            let response = app.clone().oneshot(local).await.expect("response");
            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{prefix}/local-endpoints"
            );
            for (path, body) in [
                (
                    format!("{prefix}/pair-start"),
                    json!({"device_label": "phone"}),
                ),
                (format!("{prefix}/unpair"), json!({})),
                (format!("{prefix}/host-address"), json!({})),
                (format!("{prefix}/private-link/enable"), json!({})),
                (format!("{prefix}/private-link/disable"), json!({})),
            ] {
                let mut request = Request::post(&path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request");
                request.extensions_mut().insert(AccessBasis::Localhost);
                let response = app.clone().oneshot(request).await.expect("response");
                assert_ne!(response.status(), StatusCode::NOT_FOUND, "{path}");
            }
            let mut nonce = Request::get(format!("{prefix}/api/pair/nonce-status?nonce=x"))
                .body(Body::empty())
                .expect("request");
            nonce.extensions_mut().insert(AccessBasis::Localhost);
            let response = app.clone().oneshot(nonce).await.expect("response");
            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{prefix}/api/pair/nonce-status"
            );
            let mut pair = Request::post(format!("{prefix}/pair"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"csr":"x","device_label":"phone"}"#))
                .expect("request");
            pair.extensions_mut().insert(AccessBasis::PairingPeer {
                carrier: Carrier::Direct,
            });
            let response = app.clone().oneshot(pair).await.expect("response");
            assert_ne!(response.status(), StatusCode::NOT_FOUND, "{prefix}/pair");
        }
    }

    #[tokio::test]
    async fn unpair_follows_the_fingerprint_and_label_decision_table() {
        let fingerprint = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let temporary = TempDir::new();
        established_journal(temporary.path());
        write_authorized_clients(temporary.path(), one_client());
        let app = crate::router(temporary.path().to_path_buf());

        let (status, body) = post_json(app.clone(), "/app/network/unpair", json!({})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["reason_code"], "missing_required_field");

        let (status, body) = post_json(
            app.clone(),
            "/app/link/unpair",
            json!({"fingerprint": "sha256:missing"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["reason_code"], "paired_device_not_found");

        let (status, body) = post_json(
            app.clone(),
            "/app/network/unpair",
            json!({"device_label": "nope"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["reason_code"], "paired_device_not_found");
        assert_eq!(
            fs::read_to_string(temporary.path().join("link/authorized_clients.json")).unwrap(),
            one_client().to_string()
        );

        let (status, body) =
            post_json(app, "/app/link/unpair", json!({"device_label": "phone"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"unpaired": fingerprint}));

        let temporary = TempDir::new();
        established_journal(temporary.path());
        write_authorized_clients(temporary.path(), one_client());
        let (status, body) = post_json(
            crate::router(temporary.path().to_path_buf()),
            "/app/network/unpair",
            json!({"fingerprint": fingerprint, "device_label": "ignored"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["unpaired"], fingerprint);
        assert!(body.get("revoked_observers").is_none());

        let temporary = TempDir::new();
        established_journal(temporary.path());
        write_authorized_clients(
            temporary.path(),
            json!([
                {
                    "fingerprint": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "device_label": "phone",
                    "paired_at": "2026-08-13T00:00:00Z",
                    "instance_id": "a",
                    "kind": "cert",
                },
                {
                    "fingerprint": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "device_label": "phone",
                    "paired_at": "2026-08-13T00:00:01Z",
                    "instance_id": "b",
                    "kind": "cert",
                }
            ]),
        );
        let before = fs::read(temporary.path().join("link/authorized_clients.json")).unwrap();
        let (status, body) = post_json(
            crate::router(temporary.path().to_path_buf()),
            "/app/network/unpair",
            json!({"device_label": "phone"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["reason_code"], "invalid_operation_for_state");
        assert_eq!(
            fs::read(temporary.path().join("link/authorized_clients.json")).unwrap(),
            before
        );

        let unreadable = TempDir::new();
        established_journal(unreadable.path());
        fs::create_dir_all(unreadable.path().join("link/authorized_clients.json"))
            .expect("unreadable ledger");
        let (status, body) = post_json(
            crate::router(unreadable.path().to_path_buf()),
            "/app/link/unpair",
            json!({"device_label": "phone"}),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason_code"], "authorization_ledger_unreadable");

        let malformed = TempDir::new();
        established_journal(malformed.path());
        fs::create_dir_all(malformed.path().join("link")).expect("link directory");
        fs::write(
            malformed.path().join("link/authorized_clients.json"),
            "{not json",
        )
        .expect("malformed ledger");
        let (status, body) = post_json(
            crate::router(malformed.path().to_path_buf()),
            "/app/network/unpair",
            json!({"fingerprint": fingerprint}),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason_code"], "authorization_ledger_malformed");
    }

    fn ledger_path(root: &Path) -> std::path::PathBuf {
        root.join("link/authorized_clients.json")
    }

    fn ledger_fingerprints(root: &Path) -> Vec<String> {
        serde_json::from_slice::<Value>(&fs::read(ledger_path(root)).expect("ledger reads"))
            .expect("ledger JSON")
            .as_array()
            .expect("ledger array")
            .iter()
            .map(|entry| {
                entry["fingerprint"]
                    .as_str()
                    .expect("fingerprint")
                    .to_owned()
            })
            .collect()
    }

    #[tokio::test]
    async fn unpair_removes_only_the_targeted_client_and_refuses_a_second_unpair() {
        let client_a = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let client_b = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let temporary = TempDir::new();
        established_journal(temporary.path());
        write_authorized_clients(
            temporary.path(),
            json!([
                {
                    "fingerprint": client_a,
                    "device_label": "phone",
                    "paired_at": "2026-08-13T00:00:00Z",
                    "instance_id": "a",
                    "kind": "cert",
                },
                {
                    "fingerprint": client_b,
                    "device_label": "laptop",
                    "paired_at": "2026-08-13T00:00:01Z",
                    "instance_id": "b",
                    "kind": "cert",
                }
            ]),
        );
        let app = crate::router(temporary.path().to_path_buf());

        let (status, body) = post_json(
            app.clone(),
            "/app/network/unpair",
            json!({"fingerprint": client_a}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"unpaired": client_a}));
        assert_eq!(ledger_fingerprints(temporary.path()), [client_b]);

        let after_first = fs::read(ledger_path(temporary.path())).expect("ledger after first");
        let (status, body) =
            post_json(app, "/app/network/unpair", json!({"fingerprint": client_a})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_ne!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["reason_code"], "paired_device_not_found");
        assert_eq!(
            fs::read(ledger_path(temporary.path())).expect("ledger after second"),
            after_first
        );
        assert_eq!(ledger_fingerprints(temporary.path()), [client_b]);
    }

    #[tokio::test]
    async fn unpair_matches_computed_display_label_not_the_raw_field() {
        let ordinal = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let temporary = TempDir::new();
        established_journal(temporary.path());
        write_authorized_clients(
            temporary.path(),
            json!([{
                "fingerprint": ordinal,
                "device_label": "iPhone",
                "label_ordinal": 2,
                "paired_at": "2026-08-13T00:00:00Z",
                "instance_id": "c",
                "kind": "cert",
            }]),
        );
        let (status, body) = post_json(
            crate::router(temporary.path().to_path_buf()),
            "/app/link/unpair",
            json!({"device_label": "iPhone (2)"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"unpaired": ordinal}));
        assert!(ledger_fingerprints(temporary.path()).is_empty());

        let raw = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
        let computed = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        let temporary = TempDir::new();
        established_journal(temporary.path());
        write_authorized_clients(
            temporary.path(),
            json!([
                {
                    "fingerprint": raw,
                    "device_label": "iPhone (2)",
                    "label_ordinal": 1,
                    "paired_at": "2026-08-13T00:00:00Z",
                    "instance_id": "d",
                    "kind": "cert",
                },
                {
                    "fingerprint": computed,
                    "device_label": "iPhone",
                    "label_ordinal": 2,
                    "paired_at": "2026-08-13T00:00:01Z",
                    "instance_id": "e",
                    "kind": "cert",
                }
            ]),
        );
        let before = fs::read(ledger_path(temporary.path())).expect("ledger before collision");
        let (status, body) = post_json(
            crate::router(temporary.path().to_path_buf()),
            "/app/network/unpair",
            json!({"device_label": "iPhone (2)"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["reason_code"], "invalid_operation_for_state");
        assert_eq!(
            fs::read(ledger_path(temporary.path())).expect("ledger after collision"),
            before
        );
    }

    #[tokio::test]
    async fn unpair_missing_ledger_file_is_not_found_and_does_not_create_one() {
        let temporary = TempDir::new();
        established_journal(temporary.path());
        let path = ledger_path(temporary.path());
        assert!(!path.exists());
        let (status, body) = post_json(
            crate::router(temporary.path().to_path_buf()),
            "/app/network/unpair",
            json!({"fingerprint": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_ne!(status, StatusCode::NOT_FOUND);
        assert_ne!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason_code"], "paired_device_not_found");
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn init_is_reachable_on_the_shared_router_when_unestablished() {
        let temporary = TempDir::new();
        let app = crate::router(temporary.path().to_path_buf());
        for path in ["/init", "/init/api/state", "/init/mark"] {
            let mut request = Request::get(path).body(Body::empty()).expect("request");
            request.extensions_mut().insert(AccessBasis::Localhost);
            let response = app.clone().oneshot(request).await.expect("response");
            assert_ne!(response.status(), StatusCode::FOUND, "{path}");
            assert_ne!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }
}
