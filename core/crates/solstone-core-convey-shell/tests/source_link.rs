// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tempfile::tempdir;
use tower::ServiceExt;

use solstone_core_convey_shell::router;
use solstone_core_convey_shell::source_link::{FilesystemReads, SourceReads, source_link_router};
use solstone_core_facets::{FacetIdResolveError, RetiredFacets};
use solstone_core_journal_io::cortex_use::talent_directory_name;
use solstone_core_transcripts_web::DaySegmentRef;

fn write_file(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().expect("parent exists")).expect("parent created");
    fs::write(path, content).expect("file written");
}

fn write_json(path: &Path, value: &Value) {
    write_file(
        path,
        &format!("{}\n", serde_json::to_string(value).unwrap()),
    );
}

fn create_segment(journal: &Path, day: &str, stream: &str, key: &str) {
    let path = if stream == "_default" {
        journal
            .join("chronicle")
            .join(day)
            .join(key)
            .join("audio.jsonl")
    } else {
        journal
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(key)
            .join("audio.jsonl")
    };
    write_file(&path, "{\"start\":0.0}\n");
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

async fn get_source(app: axum::Router, ref_url: &str) -> (StatusCode, String, Option<String>) {
    let req = Request::builder()
        .uri(format!("/source?ref={}", percent_encode(ref_url)))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let location = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|h| h.to_str().ok())
        .map(str::to_owned);
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let body = String::from_utf8_lossy(&bytes).to_string();
    (status, body, location)
}

struct PanicReads;

impl SourceReads for PanicReads {
    fn path_exists(&self, _path: &Path) -> Result<bool, String> {
        panic!("PanicReads::path_exists called");
    }
    fn read_day_segments(
        &self,
        _journal: &Path,
        _day: &str,
        _now: DateTime<Utc>,
    ) -> Result<Vec<DaySegmentRef>, String> {
        panic!("PanicReads::read_day_segments called");
    }
    fn is_facet_dir(&self, _journal: &Path, _facet: &str) -> Result<bool, String> {
        panic!("PanicReads::is_facet_dir called");
    }
    fn read_retired_facets(&self, _journal: &Path) -> Result<RetiredFacets, String> {
        panic!("PanicReads::read_retired_facets called");
    }
    fn resolve_facet_id(&self, _journal: &Path, _id: &str) -> Result<String, FacetIdResolveError> {
        panic!("PanicReads::resolve_facet_id called");
    }
    fn read_news_file(
        &self,
        _journal: &Path,
        _facet: &str,
        _file: &str,
    ) -> Result<Option<String>, String> {
        panic!("PanicReads::read_news_file called");
    }
    fn read_file_text(&self, _path: &Path) -> Result<String, String> {
        panic!("PanicReads::read_file_text called");
    }
    fn read_run_record(&self, _path: &Path) -> Result<Option<String>, String> {
        panic!("PanicReads::read_run_record called");
    }
}

struct RecordReadErrorReads {
    inner: FilesystemReads,
}

impl SourceReads for RecordReadErrorReads {
    fn path_exists(&self, path: &Path) -> Result<bool, String> {
        self.inner.path_exists(path)
    }
    fn read_day_segments(
        &self,
        journal: &Path,
        day: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<DaySegmentRef>, String> {
        self.inner.read_day_segments(journal, day, now)
    }
    fn is_facet_dir(&self, journal: &Path, facet: &str) -> Result<bool, String> {
        self.inner.is_facet_dir(journal, facet)
    }
    fn read_retired_facets(&self, journal: &Path) -> Result<RetiredFacets, String> {
        self.inner.read_retired_facets(journal)
    }
    fn resolve_facet_id(&self, journal: &Path, id: &str) -> Result<String, FacetIdResolveError> {
        self.inner.resolve_facet_id(journal, id)
    }
    fn read_news_file(
        &self,
        journal: &Path,
        facet: &str,
        file: &str,
    ) -> Result<Option<String>, String> {
        self.inner.read_news_file(journal, facet, file)
    }
    fn read_file_text(&self, path: &Path) -> Result<String, String> {
        self.inner.read_file_text(path)
    }
    fn read_run_record(&self, _path: &Path) -> Result<Option<String>, String> {
        Err("injected record read error".to_owned())
    }
}

fn extract_run_relative(ref_url: &str) -> Option<(String, String)> {
    let without_scheme = ref_url.strip_prefix("sol://")?;
    let (path_part, _) = without_scheme
        .split_once('#')
        .unwrap_or((without_scheme, ""));
    let parts: Vec<&str> = path_part.split('/').collect();
    if parts.first() == Some(&"chronicle") && parts.len() >= 3 {
        let day = parts[1].to_owned();
        let rel = parts[2..].join("/");
        return Some((day, rel));
    }
    if parts.len() >= 2 {
        let day = parts[0].to_owned();
        let rel = parts[1..].join("/");
        return Some((day, rel));
    }
    None
}

#[tokio::test]
async fn source_link_corpus_walker() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let corpus_text = fs::read_to_string(manifest_dir.join("tests/source_link_corpus.json"))
        .expect("read corpus json");
    let corpus: Value = serde_json::from_str(&corpus_text).expect("parse corpus json");

