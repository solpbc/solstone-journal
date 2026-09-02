// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Live runner for the committed pairing-identity contract vectors.
//!
//! Each request body is built from the vector's `additional_fields`. Direct and
//! relay admissions must agree.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::routing::post;
use serde_json::{Map, Value};
use solstone_core_convey_http::identity::{AccessBasis, Carrier};
use solstone_core_sol_link::ca::{generate_ca, jid_from_spki};
use solstone_core_sol_link::pairing::addresses::PairingSnapshot;
use solstone_core_sol_link::pairing::nonces::NonceStore;
use solstone_core_sol_link::pairing_identity::{
    ClientLabelState, PairingIdentityFields, Platform, PlatformState,
};
use tower::ServiceExt;

use super::pair;
use crate::JournalRoot;
use crate::door::PairingAdmission;
use crate::relay_admission::RelayNonceIdentity;

const VECTORS_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../docs/openapi/pairing-contract/vectors.json"
));
const WIRE_BEHAVIOR_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../docs/openapi/pairing-contract/fixtures/wire-behavior.json"
));

const REQUIRED_VECTOR_IDS: [&str; 16] = [
    "pairing.identity.omission",
    "pairing.identity.presence.label_only",
    "pairing.identity.presence.platform_only",
    "pairing.identity.presence.both",
    "pairing.identity.lookalike.casing",
    "pairing.identity.lookalike.punctuation",
    "pairing.identity.opaque.extension",
    "pairing.identity.invalid.client_label.type",
    "pairing.identity.invalid.client_label.empty",
    "pairing.identity.invalid.client_label.oversize",
    "pairing.identity.invalid.platform.unknown",
    "pairing.identity.bound.client_label.253",
    "pairing.identity.role.empty",
    "pairing.identity.role.phone",
    "pairing.identity.role.observer",
    "pairing.identity.role.peer",
];

struct TempDir(tempfile::TempDir);

impl TempDir {
    fn new() -> Self {
        Self(tempfile::TempDir::new_in("/var/tmp").expect("temporary root"))
    }

    fn path(&self) -> &Path {
        self.0.path()
    }
}

fn vectors_document() -> Value {
    serde_json::from_str(VECTORS_JSON).expect("committed pairing vectors parse")
}

fn wire_behavior_document() -> Value {
    serde_json::from_str(WIRE_BEHAVIOR_JSON).expect("committed pairing wire-behavior parse")
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
        .try_into()
        .expect("unix timestamp fits i64")
}

fn committed_identity(root: &Path) {
    let ca = generate_ca().expect("CA");
    fs::create_dir_all(root.join("link/ca")).expect("CA directory");
    fs::write(root.join("link/ca/cert.pem"), ca.certificate_pem()).expect("CA certificate");
    fs::write(root.join("link/ca/private.pem"), ca.private_key_pem()).expect("CA key");
    fs::write(
        root.join("link/state.json"),
        serde_json::json!({
            "instance_id": jid_from_spki(ca.spki_der()).expect("JID"),
            "home_label": "Home"
        })
        .to_string(),
    )
    .expect("state");
    fs::create_dir_all(root.join("config")).expect("config directory");
    fs::write(
        root.join("config/journal.json"),
        r#"{"pairing":{"home_address":"10.0.0.2:7657"}}"#,
    )
    .expect("config");
}

fn pair_request(additional_fields: Map<String, Value>) -> spl_core::PairRequest {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("key");
    let params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("params");
    spl_core::PairRequest {
        csr: params
            .serialize_request(&key)
            .expect("csr")
            .pem()
            .expect("csr PEM"),
        device_label: "phone".into(),
        additional_fields,
    }
}

async fn drive_pair(
    root: &Path,
    nonce: &str,
    role: &str,
    additional_fields: Map<String, Value>,
    relay: bool,
) -> (StatusCode, Value) {
    committed_identity(root);
    let store = NonceStore::new(root);
    if relay {
        store
            .add_relay(nonce.to_owned(), "phone".into(), role.to_owned(), now())
            .expect("relay nonce");
    } else {
        store
            .add(
                nonce.to_owned(),
                "phone".into(),
                role.to_owned(),
                false,
                now(),
            )
            .expect("direct nonce");
    }
    let request = pair_request(additional_fields);
    let body = serde_json::to_vec(&request).expect("pair request JSON");
    let admission = if relay {
        PairingAdmission::Relay(RelayNonceIdentity::new(nonce.to_owned()))
    } else {
        PairingAdmission::Direct
    };
    let carrier = if relay {
        Carrier::ViaSpl
    } else {
        Carrier::Direct
    };
    let app = Router::new()
        .route("/pair", post(pair))
        .layer(axum::Extension(AccessBasis::PairingPeer { carrier }))
        .layer(axum::Extension(admission))
        .layer(axum::Extension(PairingSnapshot::default()))
        .layer(axum::Extension(Arc::new(JournalRoot(root.to_path_buf()))));
    let response = app
        .oneshot(
            Request::post(format!("/pair?token={nonce}"))
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let value: Value = serde_json::from_slice(&bytes).expect("JSON");
    (status, value)
}

fn ledger_object(root: &Path, fingerprint: &str) -> Map<String, Value> {
    let bytes = fs::read(root.join("link/authorized_clients.json")).expect("ledger");
    let value: Value = serde_json::from_slice(&bytes).expect("ledger JSON");
    value
        .as_array()
        .expect("ledger array")
        .iter()
        .find_map(|item| {
            let object = item.as_object()?;
            (object.get("fingerprint")?.as_str()? == fingerprint).then(|| object.clone())
        })
        .expect("ledger row for issued fingerprint")
}

fn assert_expected_identity(fields: &PairingIdentityFields, expected: &Value) {
    match expected["client_label_state"].as_str() {
        Some("absent") => assert_eq!(fields.client_label, ClientLabelState::Absent),
        Some("valid") => {
            let ClientLabelState::Valid(value) = &fields.client_label else {
                panic!("expected valid client_label, got {:?}", fields.client_label);
            };
            if let Some(expected_value) = expected["client_label_value"].as_str() {
                assert_eq!(value, expected_value);
            }
            if let Some(expected_bytes) = expected["client_label_bytes"].as_u64() {
                assert_eq!(value.len() as u64, expected_bytes);
            }
        }
        other => panic!("unexpected client_label_state {other:?}"),
    }
    match expected["platform_state"].as_str() {
        Some("absent") => assert_eq!(fields.platform, PlatformState::Absent),
        Some("valid") => {
            let PlatformState::Valid(platform) = fields.platform else {
                panic!("expected valid platform, got {:?}", fields.platform);
            };
            if let Some(expected_value) = expected["platform_value"].as_str() {
                assert_eq!(
                    platform,
                    Platform::from_wire(expected_value).expect("vocab")
                );
            }
        }
        other => panic!("unexpected platform_state {other:?}"),
    }
}

fn fixture_for<'a>(fixtures: &'a [Value], vector_id: &str) -> &'a Value {
    let fixture_id = format!("declared.{vector_id}");
    fixtures
        .iter()
        .find(|fixture| fixture["id"].as_str() == Some(fixture_id.as_str()))
        .unwrap_or_else(|| panic!("missing wire-behavior fixture {fixture_id}"))
}

