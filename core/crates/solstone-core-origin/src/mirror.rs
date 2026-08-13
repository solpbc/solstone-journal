// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fetch, verify, publish, read back, then append origin provenance.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use solstone_core_local::install::archive::DownloadHostPolicy;
use thiserror::Error;

use crate::gate::{GateError, GateTarget, verify_targets};

pub const MULTIPART_THRESHOLD_BYTES: u64 = 300 * 1024 * 1024;
const MAX_REDIRECT_HOPS: u8 = 5;
const UPSTREAM_ALLOWED_HOSTS: &[&str] = &[
    "github.com",
    "api.github.com",
    "huggingface.co",
    "release-assets.githubusercontent.com",
    "cdn-lfs.huggingface.co",
    "cas-bridge.xethub.hf.co",
];
#[derive(Debug, Clone, Copy)]
pub struct UpstreamHostPolicy<'a> {
    pub allowed_hosts: &'a [&'a str],
    pub allow_http: bool,
}

pub const PRODUCTION_UPSTREAM_POLICY: UpstreamHostPolicy<'static> = UpstreamHostPolicy {
    allowed_hosts: UPSTREAM_ALLOWED_HOSTS,
    allow_http: false,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishMode {
    SingleShot,
    Multipart,
}

pub fn select_publish_mode(size_bytes: u64) -> PublishMode {
    if size_bytes <= MULTIPART_THRESHOLD_BYTES {
        PublishMode::SingleShot
    } else {
        PublishMode::Multipart
    }
}