    let known_categories: BTreeSet<&str> = [
        "render_cases",
        "sanitizer_cases",
        "segment_cases",
        "newsletter_cases",
        "activity_cases",
        "run_cases",
        "cant_show_cases",
        "containment_cases",
        "cant_open_page_cases",
    ]
    .into_iter()
    .collect();

    let corpus_obj = corpus.as_object().expect("corpus object");
    for key in corpus_obj.keys() {
        assert!(
            known_categories.contains(key.as_str()),
            "Unknown corpus category: {key}"
        );
    }
    for key in &known_categories {
        assert!(
            corpus_obj.contains_key(*key),
            "Missing corpus category: {key}"
        );
    }

    let mut seen_ids = HashSet::new();
    let mut register_id = |id: &str| {
        assert!(!id.is_empty(), "id must not be empty");
        assert!(seen_ids.insert(id.to_owned()), "Duplicate corpus id: {id}");
    };

    for row in corpus["render_cases"].as_array().unwrap() {
        register_id(row["id"].as_str().unwrap());
    }
    for row in corpus["sanitizer_cases"].as_array().unwrap() {
        register_id(row["id"].as_str().unwrap());
    }

    let mut sentences_by_reason: BTreeMap<String, String> = BTreeMap::new();
    let mut page_heading = String::new();
    for row in corpus["cant_open_page_cases"].as_array().unwrap() {
        let id = row["id"].as_str().unwrap();
        register_id(id);
        let heading = row["heading"].as_str().unwrap();
        page_heading = heading.to_owned();
        let sentence = row["sentence"].as_str().unwrap();
        let reason_key = match id {
            "page-absence" => "absence",
            "page-cant-show" => "cant_show",
            "page-wont-follow" => "wont_follow",
            "page-couldnt-check" => "couldnt_check",
            other => panic!("unknown page case id: {other}"),
        };
        sentences_by_reason.insert(reason_key.to_owned(), sentence.to_owned());
    }

    // 1. Segments layout
    let temp_seg = tempdir().unwrap();
    let seg_root = temp_seg.path();
    create_segment(seg_root, "20200115", "room", "114500_300");
    create_segment(seg_root, "20200115", "_default", "114500_300");
    create_segment(seg_root, "20260901", "default", "100000_300");
    create_segment(seg_root, "20260901", "import.chatgpt", "100000_300");
    fs::create_dir_all(seg_root.join("chronicle/20260901/room/130000_300")).unwrap();
    let seg_app = source_link_router(seg_root.to_path_buf(), Arc::new(FilesystemReads));

    // 2. Newsletters layout
    let temp_news = tempdir().unwrap();
    let news_root = temp_news.path();
    write_file(
        &news_root.join("facets/work/news/20260901.md"),
        "# Work Newsletter",
    );
    write_json(
        &news_root.join("facets/work/facet.json"),
        &json!({"id":"a0000000-0000-4000-8000-000000000001"}),
    );
    write_file(
        &news_root.join("facets/newwork/news/20260901.md"),
        "# New Work News",
    );
    write_json(
        &news_root.join("facets/newwork/facet.json"),
        &json!({"id":"a0000000-0000-4000-8000-000000000002"}),
    );
    write_file(
        &news_root.join("facets/primary/news/20260901.md"),
        "# Primary News",
    );
    write_json(
        &news_root.join("facets/primary/facet.json"),
        &json!({"id":"a0000000-0000-4000-8000-000000000003"}),
    );
    write_json(
        &news_root.join("facets/retired.json"),
        &json!({
            "names": {
                "oldwork": {
                    "state": "renamed",
                    "successor": "a0000000-0000-4000-8000-000000000002"
                },
                "mergedfacet": {
                    "state": "merged",
                    "successor": "a0000000-0000-4000-8000-000000000003"
                },
                "deletedfacet": {
                    "state": "deleted"
                }
            }
        }),
    );
    let news_app = source_link_router(news_root.to_path_buf(), Arc::new(FilesystemReads));