#[tokio::test]
async fn committed_pairing_vectors_drive_direct_and_relay_pair_routes() {
    let document = vectors_document();
    let vectors = document["vectors"].as_array().expect("vectors array");
    let wire_behavior = wire_behavior_document();
    let fixtures = wire_behavior["fixtures"]
        .as_array()
        .expect("fixtures array");
    let present: Vec<&str> = vectors
        .iter()
        .map(|vector| vector["id"].as_str().expect("vector id"))
        .collect();
    for required in REQUIRED_VECTOR_IDS {
        assert!(
            present.contains(&required),
            "committed corpus missing {required}"
        );
    }

    for vector in vectors {
        let id = vector["id"].as_str().expect("id");
        let role = vector["role"].as_str().expect("role");
        let additional_fields = vector["additional_fields"]
            .as_object()
            .expect("additional_fields")
            .clone();
        let decision = &vector["decision"];
        let accepted = decision["accepted"].as_bool().expect("accepted");
        let expected_status = u16::try_from(decision["http_status"].as_u64().expect("http_status"))
            .expect("http status fits u16");
        let fixture = fixture_for(fixtures, id);
        assert_eq!(
            fixture["provenance"]["http_status"]
                .as_u64()
                .expect("status"),
            u64::from(expected_status),
            "{id} wire-behavior status"
        );

        let mut outcomes = Vec::new();
        for relay in [false, true] {
            let temporary = TempDir::new();
            let nonce = format!(
                "{}-{}",
                if relay { "relay" } else { "direct" },
                id.replace('.', "-")
            );
            let (status, body) = drive_pair(
                temporary.path(),
                &nonce,
                role,
                additional_fields.clone(),
                relay,
            )
            .await;
            assert_eq!(status.as_u16(), expected_status, "{id} relay={relay}");
            if accepted {
                assert_eq!(status, StatusCode::OK, "{id} relay={relay}");
                let fingerprint = body["fingerprint"].as_str().expect("fingerprint");
                let object = ledger_object(temporary.path(), fingerprint);
                let fields = PairingIdentityFields::from_object(&object);
                assert_expected_identity(&fields, &vector["expected"]);
                assert!(
                    object.get("vendor_token").is_none() && object.get("x-opaque").is_none(),
                    "{id} opaque extensions are not ledger columns"
                );
                outcomes.push((status, fields, None));
            } else {
                assert_eq!(
                    body["reason_code"].as_str(),
                    decision["reason_code"].as_str(),
                    "{id} relay={relay}"
                );
                assert_eq!(
                    body["detail"].as_str(),
                    decision["detail"].as_str(),
                    "{id} relay={relay}"
                );
                assert!(
                    !temporary
                        .path()
                        .join("link/authorized_clients.json")
                        .is_file()
                        || fs::read(temporary.path().join("link/authorized_clients.json"))
                            .ok()
                            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                            .and_then(|value| value.as_array().map(Vec::is_empty))
                            .unwrap_or(true),
                    "{id} refused ceremony writes no ledger row"
                );
                outcomes.push((
                    status,
                    PairingIdentityFields::from_object(&Map::new()),
                    Some((body["reason_code"].clone(), body["detail"].clone())),
                ));
            }
        }
        assert_eq!(
            outcomes[0].0, outcomes[1].0,
            "{id} Direct and Relay HTTP status must agree"
        );
        assert_eq!(
            outcomes[0].2, outcomes[1].2,
            "{id} Direct and Relay refusal bodies must agree"
        );
        if accepted {
            assert_eq!(
                outcomes[0].1, outcomes[1].1,
                "{id} Direct and Relay ledger identity must agree"
            );
        }
    }
}
