// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The durable record of one confirmed segment delete.
//!
//! A delete is held for its cancel window before it runs. The hold used to live
//! only in memory, so a journal that stopped inside the window lost the delete
//! while the page had already told the owner it was happening. This record is
//! written before the delete route answers, is resumed at the next start, and
//! carries the outcome afterwards so the page and cancel can report what really
//! happened.
//!
//! ⛔ Cancel means the delete has not run yet. Nothing here commits first and
//! undoes later.
//!
//! 🔴 A resumed delete removes only the segment the owner confirmed. The record
//! keeps the size, modification time and (on Unix) inode of the segment's own
//! media files at the moment of the request, and a resumed delete runs only if
//! each of them is still there, unchanged. A segment put back under the same
//! name since then (a re-import, a restore) brings new files and does not
//! match, so the delete is refused instead of removing something the owner
//! never saw. Media is what the journal never rewrites, so processing that adds
//! or rewrites transcripts, analysis or `events.jsonl` does not break a match.
//! Directory identity is deliberately not used: the shipped Linux build cannot
//! read a directory's birth time, inodes are reused at once, and device numbers
//! move between mounts.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    AtomicWriteOptions, FileLock, LockError, LockOptions, atomic_replace, hold_lock,
};

/// Where the records live, relative to the journal root.
///
/// ⛔ Not `health/`: that tree is derived runtime state, which tooling and
/// recovery steps are free to clear. An owner's confirmed delete is intent,
/// and it lives beside the action log.
pub(crate) const RECORD_DIR: &str = "config/segment-deletes";

/// How long a finished record is kept for status and cancel to read.
const FINISHED_RETENTION_DAYS: i64 = 7;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeleteState {
    /// Confirmed and waiting for its window, or waiting for the next start.
    Pending,
    /// The segment is gone and its tombstone is in place.
    Deleted,
    /// The segment was kept. `reason` says why.
    NotDeleted,
    /// Removal started and did not finish: some of the segment may be gone.
    Incomplete,
    /// The owner cancelled inside the window.
    Cancelled,
}

impl DeleteState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Deleted => "deleted",
            Self::NotDeleted => "not_deleted",
            Self::Incomplete => "incomplete",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct FileStamp {
    pub(crate) size: u64,
    pub(crate) modified_unix_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) inode: Option<u64>,
}

/// The one file the journal appends to for its own bookkeeping.
const EVENT_LOG: &str = "events.jsonl";

/// The segment's own files as the owner confirmed them.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SegmentManifest {
    pub(crate) files: BTreeMap<String, FileStamp>,
}

impl SegmentManifest {
    /// The segment's own media files, or, for a segment with none (an import
    /// of text), every regular file but the event log.
    pub(crate) fn of(segment_dir: &Path) -> std::io::Result<Self> {
        let mut media = BTreeMap::new();
        let mut other = BTreeMap::new();
        for entry in fs::read_dir(segment_dir)? {
            let entry = entry?;
            // A file renamed away between the listing and the stat (an atomic
            // write finishing) was never one of the segment's own files.
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if !metadata.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let stamp = stamp(&metadata);
            if crate::segment_media::modality(&entry.path()).is_some() {
                media.insert(name, stamp);
            } else if name != EVENT_LOG {
                other.insert(name, stamp);
            }
        }
        Ok(Self {
            files: if media.is_empty() { other } else { media },
        })
    }

    /// Whether `segment_dir` still holds every confirmed file, unchanged.
    pub(crate) fn still_held_by(&self, segment_dir: &Path) -> std::io::Result<bool> {
        for (name, confirmed) in &self.files {
            match fs::metadata(segment_dir.join(name)) {
                Ok(metadata) if metadata.is_file() && stamp(&metadata) == *confirmed => {}
                Ok(_) => return Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }
}

fn stamp(metadata: &fs::Metadata) -> FileStamp {
    FileStamp {
        size: metadata.len(),
        modified_unix_ns: metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|elapsed| u64::try_from(elapsed.as_nanos()).ok())
            .unwrap_or(0),
        inode: inode(metadata),
    }
}

#[cfg(unix)]
fn inode(metadata: &fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.ino())
}

