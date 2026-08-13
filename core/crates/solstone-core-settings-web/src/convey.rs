// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{fs, path::Path};

use axum::response::Response;
use serde_json::json;

use crate::http::json_response;

const DEFAULT_PORT: u16 = 5015;

pub async fn status(journal_root: std::path::PathBuf) -> Response {
    let port = read_port(&journal_root).unwrap_or(DEFAULT_PORT);
    let dashboard_url = format!("http://localhost:{port}");
    let bind = format!("127.0.0.1:{port}");
    json_response(json!({
        "dashboard_url": dashboard_url,
        "status_text": format!("convey\n  bind:              {bind}\n  dashboard url:     {dashboard_url}"),
    }))
}

fn read_port(root: &Path) -> Option<u16> {
    fs::read_to_string(root.join("health/convey.port"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, http::Request};
    use tower::ServiceExt;

    use crate::test_support::{established_root, shell_router};

    #[tokio::test]
    async fn ac12_convey_status_uses_live_bound_port() {
        let root = established_root();
        std::fs::create_dir_all(root.path().join("health")).expect("health directory");
        std::fs::write(root.path().join("health/convey.port"), "7123\n").expect("port file");
        let response = shell_router(root.path())
            .oneshot(
                Request::get("/app/settings/api/convey/status")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let body: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(body["dashboard_url"], "http://localhost:7123");
        assert!(
            body["status_text"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
        assert!(body["status_text"].as_str().expect("text").contains("7123"));
    }
}
