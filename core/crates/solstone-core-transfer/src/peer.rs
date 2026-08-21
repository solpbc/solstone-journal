// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared paired-link bridge setup and loopback HTTP transport.

use std::collections::BTreeSet;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_journal_config::read_journal_config;
use spl_core::bridge::BridgeNames;
use spl_transport::client::{DialedCarrier, TransportClient};
use spl_transport::credential::{Credential, EndpointAddr};
use spl_transport::journal_bridge::{
    self, BridgePolicy, CapabilityGate, CarrierOpener, JournalBridgeConfig,
};
use spl_transport::relay_pairing::enroll_device;
use spl_transport::{TransportError, tls};

use crate::TransferError;

const DEFAULT_RELAY_URL: &str = "https://link.solstone.app";
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Paired peer selected by [`resolve_peer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPeer {
    /// Directory containing the paired-link bundle.
    pub dir: PathBuf,
    /// Stable remote journal instance identifier.
    pub instance_id: String,
    /// Requested peer label.
    pub label: String,
}

/// Result of an attempted remote peer unpair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnpairOutcome {
    /// The peer accepted the request and its local bundle was removed.
    Unpaired,
    /// The peer rejected the request; the local bundle remains intact.
    Rejected { status: u16, body: Vec<u8> },
}

struct TransferCarrierOpener {
    client: Arc<TransportClient>,
}

impl CarrierOpener for TransferCarrierOpener {
    fn proxy_headers(
        &self,
        upstream_headers: &[(String, String)],
    ) -> Result<Vec<(String, String)>, TransportError> {
        let mut headers = upstream_headers.to_vec();
        headers.push(("host".to_string(), "pl.peer".to_string()));
        Ok(headers)
    }

    fn dial_carrier(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<DialedCarrier, TransportError>> + Send + '_>> {
        Box::pin(async move { self.client.dial_carrier().await })
    }
}

/// Response returned by the paired-link loopback bridge.
pub(crate) struct PeerHttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Synchronous HTTP client connected to the current paired-link bridge.
pub(crate) struct PeerLoopbackClient {
    agent: ureq::Agent,
    base_url: String,
    host: String,
}

impl PeerLoopbackClient {
    pub(crate) fn get(&self, path: &str) -> Result<PeerHttpResponse, TransferError> {
        let response = self
            .agent
            .get(format!("{}{path}", self.base_url))
            .header("host", &self.host)
            .call()
            .map_err(transport_error)?;
        read_response(response)
    }

    pub(crate) fn post(
        &self,
        path: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<PeerHttpResponse, TransferError> {
        let response = self
            .agent
            .post(format!("{}{path}", self.base_url))
            .header("host", &self.host)
            .header("content-type", content_type)
            .send(body)
            .map_err(transport_error)?;
        read_response(response)
    }
}

fn read_response(
    mut response: ureq::http::Response<ureq::Body>,
) -> Result<PeerHttpResponse, TransferError> {
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_to_vec()
        .map_err(|error| TransferError::Transport(error.to_string()))?;
    Ok(PeerHttpResponse { status, body })
}

/// Open one bridge session, run a synchronous operation, and drain it before returning.
pub(crate) fn with_peer_bridge<T>(
    journal: &Path,
    peer: &ResolvedPeer,
    operation: impl FnOnce(&PeerLoopbackClient) -> Result<T, TransferError>,
) -> Result<T, TransferError> {
    let mut credential = load_credential(journal, peer)?;
    let relay_only = credential.relay_origin.is_some() && credential.endpoints.is_empty();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| TransferError::Bridge(error.to_string()))?;

    if relay_only {
        let origin = credential
            .relay_origin
            .as_deref()
            .ok_or_else(|| TransferError::CredentialLoad("relay origin missing".to_string()))?;
        let attestation = credential
            .home_attestation
            .as_deref()
            .ok_or_else(|| TransferError::CredentialLoad("home attestation missing".to_string()))?;
        credential.device_token = Some(
            runtime
                .block_on(enroll_device(origin, &credential.instance_id, attestation))
                .map_err(transport_error)?,
        );
    }
    let endpoint_hosts = credential
        .endpoints
        .iter()
        .map(|endpoint| endpoint.host.clone())
        .collect();
    let client = Arc::new(
        if relay_only {
            TransportClient::new_relay_only(credential, None)
        } else {
            TransportClient::new(credential, None)
        }
        .map_err(transport_error)?,
    );
    let handle = runtime
        .block_on(journal_bridge::start(JournalBridgeConfig {
            opener: Arc::new(TransferCarrierOpener { client }),
            bridge_names: bridge_names(),
            endpoint_hosts,
            policy: bridge_policy(),
        }))
        .map_err(|error| TransferError::Bridge(format!("{error:?}")))?;
    let port = handle.port();
    let loopback_host = format!("127.0.0.1:{port}");
    let loopback = PeerLoopbackClient {
        agent: loopback_agent(),
        base_url: format!("http://{loopback_host}"),
        host: loopback_host,
    };
    let result = operation(&loopback);
    let _status = runtime.block_on(handle.shutdown_and_wait());
    result
}

/// Resolve a paired peer label using the Python resolver's error text.
pub fn resolve_peer(journal: &Path, label: &str) -> Result<ResolvedPeer, TransferError> {
    let peers_dir = journal.join("peers");
    if !peers_dir.is_dir() {
        return Err(TransferError::NoPeersPaired);
    }
    let mut entries = fs::read_dir(&peers_dir)
        .map_err(|error| {
            TransferError::CredentialLoad(format!("{}: {error}", peers_dir.display()))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            TransferError::CredentialLoad(format!("{}: {error}", peers_dir.display()))
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut labels = BTreeSet::new();
    let mut matches = Vec::new();
    for entry in entries {
        let dir = entry.path();
        if !dir.is_dir() || !dir.join("peer.json").is_file() {
            continue;
        }
        let peer: Value =
            serde_json::from_slice(&fs::read(dir.join("peer.json")).map_err(|error| {
                TransferError::CredentialLoad(format!(
                    "invalid peer.json in {}: {error}",
                    dir.display()
                ))
            })?)
            .map_err(|error| {
                TransferError::CredentialLoad(format!(
                    "invalid peer.json in {}: {error}",
                    dir.display()
                ))
            })?;
        let peer_label = peer.get("label").and_then(Value::as_str);
        if let Some(peer_label) = peer_label.filter(|value| !value.is_empty()) {
            labels.insert(peer_label.to_string());
        }
        if peer_label == Some(label) {
            matches.push((dir, peer));
        }
    }
    if matches.is_empty() {
        return Err(TransferError::PeerNotFound {
            label: label.to_string(),
            available: if labels.is_empty() {
                "none".to_string()
            } else {
                labels.into_iter().collect::<Vec<_>>().join(", ")
            },
        });
    }
    if matches.len() > 1 {
        return Err(TransferError::AmbiguousPeer {
            label: label.to_string(),
            instance_ids: matches
                .iter()
                .map(|(dir, peer)| {
                    peer.get("instance_id")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| {
                            dir.file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or_default()
                        })
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join(", "),
        });
    }
    let (dir, peer) = matches.pop().expect("nonempty matches");
    Ok(ResolvedPeer {
        instance_id: peer.get("instance_id").and_then(Value::as_str).map_or_else(
            || {
                dir.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string()
            },
            ToString::to_string,
        ),
        dir,
        label: label.to_string(),
    })
}

/// Request removal of a paired peer and remove its local bundle on success.
pub fn unpair_peer(journal: &Path, peer: &ResolvedPeer) -> Result<UnpairOutcome, TransferError> {
    let fingerprint = certificate_fingerprint(&peer.dir.join("cert.pem"))?;
    with_peer_bridge(journal, peer, |loopback| {
        let response = loopback.post(
            "/app/network/unpair",
            "application/json",
            format!(r#"{{"fingerprint": "{fingerprint}"}}"#).into_bytes(),
        )?;
        if response.status == 200 {
            fs::remove_dir_all(&peer.dir)?;
            Ok(UnpairOutcome::Unpaired)
        } else {
            Ok(UnpairOutcome::Rejected {
                status: response.status,
                body: response.body,
            })
        }
    })
}

fn certificate_fingerprint(path: &Path) -> Result<String, TransferError> {
    let pem = fs::read_to_string(path)
        .map_err(|error| TransferError::CredentialLoad(format!("{}: {error}", path.display())))?;
    let certificates = tls::parse_certs(&pem).map_err(|error| {
        TransferError::CredentialLoad(format!("invalid cert.pem in {}: {error}", path.display()))
    })?;
    let certificate = certificates.first().ok_or_else(|| {
        TransferError::CredentialLoad("cert.pem contains no certificates".to_string())
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(certificate.as_ref())))
}

pub(crate) struct MultipartFile {
    pub field_name: String,
    pub file_name: String,
    pub contents: Vec<u8>,
}

pub(crate) fn multipart_body(metadata: &str, files: &[MultipartFile]) -> (Vec<u8>, String) {
    let boundary = format!(
        "solstone-native-{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let mut body = Vec::new();
    push_multipart_field(&mut body, &boundary, "metadata", metadata.as_bytes());
    for file in files {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n", escape_multipart(&file.field_name), escape_multipart(&file.file_name)).as_bytes());
        body.extend_from_slice(&file.contents);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (body, boundary)
}

fn load_credential(journal: &Path, peer: &ResolvedPeer) -> Result<Credential, TransferError> {
    let private_key = read_bundle_file(&peer.dir, "private.pem")?;
    let client_cert = read_bundle_file(&peer.dir, "cert.pem")?;
    let ca_chain = read_bundle_file(&peer.dir, "chain.pem")?;
    let attestation = read_bundle_file(&peer.dir, "home_attestation.jwt")?;
    let peer_json: Value =
        serde_json::from_slice(&fs::read(peer.dir.join("peer.json")).map_err(|error| {
            TransferError::CredentialLoad(format!(
                "invalid peer.json in {}: {error}",
                peer.dir.display()
            ))
        })?)
        .map_err(|error| {
            TransferError::CredentialLoad(format!(
                "invalid peer.json in {}: {error}",
                peer.dir.display()
            ))
        })?;
    let parsed_chain = tls::parse_certs(&ca_chain).map_err(|error| {
        TransferError::CredentialLoad(format!(
            "invalid chain.pem in {}: {error}",
            peer.dir.display()
        ))
    })?;
    let Some(first_ca) = parsed_chain.first() else {
        return Err(TransferError::CredentialLoad(
            "chain.pem contains no certificates".to_string(),
        ));
    };
    Ok(Credential {
        client_key_pem: private_key,
        client_cert_pem: client_cert,
        ca_chain_pem: vec![ca_chain],
        ca_fp_prefix: spl_core::ca::sha256(first_ca.as_ref())[..16].to_vec(),
        instance_id: peer.instance_id.clone(),
        home_label: peer.label.clone(),
        endpoints: endpoint_addrs(peer_json.get("local_endpoints")),
        home_attestation: Some(attestation),
        local_endpoints: peer_json.get("local_endpoints").cloned(),
        relay_origin: Some(relay_origin(journal)?),
        device_token: None,
        device_token_expires_at: None,
    })
}

fn relay_origin(journal: &Path) -> Result<String, TransferError> {
    if let Ok(value) = std::env::var("SOL_LINK_RELAY_URL") {
        let value = value.trim();
        if !value.is_empty() {
            return Ok(value.trim_end_matches('/').to_string());
        }
    }
    if let Ok(config) = read_journal_config(journal)
        && let Some(url) = config
            .config
            .as_ref()
            .and_then(|config| config.get("link"))
            .and_then(Value::as_object)
            .and_then(|link| link.get("relay_url"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|url| !url.is_empty())
    {
        return Ok(url.trim_end_matches('/').to_string());
    }
    Ok(DEFAULT_RELAY_URL.to_string())
}

fn bridge_names() -> BridgeNames {
    BridgeNames {
        capability_cookie_name: "__solstone_link_cap".to_string(),
        upstream_cookie_prefix: String::new(),
        observer_header_name: "x-solstone-observer".to_string(),
        protocol_version_header_name: "x-solstone-protocol-version".to_string(),
    }
}
fn bridge_policy() -> BridgePolicy {
    BridgePolicy {
        capability_gate: CapabilityGate::Disabled,
        ..BridgePolicy::default()
    }
}
fn endpoint_addrs(value: Option<&Value>) -> Vec<EndpointAddr> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let object = entry.as_object()?;
            let host = object
                .get("ip")
                .or_else(|| object.get("host"))?
                .as_str()?
                .trim();
            let port = u16::try_from(object.get("port")?.as_u64()?).ok()?;
            (!host.is_empty() && port != 0).then(|| EndpointAddr {
                host: host.to_string(),
                port,
            })
        })
        .collect()
}
fn read_bundle_file(peer_dir: &Path, name: &str) -> Result<String, TransferError> {
    fs::read_to_string(peer_dir.join(name)).map_err(|error| {
        TransferError::CredentialLoad(format!("{}: {error}", peer_dir.join(name).display()))
    })
}
fn loopback_agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(UPLOAD_TIMEOUT))
            .build(),
    )
}
fn push_multipart_field(body: &mut Vec<u8>, boundary: &str, name: &str, value: &[u8]) {
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"{}\"\r\n\r\n",
            escape_multipart(name)
        )
        .as_bytes(),
    );
    body.extend_from_slice(value);
    body.extend_from_slice(b"\r\n");
}
fn escape_multipart(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}
fn transport_error(error: impl std::fmt::Display) -> TransferError {
    TransferError::Transport(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::resolve_peer;

    use std::fs;

    use serde_json::json;

    #[test]
    fn peer_resolver_reports_python_compatible_messages() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_peer(root.path(), "office").unwrap_err().to_string(),
            "no peers paired (run \"solstone link join --as peer\" first)"
        );
        let peers = root.path().join("peers");
        for (directory, label, instance_id) in [
            ("z", "zebra", "second"),
            ("a", "alpha", "first"),
            ("b", "alpha", "third"),
        ] {
            let directory = peers.join(directory);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join("peer.json"),
                json!({"label": label, "instance_id": instance_id}).to_string(),
            )
            .unwrap();
        }
        assert_eq!(
            resolve_peer(root.path(), "none").unwrap_err().to_string(),
            "no peer with label \"none\"; available: alpha, zebra"
        );
        assert_eq!(
            resolve_peer(root.path(), "alpha").unwrap_err().to_string(),
            "multiple peers with label \"alpha\": first, third; use <journal_root>/peers/<instance_id> directly"
        );
    }
}