/// HuggingFace serves a 64-hex SHA-256 in `x-linked-etag` for LFS-tracked
/// objects and a 40-hex git blob SHA-1 for non-LFS objects. Both forms are
/// corroborated by their respective digest; 11 parakeet-coreml rows are non-LFS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpstreamVerification {
    UpstreamSha256,
    UpstreamGitBlobSha1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpstreamMetadataVerification {
    verification: UpstreamVerification,
    expected_blob_oid: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamMetadataKind {
    GithubRelease,
    HuggingFace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorTarget {
    pub origin_key: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub upstream_url: String,
    pub unit: String,
    pub version: String,
    pub filename: String,
    pub metadata_kind: UpstreamMetadataKind,
    pub metadata_url: String,
}

pub struct PublishRequest<'a> {
    pub target: &'a MirrorTarget,
    pub source: &'a Path,
}

/// A backend selects only the transport requested by the object size.
pub trait PublishBackend {
    fn publish(&self, mode: PublishMode, request: PublishRequest<'_>) -> Result<(), MirrorError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirrorOutcome {
    Mirrored {
        origin_key: String,
        verification: UpstreamVerification,
    },
    Skipped {
        origin_key: String,
        reason: &'static str,
    },
}

#[derive(Debug, Error)]
pub enum MirrorError {
    #[error("cannot create staging directory {path}: {source}")]
    StagingCreate {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot create staging file {path}: {source}")]
    StagingOpen {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot write staging file {path}: {source}")]
    StagingWrite {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("upstream request failed for {url}: {message}")]
    UpstreamRequest { url: String, message: String },
    #[error("upstream metadata did not corroborate {origin_key}")]
    UnverifiedUpstream { origin_key: String },
    #[error("upstream object size mismatch for {origin_key}: expected {expected}, got {actual}")]
    UpstreamSizeMismatch {
        origin_key: String,
        expected: u64,
        actual: u64,
    },
    #[error("upstream object digest mismatch for {origin_key}")]
    UpstreamDigestMismatch { origin_key: String },
    #[error("upstream git blob digest mismatch for {origin_key}")]
    UpstreamGitBlobDigestMismatch { origin_key: String },
    #[error("unsupported upstream URL {url}")]
    UnsupportedUpstream { url: String },
    #[error("upstream host refused: {host}")]
    UpstreamHostRefused { host: String },
    #[error("upstream scheme refused for {host}: {scheme}")]
    UpstreamInsecureScheme { scheme: String, host: String },
    #[error("upstream redirect hop limit exceeded: {limit}")]
    UpstreamRedirectHopLimitExceeded { limit: u8 },
    #[error("upstream URL authority must not include userinfo: {authority}")]
    UpstreamUrlUserinfoRefused { authority: String },
    #[error("invalid upstream URL: {detail}")]
    UpstreamUrlInvalid { detail: String },
    #[error(transparent)]
    OriginReadBack(#[from] GateError),
    #[error("cannot append provenance {path}: {source}")]
    ProvenanceAppend {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot serialize provenance: {source}")]
    ProvenanceSerialize { source: serde_json::Error },
}

pub fn current_mirror_targets() -> Result<Vec<MirrorTarget>, MirrorError> {
    solstone_core_assets::catalog()
        .iter()
        .filter(|artifact| artifact.unit != "llama-server-cuda")
        .map(|artifact| {
            Ok(MirrorTarget {
                origin_key: artifact.origin_key.to_owned(),
                sha256: artifact.sha256.to_owned(),
                size_bytes: artifact.size_bytes,
                upstream_url: artifact.upstream_url.to_owned(),
                unit: artifact.unit.to_owned(),
                version: artifact.version.to_owned(),
                filename: artifact.filename.to_owned(),
                metadata_kind: metadata_kind(artifact.upstream_url),
                metadata_url: metadata_url(artifact.upstream_url)?,
            })
        })
        .collect()
}

pub fn mirror_current_catalog(
    backend: &impl PublishBackend,
    staging_dir: &Path,
    provenance_log: &Path,
    origin_policy: &DownloadHostPolicy<'_>,
    upstream_policy: &UpstreamHostPolicy<'_>,
) -> Result<Vec<MirrorOutcome>, MirrorError> {
    let mut outcomes = Vec::new();
    for artifact in solstone_core_assets::catalog() {
        if artifact.unit == "llama-server-cuda" {
            // Origin-to-origin copying cannot repair these runtimes; recover them by
            // rerunning scripts/repack_cuda_runtime.py against the pinned OCI image.
            outcomes.push(MirrorOutcome::Skipped {
                origin_key: artifact.origin_key.to_owned(),
                reason: "upstream URL is the origin; rerun scripts/repack_cuda_runtime.py",
            });
        }
    }
    for target in current_mirror_targets()? {
        outcomes.push(mirror_one(
            &target,
            backend,
            staging_dir,
            provenance_log,
            origin_policy,
            upstream_policy,
        )?);
    }
    Ok(outcomes)
}

pub fn mirror_one(
    target: &MirrorTarget,
    backend: &impl PublishBackend,
    staging_dir: &Path,
    provenance_log: &Path,
    origin_policy: &DownloadHostPolicy<'_>,
    upstream_policy: &UpstreamHostPolicy<'_>,
) -> Result<MirrorOutcome, MirrorError> {
    let verification = verify_upstream_metadata(target, upstream_policy)?;
    fs::create_dir_all(staging_dir).map_err(|source| MirrorError::StagingCreate {
        path: staging_dir.to_path_buf(),
        source,
    })?;
    let source = staging_dir.join("mirror-source");
    download_upstream(
        target,
        &source,
        upstream_policy,
        verification.expected_blob_oid.as_deref(),
    )?;
    backend.publish(
        select_publish_mode(target.size_bytes),
        PublishRequest {
            target,
            source: &source,
        },
    )?;
    read_back_before_logging(target, staging_dir, origin_policy)?;
    append_provenance(provenance_log, target, verification.verification)?;
    Ok(MirrorOutcome::Mirrored {
        origin_key: target.origin_key.clone(),
        verification: verification.verification,
    })
}

fn read_back_before_logging(
    target: &MirrorTarget,
    staging_dir: &Path,
    origin_policy: &DownloadHostPolicy<'_>,
) -> Result<(), MirrorError> {
    let read_back = GateTarget {
        origin_key: target.origin_key.clone(),
        sha256: target.sha256.clone(),
        size_bytes: Some(target.size_bytes),
        unit: target.unit.clone(),
        version: Some(target.version.clone()),
        upstream_url: Some(target.upstream_url.clone()),
    };
    verify_targets(&[read_back], &staging_dir.join("read-back"), origin_policy)?;
    Ok(())
}

fn verify_upstream_metadata(
    target: &MirrorTarget,
    policy: &UpstreamHostPolicy<'_>,
) -> Result<UpstreamMetadataVerification, MirrorError> {
    match target.metadata_kind {
        UpstreamMetadataKind::GithubRelease => verify_github_metadata(target, policy),
        UpstreamMetadataKind::HuggingFace => verify_huggingface_metadata(target, policy),
    }
}

fn verify_github_metadata(
    target: &MirrorTarget,
    policy: &UpstreamHostPolicy<'_>,
) -> Result<UpstreamMetadataVerification, MirrorError> {
    let url = &target.metadata_url;
    let response = request(url, policy)?;
    let body =
        response
            .into_body()
            .read_to_string()
            .map_err(|error| MirrorError::UpstreamRequest {
                url: url.clone(),
                message: error.to_string(),
            })?;
    let release: Value =
        serde_json::from_str(&body).map_err(|_| MirrorError::UnverifiedUpstream {
            origin_key: target.origin_key.clone(),
        })?;
    let expected = format!("sha256:{}", target.sha256);
    let matches = release
        .get("assets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|asset| {
            asset.get("name").and_then(Value::as_str) == Some(target.filename.as_str())
                && asset.get("digest").and_then(Value::as_str) == Some(expected.as_str())
        });
    matches
        .then_some(UpstreamMetadataVerification {
            verification: UpstreamVerification::UpstreamSha256,
            expected_blob_oid: None,
        })
        .ok_or_else(|| MirrorError::UnverifiedUpstream {
            origin_key: target.origin_key.clone(),
        })
}

fn verify_huggingface_metadata(
    target: &MirrorTarget,
    policy: &UpstreamHostPolicy<'_>,
) -> Result<UpstreamMetadataVerification, MirrorError> {
    let response = request(&target.metadata_url, policy)?;
    let etag = response
        .headers()
        .get("x-linked-etag")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .trim_matches('"')
        .trim_start_matches("sha256:");
    if etag == target.sha256 {
        return Ok(UpstreamMetadataVerification {
            verification: UpstreamVerification::UpstreamSha256,
            expected_blob_oid: None,
        });
    }
    let is_git_blob_sha1 = etag.len() == 40 && etag.bytes().all(|byte| byte.is_ascii_hexdigit());
    if is_git_blob_sha1 {
        Ok(UpstreamMetadataVerification {
            verification: UpstreamVerification::UpstreamGitBlobSha1,
            expected_blob_oid: Some(etag.to_owned()),
        })
    } else {
        Err(MirrorError::UnverifiedUpstream {
            origin_key: target.origin_key.clone(),
        })
    }
}

#[cfg(test)]
pub(crate) fn verify_huggingface_metadata_for_test(
    target: &MirrorTarget,
    policy: &UpstreamHostPolicy<'_>,
) -> Result<(UpstreamVerification, Option<String>), MirrorError> {
    let verification = verify_huggingface_metadata(target, policy)?;
    Ok((verification.verification, verification.expected_blob_oid))
}

fn download_upstream(
    target: &MirrorTarget,
    destination: &Path,
    policy: &UpstreamHostPolicy<'_>,
    expected_blob_oid: Option<&str>,
) -> Result<(), MirrorError> {
    let response = request(&target.upstream_url, policy)?;
    let mut body = response.into_body().into_reader();
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(destination)
        .map_err(|source| MirrorError::StagingOpen {
            path: destination.to_path_buf(),
            source,
        })?;
    let mut digest = Sha256::new();
    let mut blob_digest = expected_blob_oid.map(|_| {
        let mut digest = Sha1::new();
        digest.update(b"blob ");
        digest.update(target.size_bytes.to_string().as_bytes());
        digest.update(b"\0");
        digest
    });
    let mut actual = 0_u64;
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        let read = body
            .read(&mut chunk)
            .map_err(|error| MirrorError::UpstreamRequest {
                url: target.upstream_url.clone(),
                message: error.to_string(),
            })?;
        if read == 0 {
            break;
        }
        file.write_all(&chunk[..read])
            .map_err(|source| MirrorError::StagingWrite {
                path: destination.to_path_buf(),
                source,
            })?;
        digest.update(&chunk[..read]);
        if let Some(blob_digest) = blob_digest.as_mut() {
            blob_digest.update(&chunk[..read]);
        }
        actual += read as u64;
    }
    if actual != target.size_bytes {
        return Err(MirrorError::UpstreamSizeMismatch {
            origin_key: target.origin_key.clone(),
            expected: target.size_bytes,
            actual,
        });
    }
    let actual_digest = format!("{:x}", digest.finalize());
    if actual_digest != target.sha256 {
        return Err(MirrorError::UpstreamDigestMismatch {
            origin_key: target.origin_key.clone(),
        });
    }
    if let Some(expected_blob_oid) = expected_blob_oid {
        let actual_blob_oid = format!(
            "{:x}",
            blob_digest.expect("blob digest was initialized").finalize()
        );
        if actual_blob_oid != expected_blob_oid {
            return Err(MirrorError::UpstreamGitBlobDigestMismatch {
                origin_key: target.origin_key.clone(),
            });
        }
    }
    Ok(())
}

fn request(
    url: &str,
    policy: &UpstreamHostPolicy<'_>,
) -> Result<ureq::http::Response<ureq::Body>, MirrorError> {
    let mut current = validate_url(url, policy)?;
    let agent = ureq::agent();
    let mut followed = 0_u8;
    loop {
        let response = agent
            .get(current.as_str())
            .config()
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|error| MirrorError::UpstreamRequest {
                url: current.as_str(),
                message: error.to_string(),
            })?;
        if response.status().is_redirection() {
            if followed == MAX_REDIRECT_HOPS {
                return Err(MirrorError::UpstreamRedirectHopLimitExceeded {
                    limit: MAX_REDIRECT_HOPS,
                });
            }
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| MirrorError::UpstreamUrlInvalid {
                    detail: "redirect response has no Location header".to_owned(),
                })?;
            current = validate_url(&resolve_location(&current, location)?.as_str(), policy)?;
            followed += 1;
            continue;
        }
        if response.status().is_success() {
            return Ok(response);
        }
        return Err(MirrorError::UpstreamRequest {
            url: current.as_str(),
            message: format!("unexpected HTTP status {}", response.status()),
        });
    }
}

fn metadata_url(upstream_url: &str) -> Result<String, MirrorError> {
    if upstream_url.contains("github.com/") {
        github_release_api_url(upstream_url)
    } else {
        Ok(upstream_url.to_owned())
    }
}

#[derive(Debug, Clone)]
struct AbsoluteUrl {
    scheme: String,
    authority: String,
    host: String,
    path_and_query: String,
}

impl AbsoluteUrl {
    fn as_str(&self) -> String {
        format!(
            "{}://{}{}",
            self.scheme, self.authority, self.path_and_query
        )
    }
}

fn validate_url(url: &str, policy: &UpstreamHostPolicy<'_>) -> Result<AbsoluteUrl, MirrorError> {
    let parsed = parse_absolute_url(url)?;
    if !policy
        .allowed_hosts
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&parsed.host))
    {
        return Err(MirrorError::UpstreamHostRefused { host: parsed.host });
    }
    if parsed.scheme == "http" && !policy.allow_http {
        return Err(MirrorError::UpstreamInsecureScheme {
            scheme: parsed.scheme,
            host: parsed.host,
        });
    }
    Ok(parsed)
}

