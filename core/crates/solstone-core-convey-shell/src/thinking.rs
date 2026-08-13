// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read routes for Thinking.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Extension, Query};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;

use crate::{JournalRoot, asset_response};

pub fn router(journal: Arc<JournalRoot>) -> Router {
    Router::new()
        .route("/app/thinking/", get(shell))
        .route("/app/thinking", get(shell_redirect))
        .route("/app/thinking/workspace", get(workspace))
        .route("/app/thinking/static/thinking.js", get(script))
        .route("/app/thinking/api/state", get(state))
        .route("/app/thinking/api/providers", get(providers))
        .route("/app/thinking/api/keys", get(keys))
        .route(
            "/app/thinking/api/providers/local/status",
            get(local_status),
        )
        .route("/app/thinking/api/local/availability", get(availability))
        .route(
            "/app/thinking/api/local/bootstrap/status",
            get(bootstrap_status),
        )
        .route("/app/thinking/api/local/models", get(models))
        .route("/app/thinking/api/local/runtime", get(runtime))
        .route("/app/thinking/api/generators", get(generators))
        .route("/app/thinking/api/validate-keys", get(validate_keys))
        .layer(Extension(journal))
}

async fn shell() -> Response {
    asset_response("/static/shell.html")
}
async fn shell_redirect() -> Response {
    let location = "http://localhost/app/thinking/";
    Response::builder()
        .status(StatusCode::PERMANENT_REDIRECT)
        .header(header::LOCATION, location)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(format!(
            "<!doctype html>\n<html lang=en>\n<title>Redirecting...</title>\n<h1>Redirecting...</h1>\n<p>You should be redirected automatically to the target URL: <a href=\"{location}\">{location}</a>. If not, click the link.\n"
        )))
        .expect("redirect builds")
}
async fn workspace() -> Response {
    asset_response("/app/thinking/workspace")
}
async fn script() -> Response {
    asset_response("/app/thinking/static/thinking.js")
}

async fn state(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let journal = journal.as_ref();
    match config(&journal.0) {
        Ok(config) => json_response(
            json!({"providers":solstone_core_thinking::providers::payload(&journal.0,&config,solstone_core_thinking::local::default_model()),"keys":solstone_core_thinking::providers::keys(&config),"copy":solstone_core_thinking_copy::thinking_copy_payload()}),
        ),
        Err(response) => *response,
    }
}
async fn providers(
    Extension(journal): Extension<Arc<JournalRoot>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let journal = journal.as_ref();
    let Some(model) =
        solstone_core_thinking::local::accepted_model(query.get("local_model").map(String::as_str))
    else {
        return json_error(solstone_core_thinking::local::invalid_model(
            query
                .get("local_model")
                .map(String::as_str)
                .unwrap_or_default(),
        ));
    };
    match config(&journal.0) {
        Ok(config) => json_response(solstone_core_thinking::providers::payload(
            &journal.0, &config, model,
        )),
        Err(response) => *response,
    }
}
async fn keys(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let journal = journal.as_ref();
    match config(&journal.0) {
        Ok(config) => json_response(solstone_core_thinking::providers::keys(&config)),
        Err(response) => *response,
    }
}
async fn local_status(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let journal = journal.as_ref();
    match config(&journal.0) {
        Ok(config) => json_response(solstone_core_thinking::providers::local_status_only(
            &journal.0, &config,
        )),
        Err(response) => *response,
    }
}
async fn availability(
    Extension(journal): Extension<Arc<JournalRoot>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let journal = journal.as_ref();
    model_response(
        &journal.0,
        query.get("model").map(String::as_str),
        |model| solstone_core_thinking::local::availability(&journal.0, model),
    )
}
async fn bootstrap_status(
    Extension(journal): Extension<Arc<JournalRoot>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let journal = journal.as_ref();
    model_response(
        &journal.0,
        query.get("model").map(String::as_str),
        |model| solstone_core_thinking::local::bootstrap_status(&journal.0, model),
    )
}
async fn models() -> Response {
    json_response(solstone_core_thinking::local::models())
}
async fn runtime(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    json_response(solstone_core_thinking::local::runtime(&journal.0))
}
async fn generators(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let journal = journal.as_ref();
    match config(&journal.0) {
        Ok(config) => match solstone_core_thinking::generators::generators(&config) {
            Ok(value) => json_response(value),
            Err(detail) => server_error(detail),
        },
        Err(response) => *response,
    }
}
async fn validate_keys(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let journal = journal.as_ref();
    match config(&journal.0) {
        Ok(config) => json_response(solstone_core_thinking::providers::validate_keys(&config)),
        Err(response) => *response,
    }
}