    // 3. Activities layout
    let temp_act = tempdir().unwrap();
    let act_root = temp_act.path();
    create_segment(act_root, "20260901", "room", "100000_300");
    create_segment(act_root, "20260901", "room", "100500_300");
    create_segment(act_root, "20260901", "room", "101000_300");
    create_segment(act_root, "20260901", "desk", "101000_300");
    fs::create_dir_all(act_root.join("chronicle/20260901/room/101500_300")).unwrap();

    let activity_rows = [
        json!({"id":"act_sync","segments":["100000_300"]}).to_string(),
        json!({"id":"act_multi","segments":["101500_300","100500_300"]}).to_string(),
        json!({"id":"act_empty_segments","segments":[]}).to_string(),
        json!({"id":"act_ambiguous","segments":["101000_300"]}).to_string(),
        json!({"id":"act_invalid_keys","segments":["notakey"]}).to_string(),
        json!({"id":"act_gone_keys","segments":["105000_300"]}).to_string(),
    ]
    .join("\n");
    write_file(
        &act_root.join("facets/work/activities/20260901.jsonl"),
        &activity_rows,
    );
    write_json(
        &act_root.join("facets/work/facet.json"),
        &json!({"id":"a0000000-0000-4000-8000-000000000001"}),
    );
    write_file(
        &act_root.join("facets/newwork/activities/20260901.jsonl"),
        &activity_rows,
    );
    write_json(
        &act_root.join("facets/newwork/facet.json"),
        &json!({"id":"a0000000-0000-4000-8000-000000000002"}),
    );
    write_json(
        &act_root.join("facets/retired.json"),
        &json!({
            "names": {
                "oldwork": {
                    "state": "renamed",
                    "successor": "a0000000-0000-4000-8000-000000000002"
                }
            }
        }),
    );
    let act_app = source_link_router(act_root.to_path_buf(), Arc::new(FilesystemReads));

    // 4. Runs layout from corpus run_cases
    let temp_run = tempdir().unwrap();
    let run_root = temp_run.path();
    let mut day_index_lines: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in corpus["run_cases"].as_array().unwrap() {
        let ref_url = row["ref"].as_str().unwrap();
        let (day, rel) = extract_run_relative(ref_url).expect("valid run ref coordinate");
        let output_present = row
            .get("output_present")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if output_present {
            let output_path = run_root.join("chronicle").join(&day).join(&rel);
            write_file(&output_path, "output content\n");
        }
        if let Some(index_rows) = row.get("index_rows").and_then(Value::as_array) {
            for idx_row in index_rows {
                let map = idx_row.as_object().unwrap();
                let name = map.get("name").and_then(Value::as_str);
                let use_id = map.get("use_id").and_then(Value::as_str);
                let record = map.get("record").and_then(Value::as_str);

                if let (Some(name), Some(use_id), Some(record)) = (name, use_id, record) {
                    let record_path = run_root
                        .join("talents")
                        .join(talent_directory_name(name))
                        .join(format!("{use_id}.jsonl"));
                    match record {
                        "readable" => {
                            write_file(&record_path, &format!("{{\"use_id\":\"{use_id}\"}}\n"));
                        }
                        "malformed" => {
                            write_file(&record_path, "not-json\n");
                        }
                        "missing" => {}
                        other => panic!("unknown record state: {other}"),
                    }
                }

                let mut line_obj = map.clone();
                line_obj.remove("record");
                let line_str = serde_json::to_string(&Value::Object(line_obj)).unwrap();
                day_index_lines
                    .entry(day.clone())
                    .or_default()
                    .push(line_str);
            }
        }
    }

    for (day, lines) in day_index_lines {
        let text = lines.join("\n") + "\n";
        write_file(
            &run_root.join("talents").join(format!("{day}.jsonl")),
            &text,
        );
    }

