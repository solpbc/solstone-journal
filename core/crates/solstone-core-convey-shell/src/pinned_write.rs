// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! `PUT /api/shell/pins`. The mutation closure reads, toggles, and normalizes.

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};
use solstone_core_journal_io::LockOptions;

use crate::JournalRoot;
use crate::pinned::{is_pinnable, known_view, toggle};

const INVALID: &str = "that setting couldn't be saved because one value was invalid.";
/// A pin request is one view name and a boolean.
const MAX_BODY_BYTES: usize = 4096;

pub(crate) async fn put_pins(
    Extension(journal): Extension<Arc<JournalRoot>>,
    body: Body,
) -> Response {
    let bytes = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return refuse("the pins request could not be read");
        }
    };
    let request = match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(object)) => object,
        _ => return refuse("pins request must be an object"),
    };
    let Some(view) = request.get("view").and_then(Value::as_str) else {
        return refuse("view must be a string");
    };
    let view = view.to_owned();
    let Some(pinned) = request.get("pinned").and_then(Value::as_bool) else {
        return refuse("pinned must be a boolean");
    };
    if !known_view(&view) {
        return refuse("unknown view");
    }
    if !is_pinnable(&view) {
        return refuse("that view can't be pinned");
    }

    let saved = mutate_journal_config(&journal.0, LockOptions::default(), |config| {
        match toggle(config, &view, pinned) {
            Ok((list, changed)) => JournalConfigMutation {
                changed,
                value: Ok(list),
            },
            Err(detail) => JournalConfigMutation {
                changed: false,
                value: Err(detail),
            },
        }
    });
    match saved {
        Ok(transaction) => match transaction.value {
            Ok(pinned) => (
                StatusCode::OK,
                Json(serde_json::json!({ "pinned": pinned })),
            )
                .into_response(),
            Err(detail) => refuse(detail),
        },
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "your settings couldn't be saved.",
        )
            .into_response(),
    }
}

