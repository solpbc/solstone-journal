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
        get(
            move |locale: Option<
                Extension<solstone_core_convey_http::coalesce::ResponseLocale>,
            >,
                  journal: Extension<Arc<JournalRoot>>| {
                get_badges(locale, journal, backup_deps.clone(), import_route.clone())
            },
        )
        .layer(solstone_core_convey_http::coalesce::layer()),
    )
}

async fn get_badges(
    locale: Option<Extension<solstone_core_convey_http::coalesce::ResponseLocale>>,
    Extension(journal): Extension<Arc<JournalRoot>>,
    backup_deps: BackupWebDeps,
    import_route: Option<Arc<ImportRouteSlot>>,
) -> Response {
    let locale = crate::window_catalog::response_locale(locale);
    spawn_blocking_response(OwnerReadRole::ShellBadges, move || {
        Json(badges_payload_in(
            &journal.0,
            &backup_deps,
            import_route.as_deref(),
            locale,
        ))
        .into_response()
    })
    .await
}

pub(crate) fn apply_security_to_home_badge(
    security: Option<&Value>,
    issues: &mut usize,
    severity: &mut Option<&'static str>,
    uncertain: &mut bool,
) {
    let Some(security) = security else {
        return;
    };
    let flags = security["flags"].as_array().map_or(0, Vec::len);
    if flags > 0 {
        *issues += flags;
        *severity = Some("red");
    } else if security["state"].as_str() == Some("unconfirmed") {
        *uncertain = true;
    }
}

