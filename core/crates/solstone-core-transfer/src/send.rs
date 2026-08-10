// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native paired-link journal segment sending.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Duration as ChronoDuration, NaiveDate};
use serde_json::{Value, json};
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_io::{PathOrDay, iter_segments};
use spl_core::bridge::BridgeNames;
use spl_transport::client::{DialedCarrier, TransportClient};
use spl_transport::credential::{Credential, EndpointAddr};
use spl_transport::journal_bridge::{
    self, BridgePolicy, CapabilityGate, CarrierOpener, JournalBridgeConfig,
};
use spl_transport::relay_pairing::enroll_device;
use spl_transport::{TransportError, tls};

use crate::TransferError;
use crate::export::hash_file;
use crate::manifest::{ManifestFile, SegmentManifest};

/// Segment control files that must never be included in a journal upload.
pub const RESERVED_SEGMENT_FILENAMES: [&str; 3] =
    ["stream.json", "ingest.json", "ingest.json.lock"];

const DEFAULT_RELAY_URL: &str = "https://link.solstone.app";
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const RETRY_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(15),
];

/// Input to [`send`].
#[derive(Debug, Clone)]
pub struct SendRequest {
    /// Paired peer's human-facing label.
    pub to: String,
    /// One day, an inclusive day range, or all existing chronicle days.
    pub day: Option<String>,
    /// Count changed segments without posting any uploads.
    pub dry_run: bool,
}

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

/// Terminal outcome for a completed send operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendTerminal {
    /// All applicable segments were considered.
    Complete,
    /// The paired-link identity was missing or invalid at the remote journal.
    AuthenticationInvalid,
    /// The paired-link identity was revoked or disabled at the remote journal.
    AuthenticationRevoked,
}

/// Counts accumulated by a send operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendReport {
    /// Segments sent, or segments that would be sent in dry-run mode.
    pub sent: usize,
    /// Segments already synchronized or containing only reserved files.
    pub skipped: usize,
    /// Segments rejected without a retryable failure.
    pub failed: usize,
    /// Total uploaded regular-file bytes.
    pub bytes_transferred: u64,
    /// Whether the operation stopped normally or on an authorization response.
    pub terminal: SendTerminal,
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
        // The bridge strips the loopback Host header. The PL HTTP peer expects
        // the virtual host used by the Python `PlHttpSession`.
        headers.push(("host".to_string(), "pl.peer".to_string()));
        Ok(headers)
    }

    fn dial_carrier(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<DialedCarrier, TransportError>> + Send + '_>> {
        Box::pin(async move { self.client.dial_carrier().await })
    }
}

