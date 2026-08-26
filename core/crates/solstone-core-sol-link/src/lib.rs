// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::future::Future;
use std::sync::Arc;

use solstone_core_sol_client::seam::{
    LinkJoinCredential, LinkJoinDirectRequest, LinkJoinPairingError, LinkJoinPairingErrorKind,
    LinkJoinPairingSeam, LinkJoinRelayControlEndpoint, LinkJoinRelayErrorKind,
    LinkJoinRelayRequest,
};
use spl_transport::credential::Credential;
use spl_transport::{RelayControlEndpoint, RelayError, TransportError, tls};

#[cfg(feature = "host")]
pub mod acceptor;
#[cfg(feature = "host")]
pub mod ca;
#[cfg(feature = "host")]
pub mod client_status;
#[cfg(feature = "host")]
pub mod committed;
#[cfg(feature = "client")]
mod direct_seam;
#[cfg(feature = "host")]
pub mod door;
#[cfg(feature = "host")]
pub mod establish;
#[cfg(feature = "host")]
pub mod http;
#[cfg(feature = "host")]
pub mod ledger;
#[cfg(feature = "host")]
pub mod mark;
#[cfg(feature = "host")]
pub mod pairing;
#[cfg(feature = "client")]
mod pairing_entry;
#[cfg(feature = "host")]
mod publish_checkpoint;
#[cfg(feature = "client")]
mod serve;
#[cfg(feature = "host")]
pub mod service_identity;

#[cfg(feature = "host")]
pub use acceptor::{
    DEVICE_DOOR_AUTHORIZATION_REFRESH_INTERVAL, build_device_door_acceptor,
    serve_device_door_connection,
};
#[cfg(feature = "host")]
pub use door::{
    DeviceDoorAuthorization, DeviceDoorConfigError, DeviceDoorVerifier,
    authorization_publication_ticks, build_device_door_server_config, refresh_once,
    spawn_authorization_refresh,
};
#[cfg(feature = "client")]
pub use serve::SplLinkServeRunner;

/// Test-only serve internals for the `sol_link_serving` integration target.
#[cfg(all(feature = "client", any(test, feature = "host")))]
#[doc(hidden)]
pub mod serve_test_support {
    pub use crate::serve::{
        STATUS_PATH, StatusClock, StatusTracker, bridge_names, bridge_policy_for_port,
    };
}

/// Test-only publication checkpoints for the `sol_link_publish_crash` target.
#[cfg(all(feature = "host", feature = "test-hooks"))]
#[doc(hidden)]
pub mod publish_test_hooks {
    pub use crate::publish_checkpoint::PublishCheckpoint;
}

/// Test-only certificate fixtures shared by this package's unit and integration tests.
#[cfg(any(test, feature = "host"))]
#[doc(hidden)]
pub mod test_support {
    use rcgen::{
        BasicConstraints, Certificate, CertificateParams, IsCa, KeyPair, KeyUsagePurpose,
        PKCS_ECDSA_P256_SHA256,
    };

    pub const FIXED_CERTIFICATE_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIBqTCCAU+gAwIBAgIUKZ4GlQ+jaITZjYye0LTx71Oqx/kwCgYIKoZIzj0EAwIw\nKjEoMCYGA1UEAwwfc29sc3RvbmUgZml4ZWQgZG9vciBsb29rdXAgdGVzdDAeFw0y\nNjA4MDQyMjMyNDFaFw0zNjA4MDEyMjMyNDFaMCoxKDAmBgNVBAMMH3NvbHN0b25l\nIGZpeGVkIGRvb3IgbG9va3VwIHRlc3QwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNC\nAAQLWc/O7vh+eaolXyLl4UttktPMSL8L53AtLdpZnRxmQC0eA73pSSSHXyUricim\ncdS9bsJS5CKw4vsk+W8Oh8rGo1MwUTAdBgNVHQ4EFgQUrMksIzdtNTRky8Sk8RLe\nM0kYEQMwHwYDVR0jBBgwFoAUrMksIzdtNTRky8Sk8RLeM0kYEQMwDwYDVR0TAQH/\nBAUwAwEB/zAKBggqhkjOPQQDAgNIADBFAiAVugzqjG4CX0sUgtnU3Xuo4gh9XK1P\nKJnZhZwLOZPNdgIhAMNXOb63RcTM0DDHjfwiz6hLCvQ10aPUkW8izj8nv36W\n-----END CERTIFICATE-----\n";
    pub const FIXED_CERTIFICATE_SHA256: &str =
        "fbce31e7e99dbb0361851f0a27fe1909df27dc85ec268a9326c719dc8351d83e";