pub(crate) fn badges_payload_in(
    root: &Path,
    backup_deps: &BackupWebDeps,
    import_route: Option<&ImportRouteSlot>,
    locale: &str,
) -> Value {
    let summary = solstone_core_home::health_glance::status_badge_summary(root);
    let mut issues = summary.issues;
    let mut severity = summary.severity;
    let mut uncertain = summary.uncertain;

    #[cfg(feature = "host")]
    {
        let security = crate::security_advisory::status();
        apply_security_to_home_badge(
            security.as_ref(),
            &mut issues,
            &mut severity,
            &mut uncertain,
        );
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

    let email_attention = usize::from(solstone_core_settings_web::email_needs_attention(root));
    let photos_attention = usize::from(
        solstone_core_photos_library::photos_library_enabled(root)
            && solstone_core_photos_library::dashboard_state(root)
                .is_ok_and(|s| s.state == "needs_attention"),
    );
    let sources_attention = email_attention + photos_attention;
    if sources_attention > 0 {
        issues += sources_attention;
        if severity != Some("red") {
            severity = Some("amber");
        }
    }

    if issues == 0 && uncertain && severity.is_none() {
        severity = Some("amber");
    }

    let curation = solstone_core_facets_web::to_review_count(root);
    let (import, import_reviews) =
        solstone_core_import_web::pending_review_summaries_in(root, import_route, locale);

    json!({
        "home": {
            "issues": issues,
            "severity": severity,
        },
        "curation": curation,
        "import": import,
        "import_reviews": import_reviews,
        "sources": sources_attention,
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
        let parent = fs::canonicalize(std::env::temp_dir()).expect("temporary directory");
        let dir = tempfile::TempDir::new_in(parent).expect("journal root");
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
        let reviews = body["import_reviews"].as_array().expect("import_reviews");
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0]["id"], staged_id);
        assert_eq!(reviews[0]["swept_after_hour"], true);
    }

    #[test]
    fn security_unconfirmed_marks_home_badge_uncertain_while_checking_stays_clean() {
        let mut issues = 0;
        let mut severity = None;
        let mut uncertain = false;
        super::apply_security_to_home_badge(
            Some(&json!({"state": "checking", "flags": []})),
            &mut issues,
            &mut severity,
            &mut uncertain,
        );
        assert_eq!(issues, 0);
        assert!(severity.is_none());
        assert!(!uncertain);

        super::apply_security_to_home_badge(
            Some(&json!({"state": "unconfirmed", "flags": []})),
            &mut issues,
            &mut severity,
            &mut uncertain,
        );
        assert_eq!(issues, 0);
        assert!(uncertain);
    }

    #[tokio::test]
    async fn shell_badges_reflects_incomplete_and_urgent_integrity_receipts() {
        let parent = fs::canonicalize(std::env::temp_dir()).expect("temporary directory");
        let dir = tempfile::TempDir::new_in(parent).expect("journal root");
        fs::create_dir_all(dir.path().join("config")).expect("config dir");
        fs::create_dir_all(dir.path().join("health")).expect("health dir");
        let now = jiff::Timestamp::now();
        fs::write(
            dir.path().join("config/journal.json"),
            serde_json::to_vec(&json!({"setup":{"completed_at": now.as_second()}})).unwrap(),
        )
        .expect("journal config");
        solstone_core_brain::begin_refresh(dir.path(), now, None, None, false, None)
            .expect("initial none brain record");

        // Incomplete integrity check -> uncertain amber badge with 0 issues.
        fs::write(
            dir.path().join("health/integrity.json"),
            serde_json::to_vec(&json!({
                "schema_version": 1,
                "state": "incomplete",
                "scope_kind": "routine",
                "checked_at_ms": now.as_millisecond(),
                "duration_ms": 10,
                "days_checked": 3,
                "days_total": 10,
                "days_unreadable": 0,
                "segments_checked": 5,
                "incomplete": true,
                "unavailable": false,
                "urgent_count": 0,
                "maintenance_count": 0,
                "full_coverage_complete": false,
                "findings": []
            }))
            .unwrap(),
        )
        .expect("write incomplete receipt");

        let app = crate::router(dir.path().to_path_buf());
        let mut req = Request::get("/api/shell/badges")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(AccessBasis::Localhost);
        let resp = app.clone().oneshot(req).await.unwrap();
        let body: Value =
            serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(body["home"]["issues"], 0);
        assert_eq!(body["home"]["severity"], "amber");

        // Urgent integrity finding -> red issue count on home badge.
        fs::write(
            dir.path().join("health/integrity.json"),
            serde_json::to_vec(&json!({
                "schema_version": 1,
                "state": "needs_attention",
                "scope_kind": "routine",
                "checked_at_ms": now.as_millisecond(),
                "duration_ms": 10,
                "days_checked": 10,
                "days_total": 10,
                "days_unreadable": 0,
                "segments_checked": 5,
                "incomplete": false,
                "unavailable": false,
                "urgent_count": 1,
                "maintenance_count": 0,
                "full_coverage_complete": true,
                "findings": [{
                    "tier": 1,
                    "kind": "missing_original",
                    "day": "20261120"
                }]
            }))
            .unwrap(),
        )
        .expect("write urgent receipt");

        let mut req = Request::get("/api/shell/badges")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(AccessBasis::Localhost);
        let resp = app.oneshot(req).await.unwrap();
        let body: Value =
            serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(body["home"]["issues"], 1);
        assert_eq!(body["home"]["severity"], "red");
    }

    #[tokio::test]
    async fn shell_badges_surfaces_email_needs_attention_on_sources_and_home() {
        let parent = fs::canonicalize(std::env::temp_dir()).expect("temporary directory");
        let dir = tempfile::TempDir::new_in(parent).expect("journal root");
        fs::create_dir_all(dir.path().join("config/email")).expect("config/email dir");
        let now = jiff::Timestamp::now();
        fs::write(
            dir.path().join("config/journal.json"),
            serde_json::to_vec(&json!({
                "setup": { "completed_at": now.as_second() },
                "email": {
                    "provider": "gmail",
                    "account": "maya@example.test",
                    "app_password": "abcdefghijklmnop"
                }
            }))
            .unwrap(),
        )
        .expect("journal config");
        fs::write(
            dir.path().join("config/email/state.json"),
            serde_json::to_vec(&json!({
                "phase": "needs_attention",
                "attention_reason": "password_rejected"
            }))
            .unwrap(),
        )
        .expect("email state");
        solstone_core_brain::begin_refresh(dir.path(), now, None, None, false, None)
            .expect("initial none brain record");

        let app = crate::router(dir.path().to_path_buf());
        let mut req = Request::get("/api/shell/badges")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(AccessBasis::Localhost);
        let resp = app.oneshot(req).await.unwrap();
        let body: Value =
            serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(body["sources"], 1);
        assert_eq!(body["home"]["issues"], 1);
        assert_eq!(body["home"]["severity"], "amber");
    }

    #[tokio::test]
    async fn shell_badges_gates_photos_needs_attention_behind_feature_flag() {
        let parent = fs::canonicalize(std::env::temp_dir()).expect("temporary directory");
        let dir = tempfile::TempDir::new_in(parent).expect("journal root");
        fs::create_dir_all(dir.path().join("config")).expect("config dir");
        let now = jiff::Timestamp::now();
        fs::write(
            dir.path().join("config/journal.json"),
            serde_json::to_vec(&json!({
                "setup": { "completed_at": now.as_second() }
            }))
            .unwrap(),
        )
        .expect("journal config off");
        solstone_core_photos_library::scan_start(
            dir.path(),
            "sha256:test-device",
            "Test Phone",
            "iphone",
            "limited",
            10,
            Vec::new(),
        )
        .expect("photos scan_start");
        solstone_core_brain::begin_refresh(dir.path(), now, None, None, false, None)
            .expect("initial none brain record");

        let app = crate::router(dir.path().to_path_buf());
        let mut req_off = Request::get("/api/shell/badges")
            .body(Body::empty())
            .unwrap();
        req_off.extensions_mut().insert(AccessBasis::Localhost);
        let resp_off = app.clone().oneshot(req_off).await.unwrap();
        let body_off: Value =
            serde_json::from_slice(&to_bytes(resp_off.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body_off["sources"], 0);
        assert_eq!(body_off["home"]["issues"], 0);

        fs::write(
            dir.path().join("config/journal.json"),
            serde_json::to_vec(&json!({
                "setup": { "completed_at": now.as_second() },
                "photos_library": { "enabled": true }
            }))
            .unwrap(),
        )
        .expect("journal config on");
        let mut req_on = Request::get("/api/shell/badges")
            .body(Body::empty())
            .unwrap();
        req_on.extensions_mut().insert(AccessBasis::Localhost);
        let resp_on = app.oneshot(req_on).await.unwrap();
        let body_on: Value =
            serde_json::from_slice(&to_bytes(resp_on.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body_on["sources"], 1);
        assert_eq!(body_on["home"]["issues"], 1);
        assert_eq!(body_on["home"]["severity"], "amber");
    }
}
