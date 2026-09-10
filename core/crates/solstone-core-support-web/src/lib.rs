// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local-only report handoff for the Support Convey surface.

use std::path::PathBuf;

use axum::{
    Router,
    http::header,
    response::{Redirect, Response},
    routing::get,
};
use serde_json::json;

const SHELL: &[u8] = include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
const WORKSPACE: &[u8] = include_bytes!("../assets/workspace.html");
const SUPPORT_JS: &[u8] = include_bytes!("../assets/static/support.js");

/// Build the local Support route surface. The journal root is deliberately unused:
/// reporting reads no journal data and sends no request to a support service.
pub fn routes(_journal_root: PathBuf) -> Router {
    Router::new()
        .route(
            "/app/support",
            get(|| async { Redirect::permanent("/app/support/") }),
        )
        .route(
            "/app/support/",
            get(|| async { bytes(SHELL, "text/html; charset=utf-8") }),
        )
        .route(
            "/app/support/workspace",
            get(|| async { bytes(WORKSPACE, "text/html; charset=utf-8") }),
        )
        .route(
            "/app/support/static/support.js",
            get(|| async { bytes(SUPPORT_JS, "text/javascript; charset=utf-8") }),
        )
        .route("/app/support/api/context", get(context))
}

fn bytes(value: &'static [u8], content_type: &'static str) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::from(value))
        .expect("embedded support asset response")
}

async fn context() -> axum::Json<serde_json::Value> {
    axum::Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "os": os_name(),
        "os_version": os_version(),
    }))
}

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "macOS",
        "windows" => "Windows",
        other => other,
    }
}

#[cfg(unix)]
fn os_version() -> String {
    nix::sys::utsname::uname()
        .map(|value| value.release().to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown".to_owned())
}

#[cfg(windows)]
fn os_version() -> String {
    std::process::Command::new("cmd")
        .args(["/C", "ver"])
        .output()
        .ok()
        .filter(|result| result.status.success())
        .map(|result| String::from_utf8_lossy(&result.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(not(any(unix, windows)))]
fn os_version() -> String {
    "unknown".to_owned()
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, http::Request};
    use serde_json::Value;
    use tower::ServiceExt as _;

    use super::*;

    #[tokio::test]
    async fn exposes_only_local_report_routes() {
        let app = routes(PathBuf::from("/must-not-be-read"));
        for path in [
            "/app/support/api/tickets",
            "/app/support/api/articles",
            "/app/support/api/announcements",
            "/app/support/api/register",
            "/app/support/api/badge-count",
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(axum::body::Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                axum::http::StatusCode::NOT_FOUND,
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn context_has_exact_fixed_platform_keys() {
        let response = routes(PathBuf::new())
            .oneshot(
                Request::get("/app/support/api/context")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let mut keys = value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(keys, ["os", "os_version", "version"]);
        assert!(
            value["version"]
                .as_str()
                .is_some_and(|item| !item.is_empty())
        );
        assert!(value["os"].as_str().is_some_and(|item| !item.is_empty()));
        assert!(
            value["os_version"]
                .as_str()
                .is_some_and(|item| !item.is_empty())
        );
    }
}
