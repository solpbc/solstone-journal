// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use solstone_core_journal_io::{JsonWriteOptions, LockOptions, hold_lock, write_json};
use solstone_core_segment::SegmentDir;

use crate::apply::{AppliedDisposition, AppliedFile, ApplyError};
use crate::held::{manifest_fields, read_lenient_manifest};

/// Merge applied ingest facts into one durable, deterministic ingest manifest.
pub fn write_ingest_manifest(
    segment: &SegmentDir,
    requested_segment: &str,
    files: &[AppliedFile],
) -> Result<(), ApplyError> {
    let path = segment.path().join("ingest.json");
    let _lock = hold_lock(&path, LockOptions::default()).map_err(ApplyError::Lock)?;
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
    let value = serde_json::json!({
        "schema_version": 1,
        "requested_segment": requested_segment,
        "files": merged,
    });
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use solstone_core_segment::{ContentName, SegmentDir};

    use crate::{AppliedDisposition, AppliedFile};

    use super::write_ingest_manifest;

    #[test]
    fn merge_preserves_other_entries_and_skips_unwritten_files() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("solstone-core-manifest-{suffix}"));
        let segment = SegmentDir::resolve(&root, "20260804", "120000_1", "device").unwrap();
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
        let _ = fs::remove_dir_all(root);
    }
}
