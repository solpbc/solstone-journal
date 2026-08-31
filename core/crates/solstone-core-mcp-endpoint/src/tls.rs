// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Opaque, hot-swappable TLS certificate selection for the MCP listener.
//!
//! The listener is owned by Lane B. This module owns only the exact TLS 1.3
//! certificate-selection contract that listener consumes. It intentionally
//! starts without a normal certificate; later certificate lifecycle work can
//! install one without rebuilding an already-running listener configuration.

use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwapOption;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

const ACME_TLS_ALPN: &[u8] = b"acme-tls/1";
const HTTP11_ALPN: &[u8] = b"http/1.1";

/// Opaque, owner-held certificate state for one authorized MCP hostname.
///
/// The construction and installation routes remain crate-private. Callers can
/// hand it to [`mcp_endpoint_server_config`] but cannot access key material,
/// a resolver, hostname, or persistent state.
pub struct McpEndpointTlsService {
    resolver: Arc<McpEndpointCertificateResolver>,
}

struct McpEndpointCertificateResolver {
    hostname: String,
    ordinary: ArcSwapOption<ActiveOrdinaryCertificate>,
    challenge: ArcSwapOption<ActiveChallengeCertificate>,
}

struct ActiveOrdinaryCertificate {
    key: Arc<CertifiedKey>,
    expires_at: Option<Instant>,
}

struct ActiveChallengeCertificate {
    key: Arc<CertifiedKey>,
    generation: u64,
}

impl fmt::Debug for McpEndpointCertificateResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpEndpointCertificateResolver")
            .finish_non_exhaustive()
    }
}

impl McpEndpointTlsService {
    /// Construct empty state for a hostname already validated by the account
    /// and bridge binding. It deliberately serves no default certificate.
    pub(crate) fn for_authorized_hostname(hostname: String) -> Self {
        Self {
            resolver: Arc::new(McpEndpointCertificateResolver {
                hostname,
                ordinary: ArcSwapOption::empty(),
                challenge: ArcSwapOption::empty(),
            }),
        }
    }

    #[cfg(test)]
    fn install_ordinary_for_test(&self, key: Arc<CertifiedKey>) {
        self.resolver
            .ordinary
            .store(Some(Arc::new(ActiveOrdinaryCertificate {
                key,
                expires_at: None,
            })));
    }

    #[cfg(test)]
    fn install_challenge_for_test(&self, key: Arc<CertifiedKey>, generation: u64) {
        self.resolver
            .challenge
            .store(Some(Arc::new(ActiveChallengeCertificate {
                key,
                generation,
            })));
    }
}

/// Build the one TLS configuration that Lane B's dedicated listener consumes.
///
/// Its resolver remains shared with the service, so later normal-certificate
/// installs and short-lived ACME challenges affect only new handshakes.
pub fn mcp_endpoint_server_config(service: &McpEndpointTlsService) -> Arc<rustls::ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let resolver: Arc<dyn ResolvesServerCert> = service.resolver.clone();
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("ring provider supports TLS 1.3")
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    config.alpn_protocols = vec![ACME_TLS_ALPN.to_vec(), HTTP11_ALPN.to_vec()];
    Arc::new(config)
}

impl ResolvesServerCert for McpEndpointCertificateResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let server_name = hello.server_name()?;
        if !is_exact_authorized_hostname(server_name, &self.hostname) {
            return None;
        }
        let alpn = hello
            .alpn()
            .map(|protocols| protocols.collect::<Vec<_>>())
            .unwrap_or_default();
        if is_acme_attempt(&alpn) {
            if !is_only_acme_alpn(&alpn) {
                return None;
            }
            return self
                .challenge
                .load_full()
                .as_deref()
                .filter(|challenge| challenge.generation != 0)
                .map(|challenge| Arc::clone(&challenge.key));
        }
        self.ordinary
            .load_full()
            .as_deref()
            .filter(|ordinary| {
                ordinary
                    .expires_at
                    .is_none_or(|expiry| Instant::now() < expiry)
            })
            .map(|ordinary| Arc::clone(&ordinary.key))
    }
}

fn is_exact_authorized_hostname(candidate: &str, expected: &str) -> bool {
    !expected.is_empty()
        && candidate == expected
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.')
}

fn is_acme_attempt(protocols: &[&[u8]]) -> bool {
    protocols.contains(&ACME_TLS_ALPN)
}

