// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Serialize;
use solstone_core_journal_io::{AtomicWriteOptions, write_bytes_exclusive};

use crate::{SegmentDir, SegmentError};

const TOMBSTONE_REASON: &str = "owner_location_data_delete";

#[derive(Serialize)]
struct Tombstone<'a> {
    deleted_at: &'a str,
    reason: &'static str,
    did: &'a str,
}

/// Create the terminal tombstone sidecar without overwriting an existing one.
///
/// ⚠ **Superseded.** `reason` is hard-coded to a source-scoped delete, and deleting
/// a source is no longer an operation an owner can request — so this would stamp a
/// durable, owner-facing claim that outlived the feature it described. It also
/// cannot serve a staged removal, because `SegmentDir` rejects a leading-dot
/// component. The retention executor owns tombstone contents now; this goes with
/// the retired partial-delete path that is its only caller.
pub fn write_tombstone(
    segment: &SegmentDir,
    deleted_at: &str,
    did: &str,
) -> Result<(), SegmentError> {
    let path = segment.path.join("tombstone.json");
    let bytes = serde_json::to_vec(&Tombstone {
        deleted_at,
        reason: TOMBSTONE_REASON,
        did,
    })
    .map_err(|source| SegmentError::Serialization {
        path: path.clone(),
        source,
    })?;
    write_bytes_exclusive(path, &bytes, AtomicWriteOptions::default())?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;

    use crate::test_support::TempDir;

    use super::*;

    #[test]
    fn writes_once_without_content_metadata() {
        let temporary = TempDir::new();
        let segment =
            SegmentDir::resolve(temporary.path(), "20260804", "120000_60", "location").unwrap();
        write_tombstone(&segment, "2026-08-04T12:00:00Z", "unknown").unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(segment.path.join("tombstone.json")).unwrap())
                .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "deleted_at": "2026-08-04T12:00:00Z",
                "reason": "owner_location_data_delete",
                "did": "unknown",
            })
        );
        assert!(write_tombstone(&segment, "2026-08-04T12:00:01Z", "other").is_err());
    }
}
