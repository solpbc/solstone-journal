// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! `GET /api/shell/badges`: coalesced, non-blocking count summary for the
//! persistent navigation chrome (`home`, `curation`, and `import`).

use std::path::Path;
use std::sync::Arc;

use axum::extract::Extension;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use solstone_core_backup_web::BackupWebDeps;
use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};
use solstone_core_import_web::ImportRouteSlot;

use crate::JournalRoot;

pub(crate) fn routes(
    backup_deps: BackupWebDeps,
    import_route: Option<Arc<ImportRouteSlot>>,
) -> Router {
    Router::new().route(
        "/api/shell/badges",
        get(move |journal: Extension<Arc<JournalRoot>>| {
            get_badges(journal, backup_deps.clone(), import_route.clone())
        })
        .layer(solstone_core_convey_http::coalesce::layer()),
    )
}

async fn get_badges(
    Extension(journal): Extension<Arc<JournalRoot>>,
    backup_deps: BackupWebDeps,
    import_route: Option<Arc<ImportRouteSlot>>,
) -> Response {
    spawn_blocking_response(OwnerReadRole::ShellBadges, move || {
        Json(badges_payload(
            &journal.0,
            &backup_deps,
            import_route.as_deref(),
        ))
        .into_response()
    })
    .await
}

pub(crate) fn badges_payload(
    root: &Path,
    backup_deps: &BackupWebDeps,
    import_route: Option<&ImportRouteSlot>,
) -> Value {
    let summary = solstone_core_home::health_glance::status_badge_summary(root);
    let mut issues = summary.issues;
    let mut severity = summary.severity;
    let mut uncertain = summary.uncertain;

    #[cfg(feature = "host")]
    if let Some(security) = crate::security_advisory::status() {
        let flags = security["flags"].as_array().map_or(0, Vec::len);
        if flags > 0 {
            issues += flags;
            severity = Some("red");
        }
    }

    let backup = solstone_core_backup_web::home_backup_observation(backup_deps);
    if backup["available"].as_bool() == Some(true) {
        if backup["issue"].as_bool() == Some(true) {
            issues += 1;
            if severity != Some("red") {
                severity = Some("amber");
            }
        }
    } else {
        uncertain = true;
    }

    if issues == 0 && uncertain && severity.is_none() {
        severity = Some("amber");
    }

    let curation = solstone_core_facets_web::to_review_count(root);
    let import = solstone_core_import_web::pending_review_count(root, import_route);

    json!({
        "home": {
            "issues": issues,
            "severity": severity,
        },
        "curation": curation,
        "import": import,
    })
}

#[cfg(all(test, feature = "host"))]
mod tests {
    use std::fs;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};
    use solstone_core_convey_http::identity::AccessBasis;
    use tower::ServiceExt;

    #[tokio::test]
    async fn shell_badges_returns_calm_zeroes_on_fresh_established_journal_and_counts_review_work()
    {
        let dir = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
        fs::create_dir_all(dir.path().join("config")).expect("config dir");
        let now = jiff::Timestamp::now();
        fs::write(
            dir.path().join("config/journal.json"),
            serde_json::to_vec(&json!({"setup":{"completed_at": now.as_second()}})).unwrap(),
        )
        .expect("journal config");
        solstone_core_brain::begin_refresh(dir.path(), now, None, None, false, None)
            .expect("initial none brain record");

        let app = crate::router(dir.path().to_path_buf());
        let mut req = Request::get("/api/shell/badges")
            .body(Body::empty())
            .expect("request");
        req.extensions_mut().insert(AccessBasis::Localhost);
        let resp = app.clone().oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &to_bytes(resp.into_body(), usize::MAX)
                .await
                .expect("body bytes"),
        )
        .expect("json");
        assert_eq!(body["home"]["issues"], 0);
        assert!(body["home"]["severity"].is_null());
        assert_eq!(body["curation"], 0);
        assert_eq!(body["import"], 0);

        // Stage one confirmation-required import and one pending curation suggestion.
        let staged_id = "20261124_120000";
        let staged = dir.path().join("imports").join(staged_id);
        fs::create_dir_all(&staged).expect("staged import dir");
        let payload_file = staged.join("export.zip");
        fs::write(&payload_file, b"PK\x03\x04test").expect("payload");
        let source_hash = "a".repeat(64);
        let file_path = payload_file.to_string_lossy().into_owned();
        fs::write(
            staged.join("import.json"),
            serde_json::to_vec(&json!({
                "import_id": staged_id,
                "file_path": file_path,
                "file_size": 8,
                "source_hash": source_hash,
                "source_timestamp": "2026-11-24T12:00:00Z",
                "source_hint": "strava",
                "confirmation_required": true,
                "saved_start": {
                    "version": 1,
                    "import_id": staged_id,
                    "source_hash": source_hash,
                    "file_path": file_path,
                    "file_size": 8,
                    "consumed": false,
                },
            }))
            .unwrap(),
        )
        .expect("import.json");

        fs::create_dir_all(dir.path().join("facets")).expect("facets dir");
        fs::write(
            dir.path().join("facets/review-candidates.jsonl"),
            format!(
                "{}\n",
                json!({
                    "name_key": "rock_climbing",
                    "name": "Rock Climbing",
                    "status": "open",
                    "count": 3
                })
            ),
        )
        .expect("review-candidates.jsonl");

        let mut req = Request::get("/api/shell/badges")
            .body(Body::empty())
            .expect("request");
        req.extensions_mut().insert(AccessBasis::Localhost);
        let resp = app.oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &to_bytes(resp.into_body(), usize::MAX)
                .await
                .expect("body bytes"),
        )
        .expect("json");
        assert_eq!(body["curation"], 1);
        assert_eq!(body["import"], 1);
    }
}
