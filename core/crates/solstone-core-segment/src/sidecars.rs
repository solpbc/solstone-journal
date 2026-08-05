// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Serialize;
use solstone_core_journal_io::append_jsonl;

use crate::{SegmentDir, SegmentError};

/// Append one durable event record to a segment's journal-authored event log.
pub fn append_event<T: Serialize>(segment: &SegmentDir, record: &T) -> Result<(), SegmentError> {
    append_jsonl(segment.path.join("events.jsonl"), record)?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;

    use crate::test_support::TempDir;

    use super::*;

    #[test]
    fn append_failure_propagates() {
        let temporary = TempDir::new();
        let segment =
            SegmentDir::resolve(temporary.path(), "20260804", "120000_60", "workstation").unwrap();
        let blocked = temporary.path().join("chronicle/20260804/workstation");
        fs::create_dir_all(blocked.parent().unwrap()).unwrap();
        fs::write(&blocked, b"not a directory").unwrap();
        assert!(append_event(&segment, &serde_json::json!({"event": "x"})).is_err());
    }
}
