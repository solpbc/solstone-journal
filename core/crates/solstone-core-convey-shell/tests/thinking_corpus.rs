// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use chrono::{Duration, Utc};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_brain::{begin_refresh, finish_refresh};
use solstone_core_convey_shell::router;
use tower::ServiceExt;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(phase: &str) -> Self {
        let serial = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-thinking-{phase}-{}-{nanos}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("journal creates");
        Self(path)
    }

    fn config(&self, value: Value) {
        fs::create_dir_all(self.0.join("config")).expect("config directory creates");
        fs::write(
            self.0.join("config/journal.json"),
            format!(
                "{}\n",
                serde_json::to_string_pretty(&value).expect("config serializes")
            ),
        )
        .expect("config writes");
    }

    fn corrupt_config(&self) {
        fs::create_dir_all(self.0.join("config")).expect("config directory creates");
        fs::write(
            self.0.join("config/journal.json"),
            br#"{"setup": {"completed_at": 17672256"#,
        )
        .expect("corrupt config writes");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn corpus() -> Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/convey_thinking_corpus.json"
    ))
    .expect("thinking corpus parses")
}

fn confidential() -> Value {
    json!({
        "endpoint_url": "https://spp.example.invalid/v1",
        "served_model_id": "served-model",
        "credential_fingerprint_sha256": sha256(b"corpus-confidential-credential"),
        "prior_active": {"provider": "openai", "model": "gpt-5"},
    })
}

fn established(extra: Value) -> Value {
    let mut value = json!({"setup": {"completed_at": 1767225600}});
    value
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    value
}

fn journal_for_phase(phase: &str) -> TempDir {
    let root = TempDir::new(phase);
    let config = match phase {
        "unestablished" => return root,
        "corrupt" => {
            root.corrupt_config();
            return root;
        }
        "none" => established(json!({})),
        "bundled_local" => established(
            json!({"providers": {"active": {"provider": "local", "model": "local/qwen3.5-4b"}}}),
        ),
        "byo_cloud" => established(
            json!({"env": {"OPENAI_API_KEY": "sk-corpus-not-a-real-key"}, "providers": {"active": {"provider": "openai", "model": "gpt-5"}, "byo_models": {"openai": "gpt-5"}, "key_validation": {"openai": {"valid": true, "timestamp": "2026-01-01T00:00:00+00:00"}}}}),
        ),
        "byo_endpoint" => established(
            json!({"providers": {"active": {"provider": "local", "model": "served-model"}, "local": {"endpoint_url": "http://127.0.0.1:1/v1", "served_model_id": "served-model", "credential": "corpus-credential"}}}),
        ),
        "confidential_inactive" => established(
            json!({"env": {"OPENAI_API_KEY": "sk-corpus-not-a-real-key"}, "providers": {"active": {"provider": "openai", "model": "gpt-5"}, "local": {"endpoint_url": "https://spp.example.invalid/v1", "served_model_id": "served-model", "credential": "corpus-confidential-credential"}}, "services": {"confidential": confidential()}}),
        ),
        "confidential" => established(
            json!({"providers": {"active": {"provider": "local", "model": "served-model"}, "local": {"endpoint_url": "https://spp.example.invalid/v1", "served_model_id": "served-model", "credential": "corpus-confidential-credential"}}, "services": {"confidential": confidential()}}),
        ),
        _ => panic!("unknown phase {phase}"),
    };
    root.config(config);
    if phase == "none" {
        assert!(
            solstone_core_thinking::read_config(&root.0)
                .expect("none config reads")
                .get("providers")
                .is_none()
        );
    }
    if phase != "none" {
        seed_brain(&root.0, phase);
    }
    root
}

