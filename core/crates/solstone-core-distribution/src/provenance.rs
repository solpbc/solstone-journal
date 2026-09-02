// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::digest::sha256_hex;
use crate::select::ArtifactId;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
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

/// Bind cargo's JSON output to inventory artifact ids.
///
/// `expected_triple` is the triple the lane asked cargo to build. An artifact
/// is stamped with it only when that exact directory component appears in the
/// artifact path — cargo writes a cross build to `target/<triple>/release/` and
/// a host build to `target/release/`, so an unstamped id is precisely the
/// host-built artifact `refuse_wrong_triple` exists to reject.
///
/// ⛔ This used to key on the substring `-unknown-linux-`, which is a fact
/// about one platform's triples rather than about cargo's layout: every
/// `aarch64-apple-darwin` artifact came back with an EMPTY triple and the whole
/// admitted set was refused as wrong-triple. Matching the expected triple
/// exactly is both platform-neutral and strictly stronger than the substring.
pub fn bind_cargo_json(
    lines: &str,
    expected_triple: &str,
) -> Result<BTreeMap<ArtifactId, PathBuf>, ProvenanceError> {
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
            .map(|id| package_from_id(id, &bin))
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
                    (!expected_triple.is_empty() && name == expected_triple)
                        .then(|| name.into_owned())
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

pub fn bind_ffmpeg_build_script_out_dirs(lines: &str) -> Vec<PathBuf> {
    #[derive(serde::Deserialize)]
    struct Message {
        reason: Option<String>,
        package_id: Option<String>,
        out_dir: Option<String>,
    }

    lines
        .lines()
        .filter_map(|line| serde_json::from_str::<Message>(line).ok())
        .filter(|message| message.reason.as_deref() == Some("build-script-executed"))
        .filter(|message| {
            message
                .package_id
                .as_deref()
                .is_some_and(|id| package_from_id(id, "ffmpeg-sys-next") == "ffmpeg-sys-next")
        })
        .filter_map(|message| message.out_dir.map(PathBuf::from))
        .collect()
}

fn package_from_id(id: &str, bin: &str) -> String {
    if let Some((name, rest)) = id.split_once(' ')
        && !name.contains(['/', '\\', '+', '#'])
        && rest
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_digit() || ch == '(')
    {
        return name.to_owned();
    }
    if let Some((path, after_hash)) = id.rsplit_once('#') {
        let hashed = after_hash.split('@').next().unwrap_or(after_hash);
        if !hashed.is_empty() && hashed.chars().next().is_some_and(|ch| !ch.is_ascii_digit()) {
            return hashed.to_owned();
        }
        if let Some(name) = path.rsplit('/').next()
            && !name.is_empty()
        {
            return name.to_owned();
        }
    }
    bin.to_owned()
}
