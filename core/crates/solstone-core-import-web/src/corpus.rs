// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Corpus replay and derivation tests for the native Import read surface.

#[derive(Clone, Copy)]
pub(crate) enum Segment {
    Key(&'static str),
    AnyArrayIndex,
}
pub(crate) type JsonPath = &'static [Segment];
pub(crate) const CTIME_PATHS: &[JsonPath] = &[
    &[Segment::Key("created_at")],
    &[Segment::Key("imported_at")],
    &[
        Segment::Key("imports"),
        Segment::AnyArrayIndex,
        Segment::Key("created_at"),
    ],
    &[
        Segment::Key("imports"),
        Segment::AnyArrayIndex,
        Segment::Key("imported_at"),
    ],
];
/// Corpus-declared over-fire: a source-status root `created_at` is milliseconds, not ctime.
pub(crate) const DECLARED_STATUS_ROOT_CREATED_AT_OVERFIRE: JsonPath = &[Segment::Key("created_at")];

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, os::unix::fs::PermissionsExt, path::Path};

    use axum::{
        body::{Body, to_bytes},
        http::{HeaderMap, Request, StatusCode},
    };
    use serde_json::{Map, Value, json};
    use sha2::{Digest, Sha256};
    use tower::ServiceExt;

    use super::{CTIME_PATHS, DECLARED_STATUS_ROOT_CREATED_AT_OVERFIRE, JsonPath, Segment};
    use crate::{
        imports::{ImportInfo, resolve_status_with_timeout},
        test_support::{CONTENT, FAILED, OK, PENDING, phase_root, populated_root, seed_import},
    };

    const CORPUS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/convey_import_corpus.json"
    ));
    const DOOR_PATHS: &[&str] = &[
        "/app/import/journal/corpusSo/manifest/entities",
        "/app/import/journal/00000000/manifest/entities",
        "/app/import/journal/corpusSo/ingest/entities",
    ];
    const BROWSER_WRITE_PATHS: &[&str] = &[
        "/app/import/api/save-path",
        "/app/import/api/meta",
        "/app/import/api/journal-sources/create",
        "/app/import/api/journal-sources/corpus_peer/resolve-config",
    ];
    const UNREGISTERED: &[(&str, &str)] = &[
        ("POST", "/app/import/api/save"),
        ("POST", "/app/import/api/save-path"),
        ("POST", "/app/import/api/meta"),
        ("POST", "/app/import/api/start"),
        ("POST", "/app/import/api/journal-sources/create"),
        ("POST", "/app/import/api/journal-sources/corpus_peer/revoke"),
        (
            "POST",
            "/app/import/api/journal-sources/corpus_peer/resolve-entity",
        ),
        (
            "POST",
            "/app/import/api/journal-sources/corpus_peer/resolve-facet",
        ),
        (
            "POST",
            "/app/import/api/journal-sources/corpus_peer/resolve-config",
        ),
        (
            "POST",
            "/app/import/api/journal-sources/corpus_peer/resolve-config-all",
        ),
        ("GET", "/app/import/journal/corpusSo/manifest/entities"),
        ("POST", "/app/import/journal/corpusSo/ingest/segments"),
        ("POST", "/app/import/journal/corpusSo/ingest/entities"),
        ("POST", "/app/import/journal/corpusSo/ingest/facets"),
        ("POST", "/app/import/journal/corpusSo/ingest/imports"),
        ("POST", "/app/import/journal/corpusSo/ingest/config"),
    ];

    async fn request(
        root: &Path,
        method: &str,
        uri: &str,
        request_json: Option<&Value>,
    ) -> (StatusCode, String, Option<String>, Vec<u8>) {
        let mut builder = Request::builder().method(method).uri(uri);
        if request_json.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let body = request_json
            .map(|value| Body::from(serde_json::to_vec(value).expect("request JSON")))
            .unwrap_or_else(Body::empty);
        let response = solstone_core_convey_shell::router(root.to_path_buf())
            .oneshot(builder.body(body).expect("request"))
            .await
            .expect("router response");
        let status = response.status();
        let headers: HeaderMap = response.headers().clone();
        let content_type = headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let location = headers
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body")
            .to_vec();
        (status, content_type, location, body)
    }

    async fn json_request(root: &Path, method: &str, uri: &str) -> (StatusCode, Value) {
        let (status, _, _, body) = request(root, method, uri, None).await;
        (
            status,
            serde_json::from_slice(&body).expect("JSON response"),
        )
    }

    fn normalize_path(value: &mut Value, path: JsonPath) {
        match (value, path) {
            (Value::Object(object), [Segment::Key(key)]) => {
                if let Some(value) = object.get_mut(*key) {
                    *value = Value::String("<DIR_CTIME>".to_owned());
                }
            }
            (Value::Object(object), [Segment::Key(key), rest @ ..]) => {
                if let Some(value) = object.get_mut(*key) {
                    normalize_path(value, rest);
                }
            }
            (Value::Array(items), [Segment::AnyArrayIndex, rest @ ..]) => {
                for item in items {
                    normalize_path(item, rest);
                }
            }
            _ => {}
        }
    }

    fn normalize(mut value: Value, paths: &[JsonPath]) -> Value {
        for path in paths {
            normalize_path(&mut value, path);
        }
        value
    }

    fn canonical_json(value: &Value) -> String {
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
                serde_json::to_string(value).expect("scalar JSON")
            }
            Value::Array(items) => format!(
                "[{}]",
                items
                    .iter()
                    .map(canonical_json)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Value::Object(items) => {
                let mut keys: Vec<_> = items.keys().collect();
                keys.sort_unstable();
                format!(
                    "{{{}}}",
                    keys.into_iter()
                        .map(|key| format!(
                            "{}:{}",
                            serde_json::to_string(key).unwrap(),
                            canonical_json(&items[key])
                        ))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
        }
    }

    fn declared_deviation(phase: &str, method: &str, path: &str) -> bool {
        (matches!(phase, "empty" | "populated") && method == "GET" && path == "/app/import/")
            || (DOOR_PATHS.contains(&path))
            || (matches!(phase, "empty" | "populated")
                && method == "POST"
                && BROWSER_WRITE_PATHS.contains(&path))
    }

    #[tokio::test]
    async fn ac4_corpus_replay_has_only_the_declared_28_deviations() {
        let corpus: Value = serde_json::from_str(CORPUS).expect("corpus JSON");
        let mut passed = 0;
        let mut declared = Vec::new();
        let mut unexpected = Vec::new();
        for phase in ["unestablished", "corrupt", "empty", "populated"] {
            let root = phase_root(phase);
            for case in corpus["phases"][phase].as_array().expect("phase cases") {
                let method = case["method"].as_str().expect("method");
                let path = case["path"].as_str().expect("path");
                let (status, content_type, location, mut actual_body) =
                    request(root.path(), method, path, case.get("request_json")).await;
                if case.get("body_normalized").is_some() {
                    actual_body = String::from_utf8_lossy(&actual_body)
                        .replace(&*root.path().to_string_lossy(), "<JOURNAL_ROOT>")
                        .into_bytes();
                }
                let body_matches = if let Some(expected) = case.get("json") {
                    let mut paths = CTIME_PATHS.to_vec();
                    if path.ends_with("/status") {
                        paths.push(DECLARED_STATUS_ROOT_CREATED_AT_OVERFIRE);
                    }
                    serde_json::from_slice::<Value>(&actual_body)
                        .map(|value| normalize(value, &paths))
                        .ok()
                        .as_ref()
                        == Some(expected)
                } else {
                    actual_body.len() == case["body_bytes"].as_u64().expect("body bytes") as usize
                        && format!("{:x}", Sha256::digest(&actual_body))
                            == case["body_sha256"].as_str().expect("body hash")
                };
                let matches = status.as_u16() == case["status"].as_u64().expect("status") as u16
                    && content_type == case["content_type"].as_str().expect("content type")
                    && location.as_deref() == case.get("location").and_then(Value::as_str)
                    && body_matches;
                if matches {
                    passed += 1;
                } else if declared_deviation(phase, method, path) {
                    declared.push(format!("{phase} {method} {path}"));
                } else {
                    unexpected.push(format!("{phase} {method} {path}"));
                }
            }
        }
        assert_eq!(passed, 108, "unexpected replay cases: {unexpected:?}");
        assert_eq!(declared.len(), 28, "declared roster changed: {declared:?}");
        assert!(
            unexpected.is_empty(),
            "unexpected replay cases: {unexpected:?}"
        );
    }

    #[tokio::test]
    async fn ac6_populated_sources_match_the_recorded_catalogue() {
        let root = populated_root();
        let (status, body) = json_request(root.path(), "GET", "/app/import/api/sources").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total"], 11);
        let expected = [
            "ics",
            "chatgpt",
            "claude",
            "gemini",
            "obsidian",
            "kindle",
            "journal_archive",
            "recording",
            "document",
            "image",
            "quick",
        ];
        let rows = body["items"].as_array().expect("items array");
        assert_eq!(
            rows.iter()
                .map(|row| row["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        let expected_keys: BTreeSet<_> = [
            "name",
            "display_name",
            "icon",
            "description",
            "input_type",
            "upload_prompt",
            "has_guide",
            "accept",
            "icon_svg",
        ]
        .into_iter()
        .collect();
        assert!(rows.iter().all(|row| {
            row.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                == expected_keys
        }));
        let expected: Value = serde_json::from_str(CORPUS).unwrap();
        let case = expected["phases"]["populated"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["path"] == "/app/import/api/sources")
            .unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(canonical_json(&body))),
            case["body_sha256"]
        );
    }

    #[tokio::test]
    async fn ac7_populated_list_is_a_heterogeneous_array() {
        let root = populated_root();
        let (_, body) = json_request(root.path(), "GET", "/app/import/api/list").await;
        let rows = body["imports"]
            .as_array()
            .expect("imports sequence, not map");
        assert_eq!(rows.len(), 4);
        assert_eq!(
            rows.iter()
                .filter(|row| row.as_object().unwrap().len() == 24)
                .count(),
            3
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.as_object().unwrap().len() == 17)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn ac8_populated_list_orders_by_upload_timestamp_not_ctime() {
        let root = phase_root("empty");
        seed_import(
            root.path(),
            "20260101_000000",
            "old.txt",
            "text/plain",
            "old",
            Some(json!({"processed": true})),
        );
        seed_import(
            root.path(),
            "20260102_000000",
            "new.txt",
            "text/plain",
            "new",
            Some(json!({"processed": true})),
        );
        for (timestamp, upload) in [
            ("20260101_000000", 2_000_000.0),
            ("20260102_000000", 1_000_000.0),
        ] {
            let path = root
                .path()
                .join("imports")
                .join(timestamp)
                .join("import.json");
            let mut metadata: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            metadata["upload_timestamp"] = json!(upload);
            fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        }
        let (_, body) = json_request(root.path(), "GET", "/app/import/api/list").await;
        assert_eq!(body["imports"][0]["timestamp"], "20260101_000000");
    }

    #[tokio::test]
    async fn ac9_backfill_both_routes_persist_private_seven_key_manifests() {
        let root = phase_root("empty");
        let jsonl = "20260103_000000";
        let markdown = "20260104_000000";
        seed_import(
            root.path(),
            jsonl,
            "source.jsonl",
            "application/json",
            "jsonl",
            Some(
                json!({"source_type":"chatgpt", "all_created_files":["chronicle/20260103/import.chatgpt/key-a/conversation_transcript.jsonl"]}),
            ),
        );
        seed_import(
            root.path(),
            markdown,
            "source.md",
            "text/markdown",
            "markdown",
            Some(
                json!({"source_type":"obsidian", "all_created_files":["chronicle/20260104/import.obsidian/key-b/note.md"]}),
            ),
        );
        let transcript = root.path().join("chronicle/20260103/import.chatgpt/key-a");
        fs::create_dir_all(&transcript).unwrap();
        fs::write(
            transcript.join("conversation_transcript.jsonl"),
            "{\"topics\":\"topic\"}\n{\"speaker\":\"Human\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        let note = root.path().join("chronicle/20260104/import.obsidian/key-b");
        fs::create_dir_all(&note).unwrap();
        fs::write(
            note.join("note.md"),
            "# ignored\n## One\nfirst\n## Two\nsecond\n",
        )
        .unwrap();
        for (timestamp, expected_prefix) in [(jsonl, "seg-"), (markdown, "item-")] {
            let uri = format!("/app/import/api/{timestamp}/content");
            let (status, _, _, first) = request(root.path(), "GET", &uri, None).await;
            assert_eq!(status, StatusCode::OK, "{timestamp} first route");
            let manifest = root
                .path()
                .join("imports")
                .join(timestamp)
                .join("content_manifest.jsonl");
            assert_eq!(
                fs::metadata(&manifest).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let rows: Vec<Value> = fs::read_to_string(&manifest)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let keys: BTreeSet<_> = ["id", "title", "date", "type", "preview", "meta", "segments"]
                .into_iter()
                .collect();
            assert!(rows.iter().all(|row| {
                row.as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>()
                    == keys
                    && row["id"].as_str().unwrap().starts_with(expected_prefix)
            }));
            let (_, _, _, second) = request(root.path(), "GET", &uri, None).await;
            assert_eq!(first, second, "{timestamp} uses persisted manifest");
        }
    }

    #[tokio::test]
    async fn ac10_populated_content_list_matches_filter_and_not_found_contracts() {
        let root = populated_root();
        let (status, body) = json_request(
            root.path(),
            "GET",
            &format!("/app/import/api/{CONTENT}/content?month=202608&per_page=1&page=2"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (
                body["total"].clone(),
                body["pages"].clone(),
                body["items"].as_array().unwrap().len()
            ),
            (json!(2), json!(2), 1)
        );
        assert_eq!(body["months"], json!({"202608":2,"202609":1}));
        let (status, body) =
            json_request(root.path(), "GET", &format!("/app/import/api/{OK}/content")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["reason_code"], "import_not_found");
    }

    #[tokio::test]
    async fn ac11_populated_detail_bodies_preserve_status_and_errors() {
        let root = populated_root();
        for (timestamp, status, error) in [
            (OK, "success", Value::Null),
            (
                FAILED,
                "failed",
                json!("calendar payload could not be parsed"),
            ),
            (PENDING, "pending", Value::Null),
        ] {
            let (code, body) =
                json_request(root.path(), "GET", &format!("/app/import/api/{timestamp}")).await;
            assert_eq!(code, StatusCode::OK, "{timestamp}");
            assert_eq!(body["status"], status, "{timestamp}");
            assert_eq!(body["error"], error, "{timestamp}");
        }
        let (code, body) =
            json_request(root.path(), "GET", "/app/import/api/20991231_235959").await;
        assert_eq!(
            (code, body["reason_code"].clone()),
            (StatusCode::NOT_FOUND, json!("import_not_found"))
        );
    }

    #[tokio::test]
    async fn ac12_populated_journal_source_reads_match_key_sets() {
        let root = populated_root();
        let (_, list) =
            json_request(root.path(), "GET", "/app/import/api/journal-sources/list").await;
        assert_eq!(list.as_object().unwrap().len(), 2);
        assert_eq!(list["items"][0].as_object().unwrap().len(), 4);
        let (_, status) = json_request(
            root.path(),
            "GET",
            "/app/import/api/journal-sources/corpus_peer/status",
        )
        .await;
        assert_eq!(status.as_object().unwrap().len(), 7);
        let (_, staged) = json_request(
            root.path(),
            "GET",
            "/app/import/api/journal-sources/corpus_peer/staged",
        )
        .await;
        assert_eq!(staged, json!({"items":[],"total":0}));
        for path in [
            "/app/import/api/journal-sources/missing/status",
            "/app/import/api/journal-sources/missing/staged",
        ] {
            let (code, body) = json_request(root.path(), "GET", path).await;
            assert_eq!(
                (code, body["reason_code"].clone()),
                (StatusCode::NOT_FOUND, json!("journal_source_problem"))
            );
        }
    }

    #[tokio::test]
    async fn ac12a_journal_source_created_at_is_never_name_normalized() {
        let root = populated_root();
        let (_, status) = json_request(
            root.path(),
            "GET",
            "/app/import/api/journal-sources/corpus_peer/status",
        )
        .await;
        let (_, list) =
            json_request(root.path(), "GET", "/app/import/api/journal-sources/list").await;
        assert_eq!(status["created_at"], 1_767_225_600_000_i64);
        assert_eq!(list["items"][0]["created_at"], 1_767_225_600_000_i64);
    }

    #[tokio::test]
    async fn ac12b_staged_area_reads_cover_entities_facets_and_config() {
        let root = populated_root();
        let state = root.path().join("imports/corpusSo");
        fs::create_dir_all(state.join("entities/staged")).unwrap();
        fs::write(state.join("entities/staged/e-1.json"), json!({"reason":"candidate","source_entity":{"name":"Ada"},"match_candidates":[],"staged_at":1}).to_string()).unwrap();
        fs::create_dir_all(state.join("facets/staged/work/activities")).unwrap();
        fs::write(
            state.join("facets/staged/work/activities/a.staged.json"),
            json!({"custom":"payload"}).to_string(),
        )
        .unwrap();
        fs::write(
            state.join("config/diff.json"),
            json!({"changed":true}).to_string(),
        )
        .unwrap();
        let (_, body) = json_request(
            root.path(),
            "GET",
            "/app/import/api/journal-sources/corpus_peer/staged",
        )
        .await;
        assert_eq!(body["total"], 3);
        let items = body["items"].as_array().unwrap();
        let entity = items
            .iter()
            .find(|item| item["area"] == "entities")
            .unwrap();
        assert_eq!(entity.as_object().unwrap().len(), 6);
        let facet = items.iter().find(|item| item["area"] == "facets").unwrap();
        assert_eq!(facet["custom"], "payload");
        assert_eq!(facet["facet"], "work");
        let config = items.iter().find(|item| item["area"] == "config").unwrap();
        assert_eq!(config, &json!({"area":"config","diff":{"changed":true}}));
        let (code, body) = json_request(
            root.path(),
            "GET",
            "/app/import/api/journal-sources/corpus_peer/staged?area=nope",
        )
        .await;
        assert_eq!(
            (code, body["reason_code"].clone()),
            (StatusCode::BAD_REQUEST, json!("invalid_request_value"))
        );
    }

    #[tokio::test]
    async fn ac13_guides_reject_decoded_traversal_and_case_variants() {
        let root = populated_root();
        let (code, content_type, _, bytes) =
            request(root.path(), "GET", "/app/import/api/guide/ics", None).await;
        assert_eq!(
            (code, content_type, bytes),
            (
                StatusCode::OK,
                "text/markdown; charset=utf-8".to_owned(),
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../../solstone/apps/import/guides/ics.md"
                ))
                .to_vec()
            )
        );
        let (_, missing) = json_request(root.path(), "GET", "/app/import/api/guide/nope").await;
        assert_eq!(missing["reason_code"], "file_not_found");
        for path in ["..", "%2e%2e%2f", "ICS"] {
            let (code, body) =
                json_request(root.path(), "GET", &format!("/app/import/api/guide/{path}")).await;
            assert_eq!(
                (code, body),
                (
                    StatusCode::BAD_REQUEST,
                    json!({"detail":"Invalid source name","error":"I couldn't use one of those values.","reason_code":"invalid_request_value"})
                )
            );
        }
    }

    #[tokio::test]
    async fn ac14_list_is_behind_the_three_session_gate_outcomes() {
        for (phase, expected) in [
            ("unestablished", StatusCode::FOUND),
            ("empty", StatusCode::OK),
            ("corrupt", StatusCode::INTERNAL_SERVER_ERROR),
        ] {
            let root = phase_root(phase);
            let (status, _, location, _) =
                request(root.path(), "GET", "/app/import/api/list", None).await;
            assert_eq!(status, expected, "{phase}");
            if phase == "unestablished" {
                assert_eq!(location.as_deref(), Some("/init"));
            }
        }
    }

    #[tokio::test]
    async fn ac15_unregistered_paths_keep_their_phase_specific_fallbacks() {
        assert_eq!(UNREGISTERED.len(), 16);
        for phase in ["empty", "unestablished", "corrupt"] {
            let root = phase_root(phase);
            for (method, path) in UNREGISTERED {
                let (status, content_type, location, body) =
                    request(root.path(), method, path, None).await;
                match phase {
                    "empty" if *method == "POST" => assert_eq!(
                        (status, body.len()),
                        (StatusCode::METHOD_NOT_ALLOWED, 0),
                        "{phase} {method} {path}"
                    ),
                    "empty" => assert_eq!(
                        status,
                        StatusCode::NOT_IMPLEMENTED,
                        "{phase} {method} {path}"
                    ),
                    "unestablished" => assert_eq!(
                        (status, location.as_deref()),
                        (StatusCode::FOUND, Some("/init")),
                        "{phase} {method} {path}"
                    ),
                    "corrupt" if path.contains("/api/") => assert_eq!(
                        (status, content_type),
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "application/json".to_owned()
                        ),
                        "{phase} {method} {path}"
                    ),
                    "corrupt" => assert_eq!(
                        (status, content_type),
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "text/plain; charset=utf-8".to_owned()
                        ),
                        "{phase} {method} {path}"
                    ),
                    _ => unreachable!(),
                }
            }
        }
    }

    #[test]
    fn ac17_status_timeout_and_processing_completed_are_derivations() {
        let mut values = Map::new();
        values.insert("error".into(), Value::Null);
        values.insert("error_stage".into(), Value::Null);
        values.insert("processed".into(), Value::Bool(false));
        values.insert("task_id".into(), json!("task"));
        let info = ImportInfo {
            imported_at: 100.0,
            values: values.clone(),
        };
        assert_eq!(resolve_status_with_timeout(&info, 109.0, 10.0).0, "running");
        assert_eq!(
            resolve_status_with_timeout(&info, 111.0, 10.0),
            ("failed", json!("Import never completed"), json!("timeout"))
        );
        values.insert("processing_completed".into(), json!(true));
        assert_eq!(
            resolve_status_with_timeout(
                &ImportInfo {
                    imported_at: 0.0,
                    values
                },
                1_000.0,
                10.0
            )
            .0,
            "success"
        );
    }

    #[test]
    fn ac17a_upload_timestamp_is_the_shared_sort_and_timeout_time() {
        let mut values = Map::new();
        values.insert("error".into(), Value::Null);
        values.insert("error_stage".into(), Value::Null);
        values.insert("processed".into(), Value::Bool(false));
        values.insert("task_id".into(), json!("task"));
        let old_upload = ImportInfo {
            imported_at: 100.0,
            values: values.clone(),
        };
        let recent_upload = ImportInfo {
            imported_at: 999.0,
            values,
        };
        assert_eq!(
            resolve_status_with_timeout(&old_upload, 200.0, 10.0).0,
            "failed"
        );
        assert_eq!(
            resolve_status_with_timeout(&recent_upload, 1_000.0, 10.0).0,
            "running"
        );
    }

    #[tokio::test]
    async fn ac18_corrupt_metadata_has_reference_key_arithmetic() {
        let root = phase_root("empty");
        for (timestamp, imported) in [
            ("20260105_000000", Some(json!({"processed":true}))),
            ("20260106_000000", None),
            (
                "20260107_000000",
                Some(
                    json!({"processed":true,"all_created_files":["120000_a.jsonl","130000_b.jsonl"]}),
                ),
            ),
        ] {
            let directory = root.path().join("imports").join(timestamp);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("import.json"), "{").unwrap();
            if let Some(imported) = imported {
                fs::write(directory.join("imported.json"), imported.to_string()).unwrap();
            }
        }
        let (_, body) = json_request(root.path(), "GET", "/app/import/api/list").await;
        let rows = body["imports"].as_array().unwrap();
        assert!(rows.iter().any(
            |row| row["timestamp"] == "20260105_000000" && row.as_object().unwrap().len() == 14
        ));
        assert!(rows.iter().any(
            |row| row["timestamp"] == "20260106_000000" && row.as_object().unwrap().len() == 7
        ));
        assert!(rows.iter().any(
            |row| row["timestamp"] == "20260107_000000" && row.as_object().unwrap().len() == 15
        ));
    }

    #[test]
    fn ac19_four_phase_seeds_match_the_corpus_generator_layout() {
        for phase in ["unestablished", "corrupt", "empty", "populated"] {
            let root = phase_root(phase);
            let source = root
                .path()
                .join("apps/import/journal_sources/corpus_peer.json");
            assert_eq!(
                serde_json::from_slice::<Value>(&fs::read(source).unwrap()).unwrap(),
                json!({"key":"corpusSourceKey0000000000000000000000000000","name":"corpus_peer","created_at":1767225600000_i64,"enabled":true,"revoked":false,"revoked_at":null,"stats":{"segments_received":0,"entities_received":0,"facets_received":0,"imports_received":0,"config_received":0}}),
                "{phase} source registry"
            );
            for area in ["segments", "entities", "facets", "imports", "config"] {
                assert!(
                    root.path().join("imports/corpusSo").join(area).is_dir(),
                    "{phase} {area}"
                );
            }
            match phase {
                "unestablished" => assert!(!root.path().join("config/journal.json").exists()),
                "corrupt" => assert_eq!(
                    fs::read_to_string(root.path().join("config/journal.json")).unwrap(),
                    "{\"setup\": {\"completed_at\": 17672256"
                ),
                "empty" => assert_eq!(
                    fs::read_to_string(root.path().join("config/journal.json")).unwrap(),
                    "{\"setup\":{\"completed_at\":1767225600}}\n"
                ),
                "populated" => {
                    for (timestamp, client_item_id) in [
                        (OK, "corpus-item-1"),
                        (FAILED, "corpus-item-2"),
                        (PENDING, "corpus-item-3"),
                        (CONTENT, "corpus-item-4"),
                    ] {
                        let metadata: Value = serde_json::from_slice(
                            &fs::read(
                                root.path()
                                    .join("imports")
                                    .join(timestamp)
                                    .join("import.json"),
                            )
                            .unwrap(),
                        )
                        .unwrap();
                        assert_eq!(metadata["client_item_id"], client_item_id, "{timestamp}");
                        assert!(
                            metadata.get("task_id").is_none(),
                            "generator does not persist task_id"
                        );
                    }
                }
                _ => unreachable!(),
            }
        }
    }
}
