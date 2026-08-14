// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::Body,
    http::{Response, header},
};

const SHELL: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/convey/static/shell.html"
));
const WORKSPACE: &[u8] = include_bytes!("../assets/workspace.html");
const HEALTH_JS: &[u8] = include_bytes!("../assets/health.js");

pub async fn shell() -> Response<Body> {
    asset(SHELL, "text/html; charset=utf-8")
}
pub async fn workspace() -> Response<Body> {
    asset(WORKSPACE, "text/html; charset=utf-8")
}
pub async fn health_js() -> Response<Body> {
    asset(HEALTH_JS, "text/javascript; charset=utf-8")
}

fn asset(bytes: &'static [u8], content_type: &'static str) -> Response<Body> {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("embedded health asset response")
}
