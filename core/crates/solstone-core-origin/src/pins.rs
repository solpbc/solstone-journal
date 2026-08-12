// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pin inputs owned by the repository rather than a shipped runtime package.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

const SNAPSHOTS: &[(&str, &str)] = &[
    ("1.0.12", include_str!("../pins/v1.0.12.json")),
    ("1.0.13", include_str!("../pins/v1.0.13.json")),
    ("1.0.15", include_str!("../pins/v1.0.15.json")),
    ("1.0.16", include_str!("../pins/v1.0.16.json")),
    ("1.0.17", include_str!("../pins/v1.0.17.json")),
    ("1.0.18", include_str!("../pins/v1.0.18.json")),
    ("1.0.19", include_str!("../pins/v1.0.19.json")),
    ("1.0.20", include_str!("../pins/v1.0.20.json")),
    ("1.0.21", include_str!("../pins/v1.0.21.json")),
    ("1.0.22", include_str!("../pins/v1.0.22.json")),
];

const AUTHORITY_PATH: &str = "solstone/think/providers/nvattest_authority_v1.json";
const TRANSPARENCY_LOG_PATH: &str = "transparency-head-log.jsonl";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginPin {
    pub origin_key: String,
    pub sha256: String,
    pub unit: String,
    pub version: Option<String>,
    pub size_bytes: Option<u64>,
    pub upstream_url: Option<String>,
}

#[derive(Debug, Error)]
pub enum PinsError {
    #[error("cannot resolve repository root from {manifest_dir}")]
    RepositoryRootUnavailable { manifest_dir: PathBuf },
    #[error("cannot read nvattest authority {path}: {source}")]
    AuthorityRead {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot parse nvattest authority {path}: {source}")]
    AuthorityParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("cannot read transparency log {path}: {source}")]
    TransparencyLogRead {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot parse transparency log {path} at line {line}: {source}")]
    TransparencyLogParse {
        path: PathBuf,
        line: usize,
        source: serde_json::Error,
    },
    #[error("transparency log {path} has no release rows")]
    TransparencyLogEmpty { path: PathBuf },
    #[error("cannot parse pin snapshot for release {release_version}: {source}")]
    SnapshotParse {
        release_version: String,
        source: serde_json::Error,
    },
    #[error(
        "pin snapshot filename version {filename_version} disagrees with document version {document_version}"
    )]
    SnapshotVersionMismatch {
        filename_version: String,
        document_version: String,
    },
    #[error("pin snapshot {release_version} repeats origin key {origin_key}")]
    SnapshotDuplicateOriginKey {
        release_version: String,
        origin_key: String,
    },
    #[error("nvattest authority has invalid target {target}: {detail}")]
    AuthorityTargetInvalid { target: String, detail: String },
    #[error("nvattest authority {path} has no targets")]
    AuthorityTargetsEmpty { path: PathBuf },
    #[error("head pin set repeats origin key {origin_key}")]
    HeadDuplicateOriginKey { origin_key: String },
}

#[derive(Debug, Deserialize)]
struct Snapshot {
    schema_version: u8,
    release_version: String,
    non_origin_upstream_hosts: Vec<String>,
    origin_pins: Vec<SnapshotPin>,
}

#[derive(Debug, Deserialize)]
struct SnapshotPin {
    origin_key: String,
    sha256: String,
    unit: String,
}

#[derive(Debug, Deserialize)]
struct TransparencyRow {
    version: String,
}

pub fn repository_root() -> Result<PathBuf, PinsError> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .nth(3)
        .filter(|root| root.join("core").is_dir() && root.join("solstone").is_dir())
        .map(Path::to_path_buf)
        .ok_or(PinsError::RepositoryRootUnavailable { manifest_dir })
}

pub fn transparency_log_path() -> Result<PathBuf, PinsError> {
    Ok(repository_root()?.join(TRANSPARENCY_LOG_PATH))
}

pub fn authority_path() -> Result<PathBuf, PinsError> {
    Ok(repository_root()?.join(AUTHORITY_PATH))
}

pub fn supported_release_versions() -> Result<BTreeSet<String>, PinsError> {
    let path = transparency_log_path()?;
    supported_release_versions_from_path(&path)
}

fn supported_release_versions_from_path(path: &Path) -> Result<BTreeSet<String>, PinsError> {
    let text = fs::read_to_string(path).map_err(|source| PinsError::TransparencyLogRead {
        path: path.to_path_buf(),
        source,
    })?;
    let versions = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str::<TransparencyRow>(line)
                .map(|row| row.version)
                .map_err(|source| PinsError::TransparencyLogParse {
                    path: path.to_path_buf(),
                    line: index + 1,
                    source,
                })
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if versions.is_empty() {
        return Err(PinsError::TransparencyLogEmpty {
            path: path.to_path_buf(),
        });
    }
    Ok(versions)
}

#[cfg(test)]
pub(super) fn supported_release_versions_from_test_path(
    path: &Path,
) -> Result<BTreeSet<String>, PinsError> {
    supported_release_versions_from_path(path)
}

pub fn snapshot_versions() -> Result<BTreeSet<String>, PinsError> {
    snapshots().map(|items| items.into_keys().collect())
}

pub fn historical_origin_pins() -> Result<BTreeMap<String, Vec<OriginPin>>, PinsError> {
    snapshots()
}

