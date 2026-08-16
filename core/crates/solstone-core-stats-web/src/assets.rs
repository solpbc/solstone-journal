// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::Body,
    http::{Response, header},
};

const SHELL: &[u8] = include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
const WORKSPACE: &[u8] = include_bytes!("../assets/workspace.html");
include!(concat!(env!("OUT_DIR"), "/static_assets.rs"));

pub async fn shell() -> Response<Body> {
    asset_response(SHELL, "text/html; charset=utf-8")
}
pub async fn workspace() -> Response<Body> {
    asset_response(WORKSPACE, "text/html; charset=utf-8")
}
pub async fn background() -> Response<Body> {
    not_found()
}
pub async fn static_asset(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response<Body> {
    STATIC_ASSETS
        .iter()
        .find(|(asset, _, _)| *asset == name)
        .map_or_else(not_found, |(_, content_type, bytes)| {
            asset_response(bytes, content_type)
        })
}

pub fn not_found() -> Response<Body> {
    Response::builder().status(axum::http::StatusCode::NOT_FOUND).header(header::CONTENT_TYPE, "text/html; charset=utf-8").body(Body::from("<!doctype html>\n<html lang=en>\n<title>404 Not Found</title>\n<h1>Not Found</h1>\n<p>The requested URL was not found on the server. If you entered the URL manually please check your spelling and try again.</p>\n")).expect("stats 404 response")
}
fn asset_response(bytes: &'static [u8], content_type: &'static str) -> Response<Body> {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("embedded stats asset response")
}
