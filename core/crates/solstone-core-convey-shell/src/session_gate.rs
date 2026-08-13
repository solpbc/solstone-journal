// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;

use crate::registry::known_app;
use crate::session::{SessionState, classify_session};

#[derive(Debug, Clone)]
pub struct SessionGateState {
    pub journal_root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionExemption {
    Favicon,
    TopLevelStatic,
    UnknownAppPrefix,
    UnmatchedFallback,
}

pub const SESSION_GATE_EXEMPTIONS: &[SessionExemption] = &[
    SessionExemption::Favicon,
    SessionExemption::TopLevelStatic,
    SessionExemption::UnknownAppPrefix,
    SessionExemption::UnmatchedFallback,
];

pub fn apply_layer(router: axum::Router, journal_root: PathBuf) -> axum::Router {
    router.route_layer(middleware::from_fn_with_state(
        SessionGateState { journal_root },
        require_session,
    ))
}

fn is_exempt(path: &str) -> bool {
    SESSION_GATE_EXEMPTIONS
        .iter()
        .any(|exemption| match exemption {
            SessionExemption::Favicon => path == "/favicon.ico",
            SessionExemption::TopLevelStatic => path.starts_with("/static/"),
            SessionExemption::UnknownAppPrefix => {
                let mut segments = path.trim_start_matches('/').split('/');
                matches!(segments.next(), Some("app"))
                    && segments
                        .next()
                        .is_some_and(|name| known_app(name).is_none())
            }
            // The router leaves unmatched paths outside its route layer. This
            // declarative entry records that structural exemption with the others.
            SessionExemption::UnmatchedFallback => false,
        })
}

async fn require_session(
    State(state): State<SessionGateState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if is_exempt(path) {
        return next.run(request).await;
    }
    match classify_session(&state.journal_root) {
        SessionState::Established => next.run(request).await,
        SessionState::Unestablished => redirect_to_init(),
        SessionState::Corrupt { detail } => corrupt_response(path, detail),
    }
}

fn redirect_to_init() -> Response {
    let location = "/init";
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(redirect_body(location)))
        .expect("redirect response builds")
}

pub(crate) fn redirect_body(location: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=en>\n<title>Redirecting...</title>\n<h1>Redirecting...</h1>\n<p>You should be redirected automatically to the target URL: <a href=\"{location}\">{location}</a>. If not, click the link.\n"
    )
}

fn corrupt_response(path: &str, detail: String) -> Response {
    if path
        .trim_matches('/')
        .split('/')
        .any(|segment| segment == "api")
    {
        return Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(format!(
                "{{\"detail\":{},\"error\":\"I couldn't read your settings.\",\"reason_code\":\"corrupt_config\"}}\n",
                serde_json::to_string(&detail).expect("corrupt detail serializes")
            )))
            .expect("corrupt API response builds");
    }
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(detail))
        .expect("corrupt response builds")
}

#[cfg(test)]
mod tests {
    use super::{SessionExemption, apply_layer};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    #[tokio::test]
    async fn unlisted_routes_are_gated_by_default() {
        let temporary = std::env::temp_dir().join(format!(
            "solstone-convey-shell-default-gate-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temporary);
        std::fs::create_dir(&temporary).expect("temporary directory creates");
        let router = apply_layer(
            axum::Router::new().route("/throwaway", get(|| async { "reachable" })),
            temporary.clone(),
        );
        let response = router
            .oneshot(Request::get("/throwaway").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(response.headers()["location"], "/init");
        let _ = std::fs::remove_dir_all(temporary);
    }

    #[test]
    fn exemption_inventory_is_named_and_closed() {
        assert_eq!(
            super::SESSION_GATE_EXEMPTIONS,
            [
                SessionExemption::Favicon,
                SessionExemption::TopLevelStatic,
                SessionExemption::UnknownAppPrefix,
                SessionExemption::UnmatchedFallback,
            ]
        );
    }
}
