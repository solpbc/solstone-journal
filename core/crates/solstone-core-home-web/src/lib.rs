// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::{
    Router,
    body::Body,
    http::{StatusCode, header},
    response::Response,
    routing::get,
};

mod assets;

pub fn routes(_journal_root: PathBuf) -> Router {
    Router::new()
        .route("/app/home/", get(assets::shell))
        .route("/app/home", get(shell_redirect))
        .route("/app/home/workspace", get(assets::workspace))
        .route("/app/home/static/home.js", get(assets::home_js))
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
    use std::fs;
    use std::path::{Path, PathBuf};

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use regex::Regex;
    use serde_json::Value;
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

    #[tokio::test]
    async fn home_routes_match_assets_and_session_gate() {
        let paths: [(&str, &str, &[u8]); 3] = [
            (
                "/app/home/",
                "text/html; charset=utf-8",
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../../solstone/convey/static/shell.html"
                )),
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
    async fn home_api_routes_remain_unconverted_until_native_api_routes_land() {
        // Retire when home's API routes land natively.
        let established = established_root();
        let router = shell_router(established.path());
        for path in ["/app/home/api/pulse", "/app/home/api/briefing"] {
            let response = get(router.clone(), path).await;
            assert_eq!(response.0, StatusCode::NOT_IMPLEMENTED, "{path}");
            let refusal: Value = serde_json::from_slice(&response.3).expect("refusal JSON");
            assert_eq!(refusal["reason_code"], "app_not_converted", "{path}");
        }
    }

    #[tokio::test]
    async fn unregistered_home_paths_remain_typed_unconverted_refusals() {
        let established = established_root();
        let router = shell_router(established.path());
        for path in [
            "/app/home/background",
            "/app/home/static/anything-else.js",
            "/app/home/nonexistent",
        ] {
            let response = get(router.clone(), path).await;
            assert_eq!(response.0, StatusCode::NOT_IMPLEMENTED, "{path}");
            let refusal: Value = serde_json::from_slice(&response.3).expect("refusal JSON");
            assert_eq!(refusal["reason_code"], "app_not_converted", "{path}");
        }
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

        let patterns = forbidden_patterns();
        assert_eq!(patterns.len(), 13);
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
