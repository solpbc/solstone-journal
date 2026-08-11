// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use solstone_core_processing_record::{is_failure_exhausted, vocab};

use crate::DataState;

const ANALYZING_STALE_SECONDS: i64 = 1_800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerVerdict {
    ChunksWin,
    None,
    Corrupt,
    Stale,
    Active,
}

pub fn derive_modality_state(
    segment_path: &Path,
    modality: &str,
    has_chunks: bool,
    has_jsonl: bool,
    has_raw: bool,
    record: Option<&Value>,
    now: DateTime<Utc>,
) -> DataState {
    let marker_path = segment_path.join(format!(".analyzing_{modality}"));
    let failed_path = segment_path.join(format!(".analyze_failed_{modality}"));
    let marker = classify_marker(&marker_path, has_chunks, now);

    if record.and_then(|record| record.get("state").and_then(Value::as_str))
        == Some(vocab::STATE_FAILED)
    {
        return if record.is_some_and(is_failure_exhausted) {
            DataState::FailedFinal
        } else {
            DataState::Failed
        };
    }
    if marker == MarkerVerdict::ChunksWin {
        return DataState::Analyzed;
    }
    if record.and_then(|record| record.get("state").and_then(Value::as_str))
        == Some(vocab::STATE_EMPTY)
    {
        return DataState::Empty;
    }
    if matches!(marker, MarkerVerdict::Corrupt | MarkerVerdict::Stale) {
        return DataState::Failed;
    }
    if marker == MarkerVerdict::Active {
        return DataState::Analyzing;
    }
    if failed_path.is_file() {
        return DataState::Failed;
    }
    if has_jsonl || has_raw {
        return DataState::Pending;
    }
    DataState::Absent
}

fn classify_marker(marker_path: &Path, has_chunks: bool, now: DateTime<Utc>) -> MarkerVerdict {
    if has_chunks {
        return MarkerVerdict::ChunksWin;
    }
    if !marker_path.is_file() {
        return MarkerVerdict::None;
    }
    let Ok(contents) = fs::read_to_string(marker_path) else {
        return MarkerVerdict::Corrupt;
    };
    let Ok(Value::Object(_)) = serde_json::from_str::<Value>(&contents) else {
        return MarkerVerdict::Corrupt;
    };
    marker_verdict_from_modified(
        fs::metadata(marker_path).and_then(|metadata| metadata.modified()),
        now,
    )
}

fn marker_verdict_from_modified(
    modified: Result<SystemTime, io::Error>,
    now: DateTime<Utc>,
) -> MarkerVerdict {
    let age = modified
        .map(DateTime::<Utc>::from)
        .map(|modified| now.signed_duration_since(modified))
        .unwrap_or_else(|_| Duration::zero());
    if age > Duration::seconds(ANALYZING_STALE_SECONDS) {
        MarkerVerdict::Stale
    } else {
        MarkerVerdict::Active
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use chrono::Utc;

    use super::{MarkerVerdict, marker_verdict_from_modified};

    #[test]
    fn unreadable_marker_metadata_keeps_a_valid_marker_active() {
        assert_eq!(
            marker_verdict_from_modified(Err(io::Error::other("metadata unavailable")), Utc::now(),),
            MarkerVerdict::Active
        );
    }
}