    let run_app = source_link_router(run_root.to_path_buf(), Arc::new(FilesystemReads));
    let panic_app = source_link_router(run_root.to_path_buf(), Arc::new(PanicReads));
    let read_err_app = source_link_router(
        run_root.to_path_buf(),
        Arc::new(RecordReadErrorReads {
            inner: FilesystemReads,
        }),
    );

    let assert_response_contract =
        |id: &str, status: StatusCode, body: &str, location: Option<&str>, row: &Value| {
            let expected_status =
                StatusCode::from_u16(row["status"].as_u64().unwrap() as u16).unwrap();
            assert_eq!(status, expected_status, "case {id}: status mismatch");
            if expected_status == StatusCode::FOUND {
                let expected_loc = row["location"].as_str().unwrap();
                assert_eq!(location, Some(expected_loc), "case {id}: location mismatch");
            } else {
                let reason = row["reason"].as_str().unwrap();
                let expected_sentence = sentences_by_reason.get(reason).unwrap();
                assert!(
                    body.contains(expected_sentence),
                    "case {id}: body must contain '{expected_sentence}', got:\n{body}"
                );
                assert!(
                    body.contains(&page_heading),
                    "case {id}: body must contain heading '{page_heading}'"
                );
                assert!(
                    body.contains(
                        r#"<meta name="viewport" content="width=device-width, initial-scale=1"/>"#
                    ),
                    "case {id}: body must contain viewport meta"
                );
                assert!(
                    body.contains(r#"<link rel="stylesheet" href="/static/tokens.css">"#),
                    "case {id}: body must contain tokens.css link"
                );
                assert!(
                    body.contains(r#"<link rel="stylesheet" href="/static/tokens-dark.css">"#),
                    "case {id}: body must contain tokens-dark.css link"
                );
            }
        };

    // Assert segment_cases
    for row in corpus["segment_cases"].as_array().unwrap() {
        let id = row["id"].as_str().unwrap();
        register_id(id);
        let ref_url = row["ref"].as_str().unwrap();
        let (status, body, loc) = get_source(seg_app.clone(), ref_url).await;
        assert_response_contract(id, status, &body, loc.as_deref(), row);
    }

    // Assert newsletter_cases
    for row in corpus["newsletter_cases"].as_array().unwrap() {
        let id = row["id"].as_str().unwrap();
        register_id(id);
        let ref_url = row["ref"].as_str().unwrap();
        let fixture = row.get("fixture").and_then(Value::as_str);

        if fixture == Some("retired_absent") {
            let temp_absent = tempdir().unwrap();
            let absent_root = temp_absent.path();
            fs::create_dir_all(absent_root.join("facets")).unwrap();
            let app = source_link_router(absent_root.to_path_buf(), Arc::new(FilesystemReads));
            let (status, body, loc) = get_source(app, ref_url).await;
            assert_response_contract(id, status, &body, loc.as_deref(), row);
            continue;
        }

        if fixture == Some("symlink_escape") {
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                let temp_outside = tempdir().unwrap();
                let temp_escape = tempdir().unwrap();
                let escape_root = temp_escape.path();
                symlink(temp_outside.path(), escape_root.join("facets")).unwrap();
                let app = source_link_router(escape_root.to_path_buf(), Arc::new(FilesystemReads));
                let (status, body, loc) = get_source(app, ref_url).await;
                assert_response_contract(id, status, &body, loc.as_deref(), row);
            }
            continue;
        }

        let (status, body, loc) = get_source(news_app.clone(), ref_url).await;
        assert_response_contract(id, status, &body, loc.as_deref(), row);
    }

    // Assert activity_cases
    for row in corpus["activity_cases"].as_array().unwrap() {
        let id = row["id"].as_str().unwrap();
        register_id(id);
        let ref_url = row["ref"].as_str().unwrap();
        let (status, body, loc) = get_source(act_app.clone(), ref_url).await;
        assert_response_contract(id, status, &body, loc.as_deref(), row);
    }

    // Assert run_cases
    for row in corpus["run_cases"].as_array().unwrap() {
        let id = row["id"].as_str().unwrap();
        register_id(id);
        let ref_url = row["ref"].as_str().unwrap();
        let app = if row.get("fixture").and_then(Value::as_str) == Some("read_error") {
            read_err_app.clone()
        } else {
            run_app.clone()
        };
        let (status, body, loc) = get_source(app, ref_url).await;
        assert_response_contract(id, status, &body, loc.as_deref(), row);
    }

    // Assert cant_show_cases (using PanicReads double)
    for row in corpus["cant_show_cases"].as_array().unwrap() {
        let id = row["id"].as_str().unwrap();
        register_id(id);
        let ref_url = row["ref"].as_str().unwrap();
        let (status, body, loc) = get_source(panic_app.clone(), ref_url).await;
        assert_response_contract(id, status, &body, loc.as_deref(), row);
    }

    // Assert containment_cases (using PanicReads double)
    for row in corpus["containment_cases"].as_array().unwrap() {
        let id = row["id"].as_str().unwrap();
        register_id(id);
        let ref_url = row["ref"].as_str().unwrap();
        let (status, body, loc) = get_source(panic_app.clone(), ref_url).await;
        assert_response_contract(id, status, &body, loc.as_deref(), row);
    }
}

struct InjectedFaultReads {
    inner: FilesystemReads,
    fail_segments: bool,
    fail_day_stat: bool,
    fail_news: bool,
    fail_index: bool,
    fail_activity: bool,
    retired_override: Option<RetiredFacets>,
}

impl InjectedFaultReads {
    fn new() -> Self {
        Self {
            inner: FilesystemReads,
            fail_segments: false,
            fail_day_stat: false,
            fail_news: false,
            fail_index: false,
            fail_activity: false,
            retired_override: None,
        }
    }
}

impl SourceReads for InjectedFaultReads {
    fn path_exists(&self, path: &Path) -> Result<bool, String> {
        let path_str = path.to_string_lossy();
        if self.fail_day_stat && path_str.contains("chronicle") {
            return Err("injected day stat failure".to_owned());
        }
        self.inner.path_exists(path)
    }

