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
use std::sync::atomic::{AtomicBool, Ordering};

/// A sync this server started is still running. The hourly run is its own process.
static SYNCING: AtomicBool = AtomicBool::new(false);

fn failed(error: granola::Error) -> Response {
    refused(error, error.message())
}
fn refused(error: granola::Error, message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(json!({"error":message,"code":error.code()})),
    )
        .into_response()
}
/// What a key Granola refuses means while the owner is pasting it.
const fn pasted(error: granola::Error) -> &'static str {
    match error {
        granola::Error::Key => {
            "Granola didn't accept that key. copy it again from Granola, or create a new one."
        }
        granola::Error::Access => {
            "Granola turned that key away. API access needs Granola Business or Enterprise, and on Enterprise an admin may need to allow it."
        }
        other => other.message(),
    }
}
/// Run the source in the background unless this server already is.
fn start_sync(root: PathBuf) -> bool {
    if SYNCING.swap(true, Ordering::SeqCst) {
        return false;
    }
    tokio::task::spawn_blocking(move || {
        if let Err(error) = granola::sync(&root) {
            log::warn!("{error}");
        }
        SYNCING.store(false, Ordering::SeqCst);
    });
    true
}
pub async fn get(root: PathBuf) -> Response {
    match granola::status(&root) {
        Ok(mut value) => {
            value["syncing"] = json!(SYNCING.load(Ordering::SeqCst));
            json_response(value)
        }
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
        .map(|key| key.trim().to_owned())
    else {
        return failed(granola::Error::InvalidKey);
    };
    if let Err(error) = granola::validate_key(&key) {
        return failed(error);
    }
    // A key Granola refuses never replaces the saved one. If Granola can't be
    // asked right now, the key is saved and the sync reports what it finds.
    let probe = key.clone();
    match tokio::task::spawn_blocking(move || granola::check_key(&probe)).await {
        Ok(Err(error @ (granola::Error::Key | granola::Error::Access))) => {
            return refused(error, pasted(error));
        }
        Ok(_) => {}
        Err(_) => return failed(granola::Error::Storage),
    }
    let sync_root = root.clone();
    let result = tokio::task::spawn_blocking(move || change(&root, Some(&key))).await;
    match result {
        Ok(Ok(())) => {
            start_sync(sync_root);
            json_response(json!({"success":true}))
        }
        Ok(Err(error)) => failed(error),
        Err(_) => failed(granola::Error::Storage),
    }
}
/// Check Granola now rather than at the next hourly run.
pub async fn sync_now(root: PathBuf) -> Response {
    let status_root = root.clone();
    match tokio::task::spawn_blocking(move || granola::status(&status_root)).await {
        Ok(Ok(value)) if value["connected"] == json!(true) => {}
        Ok(Ok(_)) => return failed(granola::Error::InvalidKey),
        Ok(Err(error)) => return failed(error),
        Err(_) => return failed(granola::Error::Storage),
    }
    if !start_sync(root) {
        return failed(granola::Error::Busy);
    }
    json_response(json!({"success":true}))
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
        assert_eq!(status["syncing"], false);
        assert_eq!(status["meetings"], 0);
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
            .clone()
            .oneshot(
                Request::delete("/app/settings/api/granola")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(granola::status(root.path()).unwrap()["connected"], false);
        // Nothing to check without a key, and nothing starts.
        let response = router
            .clone()
            .oneshot(
                Request::post("/app/settings/api/granola/sync")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(SYNCING.load(Ordering::SeqCst), false);
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