fn seed_brain(journal: &Path, phase: &str) {
    let now = Utc::now();
    let expires = now + Duration::days(1);
    let component =
        json!({"status":"ok","observed_at":now.to_rfc3339(),"expires_at":expires.to_rfc3339()});
    let bundled_runtime = (phase == "bundled_local").then(|| "c".repeat(64));
    let permit = begin_refresh(journal, now, None, None, false, bundled_runtime.clone())
        .expect("brain refresh begins")
        .unwrap_or_else(|| panic!("{phase} configured brain produces a permit"));
    finish_refresh(
        journal,
        permit,
        json!({"configuration": component, "lane_prerequisites": component, "generate": component, "cogitate": component}),
        now,
        bundled_runtime,
    )
    .expect("brain refresh finishes");
}

async fn request(
    app: axum::Router,
    method: &str,
    path: &str,
) -> (StatusCode, String, Option<String>, Vec<u8>) {
    request_with_body(app, method, path, None).await
}

async fn request_with_body(
    app: axum::Router,
    method: &str,
    path: &str,
    request_json: Option<&Value>,
) -> (StatusCode, String, Option<String>, Vec<u8>) {
    let mut builder = Request::builder()
        .method(Method::from_bytes(method.as_bytes()).expect("method parses"))
        .uri(path);
    let body = match request_json {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(value).expect("request body serializes"))
        }
        None => Body::empty(),
    };
    let response = app
        .oneshot(builder.body(body).expect("request builds"))
        .await
        .expect("router responds");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .expect("content type present")
        .to_str()
        .expect("content type text")
        .to_owned();
    let location = response
        .headers()
        .get(header::LOCATION)
        .map(|value| value.to_str().expect("location text").to_owned());
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads")
        .to_vec();
    (status, content_type, location, body)
}

/// `native_deviations[0]` is shaped like eight corpus vectors, one per phase:
/// bodyless `PUT /api/generators`. The corrupt and unestablished vectors are
/// preempted by the session gate; exactly six established vectors reach this
/// native assertion. The reference crashes its handler while parsing a
/// bodyless request and returns a generic 500, while native deliberately
/// returns the standard typed 400 refusal used by sibling write routes.
fn is_generators_missing_body_deviation(phase: &str, case: &Value) -> bool {
    is_established_phase(phase)
        && case["method"] == "PUT"
        && case["path"] == "/app/thinking/api/generators"
        && case.get("request_json").is_none()
}

fn assert_generators_missing_body_deviation(response: &(StatusCode, String, Option<String>, Vec<u8>)) {
    assert_eq!(response.0, StatusCode::BAD_REQUEST);
    assert_eq!(response.1, "application/json");
    assert_eq!(response.2, None);
    let actual: Value = serde_json::from_slice(&response.3).expect("deviation envelope is JSON");
    assert_eq!(
        actual,
        json!({
            "error": "I couldn't find any data in that request.",
            "reason_code": "missing_request_body",
            "detail": "No data provided",
        })
    );
}

