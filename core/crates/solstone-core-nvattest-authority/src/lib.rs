// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared authority document parser for nvattest attestation provider artifacts.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

pub const AUTHORITY_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/nvattest_authority_v1.json"
));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvattestArtifactSpec {
    pub platform: String,
    pub version: String,
    pub name: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub url: String,
    pub origin_key: String,
    pub inventory: Vec<NvattestInventoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestInventoryEntry {
    pub relpath: String,
    pub executable: bool,
    pub kind: String,
    #[serde(default)]
    pub symlink_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityOriginPin {
    pub origin_key: String,
    pub sha256: String,
    pub version: String,
    pub size_bytes: Option<u64>,
    pub upstream_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityParseError {
    Malformed(String),
    PlatformUnsupported(String),
}

impl fmt::Display for AuthorityParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(detail) => write!(f, "malformed nvattest authority: {detail}"),
            Self::PlatformUnsupported(platform) => {
                write!(f, "unsupported nvattest platform: {platform}")
            }
        }
    }
}

impl std::error::Error for AuthorityParseError {}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    pub schema_version: u32,
    pub targets: BTreeMap<String, Target>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub artifact: ArtifactObject,
    pub companion_manifest: CompanionManifestObject,
    pub inventory: Vec<NvattestInventoryEntry>,
    pub source: SourceObject,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactObject {
    pub name: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanionManifestObject {
    pub name: String,
    pub sha256: String,
    pub url: String,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceObject {
    pub fork_commit: String,
    pub upstream_base: String,
    pub url_prefix: String,
    pub version: String,
}

pub fn parse(json: &str) -> Result<Authority, AuthorityParseError> {
    let authority: Authority =
        serde_json::from_str(json).map_err(|e| AuthorityParseError::Malformed(e.to_string()))?;
    if authority.schema_version != 1 {
        return Err(AuthorityParseError::Malformed(format!(
            "unexpected schema_version: {}",
            authority.schema_version
        )));
    }
    Ok(authority)
}

pub fn artifact_spec(
    authority: &Authority,
    platform: &str,
) -> Result<NvattestArtifactSpec, AuthorityParseError> {
    let Some(target) = authority.targets.get(platform) else {
        return Err(AuthorityParseError::PlatformUnsupported(
            platform.to_owned(),
        ));
    };
    let basename = target
        .artifact
        .url
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            AuthorityParseError::Malformed(format!("{platform}: artifact URL has no file name"))
        })?;
    if basename != target.artifact.name {
        return Err(AuthorityParseError::Malformed(format!(
            "{platform}: artifact basename disagrees with name"
        )));
    }
    Ok(NvattestArtifactSpec {
        platform: platform.to_owned(),
        version: target.source.version.clone(),
        name: target.artifact.name.clone(),
        sha256: target.artifact.sha256.clone(),
        size_bytes: target.artifact.size_bytes,
        url: target.artifact.url.clone(),
        origin_key: format!("providers/nvattest/{}", target.artifact.name),
        inventory: target.inventory.clone(),
    })
}

pub fn origin_pins(authority: &Authority) -> Result<Vec<AuthorityOriginPin>, AuthorityParseError> {
    let mut pins = Vec::new();
    for (platform, target) in &authority.targets {
        let prefix = &target.source.url_prefix;
        let version = &target.source.version;

        // 1. artifact
        let art_suffix = target.artifact.url.strip_prefix(prefix).ok_or_else(|| {
            AuthorityParseError::Malformed(format!(
                "{platform}: artifact URL is outside source.url_prefix"
            ))
        })?;
        if art_suffix != target.artifact.name {
            return Err(AuthorityParseError::Malformed(format!(
                "{platform}: artifact URL basename disagrees with name"
            )));
        }
        pins.push(AuthorityOriginPin {
            origin_key: art_suffix.to_owned(),
            sha256: target.artifact.sha256.clone(),
            version: version.clone(),
            size_bytes: Some(target.artifact.size_bytes),
            upstream_url: target.artifact.url.clone(),
        });

        // 2. companion_manifest
        let companion = &target.companion_manifest;
        let comp_suffix = companion.url.strip_prefix(prefix).ok_or_else(|| {
            AuthorityParseError::Malformed(format!(
                "{platform}: companion_manifest URL is outside source.url_prefix"
            ))
        })?;
        if comp_suffix != companion.name {
            return Err(AuthorityParseError::Malformed(format!(
                "{platform}: companion_manifest URL basename disagrees with name"
            )));
        }
        pins.push(AuthorityOriginPin {
            origin_key: comp_suffix.to_owned(),
            sha256: companion.sha256.clone(),
            version: version.clone(),
            size_bytes: companion.size_bytes,
            upstream_url: companion.url.clone(),
        });
    }
    Ok(pins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_committed_authority() {
        let auth = parse(AUTHORITY_JSON).expect("valid committed authority");
        assert_eq!(auth.schema_version, 1);
        assert!(auth.targets.contains_key("linux-x86_64"));
        assert!(auth.targets.contains_key("linux-aarch64"));
        assert!(auth.targets.contains_key("macos-arm64"));

        let spec = artifact_spec(&auth, "linux-x86_64").expect("spec for linux-x86_64");
        assert_eq!(spec.name, "libnvat-linux-x86_64-1.2.2-sol.6-archive.tar.xz");
        assert_eq!(
            spec.origin_key,
            "providers/nvattest/libnvat-linux-x86_64-1.2.2-sol.6-archive.tar.xz"
        );
        assert_eq!(spec.size_bytes, 7793216);
        assert!(!spec.inventory.is_empty());

        let pins = origin_pins(&auth).expect("origin pins");
        assert_eq!(pins.len(), 6); // 3 targets * 2 objects
    }

    #[test]
    fn parse_unsupported_platform() {
        let auth = parse(AUTHORITY_JSON).unwrap();
        let err = artifact_spec(&auth, "windows-x86_64").unwrap_err();
        assert!(matches!(err, AuthorityParseError::PlatformUnsupported(_)));
    }
}