#[cfg(not(unix))]
fn inode(_metadata: &fs::Metadata) -> Option<u64> {
    None
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct DeleteRecord {
    pub(crate) pending_id: String,
    pub(crate) day: String,
    pub(crate) stream: String,
    pub(crate) key: String,
    pub(crate) requested_at: String,
    pub(crate) commit_at_ms: i64,
    pub(crate) manifest: SegmentManifest,
    pub(crate) state: DeleteState,
    /// Set when a commit has claimed the record. A cancel after this is too late.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) finished_at: Option<String>,
}

impl DeleteRecord {
    pub(crate) fn finish(&mut self, state: DeleteState, reason: Option<String>) {
        self.state = state;
        self.reason = reason;
        self.finished_at = Some(Utc::now().to_rfc3339());
    }
}

fn record_path(journal_root: &Path, pending_id: &str) -> PathBuf {
    journal_root
        .join(RECORD_DIR)
        .join(format!("{pending_id}.json"))
}

/// Serialize every read-modify-write of one record, across processes. A
/// commit holds it from its claim until the outcome is written.
pub(crate) fn lock(
    journal_root: &Path,
    pending_id: &str,
    wait: Duration,
) -> Result<FileLock, LockError> {
    hold_lock(
        record_path(journal_root, pending_id),
        LockOptions {
            timeout: wait,
            poll_interval: Duration::from_millis(20),
            mode: Some(0o600),
        },
    )
}

pub(crate) fn write(journal_root: &Path, record: &DeleteRecord) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    atomic_replace(
        record_path(journal_root, &record.pending_id),
        &bytes,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn read(journal_root: &Path, pending_id: &str) -> Option<DeleteRecord> {
    let bytes = fs::read(record_path(journal_root, pending_id)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Every readable record.
fn all(journal_root: &Path) -> Vec<(PathBuf, DeleteRecord)> {
    let Ok(entries) = fs::read_dir(journal_root.join(RECORD_DIR)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .filter_map(|path| {
            let record = fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<DeleteRecord>(&bytes).ok())?;
            Some((path, record))
        })
        .collect()
}

fn finished_days_ago(record: &DeleteRecord, now: DateTime<Utc>) -> Option<i64> {
    let finished = DateTime::parse_from_rfc3339(record.finished_at.as_deref()?).ok()?;
    Some(
        now.signed_duration_since(finished.with_timezone(&Utc))
            .num_days(),
    )
}

/// Every record still waiting to run, earliest deadline first. Finished records
/// past their retention are removed on the way.
pub(crate) fn pending_and_prune(journal_root: &Path, now: DateTime<Utc>) -> Vec<DeleteRecord> {
    let mut pending = Vec::new();
    for (path, record) in all(journal_root) {
        if record.state == DeleteState::Pending {
            pending.push(record);
        } else if finished_days_ago(&record, now)
            .is_some_and(|days| days >= FINISHED_RETENTION_DAYS)
        {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_file(path.with_extension("json.lock"));
        }
    }
    pending.sort_by_key(|record| record.commit_at_ms);
    pending
}

/// Held while a delete request checks for a waiting delete and writes its own.
pub(crate) fn create_lock(journal_root: &Path) -> Result<FileLock, LockError> {
    hold_lock(
        journal_root.join(RECORD_DIR).join("create"),
        LockOptions {
            timeout: Duration::from_secs(5),
            poll_interval: Duration::from_millis(20),
            mode: Some(0o600),
        },
    )
}

/// A delete already waiting for this segment, so a second request joins it
/// instead of racing it.
pub(crate) fn pending_for(
    journal_root: &Path,
    day: &str,
    stream: &str,
    key: &str,
) -> Option<DeleteRecord> {
    all(journal_root)
        .into_iter()
        .map(|(_, record)| record)
        .find(|record| {
            record.state == DeleteState::Pending
                && record.started_at.is_none()
                && record.day == day
                && record.stream == stream
                && record.key == key
        })
}

/// How long past its deadline a delete may stay pending before it is reported.
const OVERDUE_MINUTES: i64 = 5;

/// Deletes that ended without removing the segment, still inside retention,
/// and deletes stuck pending well past their deadline.
pub(crate) fn unfinished_outcomes(journal_root: &Path, now: DateTime<Utc>) -> Vec<DeleteRecord> {
    let mut outcomes = all(journal_root)
        .into_iter()
        .map(|(_, record)| record)
        .filter(|record| match record.state {
            DeleteState::NotDeleted | DeleteState::Incomplete => {
                finished_days_ago(record, now).is_some_and(|days| days < FINISHED_RETENTION_DAYS)
            }
            DeleteState::Pending => {
                now.timestamp_millis() - record.commit_at_ms > OVERDUE_MINUTES * 60_000
            }
            DeleteState::Deleted | DeleteState::Cancelled => false,
        })
        .collect::<Vec<_>>();
    outcomes.sort_by(|left, right| left.finished_at.cmp(&right.finished_at));
    outcomes
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::{Duration, Utc};
    use tempfile::TempDir;

    use super::{DeleteRecord, DeleteState, SegmentManifest, pending_and_prune, read, write};

    fn record(id: &str, state: DeleteState, commit_at_ms: i64) -> DeleteRecord {
        DeleteRecord {
            pending_id: id.repeat(32),
            day: "20260731".into(),
            stream: "field".into(),
            key: "090000_300".into(),
            requested_at: Utc::now().to_rfc3339(),
            commit_at_ms,
            manifest: SegmentManifest::default(),
            state,
            started_at: None,
            reason: None,
            finished_at: None,
        }
    }

    #[test]
    fn a_segment_put_back_under_the_same_name_does_not_match_the_confirmed_one() {
        let root = TempDir::new().unwrap();
        let segment = root.path().join("090000_300");
        fs::create_dir(&segment).unwrap();
        fs::write(segment.join("audio.flac"), b"raw").unwrap();
        fs::write(segment.join("audio.jsonl"), b"{}").unwrap();
        fs::write(segment.join("events.jsonl"), b"{}").unwrap();
        let confirmed = SegmentManifest::of(&segment).unwrap();
        assert_eq!(
            confirmed.files.keys().collect::<Vec<_>>(),
            vec!["audio.flac"]
        );

        // Processing adds and rewrites its own files; still the confirmed segment.
        fs::write(segment.join("audio.jsonl"), b"{\"rewritten\":true}").unwrap();
        fs::write(segment.join("events.jsonl"), b"{}\n{}").unwrap();
        fs::create_dir(segment.join("talents")).unwrap();
        assert!(confirmed.still_held_by(&segment).unwrap());

        // Removed and put back, with nothing held open: the new files are not
        // the ones the owner confirmed, even when the name and size match.
        fs::remove_dir_all(&segment).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::create_dir(&segment).unwrap();
        fs::write(segment.join("audio.flac"), b"raw").unwrap();
        assert!(!confirmed.still_held_by(&segment).unwrap());

        fs::remove_file(segment.join("audio.flac")).unwrap();
        assert!(!confirmed.still_held_by(&segment).unwrap());
    }

    #[test]
    fn pending_records_are_returned_by_deadline_and_old_finished_ones_are_pruned() {
        let root = TempDir::new().unwrap();
        write(root.path(), &record("b", DeleteState::Pending, 20)).unwrap();
        write(root.path(), &record("a", DeleteState::Pending, 10)).unwrap();
        let mut old = record("c", DeleteState::Deleted, 0);
        old.finished_at = Some((Utc::now() - Duration::days(8)).to_rfc3339());
        write(root.path(), &old).unwrap();
        let mut recent = record("d", DeleteState::NotDeleted, 0);
        recent.finished_at = Some(Utc::now().to_rfc3339());
        write(root.path(), &recent).unwrap();

        let pending = pending_and_prune(root.path(), Utc::now());
        assert_eq!(
            pending
                .iter()
                .map(|record| record.commit_at_ms)
                .collect::<Vec<_>>(),
            vec![10, 20]
        );
        assert!(read(root.path(), &"c".repeat(32)).is_none());
        assert_eq!(
            read(root.path(), &"d".repeat(32)).unwrap().state,
            DeleteState::NotDeleted
        );
    }
}
