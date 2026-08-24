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
pub(crate) mod tests {
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
        "/app/import/journal/corpusSo/ingest/segments",
        "/app/import/journal/corpusSo/ingest/entities",
        "/app/import/journal/corpusSo/ingest/imports",
        "/app/import/journal/corpusSo/ingest/config",
        "/app/import/journal/corpusSo/ingest/facets",
    ];
    const BROWSER_WRITE_PATHS: &[&str] = &[
        "/app/import/api/save",
        "/app/import/api/save-path",
        "/app/import/api/meta",
        "/app/import/api/start",
        "/app/import/api/journal-archive/preview",
        "/app/import/api/journal-sources/create",
        "/app/import/api/journal-sources/corpus_peer/revoke",
        "/app/import/api/journal-sources/corpus_peer/resolve-entity",
        "/app/import/api/journal-sources/corpus_peer/resolve-facet",
        "/app/import/api/journal-sources/corpus_peer/resolve-config",
        "/app/import/api/journal-sources/corpus_peer/resolve-config-all",
    ];

    pub(crate) async fn request(
        root: &Path,
        method: &str,
        uri: &str,
        request_json: Option<&Value>,
    ) -> (StatusCode, String, Option<String>, Vec<u8>) {
        let (status, headers, body) = response(root, method, uri, request_json).await;
        let content_type = headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let location = headers
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        (status, content_type, location, body)
    }

    pub(crate) async fn response_header(
        root: &Path,
        method: &str,
        uri: &str,
        request_json: Option<&Value>,
        name: &str,
    ) -> Option<String> {
        let (_, headers, _) = response(root, method, uri, request_json).await;
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned)
    }

    async fn response(
        root: &Path,
        method: &str,
        uri: &str,
        request_json: Option<&Value>,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
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
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body")
            .to_vec();
        (status, headers, body)
    }

    pub(crate) async fn json_request(root: &Path, method: &str, uri: &str) -> (StatusCode, Value) {
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

    #[tokio::test]
    async fn ac4_corpus_replay_matches_every_recorded_case() {
        let corpus: Value = serde_json::from_str(CORPUS).expect("corpus JSON");
        let mut passed = 0;
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
                } else {
                    unexpected.push(format!("{phase} {method} {path}"));
                }
            }
        }
        assert_eq!(passed, 136, "unexpected replay cases: {unexpected:?}");
        assert!(
            unexpected.is_empty(),
            "unexpected replay cases: {unexpected:?}"
        );
    }

    #[tokio::test]
    async fn ac5_import_shell_matches_real_shell_bytes_in_established_phases() {
        let shell = include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
        for phase in ["empty", "populated"] {
            let root = phase_root(phase);
            let (status, content_type, _, body) =
                request(root.path(), "GET", "/app/import/", None).await;
            assert_eq!(status, StatusCode::OK, "{phase}");
            assert_eq!(content_type, "text/html; charset=utf-8", "{phase}");
            assert_eq!(body.as_slice(), shell, "{phase}");
        }
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
            b"old\n",
        );
        seed_import(
            root.path(),
            "20260102_000000",
            "new.txt",
            "text/plain",
            "new",
            Some(json!({"processed": true})),
            b"new\n",
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
            b"jsonl\n",
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
            b"markdown\n",
        );
        let transcript = root.path().join("chronicle/20260103/import.chatgpt/key-a");
        fs::create_dir_all(&transcript).unwrap();
        let unicode_preview = format!("{}😀{}", "a".repeat(79), "z".repeat(200));
        fs::write(
            transcript.join("conversation_transcript.jsonl"),
            format!(
                "{{\"topics\":\"\"}}\n{{\"speaker\":\"Human\",\"text\":{}}}\n",
                serde_json::to_string(&unicode_preview).unwrap()
            ),
        )
        .unwrap();
        let note = root.path().join("chronicle/20260104/import.obsidian/key-b");
        fs::create_dir_all(&note).unwrap();
        fs::write(note.join("note.md"), "## One\nfirst\n## Two\nsecond\n").unwrap();
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
            if timestamp == jsonl {
                assert_eq!(rows[0]["preview"].as_str().unwrap().chars().count(), 200);
                assert_eq!(rows[0]["title"].as_str().unwrap().chars().count(), 80);
                assert!(rows[0]["title"].as_str().unwrap().ends_with('😀'));
            } else {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0]["title"], "One");
            }
            let (_, _, _, second) = request(root.path(), "GET", &uri, None).await;
            assert_eq!(first, second, "{timestamp} uses persisted manifest");
        }
        let detail = "20260105_000000";
        seed_import(
            root.path(),
            detail,
            "detail.jsonl",
            "application/json",
            "detail",
            Some(
                json!({"source_type":"chatgpt", "all_created_files":["chronicle/20260105/import.chatgpt/key-c/conversation_transcript.jsonl"]}),
            ),
            b"detail\n",
        );
        let detail_transcript = root.path().join("chronicle/20260105/import.chatgpt/key-c");
        fs::create_dir_all(&detail_transcript).unwrap();
        fs::write(
            detail_transcript.join("conversation_transcript.jsonl"),
            "{\"topics\":\"detail\"}\n{\"speaker\":\"Human\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        let detail_uri = format!("/app/import/api/{detail}/content/seg-0");
        let (status, _, _, first) = request(root.path(), "GET", &detail_uri, None).await;
        assert_eq!(status, StatusCode::OK, "detail route first request");
        let manifest = root
            .path()
            .join("imports")
            .join(detail)
            .join("content_manifest.jsonl");
        assert_eq!(
            fs::metadata(&manifest).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let row: Value =
            serde_json::from_str(fs::read_to_string(&manifest).unwrap().trim()).unwrap();
        let keys: BTreeSet<_> = ["id", "title", "date", "type", "preview", "meta", "segments"]
            .into_iter()
            .collect();
        assert_eq!(
            row.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            keys
        );
        assert_eq!(row["id"], "seg-0");
        let (_, _, _, second) = request(root.path(), "GET", &detail_uri, None).await;
        assert_eq!(first, second, "detail route uses its persisted manifest");
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
        let corrupt = "20260106_000000";
        let directory = root.path().join("imports").join(corrupt);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("imported.json"), "{").unwrap();
        let (status, body) = json_request(
            root.path(),
            "GET",
            &format!("/app/import/api/{corrupt}/content"),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "import_metadata_failed");
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
        let directory = root.path().join("imports").join(FAILED);
        let decisions = root.path().join("decisions.jsonl");
        fs::write(
            &decisions,
            "{\"action\":\"entity_staged\",\"source\":{\"name\":\"Ada\"},\"target\":{\"name\":\"Ada Lovelace\"},\"staging_path\":\"entities/ada.json\"}\n{\"action\":\"segment_errored\",\"item_id\":\"segment-1\",\"reason\":\"bad\"}\n",
        )
        .unwrap();
        let mut imported: Value =
            serde_json::from_slice(&fs::read(directory.join("imported.json")).unwrap()).unwrap();
        imported["merge_summary"] = json!({});
        imported["merge_log_path"] = json!(decisions);
        imported["merge_staging_path"] = json!("staging");
        imported["summary_errors"] = json!(["summary failed"]);
        fs::write(
            directory.join("imported.json"),
            serde_json::to_vec(&imported).unwrap(),
        )
        .unwrap();
        fs::write(
            directory.join("segments.json"),
            json!({"segments":["one"]}).to_string(),
        )
        .unwrap();
        let (_, body) =
            json_request(root.path(), "GET", &format!("/app/import/api/{FAILED}")).await;
        assert_eq!(body["segments_json"], json!({"segments":["one"]}));
        assert_eq!(
            body["merge_artifact_paths"],
            json!({"decisions":decisions,"staging":"staging"})
        );
        assert_eq!(
            body["decision_highlights"],
            json!({"staged_entities":[{"source_name":"Ada","target_name":"Ada Lovelace","staging_path":"entities/ada.json"}],"errored_segments":[{"item_id":"segment-1","reason":"bad"}]})
        );
        assert_eq!(body["summary_errors"], json!(["summary failed"]));
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
        assert_eq!(
            entity
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            [
                "area",
                "source_id",
                "reason",
                "source_entity",
                "match_candidates",
                "staged_at"
            ]
            .into_iter()
            .collect()
        );
        let facet = items.iter().find(|item| item["area"] == "facets").unwrap();
        assert_eq!(
            facet
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            ["area", "staged_file", "facet", "file_type", "custom"]
                .into_iter()
                .collect()
        );
        let config = items.iter().find(|item| item["area"] == "config").unwrap();
        assert_eq!(
            config
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            ["area", "diff"].into_iter().collect()
        );
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
                include_bytes!("../assets/guides/ics.md").to_vec()
            )
        );
        let (code, missing) = json_request(root.path(), "GET", "/app/import/api/guide/nope").await;
        assert_eq!(
            (code, missing),
            (
                StatusCode::NOT_FOUND,
                json!({"detail":"No guide available for 'nope'","error":"I couldn't find that file.","reason_code":"file_not_found"})
            )
        );
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
    async fn ac15_registered_write_routes_preserve_phase_and_door_auth_guards() {
        for phase in ["empty", "populated", "unestablished", "corrupt"] {
            let root = phase_root(phase);
            for path in DOOR_PATHS {
                let method = if path.contains("/manifest/") {
                    "GET"
                } else {
                    "POST"
                };
                let (status, content_type, _, _) = request(root.path(), method, path, None).await;
                assert_eq!(
                    (status, content_type),
                    (
                        StatusCode::UNAUTHORIZED,
                        "text/html; charset=utf-8".to_owned()
                    ),
                    "{phase} {path}"
                );
            }
        }
        for phase in ["unestablished", "corrupt"] {
            let root = phase_root(phase);
            for path in BROWSER_WRITE_PATHS {
                let (status, content_type, location, _) =
                    request(root.path(), "POST", path, None).await;
                if phase == "unestablished" {
                    assert_eq!(
                        (status, location.as_deref()),
                        (StatusCode::FOUND, Some("/init")),
                        "{path}"
                    );
                } else {
                    assert_eq!(
                        (status, content_type),
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "application/json".to_owned()
                        ),
                        "{path}"
                    );
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
        let root = phase_root("empty");
        for (timestamp, upload) in [
            ("20260108_000000", 100_000.0),
            ("20260109_000000", 199_995.0),
        ] {
            seed_import(
                root.path(),
                timestamp,
                "waiting.md",
                "text/plain",
                "timeout",
                None,
                b"# waiting\n",
            );
            let path = root
                .path()
                .join("imports")
                .join(timestamp)
                .join("import.json");
            let mut metadata: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            metadata["task_id"] = json!("task");
            metadata["upload_timestamp"] = json!(upload * 1000.0);
            fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        }
        let old_upload = crate::imports::load_import_info(root.path(), "20260108_000000").unwrap();
        let recent_upload =
            crate::imports::load_import_info(root.path(), "20260109_000000").unwrap();
        assert_eq!(old_upload.imported_at, 100_000.0);
        assert_eq!(recent_upload.imported_at, 199_995.0);
        assert_eq!(
            resolve_status_with_timeout(&old_upload, 200_000.0, 10.0).0,
            "failed"
        );
        assert_eq!(
            resolve_status_with_timeout(&recent_upload, 200_000.0, 10.0).0,
            "running"
        );
    }

    #[test]
    fn legacy_import_metadata_keeps_facet_available_to_loaders() {
        let root = phase_root("empty");
        let timestamp = "20260110_000000";
        seed_import(
            root.path(),
            timestamp,
            "legacy.txt",
            "text/plain",
            "legacy",
            None,
            b"legacy\n",
        );
        let path = root
            .path()
            .join("imports")
            .join(timestamp)
            .join("import.json");
        let mut metadata: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        metadata["facet"] = json!("work");
        fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();

        let info = crate::imports::load_import_info(root.path(), timestamp).unwrap();
        assert_eq!(info.values["facet"], json!("work"));
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
        // These expectations are the generator's _build_journal and
        // _seed_journal_sources contract (scripts/convey_import_corpus.py:377-508),
        // deliberately independent of test_support's writers.
        let source_record = json!({"key":"corpusSourceKey0000000000000000000000000000","name":"corpus_peer","created_at":1767225600000_i64,"enabled":true,"revoked":false,"revoked_at":null,"stats":{"segments_received":0,"entities_received":0,"facets_received":0,"imports_received":0,"config_received":0}});
        for phase in ["unestablished", "corrupt", "empty", "populated"] {
            let root = phase_root(phase);
            let source = root
                .path()
                .join("apps/import/journal_sources/corpus_peer.json");
            match phase {
                "unestablished" | "empty" | "populated" => {
                    assert_eq!(
                        serde_json::from_slice::<Value>(&fs::read(&source).unwrap()).unwrap(),
                        source_record,
                        "{phase} source registry"
                    );
                    assert_eq!(
                        fs::read_to_string(root.path().join("imports/corpusSo/source.json"))
                            .unwrap(),
                        "{}"
                    );
                    for area in ["segments", "entities", "facets", "imports", "config"] {
                        assert!(
                            root.path().join("imports/corpusSo").join(area).is_dir(),
                            "{phase} {area}"
                        );
                    }
                }
                "corrupt" => {
                    assert!(!source.exists(), "generator does not seed corrupt sources");
                    assert!(!root.path().join("imports/corpusSo").exists());
                }
                _ => unreachable!(),
            }
            match phase {
                "unestablished" => assert!(!root.path().join("config/journal.json").exists()),
                "corrupt" => assert_eq!(
                    fs::read_to_string(root.path().join("config/journal.json")).unwrap(),
                    "{\"setup\": {\"completed_at\": 17672256"
                ),
                "empty" => assert_eq!(
                    fs::read_to_string(root.path().join("config/journal.json")).unwrap(),
                    "{\n  \"setup\": {\n    \"completed_at\": 1767225600\n  }\n}\n"
                ),
                "populated" => {
                    assert_eq!(
                        fs::read_to_string(root.path().join("config/journal.json")).unwrap(),
                        "{\n  \"setup\": {\n    \"completed_at\": 1767225600\n  }\n}\n"
                    );
                    for (timestamp, filename, mime_type, client_item_id, payload, imported) in [
                        (
                            OK,
                            "notes.txt",
                            "text/plain",
                            "corpus-item-1",
                            b"corpus import payload\n".as_slice(),
                            Some(json!({"processed":true,"files_written":1,"days":["20260801"]})),
                        ),
                        (
                            FAILED,
                            "broken.ics",
                            "text/calendar",
                            "corpus-item-2",
                            b"not really an ics\n".as_slice(),
                            Some(
                                json!({"processed":false,"error":"calendar payload could not be parsed","error_stage":"detect"}),
                            ),
                        ),
                        (
                            PENDING,
                            "waiting.md",
                            "text/plain",
                            "corpus-item-3",
                            b"# waiting\n".as_slice(),
                            None,
                        ),
                        (
                            CONTENT,
                            "conversations.json",
                            "application/json",
                            "corpus-item-4",
                            b"[]\n".as_slice(),
                            Some(
                                json!({"processed":true,"files_written":3,"source_type":"chatgpt","days":["20260801","20260802","20260901"]}),
                            ),
                        ),
                    ] {
                        let directory = root.path().join("imports").join(timestamp);
                        let metadata: Value = serde_json::from_slice(
                            &fs::read(directory.join("import.json")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(
                            metadata,
                            json!({"original_filename":filename,"file_size":42,"mime_type":mime_type,"facet":null,"setting":null,"user_timestamp":null,"imported_via":"web_dashboard","link_id":null,"observer_handle":null,"source":"corpus","source_hash":"sha256:0000000000000000000000000000000000000000000000000000000000000000","client_item_id":client_item_id}),
                            "{timestamp} import metadata"
                        );
                        assert_eq!(
                            fs::read(directory.join(filename)).unwrap(),
                            payload,
                            "{timestamp} payload"
                        );
                        let imported_path = directory.join("imported.json");
                        match imported {
                            Some(expected) => assert_eq!(
                                serde_json::from_slice::<Value>(&fs::read(imported_path).unwrap())
                                    .unwrap(),
                                expected,
                                "{timestamp} imported result"
                            ),
                            None => assert!(
                                !imported_path.exists(),
                                "{timestamp} has no imported result"
                            ),
                        }
                    }
                    let manifest: Vec<Value> = fs::read_to_string(
                        root.path()
                            .join("imports")
                            .join(CONTENT)
                            .join("content_manifest.jsonl"),
                    )
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                    assert_eq!(
                        manifest,
                        vec![
                            json!({"id":"corpus-entry-1","date":"20260801","title":"first conversation","preview":"a short preview of the first entry","body":"the full body of the first entry"}),
                            json!({"id":"corpus-entry-2","date":"20260802","title":"second conversation","preview":"a short preview of the second entry","body":"the full body of the second entry"}),
                            json!({"id":"corpus-entry-3","date":"20260901","title":"a September conversation","preview":"a short preview of the third entry","body":"the full body of the third entry"}),
                        ]
                    );
                }
                _ => unreachable!(),
            }
        }
    }
}
