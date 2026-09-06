// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::Serialize;
use solstone_core_convey_http::identity::{AccessBasis, LinkedDeviceCid};
use solstone_core_sol_link::DeviceDoorAuthorization;
use solstone_core_sol_link::ledger::{
    AuthorizationLedger, AuthorizedClientsRead, read_authorized_clients,
};
use solstone_core_sol_link::pairing::nonces::{
    NonceStore, direct_pairing_window_open, relay_pairing_nonce_open,
};
use tokio::sync::watch;

use crate::door::PairingAdmission;

/// Per-router authorization-read instrumentation for black-box contract tests.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct AuthorizationGateReadProbe(Arc<AtomicU64>);

impl AuthorizationGateReadProbe {
    /// Create a zeroed read counter for one instrumented gate instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of linked-device authorization reads by that gate instance.
    pub fn reads(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    fn record(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct AuthorizationGateState {
    pub authorization: watch::Receiver<DeviceDoorAuthorization>,
    pub authorized_clients_path: PathBuf,
    read_probe: Option<AuthorizationGateReadProbe>,
}

#[derive(Clone)]
struct PairingConfinementState {
    journal_root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationExemption {
    Favicon,
    TopLevelStatic,
}

pub const AUTHORIZATION_GATE_EXEMPTIONS: &[AuthorizationExemption] = &[
    AuthorizationExemption::Favicon,
    AuthorizationExemption::TopLevelStatic,
];

pub(crate) const PAIR_PATHS: &[&str] = &[spl_core::PAIR_PATH, "/app/link/pair"];
// [check] Only boot assets bypass request authorization. APIs, app paths, and
// `/sse/events` remain gated: this gate refuses a new SSE request, while
// carrier-level revocation closes an already-open device stream.

#[derive(Serialize)]
struct AuthorizationRefusal {
    error: &'static str,
    reason: &'static str,
    reason_code: &'static str,
    detail: &'static str,
}

/// A router deliberately prepared for the paired-device door.
pub struct DoorRouter(Router);

impl DoorRouter {
    /// Keep the non-confining library test surface explicit at the type boundary.
    pub fn unconfined(router: Router) -> Self {
        Self(router)
    }

    /// Deliberately unwrap a door router for a non-door surface or direct router test.
    pub fn into_inner(self) -> Router {
        self.0
    }
}

/// Build the production router whose paired-device requests are rechecked per request.
pub fn authorized_router(
    journal_root: PathBuf,
    authorization: watch::Receiver<DeviceDoorAuthorization>,
) -> DoorRouter {
    authorized_router_with_router_inner(
        crate::router(journal_root.clone()),
        journal_root,
        authorization,
        None,
    )
}

/// Build an authorization gate with a counter scoped to this router instance.
#[doc(hidden)]
pub fn authorized_router_with_read_probe(
    journal_root: PathBuf,
    authorization: watch::Receiver<DeviceDoorAuthorization>,
    read_probe: AuthorizationGateReadProbe,
) -> DoorRouter {
    authorized_router_with_router_inner(
        crate::router(journal_root.clone()),
        journal_root,
        authorization,
        Some(read_probe),
    )
}

/// Apply the door-only production layers to a prebuilt router. Loopback is
/// structurally excluded because the wrapper cannot become a plain router
/// without an explicit `into_inner` call.
pub fn authorized_router_with_router(
    router: Router,
    journal_root: PathBuf,
    authorization: watch::Receiver<DeviceDoorAuthorization>,
) -> DoorRouter {
    authorized_router_with_router_inner(router, journal_root, authorization, None)
}

fn authorized_router_with_router_inner(
    router: Router,
    journal_root: PathBuf,
    authorization: watch::Receiver<DeviceDoorAuthorization>,
    read_probe: Option<AuthorizationGateReadProbe>,
) -> DoorRouter {
    let authorized_clients_path = AuthorizationLedger::new(&journal_root)
        .authorized_clients_path()
        .to_path_buf();
    // [check] Axum 0.8.9 `Router::route_layer` runs only for a matching route,
    // so router()'s existing fallback still returns 404 rather than this gate's
    // 403. It still applies before a matched route's 405 method response; using
    // `route_layer`, not `layer`, preserves strict-slash and unknown-path 404s.
    // Applying it after router() has composed every route wraps the full
    // surface, including routes from merged sub-routers.
    DoorRouter(
        router
            .route_layer(middleware::from_fn_with_state(
                AuthorizationGateState {
                    authorization,
                    authorized_clients_path,
                    read_probe,
                },
                require_authorization,
            ))
            .layer(middleware::from_fn_with_state(
                PairingConfinementState { journal_root },
                require_pairing_confinement,
            )),
    )
}

async fn require_pairing_confinement(
    State(state): State<PairingConfinementState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let pairing_peer = matches!(
        request.extensions().get::<AccessBasis>(),
        Some(AccessBasis::PairingPeer { .. })
    );
    if !pairing_peer {
        return next.run(request).await;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_secs()
        .try_into()
        .expect("Unix seconds fit i64");
    let store = NonceStore::new(&state.journal_root);
    let pairing_admitted = match request.extensions().get::<PairingAdmission>() {
        Some(PairingAdmission::Direct) => true,
        Some(PairingAdmission::Relay(identity)) => {
            relay_pairing_nonce_open(&store, identity.nonce_value(), now)
        }
        None => direct_pairing_window_open(&store, now),
    };
    if !pairing_admitted {
        return pairing_confinement_response("pairing window closed");
    }
    let raw_path = request.uri().path();
    let decoded = percent_encoding::percent_decode_str(raw_path).decode_utf8();
    if decoded.as_deref().ok() != Some(raw_path) {
        return pairing_confinement_response("pairing tunnel may only use /app/network/pair");
    }
    if !PAIR_PATHS.contains(&raw_path) || request.method() != Method::POST {
        return pairing_confinement_response("pairing tunnel may only use /app/network/pair");
    }
    next.run(request).await
}

fn pairing_confinement_response(body: &'static str) -> Response {
    (StatusCode::FORBIDDEN, body).into_response()
}

fn is_exempt(path: &str) -> bool {
    AUTHORIZATION_GATE_EXEMPTIONS
        .iter()
        .any(|exemption| match exemption {
            AuthorizationExemption::Favicon => path == "/favicon.ico",
            AuthorizationExemption::TopLevelStatic => path.starts_with("/static/"),
        })
}

fn is_authorized(posture: &AuthorizedClientsRead, cid: &LinkedDeviceCid) -> bool {
    matches!(posture, AuthorizedClientsRead::Present(entries) if entries.iter().any(|entry| entry.fingerprint == cid.as_str()))
}

fn pl_revoked_response() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(AuthorizationRefusal {
            error: "that paired device couldn't be used because it was revoked.",
            reason: "pl_revoked",
            reason_code: "pl_revoked",
            detail: "paired device revoked",
        }),
    )
        .into_response()
}

async fn require_authorization(
    State(state): State<AuthorizationGateState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if is_exempt(request.uri().path()) {
        return next.run(request).await;
    }
    let Some(basis) = request.extensions().get::<AccessBasis>() else {
        return pl_revoked_response();
    };
    // `PairingPeer` intentionally falls through here: the door-only
    // confinement layer refuses it before this authorization gate is reached.
    let AccessBasis::LinkedDevice { cid, .. } = basis else {
        return next.run(request).await;
    };
    // This gate resolves each request's authorization by reading the ledger;
    // carrier-level revocation is separate and lives in the door's carrier loop.
    // The gate holds no watch borrow: the posture is read from the ledger per request.
    let path = state.authorized_clients_path.clone();
    if let Some(read_probe) = &state.read_probe {
        read_probe.record();
    }
    let posture = match tokio::time::timeout(
        Duration::from_millis(1000),
        tokio::task::spawn_blocking(move || read_authorized_clients(&path)),
    )
    .await
    {
        Ok(Ok(posture)) => posture,
        Err(_) => {
            log::warn!("paired-device authorization read timed out after 1000 ms");
            return pl_revoked_response();
        }
        Ok(Err(error)) => {
            log::warn!("paired-device authorization read task failed: {error}");
            return pl_revoked_response();
        }
    };
    if !is_authorized(&posture, cid) {
        return pl_revoked_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::body::to_bytes;
    use axum::http::{Method, Request, StatusCode};
    use axum::routing::{get, post};
    use axum::{Router, middleware};
    use solstone_core_convey_http::identity::{AccessBasis, Carrier};
    use solstone_core_sol_link::DeviceDoorAuthorization;
    use solstone_core_sol_link::ledger::{AuthorizationLedger, AuthorizedClientsRead};
    use tokio::sync::watch;
    use tower::ServiceExt;

    use crate::door::PairingAdmission;
    use crate::relay_admission::RelayNonceIdentity;

    use super::{
        AUTHORIZATION_GATE_EXEMPTIONS, AuthorizationExemption, AuthorizationGateState,
        authorized_router_with_router, require_authorization,
    };

    #[test]
    fn exemption_inventory_is_named_and_closed() {
        assert_eq!(
            AUTHORIZATION_GATE_EXEMPTIONS,
            [
                AuthorizationExemption::Favicon,
                AuthorizationExemption::TopLevelStatic,
            ]
        );
    }

    #[tokio::test]
    async fn pairing_peer_falls_through_the_authorization_gate() {
        let temporary = tempfile::TempDir::new_in("/var/tmp").expect("temporary root");
        let (_sender, authorization) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let app = Router::new()
            .route("/probe", get(|| async { StatusCode::OK }))
            .route_layer(middleware::from_fn_with_state(
                AuthorizationGateState {
                    authorization,
                    authorized_clients_path: AuthorizationLedger::new(temporary.path())
                        .authorized_clients_path()
                        .to_path_buf(),
                    read_probe: None,
                },
                require_authorization,
            ));
        let mut request = Request::get("/probe").body(Body::empty()).unwrap();
        request.extensions_mut().insert(AccessBasis::PairingPeer {
            carrier: Carrier::Direct,
        });

        assert_eq!(app.oneshot(request).await.unwrap().status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn pairing_confinement_checks_window_then_path_and_covers_unmatched_routes() {
        let temporary = tempfile::TempDir::new_in("/var/tmp").expect("temporary root");
        let (_sender, authorization) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let app = authorized_router_with_router(
            Router::new()
                .route(spl_core::PAIR_PATH, post(|| async { StatusCode::OK }))
                .route("/app/link/pair", post(|| async { StatusCode::OK }))
                .route("/app/network/pair-start", post(|| async { StatusCode::OK }))
                .route("/app/link/pair-start", post(|| async { StatusCode::OK }))
                .route("/app/network/unpair", post(|| async { StatusCode::OK }))
                .route("/app/link/unpair", post(|| async { StatusCode::OK })),
            temporary.path().to_path_buf(),
            authorization,
        )
        .into_inner();
        async fn response(app: Router, method: Method, path: &str) -> (StatusCode, Vec<u8>) {
            let mut request = Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .expect("request");
            request.extensions_mut().insert(AccessBasis::PairingPeer {
                carrier: Carrier::Direct,
            });
            let response = app.oneshot(request).await.expect("response");
            let status = response.status();
            (
                status,
                to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body")
                    .to_vec(),
            )
        }
        for path in super::PAIR_PATHS {
            let closed = response(app.clone(), Method::POST, path).await;
            assert_eq!(
                closed,
                (StatusCode::FORBIDDEN, b"pairing window closed".to_vec()),
                "{path}"
            );
        }
        let store = solstone_core_sol_link::pairing::nonces::NonceStore::new(temporary.path());
        store
            .add(
                "nonce".into(),
                "phone".into(),
                "".into(),
                false,
                2_200_000_000,
            )
            .expect("open window");
        for path in super::PAIR_PATHS {
            assert_eq!(
                response(app.clone(), Method::POST, path).await.0,
                StatusCode::OK,
                "{path}"
            );
        }
        let path_body = b"pairing tunnel may only use /app/network/pair".to_vec();
        for path in [
            spl_core::PAIR_PATH,
            "/app/link/pair",
            "/app/network/%70air",
            "/app/link/%70air",
            "/app/network/pair-start",
            "/app/link/pair-start",
            "/app/network/unpair",
            "/app/link/unpair",
            "/__door_test/pairing-probe",
        ] {
            let method = if path.ends_with("/pair") {
                Method::GET
            } else {
                Method::POST
            };
            assert_eq!(
                response(app.clone(), method, path).await,
                (StatusCode::FORBIDDEN, path_body.clone()),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn relay_admission_requires_its_exact_live_nonce() {
        let temporary = tempfile::TempDir::new_in("/var/tmp").expect("temporary root");
        let (_sender, authorization) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let app = authorized_router_with_router(
            Router::new().route(spl_core::PAIR_PATH, post(|| async { StatusCode::OK })),
            temporary.path().to_path_buf(),
            authorization,
        )
        .into_inner();
        let store = solstone_core_sol_link::pairing::nonces::NonceStore::new(temporary.path());
        let now = 2_200_000_000;
        store
            .add_relay("relay-a".into(), "phone".into(), "".into(), now)
            .expect("relay window");

        async fn response(app: Router, nonce: &str) -> StatusCode {
            let mut request = Request::post(spl_core::PAIR_PATH)
                .body(Body::empty())
                .expect("request");
            request.extensions_mut().insert(AccessBasis::PairingPeer {
                carrier: Carrier::ViaSpl,
            });
            request
                .extensions_mut()
                .insert(PairingAdmission::Relay(RelayNonceIdentity::new(
                    nonce.to_owned(),
                )));
            app.oneshot(request).await.expect("response").status()
        }

        assert_eq!(response(app.clone(), "relay-a").await, StatusCode::OK);
        assert_eq!(
            response(app.clone(), "relay-b").await,
            StatusCode::FORBIDDEN
        );
        store.consume("relay-a", now).expect("consume relay");
        assert_eq!(response(app, "relay-a").await, StatusCode::FORBIDDEN);
    }
}