fn parse_absolute_url(url: &str) -> Result<AbsoluteUrl, MirrorError> {
    let url = url.split('#').next().unwrap_or_default();
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| invalid_url("URL must be absolute http(s) URL"))?;
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return Err(invalid_url("unsupported URL scheme"));
    }
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.contains('@') {
        return Err(MirrorError::UpstreamUrlUserinfoRefused {
            authority: authority.to_owned(),
        });
    }
    let (host, authority) = parse_authority(authority)?;
    let path_and_query = match &rest[authority_end..] {
        "" => "/".to_owned(),
        query if query.starts_with('?') => format!("/{query}"),
        path => path.to_owned(),
    };
    Ok(AbsoluteUrl {
        scheme,
        authority,
        host,
        path_and_query,
    })
}

fn parse_authority(authority: &str) -> Result<(String, String), MirrorError> {
    if authority.is_empty() || authority.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(invalid_url("URL has malformed authority"));
    }
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) if !port.contains(':') && !port.is_empty() => {
            if !port.bytes().all(|byte| byte.is_ascii_digit()) || port.parse::<u16>().is_err() {
                return Err(invalid_url("URL has malformed port"));
            }
            (host, Some(port))
        }
        Some(_) => return Err(invalid_url("URL has malformed host")),
        None => (authority, None),
    };
    if host.is_empty() {
        return Err(invalid_url("URL has empty host"));
    }
    let host = host.to_ascii_lowercase();
    let authority = port.map_or_else(|| host.clone(), |port| format!("{host}:{port}"));
    Ok((host, authority))
}

