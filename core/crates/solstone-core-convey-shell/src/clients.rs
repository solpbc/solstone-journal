// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Certificate-authorized paired-client routes, mounted on Network.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Extension, Path};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use serde_json::{Map, Value, json};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_sol_link::client_description::{
    JournalIdentityMeta, PatchClientLabelRequest, PutSelfDescriptionRequest,
    StoredClientDescription, current_display_label,
};
use solstone_core_sol_link::client_description_store::{
    DescriptionMutationError, get_description_response, patch_owner_label, put_self_description,
    read_descriptions,
};
use solstone_core_sol_link::client_status::{
    ClientActivityState, ClientAssessment, ClientCaptureState, ClientInspection,
    ClientLedgerUnavailable, ClientReach, ConnectionFreshness, ConnectionGroup, ConnectionState,
    SourceDelivery, inspect_clients_at,
};
use solstone_core_sol_link::ledger::AuthorizationLedger;

use crate::JournalRoot;

const CLIENT_ENTRY_FIELDS: [&str; 29] = [
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
    "source_delivery",
    "reported",
    "owner_label",
    "description_revision",
    "description_updated_at",
];

pub(crate) fn router(prefix: &str) -> Router {
    Router::new()
        .route(&format!("{prefix}/api/clients"), get(list))
        .route(
            &format!("{prefix}/api/clients/self"),
            get(get_self)
                .put(put_self)
                .layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route(
            &format!("{prefix}/api/clients/{{cid}}"),
            axum::routing::delete(delete_client),
        )
        .route(
            &format!("{prefix}/api/clients/{{cid}}/label"),
            axum::routing::patch(patch_label).layer(DefaultBodyLimit::max(16 * 1024)),
        )
}

pub(crate) async fn redirect_app() -> Redirect {
    Redirect::permanent("/app/network/")
}

pub(crate) async fn redirect_workspace() -> Redirect {
    Redirect::permanent("/app/network/workspace")
}

fn journal_identity_meta(journal_root: &std::path::Path) -> JournalIdentityMeta {
    let name = match solstone_core_spl::load_link_state(journal_root, "solstone") {
        solstone_core_spl::LinkStateRead::Present(state) => Some(state.home_label),
        _ => None,
    };
    JournalIdentityMeta {
        name,
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

async fn get_self(
    Extension(root): Extension<Arc<JournalRoot>>,
    basis: Option<Extension<AccessBasis>>,
) -> Response {
    let Some(Extension(basis)) = basis else {
        return crate::network::refusal(
            "client_description_forbidden",
            "access basis required",
            StatusCode::FORBIDDEN,
        );
    };
    let cid = match basis {
        AccessBasis::LinkedDevice { cid, .. } => cid.as_str().to_owned(),
        AccessBasis::Localhost | AccessBasis::PairingPeer { .. } => {
            return crate::network::refusal(
                "client_description_forbidden",
                "linked device access required",
                StatusCode::FORBIDDEN,
            );
        }
    };
    let meta = journal_identity_meta(&root.0);
    match get_description_response(&root.0, &cid, false, meta) {
        Ok(resp) => Json(resp).into_response(),
        Err(DescriptionMutationError::NotAuthorized) => crate::network::refusal(
            "client_description_forbidden",
            "client is not authorized",
            StatusCode::FORBIDDEN,
        ),
        Err(DescriptionMutationError::UnreadableStore(_)) => crate::network::refusal(
            "client_description_unreadable",
            "client description store could not be read",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        Err(DescriptionMutationError::UnreadableLedger(_)) => crate::network::refusal(
            "authorization_ledger_unreadable",
            "authorized-client ledger could not be read",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        Err(_) => crate::network::refusal(
            "client_description_unreadable",
            "client description could not be read",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    }
}

async fn put_self(
    Extension(root): Extension<Arc<JournalRoot>>,
    basis: Option<Extension<AccessBasis>>,
    body: Result<Json<PutSelfDescriptionRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Some(Extension(basis)) = basis else {
        return crate::network::refusal(
            "client_description_forbidden",
            "access basis required",
            StatusCode::FORBIDDEN,
        );
    };
    let cid = match basis {
        AccessBasis::LinkedDevice { cid, .. } => cid.as_str().to_owned(),
        AccessBasis::Localhost | AccessBasis::PairingPeer { .. } => {
            return crate::network::refusal(
                "client_description_forbidden",
                "linked device access required",
                StatusCode::FORBIDDEN,
            );
        }
    };
    let Json(request) = match body {
        Ok(json) => json,
        Err(_) => {
            return crate::network::refusal(
                "client_description_invalid",
                "invalid client description payload",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    let meta = journal_identity_meta(&root.0);
    let now = time::OffsetDateTime::now_utc();
    match put_self_description(&root.0, &cid, request, now, meta) {
        Ok(resp) => Json(resp).into_response(),
        Err(DescriptionMutationError::NotAuthorized) => crate::network::refusal(
            "client_description_forbidden",
            "client is not authorized",
            StatusCode::FORBIDDEN,
        ),
        Err(DescriptionMutationError::RevisionConflict) => crate::network::refusal(
            "revision_conflict",
            "revision conflict",
            StatusCode::CONFLICT,
        ),
        Err(DescriptionMutationError::Invalid(detail)) => crate::network::refusal(
            "client_description_invalid",
            detail,
            StatusCode::BAD_REQUEST,
        ),
        Err(DescriptionMutationError::UnreadableStore(_)) => crate::network::refusal(
            "client_description_unreadable",
            "client description store could not be read",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        Err(DescriptionMutationError::UnreadableLedger(_)) => crate::network::refusal(
            "authorization_ledger_unreadable",
            "authorized-client ledger could not be read",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        Err(_) => crate::network::refusal(
            "client_description_unreadable",
            "failed to update client description",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    }
}

async fn patch_label(
    Extension(root): Extension<Arc<JournalRoot>>,
    basis: Option<Extension<AccessBasis>>,
    Path(cid): Path<String>,
    body: Result<Json<PatchClientLabelRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Some(Extension(basis)) = basis else {
        return crate::network::refusal(
            "client_description_forbidden",
            "access basis required",
            StatusCode::FORBIDDEN,
        );
    };
    match basis {
        AccessBasis::Localhost => {}
        AccessBasis::LinkedDevice { .. } | AccessBasis::PairingPeer { .. } => {
            return crate::network::refusal(
                "client_description_forbidden",
                "owner localhost access required",
                StatusCode::FORBIDDEN,
            );
        }
    }
    let Json(request) = match body {
        Ok(json) => json,
        Err(_) => {
            return crate::network::refusal(
                "client_description_invalid",
                "invalid client label payload",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    let meta = journal_identity_meta(&root.0);
    let now = time::OffsetDateTime::now_utc();
    match patch_owner_label(&root.0, &cid, request.label, now, meta) {
        Ok(resp) => Json(resp).into_response(),
        Err(DescriptionMutationError::NotFound) => crate::network::refusal(
            "not_found",
            "paired device not found",
            StatusCode::NOT_FOUND,
        ),
        Err(DescriptionMutationError::Invalid(detail)) => crate::network::refusal(
            "client_description_invalid",
            detail,
            StatusCode::BAD_REQUEST,
        ),
        Err(DescriptionMutationError::UnreadableStore(_)) => crate::network::refusal(
            "client_description_unreadable",
            "client description store could not be read",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        Err(DescriptionMutationError::UnreadableLedger(_)) => crate::network::refusal(
            "authorization_ledger_unreadable",
            "authorized-client ledger could not be read",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        Err(_) => crate::network::refusal(
            "client_description_unreadable",
            "failed to update client label",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    }
}

async fn list(
    Extension(root): Extension<Arc<JournalRoot>>,
    basis: Option<Extension<AccessBasis>>,
) -> Response {
    let descriptions = match read_descriptions(&root.0) {
        Ok(descriptions) => descriptions,
        Err(_) => {
            return crate::network::refusal(
                "client_description_unreadable",
                "client description store could not be read",
                StatusCode::SERVICE_UNAVAILABLE,
            );
        }
    };
    match inspect_clients_at(&root.0, now_ms()) {
        ClientInspection::Empty { clients, activity }
        | ClientInspection::Ready { clients, activity } => Json(json!({
            "can_edit_labels": matches!(basis, Some(Extension(AccessBasis::Localhost))),
            "clients": clients
                .iter()
                .map(|client| client_json(client, activity, descriptions.get(&client.cid)))
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
                ClientLedgerUnavailable::DuplicateCid => (
                    "authorization_ledger_duplicate_cid",
                    "authorized-client ledger contains a duplicate client identifier",
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

fn client_json(
    client: &ClientAssessment,
    activity: ClientActivityState,
    stored_desc: Option<&StoredClientDescription>,
) -> Value {
    let entry = &client.client_entry;
    let (state, group, elapsed_ms, clock_skew, label, reach) = connection_fields(client);
    let (capture_state, unassessed_reason) = capture_fields(client, activity);
    let display_label = current_display_label(entry, stored_desc);
    let value = Map::from_iter([
        ("cid".to_owned(), json!(client.cid)),
        ("cid_short".to_owned(), json!(cid_short(&client.cid))),
        ("device_label".to_owned(), json!(entry.device_label)),
        ("client_label".to_owned(), json!(entry.client_label)),
        ("label_ordinal".to_owned(), json!(entry.label_ordinal)),
        ("display_label".to_owned(), json!(display_label)),
        (
            "reported".to_owned(),
            json!(stored_desc.and_then(|s| s.reported.as_ref())),
        ),
        (
            "owner_label".to_owned(),
            json!(stored_desc.and_then(|s| s.owner_label.as_ref())),
        ),
        (
            "description_revision".to_owned(),
            json!(stored_desc.map_or(0, |s| s.revision)),
        ),
        (
            "description_updated_at".to_owned(),
            json!(stored_desc.and_then(|s| s.updated_at.as_ref())),
        ),
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
        ("source_delivery".to_owned(), source_delivery_json(client)),
    ]);
    debug_assert_eq!(value.len(), CLIENT_ENTRY_FIELDS.len());
    Value::Object(value)
}

fn source_delivery_json(client: &ClientAssessment) -> Value {
    if client.source_delivery.is_empty() {
        return Value::Null;
    }
    Value::Object(
        client
            .source_delivery
            .iter()
            .map(|(source, row)| {
                (
                    source.clone(),
                    json!({
                        "state": source_delivery_state(row.state),
                        "elapsed_ms": row.elapsed_ms,
                        "ingest_rejection": row
                            .ingest_rejection
                            .as_ref()
                            .map_or(Value::Null, |rejection| json!(rejection)),
                    }),
                )
            })
            .collect(),
    )
}

fn source_delivery_state(state: SourceDelivery) -> &'static str {
    match state {
        SourceDelivery::Current => "current",
        SourceDelivery::NeedsAttention => "needs_attention",
        SourceDelivery::Unknown => "unknown",
    }
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

    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};

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
    async fn label_editor_capability_comes_from_authenticated_access_basis() {
        let journal = EstablishedJournal::new();
        let app = crate::router(journal.0.path().to_path_buf());
        let cid = LinkedDeviceCid::try_from(
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap();
        for basis in [
            None,
            Some(AccessBasis::Localhost),
            Some(AccessBasis::PairingPeer {
                carrier: Carrier::Direct,
            }),
            Some(AccessBasis::LinkedDevice {
                cid,
                carrier: Carrier::Direct,
            }),
        ] {
            let editable = matches!(basis, Some(AccessBasis::Localhost));
            let mut req = Request::get("/app/network/api/clients")
                .body(Body::empty())
                .unwrap();
            if let Some(basis) = basis {
                req.extensions_mut().insert(basis);
            }
            let (status, body) = request(app.clone(), req).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["can_edit_labels"], editable);
        }
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
            assert!(row["source_delivery"].is_null());
        }
    }

    #[tokio::test]
    async fn api_clients_single_source_keeps_existing_fields_and_adds_source_delivery() {
        let cid = "sha256:0123456789abcdef0123456789abcdef";
        let journal = EstablishedJournal::new();
        journal.write_ledger(json!([client(cid, "phone")]));
        journal.write_activity(json!({
            cid: {
                "last_seen_at": "2026-08-13T00:01:00Z",
                "last_accepted_ingest_at": "2026-08-13T00:02:00Z",
                "sources": {
                    "audio": {"last_accepted_ingest_at": "2026-08-13T00:02:00Z"}
                }
            }
        }));
        let (status, body) = request(
            crate::router(journal.0.path().to_path_buf()),
            Request::get("/app/network/api/clients")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let row = &body["clients"][0];
        assert_eq!(row["last_accepted_ingest_at"], "2026-08-13T00:02:00Z");
        assert!(row["source_delivery"]["audio"].is_object());
        assert_eq!(row["source_delivery"].as_object().unwrap().len(), 1);
        assert_eq!(
            row.as_object()
                .expect("client row")
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            CLIENT_ENTRY_FIELDS.into_iter().collect::<BTreeSet<_>>()
        );
    }

    #[tokio::test]
    async fn api_clients_projects_source_delivery_for_multi_source_activity() {
        let cid = "sha256:0123456789abcdef0123456789abcdef";
        let journal = EstablishedJournal::new();
        journal.write_ledger(json!([client(cid, "phone")]));
        journal.write_activity(json!({
            cid: {
                "last_seen_at": "2026-08-13T00:01:00Z",
                "last_accepted_ingest_at": "2026-08-13T00:02:00Z",
                "sources": {
                    "audio": {"last_accepted_ingest_at": "2026-08-13T00:02:00Z"},
                    "location": {"last_accepted_ingest_at": "2026-08-13T00:00:00Z"}
                }
            }
        }));
        let (status, body) = request(
            crate::router(journal.0.path().to_path_buf()),
            Request::get("/app/network/api/clients")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let row = &body["clients"][0];
        assert!(row["source_delivery"].is_object());
        assert!(row["source_delivery"]["audio"]["state"].is_string());
        assert!(row["source_delivery"]["location"]["state"].is_string());
        assert!(row["source_delivery"]["audio"]["ingest_rejection"].is_null());
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
        assert_eq!(body, json!({"clients": [], "can_edit_labels": false}));

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

        fs::write(
            journal.0.path().join("link/authorized_clients.json"),
            json!([
                client("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "one"),
                client("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "two"),
            ])
            .to_string(),
        )
        .expect("duplicate ledger");
        let (status, body) = request(
            crate::router(journal.0.path().to_path_buf()),
            Request::get("/app/network/api/clients")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason_code"], "authorization_ledger_duplicate_cid");
        assert_ne!(body.get("clients"), Some(&json!([])));
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
        assert_eq!(row.len(), 14);
        assert!(!row.contains_key("observer_handle"));
        assert_eq!(
            row.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            NETWORK_DEVICE_FIELDS.into_iter().collect::<BTreeSet<_>>()
        );
    }

    #[tokio::test]
    async fn clients_projection_ignores_pairing_identity_reader_states() {
        let cid = "sha256:0123456789abcdef0123456789abcdef";
        let expected_keys = CLIENT_ENTRY_FIELDS.into_iter().collect::<BTreeSet<_>>();
        for (client_label, expected) in [
            (None, json!("")),
            (Some(json!("")), json!("")),
            (Some(json!("Phone")), json!("Phone")),
            (Some(json!(1)), json!("")),
        ] {
            let journal = EstablishedJournal::new();
            let mut entry = client(cid, "phone");
            match client_label {
                None => {
                    entry
                        .as_object_mut()
                        .expect("object")
                        .remove("client_label");
                }
                Some(value) => {
                    entry
                        .as_object_mut()
                        .expect("object")
                        .insert("client_label".to_owned(), value);
                }
            }
            entry
                .as_object_mut()
                .expect("object")
                .insert("platform".to_owned(), json!("linux"));
            journal.write_ledger(json!([entry]));
            let (status, body) = request(
                crate::router(journal.0.path().to_path_buf()),
                Request::get("/app/network/api/clients")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let row = body["clients"][0].as_object().expect("client row");
            assert_eq!(
                row.keys().map(String::as_str).collect::<BTreeSet<_>>(),
                expected_keys
            );
            assert_eq!(row["client_label"], expected);
            assert!(!row.contains_key("platform"));
        }
    }

    #[tokio::test]
    async fn api_clients_self_admits_linked_device_and_refuses_localhost_and_pairing_peer() {
        let cid_str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let cid = LinkedDeviceCid::try_from(cid_str).expect("parse cid");
        let journal = EstablishedJournal::new();
        journal.write_ledger(json!([client(cid_str, "phone")]));
        let app = crate::router(journal.0.path().to_path_buf());

        // 1. GET self with LinkedDevice (Direct) -> 200 OK
        let mut req_direct = Request::get("/app/network/api/clients/self")
            .body(Body::empty())
            .expect("request");
        req_direct
            .extensions_mut()
            .insert(AccessBasis::LinkedDevice {
                cid: cid.clone(),
                carrier: Carrier::Direct,
            });
        let (status, body) = request(app.clone(), req_direct).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["revision"], 0);
        assert!(body["reported"].is_null());
        assert!(body["owner_label"].is_null());
        assert!(body["journal"]["version"].is_string());

        // Verify link/client-descriptions.json was NOT created by GET
        assert!(
            !journal
                .0
                .path()
                .join("link/client-descriptions.json")
                .exists()
        );

        // 2. GET self with LinkedDevice (ViaSpl) on /app/link -> 200 OK
        let mut req_viaspl = Request::get("/app/link/api/clients/self")
            .body(Body::empty())
            .expect("request");
        req_viaspl
            .extensions_mut()
            .insert(AccessBasis::LinkedDevice {
                cid: cid.clone(),
                carrier: Carrier::ViaSpl,
            });
        let (status, body) = request(app.clone(), req_viaspl).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["revision"], 0);

        // 3. Refuses Localhost -> 403 Forbidden
        let mut req_local = Request::get("/app/network/api/clients/self")
            .body(Body::empty())
            .expect("request");
        req_local.extensions_mut().insert(AccessBasis::Localhost);
        let (status, body) = request(app.clone(), req_local).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["reason_code"], "client_description_forbidden");

        // 4. Refuses PairingPeer -> 403 Forbidden
        let mut req_peer = Request::get("/app/network/api/clients/self")
            .body(Body::empty())
            .expect("request");
        req_peer.extensions_mut().insert(AccessBasis::PairingPeer {
            carrier: Carrier::Direct,
        });
        let (status, body) = request(app.clone(), req_peer).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["reason_code"], "client_description_forbidden");

        // 5. Refuses Missing AccessBasis -> 403 Forbidden
        let req_none = Request::get("/app/network/api/clients/self")
            .body(Body::empty())
            .expect("request");
        let (status, body) = request(app.clone(), req_none).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["reason_code"], "client_description_forbidden");
    }

    #[tokio::test]
    async fn api_clients_self_put_validation_and_cas_and_persistence() {
        let cid_str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let cid = LinkedDeviceCid::try_from(cid_str).expect("parse cid");
        let journal = EstablishedJournal::new();
        journal.write_ledger(json!([client(cid_str, "phone")]));
        let app = crate::router(journal.0.path().to_path_buf());

        // 1. PUT unknown keys (e.g. cid, owner_label) -> 400 client_description_invalid
        let mut req_invalid = Request::put("/app/network/api/clients/self")
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "protocol_version": 1,
                    "expected_revision": 0,
                    "cid": cid_str,
                    "reported": {"name": "Test", "platform": null, "device_type": null, "app_id": null, "app_version": null}
                })
                .to_string(),
            ))
            .expect("request");
        req_invalid
            .extensions_mut()
            .insert(AccessBasis::LinkedDevice {
                cid: cid.clone(),
                carrier: Carrier::Direct,
            });
        let (status, body) = request(app.clone(), req_invalid).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["reason_code"], "client_description_invalid");

        // 2. Stale expected_revision -> 409 revision_conflict, no store change
        let mut req_conflict = Request::put("/app/network/api/clients/self")
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "protocol_version": 1,
                    "expected_revision": 5,
                    "reported": {"name": "Test", "platform": null, "device_type": null, "app_id": null, "app_version": null}
                })
                .to_string(),
            ))
            .expect("request");
        req_conflict
            .extensions_mut()
            .insert(AccessBasis::LinkedDevice {
                cid: cid.clone(),
                carrier: Carrier::Direct,
            });
        let (status, body) = request(app.clone(), req_conflict).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["reason_code"], "revision_conflict");
        assert!(
            !journal
                .0
                .path()
                .join("link/client-descriptions.json")
                .exists()
        );

        // 3. Valid PUT -> 200 OK with revision: 1
        let mut req_valid = Request::put("/app/network/api/clients/self")
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "protocol_version": 1,
                    "expected_revision": 0,
                    "reported": {
                        "name": "Jer's Laptop",
                        "platform": "linux",
                        "device_type": null,
                        "app_id": "solstone",
                        "app_version": "2026.07.26"
                    }
                })
                .to_string(),
            ))
            .expect("request");
        req_valid
            .extensions_mut()
            .insert(AccessBasis::LinkedDevice {
                cid: cid.clone(),
                carrier: Carrier::Direct,
            });
        let (status, body) = request(app.clone(), req_valid).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["revision"], 1);
        assert_eq!(body["reported"]["name"], "Jer's Laptop");
        assert_eq!(body["reported"]["platform"], "linux");

        // Verify file exists and GET returns revision 1
        assert!(
            journal
                .0
                .path()
                .join("link/client-descriptions.json")
                .exists()
        );
        let mut req_get = Request::get("/app/network/api/clients/self")
            .body(Body::empty())
            .expect("request");
        req_get.extensions_mut().insert(AccessBasis::LinkedDevice {
            cid: cid.clone(),
            carrier: Carrier::Direct,
        });
        let (status, body) = request(app.clone(), req_get).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["revision"], 1);
        assert_eq!(body["reported"]["name"], "Jer's Laptop");
    }

    #[tokio::test]
    async fn api_clients_patch_label_and_display_label_precedence() {
        let cid_str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let cid = LinkedDeviceCid::try_from(cid_str).expect("parse cid");
        let journal = EstablishedJournal::new();
        journal.write_ledger(json!([client(cid_str, "phone")]));
        let app = crate::router(journal.0.path().to_path_buf());

        // 1. PATCH from linked device -> 403 Forbidden (localhost only)
        let mut req_patch_remote =
            Request::patch(format!("/app/network/api/clients/{cid_str}/label"))
                .header("Content-Type", "application/json")
                .body(Body::from(json!({"label": "New Name"}).to_string()))
                .expect("request");
        req_patch_remote
            .extensions_mut()
            .insert(AccessBasis::LinkedDevice {
                cid: cid.clone(),
                carrier: Carrier::Direct,
            });
        let (status, body) = request(app.clone(), req_patch_remote).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["reason_code"], "client_description_forbidden");

        // 2. PATCH unknown CID with Localhost -> 404 Not Found
        let mut req_patch_unknown = Request::patch(
            "/app/network/api/clients/sha256:unknown0000000000000000000000000/label",
        )
        .header("Content-Type", "application/json")
        .body(Body::from(json!({"label": "New Name"}).to_string()))
        .expect("request");
        req_patch_unknown
            .extensions_mut()
            .insert(AccessBasis::Localhost);
        let (status, body) = request(app.clone(), req_patch_unknown).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["reason_code"], "not_found");

        // 3. PATCH with Localhost sets owner_label
        let mut req_patch_ok = Request::patch(format!("/app/network/api/clients/{cid_str}/label"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({"label": "Owner Custom Label"}).to_string(),
            ))
            .expect("request");
        req_patch_ok.extensions_mut().insert(AccessBasis::Localhost);
        let (status, body) = request(app.clone(), req_patch_ok).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["display_label"], "Owner Custom Label");

        // 4. Linked device PUTs reported name "Reported Name"
        let mut req_put = Request::put("/app/network/api/clients/self")
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "protocol_version": 1,
                    "expected_revision": 1,
                    "reported": {"name": "Reported Name", "platform": null, "device_type": null, "app_id": null, "app_version": null}
                })
                .to_string(),
            ))
            .expect("request");
        req_put.extensions_mut().insert(AccessBasis::LinkedDevice {
            cid: cid.clone(),
            carrier: Carrier::Direct,
        });
        let (status, body) = request(app.clone(), req_put).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["owner_label"], "Owner Custom Label");
        assert_eq!(body["reported"]["name"], "Reported Name");

        // 5. List endpoint: display_label remains owner_label ("Owner Custom Label")
        let (status, body) = request(
            app.clone(),
            Request::get("/app/network/api/clients")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["clients"][0]["display_label"], "Owner Custom Label");

        // 6. PATCH with label: null clears owner_label -> reveals reported name
        let mut req_clear = Request::patch(format!("/app/network/api/clients/{cid_str}/label"))
            .header("Content-Type", "application/json")
            .body(Body::from(json!({"label": null}).to_string()))
            .expect("request");
        req_clear.extensions_mut().insert(AccessBasis::Localhost);
        let (status, body) = request(app.clone(), req_clear).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["display_label"], "Reported Name");

        // List endpoint now shows "Reported Name"
        let (status, body) = request(
            app.clone(),
            Request::get("/app/network/api/clients")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["clients"][0]["display_label"], "Reported Name");
    }
    fn description_validator() -> jsonschema::Validator {
        let authority: Value = serde_json::from_str(include_str!("../../solstone-core-repository-contracts/src/contracts/client_description_contract_authority.json")).expect("authority");
        jsonschema::validator_for(&json!({
            "$ref": "#/components/schemas/ClientDescriptionResponse",
            "components": authority["components"].clone()
        }))
        .expect("schema")
    }

    #[tokio::test]
    async fn metadata_wire_contract_matches_mounted_routes_and_requires_complete_snapshots() {
        let cid_str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let cid = LinkedDeviceCid::try_from(cid_str).expect("cid");
        let journal = EstablishedJournal::new();
        journal.write_ledger(json!([client(cid_str, "phone")]));
        let app = crate::router(journal.0.path().to_path_buf());
        let schema = description_validator();
        let route = "/app/network/api/clients/self";
        let send = |method: &str, value: Value| {
            let mut req = Request::builder()
                .method(method)
                .uri(route)
                .header("Content-Type", "application/json")
                .body(Body::from(value.to_string()))
                .expect("request");
            req.extensions_mut().insert(AccessBasis::LinkedDevice {
                cid: cid.clone(),
                carrier: Carrier::Direct,
            });
            req
        };
        let (status, initial) = request(app.clone(), send("GET", Value::Null)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(schema.is_valid(&initial), "{initial}");
        let reported = json!({"name":"é".repeat(40),"platform":"future-platform","device_type":null,"app_id":"solstone","app_version":"1"});
        let complete = json!({"protocol_version":1,"expected_revision":0,"reported":reported});
        let mut invalid = vec![
            json!({"protocol_version":1,"expected_revision":0}),
            json!({"protocol_version":1,"expected_revision":0,"reported":null}),
        ];
        for field in ["name", "platform", "device_type", "app_id", "app_version"] {
            let mut partial = complete.clone();
            partial["reported"]
                .as_object_mut()
                .expect("object")
                .remove(field);
            invalid.push(partial);
        }
        let mut too_long = complete.clone();
        too_long["reported"]["name"] = json!("é".repeat(41));
        invalid.push(too_long);
        for payload in invalid {
            let (status, _) = request(app.clone(), send("PUT", payload)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        assert!(
            !journal
                .0
                .path()
                .join("link/client-descriptions.json")
                .exists()
        );
        let (status, published) = request(app.clone(), send("PUT", complete)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(schema.is_valid(&published), "{published}");
        assert_eq!(published["revision"], 1);
        let label_route = format!("/app/network/api/clients/{cid_str}/label");
        for (payload, expected) in [
            (json!({}), StatusCode::BAD_REQUEST),
            (json!({"label":"Desk"}), StatusCode::OK),
            (json!({"label":null}), StatusCode::OK),
        ] {
            let mut req = Request::patch(&label_route)
                .header("Content-Type", "application/json")
                .body(Body::from(payload.to_string()))
                .expect("request");
            req.extensions_mut().insert(AccessBasis::Localhost);
            let (status, response) = request(app.clone(), req).await;
            assert_eq!(status, expected);
            if status == StatusCode::OK {
                assert!(schema.is_valid(&response), "{response}");
            }
        }
        for (route, key) in [
            ("/app/network/api/clients", "clients"),
            ("/app/network/api/devices", "devices"),
        ] {
            let (status, response) = request(
                app.clone(),
                Request::get(route).body(Body::empty()).expect("request"),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(response[key][0]["reported"]["platform"], "future-platform");
            assert_eq!(response[key][0]["description_revision"], 3);
            std::fs::write(
                journal.0.path().join("link/client-descriptions.json"),
                "broken",
            )
            .expect("corrupt store");
            let (status, _) = request(
                app.clone(),
                Request::get(route).body(Body::empty()).expect("request"),
            )
            .await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            // Restore this test's known current description for the second projection.
            let stored = json!({cid_str:{"protocol_version":1,"revision":3,"reported":reported,"owner_label":null,"updated_at":"2026-09-07T00:00:00Z"}});
            std::fs::write(
                journal.0.path().join("link/client-descriptions.json"),
                stored.to_string(),
            )
            .expect("test store");
        }
    }
}
