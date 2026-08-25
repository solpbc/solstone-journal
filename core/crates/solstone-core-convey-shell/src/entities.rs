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

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{HeaderMap, Request, StatusCode};
    use axum::routing::get;
    use serde_json::{Value, json};
    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceDid};
    use solstone_core_sol_link::DeviceDoorAuthorization;
    use solstone_core_sol_link::ledger::AuthorizedClientsRead;
    use tokio::sync::watch;
    use tower::ServiceExt;

    const VALID_DID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    struct Journal(tempfile::TempDir);

    impl Journal {
        fn new(config: Option<&[u8]>) -> Self {
            let dir = tempfile::TempDir::new_in("/var/tmp").expect("temporary journal creates");
            if let Some(config) = config {
                fs::create_dir(dir.path().join("config")).expect("config directory creates");
                fs::write(dir.path().join("config/journal.json"), config)
                    .expect("journal config writes");
            }
            Self(dir)
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
            let directory = self.0.path().join(format!("entities/{id}"));
            fs::create_dir_all(&directory).expect("entity directory creates");
            fs::write(
                directory.join("entity.json"),
                serde_json::to_vec(&json!({"id":id,"name":name,"type":"Person"}))
                    .expect("entity serializes"),
            )
            .expect("entity writes");
        }

        fn seed_facet_candidate(&self) {
            let directory = self.0.path().join("facets");
            fs::create_dir_all(&directory).expect("facets directory creates");
            fs::write(
                directory.join("review-candidates.jsonl"),
                b"{\"name_key\":\"alice\",\"name\":\"Alice\",\"status\":\"open\",\"count\":1}\n",
            )
            .expect("candidate writes");
        }

        fn authorize(&self, did: &str) {
            let directory = self.0.path().join("link");
            fs::create_dir_all(&directory).expect("link directory creates");
            fs::write(
                directory.join("authorized_clients.json"),
                serde_json::to_vec(&json!([{"fingerprint":did,"kind":"cert"}]))
                    .expect("ledger serializes"),
            )
            .expect("ledger writes");
        }
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
        body: &[u8],
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::from(body.to_vec()))
            .expect("request builds");
        request.extensions_mut().insert(basis);
        let response = app.oneshot(request).await.expect("router responds");
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads")
            .to_vec();
        (status, headers, body)
    }

    fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
        headers.get(name).and_then(|value| value.to_str().ok())
    }

    fn json_body(body: &[u8]) -> Value {
        serde_json::from_slice(body).expect("response JSON parses")
    }

    async fn routed(
        root: &Journal,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        oneshot(
            crate::router(root.0.path().to_path_buf()),
            method,
            path,
            AccessBasis::Localhost,
            body,
        )
        .await
    }

    #[tokio::test]
    async fn entities_workspace_is_the_copied_embedded_asset() {
        let journal = Journal::established();
        let (status, headers, body) = routed(&journal, "GET", "/app/entities/workspace", b"").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            header(&headers, "content-type"),
            Some("text/html; charset=utf-8")
        );
        // The Python source is retired, so the surviving assertion is that the
        // route serves this crate's own asset rather than something generated.
        assert_eq!(
            body,
            include_bytes!("../assets/entities/workspace.html").as_slice()
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
    async fn entities_shell_and_state_are_reachable_through_the_router() {
        let journal = Journal::established();
        let (status, _headers, body) = routed(&journal, "GET", "/app/entities/", b"").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, include_bytes!("../assets/static/shell.html"));

        let (status, _headers, body) =
            routed(&journal, "GET", "/app/entities/api/state", b"").await;
        assert_eq!(status, StatusCode::OK);
        let state = json_body(&body);
        assert_eq!(
            top_level_keys(&state),
            BTreeSet::from(["attendance_kinds".to_owned(), "entities_copy".to_owned()])
        );
        assert_eq!(
            state["entities_copy"]
                .as_object()
                .expect("copy is object")
                .len(),
            78
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
        let (status, _headers, body) =
            routed(&journal, "GET", "/app/entities/api/types", b"").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json_body(&body)["types"],
            json!([{"name":"Person"},{"name":"Company"},{"name":"Project"},{"name":"Tool"}])
        );

        let (status, _headers, body) =
            routed(&journal, "GET", "/app/entities/api/journal", b"").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            json_body(&body)["entities"]
                .as_array()
                .expect("entities is array")
                .iter()
                .any(|entity| entity["id"] == "alice")
        );

        let (status, headers, body) = routed(&journal, "POST", "/app/entities/api/move", b"").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(header(&headers, "content-type"), Some("application/json"));
        assert_eq!(json_body(&body)["reason_code"], "missing_request_body");
    }

    #[tokio::test]
    async fn entities_and_curation_routes_keep_the_three_session_gate_outcomes() {
        for path in [
            "/app/entities/api/state",
            "/app/entities/",
            "/app/curation/api/facet/candidates",
        ] {
            let journal = Journal::unestablished();
            let (status, headers, _body) = routed(&journal, "GET", path, b"").await;
            assert_eq!(status, StatusCode::FOUND, "{path}");
            assert_eq!(header(&headers, "location"), Some("/init"), "{path}");
        }

        let established = Journal::established();
        assert_eq!(
            routed(&established, "GET", "/app/entities/", b"").await.0,
            StatusCode::OK
        );
        assert_eq!(
            routed(&established, "GET", "/app/entities/api/state", b"")
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            routed(
                &established,
                "GET",
                "/app/curation/api/facet/candidates",
                b""
            )
            .await
            .0,
            StatusCode::OK
        );

        for path in [
            "/app/entities/api/state",
            "/app/entities/",
            "/app/curation/api/facet/candidates",
        ] {
            let journal = Journal::corrupt();
            let (status, headers, body) = routed(&journal, "GET", path, b"").await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{path}");
            if path == "/app/entities/" {
                assert_eq!(
                    header(&headers, "content-type"),
                    Some("text/plain; charset=utf-8")
                );
            } else {
                assert_eq!(json_body(&body)["reason_code"], "corrupt_config");
            }
        }
    }

    #[tokio::test]
    async fn entities_routes_preserve_shell_fallbacks_and_scoped_conversion() {
        let journal = Journal::established();
        let (status, headers, body) = routed(&journal, "GET", "/definitely-not-a-route", b"").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            header(&headers, "content-type"),
            Some("text/html; charset=utf-8")
        );
        assert!(
            String::from_utf8(body)
                .expect("body text")
                .contains("<title>404 Not Found</title>")
        );

        let (status, headers, _body) =
            routed(&journal, "GET", "/app/entities/no-such-thing", b"").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            header(&headers, "content-type"),
            Some("text/html; charset=utf-8")
        );

        let (status, _headers, body) =
            routed(&journal, "GET", "/app/entities/api/state", b"").await;
        assert_ne!(status, StatusCode::NOT_IMPLEMENTED);
        assert_ne!(
            json_body(&body).get("reason_code"),
            Some(&Value::String("app_not_converted".to_owned()))
        );

        let (status, _headers, body) =
            routed(&journal, "GET", "/app/activities/workspace", b"").await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(json_body(&body)["reason_code"], "app_not_converted");
        assert_eq!(json_body(&body)["app"], "activities");
    }

    #[tokio::test]
    async fn unported_entity_plates_and_assist_keep_their_reference_refusals() {
        let journal = Journal::established();
        for path in ["/app/entities/api/search?query=x"] {
            let (status, _headers, body) = routed(&journal, "GET", path, b"").await;
            assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{path}");
            assert_eq!(
                json_body(&body)["reason_code"],
                "index_plate_not_ported",
                "{path}"
            );
        }
        for path in [
            "/app/entities/api/network?entity=x",
            "/app/entities/api/history?entity=x",
        ] {
            let (status, _headers, body) = routed(&journal, "GET", path, b"").await;
            assert_eq!(status, StatusCode::OK, "{path}");
            let payload = json_body(&body);
            assert_eq!(payload["resolved"], Value::Null, "{path}");
            assert_eq!(payload["query"], "x", "{path}");
            assert!(payload["candidates"].is_array(), "{path}");
        }
        let (status, _headers, body) =
            routed(&journal, "GET", "/app/entities/api/overview", b"").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json_body(&body)["reason_code"], "edge_index_unavailable");
        // Honest not-ported refusal: native talent spawn is not available on this route.
        let (status, _headers, body) = routed(
            &journal,
            "POST",
            "/app/entities/api/work/assist",
            br#"{"name":"x"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(json_body(&body)["reason_code"], "talent_not_ported");
    }

    #[tokio::test]
    async fn curation_api_is_reachable_through_the_converted_curation_workspace() {
        let journal = Journal::established();
        journal.seed_facet_candidate();
        // Curation is a converted workspace whose facet-store API is shared with entities.
        let (status, _headers, body) =
            routed(&journal, "GET", "/app/curation/api/facet/candidates", b"").await;
        assert_eq!(status, StatusCode::OK);
        let candidates = json_body(&body);
        assert_eq!(
            top_level_keys(&candidates),
            BTreeSet::from(["items".to_owned(), "total".to_owned()])
        );
        assert_eq!(candidates["total"], 1);
    }

    #[tokio::test]
    async fn paired_device_confinement_and_authorization_precede_entities_handlers() {
        let journal = Journal::established();
        let before = fs::read(journal.0.path().join("config/journal.json")).expect("config reads");
        let (_sender, receiver) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let app =
            crate::authorization_gate::authorized_router(journal.0.path().to_path_buf(), receiver)
                .into_inner();
        let (status, headers, body) = oneshot(
            app,
            "POST",
            "/app/entities/api/move",
            AccessBasis::PairingPeer {
                carrier: Carrier::Direct,
            },
            b"",
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(
            header(&headers, "content-type")
                .unwrap_or_default()
                .starts_with("text/plain")
        );
        assert_eq!(body, b"pairing window closed");
        assert_eq!(
            fs::read(journal.0.path().join("config/journal.json")).expect("config rereads"),
            before
        );

        let authorized = Journal::established();
        authorized.authorize(VALID_DID);
        let (_sender, receiver) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let app = crate::authorization_gate::authorized_router(
            authorized.0.path().to_path_buf(),
            receiver,
        )
        .into_inner();
        let did = LinkedDeviceDid::try_from(VALID_DID).expect("valid DID");
        let (status, headers, body) = oneshot(
            app,
            "POST",
            "/app/entities/api/move",
            AccessBasis::LinkedDevice {
                carrier: Carrier::Direct,
                did,
            },
            b"",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(header(&headers, "content-type"), Some("application/json"));
        assert_eq!(
            serde_json::from_slice::<Value>(&body).expect("JSON")["reason_code"],
            "missing_request_body"
        );

        let revoked = Journal::established();
        let (_sender, receiver) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let app =
            crate::authorization_gate::authorized_router(revoked.0.path().to_path_buf(), receiver)
                .into_inner();
        let did = LinkedDeviceDid::try_from(VALID_DID).expect("valid DID");
        let (status, _headers, body) = oneshot(
            app,
            "POST",
            "/app/entities/api/move",
            AccessBasis::LinkedDevice {
                carrier: Carrier::Direct,
                did,
            },
            b"",
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
            .merge(solstone_core_entities::api_router(journal.0.path()));
        let _shell = crate::router(journal.0.path().to_path_buf());
    }

    #[test]
    #[should_panic(expected = "Cannot merge two `Router`s that both have a fallback")]
    fn axum_rejects_two_fallback_carrying_routers() {
        let _ = Router::<()>::new()
            .fallback(|| async { StatusCode::NOT_FOUND })
            .merge(Router::new().fallback(|| async { StatusCode::NOT_FOUND }));
    }
}
