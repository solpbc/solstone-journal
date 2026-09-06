// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, header},
};
use chrono::NaiveDateTime;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use crate::{
    routes,
    test_support::{corpus, fixed_clock, phase_root, write},
};

const CORPUS_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/convey_facets_corpus.json"
));

// Mirrors sol-client json_format::{sort_json, ensure_ascii}; copied for AC3/AC19 because those helpers are private and production code must not take a client dependency. Keep sort_json(v) -> serde_json::to_string(&sorted) -> ensure_ascii(&s).
fn sort_json(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sort_json).collect()),
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort();
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), sort_json(&object[key]));
            }
            Value::Object(sorted)
        }
        _ => value.clone(),
    }
}

fn ensure_ascii(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_ascii() {
            output.push(ch);
        } else {
            let codepoint = ch as u32;
            if codepoint <= 0xFFFF {
                output.push_str(&format!("\\u{codepoint:04x}"));
            } else {
                let adjusted = codepoint - 0x1_0000;
                let high = 0xD800 + (adjusted >> 10);
                let low = 0xDC00 + (adjusted & 0x3FF);
                output.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
            }
        }
    }
    output
}
fn canonical(value: &Value) -> Vec<u8> {
    ensure_ascii(&serde_json::to_string(&sort_json(value)).expect("JSON")).into_bytes()
}

fn rewrite_sol_urls_in_value(value: &mut Value) {
    match value {
        Value::String(text) => *text = rewrite_sol_urls(text),
        Value::Array(values) => {
            for value in values {
                rewrite_sol_urls_in_value(value);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                rewrite_sol_urls_in_value(value);
            }
        }
        _ => {}
    }
}

fn rewrite_sol_urls(text: &str) -> String {
    const PREFIX: &str = "/app/sol/";
    let mut rewritten = String::new();
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find(PREFIX) {
        let start = cursor + offset;
        rewritten.push_str(&text[cursor..start]);
        let path_start = start + PREFIX.len();
        let path_end = text[path_start..]
            .find(is_url_delimiter)
            .map_or(text.len(), |offset| path_start + offset);
        let path = &text[path_start..path_end];
        rewritten.push_str(&rewrite_sol_path(path).unwrap_or_else(|| format!("{PREFIX}{path}")));
        cursor = path_end;
    }
    rewritten.push_str(&text[cursor..]);
    rewritten
}

fn is_url_delimiter(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '"' | '\'' | ')' | ']' | '<' | '>')
}

fn rewrite_sol_path(path: &str) -> Option<String> {
    let parts = path.split('/').collect::<Vec<_>>();
    if matches!(parts.as_slice(), [day, "talents", "facet_newsletter"] if day_key(day)) {
        return Some(format!("/app/thinking/#runs/{}/facet_newsletter", parts[0]));
    }
    if let Some((day, fragment)) = path.split_once('#')
        && day_key(day)
    {
        let parts = fragment.split('/').collect::<Vec<_>>();
        return match parts.as_slice() {
            [talent] if !talent.is_empty() => Some(format!("/app/thinking/#runs/{day}/{talent}")),
            [talent, use_id] if !talent.is_empty() && !use_id.is_empty() => {
                Some(format!("/app/thinking/#runs/{day}/{talent}/{use_id}"))
            }
            _ => None,
        };
    }
    if day_key(path) {
        return Some(format!("/app/thinking/#runs/{path}"));
    }
    (!path.is_empty() && !path.contains('/')).then(|| format!("/app/thinking/#runs/run/{path}"))
}

fn day_key(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn substitute(value: &str, root: &Path) -> String {
    let root_text = root.display().to_string();
    let canonical = root
        .canonicalize()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| root_text.clone());
    value
        .replace(&canonical, "<JOURNAL_ROOT>")
        .replace(&root_text, "<JOURNAL_ROOT>")
}
fn normalize(value: &mut Value, root: &Path) {
    match value {
        Value::String(text) => *text = substitute(text, root),
        Value::Array(values) => values.iter_mut().for_each(|value| normalize(value, root)),
        Value::Object(values) => values.values_mut().for_each(|value| normalize(value, root)),
        _ => {}
    }
}

