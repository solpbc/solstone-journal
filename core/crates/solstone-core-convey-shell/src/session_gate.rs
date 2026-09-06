// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use solstone_core_convey_http::envelope::error_envelope;

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
    InitSetup,
    UnknownAppPrefix,
    ImportDoor,
    UnmatchedFallback,
}

pub const SESSION_GATE_EXEMPTIONS: &[SessionExemption] = &[
    SessionExemption::Favicon,
    SessionExemption::TopLevelStatic,
    SessionExemption::InitSetup,
    SessionExemption::UnknownAppPrefix,
    SessionExemption::ImportDoor,
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
            SessionExemption::InitSetup => path == "/init" || path.starts_with("/init/"),
            SessionExemption::UnknownAppPrefix => {
                let mut segments = path.trim_start_matches('/').split('/');
                matches!(segments.next(), Some("app"))
                    && segments.next().is_some_and(|name| {
                        // `/app/link` aliases `/app/network` and is not an app
                        // registry entry; it must stay session-gated.
                        // `/app/devices` left the registry; leftover ingest
                        // routes and the permanent redirects must stay gated.
                        name != "link" && name != "devices" && known_app(name).is_none()
                    })
            }
            SessionExemption::ImportDoor => {
                let segments = path.trim_matches('/').split('/').collect::<Vec<_>>();
                matches!(
                    segments.as_slice(),
                    ["app", "import", "journal", prefix, "manifest", area]
                        if !prefix.is_empty() && !area.is_empty()
                ) || matches!(
                    segments.as_slice(),
                    ["app", "import", "journal", prefix, "ingest", kind]
                        if !prefix.is_empty()
                            && matches!(*kind, "segments" | "entities" | "facets" | "imports" | "config")
                )
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
        return error_envelope(
            "corrupt_config",
            "your settings couldn't be read.",
            detail,
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response();
    }
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(detail))
        .expect("corrupt response builds")
}

#[cfg(test)]
mod tests {
    use super::{SessionExemption, apply_layer, is_exempt};
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
                SessionExemption::InitSetup,
                SessionExemption::UnknownAppPrefix,
                SessionExemption::ImportDoor,
                SessionExemption::UnmatchedFallback,
            ]
        );
    }

    #[test]
    fn init_setup_exemption_is_the_init_prefix_only() {
        assert!(is_exempt("/init"));
        assert!(is_exempt("/init/"));
        assert!(is_exempt("/init/mark"));
        assert!(is_exempt("/init/api/state"));
        assert!(!is_exempt("/initfoo"));
        assert!(!is_exempt("/app/link/workspace"));
        assert!(!is_exempt("/app/devices"));
        assert!(!is_exempt("/app/devices/"));
        assert!(!is_exempt("/app/devices/workspace"));
        assert!(!is_exempt("/app/devices/ingest"));
        assert!(!is_exempt("/app/devices/ingest/manifest"));
        assert!(!is_exempt("/app/devices/ingest/manifest/20260804"));
        assert!(!is_exempt("/app/devices/ingest/segments/20260804"));
    }

    #[test]
    fn import_door_exemption_is_closed_to_six_route_shapes() {
        for path in [
            "/app/import/journal/prefix01/manifest/entities",
            "/app/import/journal/prefix01/ingest/segments",
            "/app/import/journal/prefix01/ingest/entities",
            "/app/import/journal/prefix01/ingest/facets",
            "/app/import/journal/prefix01/ingest/imports",
            "/app/import/journal/prefix01/ingest/config",
        ] {
            assert!(is_exempt(path), "{path}");
        }
        for path in [
            "/app/import/api/save",
            "/app/import/api/journal-sources/create",
            "/app/import/api/list",
            "/app/import/",
            "/app/import/journal/prefix01/ingest/segments/20260813",
            "/app/import/journal/prefix01/ingest/unknown",
        ] {
            assert!(!is_exempt(path), "{path}");
        }
    }
}