fn replay_full_recorded_case(
    phase: &str,
    index: usize,
    journal: &Path,
    case: &Value,
    response: &(StatusCode, String, Option<String>, Vec<u8>),
) -> (usize, bool) {
    let path = case["path"].as_str().expect("path");
    assert_eq!(
        response.0.as_u16(),
        case["status"].as_u64().expect("status") as u16,
        "{phase} {path} status: {}",
        String::from_utf8_lossy(&response.3)
    );
    assert_eq!(
        response.1,
        case["content_type"].as_str().expect("content type"),
        "{phase} {path} content type"
    );
    assert_eq!(
        response.2.as_deref(),
        case.get("location").and_then(Value::as_str),
        "{phase} {path} location"
    );
    let mut normalized_response = None;
    let actual_hash = match (
        case["body_sha256_basis"].as_str(),
        case.get("body_normalized"),
    ) {
        (Some("raw-body"), None) => sha256(&response.3),
        (Some("raw-body"), Some(_)) => sha256(
            String::from_utf8(response.3.clone())
                .expect("normalized body is text")
                .replace(&journal.display().to_string(), "<JOURNAL_ROOT>")
                .as_bytes(),
        ),
        (Some("normalized-json"), _) => {
            let mut value: Value =
                serde_json::from_slice(&response.3).expect("normalized response is JSON");
            for (path, kind) in case
                .get("normalized_fields")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                set_path(&mut value, path, placeholder(kind.as_str().expect("kind")));
            }
            let hash = sha256(canonical_json(&value).as_bytes());
            normalized_response = Some(value);
            hash
        }
        other => panic!("unknown body arm {other:?}"),
    };
    if actual_hash != case["body_sha256"].as_str().expect("body hash") {
        // This explicit 198-vector allowlist is the only semantic fallback
        // outside the corrupt API-envelope bucket. Native `error_envelope()`
        // emits {"error","reason_code","detail"} in insertion order with
        // no trailing newline; recorded Flask `jsonify()` emits sorted
        // {"detail","error","reason_code"} plus a trailing newline.
        // Their decoded fields are identical. This retains that field-equality
        // pin while leaving serialization parity to the shared envelope owner;
        // all other cases remain byte-pinned.
        if is_established_error_envelope_byte_fallback(phase, index)
            && let Some(expected) = case.get("json")
            && let Ok(mut actual) = serde_json::from_slice::<Value>(&response.3)
        {
            normalize_journal_root(&mut actual, &journal.display().to_string());
            assert_eq!(&actual, expected, "{phase} {path} envelope fields");
            return (body_arm(case), true);
        }
        if let Some(actual) = normalized_response {
            panic!(
                "{phase} {path} normalized JSON differs at {:?}",
                differing_paths(&actual, &case["json"], "")
            );
        }
    }
    assert_eq!(
        actual_hash,
        case["body_sha256"].as_str().expect("body hash"),
        "{phase} {path} body"
    );
    (body_arm(case), false)
}

fn is_established_phase(phase: &str) -> bool {
    matches!(
        phase,
        "none"
            | "bundled_local"
            | "byo_cloud"
            | "byo_endpoint"
            | "confidential_inactive"
            | "confidential"
    )
}

fn is_established_error_envelope_byte_fallback(phase: &str, index: usize) -> bool {
    match phase {
        "none" | "bundled_local" | "byo_cloud" | "byo_endpoint" => {
            matches!(index, 18..=47 | 49 | 50 | 54)
        }
        "confidential_inactive" | "confidential" => {
            matches!(index, 18..=34 | 36..=47 | 49 | 50 | 54 | 55)
        }
        _ => false,
    }
}

const CORRUPT_API_ENVELOPE_PATHS: &[&str] = &[
    "/app/thinking/api/state",
    "/app/thinking/api/providers",
    "/app/thinking/api/providers?local_model=local/qwen3.5-4b",
    "/app/thinking/api/providers?local_model=nope",
    "/app/thinking/api/providers?local_model=",
    "/app/thinking/api/keys",
    "/app/thinking/api/providers/local/status",
    "/app/thinking/api/local/availability",
    "/app/thinking/api/local/availability?model=nope",
    "/app/thinking/api/local/bootstrap/status",
    "/app/thinking/api/local/bootstrap/status?model=nope",
    "/app/thinking/api/local/models",
    "/app/thinking/api/local/runtime",
    "/app/thinking/api/generators",
    "/app/thinking/api/keys/check",
    "/app/thinking/api/validate-model",
    "/app/thinking/api/local/endpoint",
    "/app/thinking/api/local/runtime/retry",
];

fn is_corrupt_api_envelope_case(phase: &str, case: &Value) -> bool {
    phase == "corrupt"
        && case["content_type"] == "application/json"
        && CORRUPT_API_ENVELOPE_PATHS.contains(&case["path"].as_str().expect("path"))
}

