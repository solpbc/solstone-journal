// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use chrono::{TimeZone, Utc};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use solstone_core_journal_stats_cli::bounded_input_mtime;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::attach::{TranscriptSegment, attach_visible_streams_to_ranges};
    use crate::shell::redirect_target;
    use crate::{Clock, router};

    const DAY: &str = "20260731";

    fn shell() -> axum::response::Response {
        axum::response::Response::new(Body::from("shell"))
    }

    fn write(root: &Path, relative: &str, contents: impl AsRef<[u8]>) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("directory");
        fs::write(path, contents).expect("file");
    }

    fn config(root: &Path) {
        write(
            root,
            "config/journal.json",
            br#"{"setup":{"completed_at":1700000000000}}"#,
        );
    }

    fn segment(root: &Path, day: &str, stream: &str, key: &str, files: &[(&str, &[u8])]) {
        for (name, contents) in files {
            write(
                root,
                &format!("chronicle/{day}/{stream}/{key}/{name}"),
                contents,
            );
        }
    }

    fn root() -> TempDir {
        let root = TempDir::new().expect("journal");
        config(root.path());
        segment(
            root.path(),
            DAY,
            "field",
            "090000_300",
            &[("mic_transcript.md", b"audio")],
        );
        segment(
            root.path(),
            DAY,
            "import.notes",
            "131000_60",
            &[("imported.md", b"import")],
        );
        segment(
            root.path(),
            DAY,
            "import.apple_health",
            "132000_60",
            &[("imported.md", b"health"), ("retained.m4a", b"raw")],
        );
        segment(
            root.path(),
            "20260801",
            "field",
            "090000_60",
            &[("mic_transcript.md", b"uncached")],
        );
        root
    }

    fn app(root: &Path) -> axum::Router {
        router(
            root.to_path_buf(),
            Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap()),
            shell,
        )
    }

    async fn response(
        app: axum::Router,
        path: &str,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let response = app
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, headers, body)
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u64, SystemTime, String)> {
        fn visit(
            root: &Path,
            path: &Path,
            output: &mut BTreeMap<PathBuf, (u64, SystemTime, String)>,
        ) {
            for entry in fs::read_dir(path).expect("read") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    visit(root, &path, output);
                    continue;
                }
                let metadata = fs::metadata(&path).expect("metadata");
                output.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    (
                        metadata.len(),
                        metadata.modified().unwrap(),
                        format!("{:x}", Sha256::digest(fs::read(&path).unwrap())),
                    ),
                );
            }
        }
        let mut output = BTreeMap::new();
        visit(root, root, &mut output);
        output
    }

    #[tokio::test]
    async fn corpus_has_the_thirteen_scoped_transcripts_cases_and_gets_are_read_only() {
        let corpus: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/convey_records_corpus.json"
        )))
        .unwrap();
        let populated = corpus["phases"]["populated"]["transcripts"]
            .as_array()
            .unwrap();
        let selected = populated
            .iter()
            .filter(|case| {
                matches!(
                    case["path"].as_str(),
                    Some(
                        "/app/transcripts/"
                            | "/app/transcripts/20260731"
                            | "/app/transcripts/notaday"
                            | "/app/transcripts/api/index"
                            | "/app/transcripts/api/ranges/20260731"
                            | "/app/transcripts/api/segments/20260731"
                            | "/app/transcripts/api/day/20260731"
                            | "/app/transcripts/api/stats/202607"
                            | "/app/transcripts/api/stats/202608"
                            | "/app/transcripts/api/stats/202609"
                            | "/app/transcripts/api/stats/nope"
                    )
                )
            })
            .count();
        assert_eq!(selected, 11);
        assert_eq!(
            corpus["phases"]["unestablished"]["transcripts"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            corpus["phases"]["corrupt"]["transcripts"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let root = root();
        let before = snapshot(root.path());
        let app = app(root.path());
        for path in [
            "/app/transcripts/",
            "/app/transcripts/20260731",
            "/app/transcripts/notaday",
            "/app/transcripts/api/index",
            "/app/transcripts/api/ranges/20260731",
            "/app/transcripts/api/segments/20260731",
            "/app/transcripts/api/day/20260731",
            "/app/transcripts/api/stats/202607",
            "/app/transcripts/api/stats/202608",
            "/app/transcripts/api/stats/202609",
            "/app/transcripts/api/stats/nope",
        ] {
            let _ = response(app.clone(), path).await;
        }
        assert_eq!(snapshot(root.path()), before);
    }

    #[tokio::test]
    async fn corpus_replay_checks_all_thirteen_scoped_statuses() {
        let corpus: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/convey_records_corpus.json"
        )))
        .unwrap();
        let selected = |phase: &str| {
            corpus["phases"][phase]["transcripts"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|case| {
                    phase != "populated"
                        || matches!(
                            case["path"].as_str(),
                            Some(
                                "/app/transcripts/"
                                    | "/app/transcripts/20260731"
                                    | "/app/transcripts/notaday"
                                    | "/app/transcripts/api/index"
                                    | "/app/transcripts/api/ranges/20260731"
                                    | "/app/transcripts/api/segments/20260731"
                                    | "/app/transcripts/api/day/20260731"
                                    | "/app/transcripts/api/stats/202607"
                                    | "/app/transcripts/api/stats/202608"
                                    | "/app/transcripts/api/stats/202609"
                                    | "/app/transcripts/api/stats/nope"
                            )
                        )
                })
                .collect::<Vec<_>>()
        };
        let populated = root();
        let router = app(populated.path());
        let mut total = 0;
        for case in selected("populated") {
            total += 1;
            assert_eq!(
                response(router.clone(), case["path"].as_str().unwrap())
                    .await
                    .0
                    .as_u16(),
                case["status"].as_u64().unwrap() as u16,
                "{}",
                case["path"]
            );
        }
        let unestablished = TempDir::new().unwrap();
        for case in selected("unestablished") {
            total += 1;
            assert_eq!(
                response(
                    solstone_core_convey_shell::router(unestablished.path().to_path_buf()),
                    case["path"].as_str().unwrap()
                )
                .await
                .0
                .as_u16(),
                case["status"].as_u64().unwrap() as u16
            );
        }
        let corrupt = TempDir::new().unwrap();
        write(corrupt.path(), "config/journal.json", b"{");
        for case in selected("corrupt") {
            total += 1;
            assert_eq!(
                response(
                    solstone_core_convey_shell::router(corrupt.path().to_path_buf()),
                    case["path"].as_str().unwrap()
                )
                .await
                .0
                .as_u16(),
                case["status"].as_u64().unwrap() as u16
            );
        }
        assert_eq!(total, 13);
    }

    #[tokio::test]
    async fn corpus_replay_matches_recorded_json_for_each_migrated_api_case() {
        let output = std::process::Command::new("python")
            .args([
                "-c",
                "from scripts.records_corpus_seed import build_populated_journal; print(build_populated_journal('20261001')[0])",
            ])
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.."))
            .output()
            .expect("corpus seeder starts");
        assert!(
            output.status.success(),
            "corpus seeder: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let root = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
        let corpus: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/convey_records_corpus.json"
        )))
        .unwrap();
        let router = app(&root);
        let cases = corpus["phases"]["populated"]["transcripts"]
            .as_array()
            .unwrap();
        for case in cases.iter().filter(|case| {
            matches!(
                case["path"].as_str(),
                Some(
                    "/app/transcripts/api/index"
                        | "/app/transcripts/api/ranges/20260731"
                        | "/app/transcripts/api/segments/20260731"
                        | "/app/transcripts/api/day/20260731"
                        | "/app/transcripts/api/stats/202607"
                        | "/app/transcripts/api/stats/202608"
                        | "/app/transcripts/api/stats/202609"
                        | "/app/transcripts/api/stats/nope"
                )
            )
        }) {
            let (status, _, body) = response(router.clone(), case["path"].as_str().unwrap()).await;
            assert_eq!(
                status.as_u16(),
                case["status"].as_u64().unwrap() as u16,
                "{}",
                case["path"]
            );
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                case["json"],
                "{}",
                case["path"]
            );
        }
        fs::remove_dir_all(root).expect("seeded corpus cleanup");
    }

    #[tokio::test]
    async fn corpus_replay_matches_headers_and_bodies_for_shell_and_refusals() {
        let output = std::process::Command::new("python").args(["-c", "from scripts.records_corpus_seed import build_populated_journal; print(build_populated_journal('20261001')[0])"]).current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../..")).output().unwrap();
        assert!(output.status.success());
        let root = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
        let corpus: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/convey_records_corpus.json"
        )))
        .unwrap();
        let router = solstone_core_convey_shell::router(root.clone());
        for case in corpus["phases"]["populated"]["transcripts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| {
                matches!(
                    case["path"].as_str(),
                    Some(
                        "/app/transcripts/"
                            | "/app/transcripts/20260731"
                            | "/app/transcripts/notaday"
                            | "/app/transcripts/api/stats/nope"
                    )
                )
            })
        {
            let (status, headers, body) =
                response(router.clone(), case["path"].as_str().unwrap()).await;
            assert_eq!(status.as_u16(), case["status"].as_u64().unwrap() as u16);
            for (name, expected) in case["response_headers"].as_object().unwrap() {
                assert_eq!(
                    headers[name],
                    expected.as_str().unwrap(),
                    "{} {name}",
                    case["path"]
                );
            }
            if let Some(expected) = case.get("json") {
                assert_eq!(
                    serde_json::from_slice::<Value>(&body).unwrap(),
                    *expected,
                    "{}",
                    case["path"]
                );
            } else {
                assert_eq!(
                    format!("{:x}", Sha256::digest(&body)),
                    case["body_sha256"].as_str().unwrap(),
                    "{}",
                    case["path"]
                );
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn corpus_replay_matches_session_gate_bodies_in_both_nonready_phases() {
        let corpus: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/convey_records_corpus.json"
        )))
        .unwrap();
        for (phase, root) in [
            ("unestablished", TempDir::new().unwrap()),
            ("corrupt", TempDir::new().unwrap()),
        ] {
            if phase == "corrupt" {
                write(root.path(), "config/journal.json", b"{");
            }
            let case = &corpus["phases"][phase]["transcripts"][0];
            let (status, headers, body) = response(
                solstone_core_convey_shell::router(root.path().to_path_buf()),
                case["path"].as_str().unwrap(),
            )
            .await;
            assert_eq!(status.as_u16(), case["status"].as_u64().unwrap() as u16);
            for (name, expected) in case["response_headers"].as_object().unwrap() {
                if name == "Content-Length" {
                    assert_eq!(
                        headers[name],
                        body.len().to_string(),
                        "{phase} {name} reflects the live response body"
                    );
                    continue;
                }
                assert_eq!(headers[name], expected.as_str().unwrap(), "{phase} {name}");
            }
            let normalized = String::from_utf8(body)
                .unwrap()
                .replace(&root.path().display().to_string(), "<JOURNAL_ROOT>");
            assert_eq!(
                format!("{:x}", Sha256::digest(normalized.as_bytes())),
                case["body_sha256"].as_str().unwrap(),
                "{phase}"
            );
        }
    }

    #[tokio::test]
    async fn normalized_imports_and_range_state_are_modality_scoped() {
        let root = root();
        let (_, _, body) =
            response(app(root.path()), "/app/transcripts/api/segments/20260731").await;
        let segments = serde_json::from_slice::<Value>(&body).unwrap()["segments"]
            .as_array()
            .unwrap()
            .clone();
        for key in ["131000_60", "132000_60"] {
            assert_eq!(
                segments
                    .iter()
                    .find(|segment| segment["key"] == key)
                    .unwrap()["types"],
                json!(["markdown"])
            );
        }
        let segment = TranscriptSegment {
            key: "090000_60".into(),
            stream: "field".into(),
            start: "09:00".into(),
            end: "09:15".into(),
            types: vec!["audio".into()],
            data_state: BTreeMap::from([
                ("audio".into(), "pending".into()),
                ("screen".into(), "analyzed".into()),
            ]),
            think: None,
        };
        let value = serde_json::to_value(attach_visible_streams_to_ranges(
            &[("09:00".into(), "09:15".into())],
            &[segment],
            "audio",
        ))
        .unwrap();
        assert_eq!(value[0]["state"], "pending");
    }

    #[tokio::test]
    async fn live_endpoints_have_exact_shapes_and_combined_payload_matches() {
        let root = root();
        let app = app(root.path());
        let (_, _, ranges) = response(app.clone(), "/app/transcripts/api/ranges/20260731").await;
        let (_, _, segments) =
            response(app.clone(), "/app/transcripts/api/segments/20260731").await;
        let (_, _, day) = response(app, "/app/transcripts/api/day/20260731").await;
        let ranges: Value = serde_json::from_slice(&ranges).unwrap();
        let segments: Value = serde_json::from_slice(&segments).unwrap();
        let day: Value = serde_json::from_slice(&day).unwrap();
        assert_eq!(
            ranges
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["audio".into(), "screen".into()])
        );
        assert_eq!(
            segments
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["segments".into()])
        );
        assert_eq!(
            day.as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["audio".into(), "screen".into(), "segments".into()])
        );
        assert_eq!(day["audio"], ranges["audio"]);
        assert_eq!(day["screen"], ranges["screen"]);
        assert_eq!(day["segments"], segments["segments"]);
    }

    #[tokio::test]
    async fn stats_cache_wins_and_month_validation_is_exact() {
        let root = root();
        let cache = json!({"schema_version": 8, "stats": {"transcript_sessions":0,"transcript_segments":0,"transcript_duration":0.0,"transcript_ranges":40,"percept_sessions":0,"percept_frames":0,"percept_duration":0.0,"percept_ranges":2,"browser_segments":1,"pending_segments":0,"segments_pending_think":0,"outputs_processed":0,"outputs_pending":0,"day_bytes":0}, "agent_data":{},"facet_data":{},"heatmap_data":{"weekday":0,"hours":{}}});
        let path = root.path().join("chronicle/20260731/stats.json");
        fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
        fs::File::open(&path)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(60))
            .unwrap();
        let app = app(root.path());
        let (_, _, cached) = response(app.clone(), "/app/transcripts/api/stats/202607").await;
        let (_, _, uncached) = response(app.clone(), "/app/transcripts/api/stats/202608").await;
        assert_eq!(serde_json::from_slice::<Value>(&cached).unwrap()[DAY], 43);
        assert_eq!(
            serde_json::from_slice::<Value>(&uncached).unwrap()["20260801"],
            1
        );
        let (_, _, index) = response(app.clone(), "/app/transcripts/api/index").await;
        let index: Value = serde_json::from_slice(&index).unwrap();
        assert_eq!(index["months"]["202607"], 43);
        let (status, _, impossible) =
            response(app.clone(), "/app/transcripts/api/stats/999999").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(impossible, b"{}");
        let (status, _, invalid) = response(app, "/app/transcripts/api/stats/nope").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            serde_json::from_slice::<Value>(&invalid).unwrap()["reason_code"],
            "invalid_month"
        );

        let bounded = root.path().join("chronicle/20260731");
        let before = bounded_input_mtime(&bounded).unwrap();
        let unrelated = bounded.join("unrelated.txt");
        fs::write(&unrelated, b"not a cache input").unwrap();
        fs::File::open(&unrelated)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(120))
            .unwrap();
        assert_eq!(bounded_input_mtime(&bounded).unwrap(), before);
    }

    #[tokio::test]
    async fn shell_routes_and_partial_conversion_match_the_contract() {
        let root = root();
        let app = app(root.path());
        let (status, headers, _) = response(app.clone(), "/app/transcripts/workspace").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(
            response(app.clone(), "/app/transcripts/20260731").await.0,
            StatusCode::OK
        );
        let (status, _, body) = response(app.clone(), "/app/transcripts/notaday").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"error":"I couldn't use that day.","reason_code":"invalid_day","detail":"Day not found"})
        );
        let full = solstone_core_convey_shell::router(root.path().to_path_buf());
        for path in [
            "/app/transcripts/api/segment/x",
            "/app/transcripts/api/read/x",
            "/app/transcripts/api/serve_file/x",
        ] {
            let (status, _, body) = response(full.clone(), path).await;
            assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap()["reason_code"],
                "app_not_converted"
            );
        }
    }

    #[tokio::test]
    async fn session_gate_covers_all_three_config_phases_on_index() {
        let unestablished = TempDir::new().unwrap();
        let (status, headers, _) = response(
            solstone_core_convey_shell::router(unestablished.path().to_path_buf()),
            "/app/transcripts/api/index",
        )
        .await;
        assert_eq!(status, StatusCode::FOUND);
        assert_eq!(headers[header::LOCATION], "/init");

        let established = root();
        assert_eq!(
            response(
                solstone_core_convey_shell::router(established.path().to_path_buf()),
                "/app/transcripts/api/index",
            )
            .await
            .0,
            StatusCode::OK
        );

        let corrupt = TempDir::new().unwrap();
        write(corrupt.path(), "config/journal.json", b"{");
        assert_eq!(
            response(
                solstone_core_convey_shell::router(corrupt.path().to_path_buf()),
                "/app/transcripts/api/index",
            )
            .await
            .0,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn redirect_uses_segment_presence_and_short_circuits_newest_first() {
        let root = root();
        segment(
            root.path(),
            "20260802",
            "import.notes",
            "090000_60",
            &[("imported.md", b"newest markdown")],
        );
        let (status, headers, _) = response(app(root.path()), "/app/transcripts/").await;
        assert_eq!(status, StatusCode::FOUND);
        assert_eq!(headers[header::LOCATION], "/app/transcripts/20260802");
    }

    #[test]
    fn redirect_target_and_workspace_asset_are_deterministic() {
        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        assert_eq!(
            redirect_target(
                &BTreeMap::from([("20260802".into(), 0), ("20260801".into(), 2)]),
                now
            ),
            "20260801"
        );
        assert_eq!(redirect_target(&BTreeMap::new(), now), "20260801");
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/transcripts/workspace.html"
        ));
        let ground = std::process::Command::new("git")
            .args([
                "show",
                "f17280333736016c219d3f6a4b3a263763529833:solstone/apps/transcripts/workspace.html",
            ])
            .output()
            .unwrap();
        assert!(ground.status.success());
        assert_eq!(source, ground.stdout.as_slice());
    }
}
