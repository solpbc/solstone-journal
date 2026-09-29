// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a confirmed segment delete records about its segment.
//!
//! The hold itself (the record, its window, the resume at start and the
//! outcome) is [`solstone_core_serving::held_delete`], shared with the other
//! owner deletes. This module is the segment's half: which segment, and how to
//! recognise it again.
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
use std::path::Path;

use serde::{Deserialize, Serialize};
pub(crate) use solstone_core_serving::held_delete::DeleteState;
use solstone_core_serving::held_delete::{Record, Store};

/// Where the records live, relative to the journal root.
pub(crate) const RECORD_DIR: &str = "config/segment-deletes";
pub(crate) const STORE: Store = Store::new(RECORD_DIR);

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HoldVerdict {
    Matches,
    Missing,
    Changed,
    Unreadable,
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
            let is_media = entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .and_then(solstone_core_processing_record::media_kind)
                .is_some();
            if is_media {
                media.insert(name, stamp);
            } else if name != EVENT_LOG {
                other.insert(name, stamp);
            }
        }
        Ok(Self {
            files: if media.is_empty() { other } else { media },
        })
    }

    pub(crate) fn is_media_class(&self) -> bool {
        self.files.keys().any(|name| {
            Path::new(name)
                .extension()
                .and_then(|ext| ext.to_str())
                .and_then(solstone_core_processing_record::media_kind)
                .is_some()
        })
    }

    pub(crate) fn hold(&self, dir: &Path) -> HoldVerdict {
        let mut had_missing = false;
        for (name, confirmed) in &self.files {
            match fs::metadata(dir.join(name)) {
                Ok(metadata) => {
                    if !metadata.is_file() || stamp(&metadata) != *confirmed {
                        return HoldVerdict::Changed;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    had_missing = true;
                }
                Err(_) => return HoldVerdict::Unreadable,
            }
        }
        if had_missing {
            HoldVerdict::Missing
        } else {
            HoldVerdict::Matches
        }
    }

    pub(crate) fn has_added_owner_class_file(&self, dir: &Path) -> Result<bool, std::io::Error> {
        let is_media = self.is_media_class();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == EVENT_LOG || name == solstone_core_retention::tombstone::TOMBSTONE_NAME {
                continue;
            }
            if self.files.contains_key(&name) {
                continue;
            }
            let is_media_file = entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .and_then(solstone_core_processing_record::media_kind)
                .is_some();
            if is_media {
                if is_media_file {
                    return Ok(true);
                }
            } else {
                return Ok(true);
            }
        }
        Ok(false)
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

/// The segment the owner confirmed. Its fields sit at the top level of the record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SegmentTarget {
    pub(crate) day: String,
    pub(crate) stream: String,
    pub(crate) key: String,
    pub(crate) manifest: SegmentManifest,
}

pub(crate) type DeleteRecord = Record<SegmentTarget>;

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::SegmentManifest;

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
        assert_eq!(confirmed.hold(&segment), super::HoldVerdict::Matches);

        // Removed and put back, with nothing held open: the new files are not
        // the ones the owner confirmed, even when the name and size match.
        fs::remove_dir_all(&segment).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::create_dir(&segment).unwrap();
        fs::write(segment.join("audio.flac"), b"raw").unwrap();
        assert_eq!(confirmed.hold(&segment), super::HoldVerdict::Changed);

        fs::remove_file(segment.join("audio.flac")).unwrap();
        assert_eq!(confirmed.hold(&segment), super::HoldVerdict::Missing);
    }

    #[test]
    fn media_manifest_hold_missing_and_no_added_when_processing_present() {
        let root = TempDir::new().unwrap();
        let segment = root.path().join("090000_300");
        fs::create_dir(&segment).unwrap();
        fs::write(segment.join("audio.flac"), b"raw").unwrap();
        fs::write(segment.join("audio.jsonl"), b"{}").unwrap();
        fs::write(segment.join("events.jsonl"), b"{}").unwrap();
        let confirmed = SegmentManifest::of(&segment).unwrap();
        assert!(confirmed.is_media_class());

        // Flac removed, audio.jsonl and events.jsonl and talents/ directory present
        fs::remove_file(segment.join("audio.flac")).unwrap();
        fs::create_dir(segment.join("talents")).unwrap();
        fs::write(segment.join("talents/summary.md"), b"# summary").unwrap();

        assert_eq!(confirmed.hold(&segment), super::HoldVerdict::Missing);
        assert!(!confirmed.has_added_owner_class_file(&segment).unwrap());
    }

    #[test]
    fn media_manifest_detects_added_media_file() {
        let root = TempDir::new().unwrap();
        let segment = root.path().join("090000_300");
        fs::create_dir(&segment).unwrap();
        fs::write(segment.join("audio.flac"), b"raw").unwrap();
        let confirmed = SegmentManifest::of(&segment).unwrap();

        fs::write(segment.join("extra.wav"), b"extra-raw").unwrap();
        assert!(confirmed.has_added_owner_class_file(&segment).unwrap());
    }

    #[test]
    fn no_media_manifest_detects_added_regular_file() {
        let root = TempDir::new().unwrap();
        let segment = root.path().join("090000_300");
        fs::create_dir(&segment).unwrap();
        fs::write(segment.join("imported.md"), b"# text").unwrap();
        let confirmed = SegmentManifest::of(&segment).unwrap();
        assert!(!confirmed.is_media_class());

        fs::write(segment.join("note.txt"), b"extra note").unwrap();
        assert!(confirmed.has_added_owner_class_file(&segment).unwrap());
    }

    #[test]
    fn tombstone_beside_matching_media_manifest_is_not_added() {
        let root = TempDir::new().unwrap();
        let segment = root.path().join("090000_300");
        fs::create_dir(&segment).unwrap();
        fs::write(segment.join("audio.flac"), b"raw").unwrap();
        let confirmed = SegmentManifest::of(&segment).unwrap();

        fs::write(
            segment.join(solstone_core_retention::tombstone::TOMBSTONE_NAME),
            b"{}",
        )
        .unwrap();
        assert_eq!(confirmed.hold(&segment), super::HoldVerdict::Matches);
        assert!(!confirmed.has_added_owner_class_file(&segment).unwrap());
    }
}
