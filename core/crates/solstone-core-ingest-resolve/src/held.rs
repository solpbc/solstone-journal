// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_segment::{ContentName, SegmentDir, TerminalProofVerifier};

use crate::IngestFile;
use crate::terminal_proof::SegmentTerminalProof;

#[derive(Clone, Debug)]
pub(crate) struct ResolveManifestEntry {
    fields: Map<String, Value>,
}

/// Read the exploratory ingest manifest with Python's intentionally lenient
/// semantics. A malformed manifest is indistinguishable from no manifest.
pub(crate) fn read_lenient_manifest(segment_path: &Path) -> BTreeMap<String, ResolveManifestEntry> {
    let path = segment_path.join("ingest.json");
    let Ok(bytes) = fs::read(path) else {
        return BTreeMap::new();
    };
    let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(&bytes) else {
        return BTreeMap::new();
    };
    if root.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return BTreeMap::new();
    }
    let Some(files) = root.get("files").and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    if !files.values().all(Value::is_object) {
        return BTreeMap::new();
    }
    files
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                ResolveManifestEntry {
                    fields: value
                        .as_object()
                        .expect("entries were checked as objects")
                        .clone(),
                },
            )
        })
        .collect()
}

pub(crate) fn manifest_entry_matches(
    entry: &ResolveManifestEntry,
    sha256: &str,
    size: u64,
) -> bool {
    entry.fields.get("sha256").and_then(Value::as_str) == Some(sha256)
        && entry.fields.get("size").and_then(Value::as_u64) == Some(size)
}

pub(crate) fn manifest_fields(entry: ResolveManifestEntry) -> Map<String, Value> {
    entry.fields
}

/// Test an absent target against the same manifest-consistent terminal-proof
/// rule used by resolution and apply-time held-file revalidation.
pub(crate) fn absent_target_held(
    segment: &SegmentDir,
    name: &ContentName,
    sha256: &str,
    size: u64,
    manifest: &BTreeMap<String, ResolveManifestEntry>,
) -> bool {
    manifest
        .get(name.as_str())
        .is_none_or(|entry| manifest_entry_matches(entry, sha256, size))
        && SegmentTerminalProof::new(segment).has_terminal_proof(name, size)
}

/// Re-check one resolve-time held file immediately before the apply commit.
///
/// An existing target with mismatched bytes is deliberately treated as drift
/// without consulting terminal proof. Python falls through to that proof in
/// this narrow case; this new layer is intentionally more conservative, so
/// unexpected on-disk bytes trigger bounded honest re-resolution instead of
/// being silently accepted.
pub(crate) fn is_currently_held(
    segment: &SegmentDir,
    file: &IngestFile<'_>,
) -> Result<bool, io::Error> {
    let target = segment.path().join(file.name.as_str());
    match fs::read(&target) {
        Ok(bytes) => Ok(sha256(&bytes) == sha256(file.bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let manifest = read_lenient_manifest(segment.path());
            Ok(absent_target_held(
                segment,
                &file.name,
                &sha256(file.bytes),
                file.bytes.len() as u64,
                &manifest,
            ))
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    format!("{:x}", Sha256::digest(bytes))
}
