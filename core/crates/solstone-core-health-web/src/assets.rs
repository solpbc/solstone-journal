// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::Body,
    http::{Response, header},
};

const SHELL: &[u8] = include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
const WORKSPACE: &str = include_str!("../assets/workspace.html");
include!(concat!(env!("OUT_DIR"), "/static_assets.rs"));

#[cfg(unix)]
pub(crate) const STORAGE_PRIVACY: &str = r#"<section class="dashboard-card" id="storagePrivacy" aria-labelledby="storagePrivacyTitle" hidden>
      <h2 class="card-title" id="storagePrivacyTitle">journal folder privacy</h2>
      <p data-storage-privacy-message role="status" aria-live="polite"></p>
      <button type="button" class="action-btn" data-storage-privacy-consent>keep it on this drive</button>
      <p data-storage-privacy-result role="status" hidden></p>
    </section>"#;
#[cfg(not(unix))]
pub(crate) const STORAGE_PRIVACY: &str = "";

pub async fn shell() -> Response<Body> {
    asset_response(SHELL, "text/html; charset=utf-8")
}
pub async fn workspace() -> Response<Body> {
    use solstone_core_system::stt_backend_choice::{SttRuntimeKind, stt_host_policy};
    let applicable = matches!(
        stt_host_policy(std::env::consts::OS, std::env::consts::ARCH).runtime_kind,
        Some(SttRuntimeKind::LinuxCpp | SttRuntimeKind::MacosCoreml)
    );
    let html = WORKSPACE.replace(
        "{{SPEECH_SETUP_APPLICABLE}}",
        if applicable { "true" } else { "false" },
    );
    let html = html.replace("{{STORAGE_PRIVACY}}", STORAGE_PRIVACY);
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(html))
        .expect("health workspace response")
}
/// Serve any file under the app's static directory, mirroring the reference's
/// `static_folder`. ⛔ Not one literal path: that narrows a published contract
/// and the narrowing is silent.
pub async fn static_asset(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response<Body> {
    match STATIC_ASSETS.iter().find(|(asset, _, _)| *asset == name) {
        Some((_, content_type, bytes)) => asset_response(bytes, content_type),
        None => Response::builder()
            .status(axum::http::StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(
                "<!doctype html>\n<html lang=en>\n<title>404 Not Found</title>\n<h1>Not Found</h1>\n<p>The requested URL was not found on the server. If you entered the URL manually please check your spelling and try again.</p>\n",
            ))
            .expect("health static 404 response"),
    }
}

fn asset_response(bytes: &'static [u8], content_type: &'static str) -> Response<Body> {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("embedded health asset response")
}
