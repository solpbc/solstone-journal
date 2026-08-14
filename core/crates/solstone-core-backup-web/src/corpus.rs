use axum::{
    body::{Body, to_bytes},
    http::{Method, Request},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

async fn response_json(router: axum::Router, request: Request<Body>) -> (u16, Value) {
    let response = router.oneshot(request).await.expect("response");
    let status = response.status().as_u16();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&body).expect("json response"),
    )
}

#[tokio::test]
async fn corpus_replays_all_cases_with_only_the_deferred_root_deviation() {
    let corpus = crate::test_support::corpus();
    let mut asserted = 0;
    let mut deferred = 0;
    let mut gate = 0;
    for (phase, cases) in corpus["phases"].as_object().expect("phases") {
        let root = crate::test_support::root(phase);
        for case in cases.as_array().expect("cases") {
            let method: Method = case["method"]
                .as_str()
                .expect("method")
                .parse()
                .expect("valid method");
            let body = case
                .get("request_json")
                .map(|value| serde_json::to_vec(value).expect("request json"))
                .unwrap_or_default();
            let request = Request::builder()
                .method(method)
                .uri(case["path"].as_str().expect("path"))
                .body(Body::from(body))
                .expect("request");
            let response = solstone_core_convey_shell::router(root.path().to_path_buf())
                .oneshot(request)
                .await
                .expect("response");
            let actual = response.status().as_u16();
            let actual_body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("response body");
            let expected = case["status"].as_u64().expect("status") as u16;
            if phase == "unestablished" || phase == "corrupt" {
                assert_eq!(actual, expected, "{phase} {}", case["path"]);
                gate += 1;
            } else if case["path"] == "/app/backup/" {
                // W2 owns the root shell. It remains the only declared corpus deviation.
                assert_eq!(actual, 501, "{phase} deferred root");
                deferred += 1;
            } else {
                assert_eq!(actual, expected, "{phase} {}", case["path"]);
                // Device geometry is deliberately injected by the Python corpus driver;
                // its dedicated test below establishes the native arithmetic instead.
                if case["path"] != "/app/backup/offload/status" {
                    if let Some(expected_json) = case.get("json") {
                        assert_eq!(
                            serde_json::from_slice::<Value>(&actual_body).expect("json body"),
                            *expected_json,
                            "{phase} {}",
                            case["path"]
                        );
                    } else if let Some(expected_digest) = case.get("body_sha256") {
                        assert_eq!(
                            format!("{:x}", Sha256::digest(&actual_body)),
                            expected_digest.as_str().expect("digest"),
                            "{phase} {}",
                            case["path"]
                        );
                    }
                }
                asserted += 1;
            }
        }
    }
    assert_eq!((asserted, deferred, gate), (48, 4, 26));
}

#[tokio::test]
async fn status_shape_preserves_backup_phase_discrimination() {
    for phase in ["fresh", "enabled_never_run", "broken", "healthy"] {
        let root = crate::test_support::root(phase);
        let (status, body) = response_json(
            crate::routes(root.path().to_path_buf()),
            Request::get("/app/backup/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body.as_object().expect("object").len(), 17);
        if phase == "broken" {
            assert_eq!(body["last_backup"]["error_reason"], "locked");
            assert_eq!(body["last_verification"]["reason"], "read_data_mismatch");
        }
    }
}

#[tokio::test]
async fn offload_status_uses_injected_geometry_and_has_exact_shape() {
    let root = crate::test_support::root("fresh");
    let cache = crate::measurement::with_geometry(crate::measurement::DeviceGeometry {
        total_bytes: Some(1_000_000_000_000),
        free_bytes: Some(250_000_000_000),
    });
    let (status, body) = response_json(
        crate::routes_with_cache(root.path().to_path_buf(), cache),
        Request::get("/app/backup/offload/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body.as_object().expect("object").len(), 12);
    assert_eq!(
        body["device"],
        json!({"free_bytes":250_000_000_000u64,"total_bytes":1_000_000_000_000u64})
    );
    assert_eq!(
        body["suggested_defaults"],
        json!({"budget_bytes":500_000_000_000u64,"floor_bytes":100_000_000_000u64})
    );
}

#[tokio::test]
async fn generation_is_fill_only_and_confirmation_accepts_display_separators() {
    let root = crate::test_support::root("fresh");
    let first = crate::routes(root.path().to_path_buf());
    let (_, generated) = response_json(
        first,
        Request::post("/app/backup/keys/generate")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let display = generated["recovery_key_display"].as_str().expect("display");
    let (_, generated_again) = response_json(
        crate::routes(root.path().to_path_buf()),
        Request::post("/app/backup/keys/generate")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(generated_again["recovery_key_display"], display);
    let (_, confirmed) = response_json(
        crate::routes(root.path().to_path_buf()),
        Request::post("/app/backup/confirm")
            .body(Body::from(
                serde_json::to_vec(&json!({"recovery_key": display.replace(' ', "-")})).unwrap(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(confirmed["recovery_key_confirmed"], true);
}

#[tokio::test]
async fn retention_and_offload_config_persist_their_coerced_values() {
    let root = crate::test_support::root("fresh");
    let root_path = root.path().to_path_buf();
    let (status, _) = response_json(
        crate::routes(root_path.clone()),
        Request::post("/app/backup/retention")
            .body(Body::from(
                serde_json::to_vec(&json!({"hourly":"1","daily":2,"weekly":"3","monthly":4}))
                    .unwrap(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        crate::config::backup(&root_path).unwrap()["retention"],
        json!({"hourly":1,"daily":2,"weekly":3,"monthly":4})
    );
    let (status, _) = response_json(
        crate::routes(root_path.clone()),
        Request::post("/app/backup/offload/config")
            .body(Body::from(
                serde_json::to_vec(&json!({"budget_bytes":101,"floor_bytes":7})).unwrap(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        crate::config::backup(&root_path).unwrap()["offload"],
        json!({"enabled":false,"budget_bytes":101,"floor_bytes":7})
    );
}

#[test]
fn corpus_declares_the_deferred_root_deviation() {
    let corpus = crate::test_support::corpus();
    assert_eq!(
        corpus["phases"]
            .as_object()
            .expect("phases")
            .values()
            .map(|value| value.as_array().expect("cases").len())
            .sum::<usize>(),
        78
    );
    assert_eq!(
        crate::refuse::BACKUP_ENABLE_NOT_IMPLEMENTED_NATIVE,
        "backup_enable_not_implemented_native"
    );
}
