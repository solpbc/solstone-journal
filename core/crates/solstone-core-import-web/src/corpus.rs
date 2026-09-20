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
    use std::{collections::BTreeSet, fs, path::Path};

    use axum::{
        body::{Body, to_bytes},
        http::{HeaderMap, Request, StatusCode},
    };
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use tower::ServiceExt;

    use super::{CTIME_PATHS, DECLARED_STATUS_ROOT_CREATED_AT_OVERFIRE, JsonPath, Segment};
    use crate::test_support::{
        CONTENT, FAILED, OK, PENDING, phase_root, populated_root, seed_import,
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
                } else if let Some(recorded) = case.get("body_sha256").and_then(Value::as_str) {
                    actual_body.len() == case["body_bytes"].as_u64().expect("body bytes") as usize
                        && format!("{:x}", Sha256::digest(&actual_body)) == recorded
                } else {
                    // No recorded digest: a served frontend asset, deliberately not
                    // body-asserted. Status, content type and Location above still are.
                    // See the fixture's corpus_maintenance note.
                    true
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
    async fn ac9_both_routes_project_seven_key_manifests_without_writing() {
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
            assert!(!manifest.exists(), "a read must not create a manifest");
            let body: Value = serde_json::from_slice(&first).unwrap();
            let rows = body["items"].as_array().unwrap();
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
            assert_eq!(first, second, "{timestamp} projects the same content");
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
        assert!(
            !manifest.exists(),
            "a detail read must not create a manifest"
        );
        let body: Value = serde_json::from_slice(&first).unwrap();
        let row = &body["item"];
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
        assert_eq!(first, second, "detail route projects the same content");
    }

    #[tokio::test]
    async fn note_detail_reads_markdown_and_refuses_an_external_symlink() {
        let root = phase_root("empty");
        let stamp = "20260907_090000";
        let segment = root
            .path()
            .join("chronicle/20260907/import.obsidian/note-a");
        fs::create_dir_all(&segment).unwrap();
        let transcript = segment.join("note_transcript.md");
        fs::write(&transcript, "# garden\n\nPlant the orchard in October.").unwrap();
        seed_import(
            root.path(),
            stamp,
            "note.md",
            "text/markdown",
            "markdown",
            Some(
                json!({"source_type":"obsidian", "all_created_files":["chronicle/20260907/import.obsidian/note-a/note_transcript.md"]}),
            ),
            b"note",
        );
        let uri = format!("/app/import/api/{stamp}/content/item-0");
        let (status, body) = json_request(root.path(), "GET", &uri).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["content"][0],
            json!({"type":"markdown","content":"# garden\n\nPlant the orchard in October."})
        );
        assert!(
            !root
                .path()
                .join(format!("imports/{stamp}/content_manifest.jsonl"))
                .exists()
        );
        // Also exercise persisted manifests, so containment is not only in projection.
        fs::write(
            root.path()
                .join(format!("imports/{stamp}/content_manifest.jsonl")),
            format!("{}\n", body["item"]),
        )
        .unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("private.md"), "OUTSIDE_SENTINEL").unwrap();
        fs::remove_file(&transcript).unwrap();
        std::os::unix::fs::symlink(outside.path().join("private.md"), &transcript).unwrap();
        let (status, body) = json_request(root.path(), "GET", &uri).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "import_content_failed");
        assert!(!body.to_string().contains("OUTSIDE_SENTINEL"));
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
                json!({"detail":"No guide available for 'nope'","error":"that file isn't available.","reason_code":"file_not_found"})
            )
        );
        for path in ["..", "%2e%2e%2f", "ICS"] {
            let (code, body) =
                json_request(root.path(), "GET", &format!("/app/import/api/guide/{path}")).await;
            assert_eq!(
                (code, body),
                (
                    StatusCode::BAD_REQUEST,
                    json!({"detail":"Invalid source name","error":"one of those values couldn't be used.","reason_code":"invalid_request_value"})
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
        let root = phase_root("empty");
        let timestamp = "20260108_000000";
        let import_dir = root.path().join("imports").join(timestamp);
        fs::create_dir_all(&import_dir).unwrap();

        let meta = json!({
            "task_id": "task_123",
            "upload_timestamp": 100_000.0 * 1000.0,
            "source": "ics"
        });
        fs::write(
            import_dir.join("import.json"),
            serde_json::to_vec(&meta).unwrap(),
        )
        .unwrap();

        let proj_running = solstone_core_import::projection::project_import_result_with_clock(
            root.path(),
            timestamp,
            101_000.0,
        );
        assert_eq!(
            proj_running.status,
            solstone_core_import::projection::ProjectionStatus::Running
        );
        assert_eq!(proj_running.error, None);

        let proj_timeout = solstone_core_import::projection::project_import_result_with_clock(
            root.path(),
            timestamp,
            104_000.0,
        );
        assert_eq!(
            proj_timeout.status,
            solstone_core_import::projection::ProjectionStatus::Failed
        );
        assert_eq!(
            proj_timeout.error,
            Some("Import never completed".to_string())
        );
        assert_eq!(proj_timeout.error_stage, Some("timeout".to_string()));

        let completed_meta = json!({
            "task_id": "task_123",
            "upload_timestamp": 100_000.0 * 1000.0,
            "processing_completed": true,
            "source": "ics"
        });
        fs::write(
            import_dir.join("import.json"),
            serde_json::to_vec(&completed_meta).unwrap(),
        )
        .unwrap();
        let proj_success = solstone_core_import::projection::project_import_result_with_clock(
            root.path(),
            timestamp,
            200_000.0,
        );
        assert_eq!(
            proj_success.status,
            solstone_core_import::projection::ProjectionStatus::Success
        );
    }

    #[test]
    fn ac17a_upload_timestamp_is_the_shared_sort_and_timeout_time() {
        let root = phase_root("empty");
        for (timestamp, upload_sec) in [
            ("20260108_000000", 100_000.0),
            ("20260109_000000", 199_000.0),
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
            metadata["upload_timestamp"] = json!(upload_sec * 1000.0);
            fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        }

        let now_sec = 200_000.0;
        let old_proj = solstone_core_import::projection::project_import_result_with_clock(
            root.path(),
            "20260108_000000",
            now_sec,
        );
        let recent_proj = solstone_core_import::projection::project_import_result_with_clock(
            root.path(),
            "20260109_000000",
            now_sec,
        );
        assert_eq!(old_proj.imported_at, 100_000.0);
        assert_eq!(recent_proj.imported_at, 199_000.0);
        assert_eq!(
            old_proj.status,
            solstone_core_import::projection::ProjectionStatus::Failed
        );
        assert_eq!(old_proj.error_stage, Some("timeout".to_string()));
        assert_eq!(
            recent_proj.status,
            solstone_core_import::projection::ProjectionStatus::Running
        );
    }

    #[tokio::test]
    async fn http_list_times_out_legacy_task_rows_from_backdated_upload() {
        let root = phase_root("empty");
        let timestamp = "20260108_120000";
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
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        metadata["task_id"] = json!("task");
        metadata["upload_timestamp"] = json!(now_ms.saturating_sub(4_000_000));
        fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();

        let (status, body) = json_request(root.path(), "GET", "/app/import/api/list").await;
        assert_eq!(status, StatusCode::OK);
        let row = body["imports"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["timestamp"] == timestamp)
            .expect("timed-out row");
        assert_eq!(row["status"], "failed");
        assert_eq!(row["error"], "Import never completed");
        assert_eq!(row["error_stage"], "timeout");
    }

    #[tokio::test]
    async fn corrupted_imported_json_returns_http_200_unavailable() {
        let root = phase_root("empty");
        let stamp = "20260111_000000";
        let dir = root.path().join("imports").join(stamp);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("import.json"), "{}").unwrap();
        fs::write(dir.join("imported.json"), "{ invalid json").unwrap();

        let (status, list) = json_request(root.path(), "GET", "/app/import/api/list").await;
        assert_eq!(status, StatusCode::OK);
        let item = list["imports"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["timestamp"] == stamp)
            .expect("row present");
        assert_eq!(item["status"], "unavailable");

        let (status, detail) =
            json_request(root.path(), "GET", &format!("/app/import/api/{stamp}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["status"], "unavailable");
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

        let meta = solstone_core_import::read_import_metadata(root.path(), timestamp).unwrap();
        assert_eq!(meta.get("facet").and_then(Value::as_str), Some("work"));
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

    const TINY_PNG_FIXTURE: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    struct FakeWebPdfWorker {
        payload: solstone_core_import_sources::document::PdfPayload,
    }

    impl solstone_core_import_sources::document::PdfWorker for FakeWebPdfWorker {
        fn execute(
            &self,
            _request: &solstone_core_import_sources::document::PdfWorkerRequest,
        ) -> Result<
            solstone_core_import_sources::document::PdfPayload,
            solstone_core_import_sources::document::WorkerFailure,
        > {
            Ok(self.payload.clone())
        }
    }

    fn collect_tree_bytes(dir: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
        let mut map = std::collections::BTreeMap::new();
        fn walk(
            root: &Path,
            current: &Path,
            map: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
        ) {
            if let Ok(entries) = fs::read_dir(current) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if path.is_file() {
                        if let Ok(bytes) = fs::read(&path)
                            && let Ok(rel) = path.strip_prefix(root)
                        {
                            map.insert(rel.to_path_buf(), bytes);
                        }
                    } else if path.is_dir() {
                        walk(root, &path, map);
                    }
                }
            }
        }
        walk(dir, dir, &mut map);
        map
    }

    #[tokio::test]
    async fn test_native_producer_roundtrip_immutability_helper() {
        let temp = phase_root("empty");
        let root = temp.path();
        let img_path = root.join("test.png");
        fs::write(&img_path, TINY_PNG_FIXTURE).unwrap();

        let id = "20260408_120000";
        let req = solstone_core_import_sources::producer::NativeProducerRequest {
            journal_root: root,
            source_path: &img_path,
            import_id: id,
            source: solstone_core_import::RegistrySource::Image,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
        };
        solstone_core_import_sources::producer::run_native_producer(
            req,
            &solstone_core_import_sources::producer::NullWireClient,
            &solstone_core_import_sources::producer::NullPdfWorker,
            &solstone_core_import_sources::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        )
        .expect("import succeeds");

        let before = collect_tree_bytes(root);

        let (status1, detail) = json_request(root, "GET", &format!("/app/import/api/{id}")).await;
        assert_eq!(status1, StatusCode::OK);
        assert_eq!(detail["status"], "success");

        let (status2, content) =
            json_request(root, "GET", &format!("/app/import/api/{id}/content")).await;
        assert_eq!(status2, StatusCode::OK);
        assert!(!content["items"].as_array().unwrap().is_empty());

        let after = collect_tree_bytes(root);
        assert_eq!(before, after, "GET requests must not mutate disk state");
    }

    #[tokio::test]
    async fn native_rows_carry_projection_facts_and_legacy_rows_keep_their_shape() {
        let temp = phase_root("empty");
        let root = temp.path();
        let img_path = root.join("photo.png");
        fs::write(&img_path, TINY_PNG_FIXTURE).unwrap();
        let native = "20260408_130002";
        solstone_core_import_sources::producer::run_native_producer(
            solstone_core_import_sources::producer::NativeProducerRequest {
                journal_root: root,
                source_path: &img_path,
                import_id: native,
                source: solstone_core_import::RegistrySource::Image,
                revision: None,
                password: None,
                force: false,
                expected_generation: None,
            },
            &solstone_core_import_sources::producer::NullWireClient,
            &solstone_core_import_sources::producer::NullPdfWorker,
            &solstone_core_import_sources::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        )
        .expect("native import succeeds");
        // A legacy importer's row: flat fields only, no attempt, no typed publication.
        let legacy = "20260408_130003";
        let legacy_dir = root.join("imports").join(legacy);
        fs::create_dir_all(&legacy_dir).unwrap();
        fs::write(
            legacy_dir.join("import.json"),
            r#"{"original_filename":"c.json"}"#,
        )
        .unwrap();
        fs::write(
            legacy_dir.join("imported.json"),
            r#"{"processed":true,"entries_written":3,"total_files_created":2,"source_type":"chatgpt"}"#,
        )
        .unwrap();

        let (status, list) = json_request(root, "GET", "/app/import/api/list").await;
        assert_eq!(status, StatusCode::OK);
        let rows = list["imports"].as_array().unwrap();
        let row = |id: &str| rows.iter().find(|row| row["timestamp"] == id).unwrap();

        // The native row shows what the import really did, with its attempt identity and its
        // gaps (the null wire client cannot describe the image, so the row says so): this is
        // what was missing when history read "1 import, 0 entries" and a stuck row before.
        let native_row = row(native);
        assert_eq!(native_row["status"], "success");
        assert_eq!(native_row["entries_written"], 1);
        assert_eq!(native_row["source_type"], "image");
        assert_eq!(native_row["has_gaps"], true);
        assert!(
            native_row["unavailable_description"].is_string(),
            "{native_row}"
        );
        assert!(native_row["generation"].is_number(), "{native_row}");
        // The legacy row is exactly its recorded shape: no projection-only keys appear.
        let legacy_row = row(legacy);
        assert_eq!(legacy_row["source_type"], "chatgpt");
        assert_eq!(legacy_row["entries_written"], 3);
        for key in ["generation", "has_gaps", "attempt_id", "unavailable_pages"] {
            assert!(
                legacy_row.get(key).is_none(),
                "legacy row grew {key}: {legacy_row}"
            );
        }
        assert_eq!(list["total_entries_written"], 4);

        let (status, detail) =
            json_request(root, "GET", &format!("/app/import/api/{native}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["entries_written"], 1);
        assert_eq!(detail["has_gaps"], true);
        assert!(detail["generation"].is_number(), "{detail}");
        let (_, legacy_detail) =
            json_request(root, "GET", &format!("/app/import/api/{legacy}")).await;
        assert!(legacy_detail.get("generation").is_none(), "{legacy_detail}");
    }

    #[tokio::test]
    async fn test_native_image_get_recovers_source_count_date_content() {
        let temp = phase_root("empty");
        let root = temp.path();
        let img_path = root.join("photo.png");
        fs::write(&img_path, TINY_PNG_FIXTURE).unwrap();

        let id = "20260408_130000";
        let req = solstone_core_import_sources::producer::NativeProducerRequest {
            journal_root: root,
            source_path: &img_path,
            import_id: id,
            source: solstone_core_import::RegistrySource::Image,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
        };
        solstone_core_import_sources::producer::run_native_producer(
            req,
            &solstone_core_import_sources::producer::NullWireClient,
            &solstone_core_import_sources::producer::NullPdfWorker,
            &solstone_core_import_sources::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        )
        .expect("import succeeds");

        let (status, body) = json_request(root, "GET", &format!("/app/import/api/{id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "success");
        assert_eq!(body["imported_json"]["status"], "success");
        assert_eq!(
            body["imported_json"]["schema"],
            "solstone.import.publication.v1"
        );

        let (c_status, c_body) =
            json_request(root, "GET", &format!("/app/import/api/{id}/content")).await;
        assert_eq!(c_status, StatusCode::OK);
        assert_eq!(c_body["source_type"], "image");
        assert_eq!(c_body["total"], 1);
        assert_eq!(
            c_body["months"]
                .as_object()
                .unwrap()
                .values()
                .map(|v| v.as_u64().unwrap())
                .sum::<u64>(),
            1
        );
        let items = c_body["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["stream"], "import.image");

        let item_id = items[0]["id"].as_str().unwrap();
        let (item_status, item_body) = json_request(
            root,
            "GET",
            &format!("/app/import/api/{id}/content/{item_id}"),
        )
        .await;
        assert_eq!(item_status, StatusCode::OK);
        assert!(!item_body["content"].as_array().unwrap().is_empty());

        // Historical fixture with imported.json but no attempt timing
        let hist_id = "20260101_100000";
        let hist_dir = root.join("imports").join(hist_id);
        fs::create_dir_all(&hist_dir).unwrap();
        fs::write(
            hist_dir.join("imported.json"),
            json!({
                "schema": "solstone.import.publication.v1",
                "status": "success",
                "importer": "image",
                "segments": []
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            hist_dir.join("import.json"),
            json!({
                "original_filename": "hist.png",
                "file_size": 100,
                "mime_type": "image/png",
                "source": "image"
            })
            .to_string(),
        )
        .unwrap();

        let (_, hist_body) = json_request(root, "GET", &format!("/app/import/api/{hist_id}")).await;
        assert_eq!(hist_body["attempt"]["duration_ms"], Value::Null);
    }

    #[tokio::test]
    async fn test_native_pdf_get_no_manifest_json() {
        let temp = phase_root("empty");
        let root = temp.path();
        let pdf_path = root.join("doc.pdf");
        fs::write(&pdf_path, b"%PDF-1.4 fake").unwrap();

        let id = "20260408_140000";
        let text = "Here is valid extracted text for the test document exceeding fifty characters.";
        let payload = solstone_core_import_sources::document::PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![solstone_core_import_sources::document::PdfPage {
                index: 0,
                chars: text.len(),
                text: Some(text.to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let worker = FakeWebPdfWorker { payload };
        let req = solstone_core_import_sources::producer::NativeProducerRequest {
            journal_root: root,
            source_path: &pdf_path,
            import_id: id,
            source: solstone_core_import::RegistrySource::Document,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
        };
        solstone_core_import_sources::producer::run_native_producer(
            req,
            &solstone_core_import_sources::producer::NullWireClient,
            &worker,
            &solstone_core_import_sources::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        )
        .expect("pdf import succeeds");

        assert!(!root.join("imports").join(id).join("manifest.json").exists());

        let (status, body) = json_request(root, "GET", &format!("/app/import/api/{id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "success");
        assert_eq!(body["imported_json"]["status"], "success");
        assert_eq!(
            body["imported_json"]["schema"],
            "solstone.import.publication.v1"
        );

        let (c_status, c_body) =
            json_request(root, "GET", &format!("/app/import/api/{id}/content")).await;
        assert_eq!(c_status, StatusCode::OK);
        assert_eq!(c_body["source_type"], "document");
        assert_eq!(c_body["total"], 1);
        let items = c_body["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["stream"], "import.document");
    }

    #[tokio::test]
    async fn test_publication_failure_get_is_not_success() {
        let temp = phase_root("empty");
        let root = temp.path();
        let id = "20260408_150000";
        let import_dir = root.join("imports").join(id);
        fs::create_dir_all(&import_dir).unwrap();
        fs::write(
            import_dir.join("import.json"),
            json!({
                "original_filename": "failed.png",
                "file_size": 100,
                "mime_type": "image/png",
                "source": "image",
                "attempt": {
                    "attempt_id": id,
                    "generation": 1,
                    "state": "unconfirmed",
                    "started_at_ms": 1000,
                    "finished_at_ms": 2000,
                    "failure_reason": "publication failed"
                }
            })
            .to_string(),
        )
        .unwrap();

        let (status, body) = json_request(root, "GET", &format!("/app/import/api/{id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_ne!(body["status"], "success");
        assert_eq!(body["status"], "unconfirmed");
    }

    #[tokio::test]
    async fn test_malformed_imported_json_is_not_ordinary_empty() {
        let temp = phase_root("empty");
        let root = temp.path();
        let id = "20260408_160000";
        let import_dir = root.join("imports").join(id);
        fs::create_dir_all(&import_dir).unwrap();
        fs::write(
            import_dir.join("import.json"),
            json!({
                "original_filename": "corrupt.png",
                "file_size": 100,
                "mime_type": "image/png",
                "source": "image"
            })
            .to_string(),
        )
        .unwrap();
        fs::write(import_dir.join("imported.json"), b"{ invalid json").unwrap();

        let (status, body) =
            json_request(root, "GET", &format!("/app/import/api/{id}/content")).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "import_metadata_failed");
    }

    #[tokio::test]
    async fn test_browse_source_day_differs_from_import_id_and_missing_retained_file() {
        let temp = phase_root("empty");
        let root = temp.path();
        let id = "20260408_170000";
        let source_day = "20260401";
        let segment_name = "120000_0";

        let chronicle_dir = root
            .join("chronicle")
            .join(source_day)
            .join("import.image")
            .join(segment_name);
        fs::create_dir_all(&chronicle_dir).unwrap();
        fs::write(
            chronicle_dir.join("image_transcript.md"),
            "# Sample Image Transcript",
        )
        .unwrap();

        let import_dir = root.join("imports").join(id);
        fs::create_dir_all(&import_dir).unwrap();
        fs::write(
            import_dir.join("import.json"),
            json!({
                "original_filename": "photo.png",
                "file_size": 100,
                "mime_type": "image/png",
                "source": "image"
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            import_dir.join("imported.json"),
            json!({
                "schema": "solstone.import.publication.v1",
                "status": "success",
                "importer": "image",
                "segments": [
                    {
                        "day": source_day,
                        "segment": segment_name,
                        "stream": "import.image",
                        "outcome": "published"
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();

        // 1. Content list returns item referencing source_day
        let (status, body) =
            json_request(root, "GET", &format!("/app/import/api/{id}/content")).await;
        assert_eq!(status, StatusCode::OK);
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["segments"][0]["day"], source_day);

        // 2. Content detail fetches from source_day
        let item_id = items[0]["id"].as_str().unwrap();
        let (item_status, item_body) = json_request(
            root,
            "GET",
            &format!("/app/import/api/{id}/content/{item_id}"),
        )
        .await;
        assert_eq!(item_status, StatusCode::OK);
        assert_eq!(
            item_body["content"][0]["content"],
            "# Sample Image Transcript"
        );

        // 3. Deleting retained chronicle segment causes import_content_failed, not empty success
        fs::remove_dir_all(&chronicle_dir).unwrap();
        let (missing_status, missing_body) = json_request(
            root,
            "GET",
            &format!("/app/import/api/{id}/content/{item_id}"),
        )
        .await;
        assert_eq!(missing_status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(missing_body["reason_code"], "import_content_failed");
    }

    #[tokio::test]
    async fn test_item_42_payload_hygiene_list_and_detail() {
        let temp = phase_root("empty");
        let root = temp.path();
        let id = "20260408_170000";
        let import_dir = root.join("imports").join(id);
        fs::create_dir_all(&import_dir).unwrap();
        fs::write(
            import_dir.join("import.json"),
            json!({
                "original_filename": "hygiene.png",
                "file_size": 100,
                "mime_type": "image/png",
                "source": "image",
                "attempt": {
                    "attempt_id": id,
                    "generation": 1,
                    "state": "completed",
                    "started_at_ms": 1000,
                    "finished_at_ms": 2000
                }
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            import_dir.join("imported.json"),
            json!({
                "schema": "solstone.import.publication.v1",
                "status": "success",
                "importer": "image",
                "segments": []
            })
            .to_string(),
        )
        .unwrap();

        // 1. List rows have no raw_metadata or raw_publication
        let (status, list) = json_request(root, "GET", "/app/import/api/list").await;
        assert_eq!(status, StatusCode::OK);
        let row = list["imports"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["timestamp"] == id)
            .expect("row found");
        let row_obj = row.as_object().unwrap();
        assert!(
            !row_obj.contains_key("raw_metadata"),
            "list row must not have raw_metadata"
        );
        assert!(
            !row_obj.contains_key("raw_publication"),
            "list row must not have raw_publication"
        );

        // 2. Detail does not insert attempt twice and has no duplicate raw_* vs import_json/imported_json
        let (status, detail) = json_request(root, "GET", &format!("/app/import/api/{id}")).await;
        assert_eq!(status, StatusCode::OK);
        let detail_obj = detail.as_object().unwrap();
        assert!(
            !detail_obj.contains_key("raw_metadata"),
            "detail must not have raw_metadata"
        );
        assert!(
            !detail_obj.contains_key("raw_publication"),
            "detail must not have raw_publication"
        );
        assert!(
            detail_obj.contains_key("import_json"),
            "detail must contain import_json"
        );
        assert!(
            detail_obj.contains_key("imported_json"),
            "detail must contain imported_json"
        );
    }
}
