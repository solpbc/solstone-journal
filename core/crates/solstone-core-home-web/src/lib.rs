// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::{
    Extension, Json, Router,
    body::Body,
    http::{StatusCode, header},
    response::Response,
    routing::{get, post},
};

mod assets;
mod clock;
mod removals;

pub use clock::Clock;

pub fn routes(journal_root: PathBuf, clock: Clock) -> Router {
    let pulse_root = journal_root.clone();
    let pulse_clock = clock.clone();
    let briefing_root = journal_root.clone();
    let removal_clock = clock.clone();
    Router::new()
        .route("/app/home/", get(assets::shell))
        .route("/app/home", get(shell_redirect))
        .route("/app/home/workspace", get(assets::workspace))
        .route("/app/home/static/home.js", get(assets::home_js))
        .route("/app/home/static/removals.js", get(assets::removals_js))
        .route("/app/home/api/removals", get(removals::list))
        .route("/app/home/api/approve", post(removals::approve))
        .route("/app/home/api/decline", post(removals::decline))
        .route("/app/home/api/recover", post(removals::recover))
        .route(
            "/app/home/api/pulse",
            get(move || pulse(pulse_root.clone(), pulse_clock.clone())),
        )
        .route(
            "/app/home/api/briefing",
            get(move || briefing(briefing_root.clone(), clock.clone())),
        )
        .layer(Extension(removal_clock))
        .with_state(journal_root)
}

async fn pulse(journal_root: PathBuf, clock: Clock) -> Json<serde_json::Value> {
    let context = solstone_core_home::HomeContext::new(journal_root, clock.now());
    Json(solstone_core_home::pulse::pulse_payload(&context))
}

async fn briefing(journal_root: PathBuf, clock: Clock) -> Json<serde_json::Value> {
    let context = solstone_core_home::HomeContext::new(journal_root, clock.now());
    Json(solstone_core_home::pulse::briefing_payload(&context))
}

