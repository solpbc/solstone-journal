// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
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
use solstone_core_transcripts_web::DaySegmentRef;

#[test]
fn source_link_dom_harness() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("node")
        .arg(manifest_dir.join("tests/source_link_dom.js"))
        .arg(&manifest_dir)
        .output()
        .expect("node executes");
    assert!(
        output.status.success(),
        "source_link_dom failed:\nSTDOUT:\n{}\nSTDERR:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

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
    write_file(&path, "{\"sample\":1}\n");
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

#[tokio::test]
async fn segments_resolution_matrix() {
    let temp = tempdir().unwrap();
    let root = temp.path();

    // 1. Named segment
    create_segment(root, "20200115", "room", "114500_300");
    // 2. Direct segment with same key on direct and named
    create_segment(root, "20200115", "_default", "114500_300");
    // 3. Literal default stream
    create_segment(root, "20260901", "default", "100000_300");
    // 4. Dot in stream name
    create_segment(root, "20260901", "import.chatgpt", "100000_300");
    // 5. Empty dir (unlisted)
    fs::create_dir_all(root.join("chronicle/20260901/room/130000_300")).unwrap();

    let app = source_link_router(root.to_path_buf(), Arc::new(FilesystemReads));

    // Named valid
    let (status, _, loc) = get_source(app.clone(), "sol://20200115/room/114500_300").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        loc.as_deref(),
        Some("/app/transcripts/20200115?stream=room#114500_300")
    );

    // Direct valid
    let (status, _, loc) = get_source(app.clone(), "sol://20200115/114500_300").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        loc.as_deref(),
        Some("/app/transcripts/20200115?stream=_default#114500_300")
    );

    // Literal default
    let (status, _, loc) = get_source(app.clone(), "sol://20260901/default/100000_300").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        loc.as_deref(),
        Some("/app/transcripts/20260901?stream=default#100000_300")
    );

    // Stream with dots
    let (status, _, loc) =
        get_source(app.clone(), "sol://20260901/import.chatgpt/100000_300").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        loc.as_deref(),
        Some("/app/transcripts/20260901?stream=import.chatgpt#100000_300")
    );

    // Mixed case scheme Sol://
    let (status, _, loc) =
        get_source(app.clone(), "Sol://20260901/import.chatgpt/100000_300").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        loc.as_deref(),
        Some("/app/transcripts/20260901?stream=import.chatgpt#100000_300")
    );

    // Deleted segment (day exists, key does not)
    let (status, body, loc) = get_source(app.clone(), "sol://20260901/room/120000_300").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("it isn't in your journal."));
    assert!(loc.is_none());

    // Empty segment dir (not listed)
    let (status, body, _) = get_source(app.clone(), "sol://20260901/room/130000_300").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("it isn't in your journal."));

    // Day missing
    let (status, body, _) = get_source(app.clone(), "sol://19990101/room/100000_300").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("it isn't in your journal."));
}

#[tokio::test]
async fn newsletters_resolution_matrix() {
    let temp = tempdir().unwrap();
    let root = temp.path();

    // Live facet
    write_file(
        &root.join("facets/work/news/20260901.md"),
        "# Work Newsletter",
    );
    write_json(
        &root.join("facets/work/facet.json"),
        &json!({"id":"a0000000-0000-4000-8000-000000000001"}),
    );

    // Renamed facet successor
    write_file(
        &root.join("facets/newwork/news/20260901.md"),
        "# New Work News",
    );
    write_json(
        &root.join("facets/newwork/facet.json"),
        &json!({"id":"a0000000-0000-4000-8000-000000000002"}),
    );

    // Merged facet successor with same day news
    write_file(
        &root.join("facets/primary/news/20260901.md"),
        "# Primary News",
    );
    write_json(
        &root.join("facets/primary/facet.json"),
        &json!({"id":"a0000000-0000-4000-8000-000000000003"}),
    );

    // Retired facets file
    write_json(
        &root.join("facets/retired.json"),
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

    let app = source_link_router(root.to_path_buf(), Arc::new(FilesystemReads));

    // Live valid without .md
    let (status, _, loc) = get_source(app.clone(), "sol://facets/work/news/20260901").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc.as_deref(), Some("/app/news/work/20260901"));

    // Live valid with .md
    let (status, _, loc) = get_source(app.clone(), "sol://facets/work/news/20260901.md").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc.as_deref(), Some("/app/news/work/20260901"));

    // Renamed facet
    let (status, _, loc) = get_source(app.clone(), "sol://facets/oldwork/news/20260901").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc.as_deref(), Some("/app/news/newwork/20260901"));

    // Missing file
    let (status, body, _) = get_source(app.clone(), "sol://facets/work/news/20200101").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("it isn't in your journal."));

    // Merged facet
    let (status, body, loc) =
        get_source(app.clone(), "sol://facets/mergedfacet/news/20260901").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("your journal can't show this kind of source."));
    assert!(loc.is_none());

    // Deleted facet
    let (status, body, _) =
        get_source(app.clone(), "sol://facets/deletedfacet/news/20260901").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("it isn't in your journal."));

    // Facet containing dot
    let (status, body, _) = get_source(app.clone(), "sol://facets/work.facet/news/20260901").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("your journal won't follow this link."));
}