fn resolve_location(current: &AbsoluteUrl, location: &str) -> Result<AbsoluteUrl, MirrorError> {
    let location = location.split('#').next().unwrap_or_default();
    if location.is_empty() {
        return Err(invalid_url("redirect Location is empty"));
    }
    if location.contains("://") {
        return parse_absolute_url(location);
    }
    if location.starts_with("//") {
        return parse_absolute_url(&format!("{}:{location}", current.scheme));
    }
    let path_and_query =
        if location.starts_with('/') {
            location.to_owned()
        } else if location.starts_with('?') {
            format!(
                "{}{}",
                current.path_and_query.split('?').next().unwrap_or("/"),
                location
            )
        } else {
            let current_path = current.path_and_query.split('?').next().unwrap_or("/");
            let base = current_path.rsplit_once('/').map_or("/", |(parent, _)| {
                if parent.is_empty() { "/" } else { parent }
            });
            format!("{base}/{location}")
        };
    parse_absolute_url(&format!(
        "{}://{}{}",
        current.scheme, current.authority, path_and_query
    ))
}

fn invalid_url(detail: &str) -> MirrorError {
    MirrorError::UpstreamUrlInvalid {
        detail: detail.to_owned(),
    }
}

fn metadata_kind(upstream_url: &str) -> UpstreamMetadataKind {
    if upstream_url.contains("github.com/") {
        UpstreamMetadataKind::GithubRelease
    } else {
        UpstreamMetadataKind::HuggingFace
    }
}

