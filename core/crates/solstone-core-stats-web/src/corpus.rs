// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Captured stats/tokens contract replay through the composed Convey router.

#[path = "../../solstone-core-health-web/src/test_support.rs"]
// The imported fixture helper exposes phase builders unused by this crate's focused cases.
#[allow(dead_code)]
mod test_support;

use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use chrono::{TimeZone, Utc};
use serde_json::Value;
use std::fs;
use tower::ServiceExt;

const REPLAYED_TOKEN_CASES: [&str; 3] = ["api_usage", "api_index", "api_stats"];
const REPLAYED_STATS_CASES: [&str; 3] = ["index", "background", "api_stats"];
const STATS_PRESENTATION_DISPOSITIONS: [(&str, Disposition); 2] = [
    ("workspace", Disposition::SupersededByMergedCard),
    ("static_dashboard_js", Disposition::SupersededByMergedCard),
];
const PRESENTATION_DISPOSITIONS: [(&str, Disposition); 4] = [
    ("index", Disposition::SupersededByMergedCard),
    ("day", Disposition::SupersededByMergedCard),
    ("workspace", Disposition::SupersededByMergedCard),
    ("background", Disposition::SupersededByMergedCard),
];
const DAILY_DISPOSITION: Disposition = Disposition::NotRetainedNoDailyPort;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Disposition {
    SupersededByMergedCard,
    NotRetainedNoDailyPort,
}

fn fixture() -> Value {
    serde_json::from_slice(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/convey_stats_tokens_corpus.json"
    )))
    .expect("stats corpus")
}

#[test]
fn ac17_replays_all_retained_stats_and_token_api_cases_through_shell() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let fixture = fixture();
            let mut replayed = 0;
            for (phase_name, phase) in fixture["phases"].as_object().expect("phases") {
                let root = test_support::phase_root(phase_name);
                let router = solstone_core_convey_shell::router(root.path().to_path_buf());
                for case in phase["stats"].as_array().expect("stats cases") {
                    if REPLAYED_STATS_CASES.contains(&case["name"].as_str().expect("name")) {
                        assert_case(router.clone(), case, phase_name, &root).await;
                        replayed += 1;
                    }
                }
                for case in phase["tokens"]
                    .as_array()
                    .expect("token cases")
                    .iter()
                    .filter(|case| {
                        REPLAYED_TOKEN_CASES.contains(&case["name"].as_str().expect("name"))
                    })
                {
                    let mut mapped = case.clone();
                    let path = mapped["path"].as_str().expect("token path").replacen(
                        "/app/tokens/api/",
                        "/app/stats/api/",
                        1,
                    );
                    mapped["path"] = Value::String(path);
                    assert_case(router.clone(), &mapped, phase_name, &root).await;
                    replayed += 1;
                }
            }
            assert_eq!(replayed, 42);
        });
}