fn gated(root: &Path) -> Router {
    solstone_core_convey_shell::session_gate::apply_layer(
        routes(root.to_path_buf(), fixed_clock()),
        root.to_path_buf(),
    )
}

async fn replay_record(router: Router, root: &Path, expected: &Value) {
    let method = expected
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET");
    let mut request = Request::builder()
        .method(method)
        .uri(expected["path"].as_str().expect("path"));
    if expected.get("request_json").is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    let body = expected
        .get("request_json")
        .map(|value| Body::from(serde_json::to_vec(value).expect("request JSON")))
        .unwrap_or_else(Body::empty);
    let response = router
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("response");
    assert_eq!(
        response.status().as_u16(),
        expected["status"].as_u64().expect("status") as u16,
        "{}",
        expected["path"]
    );
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        expected["content_type"].as_str(),
        "{}",
        expected["path"]
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    // A record with no `body_sha256` is deliberately not body-asserted: served
    // frontend assets (html/js/css) are pinned by status, content type and
    // disposition only. See the corpus_maintenance note in the fixture.
    let Some(recorded_digest) = expected.get("body_sha256").and_then(Value::as_str) else {
        return;
    };
    let digest = if expected["body_sha256_basis"] == "raw-body" {
        Sha256::digest(substitute(std::str::from_utf8(&body).expect("UTF-8"), root).as_bytes())
    } else {
        let mut value: Value = serde_json::from_slice(&body).expect("JSON body");
        normalize(&mut value, root);
        // Preserve the captured contract except the deliberately revised facet prompt.
        if let Some(copy) = value.get_mut("copy").and_then(Value::as_object_mut)
            && let Some(prompt) = copy.get_mut("CUR_FACET_BODY")
        {
            assert_eq!(
                prompt,
                "journal activity doesn't fit your facets well. create the \"{name}\" facet?"
            );
            *prompt = Value::String("solstone noticed recent activity that doesn't fit your facets well. create a \"{name}\" facet?".into());
        }
        Sha256::digest(canonical(&value))
    };
    assert_eq!(
        format!("{digest:x}"),
        recorded_digest,
        "{}",
        expected["path"]
    );
}

#[tokio::test]
async fn ac1_replay_all_16_curation_phase_records() {
    let fixture = corpus();
    let paths = [
        "/app/curation/",
        "/app/curation/workspace",
        "/app/curation/static/curation_evidence.js",
        "/app/curation/api/state",
    ];
    let mut executed = 0;
    for (phase, records) in fixture["phases"].as_object().expect("phases") {
        let root = phase_root(phase);
        let router = gated(root.path());
        for expected in records
            .as_array()
            .expect("records")
            .iter()
            .filter(|record| {
                record["path"]
                    .as_str()
                    .is_some_and(|path| paths.contains(&path))
            })
        {
            replay_record(router.clone(), root.path(), expected).await;
            executed += 1;
        }
    }
    // /api/facet/candidates is intentionally excluded: solstone-core-entities owns it
    // and covers it in core/crates/solstone-core-entities/src/router_tests.rs.
    assert_eq!(executed, 16);
}

#[tokio::test]
async fn ac1_replay_all_40_awareness_phase_records_and_no_index() {
    let fixture = corpus();
    let mut executed = 0;
    for (phase, records) in fixture["phases"].as_object().expect("phases") {
        let root = phase_root(phase);
        let router = gated(root.path());
        for expected in records
            .as_array()
            .expect("records")
            .iter()
            .filter(|record| {
                record["path"]
                    .as_str()
                    .is_some_and(|path| path.starts_with("/app/awareness/"))
            })
        {
            let route = if expected["path"] == "/app/awareness/" {
                solstone_core_convey_shell::router(root.path().to_path_buf())
            } else {
                router.clone()
            };
            replay_record(route, root.path(), expected).await;
            executed += 1;
        }
        let response = solstone_core_convey_shell::router(root.path().to_path_buf())
            .oneshot(
                Request::get("/app/awareness/")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            axum::http::StatusCode::NOT_FOUND,
            "{phase}"
        );
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8",
            "{phase}"
        );
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body")
                .len(),
            207,
            "{phase}"
        );
    }
    assert_eq!(executed, 40);
}

