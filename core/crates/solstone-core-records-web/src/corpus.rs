// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};

use rusqlite::{Connection, params};
use serde_json::{Value, json};
use solstone_core_indexer_store::db::open_index;
use tower::ServiceExt;

use crate::api_router;

const DROPPED_SEARCH_FIELDS: &[&str] = &[
    "agent_icon_svg",
    "icon_svg",
    "agent_icon",
    "facet_color",
    "facet_emoji",
    "day_grid",
    "showing_days",
    "has_more",
];

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn populated_corpus_replays_every_non_deviation_probe() {
    with_utc(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async {
                let corpus = corpus();
                let fixture = seeded_journal();
                let today = today_day();
                let mut asserted = 0;
                let mut skipped = 0;
                for probe in corpus["phases"]["populated"]["search"]
                    .as_array()
                    .expect("probe array")
                {
                    let path = probe["path"].as_str().expect("probe path");
                    if path.starts_with("/app/search/api/day_results") {
                        skipped += 1;
                        continue;
                    }
                    let response = request(&fixture.root, &path.replace("<TODAY>", &today)).await;
                    assert_eq!(
                        response.status(),
                        probe["status"].as_u64().expect("status") as u16,
                        "{}",
                        probe["why"]
                    );
                    if !response.status().is_success() && probe["json"].is_object() {
                        assert_eq!(
                            response_json(response).await,
                            probe["json"],
                            "{} error body",
                            probe["why"]
                        );
                    }
                    asserted += 1;
                }
                let established = Fixture::established();
                for probe in corpus["phases"]["established"]["search"]
                    .as_array()
                    .expect("probe array")
                {
                    let router = solstone_core_convey_shell::session_gate::apply_layer(
                        api_router(established.root.clone()),
                        established.root.clone(),
                    );
                    let response = router
                        .oneshot(
                            Request::get(probe["path"].as_str().expect("path"))
                                .body(Body::empty())
                                .expect("request"),
                        )
                        .await
                        .expect("response");
                    assert_eq!(response.status(), StatusCode::OK);
                    asserted += 1;
                }
                assert_eq!(asserted, 25);
                assert_eq!(skipped, 2);
            });
    });
}