async fn assert_case(router: axum::Router, case: &Value, phase: &str, root: &tempfile::TempDir) {
    let path = case["path"].as_str().expect("path");
    let response = router
        .oneshot(Request::get(path).body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let expected = &case["response"];
    assert_eq!(
        response.status().as_u16(),
        expected["status"].as_u64().expect("status") as u16,
        "{phase} {}",
        case["name"]
    );
    for header in ["content-type", "location", "set-cookie"] {
        if let Some(value) = expected["headers"].get(header).and_then(Value::as_str) {
            assert_eq!(
                response
                    .headers()
                    .get(header)
                    .and_then(|value| value.to_str().ok()),
                Some(value),
                "{phase} {} header {header}",
                case["name"]
            );
        }
    }
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let mut actual = if expected["body"].is_string() {
        Value::String(String::from_utf8(bytes.to_vec()).expect("text"))
    } else {
        serde_json::from_slice(&bytes).expect("json")
    };
    let mut wanted = expected["body"].clone();
    replace_text(
        &mut wanted,
        "these days stopped on their own and can't pick back up without you — here's why, and what to try.",
        "some days retry automatically; others need your help. each day shows its current status.",
    );
    if expected["status"] == 200 && case["name"] == "index" {
        wanted = Value::String(
            include_str!("../../solstone-core-convey-shell/assets/static/shell.html").into(),
        );
    }

    // These captured token routes formerly returned estimated dollars. Their
    // status/error contracts remain; successful bodies now expose measured tokens.
    if expected["status"] == 200 && path.contains("/app/stats/api/") {
        let populated = matches!(
            phase,
            "established_populated"
                | "populated_single_failure"
                | "stats_absent"
                | "stats_unparseable"
        );
        if path.contains("/usage?") {
            let measured: Value = serde_json::from_str(include_str!(
                "../../../fixtures/stats_measured_usage_contract.json"
            ))
            .unwrap();
            wanted = measured[if populated { "populated" } else { "empty" }].clone();
        } else if path.ends_with("/index") && populated {
            wanted["months"] = serde_json::json!({"202604":2400.0});
        } else if path.starts_with("/app/stats/api/stats/") && populated {
            wanted = serde_json::json!({"20260403":2400});
        }
    }

    if phase == "corrupt" {
        replace_text(
            &mut wanted,
            "/var/tmp/solstone-convey-system-corpus/corrupt",
            &root.path().display().to_string(),
        );
    }
    for selector in case["normalized"]
        .as_array()
        .expect("normalized")
        .iter()
        .filter_map(Value::as_str)
    {
        replace(&mut actual, "response.body", selector);
        replace(&mut wanted, "response.body", selector);
    }
    assert_eq!(actual, wanted, "{phase} {} {path}", case["name"]);
}

#[test]
fn ac17_fixture_census_and_non_replayed_dispositions_are_exhaustive() {
    let fixture = fixture();
    let mut replayed_stats = 0;
    let mut retained = 0;
    let mut presentation = 0;
    let mut daily = 0;
    for phase in fixture["phases"].as_object().expect("phases").values() {
        for case in phase["stats"].as_array().expect("stats") {
            let name = case["name"].as_str().expect("name");
            if REPLAYED_STATS_CASES.contains(&name) {
                replayed_stats += 1;
            } else {
                assert!(
                    STATS_PRESENTATION_DISPOSITIONS
                        .iter()
                        .any(|(known, _)| *known == name),
                    "unclassified stats presentation case {name}"
                );
                presentation += 1;
            }
        }
        for case in phase["tokens"].as_array().expect("tokens") {
            let name = case["name"].as_str().expect("name");
            if REPLAYED_TOKEN_CASES.contains(&name) {
                retained += 1;
            } else if name == "api_daily" {
                assert_eq!(DAILY_DISPOSITION, Disposition::NotRetainedNoDailyPort);
                daily += 1;
            } else {
                assert!(
                    PRESENTATION_DISPOSITIONS
                        .iter()
                        .any(|(known, _)| *known == name),
                    "unclassified presentation case {name}"
                );
                presentation += 1;
            }
        }
    }
    assert_eq!(
        (replayed_stats, retained, presentation, daily),
        (21, 21, 42, 7)
    );
    assert_eq!(replayed_stats + retained + presentation + daily, 91);
    let census = &fixture["coverage_limits"]["mutation_census"];
    assert_eq!(census["cases_that_actually_mutated"], 0);
    assert_eq!(census["mutating_method_cases"], 0);
    assert!(
        census["routes_with_a_successful_mutation"]
            .as_object()
            .expect("routes")
            .is_empty()
    );
    assert_eq!(census["total_cases"], 91);
}

#[test]
fn stats_background_is_composed_html_404_not_unconverted_refusal() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let root = test_support::phase_root("established_empty");
            let response = solstone_core_convey_shell::router(root.path().to_path_buf())
                .oneshot(
                    Request::get("/app/stats/background")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status().as_u16(), 404);
            let body = String::from_utf8(
                to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body")
                    .to_vec(),
            )
            .expect("text");
            assert!(!body.contains("app_not_converted"));
        });
}

#[test]
fn ac7_filename_day_fold_and_stats_rollup_agree() {
    // 1786321800 is 2026-08-10 00:30 UTC. Token-usage days are the
    // filename stem, so the live fold and the stats rollup both land
    // on 20260809.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let root = test_support::root();
            fs::create_dir_all(root.path().join("tokens")).expect("tokens directory");
            fs::write(
                root.path().join("tokens/20260809.jsonl"),
                "{\"model\":\"gpt-5.5\",\"timestamp\":1786321800,\"usage\":{\"input_tokens\":100,\"output_tokens\":11,\"total_tokens\":111}}\n{\"model\":\"unknown-model\",\"timestamp\":1786321800,\"usage\":{\"input_tokens\":20,\"output_tokens\":17,\"total_tokens\":37}}\n",
            )
            .expect("token fixture");
            let router = solstone_core_convey_shell::router(root.path().to_path_buf());
            let usage = |path: &str| {
                let path = path.to_owned();
                let router = router.clone();
                async move {
                let response = router
                    .oneshot(Request::get(path).body(Body::empty()).expect("request"))
                    .await
                    .expect("response");
                serde_json::from_slice::<Value>(
                    &to_bytes(response.into_body(), usize::MAX)
                        .await
                        .expect("body"),
                )
                .expect("json")
                }
            };
            let filename_day = usage("/app/stats/api/usage?day=20260809").await;
            assert_eq!(filename_day["total"]["tokens"], 148);
            assert!(filename_day["total"].get("skipped_unknown").is_none());
            assert_eq!(usage("/app/stats/api/usage?day=20260810").await["total"]["tokens"], 0);
            let by_day = solstone_core_journal_stats_cli::scan_token_usage_by_day(
                root.path(),
                Utc.with_ymd_and_hms(2026, 8, 10, 1, 0, 0).unwrap(),
            );
            assert!(by_day.contains_key("20260809"));
            assert!(!by_day.contains_key("20260810"));
            let filename_total = by_day["20260809"]
                .values()
                .filter_map(|counts| counts.get("total_tokens"))
                .sum::<i64>();
            assert_eq!(filename_total, 148);
        });
}

