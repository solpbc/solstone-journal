// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The shared activity record: what one activity of the owner's journal
//! becomes when it is shared. It carries the activity's title and summary and
//! each of its segments' times and summaries; nothing else from the journal.

use std::fmt;
use std::fs;
use std::path::Path;

use jiff::Timestamp;
use jiff::civil::Time;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ACTIVITY_COLLECTION;

/// Where an activity lives in the sharing owner's journal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ActivityRef {
    pub facet: String,
    pub day: String,
    pub id: String,
}

/// One segment of a shared activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedSegment {
    pub started_at: String,
    pub ended_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// The `app.solstone.memory.activity` record value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRecord {
    #[serde(rename = "$type")]
    pub record_type: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub started_at: String,
    pub ended_at: String,
    pub segments: Vec<SharedSegment>,
    pub created_at: String,
    pub updated_at: String,
}

impl ActivityRecord {
    /// A digest of what the record says, leaving out its own timestamps, so
    /// a sync can tell whether the activity changed since the last write.
    pub fn content_digest(&self) -> String {
        let content = serde_json::json!({
            "title": self.title,
            "summary": self.summary,
            "startedAt": self.started_at,
            "endedAt": self.ended_at,
            "segments": self.segments,
        });
        let bytes = serde_json::to_vec(&content).unwrap_or_default();
        hex(&Sha256::digest(bytes))
    }

    /// Parse a received value, tolerantly: anything beyond the fields read
    /// here is kept by the store as received, never interpreted.
    pub fn from_received(value: &Value) -> Option<Self> {
        let record: Self = serde_json::from_value(value.clone()).ok()?;
        (record.record_type == ACTIVITY_COLLECTION).then_some(record)
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordError {
    ActivityNotFound,
    ActivityUnreadable(String),
    NoSegments,
    BadSegmentKey(String),
}

impl fmt::Display for RecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ActivityNotFound => formatter.write_str("activity not found"),
            Self::ActivityUnreadable(detail) => write!(formatter, "activity unreadable: {detail}"),
            Self::NoSegments => formatter.write_str("activity has no segments left to share"),
            Self::BadSegmentKey(key) => write!(formatter, "segment key {key} is malformed"),
        }
    }
}

impl std::error::Error for RecordError {}