async fn shell_redirect() -> Response {
    let location = "/app/home/";
    Response::builder()
        .status(StatusCode::PERMANENT_REDIRECT)
        .header(header::LOCATION, location)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(format!(
            "<!doctype html>\n<html lang=en>\n<title>Redirecting...</title>\n<h1>Redirecting...</h1>\n<p>You should be redirected automatically to the target URL: <a href=\"{location}\">{location}</a>. If not, click the link.\n"
        )))
        .expect("redirect response builds")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use chrono::{TimeZone, Timelike};
    use regex::Regex;
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn established_root() -> TempDir {
        let root = TempDir::new().expect("temporary journal");
        fs::create_dir_all(root.path().join("config")).expect("config directory");
        fs::write(
            root.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1700000000000}}"#,
        )
        .expect("established config");
        root
    }

    fn corrupt_root() -> TempDir {
        let root = TempDir::new().expect("temporary journal");
        fs::create_dir_all(root.path().join("config")).expect("config directory");
        fs::write(
            root.path().join("config/journal.json"),
            b"{\"setup\":{\"completed_at\":1",
        )
        .expect("corrupt config");
        root
    }

    fn shell_router(root: &Path) -> Router {
        solstone_core_convey_shell::router(root.to_path_buf())
    }

    fn seed_client_activity(
        root: &Path,
        last_seen_at: &str,
        last_accepted_ingest_at: Option<&str>,
    ) {
        fs::create_dir_all(root.join("link")).expect("link directory");
        fs::write(
            root.join("link/authorized_clients.json"),
            r#"[{"fingerprint":"cid","device_label":"desk","paired_at":"2026-01-01T00:00:00Z","instance_id":"instance","kind":"cert"}]"#,
        )
        .expect("authorized client");
        let mut activity = json!({"cid": {"last_seen_at": last_seen_at}});
        if let Some(last_accepted_ingest_at) = last_accepted_ingest_at {
            activity["cid"]["last_accepted_ingest_at"] = last_accepted_ingest_at.into();
        }
        fs::write(root.join("link/devices.json"), activity.to_string()).expect("client activity");
    }

    async fn get(router: Router, path: &str) -> (StatusCode, String, Option<String>, Vec<u8>) {
        let response = router
            .oneshot(Request::get(path).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body")
            .to_vec();
        (status, content_type, location, body)
    }

    async fn post(router: Router, path: &str, body: &'static str) -> (StatusCode, Option<String>) {
        let response = router
            .oneshot(
                Request::post(path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .expect("request"),
            )
            .await
            .expect("response");
        let status = response.status();
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        (status, location)
    }

    #[tokio::test]
    async fn home_routes_match_assets_and_session_gate() {
        let paths: [(&str, &str, &[u8]); 4] = [
            (
                "/app/home/",
                "text/html; charset=utf-8",
                include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html"),
            ),
            (
                "/app/home/workspace",
                "text/html; charset=utf-8",
                include_bytes!("../assets/workspace.html"),
            ),
            (
                "/app/home/static/home.js",
                "text/javascript; charset=utf-8",
                include_bytes!("../assets/home.js"),
            ),
            (
                "/app/home/static/removals.js",
                "text/javascript; charset=utf-8",
                include_bytes!("../assets/removals.js"),
            ),
        ];

        let established = established_root();
        let router = shell_router(established.path());
        for (path, content_type, expected_body) in paths {
            let response = get(router.clone(), path).await;
            assert_eq!(response.0, StatusCode::OK, "{path}");
            assert_eq!(response.1, content_type, "{path}");
            assert_eq!(response.3, expected_body, "{path}");
        }

        let unestablished = TempDir::new().expect("temporary journal");
        let router = shell_router(unestablished.path());
        for (path, _, _) in paths {
            let response = get(router.clone(), path).await;
            assert_eq!(response.0, StatusCode::FOUND, "{path}");
            assert_eq!(response.2.as_deref(), Some("/init"), "{path}");
        }

        let corrupt = corrupt_root();
        let router = shell_router(corrupt.path());
        for (path, _, _) in paths {
            let response = get(router.clone(), path).await;
            assert_eq!(response.0, StatusCode::INTERNAL_SERVER_ERROR, "{path}");
            assert_eq!(response.1, "text/plain; charset=utf-8", "{path}");
        }
    }

    #[tokio::test]
    async fn bare_home_path_redirects_to_the_trailing_slash() {
        let established = established_root();
        let response = get(shell_router(established.path()), "/app/home").await;
        assert_eq!(response.0, StatusCode::PERMANENT_REDIRECT);
        assert_eq!(response.1, "text/html; charset=utf-8");
        assert_eq!(response.2.as_deref(), Some("/app/home/"));
    }

    #[tokio::test]
    async fn home_api_routes_are_served_natively() {
        // Home API routes are served natively by this crate.
        let established = established_root();
        let router = shell_router(established.path());
        for (path, keys) in [
            ("/app/home/api/pulse", pulse_keys()),
            ("/app/home/api/briefing", briefing_keys()),
        ] {
            let response = get(router.clone(), path).await;
            assert_eq!(response.0, StatusCode::OK, "{path}");
            assert_eq!(response.1, "application/json", "{path}");
            assert_json_key_set(&response.3, &keys, path);
        }
    }

    #[tokio::test]
    async fn unregistered_home_paths_are_converted_not_found_responses() {
        let established = established_root();
        let router = shell_router(established.path());
        for path in [
            "/app/home/background",
            "/app/home/static/anything-else.js",
            "/app/home/nonexistent",
        ] {
            let response = get(router.clone(), path).await;
            assert_eq!(response.0, StatusCode::NOT_FOUND, "{path}");
            assert_eq!(response.1, "text/html; charset=utf-8", "{path}");
        }
    }

    #[tokio::test]
    async fn home_api_routes_follow_the_session_gate_for_both_payloads() {
        let established = established_root();
        let router = shell_router(established.path());
        for (path, key, expected) in [
            ("/app/home/api/pulse", "home_state", json!("welcome")),
            ("/app/home/api/briefing", "needs_shared_count", json!(0)),
        ] {
            let response = get(router.clone(), path).await;
            assert_eq!(response.0, StatusCode::OK, "{path}");
            let body: Value = serde_json::from_slice(&response.3).expect("handler JSON");
            assert_eq!(body[key], expected, "{path}");
        }

        let unestablished = TempDir::new().expect("temporary journal");
        let router = shell_router(unestablished.path());
        for path in ["/app/home/api/pulse", "/app/home/api/briefing"] {
            let response = get(router.clone(), path).await;
            assert_eq!(response.0, StatusCode::FOUND, "{path}");
            assert_eq!(response.2.as_deref(), Some("/init"), "{path}");
        }

        let corrupt = corrupt_root();
        let router = shell_router(corrupt.path());
        for path in ["/app/home/api/pulse", "/app/home/api/briefing"] {
            let response = get(router.clone(), path).await;
            assert_eq!(response.0, StatusCode::INTERNAL_SERVER_ERROR, "{path}");
            assert_eq!(response.1, "application/json", "{path}");
            let body: Value = serde_json::from_slice(&response.3).expect("corrupt API JSON");
            assert_eq!(body["reason_code"], "corrupt_config", "{path}");
        }
    }

    #[tokio::test]
    async fn recover_write_follows_the_session_gate() {
        let unestablished = TempDir::new().expect("temporary journal");
        let router = shell_router(unestablished.path());
        let (status, location) = post(router, "/app/home/api/recover", "{}").await;
        assert_eq!(status, StatusCode::FOUND);
        assert_eq!(location.as_deref(), Some("/init"));
    }

    #[tokio::test]
    async fn home_pulse_request_does_not_create_awareness_in_the_empty_fixture() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/convey_home_empty_journal");
        let before = tree_entries(&root);
        let router = super::routes(
            root.clone(),
            super::Clock::fixed(
                chrono::Utc
                    .with_ymd_and_hms(2026, 8, 14, 22, 28, 35)
                    .unwrap()
                    .with_nanosecond(430_840_000)
                    .unwrap(),
            ),
        );
        let response = get(router, "/app/home/api/pulse").await;
        assert_eq!(response.0, StatusCode::OK);
        assert_eq!(tree_entries(&root), before);
        assert!(!root.join("awareness").exists());
    }

    #[tokio::test]
    async fn pulse_handler_returns_capture_health_for_a_seeded_client() {
        let root = TempDir::new().expect("temporary journal");
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 8, 14, 22, 28, 35)
            .unwrap();
        let last_seen = (now - chrono::Duration::seconds(1)).to_rfc3339();
        seed_client_activity(root.path(), &last_seen, Some(&last_seen));
        let router = super::routes(root.path().to_path_buf(), super::Clock::fixed(now));
        let response = get(router, "/app/home/api/pulse").await;
        assert_eq!(response.0, StatusCode::OK);
        let body: Value = serde_json::from_slice(&response.3).expect("JSON");
        assert_eq!(
            body["capture_health"],
            json!({
                "status": "active",
                "clients": [{
                    "name": "desk",
                    "cid": "cid",
                    "last_seen": last_seen,
                    "last_accepted_ingest_at": last_seen,
                    "last_accepted_segment": null,
                    "status": "active",
                    "reach": "active"
                }],
                "unassessed": [],
                "registry": "registry_complete"
            })
        );
    }

    #[tokio::test]
    async fn pulse_handler_reports_never_delivered_as_unassessed() {
        let root = TempDir::new().expect("temporary journal");
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 8, 14, 22, 28, 35)
            .unwrap();
        let last_seen = (now - chrono::Duration::seconds(1)).to_rfc3339();
        seed_client_activity(root.path(), &last_seen, None);
        let router = super::routes(root.path().to_path_buf(), super::Clock::fixed(now));
        let response = get(router, "/app/home/api/pulse").await;
        assert_eq!(response.0, StatusCode::OK);
        let body: Value = serde_json::from_slice(&response.3).expect("JSON");
        assert_eq!(
            body["capture_health"],
            json!({
                "status": "no_clients",
                "clients": [],
                "unassessed": [{
                    "name": "desk",
                    "cid": "cid",
                    "reason": "awaiting_first_delivery",
                    "reach": "active"
                }],
                "registry": "registry_complete"
            })
        );
    }

    #[tokio::test]
    async fn pulse_handler_reports_empty_client_ledger_as_registry_empty() {
        let root = TempDir::new().expect("temporary journal");
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 8, 14, 22, 28, 35)
            .unwrap();
        let router = super::routes(root.path().to_path_buf(), super::Clock::fixed(now));
        let response = get(router, "/app/home/api/pulse").await;
        assert_eq!(response.0, StatusCode::OK);
        let body: Value = serde_json::from_slice(&response.3).expect("JSON");
        assert_eq!(
            body["capture_health"],
            json!({
                "status": "no_clients",
                "clients": [],
                "unassessed": [],
                "registry": "registry_empty"
            })
        );
    }

    fn pulse_keys() -> BTreeSet<String> {
        [
            "today",
            "now",
            "health_glance",
            "capture_health",
            "attention",
            "pipeline_status",
            "segment_count",
            "facet_data",
            "narrative_content",
            "narrative_updated_at",
            "narrative_source",
            "narrative_header",
            "pulse_needs",
            "flow_content",
            "flow_updated_at",
            "anticipated_activities",
            "activities",
            "needs_you_items",
            "briefing_sections",
            "briefing_meta",
            "briefing_phase",
            "briefing_lateness",
            "briefing_exists",
            "briefing_summary",
            "briefing_needs_deduped",
            "briefing_needs_shared_count",
            "briefing_needs_badge",
            "latest_weekly_reflection",
            "yesterday_processing",
            "connections",
            "journal_age_days",
            "home_state",
            "welcome_framing",
            "narrative_summary",
            "today_summary",
            "needs_summary",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn briefing_keys() -> BTreeSet<String> {
        [
            "exists",
            "phase",
            "summary",
            "meta",
            "sections",
            "needs_deduped",
            "needs_shared_count",
            "needs_badge",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn assert_json_key_set(bytes: &[u8], expected: &BTreeSet<String>, path: &str) {
        let body: Value = serde_json::from_slice(bytes).expect("API JSON");
        let keys = body
            .as_object()
            .expect("API object")
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(&keys, expected, "{path}");
    }

    fn tree_entries(root: &Path) -> Vec<String> {
        let mut entries =
            fs::read_dir(root)
                .expect("fixture root")
                .filter_map(Result::ok)
                .flat_map(|entry| {
                    let path = entry.path();
                    let mut entries = vec![path.strip_prefix(root).unwrap().display().to_string()];
                    if path.is_dir() {
                        entries.extend(tree_entries(&path).into_iter().map(|child| {
                            format!("{}/{}", entry.file_name().to_string_lossy(), child)
                        }));
                    }
                    entries
                })
                .collect::<Vec<_>>();
        entries.sort();
        entries
    }

    fn joined(parts: &[&str]) -> String {
        parts.concat()
    }

    fn forbidden_patterns() -> Vec<(String, Regex)> {
        [
            (
                joined(&["direct-std-process-command"]),
                joined(&[r"\bstd::process::Com", r"mand\b"]),
            ),
            (
                joined(&["direct-tokio-process"]),
                joined(&[r"\btokio::pro", r"cess\b"]),
            ),
            (
                joined(&["direct-process-command"]),
                joined(&[r"\bprocess::Com", r"mand\b"]),
            ),
            (
                joined(&["direct-command-new"]),
                joined(&[r"\bCom", r"mand::new\s*\("]),
            ),
            (
                joined(&["direct-spawn-call"]),
                joined(&[r"\.sp", r"awn\s*\("]),
            ),
            (
                joined(&["direct-output-call"]),
                joined(&[r"\.out", r"put\s*\("]),
            ),
            (
                joined(&["direct-exec-call"]),
                joined(&[r"\bex", r"ec(?:[lv][pe]?|ve)?\s*\("]),
            ),
            (
                joined(&["py", "o3-reference"]),
                joined(&[r"\b(?:py", r"o3|Py", r"O3)\b"]),
            ),
            (
                joined(&["cp", "ython-reference"]),
                joined(&[r"\b(?:cp", r"ython|CP", r"ython)\b"]),
            ),
            (
                joined(&["python-fallback-symbol"]),
                joined(&[r"\bpy", r"thon_(?:fall", r"back|dis", r"patch)\b"]),
            ),
            (
                joined(&["compat-dispatch-symbol"]),
                joined(&[r"\bcompat(?:ibility)?_dis", r"patch\b"]),
            ),
            (
                joined(&["fall", "back-to-python-symbol"]),
                joined(&[r"\bfall", r"back_to_py", r"thon\b"]),
            ),
            (
                joined(&["python-fallback-string"]),
                joined(&[
                    r#"\b(?:fall"#,
                    r#"back|dis"#,
                    r#"patch)[^\n\"]*py"#,
                    r"thon3?\b",
                ]),
            ),
        ]
        .into_iter()
        .map(|(name, pattern)| (name, Regex::new(&pattern).expect("valid audit pattern")))
        .collect()
    }

    fn walk_rust_sources(directory: &Path, sources: &mut Vec<(PathBuf, String)>) {
        for entry in fs::read_dir(directory).expect("source directory") {
            let entry = entry.expect("source entry");
            let path = entry.path();
            if path.is_dir() {
                walk_rust_sources(&path, sources);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                sources.push((
                    path,
                    fs::read_to_string(entry.path()).expect("Rust source reads"),
                ));
            }
        }
    }

    #[test]
    fn source_audit_rejects_spawn_or_python_dispatch_patterns() {
        // This audit reads this crate's source tree only, not its dependencies.
        // The repository native-sol spawn checker does not include this crate, and
        // CI's interpreter PATH poison only replaces a PATH lookup; it cannot prove
        // that a shipped interpreter invocation is absent.
        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sources = Vec::new();
        walk_rust_sources(&source_root, &mut sources);
        let visited = sources
            .iter()
            .map(|(path, _)| {
                path.strip_prefix(&source_root)
                    .expect("source is under root")
                    .display()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert!(visited.iter().any(|path| path == "lib.rs"));
        assert!(visited.iter().any(|path| path == "assets.rs"));
        assert!(visited.iter().any(|path| path == "clock.rs"));

        let patterns = forbidden_patterns();
        let probe = joined(&["Com", "mand::", "new("]);
        assert!(
            patterns
                .iter()
                .find(|(name, _)| name == "direct-command-new")
                .expect("command pattern")
                .1
                .is_match(&probe)
        );

        let violations = sources
            .iter()
            .flat_map(|(path, source)| {
                let relative = path
                    .strip_prefix(&source_root)
                    .expect("source is under root")
                    .display()
                    .to_string();
                patterns
                    .iter()
                    .filter(move |(_, pattern)| pattern.is_match(source))
                    .map(move |(name, _)| format!("{relative}: {name}"))
            })
            .collect::<Vec<_>>();
        assert!(
            violations.is_empty(),
            "forbidden source patterns: {violations:?}"
        );
    }
}
