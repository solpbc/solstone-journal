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
use sha2::{Digest, Sha256};
use solstone_core_local::install::archive::DownloadHostPolicy;
use thiserror::Error;

use crate::gate::{GateError, GateTarget, verify_targets};

pub const MULTIPART_THRESHOLD_BYTES: u64 = 300 * 1024 * 1024;

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

/// The sole `SizeOnly` current row is
/// `assets/rerank-model/a09144355adeed5f58c8ed011d209bf8ee5a1fec/tokenizer.json`.
/// It is non-LFS on HuggingFace, so `x-linked-etag` is a 40-hex git blob SHA-1,
/// not a sha256; this is a single row with a specific cause, not a class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpstreamVerification {
    UpstreamSha256,
    SizeOnly,
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
    #[error("unsupported upstream URL {url}")]
    UnsupportedUpstream { url: String },
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

pub fn current_mirror_targets() -> Vec<MirrorTarget> {
    solstone_core_assets::catalog()
        .iter()
        .filter(|artifact| artifact.unit != "llama-server-cuda")
        .map(|artifact| MirrorTarget {
            origin_key: artifact.origin_key.to_owned(),
            sha256: artifact.sha256.to_owned(),
            size_bytes: artifact.size_bytes,
            upstream_url: artifact.upstream_url.to_owned(),
            unit: artifact.unit.to_owned(),
            version: artifact.version.to_owned(),
            filename: artifact.filename.to_owned(),
            metadata_kind: metadata_kind(artifact.upstream_url),
            metadata_url: metadata_url(artifact.upstream_url),
        })
        .collect()
}

pub fn mirror_current_catalog(
    backend: &impl PublishBackend,
    staging_dir: &Path,
    provenance_log: &Path,
    origin_policy: &DownloadHostPolicy<'_>,
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
    for target in current_mirror_targets() {
        outcomes.push(mirror_one(
            &target,
            backend,
            staging_dir,
            provenance_log,
            origin_policy,
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
) -> Result<MirrorOutcome, MirrorError> {
    let verification = verify_upstream_metadata(target)?;
    fs::create_dir_all(staging_dir).map_err(|source| MirrorError::StagingCreate {
        path: staging_dir.to_path_buf(),
        source,
    })?;
    let source = staging_dir.join("mirror-source");
    download_upstream(target, &source)?;
    backend.publish(
        select_publish_mode(target.size_bytes),
        PublishRequest {
            target,
            source: &source,
        },
    )?;
    read_back_before_logging(target, staging_dir, origin_policy)?;
    append_provenance(provenance_log, target, verification)?;
    Ok(MirrorOutcome::Mirrored {
        origin_key: target.origin_key.clone(),
        verification,
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

fn verify_upstream_metadata(target: &MirrorTarget) -> Result<UpstreamVerification, MirrorError> {
    match target.metadata_kind {
        UpstreamMetadataKind::GithubRelease => verify_github_metadata(target),
        UpstreamMetadataKind::HuggingFace => verify_huggingface_metadata(target),
    }
}

fn verify_github_metadata(target: &MirrorTarget) -> Result<UpstreamVerification, MirrorError> {
    let url = &target.metadata_url;
    let response = request(url)?;
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
        .then_some(UpstreamVerification::UpstreamSha256)
        .ok_or_else(|| MirrorError::UnverifiedUpstream {
            origin_key: target.origin_key.clone(),
        })
}

fn verify_huggingface_metadata(target: &MirrorTarget) -> Result<UpstreamVerification, MirrorError> {
    let response = request(&target.metadata_url)?;
    let etag = response
        .headers()
        .get("x-linked-etag")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .trim_matches('"')
        .trim_start_matches("sha256:");
    if etag == target.sha256 {
        return Ok(UpstreamVerification::UpstreamSha256);
    }
    let is_git_blob_sha1 = etag.len() == 40 && etag.bytes().all(|byte| byte.is_ascii_hexdigit());
    let content_length = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if is_git_blob_sha1 && content_length == Some(target.size_bytes) {
        Ok(UpstreamVerification::SizeOnly)
    } else {
        Err(MirrorError::UnverifiedUpstream {
            origin_key: target.origin_key.clone(),
        })
    }
}

fn download_upstream(target: &MirrorTarget, destination: &Path) -> Result<(), MirrorError> {
    let response = request(&target.upstream_url)?;
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
    Ok(())
}

fn request(url: &str) -> Result<ureq::http::Response<ureq::Body>, MirrorError> {
    let response = ureq::agent()
        .get(url)
        .config()
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|error| MirrorError::UpstreamRequest {
            url: url.to_owned(),
            message: error.to_string(),
        })?;
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(MirrorError::UpstreamRequest {
            url: url.to_owned(),
            message: format!("unexpected HTTP status {}", response.status()),
        })
    }
}

fn metadata_url(upstream_url: &str) -> String {
    if upstream_url.contains("github.com/") {
        github_release_api_url(upstream_url).unwrap_or_else(|_| upstream_url.to_owned())
    } else {
        upstream_url.to_owned()
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
