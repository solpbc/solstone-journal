// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_journal_io::{
    DetailedAtomicOutcome, JsonWriteOptions, LockOptions, MalformedPolicy, atomic_replace_detailed,
    hold_lock, read_json, write_json,
};
use solstone_core_segment::SegmentDir;

use crate::apply::{AppliedDisposition, AppliedFile, ApplyError};
use crate::held::{manifest_fields, read_lenient_manifest};

const ADVANCE_PENDING: &str = "stream_advance_pending";

/// Record first-admission intent before raw writes. Existing manifests without
/// this field never acquire it; an empty pre-write directory can be retried.
pub(crate) fn prepare_stream_advance(
    segment: &SegmentDir,
    requested_segment: &str,
    fresh: bool,
    writes_bytes: bool,
) -> Result<bool, ApplyError> {
    let path = segment.path().join("ingest.json");
    let _lock = hold_lock(&path, LockOptions::default()).map_err(ApplyError::Lock)?;
    if let Some(value) =
        read_json::<Option<Value>>(&path, None, MalformedPolicy::Raise).map_err(ApplyError::Read)?
    {
        let pending = value.get(ADVANCE_PENDING).and_then(Value::as_bool) == Some(true);
        if pending {
            // A prior attempt may have renamed this proof but failed its sync.
            publish_admission_proof(&path, &value)?;
        }
        return Ok(pending);
    }
    let entries = fs::read_dir(segment.path()).map_err(|source| ApplyError::Io {
        path: segment.path().to_owned(),
        source,
    })?;
    let mut ambiguous = false;
    for entry in entries {
        let entry = entry.map_err(|source| ApplyError::Io {
            path: segment.path().to_owned(),
            source,
        })?;
        if entry.file_name() == "ingest.json.lock" {
            continue;
        }
        // A freshly resolved target may contain directories that block a later
        // file write. They carry no accepted raw bytes or prior chain marker.
        if !fresh
            || !entry
                .file_type()
                .map_err(|source| ApplyError::Io {
                    path: entry.path(),
                    source,
                })?
                .is_dir()
        {
            ambiguous = true;
        }
    }
    if ambiguous {
        if writes_bytes && !segment.path().join("stream.json").is_file() {
            if fresh {
                return Err(ApplyError::Stale);
            }
            return Err(ApplyError::Io {
                path,
                source: io::Error::other("unmarked nonempty segment has no ingest admission proof"),
            });
        }
        return Ok(false);
    }
    publish_admission_proof(
        &path,
        &serde_json::json!({
            "schema_version": 1, "requested_segment": requested_segment,
            "files": {}, "stream_advance_pending": true,
        }),
    )?;
    Ok(true)
}

fn publish_admission_proof(path: &Path, value: &Value) -> Result<(), ApplyError> {
    let error = |source| ApplyError::Io {
        path: path.to_owned(),
        source,
    };
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|source| error(io::Error::other(source)))?;
    match atomic_replace_detailed(path, &bytes, 0o600)
        .map_err(|source| error(io::Error::other(source)))?
    {
        DetailedAtomicOutcome::Published => Ok(()),
        outcome => Err(error(io::Error::other(format!(
            "ingest admission proof publication was not confirmed: {outcome:?}"
        )))),
    }
}

/// Clear admission intent only after the stream owner has published its marker.
pub fn complete_stream_advance(segment: &SegmentDir) -> Result<(), ApplyError> {
    let path = segment.path().join("ingest.json");
    let _lock = hold_lock(&path, LockOptions::default()).map_err(ApplyError::Lock)?;
    let Some(mut value) = read_json::<Option<Value>>(&path, None, MalformedPolicy::Raise)
        .map_err(ApplyError::Read)?
    else {
        return Ok(());
    };
    if value.get(ADVANCE_PENDING).and_then(Value::as_bool) != Some(true) {
        return Ok(());
    }
    value[ADVANCE_PENDING] = Value::Bool(false);
    write_json(
        &path,
        &value,
        JsonWriteOptions {
            sort_keys: true,
            ..JsonWriteOptions::default()
        },
    )
    .map_err(ApplyError::Atomic)
}

/// Merge applied ingest facts into one durable, deterministic ingest manifest.
pub fn write_ingest_manifest(
    segment: &SegmentDir,
    requested_segment: &str,
    files: &[AppliedFile],
) -> Result<(), ApplyError> {
    let path = segment.path().join("ingest.json");
    let _lock = hold_lock(&path, LockOptions::default()).map_err(ApplyError::Lock)?;
    let prior: Option<Value> =
        read_json(&path, None, MalformedPolicy::Raise).map_err(ApplyError::Read)?;
    let mut merged: BTreeMap<String, Map<String, Value>> = read_lenient_manifest(segment.path())
        .into_iter()
        .map(|(name, entry)| (name, manifest_fields(entry)))
        .collect();
    for file in files {
        if matches!(file.disposition, AppliedDisposition::Unwritten) {
            continue;
        }
        merged.insert(
            file.name.as_str().to_owned(),
            Map::from_iter([
                ("sha256".to_owned(), Value::String(file.sha256.clone())),
                ("size".to_owned(), Value::from(file.size)),
            ]),
        );
    }
    let mut value = serde_json::json!({
        "schema_version": 1,
        "requested_segment": requested_segment,
        "files": merged,
    });
    if let Some(pending) = prior.as_ref().and_then(|value| value.get(ADVANCE_PENDING)) {
        value[ADVANCE_PENDING] = pending.clone();
    }
    write_json(
        path,
        &value,
        JsonWriteOptions {
            sort_keys: true,
            ..JsonWriteOptions::default()
        },
    )
    .map_err(ApplyError::Atomic)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;

    use solstone_core_segment::{ContentName, SegmentDir};

    use crate::{AppliedDisposition, AppliedFile};

    use super::write_ingest_manifest;

    #[test]
    fn merge_preserves_other_entries_and_skips_unwritten_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let segment = SegmentDir::resolve(root, "20260804", "120000_1", "device").unwrap();
        fs::create_dir_all(segment.path()).unwrap();
        fs::write(
            segment.path().join("ingest.json"),
            r#"{"schema_version":1,"requested_segment":"old","files":{"older.json":{"sha256":"old","size":3}}}"#,
        )
        .unwrap();
        write_ingest_manifest(
            &segment,
            "120000_1",
            &[
                AppliedFile {
                    name: ContentName::new("audio.flac").unwrap(),
                    sha256: "new".to_owned(),
                    size: 5,
                    disposition: AppliedDisposition::Written,
                },
                AppliedFile {
                    name: ContentName::new("notes.json").unwrap(),
                    sha256: "ignored".to_owned(),
                    size: 7,
                    disposition: AppliedDisposition::Unwritten,
                },
            ],
        )
        .unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(segment.path().join("ingest.json")).unwrap()).unwrap();
        assert_eq!(manifest["requested_segment"], "120000_1");
        assert_eq!(manifest["files"]["older.json"]["sha256"], "old");
        assert_eq!(manifest["files"]["audio.flac"]["size"], 5);
        assert!(manifest["files"].get("notes.json").is_none());
    }
}