fn is_only_acme_alpn(protocols: &[&[u8]]) -> bool {
    protocols == [ACME_TLS_ALPN]
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Arc;

    use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
    use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConnection};

    use super::*;

    const HOSTNAME: &str = "ab12cd34.solstone.me";

    fn fixture_key() -> (Arc<CertifiedKey>, CertificateDer<'static>) {
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("fixture key");
        let certificate = CertificateParams::new(vec![HOSTNAME.to_owned()])
            .expect("fixture params")
            .self_signed(&key_pair)
            .expect("fixture certificate");
        let cert = CertificateDer::from(certificate.der().to_vec());
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
        let signing_key = rustls::crypto::ring::sign::any_supported_type(&private_key)
            .expect("fixture signing key");
        (
            Arc::new(CertifiedKey::new(vec![cert.clone()], signing_key)),
            cert,
        )
    }

    fn trusted_client_config(
        certificate: CertificateDer<'static>,
        alpn: Vec<Vec<u8>>,
    ) -> Arc<ClientConfig> {
        let mut roots = RootCertStore::empty();
        roots.add(certificate).expect("fixture root");
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("ring provider supports TLS 1.3")
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = alpn;
        Arc::new(config)
    }

    fn complete_handshake(
        client: &mut ClientConnection,
        server: &mut ServerConnection,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for _ in 0..16 {
            let mut client_bytes = Vec::new();
            client.write_tls(&mut client_bytes)?;
            if !client_bytes.is_empty() {
                server.read_tls(&mut Cursor::new(client_bytes))?;
                server.process_new_packets()?;
            }
            let mut server_bytes = Vec::new();
            server.write_tls(&mut server_bytes)?;
            if !server_bytes.is_empty() {
                client.read_tls(&mut Cursor::new(server_bytes))?;
                client.process_new_packets()?;
            }
            if !client.is_handshaking() && !server.is_handshaking() {
                return Ok(());
            }
        }
        Err("fixture handshake did not converge".into())
    }

    #[test]
    fn config_is_tls13_and_advertises_the_exact_two_protocols() {
        let service = McpEndpointTlsService::for_authorized_hostname(HOSTNAME.to_owned());
        let config = mcp_endpoint_server_config(&service);
        assert_eq!(
            config.alpn_protocols,
            vec![ACME_TLS_ALPN.to_vec(), HTTP11_ALPN.to_vec()]
        );
    }

    #[test]
    fn resolver_never_has_a_default_certificate_without_exact_sni() {
        let service = McpEndpointTlsService::for_authorized_hostname(HOSTNAME.to_owned());
        service.install_ordinary_for_test(fixture_key().0);
        assert!(is_exact_authorized_hostname(HOSTNAME, HOSTNAME));
        assert!(!is_exact_authorized_hostname(
            "AB12CD34.solstone.me",
            HOSTNAME
        ));
        assert!(!is_exact_authorized_hostname("other.solstone.me", HOSTNAME));
        assert!(!is_exact_authorized_hostname(
            "x.ab12cd34.solstone.me",
            HOSTNAME
        ));
    }

    #[test]
    fn acme_attempt_requires_one_exact_protocol_and_an_active_generation() {
        let service = McpEndpointTlsService::for_authorized_hostname(HOSTNAME.to_owned());
        service.install_challenge_for_test(fixture_key().0, 1);
        assert!(is_acme_attempt(&[ACME_TLS_ALPN]));
        assert!(is_only_acme_alpn(&[ACME_TLS_ALPN]));
        assert!(!is_only_acme_alpn(&[ACME_TLS_ALPN, HTTP11_ALPN]));
        assert!(!is_only_acme_alpn(&[HTTP11_ALPN, ACME_TLS_ALPN]));
        assert!(!is_acme_attempt(&[HTTP11_ALPN]));
    }

    #[test]
    fn ordinary_http11_handshake_succeeds_but_h2_only_fails() {
        let service = McpEndpointTlsService::for_authorized_hostname(HOSTNAME.to_owned());
        let (key, certificate) = fixture_key();
        service.install_ordinary_for_test(key);
        let server_config = mcp_endpoint_server_config(&service);

        let mut ordinary_client = ClientConnection::new(
            trusted_client_config(certificate.clone(), vec![HTTP11_ALPN.to_vec()]),
            ServerName::try_from(HOSTNAME.to_owned()).expect("fixture hostname"),
        )
        .expect("ordinary client");
        let mut ordinary_server =
            ServerConnection::new(Arc::clone(&server_config)).expect("ordinary server");
        complete_handshake(&mut ordinary_client, &mut ordinary_server).expect("http1 handshake");
        assert_eq!(ordinary_client.alpn_protocol(), Some(HTTP11_ALPN));

        let mut h2_client = ClientConnection::new(
            trusted_client_config(certificate, vec![b"h2".to_vec()]),
            ServerName::try_from(HOSTNAME.to_owned()).expect("fixture hostname"),
        )
        .expect("h2 client");
        let mut h2_server = ServerConnection::new(server_config).expect("h2 server");
        assert!(complete_handshake(&mut h2_client, &mut h2_server).is_err());
    }

    #[test]
    fn hot_swap_changes_the_per_handshake_key_without_rebuilding_the_resolver() {
        let service = McpEndpointTlsService::for_authorized_hostname(HOSTNAME.to_owned());
        service.install_ordinary_for_test(fixture_key().0);
        let first = service
            .resolver
            .ordinary
            .load_full()
            .expect("first active ordinary key");
        service.install_ordinary_for_test(fixture_key().0);
        let second = service
            .resolver
            .ordinary
            .load_full()
            .expect("second active ordinary key");
        assert!(!Arc::ptr_eq(&first, &second));
    }
}
