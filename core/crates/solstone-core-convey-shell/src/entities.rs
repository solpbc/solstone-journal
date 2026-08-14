// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::response::Response;

use crate::asset_response;

pub async fn shell() -> Response {
    asset_response("/static/shell.html")
}

pub async fn workspace() -> Response {
    asset_response("/app/entities/workspace")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use serde_json::{Value, json};
    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceDid};
    use solstone_core_convey_http::listener::bind_loopback;
    use solstone_core_convey_http::serve::{serve_connection, tcp_builder};
    use solstone_core_sol_link::DeviceDoorAuthorization;
    use solstone_core_sol_link::ledger::AuthorizedClientsRead;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::watch;
    use tower::ServiceExt;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    const VALID_DID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    struct Journal(PathBuf);

    impl Journal {
        fn new(config: Option<&[u8]>) -> Self {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock is after epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "solstone-entities-shell-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("temporary journal creates");
            if let Some(config) = config {
                fs::create_dir(path.join("config")).expect("config directory creates");
                fs::write(path.join("config/journal.json"), config).expect("journal config writes");
            }
            Self(path)
        }

        fn established() -> Self {
            Self::new(Some(br#"{"setup":{"completed_at":1767225600}}"#))
        }

        fn unestablished() -> Self {
            Self::new(None)
        }

        fn corrupt() -> Self {
            Self::new(Some(b"not json"))
        }

        fn seed_entity(&self, id: &str, name: &str) {
            let directory = self.0.join(format!("entities/{id}"));
            fs::create_dir_all(&directory).expect("entity directory creates");
            fs::write(
                directory.join("entity.json"),
                serde_json::to_vec(&json!({"id":id,"name":name,"type":"Person"}))
                    .expect("entity serializes"),
            )
            .expect("entity writes");
        }

        fn seed_facet_candidate(&self) {
            let directory = self.0.join("facets");
            fs::create_dir_all(&directory).expect("facets directory creates");
            fs::write(
                directory.join("review-candidates.jsonl"),
                b"{\"name_key\":\"alice\",\"name\":\"Alice\",\"status\":\"open\",\"count\":1}\n",
            )
            .expect("candidate writes");
        }

        fn authorize(&self, did: &str) {
            let directory = self.0.join("link");
            fs::create_dir_all(&directory).expect("link directory creates");
            fs::write(
                directory.join("authorized_clients.json"),
                serde_json::to_vec(&json!([{"fingerprint":did,"kind":"cert"}]))
                    .expect("ledger serializes"),
            )
            .expect("ledger writes");
        }
    }

    impl Drop for Journal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct SocketResponse {
        status: u16,
        headers: String,
        body: Vec<u8>,
    }

    impl SocketResponse {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers.lines().skip(1).find_map(|line| {
                let (actual, value) = line.split_once(':')?;
                actual.eq_ignore_ascii_case(name).then_some(value.trim())
            })
        }

        fn json(&self) -> Value {
            serde_json::from_slice(&self.body).expect("response JSON parses")
        }
    }

    async fn socket_request(
        root: PathBuf,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> SocketResponse {
        let listeners = bind_loopback(0).await.expect("loopback binds");
        let address = listeners.ipv4_addr().expect("IPv4 address");
        let task = tokio::spawn(async move {
            let (stream, identity) = listeners.accept().await.expect("connection accepts");
            let builder = tcp_builder();
            serve_connection(stream, crate::router(root), identity, &builder)
                .await
                .expect("connection serves");
        });

        let mut client = tokio::net::TcpStream::connect(address)
            .await
            .expect("client connects");
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        client
            .write_all(request.as_bytes())
            .await
            .expect("request writes");
        client.write_all(body).await.expect("body writes");
        let mut bytes = Vec::new();
        client
            .read_to_end(&mut bytes)
            .await
            .expect("response reads");
        task.await.expect("server task joins");

        let marker = b"\r\n\r\n";
        let header_end = bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("response has headers");
        let headers = String::from_utf8(bytes[..header_end].to_vec()).expect("headers are text");
        let status = headers
            .split_whitespace()
            .nth(1)
            .expect("status exists")
            .parse()
            .expect("status parses");
        SocketResponse {
            status,
            headers,
            body: bytes[header_end + marker.len()..].to_vec(),
        }
    }

    fn reference(relative: &str) -> Vec<u8> {
        fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(relative),
        )
        .expect("reference asset reads")
    }

    fn top_level_keys(value: &Value) -> BTreeSet<String> {
        value
            .as_object()
            .expect("response is object")
            .keys()
            .cloned()
            .collect()
    }

    async fn oneshot(
        app: Router,
        method: &str,
        path: &str,
        basis: AccessBasis,
    ) -> (StatusCode, String, Vec<u8>) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .expect("request builds");
        request.extensions_mut().insert(basis);
        let response = app.oneshot(request).await.expect("router responds");
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads")
            .to_vec();
        (status, content_type, body)
    }

    #[tokio::test]
    async fn entities_workspace_is_the_copied_embedded_asset() {
        let journal = Journal::established();
        let served = socket_request(journal.0.clone(), "GET", "/app/entities/workspace", b"").await;
        assert_eq!(served.status, 200);
        assert_eq!(
            served.header("content-type"),
            Some("text/html; charset=utf-8")
        );
        // Temporary W1 scaffolding: W3 removes this copy and this test with the Python source.
        // Until then, a Python-only edit must red the native crate that embeds the copied bytes.
        assert_eq!(
            served.body,
            reference("solstone/apps/entities/workspace.html")
        );

        let generated = include_str!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));
        let entry = generated
            .lines()
            .find(|line| line.contains("path: \"/app/entities/workspace\""))
            .expect("entities workspace is embedded");
        assert!(entry.contains(env!("CARGO_MANIFEST_DIR")));
        assert!(entry.contains("assets/entities/"));
        assert!(!entry.contains("solstone/apps/"));
    }

    #[tokio::test]
    async fn entities_shell_and_state_are_reachable_over_loopback() {
        let journal = Journal::established();
        let shell = socket_request(journal.0.clone(), "GET", "/app/entities/", b"").await;
        assert_eq!(shell.status, 200);
        assert_eq!(shell.body, reference("solstone/convey/static/shell.html"));

        let state = socket_request(journal.0.clone(), "GET", "/app/entities/api/state", b"").await;
        assert_eq!(state.status, 200);
        let state = state.json();
        assert_eq!(
            top_level_keys(&state),
            BTreeSet::from(["attendance_kinds".to_owned(), "entities_copy".to_owned()])
        );
        assert_eq!(
            state["entities_copy"]
                .as_object()
                .expect("copy is object")
                .len(),
            70
        );
        assert_eq!(
            state["attendance_kinds"],
            json!(["attended-with", "co-present", "scheduled-with"])
        );
    }

    #[tokio::test]
    async fn entities_types_journal_and_missing_move_match_the_native_surface() {
        let journal = Journal::established();
        journal.seed_entity("alice", "Alice");
        let types = socket_request(journal.0.clone(), "GET", "/app/entities/api/types", b"").await;
        assert_eq!(types.status, 200);
        assert_eq!(
            types.json()["types"],
            json!([{"name":"Person"},{"name":"Company"},{"name":"Project"},{"name":"Tool"}])
        );

        let entities =
            socket_request(journal.0.clone(), "GET", "/app/entities/api/journal", b"").await;
        assert_eq!(entities.status, 200);
        assert!(
            entities.json()["entities"]
                .as_array()
                .expect("entities is array")
                .iter()
                .any(|entity| entity["id"] == "alice")
        );

        let missing =
            socket_request(journal.0.clone(), "POST", "/app/entities/api/move", b"").await;
        assert_eq!(missing.status, 400);
        assert_eq!(missing.header("content-type"), Some("application/json"));
        assert_eq!(missing.json()["reason_code"], "missing_request_body");
    }

    #[tokio::test]
    async fn entities_and_curation_routes_keep_the_three_session_gate_outcomes() {
        for path in [
            "/app/entities/api/state",
            "/app/entities/",
            "/app/curation/api/facet/candidates",
        ] {
            let journal = Journal::unestablished();
            let unestablished = socket_request(journal.0.clone(), "GET", path, b"").await;
            assert_eq!(unestablished.status, 302, "{path}");
            assert_eq!(unestablished.header("location"), Some("/init"), "{path}");
        }

        let established = Journal::established();
        assert_eq!(
            socket_request(established.0.clone(), "GET", "/app/entities/", b"")
                .await
                .status,
            200
        );
        assert_eq!(
            socket_request(established.0.clone(), "GET", "/app/entities/api/state", b"")
                .await
                .status,
            200
        );
        assert_eq!(
            socket_request(
                established.0.clone(),
                "GET",
                "/app/curation/api/facet/candidates",
                b""
            )
            .await
            .status,
            200
        );

        for path in [
            "/app/entities/api/state",
            "/app/entities/",
            "/app/curation/api/facet/candidates",
        ] {
            let journal = Journal::corrupt();
            let corrupt = socket_request(journal.0.clone(), "GET", path, b"").await;
            assert_eq!(corrupt.status, 500, "{path}");
            if path == "/app/entities/" {
                assert_eq!(
                    corrupt.header("content-type"),
                    Some("text/plain; charset=utf-8")
                );
            } else {
                assert_eq!(corrupt.json()["reason_code"], "corrupt_config");
            }
        }
    }

    #[tokio::test]
    async fn entities_routes_preserve_shell_fallbacks_and_scoped_conversion() {
        let journal = Journal::established();
        let unknown =
            socket_request(journal.0.clone(), "GET", "/definitely-not-a-route", b"").await;
        assert_eq!(unknown.status, 404);
        assert_eq!(
            unknown.header("content-type"),
            Some("text/html; charset=utf-8")
        );
        assert!(
            String::from_utf8(unknown.body)
                .expect("body text")
                .contains("<title>404 Not Found</title>")
        );

        let entity_unknown =
            socket_request(journal.0.clone(), "GET", "/app/entities/no-such-thing", b"").await;
        assert_eq!(entity_unknown.status, 404);
        assert_eq!(
            entity_unknown.header("content-type"),
            Some("text/html; charset=utf-8")
        );

        let state = socket_request(journal.0.clone(), "GET", "/app/entities/api/state", b"").await;
        assert_ne!(state.status, 501);
        assert_ne!(
            state.json().get("reason_code"),
            Some(&Value::String("app_not_converted".to_owned()))
        );

        let activities =
            socket_request(journal.0.clone(), "GET", "/app/activities/workspace", b"").await;
        assert_eq!(activities.status, 501);
        assert_eq!(activities.json()["reason_code"], "app_not_converted");
        assert_eq!(activities.json()["app"], "activities");
    }

    #[tokio::test]
    async fn unported_entity_plates_and_assist_keep_their_reference_refusals() {
        let journal = Journal::established();
        // W1 scaffolding: W2 replaces these five index-plate refusals with implementations.
        for path in [
            "/app/entities/api/network?entity=x",
            "/app/entities/api/history?entity=x",
            "/app/entities/api/overview",
            "/app/entities/api/search?query=x",
            "/app/entities/api/work/detected/preview?name=x",
        ] {
            let response = socket_request(journal.0.clone(), "GET", path, b"").await;
            assert_eq!(response.status, 501, "{path}");
            assert_eq!(
                response.json()["reason_code"],
                "index_plate_not_ported",
                "{path}"
            );
        }
        // This is the reference's transient-outage response, not an honest not-ported signal.
        let assist = socket_request(
            journal.0.clone(),
            "POST",
            "/app/entities/api/work/assist",
            br#"{"name":"x"}"#,
        )
        .await;
        assert_eq!(assist.status, 503);
        assert_eq!(assist.json()["reason_code"], "agent_unavailable");
    }

    #[tokio::test]
    async fn curation_api_is_reachable_while_curation_workspace_remains_unconverted() {
        let journal = Journal::established();
        journal.seed_facet_candidate();
        // Curation deliberately has no converted workspace; its shared facet-store API arrives via entities.
        let candidates = socket_request(
            journal.0.clone(),
            "GET",
            "/app/curation/api/facet/candidates",
            b"",
        )
        .await;
        assert_eq!(candidates.status, 200);
        let candidates = candidates.json();
        assert_eq!(
            top_level_keys(&candidates),
            BTreeSet::from(["items".to_owned(), "total".to_owned()])
        );
        assert_eq!(candidates["total"], 1);
    }

    #[tokio::test]
    async fn paired_device_confinement_and_authorization_precede_entities_handlers() {
        let journal = Journal::established();
        let before = fs::read(journal.0.join("config/journal.json")).expect("config reads");
        let (_sender, receiver) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let app =
            crate::authorization_gate::authorized_router(journal.0.clone(), receiver).into_inner();
        let (status, content_type, body) = oneshot(
            app,
            "POST",
            "/app/entities/api/move",
            AccessBasis::PairingPeer {
                carrier: Carrier::Direct,
            },
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(content_type.starts_with("text/plain"));
        assert_eq!(body, b"pairing window closed");
        assert_eq!(
            fs::read(journal.0.join("config/journal.json")).expect("config rereads"),
            before
        );

        let authorized = Journal::established();
        authorized.authorize(VALID_DID);
        let (_sender, receiver) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let app = crate::authorization_gate::authorized_router(authorized.0.clone(), receiver)
            .into_inner();
        let did = LinkedDeviceDid::try_from(VALID_DID).expect("valid DID");
        let (status, content_type, body) = oneshot(
            app,
            "POST",
            "/app/entities/api/move",
            AccessBasis::LinkedDevice {
                carrier: Carrier::Direct,
                did,
            },
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(content_type, "application/json");
        assert_eq!(
            serde_json::from_slice::<Value>(&body).expect("JSON")["reason_code"],
            "missing_request_body"
        );

        let revoked = Journal::established();
        let (_sender, receiver) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let app =
            crate::authorization_gate::authorized_router(revoked.0.clone(), receiver).into_inner();
        let did = LinkedDeviceDid::try_from(VALID_DID).expect("valid DID");
        let (status, _content_type, body) = oneshot(
            app,
            "POST",
            "/app/entities/api/move",
            AccessBasis::LinkedDevice {
                carrier: Carrier::Direct,
                did,
            },
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).expect("JSON")["reason_code"],
            "pl_revoked"
        );
    }

    #[test]
    fn fallback_free_entities_router_merges_and_shell_constructs() {
        let journal = Journal::established();
        let _merged = Router::new()
            .route("/x", get(|| async { StatusCode::OK }))
            .fallback(|| async { StatusCode::NOT_FOUND })
            .merge(solstone_core_entities::api_router(&journal.0));
        let _shell = crate::router(journal.0.clone());
    }

    #[test]
    #[should_panic(expected = "Cannot merge two `Router`s that both have a fallback")]
    fn axum_rejects_two_fallback_carrying_routers() {
        let _ = Router::<()>::new()
            .fallback(|| async { StatusCode::NOT_FOUND })
            .merge(Router::new().fallback(|| async { StatusCode::NOT_FOUND }));
    }
}
