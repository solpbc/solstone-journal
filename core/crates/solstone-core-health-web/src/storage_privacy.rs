// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    Json, Router,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use solstone_core_convey_http::window_catalog::{response_locale, text};
use solstone_core_system::storage_privacy::{inspect_in, unavailable_message};
use std::path::{Path, PathBuf};

pub fn snapshot(root: &Path, locale: &str) -> Value {
    match inspect_in(root, locale) {
        Ok(status) => json!(status),
        Err(error) => {
            log::warn!("storage privacy: {error}");
            json!({"private": false, "consented": false, "unavailable": true, "message": unavailable_message(locale)})
        }
    }
}

pub fn routes(root: PathBuf) -> Router {
    Router::new().route("/app/health/api/storage-privacy/consent", post(move |locale, Json(body): Json<Value>| {
        let locale = response_locale(locale);
        let root = root.clone();
        async move {
            match tokio::task::spawn_blocking(move || record_consent(&root, &body, locale)).await {
                Ok(response) => response,
                Err(error) => {
                    log::error!("storage privacy consent: {error}");
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": text(locale, "journal.window.health.storage.privacy.save.failed")}))).into_response()
                }
            }
        }
    }))
}

fn record_consent(root: &Path, body: &Value, locale: &str) -> Response {
    let status = match inspect_in(root, locale) {
        Ok(status) => status,
        Err(error) => {
            log::warn!("storage privacy consent: {error}");
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": unavailable_message(locale)})),
            )
                .into_response();
        }
    };
    if status.private || body["filesystem_type"].as_str() != Some(&status.filesystem_type) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": text(locale, "journal.window.health.storage.privacy.changed")})),
        )
            .into_response();
    }
    let consent = json!({
        "accepted_at": solstone_core_time::utc_rfc3339(jiff::Timestamp::now()),
        "filesystem_type": status.filesystem_type,
        "root": root,
    });
    match solstone_core_journal_config_write::mutate_journal_config(
        root,
        Default::default(),
        |config| {
            config.insert("storage_privacy_consent".into(), consent);
            solstone_core_journal_config_write::JournalConfigMutation {
                changed: true,
                value: (),
            }
        },
    ) {
        Ok(_) => Json(snapshot(root, locale)).into_response(),
        Err(error) => {
            log::warn!("storage privacy consent: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": text(locale, "journal.window.health.storage.privacy.save.failed")})),
            )
                .into_response()
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(unix)]
mod tests {
    use super::*;
    use solstone_core_convey_http::window_catalog::ENGLISH;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn consent_requires_the_displayed_storage_and_preserves_other_settings() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("config")).unwrap();
        std::fs::write(
            root.path().join("config/journal.json"),
            r#"{"owner":{"name":"someone"}}"#,
        )
        .unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let before = snapshot(root.path(), ENGLISH);
        assert_eq!(before["private"], false);
        assert_eq!(before["consented"], false);
        assert_eq!(
            record_consent(root.path(), &json!({"filesystem_type":"wrong"}), ENGLISH).status(),
            StatusCode::CONFLICT
        );
        assert_eq!(snapshot(root.path(), ENGLISH)["consented"], false);
        let response = record_consent(
            root.path(),
            &json!({"filesystem_type":before["filesystem_type"]}),
            ENGLISH,
        );
        assert_eq!(response.status(), StatusCode::OK);
        let after = snapshot(root.path(), ENGLISH);
        assert_eq!(after["consented"], true);
        assert_eq!(after["private"], false);
        let config: Value = serde_json::from_slice(
            &std::fs::read(root.path().join("config/journal.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(config["owner"]["name"], "someone");
        assert!(config["storage_privacy_consent"]["accepted_at"].is_string());
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(snapshot(root.path(), ENGLISH)["private"], true);
        assert_eq!(
            record_consent(
                root.path(),
                &json!({"filesystem_type":before["filesystem_type"]}),
                ENGLISH,
            )
            .status(),
            StatusCode::CONFLICT
        );
    }
}