/// Send changed local segments to a paired peer.
pub fn send(journal: &Path, request: SendRequest) -> Result<SendReport, TransferError> {
    let days = parse_day_spec(request.day.as_deref(), journal)?;
    let peer = resolve_peer(journal, &request.to)?;
    let mut credential = load_credential(journal, &peer)?;
    let relay_only = credential.relay_origin.is_some() && credential.endpoints.is_empty();

    // Must be multi-threaded: the synchronous `ureq` calls below block this
    // process's calling thread while `journal_bridge::start` has spawned the
    // accept loop. A current-thread runtime would bind the port but never poll
    // that loop, leaving every loopback request hung. Two workers are ample for
    // this single async-I/O carrier and avoid one worker per host CPU.
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

    // Every path after bridge start, including authorization short-circuits,
    // reaches this drain before the one-shot process can exit.
    let result = send_over_bridge(journal, &peer, &days, request.dry_run, port);
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
        if !dir.is_dir() {
            continue;
        }
        let peer_json = dir.join("peer.json");
        if !peer_json.is_file() {
            continue;
        }
        let peer: Value = serde_json::from_slice(&fs::read(&peer_json).map_err(|error| {
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
        let instance_ids = matches
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
            .join(", ");
        return Err(TransferError::AmbiguousPeer {
            label: label.to_string(),
            instance_ids,
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

/// Parse the Python send command's day selector.
pub fn parse_day_spec(spec: Option<&str>, journal: &Path) -> Result<Vec<String>, TransferError> {
    let Some(spec) = spec else {
        let chronicle = journal.join("chronicle");
        if !chronicle.is_dir() {
            return Ok(Vec::new());
        }
        let mut days = fs::read_dir(chronicle)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                (entry.path().is_dir() && is_eight_digit_day(&name)).then_some(name)
            })
            .collect::<Vec<_>>();
        days.sort();
        return Ok(days);
    };
    if is_eight_digit_day(spec) {
        // Python accepts a lone syntactically-valid day without calendar or
        // existence validation.
        return Ok(vec![spec.to_string()]);
    }
    let Some((start, end)) = spec.split_once('-') else {
        return Err(TransferError::InvalidDay);
    };
    if !is_eight_digit_day(start) || !is_eight_digit_day(end) || end.contains('-') {
        return Err(TransferError::InvalidDay);
    }
    let start = parse_calendar_day(start)?;
    let end = parse_calendar_day(end)?;
    if start > end {
        return Err(TransferError::InvalidDay);
    }
    let mut days = Vec::new();
    let mut current = start;
    while current <= end {
        days.push(current.format("%Y%m%d").to_string());
        current += ChronoDuration::days(1);
    }
    Ok(days)
}

fn send_over_bridge(
    journal: &Path,
    peer: &ResolvedPeer,
    days: &[String],
    dry_run: bool,
    port: u16,
) -> Result<SendReport, TransferError> {
    let loopback_host = format!("127.0.0.1:{port}");
    let loopback = LoopbackClient {
        agent: loopback_agent(),
        base_url: format!("http://{loopback_host}"),
        host: loopback_host,
    };
    let key_prefix: String = peer.instance_id.chars().take(8).collect();
    let manifest_path = format!("/app/import/journal/{key_prefix}/manifest/segments");
    let remote_manifest = query_remote_manifest(&loopback, &manifest_path);
    let mut report = SendReport {
        sent: 0,
        skipped: 0,
        failed: 0,
        bytes_transferred: 0,
        terminal: SendTerminal::Complete,
    };

    for day in days {
        for segment in iter_segments(journal, PathOrDay::Day(day))? {
            let files = segment_files(&segment.path)?;
            if files.is_empty() {
                report.skipped += 1;
                continue;
            }
            let local_manifest = SegmentManifest {
                files: files
                    .iter()
                    .map(|file| {
                        Ok(ManifestFile {
                            name: file
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or_default()
                                .to_string(),
                            sha256: hash_file(file)?.0,
                            size: fs::metadata(file)?.len(),
                        })
                    })
                    .collect::<Result<Vec<_>, TransferError>>()?,
            };
            let route = format!("{}/{}", segment.stream, segment.key);
            let remote = remote_manifest
                .get(day)
                .and_then(|segments| segments.get(&route));
            if manifests_match(&local_manifest, remote) {
                report.skipped += 1;
                continue;
            }
            if dry_run {
                report.sent += 1;
                continue;
            }
            let path = format!("/app/import/journal/{key_prefix}/ingest/segments/{day}");
            match upload_segment(&loopback, &path, day, &segment.stream, &segment.key, &files)? {
                UploadOutcome::Sent(bytes) => {
                    report.sent += 1;
                    report.bytes_transferred += bytes;
                }
                UploadOutcome::Failed => report.failed += 1,
                UploadOutcome::AuthenticationInvalid => {
                    report.terminal = SendTerminal::AuthenticationInvalid;
                    return Ok(report);
                }
                UploadOutcome::AuthenticationRevoked => {
                    report.terminal = SendTerminal::AuthenticationRevoked;
                    return Ok(report);
                }
            }
        }
    }
    Ok(report)
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
    let parsed_chain = tls::parse_certs(&ca_chain).map_err(transport_error)?;
    let Some(first_ca) = parsed_chain.first() else {
        return Err(TransferError::CredentialLoad(
            "chain.pem contains no certificates".to_string(),
        ));
    };
    let endpoints = endpoint_addrs(peer_json.get("local_endpoints"));
    Ok(Credential {
        client_key_pem: private_key,
        client_cert_pem: client_cert,
        ca_chain_pem: vec![ca_chain],
        ca_fp_prefix: spl_core::ca::sha256(first_ca.as_ref())[..16].to_vec(),
        instance_id: peer.instance_id.clone(),
        home_label: peer.label.clone(),
        endpoints,
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

struct LoopbackClient {
    agent: ureq::Agent,
    base_url: String,
    host: String,
}

fn query_remote_manifest(
    loopback: &LoopbackClient,
    path: &str,
) -> BTreeMap<String, BTreeMap<String, SegmentManifest>> {
    let Ok(mut response) = loopback
        .agent
        .get(format!("{}{path}", loopback.base_url))
        .header("host", &loopback.host)
        .call()
    else {
        return BTreeMap::new();
    };
    if response.status().as_u16() != 200 {
        return BTreeMap::new();
    }
    let Ok(body) = response.body_mut().read_to_vec() else {
        return BTreeMap::new();
    };
    serde_json::from_slice(&body).unwrap_or_default()
}

enum UploadOutcome {
    Sent(u64),
    Failed,
    AuthenticationInvalid,
    AuthenticationRevoked,
}

fn upload_segment(
    loopback: &LoopbackClient,
    path: &str,
    day: &str,
    stream: &str,
    key: &str,
    files: &[PathBuf],
) -> Result<UploadOutcome, TransferError> {
    let bytes = files
        .iter()
        .map(|file| fs::metadata(file).map(|metadata| metadata.len()))
        .sum::<Result<u64, _>>()?;
    for (attempt, delay) in RETRY_BACKOFF.iter().enumerate() {
        let (body, boundary) = multipart_body(day, stream, key, files)?;
        let response = loopback
            .agent
            .post(format!("{}{path}", loopback.base_url))
            .header("host", &loopback.host)
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .send(body);
        match response {
            Ok(mut response) => {
                let status = response.status().as_u16();
                let _ = response.body_mut().read_to_vec();
                match status {
                    200 => return Ok(UploadOutcome::Sent(bytes)),
                    401 => return Ok(UploadOutcome::AuthenticationInvalid),
                    403 => return Ok(UploadOutcome::AuthenticationRevoked),
                    500..=599 if attempt + 1 < RETRY_BACKOFF.len() => std::thread::sleep(*delay),
                    500..=599 => return Ok(UploadOutcome::Failed),
                    _ => return Ok(UploadOutcome::Failed),
                }
            }
            Err(_) if attempt + 1 < RETRY_BACKOFF.len() => std::thread::sleep(*delay),
            Err(_) => return Ok(UploadOutcome::Failed),
        }
    }
    Ok(UploadOutcome::Failed)
}

fn multipart_body(
    day: &str,
    stream: &str,
    key: &str,
    files: &[PathBuf],
) -> Result<(Vec<u8>, String), TransferError> {
    let boundary = format!(
        "solstone-native-{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let metadata = json!({
        "segments": [{
            "day": day,
            "stream": stream,
            "segment_key": key,
            "files": files.iter().filter_map(|file| file.file_name()).map(|name| name.to_string_lossy()).collect::<Vec<_>>(),
        }],
    })
    .to_string();
    let mut body = Vec::new();
    push_multipart_field(&mut body, &boundary, "metadata", metadata.as_bytes());
    for file in files {
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"files_0\"; filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n",
                escape_multipart(name)
            )
            .as_bytes(),
        );
        body.extend_from_slice(&fs::read(file)?);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok((body, boundary))
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

fn segment_files(segment: &Path) -> Result<Vec<PathBuf>, TransferError> {
    let mut files = fs::read_dir(segment)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|entry| {
            let path = entry.path();
            (path.is_file()
                && !RESERVED_SEGMENT_FILENAMES
                    .contains(&entry.file_name().to_string_lossy().as_ref()))
            .then_some(path)
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn manifests_match(local: &SegmentManifest, remote: Option<&SegmentManifest>) -> bool {
    let local = local
        .files
        .iter()
        .map(|file| (&file.name, &file.sha256))
        .collect::<BTreeMap<_, _>>();
    let remote = remote
        .map(|manifest| {
            manifest
                .files
                .iter()
                .map(|file| (&file.name, &file.sha256))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    local == remote
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
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(UPLOAD_TIMEOUT))
        .build();
    ureq::Agent::new_with_config(config)
}

fn is_eight_digit_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn parse_calendar_day(value: &str) -> Result<NaiveDate, TransferError> {
    NaiveDate::parse_from_str(value, "%Y%m%d").map_err(|_| TransferError::InvalidDay)
}

fn transport_error(error: impl std::fmt::Display) -> TransferError {
    TransferError::Transport(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_parser_matches_python_shapes() {
        let root = tempfile::tempdir().expect("temporary journal");
        fs::create_dir_all(root.path().join("chronicle/20260203")).expect("day");
        fs::create_dir_all(root.path().join("chronicle/invalid")).expect("invalid");
        assert_eq!(parse_day_spec(None, root.path()).unwrap(), ["20260203"]);
        assert_eq!(
            parse_day_spec(Some("20260230"), root.path()).unwrap(),
            ["20260230"]
        );
        assert_eq!(
            parse_day_spec(Some("20260227-20260301"), root.path()).unwrap(),
            ["20260227", "20260228", "20260301"]
        );
        assert!(matches!(
            parse_day_spec(Some("20260230-20260301"), root.path()),
            Err(TransferError::InvalidDay)
        ));
        assert!(matches!(
            parse_day_spec(Some("20260301-20260228"), root.path()),
            Err(TransferError::InvalidDay)
        ));
    }

    #[test]
    fn peer_resolver_reports_python_compatible_messages() {
        let root = tempfile::tempdir().expect("temporary journal");
        assert_eq!(
            resolve_peer(root.path(), "office").unwrap_err().to_string(),
            "no peers paired (run \"sol link join --as peer\" first)"
        );
        let peers = root.path().join("peers");
        for (directory, label, instance_id) in [
            ("z", "zebra", "second"),
            ("a", "alpha", "first"),
            ("b", "alpha", "third"),
        ] {
            let dir = peers.join(directory);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("peer.json"),
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

    #[test]
    fn reserved_files_are_excluded() {
        let root = tempfile::tempdir().expect("segment");
        for name in RESERVED_SEGMENT_FILENAMES {
            fs::write(root.path().join(name), b"control").unwrap();
        }
        fs::write(root.path().join("payload.json"), b"payload").unwrap();
        assert_eq!(
            segment_files(root.path()).unwrap(),
            [root.path().join("payload.json")]
        );
    }

    #[test]
    fn credential_uses_the_chain_certificate_fingerprint() {
        use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};

        let root = tempfile::tempdir().expect("temporary journal");
        let peer_dir = root.path().join("peers/instance-12345678");
        fs::create_dir_all(&peer_dir).unwrap();
        let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let ca = CertificateParams::new(Vec::<String>::new())
            .unwrap()
            .self_signed(&ca_key)
            .unwrap();
        fs::write(peer_dir.join("private.pem"), ca_key.serialize_pem()).unwrap();
        fs::write(peer_dir.join("cert.pem"), ca.pem()).unwrap();
        fs::write(peer_dir.join("chain.pem"), ca.pem()).unwrap();
        fs::write(peer_dir.join("home_attestation.jwt"), "attestation").unwrap();
        fs::write(
            peer_dir.join("peer.json"),
            json!({
                "label": "office",
                "instance_id": "instance-12345678",
                "local_endpoints": [{"ip": "127.0.0.1", "port": 7657}],
            })
            .to_string(),
        )
        .unwrap();
        let peer = resolve_peer(root.path(), "office").unwrap();
        let credential = load_credential(root.path(), &peer).unwrap();
        let parsed = tls::parse_certs(&ca.pem()).unwrap();
        assert_eq!(
            credential.ca_fp_prefix,
            spl_core::ca::sha256(parsed[0].as_ref())[..16]
        );
        assert_eq!(credential.endpoints[0].host, "127.0.0.1");
        assert!(credential.device_token.is_none());
        assert!(TransportClient::new(credential, None).is_ok());
    }
}
