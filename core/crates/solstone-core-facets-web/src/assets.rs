// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::Body,
    http::{Response, header},
    response::IntoResponse,
};

// Read the canonical Convey shell asset directly, like the sibling web crates.
const SHELL: &[u8] = include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
const NEWS_WORKSPACE: &[u8] = include_bytes!("../assets/news/workspace.html");
const CURATION_WORKSPACE: &[u8] = include_bytes!("../assets/curation/workspace.html");
const CURATION_EVIDENCE_JS: &[u8] = include_bytes!("../assets/curation/curation_evidence.js");

pub fn shell() -> axum::response::Response {
    asset(SHELL, "text/html; charset=utf-8")
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

fn asset(bytes: &'static [u8], content_type: &'static str) -> axum::response::Response {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("embedded asset response builds")
        .into_response()
}