#[tokio::test]
async fn runs_resolution_matrix() {
    let temp = tempdir().unwrap();
    let root = temp.path();

    // Files on disk
    write_file(
        &root.join("chronicle/20260901/talents/plain.md"),
        "plain output",
    );
    write_file(
        &root.join("chronicle/20260901/talents/work/_app_facet.json"),
        "{}",
    );
    write_file(&root.join("chronicle/20260901/custom.md"), "custom output");
    write_file(
        &root.join("chronicle/20260901/talents/summary.md"),
        "summary",
    );
    write_file(&root.join("chronicle/20260901/talents/equal.md"), "equal");
    write_file(&root.join("chronicle/20260901/talents/orphan.md"), "orphan");

    // Index rows
    let index_lines = vec![
        json!({"use_id":"use_plain_1","output_file":"talents/plain.md","ts":100}).to_string(),
        json!({"use_id":"use_app_1","output_file":"talents/work/_app_facet.json","ts":200})
            .to_string(),
        json!({"use_id":"use_custom_1","output_file":"custom.md","ts":300}).to_string(),
        json!({"use_id":"use_summary_1","output_file":"talents/summary.md","ts":400}).to_string(),
        json!({"use_id":"use_summary_2","output_file":"talents/summary.md","ts":500}).to_string(),
        json!({"use_id":"use_equal_first","output_file":"talents/equal.md","ts":600}).to_string(),
        json!({"use_id":"use_equal_second","output_file":"talents/equal.md","ts":600}).to_string(),
        json!({"use_id":"use_unwritten","output_file":"talents/unwritten.md","ts":700}).to_string(),
    ]
    .join("\n");
    write_file(&root.join("talents/20260901.jsonl"), &index_lines);

    let app = source_link_router(root.to_path_buf(), Arc::new(FilesystemReads));

    // Plain talent
    let (status, _, loc) = get_source(app.clone(), "sol://20260901/talents/plain.md").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc.as_deref(), Some("/app/thinking/#runs/run/use_plain_1"));

    // Chronicle prefix
    let (status, _, loc) =
        get_source(app.clone(), "sol://chronicle/20260901/talents/plain.md").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc.as_deref(), Some("/app/thinking/#runs/run/use_plain_1"));

    // App facet
    let (status, _, loc) =
        get_source(app.clone(), "sol://20260901/talents/work/_app_facet.json").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc.as_deref(), Some("/app/thinking/#runs/run/use_app_1"));

    // Custom override
    let (status, _, loc) = get_source(app.clone(), "sol://20260901/custom.md").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc.as_deref(), Some("/app/thinking/#runs/run/use_custom_1"));

    // Multi row: higher ts wins
    let (status, _, loc) = get_source(app.clone(), "sol://20260901/talents/summary.md").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        loc.as_deref(),
        Some("/app/thinking/#runs/run/use_summary_2")
    );

    // Equal ts: earlier row wins
    let (status, _, loc) = get_source(app.clone(), "sol://20260901/talents/equal.md").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        loc.as_deref(),
        Some("/app/thinking/#runs/run/use_equal_first")
    );

    // No index, no file
    let (status, body, _) = get_source(app.clone(), "sol://20260901/talents/missing.md").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("it isn't in your journal."));

    // File present, no matching row
    let (status, body, _) = get_source(app.clone(), "sol://20260901/talents/orphan.md").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("your journal can't show this kind of source."));

    // Row present, file gone
    let (status, body, _) = get_source(app.clone(), "sol://20260901/talents/unwritten.md").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("it isn't in your journal."));
}