fn refuse(detail: impl Into<String>) -> Response {
    error_envelope(
        "invalid_config_value",
        INVALID,
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};
    use solstone_core_convey_http::identity::AccessBasis;
    use tower::ServiceExt;

    use super::super::router;
    use super::StateField;

    struct Journal(tempfile::TempDir);

    impl Journal {
        fn new(body: &[u8]) -> Self {
            let dir = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
            fs::create_dir(dir.path().join("config")).expect("config directory");
            fs::write(dir.path().join("config/journal.json"), body).expect("journal config");
            Self(dir)
        }

        fn path(&self) -> &Path {
            self.0.path()
        }

        fn bytes(&self) -> Vec<u8> {
            fs::read(self.path().join("config/journal.json")).expect("config bytes")
        }

        fn config(&self) -> Value {
            serde_json::from_slice(&self.bytes()).expect("config json")
        }
    }

    fn established() -> Journal {
        Journal::new(br#"{"setup":{"completed_at":1767225600}}"#)
    }

    async fn call(
        app: axum::Router,
        method: &str,
        path: &str,
        content_type: Option<&str>,
        body: &[u8],
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(content_type) = content_type {
            builder = builder.header(header::CONTENT_TYPE, content_type);
        }
        let mut request = builder.body(Body::from(body.to_vec())).expect("request");
        request.extensions_mut().insert(AccessBasis::Localhost);
        let response = app.oneshot(request).await.expect("response");
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body")
            .to_vec();
        (status, headers, body)
    }

    fn json_body(body: &[u8]) -> Value {
        serde_json::from_slice(body).expect("json")
    }

    async fn put(app: axum::Router, body: &Value) -> (StatusCode, Value, Vec<u8>) {
        let bytes = serde_json::to_vec(body).unwrap();
        let (status, _headers, response) = call(
            app,
            "PUT",
            "/api/shell/pins",
            Some("application/json"),
            &bytes,
        )
        .await;
        let parsed = if response.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&response).unwrap_or(Value::Null)
        };
        (status, parsed, response)
    }

    fn pinned_of(config: &Value) -> Value {
        config["convey"]["pinned_views"].clone()
    }

    #[tokio::test]
    async fn shell_api_and_pins_mutation_share_one_router_without_rebuilding() {
        let journal = established();
        let app = router(journal.path().to_path_buf());
        let (status, _, body) = call(app.clone(), "GET", "/api/shell", None, b"").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json_body(&body)["pinned"],
            json!(["search", "entities", "thinking"])
        );

        let (status, response, _) = put(
            app.clone(),
            &json!({"view": "map", "pinned": true, "ignored": 1}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["pinned"],
            json!(["search", "entities", "thinking", "map"])
        );

        let (status, _, body) = call(app.clone(), "GET", "/api/shell", None, b"").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json_body(&body)["pinned"],
            json!(["search", "entities", "thinking", "map"])
        );

        let (status, response, _) =
            put(app.clone(), &json!({"view": "map", "pinned": false})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["pinned"],
            json!(["search", "entities", "thinking"])
        );
        let (status, _, body) = call(app, "GET", "/api/shell", None, b"").await;
        assert_eq!(
            json_body(&body)["pinned"],
            json!(["search", "entities", "thinking"])
        );
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn pinning_view_with_corrupt_stored_shape_materializes_defaults_plus_new_view() {
        let journal = Journal::new(
            br#"{"setup":{"completed_at":1767225600},"convey":{"pinned_views":"oops","secret":"s"}}"#,
        );
        let app = router(journal.path().to_path_buf());
        let (status, _, body) = call(app.clone(), "GET", "/api/shell", None, b"").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json_body(&body)["pinned"],
            json!(["search", "entities", "thinking"])
        );
        let before = journal.bytes();
        let (status, response, _) = put(app.clone(), &json!({"view": "map", "pinned": true})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["pinned"],
            json!(["search", "entities", "thinking", "map"])
        );
        assert_ne!(journal.bytes(), before);
        assert_eq!(
            pinned_of(&journal.config()),
            json!(["search", "entities", "thinking", "map"])
        );
        assert_eq!(journal.config()["convey"]["secret"], "s");
        let (status, _, body) = call(app, "GET", "/api/shell", None, b"").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["pinned"], json_body(&body)["pinned"]);
    }

    #[tokio::test]
    async fn pinning_view_with_explicit_empty_list_stores_only_new_view() {
        let journal =
            Journal::new(br#"{"setup":{"completed_at":1767225600},"convey":{"pinned_views":[]}}"#);
        let app = router(journal.path().to_path_buf());
        let (status, _, body) = call(app.clone(), "GET", "/api/shell", None, b"").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json_body(&body)["pinned"], json!([]));
        let (status, response, _) = put(app, &json!({"view": "map", "pinned": true})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["pinned"], json!(["map"]));
        assert_eq!(pinned_of(&journal.config()), json!(["map"]));
    }

    #[tokio::test]
    async fn unpinning_from_dirty_list_normalizes_to_clean_empty_and_noop_pin_preserves_bytes() {
        let journal = Journal::new(
            br#"{"setup":{"completed_at":1767225600},"convey":{"pinned_views":["entities","entities","news"]}}"#,
        );
        let app = router(journal.path().to_path_buf());
        let before = journal.bytes();
        let (status, response, _) =
            put(app.clone(), &json!({"view": "entities", "pinned": true})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["pinned"], json!(["entities"]));
        assert_eq!(journal.bytes(), before);

        let (status, response, _) =
            put(app.clone(), &json!({"view": "entities", "pinned": false})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["pinned"], json!([]));
        assert_eq!(pinned_of(&journal.config()), json!([]));
        let (status, _, body) = call(app, "GET", "/api/shell", None, b"").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json_body(&body)["pinned"], json!([]));
    }

    #[tokio::test]
    async fn all_thirteen_views_can_be_pinned_without_cap_in_pin_order() {
        let journal =
            Journal::new(br#"{"setup":{"completed_at":1767225600},"convey":{"pinned_views":[]}}"#);
        let app = router(journal.path().to_path_buf());
        let names = [
            "activities",
            "backup",
            "body",
            "curation",
            "entities",
            "health",
            "map",
            "search",
            "speakers",
            "stats",
            "support",
            "thinking",
            "transcripts",
        ];
        let mut expected = Vec::new();
        for name in names {
            expected.push(name);
            let (status, response, _) =
                put(app.clone(), &json!({"view": name, "pinned": true})).await;
            assert_eq!(status, StatusCode::OK, "{name}");
            assert_eq!(response["pinned"], json!(expected), "{name}");
        }
        assert_eq!(pinned_of(&journal.config()), json!(expected));
    }

    #[tokio::test]
    async fn pin_mutation_refusals_leave_file_bytes_identical_and_preserve_sibling_convey_keys() {
        let original = br#"{"setup":{"completed_at":1767225600},"convey":{"secret":"s","password_hash":"p","bind":"127.0.0.1:5015","pinned_views":["search"]}}"#;
        let journal = Journal::new(original);
        let app = router(journal.path().to_path_buf());
        let before = journal.bytes();

        for body in [
            json!({"view": "news", "pinned": true}),
            json!({"view": "home", "pinned": true}),
            json!({"view": "more", "pinned": true}),
            json!({"view": "settings", "pinned": false}),
            json!({"view": "map", "pinned": "yes"}),
            json!({"view": "map"}),
            json!({"pinned": true}),
            json!(["map", true]),
        ] {
            let (status, response, _) = put(app.clone(), &body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(response["reason_code"], "invalid_config_value");
            assert_eq!(journal.bytes(), before, "{body}");
        }

        let (status, _, _) = call(
            app.clone(),
            "PUT",
            "/api/shell/pins",
            Some("text/plain"),
            br#"{"view":"map","pinned":true}"#,
        )
        .await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(journal.bytes(), before);

        let blocked =
            Journal::new(br#"{"setup":{"completed_at":1767225600},"convey":["secret-holder"]}"#);
        let blocked_before = blocked.bytes();
        let blocked_app = router(blocked.path().to_path_buf());
        let (status, _, get_body) = call(blocked_app.clone(), "GET", "/api/shell", None, b"").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json_body(&get_body)["pinned"],
            json!(["search", "entities", "thinking"])
        );
        assert_eq!(blocked.bytes(), blocked_before);
        let (status, response, _) = put(blocked_app, &json!({"view": "map", "pinned": true})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(response["reason_code"], "invalid_config_value");
        assert_eq!(blocked.bytes(), blocked_before);

        let (status, response, _) = put(app, &json!({"view": "map", "pinned": true})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["pinned"], json!(["search", "map"]));
        let saved = journal.config();
        assert_eq!(saved["convey"]["secret"], "s");
        assert_eq!(saved["convey"]["password_hash"], "p");
        assert_eq!(saved["convey"]["bind"], "127.0.0.1:5015");
        assert_eq!(saved["convey"]["pinned_views"], json!(["search", "map"]));
        assert_eq!(saved["setup"]["completed_at"], 1767225600_u64);

        let noop = journal.bytes();
        let app = router(journal.path().to_path_buf());
        let (status, response, _) = put(app, &json!({"view": "map", "pinned": true})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["pinned"], json!(["search", "map"]));
        assert_eq!(journal.bytes(), noop);
    }

    #[tokio::test]
    async fn more_and_pins_routes_follow_session_gate_and_bare_redirect() {
        let unestablished = Journal::new(br#"{}"#);
        let app = router(unestablished.path().to_path_buf());
        for (method, path) in [
            ("GET", "/app/more/"),
            ("GET", "/app/more/workspace"),
            ("PUT", "/api/shell/pins"),
        ] {
            let (status, headers, _) = call(
                app.clone(),
                method,
                path,
                Some("application/json"),
                br#"{"view":"map","pinned":true}"#,
            )
            .await;
            assert_eq!(status, StatusCode::FOUND, "{path}");
            assert_eq!(headers.get(header::LOCATION).unwrap(), "/init");
        }

        let journal = established();
        let app = router(journal.path().to_path_buf());
        let (status, headers, _) = call(app.clone(), "GET", "/app/more", None, b"").await;
        assert_eq!(status, StatusCode::PERMANENT_REDIRECT);
        assert_eq!(headers.get(header::LOCATION).unwrap(), "/app/more/");

        let (status, headers, body) = call(app.clone(), "GET", "/app/more/", None, b"").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers.get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        assert_eq!(body, include_bytes!("../assets/static/shell.html"));

        let (status, _, body) = call(app.clone(), "GET", "/app/more/workspace", None, b"").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, include_bytes!("../assets/more/workspace.html"));

        let (status, _, _) = call(app.clone(), "GET", "/app/more/static/more.js", None, b"").await;
        assert_eq!(status, StatusCode::OK);

        let (status, response, _) = put(app, &json!({"view": "map", "pinned": true})).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            response["pinned"]
                .as_array()
                .unwrap()
                .iter()
                .any(|name| name == "map")
        );

        for path in ["/app/more/", "/app/more/workspace", "/api/shell/pins"] {
            let corrupt = Journal::new(br#"{"setup":{"completed_at":1767225600},"convey":}"#);
            let app = router(corrupt.path().to_path_buf());
            let method = if path.contains("pins") { "PUT" } else { "GET" };
            let (status, headers, body) = call(
                app,
                method,
                path,
                Some("application/json"),
                br#"{"view":"map","pinned":true}"#,
            )
            .await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{path}");
            if path.split('/').any(|segment| segment == "api") {
                assert_eq!(json_body(&body)["reason_code"], "corrupt_config");
            } else {
                assert_eq!(
                    headers.get(header::CONTENT_TYPE).unwrap(),
                    "text/plain; charset=utf-8"
                );
            }
        }
    }

    #[tokio::test]
    async fn read_shapes_stay_byte_identical_on_one_router() {
        let cases: &[(&str, Value)] = &[
            (
                r#"{"setup":{"completed_at":1767225600}}"#,
                json!(["search", "entities", "thinking"]),
            ),
            (
                r#"{"setup":{"completed_at":1767225600},"convey":{"pinned_views":[]}}"#,
                json!([]),
            ),
            (
                r#"{"setup":{"completed_at":1767225600},"convey":{"pinned_views":["news"]}}"#,
                json!([]),
            ),
            (
                r#"{"setup":{"completed_at":1767225600},"convey":{"pinned_views":["news","entities","home","entities","search"]}}"#,
                json!(["entities", "search"]),
            ),
            (
                r#"{"setup":{"completed_at":1767225600},"convey":{"pinned_views":"oops"}}"#,
                json!(["search", "entities", "thinking"]),
            ),
            (
                r#"{"setup":{"completed_at":1767225600},"convey":{"pinned_views":[1]}}"#,
                json!(["search", "entities", "thinking"]),
            ),
            (
                r#"{"setup":{"completed_at":1767225600},"convey":"nope"}"#,
                json!(["search", "entities", "thinking"]),
            ),
        ];
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        let config_path = root.path().join("config/journal.json");
        fs::write(&config_path, cases[0].0).unwrap();
        let app = router(PathBuf::from(root.path()));
        for (raw, expected) in cases {
            fs::write(&config_path, raw).unwrap();
            let before = fs::read(&config_path).unwrap();
            let (status, _, body) = call(app.clone(), "GET", "/api/shell", None, b"").await;
            assert_eq!(status, StatusCode::OK, "{raw}");
            assert_eq!(json_body(&body)["pinned"], *expected, "{raw}");
            assert_eq!(fs::read(&config_path).unwrap(), before, "{raw}");
        }
    }

    #[tokio::test]
    async fn more_state_endpoints_json_matches_assignment_and_leaves_exist_in_router() {
        let source = include_str!("../assets/more/more.js");
        let marker = "const MORE_STATE_FIELDS = ";
        let start = source.find(marker).expect("state field table");
        let rest = &source[start + marker.len()..];
        let end = rest.find(";\n").expect("state field table terminator");
        let fields: Vec<StateField> = serde_json::from_str(&rest[..end]).expect("state field json");
        let names: std::collections::BTreeSet<_> =
            fields.iter().map(|field| field.view.as_str()).collect();
        assert_eq!(
            names,
            std::collections::BTreeSet::from([
                "activities",
                "transcripts",
                "map",
                "body",
                "speakers",
                "entities",
                "import",
                "network",
                "thinking",
                "curation",
                "agents",
                "backup",
            ])
        );
        let journal = established();
        let app = router(journal.path().to_path_buf());
        for field in fields {
            if field.view == "agents" {
                continue;
            }
            let (status, _, body) = call(app.clone(), "GET", &field.url, None, b"").await;
            assert_eq!(status, StatusCode::OK, "{}", field.url);
            let parsed = json_body(&body);
            let mut current = &parsed;
            for key in &field.path {
                current = current
                    .get(key)
                    .unwrap_or_else(|| panic!("{} missing {key}", field.url));
            }
            if field.view == "thinking" {
                assert_eq!(current.as_str(), Some("none"));
            }
            if field.view == "curation" {
                assert!(current.is_number(), "curation to_review must be a number");
            }
        }

        #[cfg(unix)]
        {
            let agents_app = router(journal.path().to_path_buf()).merge(
                solstone_core_mcp_endpoint::owner_routes(journal.path().to_path_buf()),
            );
            let (status, _, body) =
                call(agents_app, "GET", "/app/agents/api/state", None, b"").await;
            assert_eq!(status, StatusCode::OK);
            let parsed = json_body(&body);
            assert!(parsed["connections"].is_array());
        }

        let (status, _, workspace_body) =
            call(app.clone(), "GET", "/app/more/workspace", None, b"").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            workspace_body,
            include_bytes!("../assets/more/workspace.html")
        );
        let html = std::str::from_utf8(&workspace_body).expect("utf-8 workspace html");
        let mut script_sources = Vec::new();
        for line in html.lines() {
            if let Some(start) = line.find("<script src=\"") {
                let rest = &line[start + "<script src=\"".len()..];
                if let Some(end) = rest.find('"') {
                    script_sources.push(&rest[..end]);
                }
            }
        }
        let mut backup_status_idx = None;
        let mut more_idx = None;
        for (idx, src) in script_sources.iter().enumerate() {
            let (status, _, _) = call(app.clone(), "GET", src, None, b"").await;
            assert_eq!(status, StatusCode::OK, "GET {src}");
            if src.ends_with("backup-status.js") {
                backup_status_idx = Some(idx);
            } else if src.ends_with("more.js") {
                more_idx = Some(idx);
            }
        }
        assert!(
            backup_status_idx.is_some(),
            "backup-status.js script tag required"
        );
        assert!(more_idx.is_some(), "more.js script tag required");
        assert!(
            backup_status_idx.unwrap() < more_idx.unwrap(),
            "backup-status.js must precede more.js"
        );
    }
}

#[cfg(test)]
#[derive(serde::Deserialize)]
struct StateField {
    view: String,
    url: String,
    path: Vec<String>,
}