    pub struct TestCa {
        certificate: Certificate,
        key: KeyPair,
    }

    impl Default for TestCa {
        fn default() -> Self {
            Self::new()
        }
    }

    impl TestCa {
        pub fn new() -> Self {
            let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("test key");
            let mut params = CertificateParams::new(Vec::<String>::new()).expect("test params");
            params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            params.key_usages.push(KeyUsagePurpose::DigitalSignature);
            params.key_usages.push(KeyUsagePurpose::KeyCertSign);
            Self {
                certificate: params.self_signed(&key).expect("test ca"),
                key,
            }
        }

        pub fn certificate(&self) -> &Certificate {
            &self.certificate
        }

        pub fn key(&self) -> &KeyPair {
            &self.key
        }

        pub fn fp_prefix(&self) -> Vec<u8> {
            spl_core::ca::sha256(self.certificate.der())[..16].to_vec()
        }
    }
}

#[cfg(all(test, feature = "host"))]
mod http_tests;

#[cfg(feature = "client")]
#[derive(Debug, Clone, Copy, Default)]
pub struct SplLinkJoinPairingSeam;

#[cfg(feature = "client")]
impl LinkJoinPairingSeam for SplLinkJoinPairingSeam {
    fn pair_direct(
        &self,
        request: LinkJoinDirectRequest,
    ) -> Result<LinkJoinCredential, LinkJoinPairingError> {
        pair_direct_with_spl_seam(request, Arc::new(direct_seam::SplDirectPairingSeam))
    }

    fn pair_relay(
        &self,
        request: LinkJoinRelayRequest,
    ) -> Result<LinkJoinCredential, LinkJoinPairingError> {
        let credential = block_on_transport(pairing_entry::relay(&request))?;
        link_credential_from_spl(credential)
    }
}

#[cfg(feature = "client")]
fn pair_direct_with_spl_seam(
    request: LinkJoinDirectRequest,
    seam: Arc<dyn spl_transport::pairing::DirectPairingSeam>,
) -> Result<LinkJoinCredential, LinkJoinPairingError> {
    let credential = block_on_transport(pairing_entry::direct(&request, seam))?;
    link_credential_from_spl(credential)
}

#[cfg(feature = "client")]
fn block_on_transport<F, T>(future: F) -> Result<T, LinkJoinPairingError>
where
    F: Future<Output = Result<T, TransportError>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| LinkJoinPairingError::new(LinkJoinPairingErrorKind::RuntimeUnavailable))?;
    runtime.block_on(future).map_err(map_transport_error)
}

#[cfg(feature = "client")]
fn link_credential_from_spl(
    credential: Credential,
) -> Result<LinkJoinCredential, LinkJoinPairingError> {
    let ca_fingerprint = ca_fingerprint(&credential.ca_chain_pem)?;
    Ok(LinkJoinCredential {
        client_key_pem: credential.client_key_pem,
        client_cert_pem: credential.client_cert_pem,
        ca_chain_pem: credential.ca_chain_pem,
        ca_fingerprint,
        instance_id: credential.instance_id,
        home_label: credential.home_label,
        home_attestation: credential.home_attestation,
        local_endpoints: credential
            .local_endpoints
            .unwrap_or(serde_json::Value::Null),
        relay_device_token: credential.device_token,
        relay_device_token_expires_at: credential.device_token_expires_at,
    })
}

#[cfg(feature = "client")]
fn ca_fingerprint(ca_chain_pem: &[String]) -> Result<String, LinkJoinPairingError> {
    let chain_pem = ca_chain_pem
        .iter()
        .map(|cert| {
            if cert.ends_with('\n') {
                cert.clone()
            } else {
                format!("{cert}\n")
            }
        })
        .collect::<String>();
    let certs = tls::parse_certs(&chain_pem).map_err(map_transport_error)?;
    let Some(first) = certs.first() else {
        return Err(LinkJoinPairingError::new(LinkJoinPairingErrorKind::Pairing));
    };
    Ok(format!(
        "sha256:{}",
        spl_core::ca::sha256_hex(first.as_ref())
    ))
}

