// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Certificate-authorized paired-client routes, mounted on Network.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::Router;
use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use serde_json::{Map, Value, json};
use solstone_core_sol_link::client_status::{
    ClientActivityState, ClientAssessment, ClientCaptureState, ClientInspection,
    ClientLedgerUnavailable, ClientReach, ConnectionFreshness, ConnectionGroup, ConnectionState,
    inspect_clients_at,
};
use solstone_core_sol_link::ledger::AuthorizationLedger;

use crate::JournalRoot;

const CLIENT_ENTRY_FIELDS: [&str; 24] = [
    "cid",
    "cid_short",
    "device_label",
    "client_label",
    "label_ordinal",
    "display_label",
    "paired_at",
    "role",
    "network",
    "kind",
    "last_seen_at",
    "last_accepted_ingest_at",
    "last_accepted_segment",
    "state",
    "group",
    "elapsed_ms",
    "clock_skew",
    "label",
    "reach",
    "capture_state",
    "capture_elapsed_ms",
    "unassessed_reason",
    "failing",
    "ingest_rejection",
];

pub(crate) fn router(prefix: &str) -> Router {
    Router::new()
        .route(&format!("{prefix}/api/clients"), get(list))
        .route(
            &format!("{prefix}/api/clients/{{cid}}"),
            axum::routing::delete(delete_client),
        )
}

pub(crate) async fn redirect_app() -> Redirect {
    Redirect::permanent("/app/network/")
}

pub(crate) async fn redirect_workspace() -> Redirect {
    Redirect::permanent("/app/network/workspace")
}

