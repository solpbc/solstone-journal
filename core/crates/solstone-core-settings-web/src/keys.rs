// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::response::Response;
use serde_json::json;

use crate::http::json_response;

/// W2 owns live third-party validation. W1 exposes only the empty-token shape.
pub async fn get() -> Response {
    json_response(json!({"key_validation": {}}))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn ac6_config_validation_projection_and_empty_revalidation_are_distinct() {
        let root = crate::test_support::phase_root("rich");
        let router = crate::test_support::shell_router(root.path());
        let config = router
            .clone()
            .oneshot(
                Request::get("/app/settings/api/config")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let config: serde_json::Value = serde_json::from_slice(
            &to_bytes(config.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert!(config["key_validation"].get("plaud").is_some());
        assert!(config["key_validation"].get("bogus").is_none());
        assert!(config.get("service_key_validation").is_none());
        let validation = router
            .oneshot(
                Request::get("/app/settings/api/validate-keys")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let validation: serde_json::Value = serde_json::from_slice(
            &to_bytes(validation.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(validation, serde_json::json!({"key_validation": {}}));
    }
}
