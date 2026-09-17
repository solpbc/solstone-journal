// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::Body,
    http::{Response, header},
};

const SHELL: &[u8] = include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
const WORKSPACE: &[u8] = include_bytes!("../assets/workspace.html");
const SETTINGS_JS: &[u8] = include_bytes!("../assets/settings.js");

pub async fn shell() -> Response<Body> {
    asset(SHELL, "text/html; charset=utf-8")
}

pub async fn workspace() -> Response<Body> {
    asset(WORKSPACE, "text/html; charset=utf-8")
}

pub async fn settings_js() -> Response<Body> {
    asset(SETTINGS_JS, "text/javascript; charset=utf-8")
}

fn asset(bytes: &'static [u8], content_type: &'static str) -> Response<Body> {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("embedded settings asset response")
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, http::Request};
    use tower::ServiceExt;

    use super::{SETTINGS_JS, WORKSPACE};
    use crate::test_support::{established_root, shell_router};

    #[tokio::test]
    async fn ac13_embedded_assets_are_served_verbatim() {
        let root = established_root();
        let router = shell_router(root.path());
        let workspace = router
            .clone()
            .oneshot(
                Request::get("/app/settings/workspace")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            to_bytes(workspace.into_body(), usize::MAX)
                .await
                .expect("body"),
            WORKSPACE
        );
        let settings_js = router
            .oneshot(
                Request::get("/app/settings/static/settings.js")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            to_bytes(settings_js.into_body(), usize::MAX)
                .await
                .expect("body"),
            SETTINGS_JS
        );
    }
}