    fn read_day_segments(
        &self,
        journal: &Path,
        day: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<DaySegmentRef>, String> {
        if self.fail_segments {
            return Err("injected segment scan failure".to_owned());
        }
        self.inner.read_day_segments(journal, day, now)
    }

    fn is_facet_dir(&self, journal: &Path, facet: &str) -> Result<bool, String> {
        self.inner.is_facet_dir(journal, facet)
    }

    fn read_retired_facets(&self, journal: &Path) -> Result<RetiredFacets, String> {
        if let Some(r) = &self.retired_override {
            return Ok(r.clone());
        }
        self.inner.read_retired_facets(journal)
    }

    fn resolve_facet_id(&self, journal: &Path, id: &str) -> Result<String, FacetIdResolveError> {
        self.inner.resolve_facet_id(journal, id)
    }

    fn read_news_file(
        &self,
        journal: &Path,
        facet: &str,
        file: &str,
    ) -> Result<Option<String>, String> {
        if self.fail_news {
            return Err("injected news read failure".to_owned());
        }
        self.inner.read_news_file(journal, facet, file)
    }

    fn read_file_text(&self, path: &Path) -> Result<String, String> {
        let path_str = path.to_string_lossy();
        if self.fail_index && path_str.contains("talents") {
            return Err("injected index read failure".to_owned());
        }
        if self.fail_activity && path_str.contains("activities") {
            return Err("injected activity read failure".to_owned());
        }
        self.inner.read_file_text(path)
    }

    fn read_run_record(&self, path: &Path) -> Result<Option<String>, String> {
        self.inner.read_run_record(path)
    }
}

#[tokio::test]
async fn check_failure_injections() {
    let temp = tempdir().unwrap();
    let root = temp.path();

    create_segment(root, "20260901", "room", "100000_300");
    write_file(&root.join("facets/work/news/20260901.md"), "news");
    write_file(&root.join("chronicle/20260901/talents/plain.md"), "plain");
    write_file(
        &root.join("talents/20260901.jsonl"),
        "{\"use_id\":\"u1\",\"output_file\":\"talents/plain.md\",\"ts\":100}\n",
    );
    write_file(
        &root.join("facets/work/activities/20260901.jsonl"),
        "{\"id\":\"act1\",\"segments\":[\"100000_300\"]}\n",
    );

    // 1. Day stat failure
    let mut reads = InjectedFaultReads::new();
    reads.fail_day_stat = true;
    let app = source_link_router(root.to_path_buf(), Arc::new(reads));
    let (status, body, _) = get_source(app, "sol://20260901/room/100000_300").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.contains("your journal couldn't check this source."));
    assert!(!body.contains("it isn't in your journal."));

