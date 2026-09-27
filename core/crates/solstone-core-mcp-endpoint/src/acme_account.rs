// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Direct ACME account registration, lookup, and key management for solstone.me.

use std::fmt;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_ASN1_SIGNING, ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _,
};
use rustls_acme::acme::{LETS_ENCRYPT_PRODUCTION_DIRECTORY, LETS_ENCRYPT_STAGING_DIRECTORY};
use serde::{Deserialize, Serialize};
use solstone_core_journal_config::McpEndpointCertificateEnvironment;
use tokio::io::AsyncReadExt;

pub const MAX_ACME_URL_FILE_BYTES: usize = 512;
pub const MAX_ACME_REPLACE_INTENT_BYTES: usize = 512;

/// Persisted account URL and its public key thumbprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcmeAccountUrlFile {
    pub account_url: String,
    pub thumbprint: String,
}

/// Compute the RFC 7638 JWK thumbprint for a P-256 public key.
pub fn p256_jwk_thumbprint(pkcs8_der: &[u8]) -> Result<String, AcmeAccountError> {
    let key_pair = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        pkcs8_der,
        &SystemRandom::new(),
    )
    .or_else(|_| {
        EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_ASN1_SIGNING,
            pkcs8_der,
            &SystemRandom::new(),
        )
    })
    .map_err(|_| AcmeAccountError::InvalidKey)?;

    let public_key = key_pair.public_key().as_ref();
    if public_key.len() != 65 || public_key[0] != 4 {
        return Err(AcmeAccountError::InvalidKey);
    }
    let (x, y) = public_key[1..].split_at(32);
    let x_b64 = URL_SAFE_NO_PAD.encode(x);
    let y_b64 = URL_SAFE_NO_PAD.encode(y);
    let canonical_json = format!(r#"{{"crv":"P-256","kty":"EC","x":"{x_b64}","y":"{y_b64}"}}"#);
    let digest = ring::digest::digest(&ring::digest::SHA256, canonical_json.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(digest.as_ref()))
}

/// Validate that an ACME account URL matches the expected environment shape.
pub fn validate_acme_account_url(
    url: &str,
    environment: McpEndpointCertificateEnvironment,
) -> Result<(), AcmeAccountError> {
    let prefix = match environment {
        McpEndpointCertificateEnvironment::Production => {
            "https://acme-v02.api.letsencrypt.org/acme/acct/"
        }
        McpEndpointCertificateEnvironment::Staging => {
            "https://acme-staging-v02.api.letsencrypt.org/acme/acct/"
        }
    };
    let Some(remainder) = url.strip_prefix(prefix) else {
        return Err(AcmeAccountError::InvalidUrlShape);
    };
    if remainder.is_empty() || !remainder.chars().all(|c| c.is_ascii_digit()) {
        return Err(AcmeAccountError::InvalidUrlShape);
    }
    Ok(())
}

pub fn directory_url_for_environment(
    environment: McpEndpointCertificateEnvironment,
) -> &'static str {
    match environment {
        McpEndpointCertificateEnvironment::Production => LETS_ENCRYPT_PRODUCTION_DIRECTORY,
        McpEndpointCertificateEnvironment::Staging => LETS_ENCRYPT_STAGING_DIRECTORY,
    }
}

