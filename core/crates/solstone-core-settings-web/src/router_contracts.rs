// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use tower::ServiceExt;

use crate::test_support::{corrupt_root, established_root, shell_router};

#[tokio::test]
async fn ac14_shell_router_applies_session_and_corrupt_contracts() {
    let unestablished = tempfile::TempDir::new().expect("temporary journal");
    let redirect = shell_router(unestablished.path())
        .oneshot(
            Request::get("/app/settings/api/config")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(redirect.status(), StatusCode::FOUND);
    assert_eq!(redirect.headers()[header::LOCATION], "/init");

    let established = established_root();
    let success = shell_router(established.path())
        .oneshot(
            Request::get("/app/settings/api/config")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(success.status(), StatusCode::OK);
    assert_eq!(success.headers()[header::CONTENT_TYPE], "application/json");

    let corrupt = corrupt_root();
    let api = shell_router(corrupt.path())
        .oneshot(
            Request::get("/app/settings/api/config")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(api.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(api.headers()[header::CONTENT_TYPE], "application/json");
    for path in [
        "/app/settings/",
        "/app/settings/facets/work-life",
        "/app/settings/workspace",
        "/app/settings/static/settings.js",
    ] {
        let response = shell_router(corrupt.path())
            .oneshot(Request::get(path).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "{path}"
        );
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; charset=utf-8",
            "{path}"
        );
        assert!(
            String::from_utf8(
                to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body")
                    .to_vec()
            )
            .expect("UTF-8")
            .contains("Your settings were NOT changed")
        );
    }
}