/// Build the shared record for one activity from the owner's journal.
/// `created_at` is the first share's time, kept across updates; `now` stamps
/// `updatedAt`. A segment the owner deleted is left out, and an activity with
/// none left is not shareable.
pub fn build_record(
    journal: &Path,
    activity: &ActivityRef,
    created_at: Timestamp,
    now: Timestamp,
) -> Result<ActivityRecord, RecordError> {
    let row = solstone_core_facets::get_activity_record(
        journal,
        &activity.facet,
        &activity.day,
        &activity.id,
    )
    .map_err(|err| RecordError::ActivityUnreadable(err.to_string()))?
    .ok_or(RecordError::ActivityNotFound)?;
    let zone = solstone_core_journal_config::owner_zone(journal);
    let stream = row.get("stream").and_then(Value::as_str);
    let keys: Vec<&str> = row
        .get("segments")
        .and_then(Value::as_array)
        .map(|keys| keys.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut segments = Vec::new();
    for key in keys {
        let directory = segment_directory(journal, &activity.day, stream, key);
        if directory.join("tombstone.json").exists() {
            continue;
        }
        let (start, end) = segment_bounds(&zone, &activity.day, key)
            .ok_or_else(|| RecordError::BadSegmentKey(key.to_owned()))?;
        segments.push(SharedSegment {
            started_at: start.to_string(),
            ended_at: end.to_string(),
            summary: segment_summary(&directory),
        });
    }
    let (Some(first), Some(last)) = (segments.first(), segments.last()) else {
        return Err(RecordError::NoSegments);
    };
    let text = |field: &str| {
        row.get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(ToOwned::to_owned)
    };
    let story = row
        .get("story")
        .and_then(|story| story.get("body"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned);
    Ok(ActivityRecord {
        record_type: ACTIVITY_COLLECTION.to_owned(),
        title: text("title")
            .or_else(|| text("description"))
            .unwrap_or_default(),
        summary: story.or_else(|| text("description")),
        started_at: first.started_at.clone(),
        ended_at: last.ended_at.clone(),
        segments,
        created_at: created_at.to_string(),
        updated_at: now.to_string(),
    })
}

fn segment_directory(
    journal: &Path,
    day: &str,
    stream: Option<&str>,
    key: &str,
) -> std::path::PathBuf {
    let day_dir = journal.join("chronicle").join(day);
    match stream {
        Some(stream) => day_dir.join(stream).join(key),
        None => day_dir.join(key),
    }
}

/// The segment's own one-line summary, from its Sense output.
fn segment_summary(directory: &Path) -> Option<String> {
    let text = fs::read_to_string(directory.join("talents/sense.json")).ok()?;
    let json: Value = serde_json::from_str(&text).ok()?;
    json.get("activity_summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .map(ToOwned::to_owned)
}

/// A segment key `HHMMSS_LEN` read as wall time in the owner's zone on `day`,
/// with its length in seconds.
fn segment_bounds(zone: &TimeZone, day: &str, key: &str) -> Option<(Timestamp, Timestamp)> {
    let (clock, length) = key.split_once('_')?;
    let digits = clock.as_bytes();
    if digits.len() != 6 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let length: i64 = length.split('_').next()?.parse().ok()?;
    let field = |at: usize| (digits[at] - b'0') as i8 * 10 + (digits[at + 1] - b'0') as i8;
    let time = Time::new(field(0), field(2), field(4), 0).ok()?;
    let date = solstone_core_time::parse_day(day)?;
    let start = solstone_core_time::from_local(zone, date.to_datetime(time))
        .earliest()?
        .timestamp();
    let end = start
        .checked_add(jiff::SignedDuration::from_secs(length))
        .ok()?;
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn activity_journal() -> (tempfile::TempDir, ActivityRef) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            r#"{"identity":{"timezone":"America/Denver"}}"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("facets/work/activities")).unwrap();
        fs::write(
            root.join("facets/work/activities/20261121.jsonl"),
            concat!(
                r#"{"id":"meeting_090000_300","activity":"meeting","stream":"desk","segments":["090000_300","090500_300","091000_300"],"title":"Sprint planning","description":"Planning the sprint","details":"","story":{"talent":"conversation","body":"We planned the sprint and split the work.","topics":[],"confidence":0.9},"edits":[],"created_at":1}"#,
                "\n"
            ),
        )
        .unwrap();
        for (key, summary) in [
            ("090000_300", "Opened the board"),
            ("090500_300", "Split the work"),
        ] {
            let talents = root
                .join("chronicle/20261121/desk")
                .join(key)
                .join("talents");
            fs::create_dir_all(&talents).unwrap();
            fs::write(
                talents.join("sense.json"),
                serde_json::json!({"activity_summary": summary, "density": "active"}).to_string(),
            )
            .unwrap();
        }
        let deleted = root.join("chronicle/20261121/desk/091000_300");
        fs::create_dir_all(&deleted).unwrap();
        fs::write(deleted.join("tombstone.json"), "{}").unwrap();
        (
            dir,
            ActivityRef {
                facet: "work".to_owned(),
                day: "20261121".to_owned(),
                id: "meeting_090000_300".to_owned(),
            },
        )
    }

    #[test]
    fn an_activity_becomes_its_summary_and_timed_segments() {
        let (dir, activity) = activity_journal();
        let now: Timestamp = "2026-11-22T10:00:00Z".parse().unwrap();
        let record = build_record(dir.path(), &activity, now, now).unwrap();
        assert_eq!(record.record_type, ACTIVITY_COLLECTION);
        assert_eq!(record.title, "Sprint planning");
        assert_eq!(
            record.summary.as_deref(),
            Some("We planned the sprint and split the work.")
        );
        // 09:00 in Denver on 2026-11-21 is 16:00 UTC; the deleted third segment is left out.
        assert_eq!(record.started_at, "2026-11-21T16:00:00Z");
        assert_eq!(record.ended_at, "2026-11-21T16:10:00Z");
        assert_eq!(record.segments.len(), 2);
        assert_eq!(
            record.segments[1].summary.as_deref(),
            Some("Split the work")
        );

        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["$type"], ACTIVITY_COLLECTION);
        assert!(
            json.get("details").is_none(),
            "nothing beyond the record's fields is shared"
        );
        assert!(json["segments"][0].get("startedAt").is_some());
    }

    #[test]
    fn the_digest_ignores_timestamps_and_follows_content() {
        let (dir, activity) = activity_journal();
        let t1: Timestamp = "2026-11-22T10:00:00Z".parse().unwrap();
        let t2: Timestamp = "2026-11-23T10:00:00Z".parse().unwrap();
        let first = build_record(dir.path(), &activity, t1, t1).unwrap();
        let later = build_record(dir.path(), &activity, t1, t2).unwrap();
        assert_eq!(first.content_digest(), later.content_digest());
        let mut edited = later.clone();
        edited.title = "Sprint planning, round two".to_owned();
        assert_ne!(first.content_digest(), edited.content_digest());
    }

    #[test]
    fn a_received_value_of_another_type_is_not_an_activity() {
        let value = serde_json::json!({"$type": "app.bsky.feed.post", "title": "x", "startedAt": "a", "endedAt": "b", "segments": [], "createdAt": "c", "updatedAt": "d"});
        assert!(ActivityRecord::from_received(&value).is_none());
    }
}
