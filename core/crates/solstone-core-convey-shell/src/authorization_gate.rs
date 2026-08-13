// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::Serialize;
use solstone_core_convey_http::identity::{AccessBasis, LinkedDeviceDid};
use solstone_core_sol_link::DeviceDoorAuthorization;
use solstone_core_sol_link::ledger::{
    AuthorizationLedger, AuthorizedClientsRead, read_authorized_clients,
};
use solstone_core_sol_link::pairing::nonces::{NonceStore, pairing_window_open};
use tokio::sync::watch;

use crate::door::PairingWindowAdmission;

static AUTHORIZATION_GATE_READ_TICKS: AtomicU64 = AtomicU64::new(0);

/// Cumulative count of authorization-ledger reads made by the request gate.
/// Debug builds only advance it; assert deltas, never an absolute value.
pub fn authorization_gate_read_ticks() -> u64 {
    AUTHORIZATION_GATE_READ_TICKS.load(Ordering::Relaxed)
}

#[derive(Clone)]
pub struct AuthorizationGateState {
    pub authorization: watch::Receiver<DeviceDoorAuthorization>,
    pub authorized_clients_path: PathBuf,
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
// [check] Only boot assets bypass request authorization. APIs, app paths, and
// `/sse/events` remain gated: this gate refuses a new SSE request, while W1a's
// carrier-level revocation closes an already-open device stream.

#[derive(Serialize)]
struct AuthorizationRefusal {
    error: &'static str,
    reason: &'static str,
    reason_code: &'static str,
    detail: &'static str,
}

/// Build the production router whose paired-device requests are rechecked per request.
pub fn authorized_router(
    journal_root: PathBuf,
    authorization: watch::Receiver<DeviceDoorAuthorization>,
) -> Router {
    authorized_router_with_router(
        crate::router(journal_root.clone()),
        journal_root,
        authorization,
    )
}

/// Apply the door-only production layers to a prebuilt router. Loopback is
/// structurally excluded because only `authorized_router*` invokes this helper.
pub fn authorized_router_with_router(
    router: Router,
    journal_root: PathBuf,
    authorization: watch::Receiver<DeviceDoorAuthorization>,
) -> Router {
    let authorized_clients_path = AuthorizationLedger::new(&journal_root)
        .authorized_clients_path()
        .to_path_buf();
    // [check] Axum 0.8.9 `Router::route_layer` runs only for a matching route,
    // so router()'s existing fallback still returns 404 rather than this gate's
    // 403. It still applies before a matched route's 405 method response; using
    // `route_layer`, not `layer`, preserves strict-slash and unknown-path 404s.
    // Applying it after router() has composed every route wraps the full
    // surface, including routes from merged sub-routers.
    router
        .route_layer(middleware::from_fn_with_state(
            AuthorizationGateState {
                authorization,
                authorized_clients_path,
            },
            require_authorization,
        ))
        .layer(middleware::from_fn_with_state(
            PairingConfinementState { journal_root },
            require_pairing_confinement,
        ))
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
    if request
        .extensions()
        .get::<PairingWindowAdmission>()
        .is_none()
        && !pairing_window_open(&NonceStore::new(&state.journal_root), now)
    {
        return pairing_confinement_response("pairing window closed");
    }
    let raw_path = request.uri().path();
    let decoded = percent_encoding::percent_decode_str(raw_path).decode_utf8();
    if decoded.as_deref().ok() != Some(raw_path) {
        return pairing_confinement_response("pairing tunnel may only use /app/network/pair");
    }
    if raw_path != spl_core::PAIR_PATH || request.method() != Method::POST {
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

fn is_authorized(posture: &AuthorizedClientsRead, did: &LinkedDeviceDid) -> bool {
    matches!(posture, AuthorizedClientsRead::Present(entries) if entries.iter().any(|entry| entry.fingerprint == did.as_str()))
}

fn pl_revoked_response() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(AuthorizationRefusal {
            error: "I couldn't use that paired device because it was revoked.",
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
    // `PairingPeer` intentionally falls through here: the forthcoming door
    // confinement layer refuses it before this authorization gate is reached.
    let AccessBasis::LinkedDevice { did, .. } = basis else {
        return next.run(request).await;
    };
    // This gate resolves each request's authorization by reading the ledger;
    // carrier-level revocation is separate and lives in the door's carrier loop.
    // The gate holds no watch borrow: the posture is read from the ledger per request.
    let path = state.authorized_clients_path.clone();
    #[cfg(debug_assertions)]
    AUTHORIZATION_GATE_READ_TICKS.fetch_add(1, Ordering::Relaxed);
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
    if !is_authorized(&posture, did) {
        return pl_revoked_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::body::Body;
    use axum::body::to_bytes;
    use axum::http::{Method, Request, StatusCode};
    use axum::routing::{get, post};
    use axum::{Router, middleware};
    use solstone_core_convey_http::identity::{AccessBasis, Carrier};
    use solstone_core_sol_link::DeviceDoorAuthorization;
    use solstone_core_sol_link::ledger::AuthorizedClientsRead;
    use tokio::sync::watch;
    use tower::ServiceExt;

    use super::{
        AUTHORIZATION_GATE_EXEMPTIONS, AuthorizationExemption, AuthorizationGateState,
        authorized_router_with_router, require_authorization,
    };

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("solstone-confinement-{nanos}-{sequence}"));
            fs::create_dir(&path).expect("temporary root");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

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
        let (_sender, authorization) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let app = Router::new()
            .route("/probe", get(|| async { StatusCode::OK }))
            .route_layer(middleware::from_fn_with_state(
                AuthorizationGateState { authorization },
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
        let temporary = TempDir::new();
        let (_sender, authorization) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let app = authorized_router_with_router(
            Router::new().route(spl_core::PAIR_PATH, post(|| async { StatusCode::OK })),
            temporary.0.clone(),
            authorization,
        );
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
        let closed = response(app.clone(), Method::POST, spl_core::PAIR_PATH).await;
        assert_eq!(
            closed,
            (StatusCode::FORBIDDEN, b"pairing window closed".to_vec())
        );
        let store = solstone_core_sol_link::pairing::nonces::NonceStore::new(&temporary.0);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs() as i64;
        store
            .add("nonce".into(), "phone".into(), "".into(), false, now)
            .expect("open window");
        assert_eq!(
            response(app.clone(), Method::POST, spl_core::PAIR_PATH)
                .await
                .0,
            StatusCode::OK
        );
        let path_body = b"pairing tunnel may only use /app/network/pair".to_vec();
        assert_eq!(
            response(app.clone(), Method::GET, spl_core::PAIR_PATH).await,
            (StatusCode::FORBIDDEN, path_body.clone())
        );
        assert_eq!(
            response(app.clone(), Method::POST, "/app/network/%70air").await,
            (StatusCode::FORBIDDEN, path_body.clone())
        );
        assert_eq!(
            response(app, Method::POST, "/__door_test/pairing-probe").await,
            (StatusCode::FORBIDDEN, path_body)
        );
    }
}
