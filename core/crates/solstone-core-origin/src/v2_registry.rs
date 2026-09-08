// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Offline provenance for journal releases admitted through the v2 trust rail.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const REGISTRY_SCHEMA: &str = "solstone-journal/v2-origin-release-registry/v1";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct V2ReleaseRegistry {
    schema: String,
    releases: Vec<VerifiedV2Release>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VerifiedV2Release {
    version: String,
    logical_target: String,
    record_sha256: String,
}

#[derive(Debug, Error)]
pub enum V2RegistryError {
    #[error("cannot read v2 release registry {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot parse v2 release registry {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("v2 release registry {path} has unsupported schema {schema}")]
    Schema { path: PathBuf, schema: String },
    #[error("v2 release registry {path} has invalid entry for {version}: {detail}")]
    InvalidEntry {
        path: PathBuf,
        version: String,
        detail: String,
    },
    #[error("v2 release registry {path} repeats version {version}")]
    DuplicateVersion { path: PathBuf, version: String },
}

fn safe_coordinate(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate(path: &Path, registry: &V2ReleaseRegistry) -> Result<(), V2RegistryError> {
    if registry.schema != REGISTRY_SCHEMA {
        return Err(V2RegistryError::Schema {
            path: path.to_path_buf(),
            schema: registry.schema.clone(),
        });
    }
    let mut versions = BTreeSet::new();
    for entry in &registry.releases {
        if !safe_coordinate(&entry.version) {
            return Err(V2RegistryError::InvalidEntry {
                path: path.to_path_buf(),
                version: entry.version.clone(),
                detail: "version is not a safe release coordinate".to_owned(),
            });
        }
        let expected = format!("software/journal/{}/release-record.json", entry.version);
        if entry.logical_target != expected {
            return Err(V2RegistryError::InvalidEntry {
                path: path.to_path_buf(),
                version: entry.version.clone(),
                detail: format!("logical_target must equal {expected}"),
            });
        }
        if !valid_sha256(&entry.record_sha256) {
            return Err(V2RegistryError::InvalidEntry {
                path: path.to_path_buf(),
                version: entry.version.clone(),
                detail: "record_sha256 must be lowercase SHA-256".to_owned(),
            });
        }
        if !versions.insert(entry.version.clone()) {
            return Err(V2RegistryError::DuplicateVersion {
                path: path.to_path_buf(),
                version: entry.version.clone(),
            });
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<V2ReleaseRegistry, V2RegistryError> {
    let bytes = fs::read(path).map_err(|source| V2RegistryError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let registry = serde_json::from_slice::<V2ReleaseRegistry>(&bytes).map_err(|source| {
        V2RegistryError::Parse {
            path: path.to_path_buf(),
            source,
        }
    })?;
    validate(path, &registry)?;
    Ok(registry)
}

pub fn versions(path: &Path) -> Result<BTreeSet<String>, V2RegistryError> {
    Ok(read(path)?
        .releases
        .into_iter()
        .map(|entry| entry.version)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(version: &str, digest: char) -> VerifiedV2Release {
        VerifiedV2Release {
            version: version.to_owned(),
            logical_target: format!("software/journal/{version}/release-record.json"),
            record_sha256: digest.to_string().repeat(64),
        }
    }

    #[test]
    fn empty_and_populated_registries_are_read_only_inputs() {
        let empty = Path::new(env!("CARGO_MANIFEST_DIR")).join("v2-release-registry.json");
        assert!(versions(&empty).unwrap().is_empty());
        let dir = tempfile::tempdir().unwrap();
        let populated = dir.path().join("registry.json");
        fs::write(
            &populated,
            format!(
                "{{\"schema\":\"{REGISTRY_SCHEMA}\",\"releases\":[{}]}}",
                serde_json::to_string(&entry("2.0.0", 'a')).unwrap()
            ),
        )
        .unwrap();
        assert_eq!(
            versions(&populated).unwrap(),
            BTreeSet::from(["2.0.0".to_owned()])
        );
    }

    #[test]
    fn malformed_unknown_duplicate_and_conflicting_entries_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        for body in [
            "not json".to_owned(),
            format!(
                "{{\"schema\":\"{REGISTRY_SCHEMA}\",\"releases\":[{{\"version\":\"../2\",\"logical_target\":\"software/journal/../2/release-record.json\",\"record_sha256\":\"{}\"}}]}}",
                "a".repeat(64)
            ),
            format!(
                "{{\"schema\":\"{REGISTRY_SCHEMA}\",\"releases\":[{},{}]}}",
                serde_json::to_string(&entry("2.0.0", 'a')).unwrap(),
                serde_json::to_string(&entry("2.0.0", 'a')).unwrap()
            ),
        ] {
            fs::write(&path, body).unwrap();
            assert!(read(&path).is_err());
        }
    }
}
