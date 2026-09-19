// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::ErrorKind;
use std::process::Command;

#[tokio::test]
async fn activity_edit_reports_corrupt_definitions_without_rewriting_them() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    let root = tempfile::tempdir().unwrap();
    let facet = root.path().join("facets/work");
    std::fs::create_dir_all(&facet).unwrap();
    std::fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();
    let router = solstone_core_settings_web::routes(root.path().to_path_buf());
    let request = || {
        Request::post("/app/settings/api/facet/work/activities")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"New activity"}"#))
            .unwrap()
    };
    let healthy = router.clone().oneshot(request()).await.unwrap();
    assert_eq!(healthy.status(), 201);
    let path = facet.join("activities/activities.jsonl");
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("new_activity"));
    let damaged = format!("{saved}{{sensitive-sentinel\n");
    std::fs::write(&path, &damaged).unwrap();
    let refused = router.oneshot(request()).await.unwrap();
    assert_eq!(refused.status(), 500);
    let bytes = to_bytes(refused.into_body(), 8192).await.unwrap();
    let response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(response["reason_code"], "settings_operation_failed");
    assert!(!String::from_utf8_lossy(&bytes).contains("sensitive-sentinel"));
    assert_eq!(std::fs::read(&path).unwrap(), damaged.as_bytes());
}

#[test]
fn facets_dom_contract() {
    match Command::new("node").arg("--version").output() {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            panic!("settings facets DOM harness requires node")
        }
        Err(error) => panic!("node availability probe failed: {error}"),
        Ok(output) if !output.status.success() => panic!(
            "node availability probe failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ),
        Ok(_) => {}
    }
    let output = Command::new("node")
        .arg("--max-old-space-size=512")
        .arg(format!(
            "{}/tests/facets_dom.js",
            env!("CARGO_MANIFEST_DIR")
        ))
        .arg(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("settings facets DOM harness starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "settings facets DOM harness failed:\nstdout:\n{}\nstderr:\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        stdout.starts_with("DOM CASES: ") && stdout.contains(" passed"),
        "settings facets DOM harness did not report its internal case count:\n{stdout}"
    );
    println!("{}", stdout.trim());
}