    // 2. Segment list failure
    let mut reads = InjectedFaultReads::new();
    reads.fail_segments = true;
    let app = source_link_router(root.to_path_buf(), Arc::new(reads));
    let (status, body, _) = get_source(app, "sol://20260901/room/100000_300").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.contains("your journal couldn't check this source."));
    assert!(!body.contains("it isn't in your journal."));

    // 3. News read failure
    let mut reads = InjectedFaultReads::new();
    reads.fail_news = true;
    let app = source_link_router(root.to_path_buf(), Arc::new(reads));
    let (status, body, _) = get_source(app, "sol://facets/work/news/20260901").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.contains("your journal couldn't check this source."));
    assert!(!body.contains("it isn't in your journal."));

    // 4. Run index failure
    let mut reads = InjectedFaultReads::new();
    reads.fail_index = true;
    let app = source_link_router(root.to_path_buf(), Arc::new(reads));
    let (status, body, _) = get_source(app, "sol://20260901/talents/plain.md").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.contains("your journal couldn't check this source."));
    assert!(!body.contains("it isn't in your journal."));

    // 5. Activity file failure
    let mut reads = InjectedFaultReads::new();
    reads.fail_activity = true;
    let app = source_link_router(root.to_path_buf(), Arc::new(reads));
    let (status, body, _) = get_source(app, "sol://facets/work/activities/20260901#act1").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.contains("your journal couldn't check this source."));
    assert!(!body.contains("it isn't in your journal."));

    // 6. Retired unreadable on newsletter
    let mut reads = InjectedFaultReads::new();
    reads.retired_override = Some(RetiredFacets::Unreadable("damaged disk".into()));
    let app = source_link_router(root.to_path_buf(), Arc::new(reads));
    let (status, body, _) = get_source(app, "sol://facets/renamed/news/20260901").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.contains("your journal couldn't check this source."));
    assert!(!body.contains("it isn't in your journal."));

    // 7. Retired unreadable on activity
    let mut reads = InjectedFaultReads::new();
    reads.retired_override = Some(RetiredFacets::Unreadable("damaged disk".into()));
    let app = source_link_router(root.to_path_buf(), Arc::new(reads));
    let (status, body, _) = get_source(app, "sol://facets/renamed/activities/20260901#act1").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.contains("your journal couldn't check this source."));
    assert!(!body.contains("it isn't in your journal."));

    // 8. Retired malformed on newsletter
    let mut reads = InjectedFaultReads::new();
    reads.retired_override = Some(RetiredFacets::Malformed("bad json".into()));
    let app = source_link_router(root.to_path_buf(), Arc::new(reads));
    let (status, body, _) = get_source(app, "sol://facets/renamed/news/20260901").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.contains("your journal couldn't check this source."));
    assert!(!body.contains("it isn't in your journal."));

    // 9. Retired malformed on activity
    let mut reads = InjectedFaultReads::new();
    reads.retired_override = Some(RetiredFacets::Malformed("bad json".into()));
    let app = source_link_router(root.to_path_buf(), Arc::new(reads));
    let (status, body, _) = get_source(app, "sol://facets/renamed/activities/20260901#act1").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.contains("your journal couldn't check this source."));
    assert!(!body.contains("it isn't in your journal."));
}

#[tokio::test]
async fn established_journal_passes_through_main_shell_router() {
    let temp = tempdir().unwrap();
    let root = temp.path();

    write_json(
        &root.join("config/journal.json"),
        &json!({
            "setup": {"completed_at": 1_700_000_000_000i64},
            "identity": {"name": "Test Owner", "timezone": "UTC"}
        }),
    );

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let corpus_text = fs::read_to_string(manifest_dir.join("tests/source_link_corpus.json"))
        .expect("read corpus json");
    let corpus: Value = serde_json::from_str(&corpus_text).expect("parse corpus json");
    let cant_show_events = corpus["cant_show_cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "cant-show-events")
        .expect("find cant-show-events in corpus");
    let ref_url = cant_show_events["ref"].as_str().unwrap();

    let app = router(root.to_path_buf());
    let (status, body, _) = get_source(app, ref_url).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("this source can't be opened"));
    assert!(!body.contains("The requested URL was not found on the server"));
}