async fn list(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    match inspect_clients_at(&root.0, now_ms()) {
        ClientInspection::Empty { clients, activity }
        | ClientInspection::Ready { clients, activity } => Json(json!({
            "clients": clients
                .iter()
                .map(|client| client_json(client, activity))
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        ClientInspection::LedgerUnavailable { reason, .. } => {
            let (reason_code, detail) = match reason {
                ClientLedgerUnavailable::Unreadable => (
                    "authorization_ledger_unreadable",
                    "authorized-client ledger could not be read",
                ),
                ClientLedgerUnavailable::Malformed => (
                    "authorization_ledger_malformed",
                    "authorized-client ledger is invalid",
                ),
            };
            log::warn!("network clients could not read the authorization ledger: {reason_code}");
            crate::network::refusal(reason_code, detail, StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

async fn delete_client(
    Extension(root): Extension<Arc<JournalRoot>>,
    Path(cid): Path<String>,
) -> Response {
    match AuthorizationLedger::new(&root.0).remove(&cid) {
        Ok(outcome) if outcome.authorized_removed => Json(json!({"unpaired": cid})).into_response(),
        Ok(_) => crate::network::refusal(
            "paired_device_not_found",
            "paired device not found",
            StatusCode::NOT_FOUND,
        ),
        Err(error) => crate::network::unpair_mutation_refusal(error),
    }
}

fn client_json(client: &ClientAssessment, activity: ClientActivityState) -> Value {
    let entry = &client.client_entry;
    let (state, group, elapsed_ms, clock_skew, label, reach) = connection_fields(client);
    let (capture_state, unassessed_reason) = capture_fields(client, activity);
    let value = Map::from_iter([
        ("cid".to_owned(), json!(client.cid)),
        ("cid_short".to_owned(), json!(cid_short(&client.cid))),
        ("device_label".to_owned(), json!(entry.device_label)),
        ("client_label".to_owned(), json!(entry.client_label)),
        ("label_ordinal".to_owned(), json!(entry.label_ordinal)),
        ("display_label".to_owned(), json!(entry.display_label())),
        ("paired_at".to_owned(), json!(entry.paired_at)),
        ("role".to_owned(), json!(entry.role.as_wire())),
        ("network".to_owned(), json!(entry.network)),
        ("kind".to_owned(), json!(entry.kind)),
        (
            "last_seen_at".to_owned(),
            client.last_seen_at.clone().map_or(Value::Null, Value::from),
        ),
        (
            "last_accepted_ingest_at".to_owned(),
            client
                .last_accepted_ingest_at
                .clone()
                .map_or(Value::Null, Value::from),
        ),
        (
            "last_accepted_segment".to_owned(),
            client
                .last_accepted_segment
                .as_ref()
                .map_or(Value::Null, |segment| json!(segment)),
        ),
        ("state".to_owned(), state),
        ("group".to_owned(), group),
        ("elapsed_ms".to_owned(), elapsed_ms),
        ("clock_skew".to_owned(), clock_skew),
        ("label".to_owned(), label),
        ("reach".to_owned(), reach),
        ("capture_state".to_owned(), json!(capture_state)),
        (
            "capture_elapsed_ms".to_owned(),
            client.capture_elapsed_ms.map_or(Value::Null, Value::from),
        ),
        ("unassessed_reason".to_owned(), unassessed_reason),
        (
            "failing".to_owned(),
            json!(matches!(client.capture_state, ClientCaptureState::Degraded)),
        ),
        (
            "ingest_rejection".to_owned(),
            client
                .ingest_rejection
                .as_ref()
                .map_or(Value::Null, |rejection| json!(rejection)),
        ),
    ]);
    debug_assert_eq!(value.len(), CLIENT_ENTRY_FIELDS.len());
    Value::Object(value)
}

fn cid_short(cid: &str) -> String {
    cid.strip_prefix("sha256:")
        .unwrap_or(cid)
        .chars()
        .take(16)
        .collect()
}

fn connection_fields(client: &ClientAssessment) -> (Value, Value, Value, Value, Value, Value) {
    match &client.connection {
        ConnectionFreshness::Unknown => (
            json!("unknown"),
            json!("unknown"),
            Value::Null,
            Value::Null,
            json!("unknown"),
            json!("unknown"),
        ),
        ConnectionFreshness::Known {
            state,
            group,
            elapsed_ms,
            clock_skew,
            label,
            reach,
        } => (
            json!(connection_state(*state)),
            json!(connection_group(*group)),
            elapsed_ms.map_or(Value::Null, Value::from),
            json!(clock_skew),
            json!(label),
            json!(client_reach(*reach)),
        ),
    }
}

fn capture_fields(
    client: &ClientAssessment,
    activity: ClientActivityState,
) -> (&'static str, Value) {
    match client.capture_state {
        ClientCaptureState::Unknown => (
            "unknown",
            match activity {
                ClientActivityState::Unreadable => json!("activity_unreadable"),
                ClientActivityState::Malformed => json!("activity_malformed"),
                ClientActivityState::Present | ClientActivityState::Missing => {
                    json!("capture_activity_unknown")
                }
            },
        ),
        ClientCaptureState::NoCapture => ("no_capture", Value::Null),
        ClientCaptureState::Degraded => ("degraded", Value::Null),
        ClientCaptureState::Active => ("active", Value::Null),
        ClientCaptureState::Stale => ("stale", Value::Null),
        ClientCaptureState::Offline => ("offline", Value::Null),
    }
}

fn connection_state(state: ConnectionState) -> &'static str {
    match state {
        ConnectionState::Connected => "connected",
        ConnectionState::Stale => "stale",
        ConnectionState::Disconnected => "disconnected",
    }
}

fn connection_group(group: ConnectionGroup) -> &'static str {
    match group {
        ConnectionGroup::Active => "active",
        ConnectionGroup::Stale => "stale",
        ConnectionGroup::Inactive => "inactive",
    }
}

fn client_reach(reach: ClientReach) -> &'static str {
    match reach {
        ClientReach::Active => "active",
        ClientReach::Stale => "stale",
        ClientReach::Offline => "offline",
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_millis()
        .try_into()
        .expect("Unix timestamp fits i64")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::network::{NETWORK_DEVICE_FIELDS, NETWORK_ROUTE_PREFIXES};

    struct EstablishedJournal(tempfile::TempDir);

    impl EstablishedJournal {
        fn new() -> Self {
            let dir = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
            fs::create_dir(dir.path().join("config")).expect("config directory");
            fs::write(
                dir.path().join("config/journal.json"),
                br#"{"setup":{"completed_at":1767225600}}"#,
            )
            .expect("journal config");
            Self(dir)
        }

        fn write_ledger(&self, entries: Value) {
            let link = self.0.path().join("link");
            fs::create_dir_all(&link).expect("link directory");
            fs::write(link.join("authorized_clients.json"), entries.to_string())
                .expect("authorization ledger");
        }

        fn write_activity(&self, activity: Value) {
            let link = self.0.path().join("link");
            fs::create_dir_all(&link).expect("link directory");
            fs::write(link.join("devices.json"), activity.to_string()).expect("activity metadata");
        }
    }

    fn client(cid: &str, label: &str) -> Value {
        json!({
            "fingerprint": cid,
            "device_label": label,
            "paired_at": "2026-08-13T00:00:00Z",
            "instance_id": "device-instance",
            "role": "peer",
            "network": "home",
            "client_label": "Phone",
            "label_ordinal": 2,
            "kind": "cert",
        })
    }

    async fn request(app: axum::Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = app.oneshot(request).await.expect("router responds");
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, value)
    }

    #[tokio::test]
    async fn api_clients_projects_the_full_client_vocabulary_on_both_prefixes() {
        let cid = "sha256:0123456789abcdef0123456789abcdef";
        let journal = EstablishedJournal::new();
        journal.write_ledger(json!([client(cid, "phone")]));
        journal.write_activity(json!({
            cid: {
                "last_seen_at": "2026-08-13T00:01:00Z",
                "last_accepted_ingest_at": "2026-08-13T00:02:00Z",
                "last_accepted_segment": {"day": "20260813", "name": "120000"},
                "ingest_rejection": {
                    "reason_code": "event_append_failed",
                    "first": "2026-08-13T00:03:00Z",
                    "latest": "2026-08-13T00:04:00Z",
                    "active_count": 2
                }
            }
        }));
        let app = crate::router(journal.0.path().to_path_buf());

        for prefix in NETWORK_ROUTE_PREFIXES {
            let (status, body) = request(
                app.clone(),
                Request::get(format!("{prefix}/api/clients"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{prefix}");
            let row = &body["clients"][0];
            let fields = row
                .as_object()
                .expect("client row")
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            assert_eq!(
                fields,
                CLIENT_ENTRY_FIELDS.into_iter().collect::<BTreeSet<_>>(),
                "{prefix}"
            );
            assert_eq!(row["cid"], cid);
            assert_eq!(row["cid_short"], "0123456789abcdef");
            assert_eq!(
                row["last_accepted_segment"],
                json!({"day": "20260813", "name": "120000"})
            );
            assert_eq!(row["capture_state"], "degraded");
            assert!(row["failing"].as_bool().expect("failing boolean"));
        }
    }

    #[tokio::test]
    async fn api_clients_missing_ledger_is_empty_but_unavailable_ledger_is_visible() {
        let journal = EstablishedJournal::new();
        let app = crate::router(journal.0.path().to_path_buf());
        let (status, body) = request(
            app.clone(),
            Request::get("/app/network/api/clients")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"clients": []}));

        fs::create_dir_all(journal.0.path().join("link/authorized_clients.json"))
            .expect("unreadable ledger");
        let (status, body) = request(
            app.clone(),
            Request::get("/app/network/api/clients")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason_code"], "authorization_ledger_unreadable");

        fs::remove_dir_all(journal.0.path().join("link/authorized_clients.json"))
            .expect("remove unreadable ledger");
        fs::write(journal.0.path().join("link/authorized_clients.json"), "{")
            .expect("malformed ledger");
        let (status, body) = request(
            app,
            Request::get("/app/network/api/clients")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason_code"], "authorization_ledger_malformed");
    }

    #[tokio::test]
    async fn api_clients_distinguishes_missing_activity_from_activity_failure() {
        let cid = "sha256:0123456789abcdef0123456789abcdef";
        let journal = EstablishedJournal::new();
        journal.write_ledger(json!([client(cid, "phone")]));
        let app = crate::router(journal.0.path().to_path_buf());

        let (status, body) = request(
            app.clone(),
            Request::get("/app/network/api/clients")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["clients"][0]["capture_state"], "no_capture");
        assert!(body["clients"][0]["unassessed_reason"].is_null());

        fs::create_dir_all(journal.0.path().join("link/devices.json"))
            .expect("unreadable activity");
        let (status, body) = request(
            app.clone(),
            Request::get("/app/network/api/clients")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["clients"][0]["capture_state"], "unknown");
        assert_eq!(
            body["clients"][0]["unassessed_reason"],
            "activity_unreadable"
        );

        fs::remove_dir_all(journal.0.path().join("link/devices.json"))
            .expect("remove unreadable activity");
        fs::write(journal.0.path().join("link/devices.json"), "{").expect("malformed activity");
        let (status, body) = request(
            app,
            Request::get("/app/network/api/clients")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["clients"][0]["capture_state"], "unknown");
        assert_eq!(
            body["clients"][0]["unassessed_reason"],
            "activity_malformed"
        );
    }

    #[tokio::test]
    async fn api_clients_deletes_by_cid_and_observer_routes_are_gone_on_both_prefixes() {
        let network_cid = "sha256:0123456789abcdef0123456789abcdef";
        let link_cid = "sha256:abcdef0123456789abcdef0123456789";
        let journal = EstablishedJournal::new();
        journal.write_ledger(json!([
            client(network_cid, "phone"),
            client(link_cid, "laptop"),
        ]));
        let app = crate::router(journal.0.path().to_path_buf());

        for prefix in NETWORK_ROUTE_PREFIXES {
            for request in [
                Request::get(format!("{prefix}/api/observers"))
                    .body(Body::empty())
                    .expect("observer list request"),
                Request::delete(format!("{prefix}/api/observers/missing"))
                    .body(Body::empty())
                    .expect("observer delete request"),
                Request::get(format!("{prefix}/api/observers/missing/key"))
                    .body(Body::empty())
                    .expect("observer key request"),
                Request::post(format!("{prefix}/api/observers/create"))
                    .body(Body::empty())
                    .expect("observer create request"),
            ] {
                let response = app
                    .clone()
                    .oneshot(request)
                    .await
                    .expect("observer response");
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{prefix}");
            }
        }

        let (status, body) = request(
            app.clone(),
            Request::delete(format!("/app/network/api/clients/{network_cid}"))
                .body(Body::empty())
                .expect("delete request"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"unpaired": network_cid}));

        let (status, body) = request(
            app.clone(),
            Request::delete(format!("/app/link/api/clients/{link_cid}"))
                .body(Body::empty())
                .expect("link delete request"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"unpaired": link_cid}));

        let (status, body) = request(
            app,
            Request::delete(format!("/app/link/api/clients/{link_cid}"))
                .body(Body::empty())
                .expect("unknown delete request"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["reason_code"], "paired_device_not_found");
    }

    #[tokio::test]
    async fn devices_redirects_remain_and_the_raw_device_projection_has_no_observer_handle() {
        let cid = "sha256:0123456789abcdef0123456789abcdef";
        let journal = EstablishedJournal::new();
        journal.write_ledger(json!([client(cid, "phone")]));
        journal.write_activity(json!({cid: {"last_seen_at": "2026-08-13T00:01:00Z"}}));
        let app = crate::router(journal.0.path().to_path_buf());

        for (path, location) in [
            ("/app/devices", "/app/network/"),
            ("/app/devices/", "/app/network/"),
            ("/app/devices/workspace", "/app/network/workspace"),
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).expect("request"))
                .await
                .expect("redirect response");
            assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT, "{path}");
            assert_eq!(response.headers()[header::LOCATION], location, "{path}");
        }

        let (status, body) = request(
            app,
            Request::get("/app/network/api/devices")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let row = body["devices"][0].as_object().expect("raw device row");
        assert_eq!(row.len(), 10);
        assert!(!row.contains_key("observer_handle"));
        assert_eq!(
            row.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            NETWORK_DEVICE_FIELDS.into_iter().collect::<BTreeSet<_>>()
        );
    }
}
