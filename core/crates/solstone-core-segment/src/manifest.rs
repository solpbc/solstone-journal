// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use solstone_core_journal_io::{MalformedPolicy, ReadError, path_lexists, read_json};

use crate::{ContentName, SegmentDir, SegmentError, is_reserved_name};

const INGEST_MANIFEST_SCHEMA_VERSION: u64 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManifestEntry {
    pub(crate) sha256: String,
    pub(crate) size: u64,
}

pub(crate) enum ManifestRead {
    Missing,
    Present(BTreeMap<ContentName, ManifestEntry>),
}

pub(crate) fn read_manifest(segment: &SegmentDir) -> Result<ManifestRead, SegmentError> {
    let path = segment.path.join("ingest.json");
    if !path_lexists(&path)? {
        return Ok(ManifestRead::Missing);
    }
    let raw = match read_json(&path, Value::Null, MalformedPolicy::Raise) {
        Ok(raw) => raw,
        Err(ReadError::Malformed(_)) => {
            return Err(SegmentError::MalformedManifest {
                path,
                message: "not valid JSON",
            });
        }
        Err(error) => return Err(SegmentError::Read(error)),
    };
    parse_manifest(path, raw).map(ManifestRead::Present)
}

fn parse_manifest(
    path: PathBuf,
    raw: Value,
) -> Result<BTreeMap<ContentName, ManifestEntry>, SegmentError> {
    let object = raw.as_object().ok_or(SegmentError::MalformedManifest {
        path: path.clone(),
        message: "root must be an object",
    })?;
    let schema_version = object.get("schema_version").and_then(Value::as_u64);
    if schema_version != Some(INGEST_MANIFEST_SCHEMA_VERSION) {
        return Err(SegmentError::UnsupportedManifestSchema {
            path,
            version: schema_version,
        });
    }
    let files =
        object
            .get("files")
            .and_then(Value::as_object)
            .ok_or(SegmentError::MalformedManifest {
                path: path.clone(),
                message: "files must be an object",
            })?;

    let mut parsed = BTreeMap::new();
    for (name, raw_entry) in files {
        if is_reserved_name(name) {
            continue;
        }
        let name = ContentName::new(name).map_err(|_| SegmentError::MalformedManifest {
            path: path.clone(),
            message: "content names must be plain in-segment names",
        })?;
        let entry = raw_entry
            .as_object()
            .ok_or(SegmentError::MalformedManifest {
                path: path.clone(),
                message: "content entries must be objects",
            })?;
        let sha256 =
            entry
                .get("sha256")
                .and_then(Value::as_str)
                .ok_or(SegmentError::MalformedManifest {
                    path: path.clone(),
                    message: "content entries need string sha256",
                })?;
        let size =
            entry
                .get("size")
                .and_then(Value::as_u64)
                .ok_or(SegmentError::MalformedManifest {
                    path: path.clone(),
                    message: "content entries need integer size",
                })?;
        parsed.insert(
            name,
            ManifestEntry {
                sha256: sha256.to_owned(),
                size,
            },
        );
    }
    Ok(parsed)
}
