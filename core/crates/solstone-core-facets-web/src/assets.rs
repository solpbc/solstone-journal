// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::Body,
    http::{Response, StatusCode, header},
    response::IntoResponse,
};

// Deliberate local shell copy: W5 removes the Python source tree; settings-web's cross-tree include is a known W5 obligation.
const SHELL: &[u8] = include_bytes!("../assets/shell.html");
const WORKSPACE: &[u8] = include_bytes!("../assets/timeline/workspace.html");
const BACKGROUND: &[u8] = include_bytes!("../assets/timeline/background.html");
const TIMELINE_JS: &[u8] = include_bytes!("../assets/timeline/timeline.js");
const TIMELINE_CSS: &[u8] = include_bytes!("../assets/timeline/timeline.css");
const PROVENANCE_JS: &[u8] = include_bytes!("../assets/timeline/timeline_provenance.js");

pub fn shell() -> axum::response::Response {
    asset(SHELL, "text/html; charset=utf-8")
}

pub fn workspace() -> axum::response::Response {
    asset(WORKSPACE, "text/html; charset=utf-8")
}

pub fn background() -> axum::response::Response {
    asset(BACKGROUND, "text/html; charset=utf-8")
}

pub fn static_asset(name: &str) -> axum::response::Response {
    match name {
        "timeline.js" => asset(TIMELINE_JS, "text/javascript; charset=utf-8"),
        "timeline.css" => asset(TIMELINE_CSS, "text/css; charset=utf-8"),
        "timeline_provenance.js" => asset(PROVENANCE_JS, "text/javascript; charset=utf-8"),
        _ => empty_not_found(),
    }
}

pub fn empty_not_found() -> axum::response::Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::empty())
        .expect("timeline response builds")
        .into_response()
}

fn asset(bytes: &'static [u8], content_type: &'static str) -> axum::response::Response {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("embedded timeline asset response builds")
        .into_response()
}