#[tokio::test]
async fn ac1_replay_all_10_in_scope_mutation_records() {
    let fixture = corpus();
    let root = phase_root("populated");
    // The preceding five curation mutations are intentionally owned by
    // solstone-core-entities. Apply their durable facet state here so the one
    // in-scope curation read sees the same sequence state without registering
    // duplicate facet-candidate routes in this crate.
    solstone_core_facets::accept_candidate(root.path(), "atlas").expect("accept atlas");
    solstone_core_facets::dismiss_candidate(root.path(), "ledger").expect("dismiss ledger");
    let router = gated(root.path());
    let mut records = fixture["mutations"]
        .as_array()
        .expect("mutations")
        .iter()
        .filter(|record| {
            record["path"].as_str().is_some_and(|path| {
                path.starts_with("/app/awareness/") || path == "/app/curation/api/state"
            })
        })
        .collect::<Vec<_>>();
    records.sort_by_key(|record| record["sequence"].as_u64().expect("sequence"));
    for expected in &records {
        replay_record(router.clone(), root.path(), expected).await;
    }
    // The five other curation mutations target the descoped facet-candidate API, owned
    // and tested by solstone-core-entities/src/router_tests.rs.
    assert_eq!(records.len(), 10);
}

#[tokio::test]
async fn ac1_replay_all_20_activities_phase_records_and_14_mutations() {
    let fixture = corpus();
    let mut phase_records = 0;
    for (phase, records) in fixture["phases"].as_object().expect("phases") {
        let root = phase_root(phase);
        let router = gated(root.path());
        for expected in records
            .as_array()
            .expect("records")
            .iter()
            .filter(|record| {
                record["path"]
                    .as_str()
                    .is_some_and(|path| path.starts_with("/app/activities/api/"))
            })
        {
            replay_record(router.clone(), root.path(), expected).await;
            phase_records += 1;
        }
    }
    assert_eq!(phase_records, 20);

    let root = phase_root("populated");
    let router = gated(root.path());
    let created = fixture["mutations"]
        .as_array()
        .expect("mutations")
        .iter()
        .find_map(|record| {
            record["path"]
                .as_str()
                .filter(|path| path.ends_with("/records?facet=work"))
                .and_then(|_| record["json"]["record"]["id"].as_str())
        })
        .expect("created activity id");
    let mut mutations = fixture["mutations"]
        .as_array()
        .expect("mutations")
        .iter()
        .filter(|record| {
            record["path"]
                .as_str()
                .is_some_and(|path| path.starts_with("/app/activities/api/"))
        })
        .cloned()
        .collect::<Vec<_>>();
    mutations.sort_by_key(|record| record["sequence"].as_u64().expect("sequence"));
    for mut expected in mutations {
        let path = expected["path"]
            .as_str()
            .expect("path")
            .replace("{created}", created);
        expected["path"] = Value::String(path);
        replay_record(router.clone(), root.path(), &expected).await;
    }
    let activity_mutations = fixture["mutations"]
        .as_array()
        .expect("mutations")
        .iter()
        .filter(|record| {
            record["path"]
                .as_str()
                .is_some_and(|path| path.starts_with("/app/activities/api/"))
        })
        .collect::<Vec<_>>();
    // Deliberate, governed AC1 coverage count: the repeat probes are part of the contract.
    assert_eq!(activity_mutations.len(), 14);
    for (sequence, suffix, status) in [
        (10, "/records?facet=work", 409),
        (16, "/mute?facet=work", 200),
        (20, "/unmute?facet=work", 200),
    ] {
        assert!(activity_mutations.iter().any(|record| {
            record["sequence"].as_u64() == Some(sequence)
                && record["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with(suffix))
                && record["status"].as_u64() == Some(status)
        }));
    }
}

#[tokio::test]
async fn ac1_replay_all_120_news_records() {
    let legacy_url_count = CORPUS_SOURCE.matches("/app/sol/").count();
    assert_eq!(
        legacy_url_count, 1,
        "facets corpus legacy Sol URL count: expected 1, actual {legacy_url_count}"
    );
    let fixture = corpus();
    let mut executed = 0;
    for (phase, records) in fixture["phases"].as_object().expect("phases") {
        let root = phase_root(phase);
        let router = gated(root.path());
        for expected in records
            .as_array()
            .expect("records")
            .iter()
            .filter(|record| {
                record["path"]
                    .as_str()
                    .is_some_and(|path| path.starts_with("/app/news/"))
            })
        {
            executed += 1;
            let response = router
                .clone()
                .oneshot(
                    Request::get(expected["path"].as_str().expect("path"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(
                response.status().as_u16(),
                expected["status"].as_u64().expect("status") as u16,
                "{phase} {}",
                expected["path"]
            );
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
                expected["content_type"].as_str(),
                "{phase} {}",
                expected["path"]
            );
            assert_eq!(
                response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|value| value.to_str().ok()),
                expected.get("location").and_then(Value::as_str),
                "{phase} {}",
                expected["path"]
            );
            let path = expected["path"].as_str().expect("path");
            // Flask's static sender adds these on five static records: /app/news/,
            // /app/news/workspace, /app/news/sample, /app/news/20260510, and
            // /app/news/work/20260510. Only the PDF record is compared.
            if path == "/app/news/work/20260510/pdf" {
                assert_eq!(
                    response
                        .headers()
                        .get(header::CONTENT_DISPOSITION)
                        .and_then(|value| value.to_str().ok()),
                    expected.get("content_disposition").and_then(Value::as_str)
                );
            }
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            if expected["body_sha256_basis"] == "reference-bytes-record-only" {
                assert_eq!(
                    path, "/app/news/work/20260510/pdf",
                    "only the named PDF record is exempt from body hashing"
                );
                assert!(expected.get("body_sha256").is_none() || expected["body_sha256"].is_null());
                continue;
            }
            let digest = if expected["body_sha256_basis"] == "raw-body" {
                Sha256::digest(
                    substitute(std::str::from_utf8(&body).expect("UTF-8"), root.path()).as_bytes(),
                )
            } else {
                let mut value: Value = serde_json::from_slice(&body).expect("JSON body");
                normalize(&mut value, root.path());
                Sha256::digest(canonical(&value))
            };
            let mut recorded_body = expected["json"].clone();
            rewrite_sol_urls_in_value(&mut recorded_body);
            if path == "/app/news/api/state" && recorded_body.get("copy").is_some() {
                recorded_body["copy"]["populated_next_footer"] = serde_json::json!("");
                if recorded_body["copy"]["empty_next"]
                    .as_str()
                    .is_some_and(|text| text.contains("tomorrow"))
                {
                    recorded_body["copy"]["empty_next"] =
                        serde_json::json!(crate::news::copy::NEWS_EMPTY_PENDING);
                }
            }
            let expected_digest = if recorded_body != expected["json"] {
                Some(format!("{:x}", Sha256::digest(canonical(&recorded_body))))
            } else {
                expected
                    .get("body_sha256")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            if let Some(expected_digest) = expected_digest {
                assert_eq!(format!("{digest:x}"), expected_digest, "{phase} {path}");
            }
        }
    }
    assert_eq!(executed, 120);
}

async fn news_json(root: &Path, path: &str) -> (axum::http::StatusCode, Value) {
    let response = routes(root.to_path_buf(), fixed_clock())
        .oneshot(Request::get(path).body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    (
        status,
        serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON"),
    )
}

#[tokio::test]
async fn ac1b_news_clock_only_moves_grid_coverage() {
    let root = phase_root("populated");
    let first = routes(root.path().to_path_buf(), fixed_clock());
    let grid = first
        .clone()
        .oneshot(
            Request::get("/app/news/api/grid")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let index = first
        .oneshot(
            Request::get("/app/news/api/index")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let grid: Value =
        serde_json::from_slice(&to_bytes(grid.into_body(), usize::MAX).await.expect("body"))
            .expect("json");
    let index: Value =
        serde_json::from_slice(&to_bytes(index.into_body(), usize::MAX).await.expect("body"))
            .expect("json");
    let one_day_later = crate::Clock::new(|| {
        NaiveDateTime::parse_from_str("2026-05-16T12:00:00", "%Y-%m-%dT%H:%M:%S")
            .expect("one-day-later clock")
    });
    let later = routes(root.path().to_path_buf(), one_day_later)
        .oneshot(
            Request::get("/app/news/api/grid")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let later: Value =
        serde_json::from_slice(&to_bytes(later.into_body(), usize::MAX).await.expect("body"))
            .expect("json");
    assert_eq!(grid["coverage"]["end"], "20260515");
    assert_eq!(later["coverage"]["end"], "20260516");
    assert_eq!(index["coverage"]["end"], "20260510");
    let empty = phase_root("established_empty");
    for clock in [
        fixed_clock(),
        crate::Clock::new(|| {
            NaiveDateTime::parse_from_str("2026-05-16T12:00:00", "%Y-%m-%dT%H:%M:%S")
                .expect("one-day-later clock")
        }),
    ] {
        let router = routes(empty.path().to_path_buf(), clock);
        let grid = router
            .clone()
            .oneshot(
                Request::get("/app/news/api/grid")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let grid: Value =
            serde_json::from_slice(&to_bytes(grid.into_body(), usize::MAX).await.expect("body"))
                .expect("JSON");
        assert_eq!(grid["coverage"], Value::Null);
        let index = router
            .oneshot(
                Request::get("/app/news/api/index")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let index: Value =
            serde_json::from_slice(&to_bytes(index.into_body(), usize::MAX).await.expect("body"))
                .expect("JSON");
        assert_eq!(index["coverage"], Value::Null);
    }
}

#[tokio::test]
async fn ac2_missing_api_raw_pdf_have_distinct_contracts() {
    let root = phase_root("established_empty");
    let (status, value) = news_json(root.path(), "/app/news/api/work/20260510").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(value["empty"], true);
    for path in ["/app/news/work/20260510/raw", "/app/news/work/20260510/pdf"] {
        let response = routes(root.path().to_path_buf(), fixed_clock())
            .oneshot(Request::get(path).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
            "Newsletter not found"
        );
    }
}

#[tokio::test]
async fn ac3_ac4_ac5_paging_limits_and_facet_validation() {
    let root = phase_root("populated");
    write(&root.path().join("facets/work/news/20260426.md"), "third");
    let (_, first) = news_json(root.path(), "/app/news/api/facet/work?limit=1").await;
    assert_eq!(first["days"].as_array().expect("days").len(), 1);
    assert_eq!(first["has_more"], true);
    assert_eq!(first["next_cursor"], "20260510");
    let (_, second) = news_json(
        root.path(),
        "/app/news/api/facet/work?limit=1&cursor=20260510",
    )
    .await;
    assert_eq!(second["days"][0]["date"], "20260503");
    let (_, last) = news_json(
        root.path(),
        "/app/news/api/facet/work?limit=1&cursor=20260503",
    )
    .await;
    assert_eq!(last["has_more"], false);
    assert!(last["next_cursor"].is_null());
    for value in ["0", "101", "nope"] {
        let (status, body) = news_json(
            root.path(),
            &format!("/app/news/api/facet/work?limit={value}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(body["reason_code"], "invalid_request_value");
    }
    for value in ["1", "100"] {
        assert_eq!(
            news_json(
                root.path(),
                &format!("/app/news/api/facet/work?limit={value}")
            )
            .await
            .0,
            axum::http::StatusCode::OK
        );
    }
    let (status, body) = news_json(root.path(), "/app/news/api/facet/work?limit").await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "limit must be an integer");
    assert_eq!(
        news_json(root.path(), "/app/news/api/facet/work?limit=%31")
            .await
            .0,
        axum::http::StatusCode::OK
    );
    let (status, body) = news_json(root.path(), "/app/news/api/facet/work?limit=0&limit=1").await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "limit must be between 1 and 100");
    assert_eq!(
        news_json(root.path(), "/app/news/api/facet/nope").await.1["days"],
        serde_json::json!([])
    );
    let (status, body) = news_json(root.path(), "/app/news/api/facet/bad!facet").await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(body["reason_code"], "invalid_request_value");
}

#[tokio::test]
async fn ac6_ac7_ac15_ac16_state_and_observer_contracts() {
    let empty = phase_root("established_empty");
    let (_, state) = news_json(empty.path(), "/app/news/api/state").await;
    let mut keys = state
        .as_object()
        .expect("keys")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(keys, vec!["copy", "newsletters", "total_count"]);
    assert!(state["copy"]["grid_lede"].is_null());
    assert!(!crate::news::routes::journal_has_any_observer_input(
        empty.path()
    ));
    let one = phase_root("established_empty");
    write(&one.path().join("facets/work/news/20260401.md"), "one");
    let (_, state) = news_json(one.path(), "/app/news/api/state").await;
    assert_eq!(state["copy"]["grid_lede"], "1 newsletter since April 2026.");
    let many = phase_root("populated");
    write(&many.path().join("facets/work/news/20260401.md"), "old");
    let (_, state) = news_json(many.path(), "/app/news/api/state").await;
    assert_eq!(
        state["copy"]["grid_lede"],
        "4 newsletters since April 2026."
    );
    assert!(crate::news::routes::journal_has_any_observer_input(
        many.path()
    ));
    let capped = phase_root("established_empty");
    for number in 0..61 {
        write(
            &capped.path().join(format!(
                "facets/work/news/2025{:02}{:02}.md",
                1 + number / 28,
                1 + number % 28
            )),
            "n",
        );
    }
    let (_, state) = news_json(capped.path(), "/app/news/api/state").await;
    assert_eq!(state["newsletters"].as_array().expect("news").len(), 60);
    assert_eq!(state["total_count"], 61);
    let invalid_date = phase_root("established_empty");
    write(
        &invalid_date.path().join("facets/work/news/20261332.md"),
        "invalid date",
    );
    let (status, state) = news_json(invalid_date.path(), "/app/news/api/state").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(state["copy"]["grid_lede"], "1 newsletter since 20261332.");
}

#[tokio::test]
async fn ac8_live_pdf_ac10_frontmatter_and_ac11_dates() {
    let root = phase_root("populated");
    let response = routes(root.path().to_path_buf(), fixed_clock())
        .oneshot(
            Request::get("/app/news/work/20260510/pdf")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/pdf");
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"newsletter-work-20260510.pdf\""
    );
    let text = crate::pdf::writer::extract_text_checked(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("pdf"),
    );
    for line in [
        "FACET NEWSLETTER",
        "work · Sun May 10, 2026",
        "What happened",
        "A short newsletter body with a list:",
        "one item",
        "two item",
        "and a blockquote, because the PDF stylesheet has a rule for it.",
    ] {
        assert!(
            text.split_whitespace()
                .collect::<String>()
                .contains(&line.split_whitespace().collect::<String>()),
            "missing {line:?}"
        );
    }
    assert!(!text.contains("generated_at"));
    assert!(!text.contains("title:"));
    write(
        &root.path().join("facets/work/news/20260511.md"),
        "---\n\x01\n---\nbad",
    );
    let (status, body) = news_json(root.path(), "/app/news/api/work/20260511").await;
    assert_eq!(status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["reason_code"], "internal_error");
    for path in ["/app/news/work/20260511/raw", "/app/news/work/20260511/pdf"] {
        let response = routes(root.path().to_path_buf(), fixed_clock())
            .oneshot(Request::get(path).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
    }
}

#[tokio::test]
async fn ac12_ac13_ac14_invalid_path_contracts() {
    let root = phase_root("established_empty");
    let background = routes(root.path().to_path_buf(), fixed_clock())
        .oneshot(
            Request::get("/app/news/background")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(background.status(), axum::http::StatusCode::NOT_FOUND);
    assert_eq!(
        background.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    assert_eq!(
        to_bytes(background.into_body(), usize::MAX)
            .await
            .expect("body")
            .len(),
        207
    );
    let (status, bad) = news_json(root.path(), "/app/news/notaday").await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(bad["reason_code"], "invalid_day");
    for (path, code, status) in [
        (
            "/app/news/api/facet/work?cursor=nope",
            "invalid_request_value",
            axum::http::StatusCode::BAD_REQUEST,
        ),
        (
            "/app/news/api/facet/work?day=nope",
            "invalid_day",
            axum::http::StatusCode::BAD_REQUEST,
        ),
        (
            "/app/news/api/day/nope",
            "invalid_day",
            axum::http::StatusCode::NOT_FOUND,
        ),
    ] {
        let (actual, body) = news_json(root.path(), path).await;
        assert_eq!(actual, status);
        assert_eq!(body["reason_code"], code);
    }
}

#[tokio::test]
async fn ac1d_convey_shell_uses_converted_news_registry_row() {
    let root = phase_root("established_empty");
    let response = solstone_core_convey_shell::router(root.path().to_path_buf())
        .oneshot(
            Request::get("/app/news/nosuch/deep/path")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    assert_eq!(body.len(), 207);
    assert!(
        !std::str::from_utf8(&body)
            .expect("HTML")
            .contains("app_not_converted")
    );
}

#[tokio::test]
async fn ac1e_convey_shell_uses_converted_curation_registry_row() {
    let root = phase_root("established_empty");
    let response = solstone_core_convey_shell::router(root.path().to_path_buf())
        .oneshot(
            Request::get("/app/curation/nosuch/deep/path")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    assert_eq!(body.len(), 207);
    assert!(
        !std::str::from_utf8(&body)
            .expect("HTML")
            .contains("app_not_converted")
    );
}

#[test]
fn ac19_canonicalizer_matches_python_compact_ascii_and_surrogate_pairs() {
    let value = serde_json::json!({"z": "é", "a": "😀"});
    assert_eq!(
        String::from_utf8(canonical(&value)).expect("UTF-8"),
        r#"{"a":"\ud83d\ude00","z":"\u00e9"}"#
    );
}

#[tokio::test]
async fn ac5_ac6_ac11_shell_gate_and_fallback_contracts() {
    let established = phase_root("established_empty");
    // Unknown apps are exempt because known_app == None, unlike a gated known-app path below.
    let unknown = solstone_core_convey_shell::router(established.path().to_path_buf())
        .oneshot(
            Request::get("/app/nonexistent/")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unknown.status(), axum::http::StatusCode::NOT_FOUND);
    assert!(
        !to_bytes(unknown.into_body(), usize::MAX)
            .await
            .expect("body")
            .is_empty()
    );
    for path in [
        "/app/curation/",
        "/app/curation/workspace",
        "/app/curation/api/state",
    ] {
        for phase in ["unestablished", "corrupt", "established_empty"] {
            let root = phase_root(phase);
            let response = gated(root.path())
                .oneshot(Request::get(path).body(Body::empty()).expect("request"))
                .await
                .expect("response");
            let status = response.status();
            let location = response.headers().get(header::LOCATION).cloned();
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            if phase == "unestablished" {
                assert_eq!(status, axum::http::StatusCode::FOUND);
                assert_eq!(body.len(), 197);
                assert_eq!(
                    location.as_ref().and_then(|value| value.to_str().ok()),
                    Some("/init")
                );
            } else if phase == "corrupt" {
                assert_eq!(status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
                let is_api = path.contains("/api/");
                assert_eq!(
                    content_type,
                    if is_api {
                        "application/json"
                    } else {
                        "text/plain; charset=utf-8"
                    }
                );
                let actual = substitute(std::str::from_utf8(&body).expect("UTF-8"), root.path());
                let detail = "your settings file at <JOURNAL_ROOT>/config/journal.json couldn't be read. your settings were not changed. repair the file or restore config/journal.json from a backup, then try again.";
                let expected = if is_api {
                    format!(
                        r#"{{"error":"your settings couldn't be read.","reason_code":"corrupt_config","detail":"{detail}"}}"#
                    )
                } else {
                    detail.to_owned()
                };
                assert_eq!(actual, expected);
            } else {
                assert_eq!(status, axum::http::StatusCode::OK);
            }
        }
    }
}
