// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::Body,
    http::{Response, StatusCode, header},
    response::IntoResponse,
};

// Read the canonical Convey shell asset directly, like the sibling web crates.
const SHELL: &[u8] = include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
const WORKSPACE: &[u8] = include_bytes!("../assets/timeline/workspace.html");
const NEWS_WORKSPACE: &[u8] = include_bytes!("../assets/news/workspace.html");
const CURATION_WORKSPACE: &[u8] = include_bytes!("../assets/curation/workspace.html");
const CURATION_EVIDENCE_JS: &[u8] = include_bytes!("../assets/curation/curation_evidence.js");
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

pub fn news_workspace() -> axum::response::Response {
    asset(NEWS_WORKSPACE, "text/html; charset=utf-8")
}

pub fn curation_workspace() -> axum::response::Response {
    asset(CURATION_WORKSPACE, "text/html; charset=utf-8")
}

pub fn curation_evidence_js() -> axum::response::Response {
    asset(CURATION_EVIDENCE_JS, "text/javascript; charset=utf-8")
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
