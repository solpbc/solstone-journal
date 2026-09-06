// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Focused black-box contract tests for the native Thinking Sol read routes.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use solstone_core_convey_shell::router;
use tower::ServiceExt;

#[path = "thinking_sol_writes.rs"]
mod thinking_sol_writes;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from("/var/tmp").join(format!(
            "solstone-thinking-sol-reads-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&path).expect("fixture root");
        Self(path)
    }

    fn established(&self) {
        write_json(
            &self.0.join("config/journal.json"),
            json!({
                "setup": {"completed_at": 1_700_000_000_000i64},
                "identity": {"name": "Corpus Owner", "timezone": "UTC"},
            }),
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_json(path: &Path, value: Value) {
    fs::create_dir_all(path.parent().expect("parent")).expect("parent creates");
    fs::write(
        path,
        format!("{}\n", serde_json::to_string(&value).expect("json")),
    )
    .expect("json writes");
}

fn write_jsonl(path: &Path, entries: &[Value]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("parent creates");
    let text = entries
        .iter()
        .map(|entry| format!("{}\n", serde_json::to_string(entry).expect("json")))
        .collect::<String>();
    fs::write(path, text).expect("jsonl writes");
}

fn today() -> String {
    chrono::Local::now().format("%Y%m%d").to_string()
}

fn request_event(id: &str, day: &str, name: &str, facet: Option<&str>) -> Value {
    request_event_at(id, day, name, facet, 1_710_000_000_000)
}

fn request_event_at(id: &str, day: &str, name: &str, facet: Option<&str>, ts: i64) -> Value {
    let mut event = json!({"event":"request", "use_id":id, "day":day, "name":name, "prompt":"Corpus prompt", "ts":ts, "provider":"openai"});
    if let Some(facet) = facet {
        event["facet"] = json!(facet);
    }
    event
}

fn seed_populated(fixture: &Fixture, failures_today: usize) {
    fixture.established();
    let day = "20260403";
    write_jsonl(
        &fixture.0.join("talents/daily_digest/1710000000000.jsonl"),
        &[
            request_event("1710000000000", day, "daily_digest", Some("work")),
            json!({"event":"start", "model":"gpt-5.5", "provider":"openai", "ts":1710000000100i64}),
            json!({"event":"finish", "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}, "ts":1710000001000i64}),
        ],
    );
    write_jsonl(
        &fixture.0.join("talents/review/1710000060000_active.jsonl"),
        &[request_event_at(
            "1710000060000",
            day,
            "review",
            Some("work"),
            1_710_000_060_000,
        )],
    );
    fs::create_dir_all(fixture.0.join("talents/summary")).expect("summary dir");
    fs::write(fixture.0.join("talents/summary/1710000120000.jsonl"), "").expect("malformed run");
    write_jsonl(
        &fixture.0.join("talents/20260403.jsonl"),
        &[
            json!({"use_id":"1710000000000", "name":"daily_digest", "facet":"work", "status":"completed", "provider":"openai", "ts":1710000000000i64}),
        ],
    );
    let capture = today();
    let failures = (0..failures_today)
        .map(|index| json!({"use_id":format!("failure-{index}"), "name":"failed", "status":"error", "reason_code":"provider_error", "ts":1710000001000i64 + index as i64}))
        .collect::<Vec<_>>();
    write_jsonl(
        &fixture.0.join(format!("talents/{capture}.jsonl")),
        &failures,
    );
    fs::create_dir_all(fixture.0.join("chronicle/20260403/talents")).expect("output parent");
    fs::write(
        fixture
            .0
            .join("chronicle/20260403/talents/example-output.md"),
        "# Corpus output\n\nDeterministic fixture content.\n",
    )
    .expect("output");
}

async fn get(app: axum::Router, path: &str, cookie: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::builder().method("GET").uri(path);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    let response = app
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&body).unwrap_or_else(|_| json!(String::from_utf8_lossy(&body))),
    )
}

fn corpus_body(phase: &str, probe: &str) -> Value {
    let corpus: Value =
        serde_json::from_str(include_str!("../../../fixtures/convey_sol_corpus.json"))
            .expect("corpus json");
    let mut body = corpus["phases"][phase]["sol"]
        .as_array()
        .expect("sol probes")
        .iter()
        .find(|entry| entry["name"] == probe)
        .expect("probe")["response"]["body"]
        .clone();
    // Dollar estimates are retired; preserve every other captured read field.
    if let Some(row) = body.as_object_mut() {
        row.remove("cost");
    }
    if let Some(uses) = body.get_mut("uses").and_then(Value::as_array_mut) {
        for row in uses {
            row.as_object_mut().unwrap().remove("cost");
        }
    }
    body
}

fn normalize_capture_index(mut body: Value) -> Value {
    let capture_day = today();
    // Implements corpus normalized paths `response.body.coverage.end` and
    // `response.body.months#capture_month_key` for the current local capture day.
    body["coverage"]["end"] = json!(capture_day);
    let months = body["months"].as_object_mut().expect("index months");
    let captured = months
        .remove("<CAPTURE_MONTH>")
        .expect("capture month placeholder");
    months.insert(capture_day[..6].to_owned(), captured);
    body
}

fn assert_stable_talent_metadata(body: &Value, expected: &Value) {
    assert_eq!(body["uses"], expected["uses"]);
    assert_eq!(body["facets"], expected["facets"]);
    // Talent frontmatter is independently editable shipped content, so this route
    // contract bounds metadata to representative default, system, and app entries.
    let talents = body["talents"].as_object().expect("talents");
    assert_eq!(
        talents.get("conversation"),
        expected["talents"].get("conversation"),
        "system talent with explicit color and title"
    );
    assert_eq!(
        talents.get("entities:detection"),
        expected["talents"].get("entities:detection"),
        "app talent carries source and app"
    );
    assert_eq!(talents["entities:detection"]["source"], "app");
    assert_eq!(talents["entities:detection"]["app"], "entities");
}

#[tokio::test]
async fn ac2_talents_day_oracle_empty_and_populated() {
    let empty = Fixture::new();
    empty.established();
    let (status, body) = get(
        router(empty.0.clone()),
        "/app/thinking/api/talents/20260403",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_stable_talent_metadata(&body, &corpus_body("established_empty", "api_talents"));

    let fixture = Fixture::new();
    seed_populated(&fixture, 3);
    let app = router(fixture.0.clone());
    let (status, body) = get(app.clone(), "/app/thinking/api/talents/20260403", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_stable_talent_metadata(&body, &corpus_body("established_populated", "api_talents"));

    let day_index = fixture.0.join("talents/20260403.jsonl");
    fs::write(
        &day_index,
        concat!(
            "{\"use_id\":\"old\",\"name\":\"old\",\"status\":\"completed\",\"ts\":1}\n",
            "this is not json\n",
            "{\"use_id\":\"middle\",\"name\":\"middle\",\"status\":\"completed\",\"ts\":2}\n",
            "{\"use_id\":\"new\",\"name\":\"new\",\"status\":\"completed\",\"ts\":3}\n"
        ),
    )
    .expect("mixed day index");
    let (_, ordered) = get(app.clone(), "/app/thinking/api/talents/20260403", None).await;
    let ids = ordered["uses"]
        .as_array()
        .expect("uses")
        .iter()
        .map(|entry| entry["id"].as_str().expect("id"))
        .collect::<Vec<_>>();
    assert_eq!(ids, ["1710000060000", "new", "middle", "old"]);

    let (status, invalid) = get(app, "/app/thinking/api/talents/2026-04-03", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid["reason_code"], "invalid_day");
}

#[tokio::test]
async fn ac4_run_detail_completed_pending_and_malformed_oracle() {
    let fixture = Fixture::new();
    seed_populated(&fixture, 3);
    let app = router(fixture.0.clone());
    let (status, completed) = get(app.clone(), "/app/thinking/api/run/1710000000000", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        completed,
        corpus_body("established_populated", "api_run_completed")
    );
    let (status, pending) = get(app.clone(), "/app/thinking/api/run/1710000060000", None).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        pending,
        corpus_body("established_populated", "api_run_pending")
    );
    let (status, malformed) = get(app, "/app/thinking/api/run/1710000120000", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        malformed,
        corpus_body("established_populated", "api_run_malformed")
    );

    let empty = Fixture::new();
    empty.established();
    let (status, missing) = get(
        router(empty.0.clone()),
        "/app/thinking/api/run/1710000000000",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        missing,
        corpus_body("established_empty", "api_run_completed")
    );

    let pricing = Fixture::new();
    pricing.established();
    write_jsonl(
        &pricing.0.join("talents/pricing/null-usage.jsonl"),
        &[
            request_event("null-usage", "20260403", "pricing", None),
            json!({"event":"start", "model":"gpt-5.5"}),
            json!({"event":"finish", "usage":null}),
        ],
    );
    write_jsonl(
        &pricing.0.join("talents/pricing/empty-usage.jsonl"),
        &[
            request_event("empty-usage", "20260403", "pricing", None),
            json!({"event":"start", "model":"gpt-5.5"}),
            json!({"event":"finish", "usage":{}}),
        ],
    );
    write_jsonl(
        &pricing.0.join("talents/pricing/non-object-usage.jsonl"),
        &[
            request_event("non-object-usage", "20260403", "pricing", None),
            json!({"event":"start", "model":"gpt-5.5"}),
            json!({"event":"finish", "usage":[]}),
        ],
    );
    write_jsonl(
        &pricing
            .0
            .join("talents/pricing/version-without-model.jsonl"),
        &[
            request_event("version-without-model", "20260403", "pricing", None),
            json!({"event":"start", "provider":"openai"}),
            json!({"event":"finish", "usage":{"input_tokens":1,"output_tokens":1,"model_version":"gpt-5"}}),
        ],
    );
    let app = router(pricing.0.clone());
    for use_id in [
        "null-usage",
        "empty-usage",
        "non-object-usage",
        "version-without-model",
    ] {
        let (status, body) = get(
            app.clone(),
            &format!("/app/thinking/api/run/{use_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{use_id}: {body}");
        assert!(body.get("cost").is_none(), "{use_id}: {body}");
    }
}

#[tokio::test]
async fn ac4_run_detail_unparseable_shapes_distinguish_reason_codes() {
    for (label, bytes, reason) in [
        ("empty", &b""[..], "talent_run_malformed"),
        ("invalid", b"not-json\n".as_slice(), "talent_run_malformed"),
        (
            "non-utf8",
            [0xff, 0xfe].as_slice(),
            "talent_operation_failed",
        ),
    ] {
        let fixture = Fixture::new();
        fixture.established();
        fs::create_dir_all(fixture.0.join("talents/shape")).expect("shape dir");
        fs::write(fixture.0.join("talents/shape/shape-id.jsonl"), bytes).expect("shape run");
        let (status, body) = get(
            router(fixture.0.clone()),
            "/app/thinking/api/run/shape-id",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{label}: {body}");
        assert_eq!(body["reason_code"], reason, "{label}: {body}");
    }
}

#[tokio::test]
async fn ac4_run_detail_missing_or_wrong_use_id_is_not_found() {
    for (label, first_line) in [
        ("missing", r#"{"event":"request"}"#),
        ("wrong", r#"{"event":"request","use_id":"other"}"#),
    ] {
        let fixture = Fixture::new();
        fixture.established();
        fs::create_dir_all(fixture.0.join("talents/only")).expect("dir");
        fs::write(fixture.0.join("talents/only/run-x.jsonl"), first_line).expect("run");
        let (status, body) = get(
            router(fixture.0.clone()),
            "/app/thinking/api/run/run-x",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{label}: {body}");
        assert_eq!(body["reason_code"], "talent_not_found", "{label}: {body}");
    }
}

#[tokio::test]
async fn ac4_decoy_does_not_mask_genuine_match_in_either_directory_order() {
    let wrong = serde_json::to_vec(&request_event("other", "20260403", "decoy", None)).unwrap();
    let cases: [(&str, &[u8], &str, bool, StatusCode); 3] = [
        ("5a", wrong.as_slice(), "run-x.jsonl", false, StatusCode::OK),
        ("5b", b"", "run-x.jsonl", false, StatusCode::OK),
        ("5c", b"", "run-x.jsonl", true, StatusCode::ACCEPTED),
    ];
    for (label, decoy, decoy_file, active, expected) in cases {
        let genuine_file = if active {
            "run-x_active.jsonl"
        } else {
            "run-x.jsonl"
        };
        for (decoy_dir, genuine_dir) in [("aaa", "zzz"), ("zzz", "aaa")] {
            let genuine = request_event("run-x", "20260403", genuine_dir, None);
            let fixture = Fixture::new();
            fixture.established();
            let decoy_path = fixture.0.join("talents").join(decoy_dir).join(decoy_file);
            fs::create_dir_all(decoy_path.parent().expect("parent")).expect("decoy dir");
            fs::write(&decoy_path, decoy).expect("decoy");
            write_jsonl(
                &fixture
                    .0
                    .join("talents")
                    .join(genuine_dir)
                    .join(genuine_file),
                std::slice::from_ref(&genuine),
            );
            let (status, body) = get(
                router(fixture.0.clone()),
                "/app/thinking/api/run/run-x",
                None,
            )
            .await;
            assert_eq!(
                status, expected,
                "{label} {decoy_dir}->{genuine_dir}: {body}"
            );
            if expected == StatusCode::OK {
                assert_eq!(body["name"], genuine_dir, "{label}: {body}");
            } else {
                assert_eq!(body["reason_code"], "talent_run_pending", "{label}: {body}");
            }
        }
    }
}

#[tokio::test]
async fn ac3_day_index_io_error_keeps_active_run() {
    let fixture = Fixture::new();
    fixture.established();
    let day = "20260403";
    fs::create_dir_all(fixture.0.join(format!("talents/{day}.jsonl"))).expect("index directory");
    write_jsonl(
        &fixture.0.join("talents/review/active_active.jsonl"),
        &[request_event("active", day, "review", Some("work"))],
    );
    let (status, body) = get(
        router(fixture.0.clone()),
        &format!("/app/thinking/api/talents/{day}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["uses"].as_array().expect("uses").len(), 1);
    assert_eq!(body["uses"][0]["status"], "running");
}

#[tokio::test]
async fn ac3_active_use_day_falls_back_to_use_id() {
    let fixture = Fixture::new();
    fixture.established();
    let timestamp = 1_710_030_600_000i64;
    let local_day = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp)
        .expect("timestamp")
        .with_timezone(&chrono::Local)
        .format("%Y%m%d")
        .to_string();
    let mut no_day = request_event_at("1710030600000", "unused", "fallback", None, timestamp);
    no_day.as_object_mut().expect("request event").remove("day");
    write_jsonl(
        &fixture
            .0
            .join("talents/fallback/1710030600000_active.jsonl"),
        &[no_day],
    );
    let mut invalid_id = request_event("not-a-number", "unused", "invalid", None);
    invalid_id
        .as_object_mut()
        .expect("request event")
        .remove("day");
    write_jsonl(
        &fixture.0.join("talents/fallback/not-a-number_active.jsonl"),
        &[invalid_id],
    );

    let (status, body) = get(
        router(fixture.0.clone()),
        &format!("/app/thinking/api/talents/{local_day}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["uses"].as_array().expect("uses").len(), 1);
    assert_eq!(body["uses"][0]["id"], "1710030600000");
}

#[tokio::test]
async fn ac3_facet_query_cookie_and_empty_precedence() {
    let fixture = Fixture::new();
    fixture.established();
    let day = "20260403";
    write_jsonl(
        &fixture.0.join(format!("talents/{day}.jsonl")),
        &[
            json!({"use_id":"work", "name":"work", "facet":"work", "status":"completed", "ts":3}),
            json!({"use_id":"personal", "name":"personal", "facet":"personal", "status":"completed", "ts":2}),
            json!({"use_id":"empty", "name":"empty", "facet":"", "status":"completed", "ts":1}),
        ],
    );
    let app = router(fixture.0.clone());
    let (_, query) = get(
        app.clone(),
        &format!("/app/thinking/api/talents/{day}?facet=work"),
        Some("selectedFacet=personal"),
    )
    .await;
    assert_eq!(query["uses"][0]["id"], "work");
    let (_, cookie) = get(
        app.clone(),
        &format!("/app/thinking/api/talents/{day}"),
        Some("selectedFacet=personal"),
    )
    .await;
    assert_eq!(cookie["uses"].as_array().expect("uses").len(), 3);
    let (_, empty) = get(
        app,
        &format!("/app/thinking/api/talents/{day}?facet="),
        Some("selectedFacet=work"),
    )
    .await;
    assert_eq!(empty["uses"][0]["id"], "empty");
}

#[tokio::test]
async fn ac3_facet_metadata_preserves_explicit_empty_title() {
    let fixture = Fixture::new();
    fixture.established();
    write_json(
        &fixture.0.join("facets/absent-title/facet.json"),
        json!({"color":"blue"}),
    );
    write_json(
        &fixture.0.join("facets/empty-title/facet.json"),
        json!({"title":"","color":"green"}),
    );

    let (status, body) = get(
        router(fixture.0.clone()),
        "/app/thinking/api/talents/20260403",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["facets"]["absent-title"]["title"], "absent-title");
    assert_eq!(body["facets"]["empty-title"]["title"], "");
}

#[tokio::test]
async fn ac5_output_oracle_and_containment() {
    let fixture = Fixture::new();
    seed_populated(&fixture, 3);
    let facets_output = fixture.0.join("facets/work/activities/facet-output.json");
    fs::create_dir_all(facets_output.parent().expect("facet parent")).expect("facet parent");
    fs::write(&facets_output, "{\"facet\":true}\n").expect("facet output");
    fs::write(
        fixture.0.join("chronicle/20260403/talents/case.JSON"),
        "{\"case\":true}\n",
    )
    .expect("upper JSON output");
    let app = router(fixture.0.clone());
    let (status, body) = get(
        app.clone(),
        "/app/thinking/api/output/20260403/talents/example-output.md",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, corpus_body("established_populated", "api_output"));
    let (status, body) = get(
        app.clone(),
        "/app/thinking/api/output/20260403/facets/work/activities/facet-output.json",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"content":"{\"facet\":true}\n", "format":"json", "filename":"facet-output.json"})
    );
    let (status, body) = get(
        app.clone(),
        "/app/thinking/api/output/20260403/talents/case.JSON",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"content":"{\"case\":true}\n", "format":"json", "filename":"case.JSON"})
    );
    let (status, body) = get(
        app.clone(),
        "/app/thinking/api/output/20260403/../../etc/passwd",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        body,
        corpus_body("established_populated", "api_output_traversal")
    );

    let empty = Fixture::new();
    empty.established();
    let (status, body) = get(
        router(empty.0.clone()),
        "/app/thinking/api/output/20260403/talents/example-output.md",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, corpus_body("established_empty", "api_output"));
    let (status, body) = get(
        router(empty.0.clone()),
        "/app/thinking/api/output/20260403/../../etc/passwd",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        body,
        corpus_body("established_empty", "api_output_traversal")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn ac5_run_output_path_and_fetch_containment_asymmetry() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    fixture.established();
    let outside = fixture.0.parent().expect("parent").join(format!(
        "outside-{}",
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&outside, "secret").expect("outside writes");
    let link = fixture.0.join("chronicle/20260403/talents/link.md");
    fs::create_dir_all(link.parent().expect("parent")).expect("link parent");
    let cross_day_target = fixture.0.join("chronicle/20260402/talents/across.md");
    fs::create_dir_all(cross_day_target.parent().expect("cross-day parent"))
        .expect("cross-day parent");
    fs::write(&cross_day_target, "# Cross-day output\n").expect("cross-day output");
    let cross_day_link = fixture
        .0
        .join("chronicle/20260403/talents/cross-day-link.md");
    symlink(&cross_day_target, &cross_day_link).expect("cross-day symlink");
    symlink(&outside, &link).expect("symlink");
    write_jsonl(
        &fixture.0.join("talents/daily/link-run.jsonl"),
        &[
            json!({"event":"request", "use_id":"link-run", "day":"20260403", "name":"daily", "ts":1, "output":"md", "output_path":link}),
            json!({"event":"finish", "usage":{"input_tokens":1,"output_tokens":1}}),
        ],
    );
    let app = router(fixture.0.clone());
    let (status, cross_day) = get(
        app.clone(),
        "/app/thinking/api/output/20260403/talents/cross-day-link.md",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        cross_day,
        json!({"content":"# Cross-day output\n", "format":"md", "filename":"across.md"})
    );
    let (status, run) = get(app.clone(), "/app/thinking/api/run/link-run", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(run["output_file"], "chronicle/20260403/talents/link.md");
    let (status, fetch) = get(
        app,
        "/app/thinking/api/output/20260403/talents/link.md",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        fetch,
        json!({"error":"that path couldn't be used.","reason_code":"invalid_path","detail":"Invalid path"})
    );
    let _ = fs::remove_file(outside);
}

#[tokio::test]
async fn ac5_derived_output_paths_use_env_stream() {
    let fixture = Fixture::new();
    fixture.established();
    let day = "20260403";
    let streamed = fixture
        .0
        .join("20260403/stream-a/segment-a/talents/streamed.md");
    let no_env = fixture.0.join("20260403/segment-a/talents/no-env.md");
    let active = fixture
        .0
        .join("20260403/stream-a/segment-a/talents/active-stream.md");
    for path in [&streamed, &no_env, &active] {
        fs::create_dir_all(path.parent().expect("output parent")).expect("output parent");
        fs::write(path, "output").expect("output");
    }
    let mut streamed_request = request_event("streamed", day, "streamed", None);
    streamed_request["output"] = json!("md");
    streamed_request["segment"] = json!("segment-a");
    streamed_request["env"] = json!({"SOL_STREAM":"stream-a"});
    write_jsonl(
        &fixture.0.join("talents/streamed/streamed.jsonl"),
        &[
            streamed_request,
            json!({"event":"finish", "usage":{"input_tokens":1,"output_tokens":1}}),
        ],
    );
    let mut no_env_request = request_event("no-env", day, "no-env", None);
    no_env_request["output"] = json!("md");
    no_env_request["segment"] = json!("segment-a");
    write_jsonl(
        &fixture.0.join("talents/no-env/no-env.jsonl"),
        &[
            no_env_request,
            json!({"event":"finish", "usage":{"input_tokens":1,"output_tokens":1}}),
        ],
    );
    let mut active_request = request_event("active-stream", day, "active-stream", None);
    active_request["output"] = json!("md");
    active_request["segment"] = json!("segment-a");
    active_request["env"] = json!({"SOL_STREAM":"stream-a"});
    write_jsonl(
        &fixture
            .0
            .join("talents/active-stream/active-stream_active.jsonl"),
        &[active_request],
    );

    let app = router(fixture.0.clone());
    let (_, streamed_run) = get(app.clone(), "/app/thinking/api/run/streamed", None).await;
    assert_eq!(
        streamed_run["output_file"],
        "stream-a/segment-a/talents/streamed.md"
    );
    let (_, no_env_run) = get(app.clone(), "/app/thinking/api/run/no-env", None).await;
    assert_eq!(no_env_run["output_file"], "segment-a/talents/no-env.md");
    let (_, uses) = get(app, "/app/thinking/api/talents/20260403", None).await;
    let active_use = uses["uses"]
        .as_array()
        .expect("uses")
        .iter()
        .find(|use_info| use_info["id"] == "active-stream")
        .expect("active use");
    assert_eq!(
        active_use["output_file"],
        "stream-a/segment-a/talents/active-stream.md"
    );
}

#[tokio::test]
async fn ac5_absolute_output_path_is_operation_failure() {
    let fixture = Fixture::new();
    fixture.established();
    let outside = PathBuf::from("/var/tmp").join(format!(
        "solstone-outside-{}",
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&outside, "outside").expect("outside writes");
    write_jsonl(
        &fixture.0.join("talents/daily/absolute.jsonl"),
        &[
            json!({"event":"request", "use_id":"absolute", "day":"20260403", "name":"daily", "ts":1, "output":"md", "output_path":outside}),
            json!({"event":"finish", "usage":{"input_tokens":1,"output_tokens":1}}),
        ],
    );
    let (status, body) = get(
        router(fixture.0.clone()),
        "/app/thinking/api/run/absolute",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["reason_code"], "talent_operation_failed");
    let _ = fs::remove_file(outside);
}

#[tokio::test]
async fn ac6_preview_wildcard_and_composed_conversation() {
    let fixture = Fixture::new();
    fixture.established();
    let app = router(fixture.0.clone());
    let (status, missing) = get(app.clone(), "/app/thinking/api/preview/a/b", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing["reason_code"], "talent_not_found");
    let (status, preview) = get(app, "/app/thinking/api/preview/conversation", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        preview["full_prompt"]
            .as_str()
            .expect("prompt")
            .contains("$activity_context")
    );

    let populated = Fixture::new();
    seed_populated(&populated, 3);
    let (status, preview) = get(
        router(populated.0.clone()),
        "/app/thinking/api/preview/conversation",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        preview["full_prompt"]
            .as_str()
            .expect("prompt")
            .contains("$activity_context")
    );
}

#[tokio::test]
async fn ac7_stats_directory_skip_and_shape_only_dates() {
    let fixture = Fixture::new();
    fixture.established();
    write_jsonl(
        &fixture.0.join("talents/20260403.jsonl"),
        &[json!({"use_id":"one", "facet":"work"})],
    );
    fs::create_dir_all(fixture.0.join("talents/20990101.jsonl")).expect("bad stats directory");
    let app = router(fixture.0.clone());
    let (status, stats) = get(app.clone(), "/app/thinking/api/stats/202604", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stats, corpus_body("stats_unparseable", "api_stats"));
    let (status, impossible_month) = get(app.clone(), "/app/thinking/api/stats/209913", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(impossible_month, json!({}));
    let (status, impossible_day) = get(app, "/app/thinking/api/talents/20260231", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(impossible_day["uses"], json!([]));

    let empty = Fixture::new();
    empty.established();
    let (status, stats) = get(
        router(empty.0.clone()),
        "/app/thinking/api/stats/202604",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stats, corpus_body("established_empty", "api_stats"));

    let populated = Fixture::new();
    seed_populated(&populated, 3);
    let (status, stats) = get(
        router(populated.0.clone()),
        "/app/thinking/api/stats/202604",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stats, corpus_body("established_populated", "api_stats"));
}

#[tokio::test]
async fn ac7_index_empty_and_populated_totals() {
    let empty = Fixture::new();
    empty.established();
    let (status, index) = get(router(empty.0.clone()), "/app/thinking/api/index", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(index, corpus_body("established_empty", "api_index"));

    let fixture = Fixture::new();
    seed_populated(&fixture, 3);
    let app = router(fixture.0.clone());
    let (status, index) = get(app.clone(), "/app/thinking/api/index", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        index,
        normalize_capture_index(corpus_body("established_populated", "api_index"))
    );

    write_jsonl(
        &fixture.0.join("talents/20260404.jsonl"),
        &[
            json!({"use_id":"none-one", "status":"completed"}),
            json!({"use_id":"none-two", "status":"completed", "facet":""}),
        ],
    );
    let (_, stats) = get(app.clone(), "/app/thinking/api/stats/202604", None).await;
    assert_eq!(stats["20260404"]["_none"], 2);
    let total = stats
        .as_object()
        .expect("stats days")
        .values()
        .flat_map(|facets| facets.as_object().expect("facets").values())
        .map(|count| count.as_u64().expect("count"))
        .sum::<u64>();
    let (_, index) = get(app, "/app/thinking/api/index", None).await;
    assert_eq!(index["months"]["202604"].as_u64(), Some(total));
}

#[tokio::test]
async fn ac8_badge_counts_only_failed_and_updated_days_excludes_today() {
    let fixture = Fixture::new();
    seed_populated(&fixture, 1);
    let capture = today();
    write_jsonl(
        &fixture.0.join("talents/running/today_active.jsonl"),
        &[request_event("today", &capture, "running", None)],
    );
    let app = router(fixture.0.clone());
    let (status, badge) = get(app.clone(), "/app/thinking/api/badge-count", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        badge,
        corpus_body("populated_single_failure", "api_badge_count")
    );

    marker(&fixture.0, "20260401", 20, 10);
    marker(&fixture.0, "20260402", 10, 10);
    marker(&fixture.0, "20260403", 10, 20);
    marker(&fixture.0, &capture, 10, 20);
    let (status, updated) = get(app, "/app/thinking/api/updated-days", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated, json!(["20260403"]));

    let populated = Fixture::new();
    seed_populated(&populated, 3);
    let populated_capture = today();
    marker(&populated.0, "20260214", 10, 20);
    marker(&populated.0, "20260315", 10, 20);
    marker(&populated.0, &populated_capture, 10, 20);
    let app = router(populated.0.clone());
    let (status, badge) = get(app.clone(), "/app/thinking/api/badge-count", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        badge,
        corpus_body("established_populated", "api_badge_count")
    );
    let (status, updated) = get(app, "/app/thinking/api/updated-days", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        updated,
        corpus_body("established_populated", "api_updated_days")
    );

    let empty = Fixture::new();
    empty.established();
    let (_, updated) = get(
        router(empty.0.clone()),
        "/app/thinking/api/updated-days",
        None,
    )
    .await;
    assert_eq!(
        updated,
        corpus_body("established_empty", "api_updated_days")
    );
    let (_, badge) = get(
        router(empty.0.clone()),
        "/app/thinking/api/badge-count",
        None,
    )
    .await;
    assert_eq!(badge, corpus_body("established_empty", "api_badge_count"));
}

fn marker(root: &Path, day: &str, daily_seconds: u64, stream_seconds: u64) {
    let health = root.join("chronicle").join(day).join("health");
    fs::create_dir_all(&health).expect("health");
    let daily = health.join("daily.updated");
    let stream = health.join("stream.updated");
    fs::write(&daily, "").expect("daily");
    fs::write(&stream, "").expect("stream");
    let base = 1_700_000_000i64;
    filetime::set_file_mtime(
        &daily,
        filetime::FileTime::from_unix_time(base + daily_seconds as i64, 0),
    )
    .expect("daily time");
    filetime::set_file_mtime(
        &stream,
        filetime::FileTime::from_unix_time(base + stream_seconds as i64, 0),
    )
    .expect("stream time");
}

#[tokio::test]
async fn deleted_identity_route_returns_not_found() {
    let fixture = Fixture::new();
    fixture.established();
    let (status, _body) = get(
        router(fixture.0.clone()),
        "/app/thinking/api/identity",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn session_gate_corrupt_and_unestablished_oracle() {
    let corrupt = Fixture::new();
    fs::create_dir_all(corrupt.0.join("config")).expect("config");
    fs::write(corrupt.0.join("config/journal.json"), "{not json\n").expect("corrupt");
    let (status, body) = get(router(corrupt.0.clone()), "/app/thinking/api/state", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["reason_code"], "corrupt_config");
    let unestablished = Fixture::new();
    let response = router(unestablished.0.clone())
        .oneshot(
            Request::get("/app/thinking/api/state")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(response.headers()[header::LOCATION], "/init");
}

#[test]
fn run_convey_from_isolated_dir_returns_diagnostic_without_panicking() {
    let isolated = tempfile::TempDir::new_in("/var/tmp").expect("isolated root");
    let bin = isolated.path().join("bin");
    fs::create_dir_all(&bin).expect("isolated bin");
    let journal = isolated.path().join("journal");
    fs::create_dir_all(&journal).expect("isolated journal");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        solstone_core_convey_shell::run_convey_from_executable_dir(journal, 5015, &bin)
    }));
    let result = caught.expect("run_convey_from_executable_dir must not panic");
    let error = result.expect_err("isolated executable dir must miss");
    assert!(
        error.contains(&bin.display().to_string()),
        "diagnostic must name the executable dir: {error}"
    );
}

#[tokio::test]
async fn native_request_rows_without_event_or_ts_remain_readable() {
    let fixture = Fixture::new();
    fixture.established();
    let start = 1788639954771i64;
    for (offset, ending) in [(0, "finish"), (10000, "error")] {
        let id = (start + offset).to_string();
        write_jsonl(
            &fixture.0.join(format!("talents/participation/{id}.jsonl")),
            &[
                json!({"name":"participation", "use_id":id, "day":"20260905", "prompt":"review"}),
                json!({"event":"start", "ts":start + offset + 10, "model":"test-model"}),
                json!({"event":ending, "ts":start + offset + 2000, "reason_code":"context_window_exceeded", "error":"context full"}),
            ],
        );
        let (status, body) = get(
            router(fixture.0.clone()),
            &format!("/app/thinking/api/run/{id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["start"], start + offset);
        assert_eq!(body["runtime_seconds"], 2.0);
        assert_eq!(body["failed"], ending == "error");
        if ending == "error" {
            assert_eq!(body["reason_code"], "context_window_exceeded");
        }
    }
    let active_id = (start + 20000).to_string();
    write_jsonl(
        &fixture
            .0
            .join(format!("talents/participation/{active_id}_active.jsonl")),
        &[json!({"name":"participation", "use_id":active_id, "day":"20260905"})],
    );
    let (status, body) = get(
        router(fixture.0.clone()),
        "/app/thinking/api/talents/20260905",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let active = body["uses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == active_id)
        .expect("native active request visible");
    assert_eq!(active["start"], start + 20000);
    assert_eq!(active["status"], "running");
    let (status, _) = get(
        router(fixture.0.clone()),
        &format!("/app/thinking/api/run/{active_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    write_jsonl(
        &fixture.0.join("talents/participation/mismatch.jsonl"),
        &[json!({"name":"other", "use_id":"mismatch", "day":"20260905"})],
    );
    let (status, _) = get(
        router(fixture.0.clone()),
        "/app/thinking/api/run/mismatch",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}
