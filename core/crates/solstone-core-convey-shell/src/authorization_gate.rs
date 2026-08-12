// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::Serialize;
use solstone_core_convey_http::identity::{AccessBasis, LinkedDeviceDid};
use solstone_core_sol_link::DeviceDoorAuthorization;
use solstone_core_sol_link::ledger::AuthorizedClientsRead;
use tokio::sync::watch;

#[derive(Clone)]
pub struct AuthorizationGateState {
    pub authorization: watch::Receiver<DeviceDoorAuthorization>,
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

/// Apply per-request paired-device authorization to a composed production router.
pub fn authorized_router(
    router: Router,
    authorization: watch::Receiver<DeviceDoorAuthorization>,
) -> Router {
    // [check] Axum 0.8.9 `Router::route_layer` runs only for a matching route,
    // so router()'s existing fallback still returns 404 rather than this gate's
    // 403. It still applies before a matched route's 405 method response; using
    // `route_layer`, not `layer`, preserves strict-slash and unknown-path 404s.
    // Applying it after router() has composed every route wraps the full
    // surface, including routes from merged sub-routers.
    router.route_layer(middleware::from_fn_with_state(
        AuthorizationGateState { authorization },
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
    let AccessBasis::LinkedDevice { did, .. } = basis else {
        return next.run(request).await;
    };
    // [check] Keep the watch borrow scoped before await: `watch::Ref` should not
    // cross unrelated async work, and this avoids cloning the posture's entries
    // for every request.
    let decision = {
        let posture = state.authorization.borrow();
        is_authorized(posture.as_read(), did)
    };
    if !decision {
        return pl_revoked_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::{AUTHORIZATION_GATE_EXEMPTIONS, AuthorizationExemption};

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
}