/// Raw response captured by the ACME HTTP transport seam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RawResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Transport-level failures from the ACME HTTP client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    Timeout,
    Unreachable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClassificationOutcome {
    Success { account_url: String },
    RetryBadNonce { new_nonce: Option<String> },
    RateLimited { retry_after: Duration },
    Transient,
    Failure(AccountClientFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountClientFailure {
    CertificateAccountUnknown,
    CertificateAccountDeactivated,
    CertificateAccountRefused,
}

impl AccountClientFailure {
    #[must_use]
    pub const fn status_token(self) -> &'static str {
        match self {
            Self::CertificateAccountUnknown => "certificate_account_unknown",
            Self::CertificateAccountDeactivated => "certificate_account_deactivated",
            Self::CertificateAccountRefused => "certificate_account_refused",
        }
    }
}

#[derive(Deserialize)]
struct AcmeProblem {
    #[serde(rename = "type")]
    problem_type: Option<String>,
    #[allow(dead_code)]
    detail: Option<String>,
}

/// Classify an HTTP response from an ACME account endpoint.
pub fn classify_account_response(
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
    persisted_url: Option<&str>,
    environment: McpEndpointCertificateEnvironment,
) -> ClassificationOutcome {
    let find_header = |name: &str| -> Option<&str> {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };

    if status == 200 || status == 201 {
        let Some(location) = find_header("Location") else {
            return ClassificationOutcome::Failure(AccountClientFailure::CertificateAccountRefused);
        };
        let location = location.trim();
        if let Some(expected) = persisted_url {
            if location == expected {
                return ClassificationOutcome::Success {
                    account_url: location.to_owned(),
                };
            }
            return ClassificationOutcome::Failure(AccountClientFailure::CertificateAccountRefused);
        }
        if validate_acme_account_url(location, environment).is_ok() {
            return ClassificationOutcome::Success {
                account_url: location.to_owned(),
            };
        }
        return ClassificationOutcome::Failure(AccountClientFailure::CertificateAccountRefused);
    }

    if status == 429 {
        let delay_secs = find_header("Retry-After")
            .and_then(|h| h.trim().parse::<u64>().ok())
            .unwrap_or(1);
        return ClassificationOutcome::RateLimited {
            retry_after: Duration::from_secs(delay_secs),
        };
    }

    if status >= 500 {
        return ClassificationOutcome::Transient;
    }

    if let Ok(problem) = serde_json::from_slice::<AcmeProblem>(body) {
        if let Some(typ) = problem.problem_type.as_deref() {
            if typ.ends_with(":accountDoesNotExist") {
                return ClassificationOutcome::Failure(
                    AccountClientFailure::CertificateAccountUnknown,
                );
            }
            if typ.ends_with(":unauthorized") {
                return ClassificationOutcome::Failure(
                    AccountClientFailure::CertificateAccountDeactivated,
                );
            }
            if typ.ends_with(":badNonce") {
                let new_nonce = find_header("Replay-Nonce")
                    .map(str::trim)
                    .map(str::to_owned);
                return ClassificationOutcome::RetryBadNonce { new_nonce };
            }
        }
    }

    ClassificationOutcome::Failure(AccountClientFailure::CertificateAccountRefused)
}

#[derive(Serialize)]
struct JwsHeader<'a> {
    alg: &'static str,
    jwk: JwkPublic<'a>,
    nonce: &'a str,
    url: &'a str,
}

#[derive(Serialize)]
struct JwkPublic<'a> {
    crv: &'static str,
    kty: &'static str,
    x: &'a str,
    y: &'a str,
}

#[derive(Serialize)]
struct JwsBody {
    protected: String,
    payload: String,
    signature: String,
}