#[test]
fn merged_card_declares_each_state_and_day_scoped_selection_contract() {
    let script = include_str!("../assets/static/token-card.js");
    let stats_script = include_str!("../assets/static/dashboard.js");
    assert!(script.contains("stats:token-rollup"));
    assert!(stats_script.contains("dispatchEvent(new CustomEvent('stats:token-rollup'"));
    assert!(script.contains("/app/stats/api/index"));
    assert!(script.contains("history.pushState"));
    assert!(script.contains("heading.focus()"));
    assert!(!script.contains("window.location.href"));
}

#[test]
fn merged_card_mounts_the_scoped_date_nav_on_its_declared_host() {
    let workspace = include_str!("../assets/workspace.html");
    let script = include_str!("../assets/static/token-card.js");
    assert!(workspace.contains("id=\"statsDateNav\""));
    assert!(script.contains("window.DateNav && window.DateNav.mountScoped({"));
    assert!(script.contains("host: dateNavHost,"));
    assert!(script.contains("apiBase: '/app/stats/',"));
    assert!(
        script
            .contains("onSelect: day => select(day, { push: true, focus: true, syncNav: false })")
    );
}

#[test]
fn card_loading_state_is_declared() {
    assert_card_state("loading");
}
#[test]
fn card_ready_state_is_declared() {
    assert_card_state("ready");
}
#[test]
fn card_empty_state_is_declared() {
    assert_card_state("empty");
}
#[test]
fn card_usage_error_state_is_declared() {
    assert_card_state("usage-error");
}
#[test]
fn card_index_error_state_is_declared() {
    assert_card_state("index-error");
}
fn assert_card_state(state: &str) {
    let script = include_str!("../assets/static/token-card.js");
    let assignment = match state {
        "loading" => "state('loading',",
        "ready" => "state('ready',",
        "empty" => "state('empty',",
        "usage-error" => "state('usage-error',",
        "index-error" => "state('index-error',",
        _ => unreachable!("known token-card state"),
    };
    assert!(script.contains(assignment), "card state assignment {state}");
}

#[test]
fn merged_workspace_retains_stats_and_contains_the_bounded_token_detail() {
    let workspace = include_str!("../assets/workspace.html");
    for retained in [
        "id=\"statsGrid\"",
        "id=\"audioChart\"",
        "id=\"heatmap\"",
        "id=\"facetsChart\"",
        "id=\"activitiesChart\"",
    ] {
        assert!(
            workspace.contains(retained),
            "retained stats structure {retained}"
        );
    }
    for merged in [
        "id=\"tokens\"",
        "id=\"tokenTypeComparison\"",
        "id=\"tokenProviderTable\"",
        "id=\"tokenModelTable\"",
        "class=\"token-table-scroll\"",
    ] {
        assert!(
            workspace.contains(merged),
            "merged token structure {merged}"
        );
    }
    for removed in [
        "token-type-body",
        "context-search",
        "segment-search",
        "sparkline",
    ] {
        assert!(
            !workspace.contains(removed),
            "superseded token structure {removed}"
        );
    }
}

fn replace(value: &mut Value, path: &str, pattern: &str) {
    if matches(path, pattern) {
        *value = Value::String("<NORMALIZED>".to_owned());
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                replace(value, &format!("{path}.{key}"), pattern);
            }
        }
        Value::Array(values) => {
            for value in values {
                replace(value, &format!("{path}.*"), pattern);
            }
        }
        _ => {}
    }
}
fn replace_text(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::String(text) => *text = text.replace(from, to),
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| replace_text(value, from, to)),
        Value::Object(values) => values
            .values_mut()
            .for_each(|value| replace_text(value, from, to)),
        _ => {}
    }
}
fn matches(path: &str, pattern: &str) -> bool {
    let path = path.split('.').collect::<Vec<_>>();
    let pattern = pattern.split('.').collect::<Vec<_>>();
    path.len() == pattern.len()
        && path
            .iter()
            .zip(pattern)
            .all(|(value, expected)| expected == "*" || value == &expected)
}
