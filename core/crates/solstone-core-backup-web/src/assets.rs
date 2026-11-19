// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::Response,
};

const WORKSPACE: &[u8] = include_bytes!("../assets/workspace.html");
const SHELL: &[u8] = include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
const JS: &[u8] = include_bytes!("../assets/backup.js");
const STATUS_JS: &[u8] = include_bytes!("../assets/backup-status.js");
const CSS: &[u8] = include_bytes!("../assets/backup.css");

fn bytes(status: StatusCode, bytes: &'static [u8], content_type: &'static str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("backup asset response")
}
pub async fn shell() -> Response {
    bytes(StatusCode::OK, SHELL, "text/html; charset=utf-8")
}
pub async fn workspace() -> Response {
    bytes(StatusCode::OK, WORKSPACE, "text/html; charset=utf-8")
}
pub async fn background() -> Response {
    solstone_core_convey_http::window_catalog::not_found_response()
}
pub async fn static_asset(axum::extract::Path(name): axum::extract::Path<String>) -> Response {
    match name.as_str() {
        "backup.js" => bytes(StatusCode::OK, JS, "text/javascript; charset=utf-8"),
        "backup-status.js" => bytes(StatusCode::OK, STATUS_JS, "text/javascript; charset=utf-8"),
        "backup.css" => bytes(StatusCode::OK, CSS, "text/css; charset=utf-8"),
        _ => background().await,
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn shared_status_presentation_is_served_as_javascript() {
        let response =
            super::static_asset(axum::extract::Path("backup-status.js".to_owned())).await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), super::STATUS_JS);
    }
}
