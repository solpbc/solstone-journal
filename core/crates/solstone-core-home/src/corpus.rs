// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{TimeZone, Utc};
use serde_json::{Value, json};

use crate::{
    briefing, formatting, health_glance,
    model::{BacklogSource, BacklogValidity},
    needs_you,
};

const CORPUS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/convey_home_corpus.json"
));

#[test]
fn replay_convey_home_corpus() {
    let source = std::str::from_utf8(CORPUS).expect("home corpus is UTF-8");
    let legacy_url_count = source.matches("/app/sol/").count();
    assert_eq!(
        legacy_url_count, 9,
        "home corpus legacy Sol URL count: expected 9, actual {legacy_url_count}"
    );
    let corpus: Value = serde_json::from_slice(CORPUS).unwrap();
    let groups = corpus["cases"].as_object().unwrap();
    let mut asserted = 0_usize;
    for case in groups["briefing_phase"].as_array().unwrap() {
        eq(
            &briefing::compute_phase(
                case["input"]["segment_count"].as_i64().unwrap(),
                case["input"]["hour"].as_u64().unwrap() as u32,
                case["input"]["briefing_exists"].as_bool().unwrap(),
            ),
            &case["output"],
            "briefing_phase",
        );
        asserted += 1;
    }
    for case in groups["briefing_lateness"].as_array().unwrap() {
        let now = Utc
            .with_ymd_and_hms(
                2026,
                5,
                14,
                case["input"]["now_hour"].as_u64().unwrap() as u32,
                case["input"]["now_minute"].as_u64().unwrap() as u32,
                0,
            )
            .unwrap();
        eq(
            &briefing::lateness_state(now, case["input"]["phase"].as_str().unwrap()),
            &case["output"],
            "briefing_lateness",
        );
        asserted += 1;
    }
    for case in groups["duration"].as_array().unwrap() {
        eq(
            &formatting::format_duration(case["input"]["total_minutes"].as_f64().unwrap()),
            &case["output"],
            "duration",
        );
        asserted += 1;
    }
    for case in groups["hour_label"].as_array().unwrap() {
        eq(
            &formatting::format_hour_label(
                case["input"]["start_hour"].as_i64().unwrap(),
                case["input"]["end_hour"].as_i64().unwrap(),
            ),
            &case["output"],
            "hour_label",
        );
        asserted += 1;
    }
    for case in groups["join_phrases"].as_array().unwrap() {
        let parts = case["input"]["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        eq(
            &formatting::join_phrases(&parts),
            &case["output"],
            "join_phrases",
        );
        asserted += 1;
    }
    for case in groups["activity_title"].as_array().unwrap() {
        eq(
            &formatting::normalize_activity_title(&case["input"]["record"]),
            &case["output"],
            "activity_title",
        );
        asserted += 1;
    }
    for case in groups["activity_label"].as_array().unwrap() {
        eq(
            &formatting::format_activity_label(&case["input"]["activity"]),
            &case["output"],
            "activity_label",
        );
        asserted += 1;
    }
    for case in groups["newsletter_summary"].as_array().unwrap() {
        eq(
            &formatting::format_newsletter_summary(
                case["input"]["successful"].as_i64().unwrap(),
                case["input"]["attempted"].as_i64().unwrap(),
            ),
            &case["output"],
            "newsletter_summary",
        );
        asserted += 1;
    }
    for case in groups["processing_summary"].as_array().unwrap() {
        eq(
            &formatting::format_processing_summary(
                case["input"]["mode"].as_str().unwrap(),
                case["input"]["successful_newsletters"].as_i64().unwrap(),
                case["input"]["attempted_newsletters"].as_i64().unwrap(),
                case["input"]["briefing_valid"].as_bool().unwrap(),
            ),
            &case["output"],
            "processing_summary",
        );
        asserted += 1;
    }
    for case in groups["heatmap"].as_array().unwrap() {
        eq(
            &json!(formatting::top_heatmap_hours(&case["input"]["stats_data"])),
            &case["top_hours"],
            "heatmap top_hours",
        );
        eq(
            &formatting::format_heatmap_summary(&case["input"]["stats_data"]),
            &case["summary"],
            "heatmap summary",
        );
        asserted += 2;
    }
    for case in groups["briefing_render"].as_array().unwrap() {
        eq(
            &briefing::render_sections(&case["input"]["briefing"]),
            &case["sections"],
            "briefing sections",
        );
        eq(
            &json!(briefing::needs_items(&case["input"]["briefing"])),
            &case["needs_items"],
            "briefing needs",
        );
        eq(
            &briefing::meeting_count(&case["input"]["briefing"]),
            &case["meeting_count"],
            "briefing meetings",
        );
        asserted += 3;
    }
    for case in groups["briefing_summary"].as_array().unwrap() {
        eq(
            &briefing::summary(
                case["input"]["briefing"]
                    .as_object()
                    .map(|_| &case["input"]["briefing"]),
                &case["input"]["sections"],
                case["input"]["needs_count"].as_i64().unwrap(),
            ),
            &case["output"],
            "briefing summary",
        );
        asserted += 1;
    }
    for case in groups["gap_links"].as_array().unwrap() {
        let mut expected = case["output"].clone();
        rewrite_sol_urls_in_value(&mut expected);
        eq(
            &json!(formatting::format_gap_links(
                &case["input"]["pipeline_summary"],
                case["input"]["briefing_valid"].as_bool().unwrap(),
                "20260513",
                "20260514"
            )),
            &expected,
            "gap links",
        );
        asserted += 1;
    }
    for case in groups["needs_you"].as_array().unwrap() {
        eq(
            &json!(needs_you::classify_needs_you(
                &case["input"]["attention"],
                case["input"]["pulse_needs"].as_array().unwrap()
            )),
            &case["output"],
            "needs_you",
        );
        asserted += 1;
    }
    for case in groups["dedup_key"].as_array().unwrap() {
        eq(
            &needs_you::needs_dedup_key(&case["input"]["item"]),
            &case["output"],
            "dedup",
        );
        asserted += 1;
    }
    for case in groups["degraded_capture"].as_array().unwrap() {
        eq(
            &needs_you::format_degraded_capture_line(&case["input"]["capture_health"]),
            &case["output"],
            "degraded capture",
        );
        asserted += 1;
    }
    for case in groups["health_glance"].as_array().unwrap() {
        let input = &case["input"];
        let backlog = backlog(input["backlog"].as_str().unwrap());
        let capture = capture(input["capture"].as_str().unwrap());
        let pipeline = pipeline(input["pipeline"].as_str().unwrap());
        let brain = brain(input["brain"].as_str().unwrap());
        let now = Utc.with_ymd_and_hms(2026, 5, 14, 15, 30, 0).unwrap();
        let actual = health_glance::build_health_glance(
            &capture,
            &pipeline,
            input["last_observe_relative"].as_str(),
            &backlog,
            &brain,
            now,
        );
        let mut expected = case["output"].clone();
        let cta_divergence_case = [
            ("valid_fresh_clear", "none", "no_observers", "none"),
            ("valid_fresh_clear", "ready", "no_observers", "none"),
            ("valid_fresh_clear", "none", "no_observers", "empty"),
            ("valid_fresh_clear", "ready", "no_observers", "empty"),
        ]
        .contains(&(
            input["backlog"].as_str().unwrap(),
            input["brain"].as_str().unwrap(),
            input["capture"].as_str().unwrap(),
            input["pipeline"].as_str().unwrap(),
        ));
        if cta_divergence_case {
            // Four named corpus cases retain the reference CTA, verdict, and
            // severity. Count both sides of each patched field:
            // 2147 + 4×2 href + 4×2 verdict + 4×2 severity = 2171.
            assert_eq!(
                expected.pointer("/cta/href"),
                Some(&json!("/app/observer/"))
            );
            asserted += 1;
            assert_eq!(actual.pointer("/cta/href"), Some(&json!("/app/network/")));
            asserted += 1;
            *expected.pointer_mut("/cta/href").unwrap() = json!("/app/network/");
            assert_eq!(expected.pointer("/verdict"), Some(&json!("ok")));
            asserted += 1;
            assert_eq!(actual.pointer("/verdict"), Some(&json!("calm")));
            asserted += 1;
            *expected.pointer_mut("/verdict").unwrap() = json!("calm");
            assert_eq!(expected.pointer("/severity"), Some(&json!("green")));
            asserted += 1;
            assert_eq!(actual.pointer("/severity"), Some(&json!("neutral")));
            asserted += 1;
            *expected.pointer_mut("/severity").unwrap() = json!("neutral");
        }
        eq(&actual, &expected, "health glance");
        asserted += 1;
    }
    let utils = &groups["convey_utils"][0];
    for case in utils["format_date"].as_array().unwrap() {
        eq(
            &formatting::format_date(case["day"].as_str().unwrap()),
            &case["output"],
            "format date",
        );
        asserted += 1;
    }
    for case in utils["relative_time"].as_array().unwrap() {
        eq(
            &formatting::relative_time(case["seconds"].as_f64().unwrap()),
            &case["output"],
            "relative time",
        );
        asserted += 1;
    }
    assert_eq!(asserted, 2171);
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

fn eq<T: serde::Serialize>(actual: &T, expected: &Value, group: &str) {
    assert_eq!(serde_json::to_value(actual).unwrap(), *expected, "{group}");
}
fn backlog(name: &str) -> BacklogSource {
    match name {
        "missing" => BacklogSource {
            backlog: None,
            validity: BacklogValidity::Missing,
            generated_at: None,
        },
        "unparseable" => BacklogSource {
            backlog: None,
            validity: BacklogValidity::Unparseable,
            generated_at: None,
        },
        "valid_fresh_clear" => valid(
            json!({"stuck_days":0,"degraded":false}),
            Some("2026-05-14T12:00:00+00:00"),
        ),
        "valid_stale_clear" => valid(json!({"stuck_days":0}), Some("2026-05-10T00:00:00+00:00")),
        "valid_no_stamp" => valid(json!({"stuck_days":0}), None),
        "valid_degraded" => valid(
            json!({"stuck_days":0,"degraded":true}),
            Some("2026-05-14T12:00:00+00:00"),
        ),
        "valid_stuck" => valid(
            json!({"stuck_days":2,"stuck_day_rows":[{"day":"20260512","reason":"waiting on the model"}]}),
            Some("2026-05-14T12:00:00+00:00"),
        ),
        _ => unreachable!(),
    }
}
fn valid(value: Value, generated_at: Option<&str>) -> BacklogSource {
    BacklogSource {
        backlog: value.as_object().cloned(),
        validity: BacklogValidity::Valid,
        generated_at: generated_at.map(str::to_owned),
    }
}
fn capture(name: &str) -> Value {
    match name {
        "none" => Value::Null,
        "no_observers" => {
            json!({"status":"no_clients","clients":[],"unassessed":[],"registry":"registry_empty"})
        }
        "active" | "stale" | "offline" | "degraded" | "unknown" => {
            json!({"status":name,"clients":[{"name":"laptop"}]})
        }
        _ => unreachable!(),
    }
}
fn pipeline(name: &str) -> Value {
    match name {
        "none" => Value::Null,
        "empty" => json!({}),
        "warning" => json!({"status":"warning","message":"processing is behind"}),
        "headline" => json!({"status":"warning","headline":"three runs did not finish"}),
        "support" => {
            json!({"status":"warning","headline":"three runs did not finish","suggested_action":"open_support"})
        }
        _ => unreachable!(),
    }
}
fn brain(name: &str) -> Value {
    match name {
        "none" => Value::Null,
        "ready" => json!({"state":"ready","headline":"thinking is ready"}),
        "checking" => json!({"state":"checking","headline":"checking thinking"}),
        "blocked_progressing" => {
            json!({"state":"blocked","headline":"installing","progressing":true})
        }
        "blocked" => json!({"state":"blocked","headline":"no provider configured"}),
        "unhealthy" => json!({"state":"unhealthy","headline":"the model failed"}),
        "unknown_with_action" => {
            json!({"state":"unknown","headline":"thinking status unavailable","action":{"label":"check again","href":"/app/thinking/"}})
        }
        _ => unreachable!(),
    }
}
