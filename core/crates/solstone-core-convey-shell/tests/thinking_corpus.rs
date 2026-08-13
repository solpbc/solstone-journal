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
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::from_bytes(method.as_bytes()).expect("method parses"))
                .uri(path)
                .body(Body::empty())
                .expect("request builds"),
        )
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

fn replay_case(
    phase: &str,
    journal: &Path,
    case: &Value,
    response: &(StatusCode, String, Option<String>, Vec<u8>),
) -> usize {
    let path = case["path"].as_str().expect("path");
    let root_deviation = path == "/app/thinking" && matches!(phase, "unestablished" | "corrupt");
    let expected_status = if root_deviation && phase == "unestablished" {
        302
    } else if root_deviation {
        500
    } else {
        case["status"].as_u64().expect("status") as u16
    };
    assert_eq!(
        response.0.as_u16(),
        expected_status,
        "{phase} {path} status: {}",
        String::from_utf8_lossy(&response.3)
    );
    let expected_content_type = if root_deviation && phase == "corrupt" {
        "text/plain; charset=utf-8"
    } else {
        case["content_type"].as_str().expect("content type")
    };
    assert_eq!(
        response.1, expected_content_type,
        "{phase} {path} content type"
    );
    let expected_location = if root_deviation && phase == "unestablished" {
        Some("/init")
    } else if root_deviation {
        None
    } else {
        case.get("location").and_then(Value::as_str)
    };
    assert_eq!(
        response.2.as_deref(),
        expected_location,
        "{phase} {path} location"
    );
    if root_deviation {
        let expected = if phase == "unestablished" {
            b"<!doctype html>\n<html lang=en>\n<title>Redirecting...</title>\n<h1>Redirecting...</h1>\n<p>You should be redirected automatically to the target URL: <a href=\"/init\">/init</a>. If not, click the link.\n".to_vec()
        } else {
            format!("I couldn't read your settings file at {}/config/journal.json. Your settings were NOT changed. Repair the file or restore config/journal.json from a backup, then try again.", journal.display()).into_bytes()
        };
        assert_eq!(response.3, expected, "{phase} {path} deviation body");
        return body_arm(case);
    }
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
        if case["body_sha256_basis"] == "raw-body" && case.get("json").is_some() {
            assert_eq!(
                sha256(format!("{}\n", canonical_json(&case["json"])).as_bytes()),
                case["body_sha256"].as_str().expect("body hash"),
                "{phase} {path} reference JSON framing"
            );
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
    body_arm(case)
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

#[tokio::test]
async fn inherited_gate_cases_are_102() {
    let corpus = corpus();
    let mut count = 0;
    let mut arms = [0; 3];
    for phase in ["unestablished", "corrupt"] {
        let journal = journal_for_phase(phase);
        for case in corpus["phases"][phase].as_array().expect("phase cases") {
            let response = request(
                router(journal.0.clone()),
                case["method"].as_str().expect("method"),
                case["path"].as_str().expect("path"),
            )
            .await;
            let arm = replay_case(phase, &journal.0, case, &response);
            arms[arm] += 1;
            count += 1;
        }
    }
    assert_eq!(count, 102);
    assert_eq!(arms, [52, 50, 0]);
}

#[tokio::test]
async fn established_get_cases_are_108_and_all_replayed_bodies_hash() {
    let corpus = corpus();
    let mut count = 0;
    let mut arms = [0; 3];
    for phase in [
        "none",
        "bundled_local",
        "byo_cloud",
        "byo_endpoint",
        "confidential_inactive",
        "confidential",
    ] {
        let journal = journal_for_phase(phase);
        for case in corpus["phases"][phase]
            .as_array()
            .expect("phase cases")
            .iter()
            .filter(|case| case["method"] == "GET")
        {
            let response = request(
                router(journal.0.clone()),
                "GET",
                case["path"].as_str().expect("path"),
            )
            .await;
            arms[replay_case(phase, &journal.0, case, &response)] += 1;
            count += 1;
        }
    }
    assert_eq!(count, 108);
    assert_eq!(arms, [78, 0, 30]);
}

#[test]
fn fixture_body_arms_are_326_50_32_across_all_408_cases() {
    let corpus = corpus();
    let mut arms = [0; 3];
    let mut count = 0;
    for cases in corpus["phases"].as_object().expect("phase map").values() {
        for case in cases.as_array().expect("phase cases") {
            arms[body_arm(case)] += 1;
            count += 1;
        }
    }
    assert_eq!(count, 408);
    assert_eq!(arms, [326, 50, 32]);
    assert_eq!(arms.iter().sum::<usize>(), 408);
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
