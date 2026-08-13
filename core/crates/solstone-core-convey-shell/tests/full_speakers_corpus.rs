// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_convey_shell::router;
use tower::ServiceExt;

const XYZ_DEVIATION_PATH: &str =
    "/app/speakers/api/serve_audio/20260731/field/090000_300/mic_audio.xyz";

#[derive(Debug, Eq, PartialEq)]
struct FileStamp {
    size: u64,
    sha256: String,
    modified: SystemTime,
}

fn corpus_cases() -> Vec<Value> {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../fixtures/convey_speakers_corpus.json"
    ))
    .expect("speakers corpus parses");
    corpus["cases"]
        .as_array()
        .expect("cases are an array")
        .clone()
}

async fn sweep_populated_corpus(app: axum::Router) -> usize {
    let cases = corpus_cases();
    for case in &cases {
        let path = case["path"].as_str().expect("case path is text");
        let method = case["method"].as_str().expect("case method is text");
        let mut request = Request::builder().method(method).uri(path);
        for (name, value) in case["request_headers"]
            .as_object()
            .expect("request headers are an object")
        {
            request = request.header(name, value.as_str().expect("request header is text"));
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).expect("request builds"))
            .await
            .expect("router responds");
        let status = response.status().as_u16();
        let response_headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads")
            .to_vec();

        if path == XYZ_DEVIATION_PATH {
            assert_eq!(status, 400, "{path}");
            assert_eq!(
                response_headers[header::CONTENT_TYPE],
                "application/json",
                "{path}"
            );
            let actual: Value = serde_json::from_slice(&body).expect("XYZ refusal is JSON");
            assert_eq!(actual["reason_code"], "invalid_request_value", "{path}");
            continue;
        }

        assert_eq!(status, case["status"], "{path}");
        let expected_content_type = case
            .get("content_type")
            .or_else(|| {
                case.get("headers")
                    .and_then(|headers| headers.get("Content-Type"))
            })
            .and_then(Value::as_str)
            .expect("case content type is text");
        assert_eq!(
            response_headers[header::CONTENT_TYPE],
            expected_content_type,
            "{path}"
        );

        if let Some(expected_json) = case.get("json") {
            let actual: Value = serde_json::from_slice(&body).expect("JSON response parses");
            assert_json_case(path, actual, expected_json.clone());
        } else {
            assert_eq!(
                body.len(),
                case["body_bytes"].as_u64().expect("body size") as usize,
                "{path}"
            );
            assert_eq!(
                format!("{:x}", Sha256::digest(&body)),
                case["body_sha256"].as_str().expect("body hash"),
                "{path}"
            );
            if path.starts_with("/app/speakers/api/serve_audio") {
                for (name, value) in case["headers"].as_object().expect("headers are an object") {
                    assert_eq!(
                        response_headers
                            .get(name)
                            .unwrap_or_else(|| panic!("{path}: missing {name}")),
                        value.as_str().expect("header is text"),
                        "{path}: {name}"
                    );
                }
            }
        }
    }
    cases.len()
}

fn assert_json_case(path: &str, mut actual: Value, mut expected: Value) {
    if path == "/app/speakers/api/state" {
        normalize_today(&mut actual);
        normalize_today(&mut expected);
        assert_eq!(actual, expected, "{path}");
    } else if path.starts_with("/app/speakers/api/speakers/known")
        && expected.get("speakers").is_some()
    {
        assert_known_json(&actual, &expected, path);
    } else {
        assert_eq!(actual, expected, "{path}");
    }
}

fn normalize_today(value: &mut Value) {
    match value {
        Value::Object(object) => object.values_mut().for_each(normalize_today),
        Value::Array(values) => values.iter_mut().for_each(normalize_today),
        Value::String(text)
            if text.len() == 8 && text.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            *text = "<TODAY>".to_owned();
        }
        _ => {}
    }
}

fn assert_known_json(actual: &Value, expected: &Value, path: &str) {
    let mut actual = actual
        .as_object()
        .expect("actual known response is object")
        .clone();
    let mut expected = expected
        .as_object()
        .expect("expected known response is object")
        .clone();
    let actual_speakers = actual.remove("speakers").expect("actual speakers present");
    let expected_speakers = expected
        .remove("speakers")
        .expect("expected speakers present");
    assert_eq!(actual, expected, "{path}");
    let actual_speakers = actual_speakers
        .as_array()
        .expect("actual speakers are array");
    let expected_speakers = expected_speakers
        .as_array()
        .expect("expected speakers are array");
    assert_eq!(actual_speakers.len(), expected_speakers.len(), "{path}");
    for (actual, expected) in actual_speakers.iter().zip(expected_speakers) {
        let mut actual = actual
            .as_object()
            .expect("actual speaker is object")
            .clone();
        let mut expected = expected
            .as_object()
            .expect("expected speaker is object")
            .clone();
        let actual_p25 = actual.remove("intra_cosine_p25");
        let expected_p25 = expected.remove("intra_cosine_p25");
        assert_eq!(actual, expected, "{path}");
        match (actual_p25, expected_p25) {
            (Some(Value::Null), Some(Value::Null)) => {}
            (Some(actual), Some(expected)) => {
                let actual = actual.as_f64().expect("actual p25 is numeric");
                let expected = expected.as_f64().expect("expected p25 is numeric");
                assert!(
                    (actual - expected).abs() < 1e-4,
                    "{path}: p25 {actual} differs from {expected}"
                );
            }
            values => panic!("{path}: p25 nullability differs: {values:?}"),
        }
    }
}

fn content_manifest(root: &Path) -> BTreeMap<PathBuf, FileStamp> {
    let mut manifest = BTreeMap::new();
    collect_manifest(root, root, &mut manifest);
    manifest
}

fn collect_manifest(root: &Path, directory: &Path, manifest: &mut BTreeMap<PathBuf, FileStamp>) {
    for entry in fs::read_dir(directory).expect("journal directory reads") {
        let entry = entry.expect("journal directory entry reads");
        let path = entry.path();
        let relative = path.strip_prefix(root).expect("path is beneath root");
        if relative
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == "health")
        {
            continue;
        }
        if path.is_dir() {
            collect_manifest(root, &path, manifest);
        } else if path.is_file() {
            let metadata = fs::metadata(&path).expect("file metadata reads");
            let bytes = fs::read(&path).expect("file reads");
            manifest.insert(
                relative.to_path_buf(),
                FileStamp {
                    size: metadata.len(),
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                    modified: metadata.modified().expect("file mtime reads"),
                },
            );
        }
    }
}

#[tokio::test]
async fn every_populated_speakers_corpus_case_matches_the_port() {
    let journal = support::build_populated_journal();
    let asserted = sweep_populated_corpus(router(journal.root().to_path_buf())).await;
    assert_eq!(asserted, 45);
}

#[tokio::test]
async fn populated_corpus_sweep_does_not_change_journal_content() {
    let journal = support::build_populated_journal();
    let before = content_manifest(journal.root());
    let asserted = sweep_populated_corpus(router(journal.root().to_path_buf())).await;
    let after = content_manifest(journal.root());
    assert_eq!(asserted, 45);
    assert_eq!(
        before, after,
        "populated route sweep must not modify journal content"
    );
}