/// Sign an ACME JWS request payload with an ECDSA P-256 key pair.
pub fn sign_jws_request(
    key_der: &[u8],
    nonce: &str,
    endpoint_url: &str,
    payload_json: &[u8],
) -> Result<Vec<u8>, AcmeAccountError> {
    let key_pair = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        key_der,
        &SystemRandom::new(),
    )
    .or_else(|_| {
        EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_ASN1_SIGNING,
            key_der,
            &SystemRandom::new(),
        )
    })
    .map_err(|_| AcmeAccountError::InvalidKey)?;

    let public_key = key_pair.public_key().as_ref();
    if public_key.len() != 65 || public_key[0] != 4 {
        return Err(AcmeAccountError::InvalidKey);
    }
    let (x, y) = public_key[1..].split_at(32);
    let x_b64 = URL_SAFE_NO_PAD.encode(x);
    let y_b64 = URL_SAFE_NO_PAD.encode(y);

    let header = JwsHeader {
        alg: "ES256",
        jwk: JwkPublic {
            crv: "P-256",
            kty: "EC",
            x: &x_b64,
            y: &y_b64,
        },
        nonce,
        url: endpoint_url,
    };
    let protected_json =
        serde_json::to_vec(&header).map_err(|_| AcmeAccountError::Serialization)?;
    let protected_b64 = URL_SAFE_NO_PAD.encode(&protected_json);
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload_json);

    let signing_input = format!("{protected_b64}.{payload_b64}");
    let signature = key_pair
        .sign(&SystemRandom::new(), signing_input.as_bytes())
        .map_err(|_| AcmeAccountError::Signing)?;
    let signature_b64 = URL_SAFE_NO_PAD.encode(signature.as_ref());

    let body = JwsBody {
        protected: protected_b64,
        payload: payload_b64,
        signature: signature_b64,
    };
    serde_json::to_vec(&body).map_err(|_| AcmeAccountError::Serialization)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcmeAccountError {
    InvalidKey,
    InvalidUrlShape,
    Serialization,
    Signing,
}

impl fmt::Display for AcmeAccountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKey => f.write_str("invalid ACME account key"),
            Self::InvalidUrlShape => f.write_str("invalid ACME account URL shape"),
            Self::Serialization => f.write_str("ACME JSON serialization failed"),
            Self::Signing => f.write_str("ACME JWS signing failed"),
        }
    }
}

impl std::error::Error for AcmeAccountError {}

