// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::asset_response;

pub async fn shell() -> Response {
    asset_response("/static/shell.html")
}

pub async fn trends() -> Response {
    shell().await
}

pub async fn shell_for_day(Path(day): Path<String>) -> Response {
    if chrono::NaiveDate::parse_from_str(&day, "%Y%m%d").is_ok() {
        return shell().await;
    }
    day_refusal()
}

pub async fn workspace() -> Response {
    asset_response("/app/body/workspace")
}

pub async fn background() -> Response {
    crate::not_found_response()
}

fn day_refusal() -> Response {
    solstone_core_convey_http::envelope::error_envelope(
        "invalid_day",
        "that day couldn't be used.",
        "Invalid day",
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use serde_json::Value;
    use tower::ServiceExt;

    struct EstablishedJournal(tempfile::TempDir);

    impl EstablishedJournal {
        fn new(config: &[u8]) -> Self {
            let dir = tempfile::TempDir::new_in("/var/tmp").expect("temporary root creates");
            fs::create_dir(dir.path().join("config")).expect("config directory creates");
            fs::write(dir.path().join("config/journal.json"), config).expect("config writes");
            Self(dir)
        }

        fn established() -> Self {
            Self::new(br#"{"setup":{"completed_at":1767225600}}"#)
        }
    }

    async fn get(app: axum::Router, path: &str) -> (StatusCode, String, Vec<u8>) {
        let response = app
            .oneshot(
                Request::get(path)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads")
            .to_vec();
        (status, content_type, body)
    }

    fn json_body(body: &[u8]) -> Value {
        serde_json::from_slice(body).expect("response JSON parses")
    }

    #[tokio::test]
    async fn body_documents_are_embedded_and_day_validation_is_calendar_aware() {
        let journal = EstablishedJournal::established();
        let app = crate::router(journal.0.path().to_path_buf());
        let root = get(app.clone(), "/app/body/").await;
        assert_eq!(root.0, StatusCode::OK);
        assert_eq!(root.2, include_bytes!("../assets/static/shell.html"));
        let trends = get(app.clone(), "/app/body/trends").await;
        assert_eq!(trends.0, StatusCode::OK);
        assert_eq!(trends.2, root.2);
        let workspace = get(app.clone(), "/app/body/workspace").await;
        assert_eq!(workspace.0, StatusCode::OK);
        let generated = include_str!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));
        let entry = generated
            .lines()
            .find(|line| line.contains("path: \"/app/body/workspace\""))
            .expect("body workspace is embedded");
        assert!(entry.contains(env!("CARGO_MANIFEST_DIR")));
        assert!(entry.contains("assets/body/"));
        assert!(!entry.contains("solstone/apps/"));
        for path in ["/app/body/notaday", "/app/body/20260231"] {
            let refusal = get(app.clone(), path).await;
            assert_eq!(refusal.0, StatusCode::BAD_REQUEST, "{path}");
            assert_eq!(json_body(&refusal.2)["reason_code"], "invalid_day");
        }
    }

    #[tokio::test]
    async fn background_404_and_day_refusal_are_distinct() {
        let journal = EstablishedJournal::established();
        let app = crate::router(journal.0.path().to_path_buf());
        let background = get(app.clone(), "/app/body/background").await;
        assert_eq!(background.0, StatusCode::NOT_FOUND);
        assert_eq!(background.1, "text/html; charset=utf-8");
        assert_eq!(
            background.2,
            to_bytes(crate::not_found_response().into_body(), usize::MAX)
                .await
                .unwrap()
        );
        let invalid_day = get(app, "/app/body/notaday").await;
        assert_eq!(invalid_day.0, StatusCode::BAD_REQUEST);
        assert_eq!(json_body(&invalid_day.2)["reason_code"], "invalid_day");
    }

    #[tokio::test]
    async fn all_body_api_routes_are_native() {
        let journal = EstablishedJournal::established();
        let app = crate::router(journal.0.path().to_path_buf());
        let expected = serde_json::to_value(crate::refusal::AppNotConverted::new("body"))
            .expect("refusal serializes");
        for path in [
            "/app/body/api/status",
            "/app/body/api/recent",
            "/app/body/api/window?from=2026-08-01T00%3A00%3A00%2B00%3A00&to=2026-08-02T00%3A00%3A00%2B00%3A00",
        ] {
            let response = get(app.clone(), path).await;
            assert_ne!(response.0, StatusCode::NOT_IMPLEMENTED, "{path}");
            assert_ne!(json_body(&response.2), expected, "{path}");
        }
    }

    #[tokio::test]
    async fn body_conversion_leaves_other_apps_unconverted_and_unknown_body_api_html_404() {
        let journal = EstablishedJournal::established();
        let app = crate::router(journal.0.path().to_path_buf());
        assert_eq!(get(app.clone(), "/app/body/").await.0, StatusCode::OK);
        assert_eq!(
            get(app.clone(), "/app/activities/").await.0,
            StatusCode::NOT_IMPLEMENTED
        );
        for path in [
            "/app/body/",
            "/app/body/trends",
            "/app/body/20260801",
            "/app/body/workspace",
            "/app/body/background",
            "/app/body/api/index",
            "/app/body/api/stats/202608",
            "/app/body/api/status",
            "/app/body/api/recent",
            "/app/body/api/window?from=2026-08-01T00%3A00%3A00%2B00%3A00&to=2026-08-02T00%3A00%3A00%2B00%3A00",
        ] {
            let response = get(app.clone(), path).await;
            assert_ne!(
                json_body_or_null(&response.2).and_then(|value| value.get("reason_code").cloned()),
                Some(Value::String("app_not_converted".to_owned())),
                "{path}"
            );
        }
        let unknown = get(app, "/app/body/api/nope").await;
        assert_eq!(unknown.0, StatusCode::NOT_FOUND);
        assert_eq!(unknown.1, "text/html; charset=utf-8");
    }

    #[tokio::test]
    async fn session_gate_preserves_document_and_api_outcomes() {
        for (config, expected_status) in [
            (br#"{}"#.as_slice(), StatusCode::FOUND),
            (br#"not json"#.as_slice(), StatusCode::INTERNAL_SERVER_ERROR),
        ] {
            let journal = EstablishedJournal::new(config);
            for path in [
                "/app/body/",
                "/app/body/api/index",
                "/app/body/api/day/20260801",
                "/app/body/api/status",
                "/app/body/api/recent",
                "/app/body/api/window?from=2026-08-01T00%3A00%3A00%2B00%3A00&to=2026-08-02T00%3A00%3A00%2B00%3A00",
            ] {
                let response = get(crate::router(journal.0.path().to_path_buf()), path).await;
                assert_eq!(response.0, expected_status, "{path}");
                if expected_status == StatusCode::INTERNAL_SERVER_ERROR {
                    if path.contains("/api/") {
                        assert_eq!(json_body(&response.2)["reason_code"], "corrupt_config");
                        assert_eq!(
                            json_body(&response.2)["detail"],
                            crate::session::corrupt_config_detail(journal.0.path()),
                            "{path} preserves the session gate detail byte-for-byte"
                        );
                    } else {
                        assert_eq!(response.1, "text/plain; charset=utf-8");
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn bare_converted_app_paths_permanently_redirect_to_the_trailing_slash() {
        let journal = EstablishedJournal::established();
        for path in ["/app/body", "/app/speakers", "/app/health"] {
            let response = crate::router(journal.0.path().to_path_buf())
                .oneshot(
                    Request::get(path)
                        .body(Body::empty())
                        .expect("request builds"),
                )
                .await
                .expect("router responds");
            assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT, "{path}");
            assert_eq!(
                response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok()),
                Some(format!("{path}/").as_str()),
                "{path}"
            );
        }
    }

    fn json_body_or_null(body: &[u8]) -> Option<Value> {
        serde_json::from_slice(body).ok()
    }
}
