// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native network read routes and direct device-pairing handlers.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::Router;
use axum::extract::{Extension, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_sol_link::ledger::{
    AuthorizationLedger, AuthorizedClientsRead, DeviceActivityRead, read_authorized_clients,
    read_device_activity,
};
use solstone_core_sol_link::pairing::addresses::{
    PairingSnapshot, SystemInterfaceSource, SystemRouteIpv4Source, snapshot_from_sources,
};
use solstone_core_sol_link::pairing::nonces::NonceStore;
use solstone_core_sol_link::pairing::{
    CeremonyRequest, MintRequest, PairingError, complete_pairing, mint_pairing, pair_response_json,
};

use crate::network_status::{identity, local_endpoints, private_link, status};

use crate::{JournalRoot, asset_response, assets};

/// Exact network-device response vocabulary mirrored from
/// `solstone/apps/network/routes.py::_entry_to_json`.
pub(crate) const NETWORK_DEVICE_FIELDS: [&str; 11] = [
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
    "observer_handle",
];

#[derive(Deserialize)]
pub(crate) struct NonceQuery {
    nonce: String,
}

#[derive(Deserialize)]
pub(crate) struct PairTokenQuery {
    token: Option<String>,
}

pub fn router(journal: Arc<JournalRoot>) -> Router {
    Router::new()
        .route("/app/network/", get(shell))
        .route("/app/network/workspace", get(workspace))
        .route("/app/network/static/network.js", get(script))
        .route("/app/network/api/state", get(state))
        .route("/app/network/api/status", get(status))
        .route("/app/network/api/identity", get(identity))
        .route("/app/network/api/private-link", get(private_link))
        .route("/app/network/local-endpoints", get(local_endpoints))
        .layer(Extension(journal))
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
    let Some(device_label) = object.get("device_label").and_then(Value::as_str) else {
        return refusal(
            "missing_required_field",
            "device_label is required",
            StatusCode::BAD_REQUEST,
        );
    };
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
    match mint_pairing(&root.0, &request, now()) {
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
        AuthorizedClientsRead::Unreadable | AuthorizedClientsRead::Malformed => {
            log::warn!("network devices could not read the authorization ledger");
            Vec::new()
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

fn network_device_json(
    entry: &solstone_core_sol_link::ledger::ClientEntry,
    activity: Option<&Map<String, Value>>,
) -> Value {
    let last_seen_at = activity
        .and_then(|devices| devices.get(&entry.fingerprint))
        .and_then(Value::as_object)
        .and_then(|device| device.get("last_seen_at"))
        .cloned()
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
        // Verified from `solstone/think/link/auth.py`: the loader never
        // populates this field (line 322) and the writer omits it (line 358),
        // so every reloaded reference entry has `None` here.
        ("observer_handle".to_owned(), Value::Null),
    ]);
    debug_assert_eq!(value.len(), NETWORK_DEVICE_FIELDS.len());
    Value::Object(value)
}

pub(crate) async fn pair(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
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
            local_endpoints: response_local_endpoints(&snapshot),
        },
        now(),
    ) {
        Ok(response) => match pair_response_json(&response) {
            Ok(value) => {
                emit_pair_complete(&root.0, &response.fingerprint);
                Json(value).into_response()
            }
            Err(error) => pairing_refusal(error),
        },
        Err(error) => pairing_refusal(error),
    }
}

