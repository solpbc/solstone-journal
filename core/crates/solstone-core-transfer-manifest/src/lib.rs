// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Version-one transfer manifest parsing and validation.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

pub const MANIFEST_NAME: &str = "manifest.json";
pub const MANIFEST_VERSION: u64 = 1;

/// A v1 manifest validation error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    InvalidDay,
    Invalid(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDay => formatter.write_str("day must be YYYYMMDD"),
            Self::Invalid(detail) => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for ManifestError {}

/// The v1 archive manifest emitted by Python and native transfer.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TransferManifest {
    pub version: u64,
    pub day: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub segments: BTreeMap<String, SegmentManifest>,
}

/// Manifest contents for one stream/key segment.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SegmentManifest {
    pub files: Vec<ManifestFile>,
}

/// Integrity metadata for one archived file.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ManifestFile {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

/// Validated stream and segment-key route.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SegmentRoute {
    pub stream: String,
    pub key: String,
}

impl SegmentRoute {
    pub fn parse(value: &str) -> Result<Self, ManifestError> {
        let mut parts = value.split('/');
        let (Some(stream), Some(key), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(ManifestError::Invalid(format!(
                "segment key {value:?} must be stream/segment-key"
            )));
        };
        if stream.is_empty() || key.is_empty() {
            return Err(ManifestError::Invalid(format!(
                "segment key {value:?} contains an empty component"
            )));
        }
        validate_route_component(stream)?;
        validate_segment_key(key)?;
        Ok(Self {
            stream: stream.to_owned(),
            key: key.to_owned(),
        })
    }

    pub fn archive_key(&self) -> String {
        format!("{}/{}", self.stream, self.key)
    }
}

/// A validated expected archive member.
#[derive(Debug, Clone)]
pub struct ExpectedMember {
    pub route: SegmentRoute,
    pub file: ManifestFile,
}

/// Decode JSON and validate every v1 control-data field before any path use.
pub fn parse_manifest(bytes: &[u8]) -> Result<TransferManifest, ManifestError> {
    let manifest: TransferManifest =
        serde_json::from_slice(bytes).map_err(|error| ManifestError::Invalid(error.to_string()))?;
    if manifest.version != MANIFEST_VERSION {
        return Err(ManifestError::Invalid(format!(
            "version must be integer {MANIFEST_VERSION}"
        )));
    }
    if !is_day(&manifest.day) {
        return Err(ManifestError::InvalidDay);
    }
    for (route_value, segment) in &manifest.segments {
        let route = SegmentRoute::parse(route_value)?;
        let mut names = BTreeSet::new();
        for file in &segment.files {
            if !names.insert(&file.name) {
                return Err(ManifestError::Invalid(format!(
                    "duplicate file {} in {}",
                    file.name,
                    route.archive_key()
                )));
            }
            validate_relative_file_name(&file.name)?;
            validate_sha256(&file.sha256)?;
        }
    }
    Ok(manifest)
}

/// Build the complete expected tar-member map from an already parsed manifest.
pub fn expected_members(
    manifest: &TransferManifest,
) -> Result<BTreeMap<String, ExpectedMember>, ManifestError> {
    let mut expected = BTreeMap::new();
    for (route_value, segment) in &manifest.segments {
        let route = SegmentRoute::parse(route_value)?;
        let route_value = route.archive_key();
        for file in &segment.files {
            let member_name = format!("{route_value}/{}", file.name);
            if expected
                .insert(
                    member_name.clone(),
                    ExpectedMember {
                        route: route.clone(),
                        file: file.clone(),
                    },
                )
                .is_some()
            {
                return Err(ManifestError::Invalid(format!(
                    "duplicate archive member {member_name}"
                )));
            }
        }
    }
    Ok(expected)
}

pub fn is_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_segment_key(value: &str) -> Result<(), ManifestError> {
    let Some((time, length)) = value.split_once('_') else {
        return Err(ManifestError::Invalid(format!(
            "invalid segment key {value:?}"
        )));
    };
    if time.len() != 6
        || !time.bytes().all(|byte| byte.is_ascii_digit())
        || length.is_empty()
        || !length.bytes().all(|byte| byte.is_ascii_digit())
        || length
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .is_none()
    {
        return Err(ManifestError::Invalid(format!(
            "invalid segment key {value:?}"
        )));
    }
    Ok(())
}

fn validate_relative_file_name(value: &str) -> Result<(), ManifestError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\\')
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err(ManifestError::Invalid(format!(
            "unsafe file name {value:?}"
        )));
    }
    Ok(())
}

fn validate_route_component(value: &str) -> Result<(), ManifestError> {
    let path = Path::new(value);
    if value.starts_with('\\')
        || value.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ManifestError::Invalid(format!(
            "unsafe stream component {value:?}"
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), ManifestError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ManifestError::Invalid(format!("invalid sha256 {value:?}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_validates_v1_route_and_file_fields() {
        let manifest = br#"{"version":1,"day":"20260203","segments":{"audio/120000_30":{"files":[{"name":"stream.json","sha256":"dca83e717b1f64eb141057a7415a330ad1361f51703efa2e4776f40047898a04","size":6}]}}}"#;
        let parsed = parse_manifest(manifest).unwrap();
        let members = expected_members(&parsed).unwrap();
        assert_eq!(members.len(), 1);
        assert!(members.contains_key("audio/120000_30/stream.json"));
    }
}
