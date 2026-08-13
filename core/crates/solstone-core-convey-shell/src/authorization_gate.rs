// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::Serialize;
use solstone_core_convey_http::identity::{AccessBasis, LinkedDeviceDid};
use solstone_core_sol_link::DeviceDoorAuthorization;
use solstone_core_sol_link::ledger::{
    AuthorizationLedger, AuthorizedClientsRead, read_authorized_clients,
};
use tokio::sync::watch;

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
    let authorized_clients_path = AuthorizationLedger::new(&journal_root)
        .authorized_clients_path()
        .to_path_buf();
    // [check] Axum 0.8.9 `Router::route_layer` runs only for a matching route,
    // so router()'s existing fallback still returns 404 rather than this gate's
    // 403. It still applies before a matched route's 405 method response; using
    // `route_layer`, not `layer`, preserves strict-slash and unknown-path 404s.
    // Applying it after router() has composed every route wraps the full
    // surface, including routes from merged sub-routers.
    crate::router(journal_root).route_layer(middleware::from_fn_with_state(
        AuthorizationGateState {
            authorization,
            authorized_clients_path,
        },
        require_authorization,
    ))
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
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::{Router, middleware};
    use solstone_core_convey_http::identity::{AccessBasis, Carrier};
    use solstone_core_sol_link::DeviceDoorAuthorization;
    use solstone_core_sol_link::ledger::AuthorizedClientsRead;
    use tokio::sync::watch;
    use tower::ServiceExt;

    use super::{
        AUTHORIZATION_GATE_EXEMPTIONS, AuthorizationExemption, AuthorizationGateState,
        require_authorization,
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
}
