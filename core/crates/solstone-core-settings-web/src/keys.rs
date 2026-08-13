// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{body::Bytes, response::Response};
use serde_json::json;
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockError, LockOptions, mutate_journal_config,
};

use crate::http::{
    config_busy, json_response, plaud_validation_unavailable, settings_operation_failed,
};

/// W2 owns live third-party validation. W1 exposes only the empty-token shape.
pub async fn get() -> Response {
    json_response(json!({"key_validation": {}}))
}

pub async fn post(
    journal_root: std::path::PathBuf,
    lock_options: LockOptions,
    _body: Bytes,
) -> Response {
    let config = match solstone_core_journal_config::read_journal_config(&journal_root) {
        Ok(config) => config.config.unwrap_or_default(),
        Err(_) => return settings_operation_failed(),
    };
    let token = config
        .get("env")
        .and_then(|value| value.get("PLAUD_ACCESS_TOKEN"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim();
    if !token.is_empty() {
        return plaud_validation_unavailable();
    }
    match mutate_journal_config(&journal_root, lock_options, |config| {
        let existing = config
            .entry("service_key_validation".to_owned())
            .or_insert_with(|| json!({}));
        let changed = existing
            .as_object_mut()
            .is_some_and(|values| values.remove("plaud").is_some());
        JournalConfigMutation { changed, value: () }
    }) {
        Ok(_) => json_response(json!({"success": true, "key_validation": {}})),
        Err(solstone_core_journal_config_write::ConfigMutationError::Lock(LockError::Timeout(
            _,
        ))) => config_busy(),
        Err(_) => settings_operation_failed(),
    }
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
