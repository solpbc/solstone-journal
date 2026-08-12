// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Origin read verification shared by the gate and mirror publish flow.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use solstone_core_assets::Artifact;
use solstone_core_local::install::archive::{ArchiveError, DownloadHostPolicy, download_verified};
use thiserror::Error;

use crate::pins::{
    OriginPin, PinsError, authority_origin_pins, catalog_origin_pins, head_origin_pins,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateTarget {
    pub origin_key: String,
    pub sha256: String,
    pub size_bytes: Option<u64>,
    pub unit: String,
    pub version: Option<String>,
    pub upstream_url: Option<String>,
}

#[derive(Debug, Error)]
pub enum GateError {
    #[error(transparent)]
    Pins(#[from] PinsError),
    #[error("origin target {origin_key} has no declared size")]
    TargetMissingSize { origin_key: String },
    #[error("cannot create gate destination {path}: {source}")]
    DestinationCreate {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("origin verification failed for {origin_key}: {source}")]
    Download {
        origin_key: String,
        #[source]
        source: ArchiveError,
    },
    #[error("invalid HEAD target derivation: {detail}")]
    HeadTargetInvariant { detail: String },
}

pub fn head_gate_targets() -> Result<Vec<GateTarget>, GateError> {
    Ok(head_origin_pins()?
        .into_iter()
        .map(GateTarget::from)
        .collect())
}

pub fn verify_targets(
    targets: &[GateTarget],
    destination_dir: &Path,
    policy: &DownloadHostPolicy<'_>,
) -> Result<(), GateError> {
    std::fs::create_dir_all(destination_dir).map_err(|source| GateError::DestinationCreate {
        path: destination_dir.to_path_buf(),
        source,
    })?;
    for (index, target) in targets.iter().enumerate() {
        let size_bytes = target
            .size_bytes
            .ok_or_else(|| GateError::TargetMissingSize {
                origin_key: target.origin_key.clone(),
            })?;
        let artifact = artifact_for(target, size_bytes);
        let destination = destination_dir.join(format!("origin-target-{index}"));
        download_verified(&artifact, &destination, policy, |_, _| {}).map_err(|source| {
            GateError::Download {
                origin_key: target.origin_key.clone(),
                source,
            }
        })?;
    }
    Ok(())
}

pub fn assert_head_targets_correspond() -> Result<(), GateError> {
    let targets = head_gate_targets()?;
    let mut expected = catalog_origin_pins();
    expected.extend(authority_origin_pins()?);
    let target_keys = targets
        .iter()
        .map(|target| target.origin_key.as_str())
        .collect::<BTreeSet<_>>();
    let expected_keys = expected
        .iter()
        .map(|target| target.origin_key.as_str())
        .collect::<BTreeSet<_>>();
    if target_keys.len() != targets.len()
        || expected_keys.len() != expected.len()
        || target_keys != expected_keys
        || targets.iter().any(|target| {
            target.origin_key.is_empty()
                || target.sha256.len() != 64
                || !target.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err(GateError::HeadTargetInvariant {
            detail: "catalog and authority targets must correspond once to HEAD targets".to_owned(),
        });
    }
    Ok(())
}

impl From<OriginPin> for GateTarget {
    fn from(pin: OriginPin) -> Self {
        Self {
            origin_key: pin.origin_key,
            sha256: pin.sha256,
            size_bytes: pin.size_bytes,
            unit: pin.unit,
            version: pin.version,
            upstream_url: pin.upstream_url,
        }
    }
}

fn artifact_for(target: &GateTarget, size_bytes: u64) -> Artifact {
    Artifact {
        unit: leak(&target.unit),
        version: leak(target.version.as_deref().unwrap_or("head")),
        filename: leak(&target.origin_key),
        sha256: leak(&target.sha256),
        size_bytes,
        upstream_url: leak(target.upstream_url.as_deref().unwrap_or("")),
        origin_key: leak(&target.origin_key),
        artifact_key: None,
        platform: None,
        backend: None,
        extracted_binary_sha256: None,
    }
}

fn leak(value: &str) -> &'static str {
    Box::leak(value.to_owned().into_boxed_str())
}
