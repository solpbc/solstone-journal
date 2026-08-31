// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

//! Durable, journal-rooted MCP interaction audit records.

use std::error::Error;
use std::fmt;
use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, PathError, SegmentDeconflictError, day_path,
    find_available_segment, segment_path, write_bytes_exclusive,
};

const INTERACTION_FILE: &str = "interaction.json";
const MAX_SEGMENT_ATTEMPTS: usize = 128;
const STREAM: &str = "mcp.agent";

/// One MCP tool interaction retained in the journal.
///
/// The serialized shape is intentionally closed: it contains exactly the
/// current agent identity, interaction timestamp, and known tool name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractionRecord {
    pub agent_identity: String,
    pub timestamp: DateTime<Utc>,
    pub tool_name: ToolName,
}

/// MCP tools that may be represented in an interaction record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolName {
    Search,
    Fetch,
}

/// Location of one durably published interaction record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditCoordinates {
    pub day: NaiveDate,
    pub stream: String,
    pub segment: String,
}

/// Failure while publishing an MCP interaction record.
#[derive(Debug)]
pub enum AuditWriteError {
    DayPath(PathError),
    SegmentAllocation(SegmentDeconflictError),
    SegmentPath(PathError),
    NoAvailableSegment,
    Serialization(serde_json::Error),
    AtomicWrite(AtomicWriteError),
}

impl fmt::Display for AuditWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DayPath(error) => write!(formatter, "could not resolve MCP audit day: {error}"),
            Self::SegmentAllocation(error) => {
                write!(formatter, "could not allocate MCP audit segment: {error}")
            }
            Self::SegmentPath(error) => {
                write!(formatter, "could not create MCP audit segment: {error}")
            }
            Self::NoAvailableSegment => write!(formatter, "no MCP audit segment was available"),
            Self::Serialization(error) => {
                write!(
                    formatter,
                    "could not serialize MCP audit interaction: {error}"
                )
            }
            Self::AtomicWrite(error) => {
                write!(
                    formatter,
                    "could not publish MCP audit interaction: {error}"
                )
            }
        }
    }
}

impl Error for AuditWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::DayPath(error) | Self::SegmentPath(error) => Some(error),
            Self::SegmentAllocation(error) => Some(error),
            Self::Serialization(error) => Some(error),
            Self::AtomicWrite(error) => Some(error),
            Self::NoAvailableSegment => None,
        }
    }
}

/// Publish one create-exclusive MCP interaction record.
///
/// `now` is captured by the caller exactly once and drives both the record
/// timestamp and its chronicle day/segment coordinates.
pub fn write_interaction_record(
    journal_root: &Path,
    now: DateTime<Utc>,
    agent_identity: &str,
    tool_name: ToolName,
) -> Result<AuditCoordinates, AuditWriteError> {
    let day = now.date_naive();
    let day_key = day.format("%Y%m%d").to_string();
    let day_directory =
        day_path(journal_root, Some(&day_key), true).map_err(AuditWriteError::DayPath)?;
    let stream_directory = day_directory.join(STREAM);
    let candidate = format!("{}_1", now.format("%H%M%S"));
    let segment = find_available_segment(&stream_directory, &candidate, MAX_SEGMENT_ATTEMPTS)
        .map_err(AuditWriteError::SegmentAllocation)?
        .ok_or(AuditWriteError::NoAvailableSegment)?;
    let segment_directory = segment_path(journal_root, &day_key, &segment, STREAM, true)
        .map_err(AuditWriteError::SegmentPath)?;
    let record = InteractionRecord {
        agent_identity: agent_identity.to_owned(),
        timestamp: now,
        tool_name,
    };
    let contents = serde_json::to_vec(&record).map_err(AuditWriteError::Serialization)?;
    write_bytes_exclusive(
        segment_directory.join(INTERACTION_FILE),
        &contents,
        AtomicWriteOptions::default(),
    )
    .map_err(AuditWriteError::AtomicWrite)?;

    Ok(AuditCoordinates {
        day,
        stream: STREAM.to_owned(),
        segment,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::{InteractionRecord, ToolName, write_interaction_record};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn journal_root() -> PathBuf {
        let root = PathBuf::from("/var/tmp").join(format!(
            "solstone-mcp-audit-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn interaction_record_serializes_only_the_closed_audit_fields() {
        let record = InteractionRecord {
            agent_identity: "operator".to_owned(),
            timestamp: Utc.with_ymd_and_hms(2026, 8, 31, 12, 34, 56).unwrap(),
            tool_name: ToolName::Search,
        };

        assert_eq!(
            serde_json::to_value(record).unwrap(),
            json!({
                "agent_identity": "operator",
                "timestamp": "2026-08-31T12:34:56Z",
                "tool_name": "search",
            })
        );
    }

    #[test]
    fn writes_one_closed_record_at_the_returned_coordinates() {
        let root = journal_root();
        let now = Utc.with_ymd_and_hms(2026, 8, 31, 12, 34, 56).unwrap();

        let coordinates =
            write_interaction_record(&root, now, "operator", ToolName::Fetch).unwrap();

        assert_eq!(coordinates.day.to_string(), "2026-08-31");
        assert_eq!(coordinates.stream, "mcp.agent");
        assert_eq!(coordinates.segment, "123456_1");
        let record =
            fs::read_to_string(root.join("chronicle/20260831/mcp.agent/123456_1/interaction.json"))
                .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&record).unwrap(),
            json!({
                "agent_identity": "operator",
                "timestamp": "2026-08-31T12:34:56Z",
                "tool_name": "fetch",
            })
        );

        fs::remove_dir_all(root).unwrap();
    }
}
