// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const NORMALIZE: [(&str, &str); 12] = [
    ("runtime_label", "<HOST:runtime_label>"),
    ("parakeet_uses_cpp", "<HOST:parakeet_uses_cpp>"),
    ("resource.*", "<HOST:resource>"),
    ("backends.*.available", "<HOST:backend_available>"),
    ("api_keys.*", "<HOST:api_key_present>"),
    ("runtime_env.*", "<HOST:runtime_env>"),
    ("identity.timezone", "<HOST:timezone>"),
    ("dashboard_url", "<HOST:dashboard_url>"),
    ("status_text", "<HOST:status_text>"),
    ("warnings", "<VOLATILE:storage_warnings>"),
    ("key_validation.*.timestamp", "<VOLATILE:timestamp>"),
    ("entries.*.timestamp", "<VOLATILE:log_timestamp>"),
];

#[tokio::test]
async fn ac11_normalizer_path_sets_equal_the_corpus_per_case() {
    let corpus = crate::test_support::corpus();
    for (phase_name, phase) in corpus["phases"].as_object().expect("phases") {
        let root = crate::test_support::phase_root(phase_name);
        let router = crate::test_support::shell_router(root.path());
        for (name, case) in phase.as_object().expect("phase") {
            if !name.starts_with("GET ") {
                continue;
            }
            let response = router
                .clone()
                .oneshot(
                    Request::get(crate::test_support::request_path(name))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            let body: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body"),
            )
            .expect("JSON response");
            let (_, mut hits) = normalize(body, "", &root.path().display().to_string());
            hits.sort();
            hits.dedup();
            let mut expected = case["normalized_paths"]
                .as_array()
                .expect("paths")
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            expected.sort();
            expected.dedup();
            assert_eq!(hits, expected, "{phase_name} {name}");
        }
    }
}

#[tokio::test]
async fn ac3_all_captured_get_cases_match_status_and_digest() {
    let corpus = crate::test_support::corpus();
    let phases = corpus["phases"].as_object().expect("phases");
    let mut total = 0;
    for (phase_name, phase) in phases {
        let root = crate::test_support::phase_root(phase_name);
        let router = crate::test_support::shell_router(root.path());
        for (name, expected) in phase.as_object().expect("phase") {
            if !name.starts_with("GET ") {
                continue;
            }
            total += 1;
            let response = router
                .clone()
                .oneshot(
                    Request::get(crate::test_support::request_path(name))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(
                response.status().as_u16(),
                expected["status"].as_u64().expect("status") as u16,
                "{phase_name} {name}"
            );
            let body: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body"),
            )
            .expect("JSON response");
            let (normalized, mut hits) = normalize(body, "", &root.path().display().to_string());
            hits.sort();
            hits.dedup();
            let mut expected_hits = expected["normalized_paths"]
                .as_array()
                .expect("normalized paths")
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            expected_hits.sort();
            expected_hits.dedup();
            assert_eq!(hits, expected_hits, "{phase_name} {name}");
            assert_eq!(
                digest(&normalized),
                expected["digest"].as_str().expect("digest"),
                "{phase_name} {name}"
            );
        }
    }
    assert_eq!(total, 142);
}

pub(crate) fn normalize(value: Value, path: &str, root: &str) -> (Value, Vec<String>) {
    let mut hits = Vec::new();
    if let Value::String(value) = value {
        if value.contains(root) {
            hits.push(format!("{path}#journal_root"));
            return (Value::String(value.replace(root, "<JOURNAL_ROOT>")), hits);
        }
        if let Some((_, replacement)) = NORMALIZE
            .iter()
            .find(|(pattern, _)| !path.is_empty() && matches(path, pattern))
        {
            hits.push(path.to_owned());
            return (Value::String((*replacement).to_owned()), hits);
        }
        return (Value::String(value), hits);
    }
    if let Some((_, replacement)) = NORMALIZE
        .iter()
        .find(|(pattern, _)| !path.is_empty() && matches(path, pattern))
    {
        hits.push(path.to_owned());
        return (Value::String((*replacement).to_owned()), hits);
    }
    match value {
        Value::Object(values) => {
            let mut output = serde_json::Map::new();
            let mut entries = values.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            for (key, value) in entries {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let (value, child_hits) = normalize(value, &child, root);
                output.insert(key, value);
                hits.extend(child_hits);
            }
            (Value::Object(output), hits)
        }
        Value::Array(values) => {
            let mut output = Vec::new();
            for value in values {
                let child = if path.is_empty() {
                    "*".to_owned()
                } else {
                    format!("{path}.*")
                };
                let (value, child_hits) = normalize(value, &child, root);
                output.push(value);
                hits.extend(child_hits);
            }
            (Value::Array(output), hits)
        }
        value => (value, hits),
    }
}

pub(crate) fn matches(path: &str, pattern: &str) -> bool {
    let path = path.split('.').collect::<Vec<_>>();
    let pattern = pattern.split('.').collect::<Vec<_>>();
    path.len() == pattern.len()
        && path
            .iter()
            .zip(pattern)
            .all(|(part, pattern)| pattern == "*" || *part == pattern)
}

pub(crate) fn digest(value: &Value) -> String {
    let bytes = python_json(value);
    format!("{:x}", Sha256::digest(bytes))
        .chars()
        .take(16)
        .collect()
}

pub(crate) fn python_json(value: &Value) -> Vec<u8> {
    fn write(value: &Value, output: &mut String) {
        match value {
            Value::Object(values) => {
                output.push('{');
                let mut entries = values.iter().collect::<Vec<_>>();
                entries.sort_by(|left, right| left.0.cmp(right.0));
                for (index, (key, value)) in entries.into_iter().enumerate() {
                    if index > 0 {
                        output.push_str(", ");
                    }
                    output.push_str(&serde_json::to_string(key).expect("key"));
                    output.push_str(": ");
                    write(value, output);
                }
                output.push('}');
            }
            Value::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push_str(", ");
                    }
                    write(value, output);
                }
                output.push(']');
            }
            value => output.push_str(&serde_json::to_string(value).expect("value")),
        }
    }
    let mut output = String::new();
    write(value, &mut output);
    output.into_bytes()
}
