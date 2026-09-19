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

#[tokio::test]
async fn activity_reads_report_damage_without_defaults_or_mutation() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    let root = tempfile::tempdir().unwrap();
    let facet = root.path().join("facets/work");
    std::fs::create_dir_all(&facet).unwrap();
    std::fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();
    let router = solstone_core_settings_web::routes(root.path().to_path_buf());
    let path = facet.join("activities/activities.jsonl");
    let request = || {
        Request::get("/app/settings/api/facet/work/activities")
            .body(Body::empty())
            .unwrap()
    };
    let absent = router.clone().oneshot(request()).await.unwrap();
    assert_eq!(absent.status(), 200);
    let defaults: Value =
        serde_json::from_slice(&to_bytes(absent.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    assert!(!defaults["activities"].as_array().unwrap().is_empty());
    assert!(!path.parent().unwrap().exists());
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    for empty in ["", "\n \t\n"] {
        std::fs::write(&path, empty).unwrap();
        let response = router.clone().oneshot(request()).await.unwrap();
        assert_eq!(response.status(), 200);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(body, defaults);
        assert_eq!(std::fs::read(&path).unwrap(), empty.as_bytes());
    }
    let healthy =
        json!({"id":"kept","name":"Kept","custom":true,"icon":"target","extra":{"preserve":1}})
            .to_string();
    for bad in [b"{sensitive-sentinel".as_slice(), b"42", &[0xff, 0xfe]] {
        let mut damaged = format!("{healthy}\n").into_bytes();
        damaged.extend_from_slice(bad);
        damaged.extend_from_slice(b"\n{\"id\":\"last\"}\n");
        std::fs::write(&path, &damaged).unwrap();
        let response = router.clone().oneshot(request()).await.unwrap();
        assert_eq!(response.status(), 500);
        let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["reason_code"], "settings_operation_failed");
        assert!(body.get("activities").is_none());
        assert!(!String::from_utf8_lossy(&bytes).contains("sensitive-sentinel"));
        assert_eq!(std::fs::read(&path).unwrap(), damaged);
    }
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        router.clone().oneshot(request()).await.unwrap().status(),
        500
    );
    assert!(path.is_dir());
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, &healthy).unwrap();
    let response = router.clone().oneshot(request()).await.unwrap();
    assert_eq!(response.status(), 200);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    let rows = body["activities"].as_array().unwrap();
    let custom = rows.iter().find(|row| row["id"] == "kept").unwrap();
    assert_eq!(custom["extra"], json!({"preserve":1}));
    assert!(custom["icon_svg"].as_str().unwrap().contains("<svg"));
    assert!(rows.iter().any(|row| row["always_on"] == true));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), healthy);
    let absent_facet = Request::get("/app/settings/api/facet/absent/activities")
        .body(Body::empty())
        .unwrap();
    assert_eq!(router.oneshot(absent_facet).await.unwrap().status(), 404);
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
