// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use solstone_core_convey_http::envelope::error_envelope;
#[cfg(feature = "host")]
use solstone_core_ingest::INGEST_PATH_PREFIX;

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
    Ingest,
    Init,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionGateScope {
    AllBranches,
    UnestablishedOnly,
    CorruptOnly,
}

pub const SESSION_GATE_EXEMPTIONS: &[SessionExemption] = &[
    SessionExemption::Favicon,
    SessionExemption::TopLevelStatic,
    SessionExemption::UnknownAppPrefix,
    SessionExemption::UnmatchedFallback,
    SessionExemption::Ingest,
    SessionExemption::Init,
];

pub fn apply_layer(router: axum::Router, journal_root: PathBuf) -> axum::Router {
    router.route_layer(middleware::from_fn_with_state(
        SessionGateState { journal_root },
        require_session,
    ))
}

#[cfg(feature = "host")]
fn is_ingest_path(path: &str) -> bool {
    path.starts_with(INGEST_PATH_PREFIX)
}

#[cfg(not(feature = "host"))]
fn is_ingest_path(_path: &str) -> bool {
    false
}

fn exemption_scope_for(path: &str) -> Option<SessionGateScope> {
    SESSION_GATE_EXEMPTIONS
        .iter()
        .find_map(|exemption| match exemption {
            SessionExemption::Favicon if path == "/favicon.ico" => {
                Some(SessionGateScope::AllBranches)
            }
            SessionExemption::TopLevelStatic if path.starts_with("/static/") => {
                Some(SessionGateScope::AllBranches)
            }
            SessionExemption::UnknownAppPrefix => {
                let mut segments = path.trim_start_matches('/').split('/');
                (matches!(segments.next(), Some("app"))
                    && segments
                        .next()
                        .is_some_and(|name| name != "link" && known_app(name).is_none()))
                .then_some(SessionGateScope::AllBranches)
            }
            // The router leaves unmatched paths outside its route layer. This
            // declarative entry records that structural exemption with the others.
            SessionExemption::UnmatchedFallback => None,
            SessionExemption::Ingest if is_ingest_path(path) => {
                Some(SessionGateScope::UnestablishedOnly)
            }
            SessionExemption::Init if path == "/init" || path.starts_with("/init/") => {
                Some(SessionGateScope::CorruptOnly)
            }
            _ => None,
        })
}

async fn require_session(
    State(state): State<SessionGateState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let scope = exemption_scope_for(path);
    match classify_session(&state.journal_root) {
        SessionState::Established => next.run(request).await,
        SessionState::Unestablished
            if matches!(
                scope,
                Some(SessionGateScope::AllBranches | SessionGateScope::CorruptOnly)
            ) =>
        {
            next.run(request).await
        }
        SessionState::Unestablished => redirect_to_init(),
        SessionState::Corrupt { detail: _ }
            if matches!(
                scope,
                Some(SessionGateScope::AllBranches | SessionGateScope::UnestablishedOnly)
            ) =>
        {
            next.run(request).await
        }
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
            "I couldn't read your settings.",
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
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{SessionExemption, SessionGateScope, apply_layer, exemption_scope_for};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock is after epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "solstone-convey-shell-session-gate-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("temporary root creates");
            Self(path)
        }

        fn make_corrupt(&self) {
            fs::create_dir_all(self.0.join("config")).expect("config directory creates");
            fs::write(self.0.join("config/journal.json"), b"{").expect("config writes");
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn scoped_router(root: PathBuf) -> axum::Router {
        apply_layer(
            axum::Router::new()
                .route("/regular", get(|| async { "reachable" }))
                .route("/favicon.ico", get(|| async { "reachable" }))
                .route("/static/shell.html", get(|| async { "reachable" }))
                .route("/app/someunknownapp/x", get(|| async { "reachable" }))
                .route("/app/link/api/identity", get(|| async { "reachable" }))
                .route(
                    "/app/observer/ingest/manifest",
                    get(|| async { "reachable" }),
                )
                .route("/init", get(|| async { "reachable" }))
                .route("/init/mark", get(|| async { "reachable" })),
            root,
        )
    }

    async fn status(app: axum::Router, path: &str) -> StatusCode {
        app.oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

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
                SessionExemption::Ingest,
                SessionExemption::Init,
            ]
        );
    }

    #[test]
    fn only_link_is_recognized_locally_without_widening_the_app_registry() {
        assert_eq!(
            exemption_scope_for("/app/link/api/identity"),
            None,
            "link routes remain ordinarily gated"
        );
        assert_eq!(exemption_scope_for("/app/network/api/identity"), None);
        assert_eq!(
            exemption_scope_for("/app/someunknownapp/x"),
            Some(SessionGateScope::AllBranches)
        );
    }

    #[tokio::test]
    async fn branch_scopes_keep_ingest_and_init_opposite() {
        let temporary = TempDir::new();
        let unestablished = scoped_router(temporary.0.clone());

        assert_eq!(
            status(unestablished.clone(), "/app/observer/ingest/manifest").await,
            StatusCode::FOUND
        );
        for path in ["/init", "/init/mark"] {
            assert_eq!(
                status(unestablished.clone(), path).await,
                StatusCode::OK,
                "{path}"
            );
        }
        for path in [
            "/favicon.ico",
            "/static/shell.html",
            "/app/someunknownapp/x",
        ] {
            assert_eq!(
                status(unestablished.clone(), path).await,
                StatusCode::OK,
                "{path}"
            );
        }
        assert_eq!(
            status(unestablished, "/app/link/api/identity").await,
            StatusCode::FOUND
        );

        temporary.make_corrupt();
        let corrupt = scoped_router(temporary.0.clone());
        assert_eq!(
            status(corrupt.clone(), "/app/observer/ingest/manifest").await,
            StatusCode::OK
        );
        for path in ["/init", "/init/mark"] {
            assert_eq!(
                status(corrupt.clone(), path).await,
                StatusCode::INTERNAL_SERVER_ERROR,
                "{path}"
            );
        }
        for path in [
            "/favicon.ico",
            "/static/shell.html",
            "/app/someunknownapp/x",
        ] {
            assert_eq!(
                status(corrupt.clone(), path).await,
                StatusCode::OK,
                "{path}"
            );
        }
        assert_eq!(
            status(corrupt, "/app/link/api/identity").await,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