fn response_local_endpoints(snapshot: &PairingSnapshot) -> Option<Value> {
    (!snapshot.endpoints.is_empty()).then(|| {
        Value::Array(
            snapshot
                .endpoints
                .iter()
                .map(|endpoint| {
                    json!({
                        "ip": endpoint.ip.to_string(),
                        "port": spl_core::DEFAULT_DIRECT_PORT,
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
    let contents = std::fs::read(journal_root.join("config/journal.json")).ok()?;
    let value: Value = serde_json::from_slice(&contents).ok()?;
    let address = value.get("pairing")?.get("home_address")?.as_str()?;
    let (host, port) = address.rsplit_once(':')?;
    (port.parse::<u16>().ok() == Some(spl_core::DEFAULT_DIRECT_PORT))
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

fn refusal(reason_code: &str, detail: &str, status: StatusCode) -> Response {
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
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, header};
    use axum::routing::get;
    use axum::{Extension, Router};
    use solstone_core_convey_http::identity::{AccessBasis, Carrier};
    use solstone_core_sol_link::ca::{generate_ca, jid_from_spki};
    use tower::ServiceExt;

    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    const NETWORK_WORKSPACE: &str = include_str!("../assets/network/workspace.html");
    const NETWORK_SCRIPT: &str = include_str!("../assets/network/network.js");

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("solstone-network-{nanos}-{sequence}"));
            fs::create_dir(&path).expect("temporary root");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
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
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let refusal: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("posture refusal body"),
        )
        .expect("posture refusal JSON");
        assert_eq!(refusal["reason_code"], "invalid_operation_for_state");
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
            .oneshot(
                Request::post("/start")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.as_slice()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
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

        let response = crate::router(temporary.path().to_path_buf())
            .oneshot(
                Request::get("/app/network/api/devices")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
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
            "the projection neither widens nor narrows"
        );
        assert_eq!(device["last_seen_at"], "2026-08-13T00:01:00Z");
        assert_eq!(device["observer_handle"], Value::Null);
    }

    #[tokio::test]
    async fn network_native_read_routes_serve_established_assets_and_state() {
        let temporary = TempDir::new();
        established_journal(temporary.path());
        let app = crate::router(temporary.path().to_path_buf());
        for (path, content_type) in [
            ("/app/network/", "text/html; charset=utf-8"),
            ("/app/network/workspace", "text/html; charset=utf-8"),
            (
                "/app/network/static/network.js",
                "text/javascript; charset=utf-8",
            ),
            ("/app/network/api/state", "application/json"),
        ] {
            let response = get_response(app.clone(), path).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                content_type,
                "{path}"
            );
            if path == "/app/network/api/state" {
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
            "/app/network/api/state",
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
            for path in ["/app/network/workspace", "/app/network/static/network.js"] {
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
            .merge(super::router(root));
        let _shell = crate::router(temporary.path().to_path_buf());
    }

    #[test]
    fn network_shrunk_workspace_keeps_the_required_surface() {
        for expected in [
            "link-workspace-root",
            "link-state-gate",
            "link-dashboard-content",
            "link-identity-header",
            "link-identity-mark",
            "link-identity-id-value",
            "link-identity-id-copy",
            "link-status-panel",
            "link-status-dot",
            "link-status-skeleton",
            "link-status-text",
            "link-reach-selector",
            "link-reach-selector-title",
            "link-seg-byo",
            "link-seg-hosted",
            "link-mode-byo-body",
            "link-home-address-row",
            "link-home-address",
            "link-vpn-candidates-row",
            "link-vpn-candidates",
            "link-home-candidates-picker",
            "link-home-candidates-list",
            "link-home-candidates-problem",
            "link-host-address-override",
            "link-host-address-input",
            "link-host-address-apply",
            "link-host-address-clear",
            "link-host-address-error",
            "link-mode-hosted-setup",
            "link-private-link-setup",
            "link-mode-hosted-active",
            "link-private-link-disable",
            "link-spl-connecting-note",
            "link-spl-check-again",
            "link-private-link-operation",
            "link-private-link-operation-headline",
            "link-private-link-operation-detail",
            "link-private-link-operation-link",
            "link-private-link-operation-retry",
            "link-toast",
            ".link-app-onoff",
            ".link-app-onoff-sub",
            "data-copy-attr=\"data-byo-sub:APP_ONOFF_SUB_BYO; data-hosted-sub:APP_ONOFF_SUB_HOSTED\"",
            "loadNetworkState",
            "initLinkWorkspace",
            "initLink",
            "applyStatus",
            "refreshStatus",
            "renderReach",
            "renderSelector",
            "renderSplState",
            "updateReachSelectorVisibility",
            "renderVpnCandidates",
            "renderHomeCandidates",
            "selectHomeCandidate",
            "applyHostAddressOverride",
            "clearHostAddressOverride",
            "submitHostAddress",
            "renderPrivateLinkStatus",
            "startPrivateLinkSetup",
            "disablePrivateLink",
            "pollPrivateLinkUntilTerminal",
            "loadIdentity",
            "renderIdentityMark",
            "showToast",
            "revealWorkspace",
            "showWorkspaceStateError",
        ] {
            assert!(NETWORK_WORKSPACE.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn network_shrunk_workspace_drops_pairing_and_device_surface() {
        for forbidden in [
            "link-first-run-hero",
            "link-hero-title",
            "link-hero-pair",
            "link-hero-how-reach",
            "link-paired-section",
            "link-paired-h2",
            "link-devices-count",
            "link-pair-btn",
            "link-devices-list",
            "link-recent-section",
            "link-recent-h2",
            "link-recent-list",
            "link-pair-modal",
            "link-pair-modal-title",
            "link-present-selector",
            "link-present-phone",
            "link-present-computer",
            "link-present-glasses",
            "link-pair-code",
            "link-qr-container",
            "link-qr-expired",
            "link-pair-regenerate",
            "link-pair-network",
            "link-pair-link-label",
            "link-pair-link-input",
            "link-pair-link-copy",
            "link-device-label",
            "link-pair-ca-fp",
            "link-pair-cancel",
            "link-pair-error",
            "link-pair-error-cancel",
            "link-pair-success",
            "link-pair-success-heading",
            "link-pair-success-subhead",
            "link-pair-success-verify",
            "link-pair-success-verify-note",
            "link-pair-remove",
            "link-pair-done",
            "link-unpair-modal",
            "link-unpair-title",
            "link-unpair-confirm",
            "link-unpair-cancel",
            "devicesList",
            "devicesCount",
            "heroSection",
            "heroPairBtn",
            "heroHowReach",
            "pairedSection",
            "recentSection",
            "recentList",
            "pairBtn",
            "pairModal",
            "pairCancel",
            "deviceLabelInput",
            "presentSelector",
            "pairCodeBox",
            "qrContainer",
            "qrExpired",
            "pairLinkInput",
            "pairLinkCopy",
            "pairRegenerate",
            "pairNetwork",
            "caFpEl",
            "pairError",
            "pairErrorCancel",
            "pairSuccess",
            "pairSuccessHeading",
            "pairSuccessSubhead",
            "pairSuccessVerify",
            "pairSuccessVerifyNote",
            "pairRemove",
            "pairDone",
            "unpairModal",
            "unpairTitle",
            "unpairConfirm",
            "unpairCancel",
            "bindPairModalDismiss",
            "handleMenuDocumentClick",
            "handleMenuDocumentKey",
            "presentButtons",
            "refreshDevices",
            "cleanupPairEvents",
            "clearPairTimers",
            ".link-hero",
            ".link-hero-icon",
            ".link-hero-body",
            ".link-hero-actions",
            ".link-hero-how-reach",
            ".link-device-row",
            ".link-skeleton-row",
            ".link-rename-input",
            "/app/network/rename",
            "/app/network/unpair",
            "/app/network/pair-start",
            "/app/network/api/devices",
            "[data-presentation-mode]",
            "applyPosture",
        ] {
            assert!(
                !NETWORK_WORKSPACE.contains(forbidden),
                "unexpected {forbidden}"
            );
        }
    }

    #[test]
    fn network_shrunk_workspace_stays_in_the_size_band() {
        assert!((900..=1800).contains(&NETWORK_WORKSPACE.lines().count()));
    }

    #[test]
    fn network_shrunk_script_keeps_copy_helpers_and_drops_pair_helpers() {
        for expected in [
            "function applyCopy",
            "function findById",
            "const NetworkRender = { applyCopy, resolve };",
        ] {
            assert!(NETWORK_SCRIPT.contains(expected), "missing {expected}");
        }
        for forbidden in ["applyPosture", "bindPairModalDismiss"] {
            assert!(
                !NETWORK_SCRIPT.contains(forbidden),
                "unexpected {forbidden}"
            );
        }
    }
}
