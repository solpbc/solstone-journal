// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use chrono::Utc;
use solstone_core_journal_io::{AtomicWriteOptions, day_path, write_bytes_exclusive};

use crate::{ApplyError, FailedPlan, IngestFile};

/// Durable locations of one failed-resolution request's operator-review bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantineReceipt {
    pub timestamp_millis: i64,
    pub paths: Vec<PathBuf>,
}

/// Quarantine an exhausted request outside segment content.
///
/// Repair and support tooling may inspect this operator-review area, but segment
/// readers and processing must never consume it. It is not segment content and
/// deliberately does not use the segment content writer. The validated content
/// name is used as the filename because this new API has no separate submitted
/// versus written name.
pub fn quarantine_failed(
    journal_root: &Path,
    day: &str,
    plan: &FailedPlan,
    files: &[IngestFile<'_>],
) -> Result<QuarantineReceipt, ApplyError> {
    let timestamp_millis = Utc::now().timestamp_millis();
    let base = day_path(journal_root, Some(day), false)
        .map_err(ApplyError::Path)?
        .join("observer")
        .join("failed")
        .join(&plan.requested_segment)
        .join(timestamp_millis.to_string());
    let mut paths = Vec::with_capacity(files.len());
    for file in files {
        let path = base.join(file.name.as_str());
        write_bytes_exclusive(&path, file.bytes, AtomicWriteOptions::default())
            .map_err(ApplyError::Atomic)?;
        paths.push(path);
    }
    Ok(QuarantineReceipt {
        timestamp_millis,
        paths,
    })
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use solstone_core_segment::ContentName;

    use super::*;

    #[test]
    fn failed_bytes_land_only_in_operator_quarantine() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("solstone-core-quarantine-{suffix}"));
        fs::create_dir_all(&root).unwrap();
        let bytes = b"unplaced";
        let files = [IngestFile {
            name: ContentName::new("audio.flac").unwrap(),
            bytes,
        }];
        let receipt = quarantine_failed(
            &root,
            "20260804",
            &FailedPlan {
                requested_segment: "120000_1".to_owned(),
            },
            &files,
        )
        .unwrap();
        assert_eq!(receipt.paths.len(), 1);
        assert_eq!(fs::read(&receipt.paths[0]).unwrap(), bytes);
        assert_eq!(
            receipt.paths[0],
            root.join("chronicle/20260804/observer/failed/120000_1")
                .join(receipt.timestamp_millis.to_string())
                .join("audio.flac")
        );
        let _ = fs::remove_dir_all(root);
    }
}
