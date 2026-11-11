// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    http::json_response,
    request_body::{JsonBody, json_body},
};
use axum::{
    body::Bytes,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use solstone_core_import::granola;
use std::path::PathBuf;

fn failed(error: granola::Error) -> Response {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(json!({"error":error.message()})),
    )
        .into_response()
}
pub async fn get(root: PathBuf) -> Response {
    match granola::status(&root) {
        Ok(value) => json_response(value),
        Err(error) => failed(error),
    }
}
pub async fn connect(root: PathBuf, body: Bytes) -> Response {
    let JsonBody::Value(value) = json_body(body) else {
        return failed(granola::Error::InvalidKey);
    };
    let Some(key) = value
        .get("api_key")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return failed(granola::Error::InvalidKey);
    };
    let sync_root = root.clone();
    let result = tokio::task::spawn_blocking(move || change(&root, Some(&key))).await;
    match result {
        Ok(Ok(())) => {
            tokio::task::spawn_blocking(move || {
                if let Err(error) = granola::sync(&sync_root) {
                    log::warn!("{error}");
                }
            });
            json_response(json!({"success":true}))
        }
        Ok(Err(error)) => failed(error),
        Err(_) => failed(granola::Error::Storage),
    }
}
pub async fn disconnect(root: PathBuf) -> Response {
    let result = tokio::task::spawn_blocking(move || change(&root, None)).await;
    match result {
        Ok(Ok(())) => json_response(json!({"success":true})),
        Ok(Err(error)) => failed(error),
        Err(_) => failed(granola::Error::Storage),
    }
}
fn change(root: &std::path::Path, key: Option<&str>) -> Result<(), granola::Error> {
    if let Some(key) = key {
        granola::validate_key(key)?;
    }
    // Removing the key stops the running source at its next request.
    if key.is_none() {
        granola::set_key(root, None)?;
    }
    let _hold = granola::hold(root)?;
    if key.is_none() {
        granola::set_key(root, None)?;
    }
    solstone_core_system::schedule::mutate_schedule_entries(&root.join("config/schedules.json"),|entries| {
        if key.is_some() {
            entries.insert("sync:granola".into(),json!({"cmd":["solstone","journal","importer","--sync","granola","--save"],"every":"hourly"}));
        } else { entries.remove("sync:granola"); }
        solstone_core_system::schedule::ScheduleMutation { changed:true,value:() }
    }).map_err(|_|granola::Error::Storage)?;
    if let Some(key) = key {
        granola::set_key(root, Some(key))?;
    }
    drop(_hold);
    Ok(())
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn connection_schedules_the_source_masks_its_key_and_disconnect_removes_both() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("config")).unwrap();
        std::fs::write(root.path().join("config/journal.json"), "{}").unwrap();
        change(root.path(), Some("grn_personal_test_secret")).unwrap();
        let schedules: Value = serde_json::from_slice(
            &std::fs::read(root.path().join("config/schedules.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            schedules["sync:granola"]["cmd"],
            json!([
                "solstone", "journal", "importer", "--sync", "granola", "--save"
            ])
        );
        let config = solstone_core_journal_config::read_journal_config(root.path())
            .unwrap()
            .config
            .unwrap();
        let projected = crate::config::project_public_config(config).unwrap();
        assert!(
            !serde_json::to_string(&projected)
                .unwrap()
                .contains("grn_personal_test_secret")
        );
        let router = crate::routes(root.path().to_path_buf());
        let response = router
            .clone()
            .oneshot(
                Request::get("/app/settings/api/granola")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let status: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(status["connected"], true);
        assert!(!String::from_utf8_lossy(&body).contains("grn_personal_test_secret"));
        let response = router
            .clone()
            .oneshot(
                Request::post("/app/settings/api/granola")
                    .body(Body::from(r#"{"api_key":"invalid"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = router
            .oneshot(
                Request::delete("/app/settings/api/granola")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(granola::status(root.path()).unwrap()["connected"], false);
        let schedules: Value = serde_json::from_slice(
            &std::fs::read(root.path().join("config/schedules.json")).unwrap(),
        )
        .unwrap();
        assert!(schedules.get("sync:granola").is_none());
        assert!(
            !std::fs::read_to_string(root.path().join("config/journal.json"))
                .unwrap()
                .contains("grn_personal_test_secret")
        );
    }
}