fn model_response(
    _journal: &Path,
    requested: Option<&str>,
    render: impl FnOnce(&str) -> Value,
) -> Response {
    match solstone_core_thinking::local::accepted_model(requested) {
        Some(model) => json_response(render(model)),
        None => json_error(solstone_core_thinking::local::invalid_model(
            requested.unwrap_or(solstone_core_thinking::local::default_model()),
        )),
    }
}
fn config(journal: &Path) -> Result<serde_json::Map<String, Value>, Box<Response>> {
    solstone_core_thinking::read_config(journal)
        .map_err(|error| Box::new(server_error(error.to_string())))
}
fn json_error(value: Value) -> Response {
    json_response_with_status(StatusCode::BAD_REQUEST, value)
}
fn json_response(value: Value) -> Response {
    json_response_with_status(StatusCode::OK, value)
}
fn json_response_with_status(status: StatusCode, value: Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!("{}\n", flask_json(&value))))
        .expect("JSON response builds")
}
fn flask_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => flask_string(value),
        Value::Array(values) => format!(
            "[{}]",
            values.iter().map(flask_json).collect::<Vec<_>>().join(",")
        ),
        Value::Object(values) => {
            let mut fields: Vec<_> = values.iter().collect();
            fields.sort_unstable_by(|left, right| left.0.cmp(right.0));
            format!(
                "{{{}}}",
                fields
                    .into_iter()
                    .map(|(key, value)| format!("{}:{}", flask_string(key), flask_json(value)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}
fn flask_string(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' || !character.is_ascii() => {
                let code = character as u32;
                if code <= 0xffff {
                    output.push_str(&format!("\\u{code:04x}"));
                } else {
                    let code = code - 0x1_0000;
                    output.push_str(&format!(
                        "\\u{:04x}\\u{:04x}",
                        0xd800 + (code >> 10),
                        0xdc00 + (code & 0x3ff)
                    ));
                }
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}
fn server_error(detail: String) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        detail,
    )
        .into_response()
}

#[allow(dead_code)] // Wired by Thinking write routes in W2 chunks 2–3.
fn thinking_config_busy_response() -> Response {
    error_envelope(
        "config_busy",
        "I couldn't save those settings right now because they were busy. Try again in a moment.",
        "settings are busy; try again",
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use chrono::{Duration, Utc};
    use serde_json::{Value, json};
    use solstone_core_brain::{begin_refresh, finish_refresh};
    use tower::ServiceExt;

    #[test]
    /// This exists only while the embedded and Python assets coexist; the cut
    /// wave must remove this test with the Python copies.
    fn thinking_asset_copies_match_the_source_until_the_cut_wave_removes_both() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = manifest.ancestors().nth(3).expect("repository root");
        for (copied, source) in [
            (
                "assets/thinking/workspace.html",
                "solstone/apps/thinking/workspace.html",
            ),
            (
                "assets/thinking/thinking.js",
                "solstone/apps/thinking/static/thinking.js",
            ),
        ] {
            assert_eq!(
                fs::read(manifest.join(copied)).expect("embedded copy reads"),
                fs::read(root.join(source))
                    .expect("source must exist until the cut wave deletes this test too")
            );
        }
    }

    /// W2 removes this characterization pin when Thinking writes are converted.
    #[tokio::test]
    async fn providers_post_is_the_get_only_fallback_405_with_an_empty_body() {
        let root =
            std::env::temp_dir().join(format!("solstone-thinking-post-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("config")).expect("config directory creates");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1767225600}}"#,
        )
        .expect("config writes");
        let response = crate::router(root.clone())
            .oneshot(
                Request::post("/app/thinking/api/providers")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body reads")
                .is_empty()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn copy_payload_round_trips_from_api_state() {
        let root = temporary_journal("copy");
        let response = crate::router(root.clone())
            .oneshot(
                Request::get("/app/thinking/api/state")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body reads"),
        )
        .expect("state is JSON");
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../fixtures/convey_thinking_corpus.json"
        ))
        .expect("corpus parses");
        let expected = corpus["phases"]["none"]
            .as_array()
            .expect("none cases")
            .iter()
            .find(|case| case["path"] == "/app/thinking/api/state")
            .expect("state case");
        assert_eq!(body["copy"], expected["json"]["copy"]);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn invalid_brain_record_degrades_the_brain_read_projections() {
        let root = temporary_journal("invalid-brain");
        fs::write(root.join("config/journal.json"), br#"{"env":{"OPENAI_API_KEY":"key"},"providers":{"active":{"model":"gpt-5","provider":"openai"}},"setup":{"completed_at":1767225600}}"#).expect("config writes");
        let now = Utc::now();
        let evidence = json!({"status":"ok","observed_at":now.to_rfc3339(),"expires_at":(now + Duration::days(1)).to_rfc3339()});
        let permit = begin_refresh(&root, now, None, None, false, None)
            .expect("refresh starts")
            .expect("permit");
        finish_refresh(&root, permit, json!({"configuration":evidence,"lane_prerequisites":evidence,"generate":evidence,"cogitate":evidence}), now, None).expect("refresh finishes");
        let brain = root.join("health/brain.json");
        let mut invalid: Value =
            serde_json::from_slice(&fs::read(&brain).expect("record reads")).expect("record JSON");
        invalid["fingerprint_sha256"] = Value::String("x".repeat(64));
        fs::write(
            &brain,
            serde_json::to_vec(&invalid).expect("record serializes"),
        )
        .expect("record writes");
        for path in ["/app/thinking/api/state", "/app/thinking/api/providers"] {
            let response = crate::router(root.clone())
                .oneshot(
                    Request::get(path)
                        .body(Body::empty())
                        .expect("request builds"),
                )
                .await
                .expect("router responds");
            let body: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body reads"),
            )
            .expect("projection is JSON");
            let brain = if path.ends_with("state") {
                &body["providers"]["brain"]
            } else {
                &body["brain"]
            };
            assert_eq!(brain["reason_code"], "brain_record_invalid");
        }
        let response = crate::router(root.clone())
            .oneshot(
                Request::get("/app/thinking/api/providers/local/status")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body reads"),
        )
        .expect("local status is JSON");
        assert_eq!(body["generate_ready"], false);
        assert_eq!(body["cogitate_ready"], false);
        let _ = fs::remove_dir_all(root);
    }

    fn temporary_journal(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("solstone-thinking-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("config")).expect("config directory creates");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1767225600}}"#,
        )
        .expect("config writes");
        root
    }
}
