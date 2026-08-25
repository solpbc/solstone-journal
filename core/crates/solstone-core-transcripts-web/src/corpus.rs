// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::panic;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, UNIX_EPOCH};

    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode, header};
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

    /// The recorded cases carry owner-LOCAL wall-clock strings and timestamps, because the
    /// reference renders them that way and this port is faithful to it. The corpus was
    /// captured under UTC and records that in `capture_environment.tz`.
    ///
    /// So the replay must ESTABLISH the zone the recorded case depends on, rather than
    /// inheriting whichever zone the host happens to sit in. Without this the suite is green
    /// on a UTC machine and red on a developer's: measured on an America/Denver host, the
    /// segment bodies came back six hours off, which also reordered the chunks and made the
    /// diff look like a content defect rather than a harness one.
    ///
    /// The fix is to pin the condition, never to relax the expectation and never to declare a
    /// UTC shell the contract.
    /// The zone the corpus was captured under, recorded in the fixture itself.
    ///
    /// The recorded cases carry owner-LOCAL wall-clock strings and timestamps, because the
    /// reference renders them that way and this port is faithful to it. So the replay must
    /// ESTABLISH the zone its recorded cases depend on rather than inheriting the host's.
    /// Measured on an America/Denver host, the segment bodies came back six hours off, which
    /// also reordered the chunks and made a harness defect look like a content defect.
    ///
    /// Pin the condition; never relax the expectation, and never declare a UTC shell the
    /// contract.
    fn capture_timezone() -> &'static str {
        let corpus: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/convey_records_corpus.json"
        ));
        let value: Value = serde_json::from_str(corpus).expect("corpus parses");
        match value["capture_environment"]["tz"].as_str() {
            Some("UTC") | None => "UTC",
            Some(other) => panic!(
                "corpus was captured under {other}; this replay only pins UTC. Teach it the new \
                 zone rather than letting the host decide."
            ),
        }
    }

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
        request(app, Method::GET, path, &BTreeMap::new()).await
    }

    async fn request(
        app: axum::Router,
        method: Method,
        path: &str,
        request_headers: &BTreeMap<String, String>,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        request_body(app, method, path, request_headers, Body::empty()).await
    }

    async fn request_body(
        app: axum::Router,
        method: Method,
        path: &str,
        request_headers: &BTreeMap<String, String>,
        body: Body,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut builder = Request::builder().method(method).uri(path);
        for (name, value) in request_headers {
            builder = builder.header(name, value);
        }
        let response = app.oneshot(builder.body(body).unwrap()).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, headers, body)
    }

    fn captured_fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/convey_records_journal")
    }

    fn captured_fixture_manifest() -> &'static str {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/convey_records_journal.manifest"
        ))
    }

    fn fixture_inventory() -> (BTreeMap<PathBuf, String>, BTreeSet<PathBuf>) {
        let mut files = BTreeMap::new();
        let mut empty_directories = BTreeSet::new();
        for (line_number, line) in captured_fixture_manifest().lines().enumerate() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut fields = line.splitn(3, ' ');
            let kind = fields.next().expect("manifest entry kind");
            let first = fields.next().expect("manifest entry value");
            let (digest, relative) = match kind {
                "file" => (
                    Some(first),
                    fields.next().expect("file manifest entry path"),
                ),
                "empty" => (None, first),
                other => panic!("unsupported fixture manifest entry {other} on line {line_number}"),
            };
            let relative = PathBuf::from(relative);
            assert!(
                !relative.is_absolute()
                    && relative
                        .components()
                        .all(|component| matches!(component, std::path::Component::Normal(_))),
                "fixture manifest path must stay relative: {}",
                relative.display()
            );
            if let Some(digest) = digest {
                assert!(
                    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
                    "fixture digest must be lowercase SHA-256: {digest}"
                );
                assert_eq!(digest, digest.to_ascii_lowercase());
                assert!(
                    files.insert(relative.clone(), digest.to_owned()).is_none(),
                    "duplicate fixture file: {}",
                    relative.display()
                );
            } else {
                assert!(
                    empty_directories.insert(relative.clone()),
                    "duplicate empty fixture directory: {}",
                    relative.display()
                );
            }
        }
        assert!(!files.is_empty(), "fixture manifest must name files");
        assert!(
            files.keys().all(|path| !empty_directories
                .iter()
                .any(|directory| path.starts_with(directory))),
            "empty fixture directories must contain no files"
        );
        (files, empty_directories)
    }

    fn collect_fixture_files(root: &Path, current: &Path, files: &mut BTreeSet<PathBuf>) {
        let mut entries = fs::read_dir(current)
            .unwrap_or_else(|error| panic!("fixture directory {}: {error}", current.display()))
            .collect::<Result<Vec<_>, _>>()
            .expect("fixture entries read");
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type().expect("fixture entry type");
            assert!(
                !file_type.is_symlink(),
                "captured fixture must not contain a symlink: {}",
                path.display()
            );
            if file_type.is_dir() {
                collect_fixture_files(root, &path, files);
            } else {
                assert!(
                    file_type.is_file(),
                    "captured fixture contains an unsupported entry: {}",
                    path.display()
                );
                files.insert(
                    path.strip_prefix(root)
                        .expect("fixture-relative path")
                        .into(),
                );
            }
        }
    }

    fn materialize_captured_fixture(source: &Path) -> PathBuf {
        let (expected_files, empty_directories) = fixture_inventory();
        let mut actual_files = BTreeSet::new();
        collect_fixture_files(source, source, &mut actual_files);
        let expected_paths = expected_files.keys().cloned().collect::<BTreeSet<_>>();
        assert_eq!(
            actual_files, expected_paths,
            "captured fixture file inventory drifted"
        );

        for (relative, expected_digest) in &expected_files {
            let bytes = fs::read(source.join(relative)).unwrap_or_else(|error| {
                panic!("captured fixture file {}: {error}", relative.display())
            });
            assert_eq!(
                format!("{:x}", Sha256::digest(&bytes)),
                *expected_digest,
                "captured fixture digest drifted: {}",
                relative.display()
            );
        }

        let root = TempDir::new().expect("captured journal root");
        for relative in &empty_directories {
            fs::create_dir_all(root.path().join(relative)).expect("empty fixture directory");
        }
        for relative in expected_files.keys() {
            let target = root.path().join(relative);
            fs::create_dir_all(target.parent().expect("fixture file parent"))
                .expect("fixture file parent directory");
            fs::copy(source.join(relative), &target).expect("captured fixture copy");
        }
        fs::File::open(root.path().join("chronicle/20260715/stats.json"))
            .expect("fresh stats cache")
            .set_modified(UNIX_EPOCH + Duration::from_secs(4_102_444_800))
            .expect("fresh stats-cache mtime");
        root.keep()
    }

    fn seeded_root() -> PathBuf {
        let root = materialize_captured_fixture(&captured_fixture_root());
        // The frozen capture predates durable entity types, but its active
        // speaker labels model these two identities as people.
        for entity_id in ["owner", "other"] {
            let path = root.join("entities").join(entity_id).join("entity.json");
            let mut entity: Value =
                serde_json::from_slice(&fs::read(&path).expect("entity")).expect("entity json");
            entity
                .as_object_mut()
                .expect("entity object")
                .insert("type".to_owned(), json!("Person"));
            fs::write(path, serde_json::to_vec(&entity).expect("entity json"))
                .expect("entity writes");
        }
        root
    }

    fn assert_malformed_fixture_rejected(root: &Path, label: &str) {
        let result = panic::catch_unwind(|| materialize_captured_fixture(root));
        assert!(result.is_err(), "{label} must fail closed");
    }

    #[test]
    fn captured_fixture_inventory_rejects_missing_extra_and_changed_files_before_replay() {
        let missing = seeded_root();
        fs::remove_file(missing.join("chronicle/20260731/notes/130000_60/note.md"))
            .expect("remove required captured file");
        assert_malformed_fixture_rejected(&missing, "missing captured file");
        fs::remove_dir_all(missing).expect("missing fixture cleanup");

        let extra = seeded_root();
        fs::write(extra.join("ambient-extra"), b"not captured").expect("write extra fixture file");
        assert_malformed_fixture_rejected(&extra, "extra captured file");
        fs::remove_dir_all(extra).expect("extra fixture cleanup");

        let changed = seeded_root();
        fs::write(
            changed.join("chronicle/20260731/notes/130000_60/note.md"),
            b"changed bytes",
        )
        .expect("change captured file");
        assert_malformed_fixture_rejected(&changed, "changed captured file");
        fs::remove_dir_all(changed).expect("changed fixture cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn captured_fixture_inventory_rejects_symlinks_before_replay() {
        use std::os::unix::fs::symlink;

        let linked = seeded_root();
        symlink("config/journal.json", linked.join("ambient-link")).expect("create fixture link");
        assert_malformed_fixture_rejected(&linked, "captured fixture symlink");
        fs::remove_dir_all(linked).expect("linked fixture cleanup");
    }

    #[test]
    fn captured_fixture_materialization_restores_non_content_state() {
        let root = seeded_root();
        let empty = root.join("chronicle/20260915");
        assert!(empty.is_dir(), "intentional empty day must exist");
        assert_eq!(fs::read_dir(&empty).expect("empty day reads").count(), 0);
        assert_eq!(
            fs::metadata(root.join("chronicle/20260715/stats.json"))
                .expect("fresh stats cache metadata")
                .modified()
                .expect("fresh stats cache mtime"),
            UNIX_EPOCH + Duration::from_secs(4_102_444_800)
        );
        assert!(
            !root.join("convey_records_journal.manifest").exists(),
            "source manifest must stay outside the materialized journal"
        );
        fs::remove_dir_all(root).expect("materialized fixture cleanup");
    }

    fn native_read_route_case(case: &Value) -> bool {
        case["method"] == "GET"
            && matches!(
                case["path"].as_str(),
                Some(path)
                    if path.contains("/api/read/")
                        || path.contains("/api/segment/")
                        || path.contains("/api/serve_file/")
            )
    }

    fn normalize_native_json(value: &mut Value, root: &Path) {
        match value {
            Value::Array(values) => {
                for value in values {
                    normalize_native_json(value, root);
                }
            }
            Value::Object(values) => {
                if let Some(Value::Array(details)) = values.get_mut("warning_details") {
                    for detail in details {
                        if let Some(detail) = detail.as_object_mut() {
                            detail.insert("ts".into(), Value::String("<TODAY_TIMESTAMP>".into()));
                        }
                    }
                }
                for value in values.values_mut() {
                    normalize_native_json(value, root);
                }
            }
            Value::String(text) => {
                *text = text.replace(&root.display().to_string(), "<JOURNAL_ROOT>");
            }
            _ => {}
        }
    }

    async fn assert_native_read_route_case(router: axum::Router, root: &Path, case: &Value) {
        let request_headers = case["request_headers"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, value)| (name.clone(), value.as_str().unwrap().to_owned()))
            .collect();
        let (status, headers, body) = request(
            router,
            Method::GET,
            case["path"].as_str().unwrap(),
            &request_headers,
        )
        .await;
        if case["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("/mic_audio.xyz"))
        {
            assert_eq!(status, StatusCode::BAD_REQUEST, "{}", case["path"]);
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap()["reason_code"],
                "invalid_request_value",
                "{}",
                case["path"]
            );
            return;
        }
        assert_eq!(
            status.as_u16(),
            case["status"].as_u64().unwrap() as u16,
            "{}",
            case["path"]
        );
        if let Some(expected) = case.get("json") {
            let mut actual = serde_json::from_slice::<Value>(&body).unwrap();
            normalize_native_json(&mut actual, root);
            if case["path"]
                .as_str()
                .is_some_and(|path| path.contains("/api/segment/"))
                && status == StatusCode::OK
            {
                assert_eq!(
                    actual
                        .as_object()
                        .unwrap()
                        .keys()
                        .cloned()
                        .collect::<BTreeSet<_>>(),
                    BTreeSet::from([
                        "audio_file".into(),
                        "chunks".into(),
                        "cost".into(),
                        "data_state".into(),
                        "duration".into(),
                        "image_files".into(),
                        "md_files".into(),
                        "media_purged".into(),
                        "media_sizes".into(),
                        "segment_key".into(),
                        "signals".into(),
                        "speaker_labels".into(),
                        "transcripts_copy".into(),
                        "video_files".into(),
                        "warning_details".into(),
                        "warnings".into(),
                    ]),
                    "{}",
                    case["path"]
                );
            }
            assert_eq!(actual, *expected, "{}", case["path"]);
            return;
        }
        for (name, expected) in case["response_headers"].as_object().unwrap() {
            assert_eq!(
                headers[name],
                expected.as_str().unwrap(),
                "{} {name}",
                case["path"]
            );
        }
        assert_eq!(
            format!("{:x}", Sha256::digest(&body)),
            case["body_sha256"].as_str().unwrap(),
            "{}",
            case["path"]
        );
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u64, String)> {
        fn visit(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, (u64, String)>) {
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
                        format!("{:x}", Sha256::digest(fs::read(&path).unwrap())),
                    ),
                );
            }
        }
        let mut output = BTreeMap::new();
        visit(root, root, &mut output);
        output
    }

    fn deletion_root() -> TempDir {
        let root = TempDir::new().expect("journal");
        config(root.path());
        segment(
            root.path(),
            DAY,
            "field",
            "090000_300",
            &[
                ("audio.flac", b"raw"),
                ("audio.jsonl", b"{}\n"),
                ("stream.json", b"{}"),
                ("talents/sense.json", b"{}"),
            ],
        );
        root
    }

    fn delete_app(root: &Path, window: Duration) -> axum::Router {
        crate::router_with_delete_window(
            root.to_path_buf(),
            Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap()),
            shell,
            window,
        )
    }

    fn action_rows(root: &Path) -> Vec<Value> {
        let path = root.join("config/actions");
        let mut rows = Vec::new();
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let text = fs::read_to_string(entry.path()).expect("action log");
                rows.extend(
                    text.lines()
                        .map(|line| serde_json::from_str(line).expect("action row")),
                );
            }
        }
        rows
    }

    fn assert_only_write_route_paths_changed(
        before: &BTreeMap<PathBuf, (u64, String)>,
        after: &BTreeMap<PathBuf, (u64, String)>,
    ) {
        for path in before.keys().chain(after.keys()) {
            if before.get(path) != after.get(path) {
                assert!(
                    path.starts_with("chronicle/20260731/field/090000_300")
                        || path == Path::new("chronicle/20260731/field/090000_300.lock")
                        || path == Path::new("chronicle/20260731/health/stream.updated")
                        || path == Path::new("chronicle/20260731/health/stream.updated.lock")
                        || path.starts_with("config/actions"),
                    "unexpected journal mutation: {}",
                    path.display()
                );
            }
        }
    }

    async fn delete_request(app: axum::Router) -> Value {
        let (status, _, body) = request(
            app,
            Method::DELETE,
            "/app/transcripts/api/segment/20260731/field/090000_300",
            &BTreeMap::new(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        serde_json::from_slice(&body).expect("delete response")
    }

    #[tokio::test]
    async fn deferred_delete_commits_to_a_tombstone_and_logs_pending_then_committed() {
        let root = deletion_root();
        let before = snapshot(root.path());
        let app = delete_app(root.path(), Duration::ZERO);
        let response = delete_request(app.clone()).await;
        assert_eq!(response["ttl_seconds"], 10);
        assert_eq!(response["success"], true);
        let segment = root.path().join("chronicle/20260731/field/090000_300");
        let tombstone = segment.join("tombstone.json");
        assert!(
            tombstone.is_file(),
            "zero-delay deferred delete must commit before the delete response returns"
        );
        let names = fs::read_dir(&segment)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["tombstone.json"]);
        assert!(
            root.path()
                .join("chronicle/20260731/health/stream.updated")
                .is_file(),
            "a committed deletion must dirty its day"
        );
        assert_only_write_route_paths_changed(&before, &snapshot(root.path()));
        let rows = action_rows(root.path());
        let phases = rows
            .iter()
            .map(|row| row["params"]["phase"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(phases.contains(&"pending"));
        assert!(phases.contains(&"committed"));
        let (status, _, _) = request(
            app,
            Method::POST,
            &format!(
                "/app/transcripts/api/cancel-delete/{}",
                response["pending"].as_str().unwrap()
            ),
            &BTreeMap::new(),
        )
        .await;
        assert_eq!(status, StatusCode::GONE);
    }

    #[tokio::test]
    async fn cancellation_preserves_the_segment_and_never_commits() {
        let root = deletion_root();
        let before = snapshot(root.path());
        let app = delete_app(root.path(), Duration::from_secs(60));
        let response = delete_request(app.clone()).await;
        let pending = response["pending"].as_str().unwrap();
        let (status, _, body) = request(
            app,
            Method::POST,
            &format!("/app/transcripts/api/cancel-delete/{pending}"),
            &BTreeMap::new(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"cancelled":pending})
        );
        tokio::task::yield_now().await;
        let after = snapshot(root.path());
        for (path, value) in &before {
            if !path.starts_with("config/actions") {
                assert_eq!(
                    after.get(path),
                    Some(value),
                    "unexpected change: {}",
                    path.display()
                );
            }
        }
        for path in after.keys() {
            assert!(
                before.contains_key(path) || path.starts_with("config/actions"),
                "unexpected new path: {}",
                path.display()
            );
        }
        assert_only_write_route_paths_changed(&before, &after);
        let rows = action_rows(root.path());
        let phases = rows
            .iter()
            .map(|row| row["params"]["phase"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(phases.contains(&"pending"));
        assert!(phases.contains(&"cancelled"));
        assert!(!phases.contains(&"committed"));
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
        let root = seeded_root();
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

    #[test]
    fn corpus_replay_matches_all_new_read_routes_and_is_read_only() {
        // Wrapped rather than #[tokio::test] so the capture zone is established
        // AROUND the whole replay: the recorded bodies carry owner-local times.
        temp_env::with_var("TZ", Some(capture_timezone()), || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(async {
                    let _tz = capture_timezone();
                    let root = seeded_root();
                    let corpus: Value = serde_json::from_str(include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../../fixtures/convey_records_corpus.json"
                    )))
                    .unwrap();
                    let cases = corpus["phases"]["populated"]["transcripts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|case| native_read_route_case(case))
                        .collect::<Vec<_>>();
                    assert_eq!(cases.len(), 42);
                    let before = snapshot(&root);
                    let router = app(&root);
                    for case in cases {
                        assert_native_read_route_case(router.clone(), &root, case).await;
                    }
                    assert_eq!(snapshot(&root), before);
                    fs::remove_dir_all(root).expect("seeded corpus cleanup");
                });
        });
    }

    #[tokio::test]
    async fn warning_timestamps_depend_only_on_the_injected_clock() {
        let root = seeded_root();
        let path = "/app/transcripts/api/segment/20260731/speakers/134000_60";
        let early = router(
            root.clone(),
            Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap()),
            shell,
        );
        let late = router(
            root.clone(),
            Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 3, 0, 0, 0).unwrap()),
            shell,
        );
        let (_, _, early_body) = response(early, path).await;
        let (_, _, late_body) = response(late, path).await;
        let mut early: Value = serde_json::from_slice(&early_body).unwrap();
        let mut late: Value = serde_json::from_slice(&late_body).unwrap();
        let early_times = early["warning_details"]
            .as_array()
            .unwrap()
            .iter()
            .map(|detail| detail["ts"].clone())
            .collect::<Vec<_>>();
        let late_times = late["warning_details"]
            .as_array()
            .unwrap()
            .iter()
            .map(|detail| detail["ts"].clone())
            .collect::<Vec<_>>();
        assert!(!early_times.is_empty());
        assert_ne!(early_times, late_times);
        normalize_native_json(&mut early, &root);
        normalize_native_json(&mut late, &root);
        assert_eq!(
            serde_json::to_vec(&early).unwrap(),
            serde_json::to_vec(&late).unwrap()
        );

        let corpus: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/convey_records_corpus.json"
        )))
        .unwrap();
        let captured_case = corpus["phases"]["populated"]["transcripts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["path"] == path)
            .unwrap();
        assert_native_read_route_case(app(&root), &root, captured_case).await;
        fs::remove_dir_all(root).expect("seeded corpus cleanup");
    }

    #[tokio::test]
    async fn corpus_replay_matches_headers_and_bodies_for_shell_and_refusals() {
        let root = seeded_root();
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
    async fn corpus_replays_all_twelve_transcript_write_refusals() {
        let root = seeded_root();
        let corpus: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/convey_records_corpus.json"
        )))
        .unwrap();
        let cases = corpus["phases"]["populated"]["transcripts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| matches!(case["method"].as_str(), Some("POST" | "DELETE")))
            .collect::<Vec<_>>();
        assert_eq!(cases.len(), 12);
        for case in cases {
            let method = case["method"].as_str().unwrap().parse().unwrap();
            let body = match case["why"].as_str().unwrap() {
                "invalid modality refusal" => Body::from(r#"{"modality":"other"}"#),
                "analyzed refusal" | "purged refusal" => Body::from(r#"{"modality":"audio"}"#),
                _ => Body::empty(),
            };
            let (status, headers, body) = request_body(
                app(&root),
                method,
                case["path"].as_str().unwrap(),
                &BTreeMap::new(),
                body,
            )
            .await;
            assert_eq!(
                status.as_u16(),
                case["status"].as_u64().unwrap() as u16,
                "{}",
                case["why"]
            );
            for (name, expected) in case["response_headers"].as_object().unwrap() {
                assert_eq!(
                    headers[name],
                    expected.as_str().unwrap(),
                    "{} {name}",
                    case["why"]
                );
            }
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                case["json"],
                "{}",
                case["why"]
            );
        }
        fs::remove_dir_all(root).expect("seeded corpus cleanup");
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

    #[test]
    fn segment_ending_at_slot_boundary_does_not_attach_to_next_range() {
        let segment = TranscriptSegment {
            key: "070000_900".into(),
            stream: "field".into(),
            start: "07:00".into(),
            end: "07:15".into(),
            types: vec!["audio".into()],
            data_state: BTreeMap::from([("audio".into(), "pending".into())]),
            think: None,
        };

        let ranges = attach_visible_streams_to_ranges(
            &[
                ("07:00".into(), "07:15".into()),
                ("07:15".into(), "07:30".into()),
            ],
            &[segment],
            "audio",
        );

        assert_eq!(
            serde_json::to_value(ranges).unwrap(),
            json!([{
                "start": "07:00",
                "end": "07:15",
                "streams": ["field"],
                "state": "pending",
                "think": null,
            }])
        );
    }

    #[tokio::test]
    async fn short_audio_is_visible_while_zero_duration_audio_is_not() {
        let root = TempDir::new().expect("journal");
        config(root.path());
        segment(
            root.path(),
            DAY,
            "import.audio",
            "070000_17",
            &[("audio.jsonl", b"{}\n")],
        );
        segment(
            root.path(),
            DAY,
            "import.audio",
            "073000_0",
            &[("audio.jsonl", b"{}\n")],
        );

        let (status, _, body) =
            response(app(root.path()), "/app/transcripts/api/ranges/20260731").await;

        assert_eq!(status, StatusCode::OK);
        let ranges = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(
            ranges["audio"],
            json!([{
                "start": "07:00",
                "end": "07:15",
                "streams": ["import.audio"],
                "state": "pending",
                "think": null,
            }])
        );
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
            .set_modified(UNIX_EPOCH + Duration::from_secs(4_102_444_800))
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
            .set_modified(UNIX_EPOCH + Duration::from_secs(4_102_444_860))
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
            "/app/transcripts/api/segment/notaday/field/090000_300",
            "/app/transcripts/api/read/notaday",
            "/app/transcripts/api/serve_file/notaday/field/mic_audio.flac",
        ] {
            let (status, _, body) = response(full.clone(), path).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap()["reason_code"],
                "invalid_day"
            );
        }
        let response = full
            .oneshot(
                Request::post("/app/transcripts/api/segment/20260731/field/090000_300/reprocess")
                    .body(Body::from(r#"{"modality":"audio"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["reason_code"],
            "raw_media_not_available"
        );
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
    fn redirect_target_prefers_populated_days_then_clock_day() {
        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        assert_eq!(
            redirect_target(
                &BTreeMap::from([("20260802".into(), 0), ("20260801".into(), 2)]),
                now
            ),
            "20260801"
        );
        assert_eq!(redirect_target(&BTreeMap::new(), now), "20260801");
    }
}
