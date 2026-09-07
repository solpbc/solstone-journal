// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_sol_link::ledger::{AuthorizedClientsRead, read_authorized_clients};
use solstone_core_spl::relay_access::RelayAccessError;

use crate::JournalRoot;
use crate::pair_window_manager::PairWindowManager;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

pub(crate) async fn get_relay_access(
    Extension(root): Extension<Arc<JournalRoot>>,
    basis: Option<Extension<AccessBasis>>,
    pair_windows: Option<Extension<Arc<PairWindowManager>>>,
) -> Response {
    let Some(Extension(basis)) = basis else {
        return crate::network::refusal(
            "relay_access_forbidden",
            "access basis required",
            StatusCode::FORBIDDEN,
        );
    };
    let cid = match basis {
        AccessBasis::LinkedDevice { cid, .. } => cid,
        AccessBasis::Localhost | AccessBasis::PairingPeer { .. } => {
            return crate::network::refusal(
                "relay_access_forbidden",
                "linked device access required",
                StatusCode::FORBIDDEN,
            );
        }
    };

    let Some(Extension(pair_windows)) = pair_windows else {
        return crate::network::refusal(
            "relay_access_unavailable",
            "pair window manager unavailable",
            StatusCode::SERVICE_UNAVAILABLE,
        );
    };

    let cache = pair_windows.relay_access();
    let is_authorized =
        || match read_authorized_clients(&root.0.join("link/authorized_clients.json")) {
            AuthorizedClientsRead::Present(entries) => entries
                .iter()
                .any(|entry| entry.fingerprint == cid.as_str()),
            _ => false,
        };
    if !is_authorized() {
        return crate::network::refusal(
            "relay_access_forbidden",
            "linked device revoked or unlisted",
            StatusCode::FORBIDDEN,
        );
    }
    let result = cache.acquire_current(&root.0, now()).await;

    // Post-acquire authorization re-check: verify client has not been revoked
    let is_authorized = match read_authorized_clients(&root.0.join("link/authorized_clients.json"))
    {
        AuthorizedClientsRead::Present(entries) => entries
            .iter()
            .any(|entry| entry.fingerprint == cid.as_str()),
        _ => false,
    };

    if !is_authorized {
        return crate::network::refusal(
            "relay_access_forbidden",
            "linked device revoked or unlisted",
            StatusCode::FORBIDDEN,
        );
    }

    match result {
        Ok(access) => solstone_core_spl::relay_access::while_service_configuration_current(
            &root.0,
            &access.configuration,
            || Json(access.snapshot).into_response(),
        )
        .unwrap_or_else(|| {
            crate::network::refusal(
                "relay_access_unavailable",
                "relay service configuration changed",
                StatusCode::SERVICE_UNAVAILABLE,
            )
        }),
        Err(RelayAccessError::NotConfigured) => {
            Json(json!({ "protocol_version": 2, "status": "not_configured" })).into_response()
        }
        Err(RelayAccessError::Unavailable(_)) => crate::network::refusal(
            "relay_access_unavailable",
            "relay access is unavailable",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};
    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
    use std::fs;
    use tower::ServiceExt;

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
    async fn test_relay_access_authorization_matrix() {
        let cid_str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let cid = LinkedDeviceCid::try_from(cid_str).expect("parse cid");
        let journal = EstablishedJournal::new();
        journal.write_ledger(json!([client(cid_str, "phone")]));
        let app = crate::router(journal.0.path().to_path_buf());

        // 1. Anonymous / Missing basis -> 403
        let req_none = Request::get("/app/network/api/relay/access")
            .body(Body::empty())
            .expect("request");
        let (status, body) = request(app.clone(), req_none).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["reason_code"], "relay_access_forbidden");

        // 2. Localhost basis -> 403
        let mut req_local = Request::get("/app/network/api/relay/access")
            .body(Body::empty())
            .expect("request");
        req_local.extensions_mut().insert(AccessBasis::Localhost);
        let (status, body) = request(app.clone(), req_local).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["reason_code"], "relay_access_forbidden");

        // 3. PairingPeer basis -> 403
        let mut req_peer = Request::get("/app/network/api/relay/access")
            .body(Body::empty())
            .expect("request");
        req_peer.extensions_mut().insert(AccessBasis::PairingPeer {
            carrier: Carrier::Direct,
        });
        let (status, body) = request(app.clone(), req_peer).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["reason_code"], "relay_access_forbidden");

        // 4. LinkedDevice with unknown/unlisted CID -> 403
        let unknown_cid = LinkedDeviceCid::try_from(
            "sha256:9999999999abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("parse unknown cid");
        let mut req_unknown = Request::get("/app/network/api/relay/access")
            .body(Body::empty())
            .expect("request");
        req_unknown
            .extensions_mut()
            .insert(AccessBasis::LinkedDevice {
                cid: unknown_cid,
                carrier: Carrier::Direct,
            });
        let (status, body) = request(app.clone(), req_unknown).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["reason_code"], "relay_access_forbidden");

        // 5. LinkedDevice with valid CID but not configured -> 200 with {"protocol_version": 2, "status": "not_configured"}
        let mut req_direct = Request::get("/app/network/api/relay/access")
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
        assert_eq!(body["protocol_version"], 2);
        assert_eq!(body["status"], "not_configured");

        // 6. LinkedDevice via SPL transport with valid CID on /app/link -> 200 with {"protocol_version": 2, "status": "not_configured"}
        let mut req_viaspl = Request::get("/app/link/api/relay/access")
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
        assert_eq!(body["protocol_version"], 2);
        assert_eq!(body["status"], "not_configured");

        // 7. Revoke client by clearing authorized_clients -> 403
        journal.write_ledger(json!([]));
        let mut req_revoked = Request::get("/app/network/api/relay/access")
            .body(Body::empty())
            .expect("request");
        req_revoked
            .extensions_mut()
            .insert(AccessBasis::LinkedDevice {
                cid,
                carrier: Carrier::Direct,
            });
        let (status, body) = request(app, req_revoked).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["reason_code"], "relay_access_forbidden");
    }
}
