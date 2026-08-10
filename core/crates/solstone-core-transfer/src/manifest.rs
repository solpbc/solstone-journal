// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Version-one transfer manifest parsing and validation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::contained_path;

use crate::TransferError;

pub const MANIFEST_NAME: &str = "manifest.json";
pub const MANIFEST_VERSION: u64 = 1;

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

/// A validated expected archive member.
#[derive(Debug, Clone)]
pub(crate) struct ExpectedMember {
    pub route: SegmentRoute,
    pub file: ManifestFile,
}

/// Validated stream and segment-key route.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SegmentRoute {
    pub stream: String,
    pub key: String,
}

impl SegmentRoute {
    pub fn parse(value: &str) -> Result<Self, TransferError> {
        let mut parts = value.split('/');
        let (Some(stream), Some(key), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(TransferError::Manifest(format!(
                "segment key {value:?} must be stream/segment-key"
            )));
        };
        if stream.is_empty() || key.is_empty() {
            return Err(TransferError::Manifest(format!(
                "segment key {value:?} contains an empty component"
            )));
        }
        Ok(Self {
            stream: stream.to_owned(),
            key: key.to_owned(),
        })
    }

    pub fn archive_key(&self) -> String {
        format!("{}/{}", self.stream, self.key)
    }
}

/// Decode JSON and validate all shape fields required by v1.
pub(crate) fn parse_manifest(bytes: &[u8]) -> Result<TransferManifest, TransferError> {
    let manifest: TransferManifest = serde_json::from_slice(bytes)
        .map_err(|error| TransferError::Manifest(error.to_string()))?;
    if manifest.version != MANIFEST_VERSION {
        return Err(TransferError::Manifest(format!(
            "version must be integer {MANIFEST_VERSION}"
        )));
    }
    if !is_day(&manifest.day) {
        return Err(TransferError::InvalidDay);
    }
    for (route, segment) in &manifest.segments {
        let route = SegmentRoute::parse(route)?;
        let mut names = std::collections::BTreeSet::new();
        for file in &segment.files {
            if !names.insert(&file.name) {
                return Err(TransferError::Manifest(format!(
                    "duplicate file {} in {}",
                    file.name,
                    route.archive_key()
                )));
            }
            validate_sha256(&file.sha256)?;
        }
    }
    Ok(manifest)
}

/// Build an expected tar-member map after validating containment below `day`.
pub(crate) fn expected_members(
    manifest: &TransferManifest,
    day_directory: &std::path::Path,
) -> Result<BTreeMap<String, ExpectedMember>, TransferError> {
    let mut expected = BTreeMap::new();
    for (route_value, segment) in &manifest.segments {
        let route = SegmentRoute::parse(route_value)?;
        let route_value = route.archive_key();
        let segment_directory = contained_path(day_directory, &route_value)?;
        validate_segment_key(&route.key)?;
        for file in &segment.files {
            // Validate every archive-controlled route before extraction or any
            // target probing. The map below is subsequently the only member
            // name authority used while streaming tar entries.
            contained_path(&segment_directory, &file.name)?;
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
                return Err(TransferError::Manifest(format!(
                    "duplicate archive member {member_name}"
                )));
            }
        }
    }
    Ok(expected)
}

pub(crate) fn is_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_segment_key(value: &str) -> Result<(), TransferError> {
    let Some((time, length)) = value.split_once('_') else {
        return Err(TransferError::Manifest(format!(
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
        return Err(TransferError::Manifest(format!(
            "invalid segment key {value:?}"
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), TransferError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(TransferError::Manifest(format!("invalid sha256 {value:?}")));
    }
    Ok(())
}
