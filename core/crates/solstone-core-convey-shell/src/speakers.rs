// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::OnceLock;

use axum::Json;
use axum::body::Body;
use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::Local;
use serde_json::{Value, json};

use crate::asset_response;
use crate::assets;

fn speaker_copy() -> &'static Value {
    static COPY: OnceLock<Value> = OnceLock::new();
    COPY.get_or_init(|| {
        serde_json::from_str(assets::speaker_copy_json()).expect("generated speaker copy parses")
    })
}

pub async fn shell() -> Response {
    asset_response("/static/shell.html")
}

pub async fn shell_for_day(Path(day): Path<String>) -> Response {
    if day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit()) {
        return shell().await;
    }
    empty_day_not_found_response()
}

/// Match Python's bare `return "", 404`, deliberately unlike the HTML fallback.
fn empty_day_not_found_response() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::empty())
        .expect("empty speakers day response builds")
}

pub async fn workspace() -> Response {
    asset_response("/app/speakers/workspace")
}

pub async fn who_is_this() -> Response {
    asset_response("/app/speakers/static/who_is_this.js")
}

/// This wave always returns a null speaker filter because entity lookups are not ported.
/// That deliberately differs from Python's optional `?speaker=` resolution and is not corpus-tested.
pub async fn state() -> Response {
    Json(json!({
        "today": Local::now().format("%Y%m%d").to_string(),
        "owner_min_statements": 30,
        "owner_status_routing_tokens": {"candidate": "candidate", "confirmed": "confirmed"},
        "not_in_new_voices_copy": assets::not_in_new_voices_copy(),
        "speaker_copy": speaker_copy(),
        "speaker_filter_name": Value::Null,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct EstablishedJournal(PathBuf);

    impl EstablishedJournal {
        fn new() -> Self {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock is after epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "solstone-speakers-day-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("temporary journal creates");
            fs::create_dir(path.join("config")).expect("config directory creates");
            fs::write(
                path.join("config/journal.json"),
                br#"{"setup":{"completed_at":1767225600}}"#,
            )
            .expect("journal config writes");
            Self(path)
        }
    }

    impl Drop for EstablishedJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    async fn get(app: axum::Router, path: &str) -> axum::response::Response {
        app.oneshot(
            Request::get(path)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds")
    }

    #[tokio::test]
    async fn speakers_day_matches_python_empty_and_shell_responses() {
        let journal = EstablishedJournal::new();
        let app = crate::router(journal.0.clone());

        let invalid = get(app.clone(), "/app/speakers/notaday").await;
        assert_eq!(invalid.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            invalid.headers()["content-type"],
            "text/html; charset=utf-8"
        );
        assert!(
            to_bytes(invalid.into_body(), usize::MAX)
                .await
                .expect("invalid day body reads")
                .is_empty()
        );

        let valid = get(app.clone(), "/app/speakers/20260731").await;
        assert_eq!(valid.status(), StatusCode::OK);
        let valid_body = to_bytes(valid.into_body(), usize::MAX)
            .await
            .expect("valid day body reads");

        let shell = get(app, "/app/speakers/").await;
        assert_eq!(shell.status(), StatusCode::OK);
        let shell_body = to_bytes(shell.into_body(), usize::MAX)
            .await
            .expect("shell body reads");
        assert_eq!(valid_body, shell_body);
    }
}