pub fn head_origin_pins() -> Result<Vec<OriginPin>, PinsError> {
    let mut pins = catalog_origin_pins();
    pins.extend(authority_origin_pins()?);
    let mut keys = BTreeSet::new();
    for pin in &pins {
        if !keys.insert(pin.origin_key.clone()) {
            return Err(PinsError::HeadDuplicateOriginKey {
                origin_key: pin.origin_key.clone(),
            });
        }
    }
    Ok(pins)
}

pub fn catalog_origin_pins() -> Vec<OriginPin> {
    solstone_core_assets::catalog()
        .iter()
        .map(|artifact| OriginPin {
            origin_key: artifact.origin_key.to_owned(),
            sha256: artifact.sha256.to_owned(),
            unit: artifact.unit.to_owned(),
            version: Some(artifact.version.to_owned()),
            size_bytes: Some(artifact.size_bytes),
            upstream_url: Some(artifact.upstream_url.to_owned()),
        })
        .collect()
}

fn snapshots() -> Result<BTreeMap<String, Vec<OriginPin>>, PinsError> {
    SNAPSHOTS
        .iter()
        .map(|(filename_version, text)| {
            let snapshot = serde_json::from_str::<Snapshot>(text).map_err(|source| {
                PinsError::SnapshotParse {
                    release_version: (*filename_version).to_owned(),
                    source,
                }
            })?;
            if snapshot.release_version != *filename_version {
                return Err(PinsError::SnapshotVersionMismatch {
                    filename_version: (*filename_version).to_owned(),
                    document_version: snapshot.release_version,
                });
            }
            let _hosts = snapshot.non_origin_upstream_hosts;
            let _schema = snapshot.schema_version;
            let mut keys = BTreeSet::new();
            let pins = snapshot
                .origin_pins
                .into_iter()
                .map(|pin| {
                    if !keys.insert(pin.origin_key.clone()) {
                        return Err(PinsError::SnapshotDuplicateOriginKey {
                            release_version: (*filename_version).to_owned(),
                            origin_key: pin.origin_key,
                        });
                    }
                    Ok(OriginPin {
                        origin_key: pin.origin_key,
                        sha256: pin.sha256,
                        unit: pin.unit,
                        version: Some((*filename_version).to_owned()),
                        size_bytes: None,
                        upstream_url: None,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(((*filename_version).to_owned(), pins))
        })
        .collect()
}

pub fn authority_origin_pins() -> Result<Vec<OriginPin>, PinsError> {
    let path = authority_path()?;
    authority_origin_pins_from_path(&path)
}

fn authority_origin_pins_from_path(path: &Path) -> Result<Vec<OriginPin>, PinsError> {
    let text = fs::read_to_string(path).map_err(|source| PinsError::AuthorityRead {
        path: path.to_path_buf(),
        source,
    })?;
    let authority: Value =
        serde_json::from_str(&text).map_err(|source| PinsError::AuthorityParse {
            path: path.to_path_buf(),
            source,
        })?;
    let targets = authority
        .get("targets")
        .and_then(Value::as_object)
        .ok_or_else(|| PinsError::AuthorityTargetInvalid {
            target: "targets".to_owned(),
            detail: "missing object".to_owned(),
        })?;
    if targets.is_empty() {
        return Err(PinsError::AuthorityTargetsEmpty {
            path: path.to_path_buf(),
        });
    }
    let mut pins = Vec::new();
    for (target, entry) in targets {
        let source = entry
            .get("source")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid_authority(target, "missing source object"))?;
        let prefix = source
            .get("url_prefix")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_authority(target, "missing source.url_prefix"))?;
        let version = source
            .get("version")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_authority(target, "missing source.version"))?;
        for object_name in ["artifact", "companion_manifest"] {
            let object = entry
                .get(object_name)
                .and_then(Value::as_object)
                .ok_or_else(|| invalid_authority(target, &format!("missing {object_name}")))?;
            let name = object
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_authority(target, &format!("missing {object_name}.name")))?;
            let sha256 = object
                .get("sha256")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    invalid_authority(target, &format!("missing {object_name}.sha256"))
                })?;
            let url = object
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_authority(target, &format!("missing {object_name}.url")))?;
            let size_bytes = object.get("size_bytes").and_then(Value::as_u64);
            let origin_key = url.strip_prefix(prefix).ok_or_else(|| {
                invalid_authority(target, "object URL is outside source.url_prefix")
            })?;
            pins.push(OriginPin {
                origin_key: format!("providers/nvattest/{origin_key}"),
                sha256: sha256.to_owned(),
                unit: "nvattest".to_owned(),
                version: Some(version.to_owned()),
                size_bytes,
                upstream_url: Some(url.to_owned()),
            });
            if origin_key != name {
                return Err(invalid_authority(
                    target,
                    "object URL basename disagrees with name",
                ));
            }
        }
    }
    Ok(pins)
}

#[cfg(test)]
pub(super) fn authority_origin_pins_from_test_path(
    path: &Path,
) -> Result<Vec<OriginPin>, PinsError> {
    authority_origin_pins_from_path(path)
}

fn invalid_authority(target: &str, detail: &str) -> PinsError {
    PinsError::AuthorityTargetInvalid {
        target: target.to_owned(),
        detail: detail.to_owned(),
    }
}