#[tokio::test]
async fn activities_resolution_matrix() {
    let temp = tempdir().unwrap();
    let root = temp.path();

    write_json(
        &root.join("facets/work/facet.json"),
        &json!({"id":"a0000000-0000-4000-8000-000000000001"}),
    );
    write_json(
        &root.join("facets/newwork/facet.json"),
        &json!({"id":"a0000000-0000-4000-8000-000000000002"}),
    );
    write_json(
        &root.join("facets/retired.json"),
        &json!({
            "names": {
                "oldwork": {
                    "state": "renamed",
                    "successor": "a0000000-0000-4000-8000-000000000002"
                }
            }
        }),
    );

    // Segments in journal
    create_segment(root, "20260901", "room", "100000_300");
    create_segment(root, "20260901", "room", "100500_300");
    // Ambiguous segment (in both room and desk)
    create_segment(root, "20260901", "room", "101000_300");
    create_segment(root, "20260901", "desk", "101000_300");
    // Unlisted empty segment
    fs::create_dir_all(root.join("chronicle/20260901/room/101500_300")).unwrap();

    let activity_rows = vec![
        json!({"id":"act_sync","segments":["100000_300"]}).to_string(),
        json!({"id":"act_multi","segments":["101500_300","100500_300"]}).to_string(),
        json!({"id":"act_empty_segments","segments":[]}).to_string(),
        json!({"id":"act_ambiguous","segments":["101000_300"]}).to_string(),
        json!({"id":"act_invalid_keys","segments":["notakey"]}).to_string(),
        json!({"id":"act_gone_keys","segments":["105000_300"]}).to_string(),
    ]
    .join("\n");
    write_file(
        &root.join("facets/work/activities/20260901.jsonl"),
        &activity_rows,
    );
    write_file(
        &root.join("facets/newwork/activities/20260901.jsonl"),
        &activity_rows,
    );

    let app = source_link_router(root.to_path_buf(), Arc::new(FilesystemReads));

    // Valid single segment
    let (status, _, loc) = get_source(
        app.clone(),
        "sol://facets/work/activities/20260901#act_sync",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        loc.as_deref(),
        Some("/app/transcripts/20260901?stream=room#100000_300")
    );

    // Multi-segment: first is unlisted empty dir, second is listed
    let (status, _, loc) = get_source(
        app.clone(),
        "sol://facets/work/activities/20260901#act_multi",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        loc.as_deref(),
        Some("/app/transcripts/20260901?stream=room#100500_300")
    );

    // Renamed facet
    let (status, _, loc) = get_source(
        app.clone(),
        "sol://facets/oldwork/activities/20260901#act_sync",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        loc.as_deref(),
        Some("/app/transcripts/20260901?stream=room#100000_300")
    );

    // Missing id
    let (status, body, _) = get_source(
        app.clone(),
        "sol://facets/work/activities/20260901#act_missing",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("it isn't in your journal."));

    // Empty segments
    let (status, body, _) = get_source(
        app.clone(),
        "sol://facets/work/activities/20260901#act_empty_segments",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("your journal can't show this kind of source."));

    // Ambiguous only
    let (status, body, loc) = get_source(
        app.clone(),
        "sol://facets/work/activities/20260901#act_ambiguous",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("your journal can't show this kind of source."));
    assert!(loc.is_none());

    // Invalid segment keys
    let (status, body, _) = get_source(
        app.clone(),
        "sol://facets/work/activities/20260901#act_invalid_keys",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("your journal can't show this kind of source."));

    // All keys gone
    let (status, body, _) = get_source(
        app.clone(),
        "sol://facets/work/activities/20260901#act_gone_keys",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("it isn't in your journal."));

    // Missing activity day file is absence
    let (status, body, loc) = get_source(
        app.clone(),
        "sol://facets/work/activities/20200101#act_sync",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("it isn't in your journal."));
    assert!(loc.is_none());
}

#[tokio::test]
async fn cant_show_and_containment_refusals() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let app = source_link_router(root.to_path_buf(), Arc::new(FilesystemReads));

    // Can't show arm
    for r in [
        "sol://facets/work/events/20260901",
        "sol://reflections/weekly/20260901",
        "sol://facets/work/reflections/20260901",
        "sol://unknown/collection/resource",
    ] {
        let (status, body, loc) = get_source(app.clone(), r).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{r}");
        assert!(
            body.contains("your journal can't show this kind of source."),
            "{r}"
        );
        assert!(loc.is_none(), "{r}");
    }

    // Containment refusals (400)
    for r in [
        "sol://20260901/../config",
        "sol://20260901\\..\\config",
        "sol://20260901/%2e%2e/config",
        "sol://20260901/%252e%252e/config",
        "sol://20260901//100000_300",
        "sol:///etc/passwd",
        "sol:////attacker.com",
        "https://example.com",
        "sol://20260901/notasegment",
        "sol://20260901/room/notasegment",
        "sol://20260231/100000_300",
    ] {
        let (status, body, loc) = get_source(app.clone(), r).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{r}");
        assert!(body.contains("your journal won't follow this link."), "{r}");
        assert!(loc.is_none(), "{r}");
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

    // Established journal setup
    write_json(
        &root.join("config/journal.json"),
        &json!({
            "setup": {"completed_at": 1_700_000_000_000i64},
            "identity": {"name": "Test Owner", "timezone": "UTC"}
        }),
    );

    let app = router(root.to_path_buf());
    let (status, body, _) = get_source(app, "sol://facets/work/events/20260901").await;

    // Must be 404 from our handler ("this source can't be opened"), not the generic shell 404
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("this source can't be opened"));
    assert!(!body.contains("The requested URL was not found on the server"));
}