fn assert_corrupt_api_envelope_case(
    journal: &Path,
    case: &Value,
    response: &(StatusCode, String, Option<String>, Vec<u8>),
) {
    let path = case["path"].as_str().expect("path");
    assert_eq!(
        response.0,
        StatusCode::INTERNAL_SERVER_ERROR,
        "corrupt {path}"
    );
    assert_eq!(response.1, "application/json", "corrupt {path}");
    assert_eq!(response.2, None, "corrupt {path}");
    let mut actual: Value = serde_json::from_slice(&response.3).expect("corrupt envelope is JSON");
    normalize_journal_root(&mut actual, &journal.display().to_string());
    assert_eq!(actual, case["json"], "corrupt {path} envelope fields");
}

fn assert_no_slash_deviation(
    phase: &str,
    journal: &Path,
    response: &(StatusCode, String, Option<String>, Vec<u8>),
) {
    let (status, content_type, location, body) = response;
    match phase {
        "unestablished" => {
            assert_eq!(*status, StatusCode::FOUND);
            assert_eq!(content_type, "text/html; charset=utf-8");
            assert_eq!(location.as_deref(), Some("/init"));
            assert_eq!(
                body,
                b"<!doctype html>\n<html lang=en>\n<title>Redirecting...</title>\n<h1>Redirecting...</h1>\n<p>You should be redirected automatically to the target URL: <a href=\"/init\">/init</a>. If not, click the link.\n"
            );
        }
        "corrupt" => {
            assert_eq!(*status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(content_type, "text/plain; charset=utf-8");
            assert_eq!(location, &None);
            assert_eq!(
                body,
                &format!("I couldn't read your settings file at {}/config/journal.json. Your settings were NOT changed. Repair the file or restore config/journal.json from a backup, then try again.", journal.display()).into_bytes()
            );
        }
        other => panic!("unexpected no-slash deviation phase {other}"),
    }
}

fn normalize_journal_root(value: &mut Value, journal_root: &str) {
    match value {
        Value::String(text) => *text = text.replace(journal_root, "<JOURNAL_ROOT>"),
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| normalize_journal_root(value, journal_root)),
        Value::Object(fields) => fields
            .values_mut()
            .for_each(|value| normalize_journal_root(value, journal_root)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn body_arm(case: &Value) -> usize {
    match (
        case["body_sha256_basis"].as_str(),
        case.get("body_normalized"),
    ) {
        (Some("raw-body"), None) => 0,
        (Some("raw-body"), Some(_)) => 1,
        (Some("normalized-json"), _) => 2,
        _ => unreachable!(),
    }
}

fn differing_paths(actual: &Value, expected: &Value, prefix: &str) -> Vec<String> {
    if actual == expected {
        return Vec::new();
    }
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => actual
            .keys()
            .chain(expected.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .flat_map(|key| {
                differing_paths(
                    actual.get(key).unwrap_or(&Value::Null),
                    expected.get(key).unwrap_or(&Value::Null),
                    &format!("{prefix}.{key}"),
                )
            })
            .collect(),
        _ => vec![prefix.to_owned()],
    }
}

fn set_path(value: &mut Value, path: &str, replacement: &str) {
    let mut current = value;
    let mut fields = path.split('.').peekable();
    while let Some(field) = fields.next() {
        if fields.peek().is_none() {
            current
                .as_object_mut()
                .expect("normalization object")
                .insert(field.to_owned(), Value::String(replacement.to_owned()));
            return;
        }
        current = current
            .as_object_mut()
            .expect("normalization object")
            .get_mut(field)
            .expect("normalization field");
    }
}

fn placeholder(kind: &str) -> &'static str {
    match kind {
        "capture-clock" => "<CAPTURE_CLOCK>",
        "host" => "<HOST_DEPENDENT>",
        "version" => "<VERSION>",
        other => panic!("unknown normalization kind {other}"),
    }
}
fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => json_string(value),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let mut fields: Vec<_> = values.iter().collect();
            fields.sort_unstable_by(|left, right| left.0.cmp(right.0));
            format!(
                "{{{}}}",
                fields
                    .into_iter()
                    .map(|(key, value)| format!("{}:{}", json_string(key), canonical_json(value)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn json_string(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' || !character.is_ascii() => {
                let code = character as u32;
                if code <= 0xffff {
                    output.push_str(&format!("\\u{code:04x}"));
                } else {
                    let code = code - 0x1_0000;
                    output.push_str(&format!(
                        "\\u{:04x}\\u{:04x}",
                        0xd800 + (code >> 10),
                        0xdc00 + (code & 0x3ff)
                    ));
                }
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn assert_top_level_keys(body: &Value, mut expected: Vec<&str>) {
    let mut actual: Vec<_> = body
        .as_object()
        .expect("response is an object")
        .keys()
        .map(String::as_str)
        .collect();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

/// Replays all 448 fixture cases against one journal per phase in the
/// generator's recorded order. Every recorded request body is sent. Each
/// established phase contains 35 non-GET cases; the trailing
/// `DELETE /api/local/endpoint` remains order-dependent, but is not unique.
/// The result has one arm per fixture record: byte pins, two documented native
/// deviations, corrupt semantic envelopes, and the explicit shared-envelope
/// serialization fallback remain mutually exclusive assertion buckets.
#[tokio::test]
async fn all_fixture_cases_replay_in_recorded_phase_order_with_bodies() {
    let corpus = corpus();
    let mut count = 0;
    let mut arms = [0; 3];
    let mut byte_pinned = 0;
    let mut corrupt_semantic = 0;
    let mut established_error_envelope_fallback = 0;
    let mut no_slash_deviations = 0;
    let mut generators_missing_body_deviation = 0;
    for phase in [
        "unestablished",
        "corrupt",
        "none",
        "bundled_local",
        "byo_cloud",
        "byo_endpoint",
        "confidential_inactive",
        "confidential",
    ] {
        let journal = journal_for_phase(phase);
        for (index, case) in corpus["phases"][phase]
            .as_array()
            .expect("phase cases")
            .iter()
            .enumerate()
        {
            let response = request_with_body(
                router(journal.0.clone()),
                case["method"].as_str().expect("method"),
                case["path"].as_str().expect("path"),
                case.get("request_json"),
            )
            .await;
            let arm = body_arm(case);
            if is_corrupt_api_envelope_case(phase, case) {
                assert_corrupt_api_envelope_case(&journal.0, case, &response);
                corrupt_semantic += 1;
            } else if case["path"] == "/app/thinking"
                && matches!(phase, "unestablished" | "corrupt")
            {
                assert_no_slash_deviation(phase, &journal.0, &response);
                no_slash_deviations += 1;
            } else if is_generators_missing_body_deviation(phase, case) {
                assert_generators_missing_body_deviation(&response);
                generators_missing_body_deviation += 1;
            } else {
                let (replayed_arm, fallback) =
                    replay_full_recorded_case(phase, index, &journal.0, case, &response);
                assert_eq!(replayed_arm, arm, "{phase} index {index} arm");
                if fallback {
                    established_error_envelope_fallback += 1;
                } else {
                    byte_pinned += 1;
                }
            }
            arms[arm] += 1;
            count += 1;
        }
    }
    assert_eq!(count, 448);
    assert_eq!(arms, [361, 55, 32]);
    assert_eq!(byte_pinned, 193);
    assert_eq!(corrupt_semantic, 49);
    assert_eq!(established_error_envelope_fallback, 198);
    assert_eq!(no_slash_deviations, 2);
    assert_eq!(generators_missing_body_deviation, 6);
    assert_eq!(
        byte_pinned
            + corrupt_semantic
            + established_error_envelope_fallback
            + no_slash_deviations
            + generators_missing_body_deviation,
        448
    );
}

#[test]
fn fixture_body_arms_are_361_55_32_across_all_448_cases() {
    let corpus = corpus();
    let mut arms = [0; 3];
    let mut count = 0;
    for cases in corpus["phases"].as_object().expect("phase map").values() {
        for case in cases.as_array().expect("phase cases") {
            arms[body_arm(case)] += 1;
            count += 1;
        }
    }
    assert_eq!(count, 448);
    assert_eq!(arms, [361, 55, 32]);
    assert_eq!(arms.iter().sum::<usize>(), 448);
    assert_eq!(corpus["native_deviations"].as_array().expect("deviations").len(), 2);
}

#[tokio::test]
async fn copy_payload_round_trips_from_api_state() {
    let corpus = corpus();
    let journal = journal_for_phase("none");
    let response = request(router(journal.0.clone()), "GET", "/app/thinking/api/state").await;
    let actual: Value = serde_json::from_slice(&response.3).expect("state is JSON");
    let expected = corpus["phases"]["none"]
        .as_array()
        .expect("none cases")
        .iter()
        .find(|case| case["path"] == "/app/thinking/api/state")
        .expect("state case");
    assert_eq!(actual["copy"], expected["json"]["copy"]);
}

#[tokio::test]
async fn invalid_brain_record_degrades_the_brain_read_projections() {
    let journal = journal_for_phase("byo_cloud");
    let path = journal.0.join("health/brain.json");
    let mut invalid: Value = serde_json::from_slice(&fs::read(&path).expect("seed record reads"))
        .expect("seed record is JSON");
    invalid["fingerprint_sha256"] = Value::String("x".repeat(64));
    fs::write(
        &path,
        serde_json::to_vec(&invalid).expect("invalid record serializes"),
    )
    .expect("invalid record writes");
    for path in ["/app/thinking/api/state", "/app/thinking/api/providers"] {
        let response = request(router(journal.0.clone()), "GET", path).await;
        let body: Value = serde_json::from_slice(&response.3).expect("projection is JSON");
        let brain = if path.ends_with("state") {
            &body["providers"]["brain"]
        } else {
            &body["brain"]
        };
        assert_eq!(brain["state"], "unknown", "{path}");
        assert_eq!(brain["reason_code"], "brain_record_invalid", "{path}");
    }
    let response = request(
        router(journal.0.clone()),
        "GET",
        "/app/thinking/api/providers/local/status",
    )
    .await;
    let body: Value = serde_json::from_slice(&response.3).expect("local status is JSON");
    assert_eq!(body["generate_ready"], false);
    assert_eq!(body["cogitate_ready"], false);
}

#[tokio::test]
async fn post_keys_check_refusals_have_exact_top_level_keys() {
    let journal = journal_for_phase("none");
    for request_json in [
        None,
        Some(json!({"env_var":"OPENAI_API_KEY","value":""})),
        Some(json!({"env_var":"bogus","value":"x"})),
    ] {
        let response = request_with_body(
            router(journal.0.clone()),
            "POST",
            "/app/thinking/api/keys/check",
            request_json.as_ref(),
        )
        .await;
        assert_eq!(response.0, StatusCode::BAD_REQUEST);
        let body: Value = serde_json::from_slice(&response.3).expect("response is JSON");
        assert_top_level_keys(&body, vec!["detail", "error", "reason_code"]);
    }
}

#[tokio::test]
async fn post_validate_model_missing_key_has_exact_top_level_keys() {
    let journal = journal_for_phase("none");
    let response = request_with_body(
        router(journal.0.clone()),
        "POST",
        "/app/thinking/api/validate-model",
        Some(&json!({"provider":"openai","model":"gpt-5"})),
    )
    .await;
    assert_eq!(response.0, StatusCode::OK);
    let body: Value = serde_json::from_slice(&response.3).expect("response is JSON");
    assert_top_level_keys(
        &body,
        vec!["message", "model", "provider", "reason_code", "valid"],
    );

    let response = request_with_body(
        router(journal.0.clone()),
        "POST",
        "/app/thinking/api/validate-model",
        Some(&json!({"provider":"local","model":"m"})),
    )
    .await;
    assert_eq!(response.0, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&response.3).expect("response is JSON");
    assert_top_level_keys(&body, vec!["detail", "error", "reason_code"]);
}

#[tokio::test]
async fn post_local_bootstrap_byo_refusal_has_exact_top_level_keys() {
    let journal = TempDir::new("bootstrap-byo");
    journal.config(established(json!({
        "providers": {
            "local": {
                "endpoint_url": "http://127.0.0.1:1/v1",
                "served_model_id": "served-model"
            }
        }
    })));
    let response = request_with_body(
        router(journal.0.clone()),
        "POST",
        "/app/thinking/api/local/bootstrap",
        None,
    )
    .await;
    assert_eq!(response.0, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&response.3).expect("response is JSON");
    assert_top_level_keys(&body, vec!["detail", "error", "reason_code"]);
}

#[tokio::test]
async fn post_local_runtime_retry_refusal_has_exact_top_level_keys() {
    let journal = journal_for_phase("none");
    let response = request_with_body(
        router(journal.0.clone()),
        "POST",
        "/app/thinking/api/local/runtime/retry",
        Some(&json!({"health_revision":1})),
    )
    .await;
    assert_eq!(response.0, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&response.3).expect("response is JSON");
    assert_top_level_keys(&body, vec!["detail", "error", "reason_code"]);
}

#[tokio::test]
async fn post_brain_check_without_callosum_has_exact_top_level_keys() {
    let journal = journal_for_phase("none");
    let response = request_with_body(
        router(journal.0.clone()),
        "POST",
        "/app/thinking/api/brain/check",
        None,
    )
    .await;
    assert_eq!(response.0, StatusCode::OK);
    let body: Value = serde_json::from_slice(&response.3).expect("response is JSON");
    assert_top_level_keys(&body, vec!["brain", "error", "ok"]);
}

struct PanicValidator;
impl solstone_core_thinking::providers::ManagedKeyValidator for PanicValidator {
    fn validate(&self, _provider: &str, _key: &str) -> Result<Value, String> {
        panic!("managed-provider validation must be stubbed")
    }
}
struct FailureValidator;
impl solstone_core_thinking::providers::ManagedKeyValidator for FailureValidator {
    fn validate(&self, provider: &str, _key: &str) -> Result<Value, String> {
        Ok(
            json!({"valid":false,"reason_code":"provider_rejected","error":format!("{provider} rejected the key")}),
        )
    }
}

#[tokio::test]
async fn validate_keys_is_non_persisting_and_has_the_exact_contract_shape() {
    let zero = Map::new();
    assert_eq!(
        solstone_core_thinking::providers::validate_keys_with(&zero, &PanicValidator),
        json!({"key_validation":{}})
    );
    for env in [
        json!({"OPENAI_API_KEY":"one"}),
        json!({"GOOGLE_API_KEY":"one","OPENAI_API_KEY":"two","ANTHROPIC_API_KEY":"three"}),
    ] {
        let journal = TempDir::new("validate-keys");
        journal.config(established(json!({"env":env})));
        let before =
            fs::read(journal.0.join("config/journal.json")).expect("config reads before GET");
        let response = request(
            router(journal.0.clone()),
            "GET",
            "/app/thinking/api/validate-keys",
        )
        .await;
        assert_eq!(response.0, StatusCode::OK);
        let body: Value = serde_json::from_slice(&response.3).expect("validation is JSON");
        assert_eq!(
            body.as_object()
                .expect("contract object")
                .keys()
                .collect::<Vec<_>>(),
            ["key_validation"]
        );
        let validation = body["key_validation"]
            .as_object()
            .expect("validation object");
        assert_eq!(validation.len(), env.as_object().expect("env object").len());
        for result in validation.values() {
            assert!(result.get("reason_code").is_some());
        }
        assert_eq!(
            fs::read(journal.0.join("config/journal.json")).expect("config reads after GET"),
            before
        );
    }
    let one = Map::from_iter([(String::from("env"), json!({"OPENAI_API_KEY":"one"}))]);
    assert_eq!(
        solstone_core_thinking::providers::validate_keys_with(&one, &FailureValidator)["key_validation"]
            ["openai"]["reason_code"],
        "provider_rejected"
    );
}
