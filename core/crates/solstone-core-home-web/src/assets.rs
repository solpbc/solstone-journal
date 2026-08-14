// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::Body,
    http::{Response, header},
};

// solstone/convey/static/shell.html is this crate's only out-of-crate compile input.
const SHELL: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/convey/static/shell.html"
));
const WORKSPACE: &[u8] = include_bytes!("../assets/workspace.html");
const HOME_JS: &[u8] = include_bytes!("../assets/home.js");

pub async fn shell() -> Response<Body> {
    asset(SHELL, "text/html; charset=utf-8")
}

pub async fn workspace() -> Response<Body> {
    asset(WORKSPACE, "text/html; charset=utf-8")
}

pub async fn home_js() -> Response<Body> {
    asset(HOME_JS, "text/javascript; charset=utf-8")
}

fn asset(bytes: &'static [u8], content_type: &'static str) -> Response<Body> {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("embedded home asset response")
}

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_assets_match_python_reference_sources() {
        // Retire this workspace half when the retention approval surface intentionally
        // adds one <script> line; that is new owner-facing work, not a port divergence.
        // Replace it with an assertion naming that line. home.js remains byte-identical.
        assert_eq!(
            include_bytes!("../assets/workspace.html"),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../solstone/apps/home/workspace.html"
            )),
        );
        assert_eq!(
            include_bytes!("../assets/home.js"),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../solstone/apps/home/static/home.js"
            )),
        );
    }
}