#[cfg(feature = "client")]
fn map_transport_error(error: TransportError) -> LinkJoinPairingError {
    LinkJoinPairingError::new(match error {
        TransportError::Io(error) => {
            drop(error);
            LinkJoinPairingErrorKind::Io
        }
        TransportError::Tls(message) => {
            drop(message);
            LinkJoinPairingErrorKind::Tls
        }
        TransportError::Crypto(message) => {
            drop(message);
            LinkJoinPairingErrorKind::Crypto
        }
        TransportError::Mux(error) => {
            drop(error);
            LinkJoinPairingErrorKind::Mux
        }
        TransportError::Http(error) => {
            drop(error);
            LinkJoinPairingErrorKind::Http
        }
        TransportError::Json(error) => {
            drop(error);
            LinkJoinPairingErrorKind::Json
        }
        TransportError::PairLink(message) => {
            drop(message);
            LinkJoinPairingErrorKind::PairLink
        }
        TransportError::Pairing(message) => {
            if message == "relay response missing home attestation" {
                LinkJoinPairingErrorKind::PairResponseMissingHomeAttestation
            } else {
                LinkJoinPairingErrorKind::Pairing
            }
        }
        TransportError::Rejected { status, body } => {
            drop(body);
            LinkJoinPairingErrorKind::Rejected { status }
        }
        TransportError::Relay(error) => LinkJoinPairingErrorKind::Relay(map_relay_error(error)),
        TransportError::RelayControlRejected { endpoint, status } => {
            LinkJoinPairingErrorKind::RelayControlRejected {
                endpoint: map_relay_control_endpoint(endpoint),
                status,
            }
        }
        TransportError::NoEndpoint => LinkJoinPairingErrorKind::NoEndpoint,
        TransportError::NotPaired => LinkJoinPairingErrorKind::NotPaired,
        TransportError::LocalOffset => LinkJoinPairingErrorKind::LocalOffset,
    })
}

#[cfg(feature = "client")]
fn map_relay_error(error: RelayError) -> LinkJoinRelayErrorKind {
    match error {
        RelayError::HomeOffline => LinkJoinRelayErrorKind::HomeOffline,
        RelayError::Unauthorized => LinkJoinRelayErrorKind::Unauthorized,
        RelayError::Unpaid => LinkJoinRelayErrorKind::Unpaid,
        RelayError::UnknownInstance => LinkJoinRelayErrorKind::UnknownInstance,
        RelayError::PairWindowClosed => LinkJoinRelayErrorKind::PairWindowClosed,
        RelayError::Overflow => LinkJoinRelayErrorKind::Overflow,
        RelayError::Abnormal => LinkJoinRelayErrorKind::Abnormal,
        RelayError::UpgradeRejected => LinkJoinRelayErrorKind::UpgradeRejected,
        RelayError::Stalled => LinkJoinRelayErrorKind::Stalled,
    }
}