pub trait AcmeTransportSeam: Send + Sync {
    fn exchange(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<RawResponse, TransportError>;
}

#[cfg(test)]
pub(crate) static TEST_ACME_TRANSPORT: std::sync::RwLock<Option<Arc<dyn AcmeTransportSeam>>> =
    std::sync::RwLock::new(None);

pub async fn exchange(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<RawResponse, TransportError> {
    #[cfg(test)]
    {
        if let Ok(guard) = TEST_ACME_TRANSPORT.read() {
            if let Some(transport) = guard.as_ref() {
                return transport.exchange(method, url, headers, body);
            }
        }
    }
    tokio_exchange(method, url, headers, body).await
}

async fn tokio_exchange(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<RawResponse, TransportError> {
    let timeout_duration = Duration::from_secs(10);
    tokio::time::timeout(timeout_duration, async {
        tokio_exchange_inner(method, url, headers, body).await
    })
    .await
    .map_err(|_| TransportError::Timeout)?
}

async fn tokio_exchange_inner(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<RawResponse, TransportError> {
    let (hostname, port, path_and_query) =
        parse_https_url(url).map_err(|_| TransportError::Unreachable)?;

    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let client_config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|_| TransportError::Unreachable)?
    .with_root_certificates(roots)
    .with_no_client_auth();

    let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));

    let addrs = tokio::net::lookup_host((hostname.as_str(), port))
        .await
        .map_err(|_| TransportError::Unreachable)?
        .collect::<Vec<SocketAddr>>();

    let mut tcp_stream = None;
    for addr in addrs {
        if let Ok(stream) = tokio::net::TcpStream::connect(addr).await {
            tcp_stream = Some(stream);
            break;
        }
    }
    let tcp_stream = tcp_stream.ok_or(TransportError::Unreachable)?;

    let server_name = rustls::pki_types::ServerName::try_from(hostname.clone())
        .map_err(|_| TransportError::Unreachable)?
        .to_owned();

    let tls_stream = connector
        .connect(server_name, tcp_stream)
        .await
        .map_err(|_| TransportError::Unreachable)?;

    let mut req_bytes = Vec::new();
    req_bytes.extend_from_slice(format!("{method} {path_and_query} HTTP/1.1\r\n").as_bytes());
    req_bytes.extend_from_slice(format!("Host: {hostname}\r\n").as_bytes());
    req_bytes.extend_from_slice(b"User-Agent: solstone\r\n");
    req_bytes.extend_from_slice(b"Connection: close\r\n");
    req_bytes.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    for (k, v) in headers {
        req_bytes.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    req_bytes.extend_from_slice(b"\r\n");
    req_bytes.extend_from_slice(body);

    let mut writer = tls_stream;
    write_http_request(&mut writer, &req_bytes).await?;

    let mut resp_bytes = Vec::new();
    let mut buf = [0u8; 4096];
    while resp_bytes.len() < 65536 {
        match writer.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => resp_bytes.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }

    parse_http11_response(&resp_bytes).ok_or(TransportError::Unreachable)
}

async fn write_http_request<W: tokio::io::AsyncWriteExt + Unpin>(
    writer: &mut W,
    req_bytes: &[u8],
) -> Result<(), TransportError> {
    writer
        .write_all(req_bytes)
        .await
        .map_err(|_| TransportError::Unreachable)?;
    writer
        .flush()
        .await
        .map_err(|_| TransportError::Unreachable)?;
    Ok(())
}

fn parse_https_url(url: &str) -> Result<(String, u16, String), ()> {
    let remainder = url.strip_prefix("https://").ok_or(())?;
    let (host_part, path_part) = match remainder.find('/') {
        Some(idx) => (&remainder[..idx], &remainder[idx..]),
        None => (remainder, "/"),
    };
    let (hostname, port) = match host_part.find(':') {
        Some(idx) => {
            let host = &host_part[..idx];
            let port_str = &host_part[idx + 1..];
            let p: u16 = port_str.parse().map_err(|_| ())?;
            (host.to_owned(), p)
        }
        None => (host_part.to_owned(), 443),
    };
    Ok((hostname, port, path_part.to_owned()))
}

fn parse_http11_response(bytes: &[u8]) -> Option<RawResponse> {
    let header_end = bytes.windows(4).position(|w| w == b"\r\n\r\n")?;
    let header_bytes = &bytes[..header_end];
    let body = bytes[header_end + 4..].to_vec();

    let header_str = std::str::from_utf8(header_bytes).ok()?;
    let mut lines = header_str.split("\r\n");
    let status_line = lines.next()?;
    let mut parts = status_line.splitn(3, ' ');
    let _version = parts.next()?;
    let status_code: u16 = parts.next()?.parse().ok()?;

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }

    Some(RawResponse {
        status: status_code,
        headers,
        body,
    })
}

#[derive(Deserialize)]
pub(crate) struct DirectoryEndpoints {
    #[serde(rename = "newNonce")]
    pub(crate) new_nonce: String,
    #[serde(rename = "newAccount")]
    pub(crate) new_account: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureAccountOutcome {
    Ready { account_url: String },
    Failed { status: &'static str },
    Transient,
}

pub async fn ensure_acme_account(
    journal_root: &Path,
    environment: McpEndpointCertificateEnvironment,
) -> EnsureAccountOutcome {
    #[cfg(unix)]
    {
        ensure_acme_account_unix(journal_root, environment).await
    }
    #[cfg(not(unix))]
    {
        let _ = (journal_root, environment);
        EnsureAccountOutcome::Transient
    }
}

#[cfg(unix)]
async fn ensure_acme_account_unix(
    journal_root: &Path,
    environment: McpEndpointCertificateEnvironment,
) -> EnsureAccountOutcome {
    let Ok(root_jr) = solstone_core_journal_io::journal_root::JournalRoot::open(journal_root)
    else {
        return EnsureAccountOutcome::Failed {
            status: "certificate_account_unreadable",
        };
    };
    let Ok(tls_dir) = crate::unix::open_tls_state_directory(&root_jr) else {
        return EnsureAccountOutcome::Failed {
            status: "certificate_account_unreadable",
        };
    };
    let is_prod = matches!(environment, McpEndpointCertificateEnvironment::Production);

    let key_res = crate::unix::read_tls_acme_account_bytes(&tls_dir, is_prod);
    let url_file_bytes = crate::unix::read_tls_acme_account_url_bytes(&tls_dir, is_prod);

    let url_file_obj = match url_file_bytes {
        Ok(Some(bytes)) => match serde_json::from_slice::<AcmeAccountUrlFile>(&bytes) {
            Ok(obj) => Some(obj),
            Err(_) => {
                return EnsureAccountOutcome::Failed {
                    status: "certificate_account_refused",
                };
            }
        },
        Ok(None) => None,
        Err(_) => {
            return EnsureAccountOutcome::Failed {
                status: "certificate_account_unreadable",
            };
        }
    };

    let key_bytes = match key_res {
        Ok(Some(k)) => k,
        Ok(None) => {
            if url_file_obj.is_some() {
                return EnsureAccountOutcome::Failed {
                    status: "certificate_account_missing",
                };
            }
            // No key and no url file -> generate and fresh-register
            let keypair = match rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256) {
                Ok(kp) => kp,
                Err(_) => {
                    return EnsureAccountOutcome::Failed {
                        status: "certificate_account_refused",
                    };
                }
            };
            let generated_key = keypair.serialize_der();
            if crate::unix::persist_tls_acme_account_bytes(&tls_dir, is_prod, &generated_key)
                .is_err()
            {
                return EnsureAccountOutcome::Failed {
                    status: "certificate_account_unreadable",
                };
            }
            generated_key
        }
        Err(_) => {
            return EnsureAccountOutcome::Failed {
                status: "certificate_account_unreadable",
            };
        }
    };

    let calculated_thumbprint = match p256_jwk_thumbprint(&key_bytes) {
        Ok(tp) => tp,
        Err(_) => {
            return EnsureAccountOutcome::Failed {
                status: "certificate_account_refused",
            };
        }
    };

    let is_lookup_only = url_file_obj
        .as_ref()
        .is_some_and(|f| f.thumbprint == calculated_thumbprint);

    let directory_url = directory_url_for_environment(environment);
    let dir_resp = match exchange("GET", directory_url, &[], &[]).await {
        Ok(r) => r,
        Err(_) => return EnsureAccountOutcome::Transient,
    };
    if dir_resp.status >= 500 {
        return EnsureAccountOutcome::Transient;
    }
    let dir_endpoints: DirectoryEndpoints = match serde_json::from_slice(&dir_resp.body) {
        Ok(ep) => ep,
        Err(_) => return EnsureAccountOutcome::Transient,
    };

    let nonce_resp = match exchange("HEAD", &dir_endpoints.new_nonce, &[], &[]).await {
        Ok(r) => r,
        Err(_) => return EnsureAccountOutcome::Transient,
    };
    let mut nonce = match nonce_resp.header("Replay-Nonce") {
        Some(n) => n.trim().to_owned(),
        None => return EnsureAccountOutcome::Transient,
    };

    let payload_json = if is_lookup_only {
        br#"{"onlyReturnExisting":true}"#.to_vec()
    } else {
        br#"{"termsOfServiceAgreed":true}"#.to_vec()
    };

    let persisted_url_str = if is_lookup_only {
        url_file_obj.as_ref().map(|f| f.account_url.as_str())
    } else {
        None
    };

    for attempt in 0..2 {
        let jws = match sign_jws_request(
            &key_bytes,
            &nonce,
            &dir_endpoints.new_account,
            &payload_json,
        ) {
            Ok(j) => j,
            Err(_) => {
                return EnsureAccountOutcome::Failed {
                    status: "certificate_account_refused",
                };
            }
        };

        let headers = [("Content-Type", "application/jose+json")];
        let post_resp = match exchange("POST", &dir_endpoints.new_account, &headers, &jws).await {
            Ok(r) => r,
            Err(_) => return EnsureAccountOutcome::Transient,
        };

        match classify_account_response(
            post_resp.status,
            &post_resp.headers,
            &post_resp.body,
            persisted_url_str,
            environment,
        ) {
            ClassificationOutcome::Success { account_url } => {
                let url_file = AcmeAccountUrlFile {
                    account_url: account_url.clone(),
                    thumbprint: calculated_thumbprint,
                };
                if let Ok(url_file_bytes) = serde_json::to_vec(&url_file) {
                    let _ = crate::unix::persist_tls_acme_account_url_bytes(
                        &tls_dir,
                        is_prod,
                        &url_file_bytes,
                    );
                }
                crate::owner_state::delete_mcp_account_posture(journal_root);
                return EnsureAccountOutcome::Ready { account_url };
            }
            ClassificationOutcome::RetryBadNonce { new_nonce } => {
                if attempt == 0 {
                    if let Some(nn) = new_nonce {
                        nonce = nn;
                    } else if let Ok(nr) =
                        exchange("HEAD", &dir_endpoints.new_nonce, &[], &[]).await
                    {
                        if let Some(nn) = nr.header("Replay-Nonce") {
                            nonce = nn.trim().to_owned();
                        } else {
                            return EnsureAccountOutcome::Transient;
                        }
                    } else {
                        return EnsureAccountOutcome::Transient;
                    }
                    continue;
                }
                return EnsureAccountOutcome::Failed {
                    status: "certificate_account_refused",
                };
            }
            ClassificationOutcome::RateLimited { retry_after } => {
                tokio::time::sleep(retry_after).await;
                return EnsureAccountOutcome::Transient;
            }
            ClassificationOutcome::Transient => return EnsureAccountOutcome::Transient,
            ClassificationOutcome::Failure(failure) => {
                return EnsureAccountOutcome::Failed {
                    status: failure.status_token(),
                };
            }
        }
    }

    EnsureAccountOutcome::Failed {
        status: "certificate_account_refused",
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::*;

    fn generate_test_p256_pkcs8() -> Vec<u8> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("rcgen key");
        key.serialize_der()
    }

    #[test]
    fn thumbprint_is_rfc7638_canonical() {
        let pkcs8 = generate_test_p256_pkcs8();
        let thumbprint = p256_jwk_thumbprint(&pkcs8).expect("thumbprint");
        assert!(!thumbprint.is_empty());
        assert_eq!(thumbprint.len(), 43); // 32 bytes base64url unpadded is 43 chars
    }

    #[test]
    fn url_shape_validation_rules() {
        let prod_valid = "https://acme-v02.api.letsencrypt.org/acme/acct/123456789";
        let staging_valid = "https://acme-staging-v02.api.letsencrypt.org/acme/acct/987654";

        assert_eq!(
            validate_acme_account_url(prod_valid, McpEndpointCertificateEnvironment::Production),
            Ok(())
        );
        assert_eq!(
            validate_acme_account_url(staging_valid, McpEndpointCertificateEnvironment::Staging),
            Ok(())
        );

        // Cross-environment
        assert_eq!(
            validate_acme_account_url(prod_valid, McpEndpointCertificateEnvironment::Staging),
            Err(AcmeAccountError::InvalidUrlShape)
        );
        assert_eq!(
            validate_acme_account_url(staging_valid, McpEndpointCertificateEnvironment::Production),
            Err(AcmeAccountError::InvalidUrlShape)
        );

        // Invalid shapes: trailing slash, non-digits, query, empty ID
        for bad in [
            "https://acme-v02.api.letsencrypt.org/acme/acct/",
            "https://acme-v02.api.letsencrypt.org/acme/acct/123/",
            "https://acme-v02.api.letsencrypt.org/acme/acct/abc",
            "https://acme-v02.api.letsencrypt.org/acme/acct/123?q=1",
            "http://acme-v02.api.letsencrypt.org/acme/acct/123",
            "https://other.ca/acme/acct/123",
        ] {
            assert_eq!(
                validate_acme_account_url(bad, McpEndpointCertificateEnvironment::Production),
                Err(AcmeAccountError::InvalidUrlShape),
                "url should fail: {bad}"
            );
        }
    }

    #[test]
    fn classification_table_coverage() {
        let env = McpEndpointCertificateEnvironment::Production;
        let good_url = "https://acme-v02.api.letsencrypt.org/acme/acct/123456";
        let other_url = "https://acme-v02.api.letsencrypt.org/acme/acct/999999";

        // 200 Location matches persisted URL
        let outcome = classify_account_response(
            200,
            &[("Location".into(), good_url.into())],
            b"{}",
            Some(good_url),
            env,
        );
        assert_eq!(
            outcome,
            ClassificationOutcome::Success {
                account_url: good_url.into()
            }
        );

        // 200 Location differs from persisted URL
        let outcome = classify_account_response(
            200,
            &[("Location".into(), other_url.into())],
            b"{}",
            Some(good_url),
            env,
        );
        assert_eq!(
            outcome,
            ClassificationOutcome::Failure(AccountClientFailure::CertificateAccountRefused)
        );

        // 201 Location valid when no persisted URL
        let outcome = classify_account_response(
            201,
            &[("Location".into(), good_url.into())],
            b"{}",
            None,
            env,
        );
        assert_eq!(
            outcome,
            ClassificationOutcome::Success {
                account_url: good_url.into()
            }
        );

        // 400 accountDoesNotExist
        let body = br#"{"type":"urn:ietf:params:acme:error:accountDoesNotExist","detail":"no such account"}"#;
        let outcome = classify_account_response(400, &[], body, Some(good_url), env);
        assert_eq!(
            outcome,
            ClassificationOutcome::Failure(AccountClientFailure::CertificateAccountUnknown)
        );

        // 403 unauthorized
        let body = br#"{"type":"urn:ietf:params:acme:error:unauthorized","detail":"deactivated"}"#;
        let outcome = classify_account_response(403, &[], body, Some(good_url), env);
        assert_eq!(
            outcome,
            ClassificationOutcome::Failure(AccountClientFailure::CertificateAccountDeactivated)
        );

        // 400 badNonce with Replay-Nonce
        let body = br#"{"type":"urn:ietf:params:acme:error:badNonce","detail":"stale nonce"}"#;
        let outcome = classify_account_response(
            400,
            &[("Replay-Nonce".into(), "new_nonce_value".into())],
            body,
            Some(good_url),
            env,
        );
        assert_eq!(
            outcome,
            ClassificationOutcome::RetryBadNonce {
                new_nonce: Some("new_nonce_value".into())
            }
        );

        // 429 with Retry-After
        let outcome = classify_account_response(
            429,
            &[("Retry-After".into(), "10".into())],
            b"{}",
            Some(good_url),
            env,
        );
        assert_eq!(
            outcome,
            ClassificationOutcome::RateLimited {
                retry_after: Duration::from_secs(10)
            }
        );

        // 429 without Retry-After (defaults to 1s)
        let outcome = classify_account_response(429, &[], b"{}", Some(good_url), env);
        assert_eq!(
            outcome,
            ClassificationOutcome::RateLimited {
                retry_after: Duration::from_secs(1)
            }
        );

        // 500 / 503 / 502 -> Transient
        for code in [500, 502, 503, 504] {
            let outcome = classify_account_response(code, &[], b"{}", Some(good_url), env);
            assert_eq!(outcome, ClassificationOutcome::Transient);
        }

        // Generic 400 / 404 / 415 -> Refused
        for code in [400, 404, 405, 415] {
            let outcome = classify_account_response(code, &[], b"{}", Some(good_url), env);
            assert_eq!(
                outcome,
                ClassificationOutcome::Failure(AccountClientFailure::CertificateAccountRefused)
            );
        }
    }
}
