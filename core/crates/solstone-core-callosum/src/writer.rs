// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use solstone_core_journal_io::{AppendError, append_jsonl};

use crate::DurableEvent;

const EVENTS_FILE: &str = "events.jsonl";

/// A durable Callosum event-log append failure.
#[derive(Debug)]
pub enum CallosumWriteError {
    SegmentDirectoryMissing(PathBuf),
    Append(AppendError),
}

impl fmt::Display for CallosumWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SegmentDirectoryMissing(path) => {
                write!(
                    formatter,
                    "segment directory is unavailable: {}",
                    path.display()
                )
            }
            Self::Append(error) => error.fmt(formatter),
        }
    }
}

impl Error for CallosumWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SegmentDirectoryMissing(_) => None,
            Self::Append(error) => Some(error),
        }
    }
}

/// Append one recognized event row to an existing segment's `events.jsonl`.
///
/// This checks the segment directory before delegating to journal-io because
/// journal-io's append primitive creates missing parents. A concurrent delete
/// after this check remains an accepted race for this write path.
pub fn append_durable_event(
    segment_path: &Path,
    event: &DurableEvent,
) -> Result<(), CallosumWriteError> {
    if !segment_path.is_dir() {
        return Err(CallosumWriteError::SegmentDirectoryMissing(
            segment_path.to_path_buf(),
        ));
    }
    let path = segment_path.join(EVENTS_FILE);
    match event {
        DurableEvent::Callosum(event) => append_jsonl(path, event),
        DurableEvent::DeviceIngest(event) => append_jsonl(path, event),
    }
    .map_err(CallosumWriteError::Append)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::Map;

    use super::*;
    use crate::CallosumEnvelope;

    static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

    fn path(name: &str) -> PathBuf {
        let suffix = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("solstone-core-callosum-writer-{name}-{suffix}"))
    }

    fn event() -> DurableEvent {
        DurableEvent::Callosum(CallosumEnvelope {
            tract: "observe".to_owned(),
            event: "status".to_owned(),
            ts: None,
            extra: Map::new(),
        })
    }

    #[test]
    fn appends_to_an_existing_segment() {
        let segment = path("success");
        fs::create_dir_all(&segment).unwrap();

        append_durable_event(&segment, &event()).unwrap();

        assert_eq!(
            fs::read_to_string(segment.join(EVENTS_FILE)).unwrap(),
            "{\"tract\":\"observe\",\"event\":\"status\"}\n"
        );
        let _ = fs::remove_dir_all(segment);
    }

    #[test]
    fn missing_segment_directory_is_not_materialized() {
        let segment = path("missing");

        assert!(matches!(
            append_durable_event(&segment, &event()),
            Err(CallosumWriteError::SegmentDirectoryMissing(_))
        ));
        assert!(!segment.exists());
    }

    #[test]
    fn blocked_parent_is_not_materialized() {
        let root = path("blocked-parent");
        let blocked = root.join("chronicle/20260804/workstation");
        fs::create_dir_all(blocked.parent().unwrap()).unwrap();
        fs::write(&blocked, b"not a directory").unwrap();
        let segment = blocked.join("120000_60");

        assert!(matches!(
            append_durable_event(&segment, &event()),
            Err(CallosumWriteError::SegmentDirectoryMissing(_))
        ));
        assert!(blocked.is_file());
        assert!(!segment.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn journal_io_append_failure_is_propagated() {
        let segment = path("append-failure");
        fs::create_dir_all(segment.join(EVENTS_FILE)).unwrap();

        assert!(matches!(
            append_durable_event(&segment, &event()),
            Err(CallosumWriteError::Append(_))
        ));
        let _ = fs::remove_dir_all(segment);
    }
}
