// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::digest::sha256_hex;
use crate::select::ArtifactId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub commit: String,
    pub lock_sha256: String,
}

#[derive(Debug)]
pub struct ProvenanceError {
    pub message: String,
}

impl ProvenanceError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProvenanceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProvenanceError {}

pub fn lock_digest(path: &Path) -> Result<String, ProvenanceError> {
    let bytes = fs::read(path).map_err(|error| ProvenanceError::new(error.to_string()))?;
    Ok(sha256_hex(&bytes))
}

pub fn require_clean(dirty: bool) -> Result<(), ProvenanceError> {
    if dirty {
        return Err(ProvenanceError::new("unexpected:\n  dirty-tree"));
    }
    Ok(())
}

pub fn require_commit(expected: &str, actual: &str) -> Result<(), ProvenanceError> {
    if expected != actual {
        return Err(ProvenanceError::new(format!(
            "unexpected:\n  mismatched-commit {actual}"
        )));
    }
    Ok(())
}

pub fn require_lock(expected: &str, actual: &str) -> Result<(), ProvenanceError> {
    if expected != actual {
        return Err(ProvenanceError::new(format!(
            "unexpected:\n  stale-lock {actual}"
        )));
    }
    Ok(())
}

pub fn bind_cargo_json(lines: &str) -> Result<BTreeMap<ArtifactId, PathBuf>, ProvenanceError> {
    #[derive(serde::Deserialize)]
    struct Message {
        reason: Option<String>,
        package_id: Option<String>,
        target: Option<Target>,
        filenames: Option<Vec<String>>,
    }
    #[derive(serde::Deserialize)]
    struct Target {
        name: Option<String>,
        kind: Option<Vec<String>>,
    }
    let mut artifacts = BTreeMap::new();
    for line in lines.lines() {
        let Ok(message) = serde_json::from_str::<Message>(line) else {
            continue;
        };
        if message.reason.as_deref() != Some("compiler-artifact") {
            continue;
        }
        let Some(target) = message.target else {
            continue;
        };
        if !target
            .kind
            .as_ref()
            .is_some_and(|kind| kind.iter().any(|item| item == "bin"))
        {
            continue;
        }
        let Some(bin) = target.name else {
            continue;
        };
        let package = message
            .package_id
            .as_deref()
            .and_then(|id| id.split_once(' '))
            .map(|(name, _)| name.to_owned())
            .unwrap_or_else(|| bin.clone());
        let Some(filenames) = message.filenames else {
            continue;
        };
        for filename in filenames {
            let path = PathBuf::from(filename);
            let triple = path
                .components()
                .find_map(|component| {
                    let name = component.as_os_str().to_string_lossy();
                    name.contains("-unknown-linux-").then(|| name.into_owned())
                })
                .unwrap_or_default();
            artifacts.insert(
                ArtifactId {
                    package: package.clone(),
                    bin: bin.clone(),
                    triple,
                },
                path,
            );
        }
    }
    Ok(artifacts)
}