fn github_release_api_url(upstream_url: &str) -> Result<String, MirrorError> {
    let parts = upstream_url
        .strip_prefix("https://github.com/")
        .or_else(|| upstream_url.strip_prefix("http://github.com/"))
        .ok_or_else(|| MirrorError::UnsupportedUpstream {
            url: upstream_url.to_owned(),
        })?
        .split('/')
        .collect::<Vec<_>>();
    if parts.len() < 5 || parts[2] != "releases" || parts[3] != "download" {
        return Err(MirrorError::UnsupportedUpstream {
            url: upstream_url.to_owned(),
        });
    }
    Ok(format!(
        "https://api.github.com/repos/{}/{}/releases/tags/{}",
        parts[0], parts[1], parts[4]
    ))
}

fn append_provenance(
    path: &Path,
    target: &MirrorTarget,
    verification: UpstreamVerification,
) -> Result<(), MirrorError> {
    let mut row = BTreeMap::new();
    row.insert("origin_key", serde_json::json!(target.origin_key));
    row.insert("pin_sha256", serde_json::json!(target.sha256));
    row.insert("read_back", serde_json::json!("sha256"));
    row.insert("size_bytes", serde_json::json!(target.size_bytes));
    row.insert(
        "timestamp",
        serde_json::json!(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)),
    );
    row.insert("unit", serde_json::json!(target.unit));
    row.insert("upstream_url", serde_json::json!(target.upstream_url));
    row.insert(
        "verified",
        serde_json::to_value(verification)
            .map_err(|source| MirrorError::ProvenanceSerialize { source })?,
    );
    row.insert("version", serde_json::json!(target.version));
    let serialized = serde_json::to_string(&row)
        .map_err(|source| MirrorError::ProvenanceSerialize { source })?;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| MirrorError::ProvenanceAppend {
            path: path.to_path_buf(),
            source,
        })?;
    writeln!(log, "{serialized}").map_err(|source| MirrorError::ProvenanceAppend {
        path: path.to_path_buf(),
        source,
    })
}
