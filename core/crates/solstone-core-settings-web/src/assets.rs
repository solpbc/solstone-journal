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

    use super::WORKSPACE;
    use crate::test_support::{established_root, shell_router};

    #[tokio::test]
    async fn ac13_assets_match_python_sources_and_workspace_serves_embedded_bytes() {
        // Retire this half when the settings Python surface is deleted; until then it makes that cut safe.
        assert_eq!(
            include_bytes!("../assets/workspace.html"),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../solstone/apps/settings/workspace.html"
            )),
        );
        assert_eq!(
            include_bytes!("../assets/settings.js"),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../solstone/apps/settings/static/settings.js"
            )),
        );
        assert_eq!(
            include_bytes!("../assets/copy.py"),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../solstone/apps/settings/copy.py"
            )),
        );
        let root = established_root();
        let response = shell_router(root.path())
            .oneshot(
                Request::get("/app/settings/workspace")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
            WORKSPACE
        );
    }
}