#[tokio::test]
async fn established_unestablished_and_corrupt_fixture_probes_use_session_gate() {
    let corpus = corpus();
    for (phase, root) in [
        ("unestablished", Fixture::unestablished()),
        ("established", Fixture::established()),
        ("corrupt", Fixture::corrupt()),
    ] {
        for probe in corpus["phases"][phase]["search"]
            .as_array()
            .expect("probe array")
        {
            let router = solstone_core_convey_shell::session_gate::apply_layer(
                api_router(root.root.clone()),
                root.root.clone(),
            );
            let response = router
                .oneshot(
                    Request::get(probe["path"].as_str().expect("path"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(
                response.status(),
                probe["status"].as_u64().expect("status") as u16,
                "{phase}: {}",
                probe["why"]
            );
        }
    }
}

#[tokio::test]
async fn search_sentinel_bounds_return_results_and_reversed_bounds_fail() {
    let fixture = seeded_journal();
    let success = response_json(
        request(
            &fixture.root,
            "/app/search/api/search?q=needle&day_from=00000000&day_to=99999999",
        )
        .await,
    )
    .await;
    assert!(!success["days"].as_array().expect("days").is_empty());
    let failure = request(
        &fixture.root,
        "/app/search/api/search?q=needle&day_from=20260802&day_to=20260801",
    )
    .await;
    assert_eq!(failure.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn search_response_drops_only_the_declared_page_fields() {
    let fixture = seeded_journal();
    let response =
        response_json(request(&fixture.root, "/app/search/api/search?q=needle").await).await;
    assert!(!response["days"].as_array().expect("days").is_empty());
    assert_no_keys(&response, DROPPED_SEARCH_FIELDS);
}

#[tokio::test]
async fn read_cases_match_the_recorded_reason_codes() {
    let fixture = seeded_journal();
    for (path, status, reason) in [
        (
            "/app/search/api/read?path=20260731/talents/flow.md&agent=flow",
            StatusCode::BAD_REQUEST,
            "invalid_path",
        ),
        (
            "/app/search/api/read?path=entity_search:ada",
            StatusCode::BAD_REQUEST,
            "invalid_path",
        ),
        (
            "/app/search/api/read?path=20260731/talents/flow.md:3",
            StatusCode::BAD_REQUEST,
            "invalid_path",
        ),
        (
            "/app/search/api/read?path=20260731/talents/flow.md&max_bytes=1",
            StatusCode::BAD_REQUEST,
            "invalid_request_value",
        ),
        (
            "/app/search/api/read?path=../config/journal.json",
            StatusCode::BAD_REQUEST,
            "invalid_path",
        ),
        (
            "/app/search/api/read?path=20260731/talents/missing.md",
            StatusCode::NOT_FOUND,
            "file_not_found",
        ),
        (
            "/app/search/api/read?agent=missing&day=20260731",
            StatusCode::NOT_FOUND,
            "file_not_found",
        ),
    ] {
        let response = request(&fixture.root, path).await;
        assert_eq!(response.status(), status, "{path}");
        assert_eq!(response_json(response).await["reason_code"], reason);
    }
    for path in [
        "/app/search/api/read?path=20260731/talents/flow.md",
        "/app/search/api/read?agent=flow&day=20260731",
    ] {
        let response = response_json(request(&fixture.root, path).await).await;
        assert_eq!(
            response,
            json!({"path": "20260731/talents/flow.md", "content": "# Daily flow\n\nSeeded daily output.\n"})
        );
    }
}

#[tokio::test]
async fn read_rejects_path_like_agent() {
    let fixture = seeded_journal();
    let response = request(
        &fixture.root,
        "/app/search/api/read?agent=..%2F..%2Fetc%2Fpasswd&day=20260731",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await["reason_code"],
        "invalid_request_value"
    );
}

#[tokio::test]
async fn agents_day_segment_and_invalid_segment_match_the_fixture_contract() {
    let fixture = seeded_journal();
    let day =
        response_json(request(&fixture.root, "/app/search/api/agents?day=20260731").await).await;
    assert_eq!(day["daily"], json!([{ "name": "flow.md", "bytes": 35 }]));
    assert_eq!(
        day["segments"][0]["outputs"],
        json!([{ "name": "flow.md", "bytes": 39 }])
    );
    let segment = response_json(
        request(
            &fixture.root,
            "/app/search/api/agents?day=20260731&segment=090000_300",
        )
        .await,
    )
    .await;
    assert_eq!(
        segment,
        json!({"day": "20260731", "segment": "090000_300", "outputs": []})
    );
    let invalid = request(
        &fixture.root,
        "/app/search/api/agents?day=20260731&segment=../x",
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(invalid).await["reason_code"],
        "invalid_request_value"
    );
}

#[tokio::test]
async fn agents_list_nested_daily_outputs_in_reference_order() {
    let fixture = seeded_journal();
    write(
        &fixture
            .root
            .join("chronicle/20260731/talents/nested/child.md"),
        "child\n",
    );
    write(
        &fixture.root.join("chronicle/20260731/talents/nested.md"),
        "nested\n",
    );
    let response =
        response_json(request(&fixture.root, "/app/search/api/agents?day=20260731").await).await;
    assert_eq!(
        response["daily"],
        json!([
            {"name": "flow.md", "bytes": 35},
            {"name": "nested/child.md", "bytes": 6},
            {"name": "nested.md", "bytes": 7},
        ])
    );
}

#[tokio::test]
async fn day_results_is_absent_while_search_remains_live() {
    let fixture = seeded_journal();
    assert_eq!(
        request(&fixture.root, "/app/search/api/day_results?q=needle")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let search =
        response_json(request(&fixture.root, "/app/search/api/search?q=needle").await).await;
    assert!(!search["days"].as_array().expect("days").is_empty());
}

#[tokio::test]
async fn search_document_routes_serve_the_shell_workspace_and_redirect() {
    let fixture = seeded_journal();
    let shell = request(&fixture.root, "/app/search/").await;
    assert_eq!(shell.status(), StatusCode::OK);
    let workspace = request(&fixture.root, "/app/search/workspace").await;
    assert_eq!(workspace.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(workspace.into_body(), usize::MAX)
            .await
            .expect("body")
            .to_vec(),
    )
    .expect("workspace UTF-8");
    assert!(body.contains("role=\"search\"") && body.contains("/app/search/api/search"));
    let bare = request(&fixture.root, "/app/search").await;
    assert!(bare.status().is_redirection());
    assert_eq!(
        bare.headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/app/search/")
    );
}

#[tokio::test]
async fn session_gate_protects_search_document_routes() {
    for (root, expected_status) in [
        (Fixture::unestablished(), StatusCode::FOUND),
        (Fixture::established(), StatusCode::OK),
        (Fixture::corrupt(), StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        for path in ["/app/search/"] {
            let response = solstone_core_convey_shell::session_gate::apply_layer(
                api_router(root.root.clone()),
                root.root.clone(),
            )
            .oneshot(Request::get(path).body(Body::empty()).expect("request"))
            .await
            .expect("response");
            assert_eq!(response.status(), expected_status, "{path}");
            if expected_status == StatusCode::FOUND {
                assert_eq!(
                    response
                        .headers()
                        .get("location")
                        .and_then(|value| value.to_str().ok()),
                    Some("/init")
                );
            }
        }
    }
}

fn corpus() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/convey_records_corpus.json"))
        .expect("records corpus parses")
}

async fn request(root: &Path, path: &str) -> axum::response::Response {
    api_router(root.to_path_buf())
        .oneshot(Request::get(path).body(Body::empty()).expect("request"))
        .await
        .expect("response")
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("JSON body")
}

fn assert_no_keys(value: &Value, forbidden: &[&str]) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                assert!(!forbidden.contains(&key.as_str()), "unexpected field {key}");
                assert_no_keys(value, forbidden);
            }
        }
        Value::Array(values) => values
            .iter()
            .for_each(|value| assert_no_keys(value, forbidden)),
        _ => {}
    }
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn unestablished() -> Self {
        Self::new("unestablished")
    }
    fn established() -> Self {
        let fixture = Self::new("established");
        write(
            &fixture.root.join("config/journal.json"),
            r#"{"setup":{"completed_at":1}}"#,
        );
        let connection = open_index(&fixture.root).expect("empty index opens");
        insert(&connection, "needle", "20260731", "flow", "field", 0);
        connection
            .execute(
                "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) VALUES (1, 1, 'complete', 0, 1)",
                [],
            )
            .expect("complete state");
        fixture
    }
    fn corrupt() -> Self {
        let fixture = Self::new("corrupt");
        write(&fixture.root.join("config/journal.json"), "not json");
        fixture
    }
    fn new(name: &str) -> Self {
        let root = PathBuf::from("/var/tmp").join(format!(
            "solstone-records-web-{name}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("root creates");
        Self { root }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn seeded_journal() -> Fixture {
    let fixture = Fixture::established();
    write(&fixture.root.join("config/journal.json"), &json!({"setup":{"completed_at":1}, "identity":{"preferred":"Corpus Owner", "name":"Corpus Owner"}, "agent":{"name":"Corpus Assistant"}}).to_string());
    write(
        &fixture.root.join("config/chat.json"),
        r#"{"thinking_surfaces":"on_tap"}"#,
    );
    write(
        &fixture.root.join("facets/work/facet.json"),
        r##"{"title":"Work","color":"#336699","emoji":"🧪"}"##,
    );
    let today = today_day();
    let base = chrono::NaiveDate::parse_from_str(&today, "%Y%m%d")
        .expect("today")
        .and_hms_opt(9, 0, 0)
        .expect("time")
        .and_utc()
        .timestamp_millis();
    let events = [
        json!({"kind":"owner_message","ts":base,"text":"Please summarize the seeded record.","app":"chat","path":format!("/app/chat/{today}"),"facet":"work"}),
        json!({"kind":"sol_chat_request","ts":base+1000,"request_id":"seeded-threaded-request","summary":"Offer a seeded follow-up.","message":"A seeded request is ready.","category":"follow_up","dedupe":"seeded-threaded","dedupe_window":"24h","since_ts":base,"trigger_talent":"flow"}),
        json!({"kind":"sol_message","ts":base+2000,"use_id":"seeded-sol-use","text":"Here is the seeded response.","notes":"Seeded notes.","requested_target":null,"requested_task":null}),
        json!({"kind":"sol_chat_request_superseded","ts":base+3000,"request_id":"seeded-threaded-request","replaced_by":"seeded-unresolved-request"}),
        json!({"kind":"owner_chat_open","ts":base+4000,"request_id":"seeded-unresolved-request","surface":"chat"}),
        json!({"kind":"talent_queued","ts":base+5000,"use_id":"seeded-talent-use","name":"flow","task":"Inspect seeded records","queued_at":base+5000,"chat_use_id":"seeded-sol-use","ask":"Inspect seeded records","context":"seed corpus","location":"chat"}),
        json!({"kind":"talent_spawned","ts":base+6000,"use_id":"seeded-talent-use","name":"flow","task":"Inspect seeded records","started_at":base+6000}),
        json!({"kind":"talent_finished","ts":base+7000,"use_id":"seeded-talent-use","name":"flow","summary":"Seeded inspection completed."}),
        json!({"kind":"sol_chat_request","ts":base+8000,"request_id":"seeded-unresolved-request","summary":"An unresolved seeded request.","message":"Please open this unresolved request.","category":"follow_up","dedupe":"seeded-unresolved","dedupe_window":"24h","since_ts":base+7000,"trigger_talent":"flow"}),
    ]
    .iter()
    .map(serde_json::to_string)
    .collect::<Result<Vec<_>, _>>()
    .expect("events")
    .join("\n")
        + "\n";
    write(
        &fixture
            .root
            .join(format!("chronicle/{today}/chat/090000_300/chat.jsonl")),
        &events,
    );
    write(
        &fixture
            .root
            .join("chronicle/20260731/chat/140000_300/chat.jsonl"),
        "{\"kind\":\"owner_message\",\"ts\":1785476400000,\"text\":\"Historical seeded chat.\",\"app\":\"chat\",\"path\":\"/app/chat/20260731\",\"facet\":\"work\"}\n{\"kind\":\"sol_message\",\"ts\":1785476401000,\"use_id\":\"seeded-historical-use\",\"text\":\"Historical seeded response.\",\"notes\":\"\",\"requested_target\":null,\"requested_task\":null}\n",
    );
    write(
        &fixture
            .root
            .join("chronicle/20260916/chat/090000_300/chat.jsonl"),
        "{ invalid json\n",
    );
    write(
        &fixture.root.join("chronicle/20260731/talents/flow.md"),
        "# Daily flow\n\nSeeded daily output.\n",
    );
    write(
        &fixture
            .root
            .join("chronicle/20260731/field/090000_300/talents/flow.md"),
        "# Segment flow\n\nSeeded segment output.\n",
    );
    seed_index(&fixture.root);
    fixture
}

fn seed_index(root: &Path) {
    let connection = open_index(root).expect("index opens");
    for (content, day, agent, stream, index) in [
        ("needle result", "20260731", "flow", "field", 0),
        (
            "yesterday's meeting needle",
            "20260715",
            "meetings",
            "field",
            1,
        ),
        (
            "locale-probe captures a deterministic day",
            "20260731",
            "flow",
            "field",
            2,
        ),
        ("O'Brien and dogs", "20260731", "flow", "field", 3),
        (
            "nebula appears after relaxed matching",
            "20260801",
            "flow",
            "field",
            4,
        ),
    ] {
        insert(&connection, content, day, agent, stream, index);
    }
    connection.execute("REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) VALUES (1, 1, 'complete', 0, 5)", []).expect("complete state");
}

fn insert(
    connection: &Connection,
    content: &str,
    day: &str,
    agent: &str,
    stream: &str,
    index: i64,
) {
    connection.execute("INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES (?1, ?2, ?3, 'work', ?4, ?5, ?6, 'morning')", params![content, format!("{day}/{stream}/seed.md"), day, agent, stream, index]).expect("chunk inserts");
}

fn write(path: &Path, source: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("parent creates");
    fs::write(path, source).expect("file writes");
}
fn today_day() -> String {
    chrono::Local::now().format("%Y%m%d").to_string()
}

fn with_utc<F: FnOnce()>(body: F) {
    temp_env::with_var("TZ", Some("UTC"), body);
}

#[tokio::test]
async fn matched_entry_read_keeps_large_source_bounded_and_checks_reference() {
    let fixture = seeded_journal();
    let result =
        response_json(request(&fixture.root, "/app/search/api/search?q=needle").await).await;
    let hit = &result["days"][0]["results"][0];
    let path = hit["path"].as_str().unwrap();
    write(
        &fixture.root.join("chronicle").join(path),
        &"x".repeat(20000),
    );
    let query = format!(
        "/app/search/api/entry?path={path}&idx={}&entry_id={}",
        hit["idx"], hit["entry_id"]
    );
    let response = request(&fixture.root, &query).await;
    assert_eq!(response.status(), StatusCode::OK);
    let content = response_json(response).await;
    assert!(content["content"].as_str().unwrap().contains("needle"));
    assert_eq!(
        request(&fixture.root, &format!("/app/search/api/read?path={path}"))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&fixture.root, &query.replace("entry_id=", "entry_id=999"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &fixture.root,
            "/app/search/api/entry?path=x&idx=-1&entry_id=1"
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
}
