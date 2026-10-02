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
    axum::Json(report_context(&solstone_core_about::host_about(env!(
        "CARGO_PKG_VERSION"
    ))))
}

fn report_context(facts: &solstone_core_about::About) -> serde_json::Value {
    json!({"version": facts.version, "os": facts.os, "os_version": facts.os_version,
           "arch": facts.arch, "about": facts.about})
}

#[cfg(test)]
mod tests {
    use axum::http::Request;
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

    #[test]
    fn context_has_exact_fixed_platform_keys() {
        let facts = solstone_core_about::About::from_facts(
            "1.2.3",
            None,
            "ubuntu".into(),
            "24.04".into(),
            "x86_64".into(),
        );
        let value = report_context(&facts);
        let mut keys = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(keys, ["about", "arch", "os", "os_version", "version"]);
        assert_eq!(value["about"], "journal 1.2.3 · ubuntu 24.04 · x86_64");
    }
}
