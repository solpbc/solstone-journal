// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    JsonWriteOptions, LockOptions, MalformedPolicy, ReadError, hold_lock, read_json, write_json,
};

use crate::{SegmentDir, SegmentError};

/// A persistent stream state record, accepting Python's legacy `type` field.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StreamRecord {
    pub name: String,
    #[serde(alias = "type")]
    pub kind: String,
    pub host: Option<String>,
    pub platform: Option<String>,
    pub created_at: u64,
    pub last_day: Option<String>,
    pub last_segment: Option<String>,
    pub seq: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StreamHints {
    pub kind: Option<String>,
    pub host: Option<String>,
    pub platform: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamAdvance {
    pub prev_day: Option<String>,
    pub prev_segment: Option<String>,
    pub seq: u64,
}

#[derive(Serialize)]
struct StreamMarker {
    stream: String,
    prev_day: Option<String>,
    prev_segment: Option<String>,
    seq: u64,
}

/// Advance one stream, then atomically write its matching segment marker.
///
/// This is deliberately two durable writes, not a cross-file transaction. The
/// state is written before the marker because stream-state rebuild tooling
/// treats markers as ground truth and can recover by skipping an orphaned
/// advance after a marker failure. That rebuild tooling is outside this crate.
pub fn advance_stream(
    name: &str,
    day: &str,
    segment: &str,
    segment_dir: &SegmentDir,
    hints: StreamHints,
) -> Result<StreamAdvance, SegmentError> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(SegmentError::StreamInput(
            "stream name must be a plain path component",
        ));
    }
    if day != segment_dir.day || segment != segment_dir.segment || name != segment_dir.stream {
        return Err(SegmentError::StreamInput(
            "stream advance does not match segment directory",
        ));
    }
    let state_path = segment_dir
        .journal
        .join("streams")
        .join(format!("{name}.json"));
    let _lock = hold_lock(&state_path, LockOptions::default())?;
    let record = read_stream_record(&state_path)?;
    let (record, advance) = update_record(record, name, day, segment, hints)?;
    write_json(&state_path, &record, JsonWriteOptions::default())?;
    let marker = StreamMarker {
        stream: name.to_owned(),
        prev_day: advance.prev_day.clone(),
        prev_segment: advance.prev_segment.clone(),
        seq: advance.seq,
    };
    write_json(
        segment_dir.path.join("stream.json"),
        &marker,
        JsonWriteOptions::default(),
    )?;
    Ok(advance)
}

fn read_stream_record(path: &Path) -> Result<Option<StreamRecord>, SegmentError> {
    match read_json(path, None, MalformedPolicy::Raise) {
        Ok(record) => Ok(record),
        Err(error @ ReadError::Malformed(_)) => Err(SegmentError::MalformedStreamRecord {
            path: path.to_path_buf(),
            source: error,
        }),
        Err(error) => Err(SegmentError::Read(error)),
    }
}

fn update_record(
    record: Option<StreamRecord>,
    name: &str,
    day: &str,
    segment: &str,
    hints: StreamHints,
) -> Result<(StreamRecord, StreamAdvance), SegmentError> {
    match record {
        None => Ok((
            StreamRecord {
                name: name.to_owned(),
                kind: hints.kind.unwrap_or_else(|| "unknown".to_owned()),
                host: hints.host,
                platform: hints.platform,
                created_at: now_unix_seconds()?,
                last_day: Some(day.to_owned()),
                last_segment: Some(segment.to_owned()),
                seq: 1,
            },
            StreamAdvance {
                prev_day: None,
                prev_segment: None,
                seq: 1,
            },
        )),
        Some(mut record) => {
            let prev_day = record.last_day.clone();
            let prev_segment = record.last_segment.clone();
            let seq = record
                .seq
                .checked_add(1)
                .ok_or(SegmentError::StreamInput("stream sequence overflow"))?;
            record.last_day = Some(day.to_owned());
            record.last_segment = Some(segment.to_owned());
            record.seq = seq;
            if let Some(kind) = hints.kind {
                record.kind = kind;
            }
            if let Some(host) = hints.host {
                record.host = Some(host);
            }
            if let Some(platform) = hints.platform {
                record.platform = Some(platform);
            }
            Ok((
                record,
                StreamAdvance {
                    prev_day,
                    prev_segment,
                    seq,
                },
            ))
        }
    }
}

fn now_unix_seconds() -> Result<u64, SegmentError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| SegmentError::Io {
            path: PathBuf::from("stream created_at"),
            source: std::io::Error::other(error),
        })
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;

    use crate::test_support::TempDir;

    use super::*;

    #[test]
    fn malformed_state_refuses_without_changing_bytes() {
        let temporary = TempDir::new();
        let state = temporary.path().join("streams/workstation.json");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        fs::write(&state, b"{not json\n").unwrap();
        let before = fs::read(&state).unwrap();
        let segment =
            SegmentDir::resolve(temporary.path(), "20260804", "120000_60", "workstation").unwrap();

        let result = advance_stream(
            "workstation",
            "20260804",
            "120000_60",
            &segment,
            StreamHints::default(),
        );
        assert!(matches!(
            result,
            Err(SegmentError::MalformedStreamRecord { .. })
        ));
        assert_eq!(fs::read(state).unwrap(), before);
    }

    #[test]
    fn bool_sequence_is_rejected() {
        let temporary = TempDir::new();
        let state = temporary.path().join("streams/workstation.json");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        fs::write(&state, br#"{"name":"workstation","type":"observer","host":null,"platform":null,"created_at":1,"last_day":null,"last_segment":null,"seq":true}"#).unwrap();
        let segment =
            SegmentDir::resolve(temporary.path(), "20260804", "120000_60", "workstation").unwrap();
        assert!(matches!(
            advance_stream(
                "workstation",
                "20260804",
                "120000_60",
                &segment,
                StreamHints::default()
            ),
            Err(SegmentError::MalformedStreamRecord { .. })
        ));
    }

    #[test]
    fn stream_advance_failure_propagates() {
        let temporary = TempDir::new();
        fs::write(temporary.path().join("streams"), b"not a directory").unwrap();
        let segment =
            SegmentDir::resolve(temporary.path(), "20260804", "120000_60", "workstation").unwrap();
        assert!(
            advance_stream(
                "workstation",
                "20260804",
                "120000_60",
                &segment,
                StreamHints::default(),
            )
            .is_err()
        );
    }

    #[test]
    fn marker_write_failure_is_returned_after_state_advance() {
        let temporary = TempDir::new();
        let segment =
            SegmentDir::resolve(temporary.path(), "20260804", "120000_60", "workstation").unwrap();
        let marker_parent = temporary.path().join("chronicle/20260804/workstation");
        fs::create_dir_all(marker_parent.parent().unwrap()).unwrap();
        fs::write(&marker_parent, b"not a directory").unwrap();

        assert!(
            advance_stream(
                "workstation",
                "20260804",
                "120000_60",
                &segment,
                StreamHints::default(),
            )
            .is_err()
        );

        let state = temporary.path().join("streams/workstation.json");
        let record: StreamRecord = serde_json::from_slice(&fs::read(state).unwrap()).unwrap();
        assert_eq!(record.seq, 1);
        assert!(!segment.path.join("stream.json").exists());
    }
}