#[cfg(feature = "client")]
fn map_relay_control_endpoint(endpoint: RelayControlEndpoint) -> LinkJoinRelayControlEndpoint {
    match endpoint {
        RelayControlEndpoint::EnrollDevice => LinkJoinRelayControlEndpoint::EnrollDevice,
        RelayControlEndpoint::TokenRefresh => LinkJoinRelayControlEndpoint::TokenRefresh,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rcgen::CertificateSigningRequestParams;
    use serde_json::json;
    use solstone_core_sol_client::seam::{
        LinkJoinPairTarget, LinkJoinPairingErrorKind, LinkJoinRelayControlEndpoint,
        LinkJoinRelayErrorKind, LinkJoinRelayRequest,
    };
    use spl_core::PairRequest;
    use spl_core::http::HttpError;
    use spl_core::http::HttpResponse;
    use spl_core::mux::MuxError;
    use spl_core::pairlink::Endpoint;
    use spl_transport::pairing::{
        DirectPairPrepareFuture, DirectPairSendFuture, DirectPairingSeam,
        PreparedDirectPairConnection,
    };

    use super::*;

    use crate::test_support::TestCa;

    struct FakeDirectPairingSeam {
        calls: Arc<Mutex<Vec<FakeDirectCall>>>,
        ca: Arc<TestCa>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct FakeDirectCall {
        host: String,
        port: u16,
        method: String,
        path: String,
        body: Vec<u8>,
    }

    impl DirectPairingSeam for FakeDirectPairingSeam {
        fn prepare<'a>(
            &'a self,
            _config: Arc<rustls::ClientConfig>,
            endpoint: &'a Endpoint,
        ) -> DirectPairPrepareFuture<'a> {
            let host = endpoint.host.clone();
            let port = endpoint.port;
            let calls = self.calls.clone();
            let ca = self.ca.clone();
            Box::pin(async move {
                Ok(Box::new(FakePreparedDirectPairConnection {
                    host,
                    port,
                    calls,
                    ca,
                }) as Box<dyn PreparedDirectPairConnection>)
            })
        }
    }

    struct FakePreparedDirectPairConnection {
        host: String,
        port: u16,
        calls: Arc<Mutex<Vec<FakeDirectCall>>>,
        ca: Arc<TestCa>,
    }

    impl PreparedDirectPairConnection for FakePreparedDirectPairConnection {
        fn send<'a>(
            self: Box<Self>,
            method: &'a str,
            path: &'a str,
            _headers: &'a [(String, String)],
            body: &'a [u8],
        ) -> DirectPairSendFuture<'a> {
            let Self {
                host,
                port,
                calls,
                ca,
            } = *self;
            let method = method.to_string();
            let path = path.to_string();
            let body = body.to_vec();
            Box::pin(async move {
                calls.lock().expect("calls lock").push(FakeDirectCall {
                    host,
                    port,
                    method,
                    path,
                    body: body.clone(),
                });
                Ok(HttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: serde_json::to_vec(&pair_response(&body, &ca)).expect("response json"),
                })
            })
        }
    }

    fn pair_response(request_body: &[u8], ca: &TestCa) -> spl_core::PairResponse {
        let request: PairRequest = serde_json::from_slice(request_body).expect("pair request");
        let client_cert = CertificateSigningRequestParams::from_pem(&request.csr)
            .expect("csr pem")
            .signed_by(ca.certificate(), ca.key())
            .expect("client cert");
        spl_core::PairResponse {
            client_cert: client_cert.pem(),
            ca_chain: vec![ca.certificate().pem()],
            instance_id: "receiver-instance".to_string(),
            home_label: "Home".to_string(),
            fingerprint: format!("sha256:{}", spl_core::ca::sha256_hex(client_cert.der())),
            home_attestation: Some("header.payload.signature".to_string()),
            local_endpoints: Some(json!([
                {"ip": "192.168.1.10", "port": 7657, "scope": "lan"}
            ])),
        }
    }

    fn json_error() -> serde_json::Error {
        serde_json::from_str::<serde_json::Value>("{").expect_err("json error")
    }

    fn secret_substrings() -> &'static [&'static str] {
        &[
            "raw peer body secret",
            "00112233445566778899aabbccddeeff",
            "BEGIN CERTIFICATE REQUEST",
            "sha256:secretfingerprint",
            "https://go.solstone.app/p#SECRETFRAGMENT",
        ]
    }

    fn assert_no_secret_substrings(text: &str) {
        for secret in secret_substrings() {
            assert!(
                !text.contains(secret),
                "secret substring {secret:?} leaked in {text:?}"
            );
        }
    }

    #[test]
    fn direct_path_uses_fake_spl_direct_seam_without_sockets() {
        let ca = Arc::new(TestCa::new());
        let seam = Arc::new(FakeDirectPairingSeam {
            calls: Arc::new(Mutex::new(Vec::new())),
            ca: ca.clone(),
        });
        let request = LinkJoinDirectRequest {
            targets: vec![LinkJoinPairTarget {
                host: "10.0.0.42".to_string(),
                port: 7657,
            }],
            nonce_hex: "00112233445566778899aabbccddeeff".to_string(),
            ca_fp_prefix: ca.fp_prefix(),
            device_label: "laptop".to_string(),
            additional_fields: serde_json::Map::new(),
        };

        let credential =
            pair_direct_with_spl_seam(request, seam.clone()).expect("direct credential");

        assert_eq!(credential.instance_id, "receiver-instance");
        assert_eq!(
            credential.home_attestation.as_deref(),
            Some("header.payload.signature")
        );
        assert_eq!(
            credential.ca_fingerprint,
            format!(
                "sha256:{}",
                spl_core::ca::sha256_hex(ca.certificate().der())
            )
        );
        assert_eq!(credential.local_endpoints[0]["ip"], "192.168.1.10");
        let calls = seam.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].host, "10.0.0.42");
        assert_eq!(calls[0].port, 7657);
        assert_eq!(calls[0].method, "POST");
        assert_eq!(
            calls[0].path,
            "/app/network/pair?token=00112233445566778899aabbccddeeff"
        );
        let request_body: PairRequest =
            serde_json::from_slice(&calls[0].body).expect("request body");
        assert_eq!(request_body.device_label, "laptop");
        assert!(request_body.csr.contains("BEGIN CERTIFICATE REQUEST"));
    }

    #[test]
    fn transport_error_mapping_covers_every_variant_without_secret_leaks() {
        let cases = vec![
            (
                TransportError::Io(std::io::Error::other("raw peer body secret")),
                LinkJoinPairingErrorKind::Io,
            ),
            (
                TransportError::Tls("raw peer body secret".to_string()),
                LinkJoinPairingErrorKind::Tls,
            ),
            (
                TransportError::Crypto("sha256:secretfingerprint".to_string()),
                LinkJoinPairingErrorKind::Crypto,
            ),
            (
                TransportError::Mux(MuxError::Incomplete),
                LinkJoinPairingErrorKind::Mux,
            ),
            (
                TransportError::Http(HttpError::BadStatusLine("raw peer body secret".to_string())),
                LinkJoinPairingErrorKind::Http,
            ),
            (
                TransportError::Json(json_error()),
                LinkJoinPairingErrorKind::Json,
            ),
            (
                TransportError::PairLink("https://go.solstone.app/p#SECRETFRAGMENT".to_string()),
                LinkJoinPairingErrorKind::PairLink,
            ),
            (
                TransportError::Pairing("BEGIN CERTIFICATE REQUEST".to_string()),
                LinkJoinPairingErrorKind::Pairing,
            ),
            (
                TransportError::Pairing("relay response missing home attestation".to_string()),
                LinkJoinPairingErrorKind::PairResponseMissingHomeAttestation,
            ),
            (
                TransportError::Rejected {
                    status: 409,
                    body: "raw peer body secret".to_string(),
                },
                LinkJoinPairingErrorKind::Rejected { status: 409 },
            ),
            (
                TransportError::Relay(RelayError::HomeOffline),
                LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::HomeOffline),
            ),
            (
                TransportError::Relay(RelayError::Unauthorized),
                LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::Unauthorized),
            ),
            (
                TransportError::Relay(RelayError::Unpaid),
                LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::Unpaid),
            ),
            (
                TransportError::Relay(RelayError::UnknownInstance),
                LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::UnknownInstance),
            ),
            (
                TransportError::Relay(RelayError::PairWindowClosed),
                LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::PairWindowClosed),
            ),
            (
                TransportError::Relay(RelayError::Overflow),
                LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::Overflow),
            ),
            (
                TransportError::Relay(RelayError::Abnormal),
                LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::Abnormal),
            ),
            (
                TransportError::Relay(RelayError::UpgradeRejected),
                LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::UpgradeRejected),
            ),
            (
                TransportError::Relay(RelayError::Stalled),
                LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::Stalled),
            ),
            (
                TransportError::RelayControlRejected {
                    endpoint: RelayControlEndpoint::EnrollDevice,
                    status: 403,
                },
                LinkJoinPairingErrorKind::RelayControlRejected {
                    endpoint: LinkJoinRelayControlEndpoint::EnrollDevice,
                    status: 403,
                },
            ),
            (
                TransportError::RelayControlRejected {
                    endpoint: RelayControlEndpoint::TokenRefresh,
                    status: 403,
                },
                LinkJoinPairingErrorKind::RelayControlRejected {
                    endpoint: LinkJoinRelayControlEndpoint::TokenRefresh,
                    status: 403,
                },
            ),
            (
                TransportError::NoEndpoint,
                LinkJoinPairingErrorKind::NoEndpoint,
            ),
            (
                TransportError::NotPaired,
                LinkJoinPairingErrorKind::NotPaired,
            ),
            (
                TransportError::LocalOffset,
                LinkJoinPairingErrorKind::LocalOffset,
            ),
        ];

        for (error, expected) in cases {
            let mapped = map_transport_error(error);
            assert_eq!(mapped.kind, expected);
            assert_no_secret_substrings(&format!("{mapped:?}"));
        }
    }

    #[test]
    fn relay_secret_length_error_is_sanitized() {
        let request = LinkJoinRelayRequest {
            relay_origin: "https://link.solstone.app".to_string(),
            secret: vec![1, 2, 3],
            ca_fp_spki: vec![0; 16],
            device_label: "laptop".to_string(),
            additional_fields: serde_json::Map::new(),
        };
        let error = block_on_transport(pairing_entry::relay(&request)).expect_err("relay error");
        assert_eq!(error.kind, LinkJoinPairingErrorKind::PairLink);
    }
}
