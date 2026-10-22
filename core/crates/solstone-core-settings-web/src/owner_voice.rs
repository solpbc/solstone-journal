// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Status for the "leave out other voices" switch: whether a saved voice is usable, what
//! the mode left out today, and how many deletions are still owed.

use std::path::{Path, PathBuf};

use axum::response::Response;
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Value, json};
use solstone_core_processing_record::{owner_voice, read_processing_record_header};
use solstone_core_retention::owner_voice::{owner_voice_deletion_owed, owner_voice_owed_days};

use crate::http::json_response;

pub async fn get(journal_root: PathBuf) -> Response {
    json_response(status_at(&journal_root, Utc::now()))
}

/// The status as of `now`, with days taken in the owner's zone.
pub(crate) fn status_at(journal: &Path, now: DateTime<Utc>) -> Value {
    let today = now
        .with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .date_naive();
    let available =
        solstone_core_speaker_resolve::owner_centroid::owner_voice_print(journal).is_some();
    let (left_out_s, recordings) = today_counts(journal, today);
    let pending = owner_voice_owed_days(today)
        .map(|day| {
            transcripts(journal, day, true)
                .filter(|(segment, jsonl)| owner_voice_deletion_owed(segment, jsonl))
                .count()
        })
        .sum::<usize>();
    json!({
        "available": available,
        "today": {"left_out_s": left_out_s, "recordings": recordings},
        "pending_deletions": pending,
    })
}

fn today_counts(journal: &Path, today: NaiveDate) -> (f64, u64) {
    let mut left_out_s = 0.0;
    let mut recordings = 0;
    // By record, not by original: a successful deletion removes the original.
    for (_segment, jsonl) in transcripts(journal, today, false) {
        let Some(record) = read_processing_record_header(&jsonl) else {
            continue;
        };
        let Some(Ok(filter)) = owner_voice(&record) else {
            continue;
        };
        // A recording with no scored window lost no judged speech; it is not counted.
        if filter.windows_scored == 0 || filter.left_out_s <= 0.0 {
            continue;
        }
        left_out_s += filter.left_out_s;
        recordings += 1;
    }
    (left_out_s, recordings)
}

/// `(segment, transcript)` pairs for a day, optionally only those whose transcript has a media
/// sibling of the same stem; unreadable directories are skipped.
fn transcripts(
    journal: &Path,
    day: NaiveDate,
    with_original: bool,
) -> impl Iterator<Item = (PathBuf, PathBuf)> {
    let key = day.format("%Y%m%d").to_string();
    let segments = solstone_core_segment::list_segments(journal, &key).unwrap_or_default();
    segments.into_iter().flat_map(move |segment| {
        let segment = segment.path().to_path_buf();
        let names = std::fs::read_dir(&segment)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        names
            .iter()
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
            .filter(|jsonl| {
                !with_original
                    || names.iter().any(|other| {
                        other.file_stem() == jsonl.file_stem()
                            && other
                                .extension()
                                .and_then(|ext| ext.to_str())
                                .is_some_and(|ext| {
                                    solstone_core_processing_record::is_media_extension(
                                        &ext.to_ascii_lowercase(),
                                    )
                                })
                    })
            })
            .map(|jsonl| (segment.clone(), jsonl.clone()))
            .collect::<Vec<_>>()
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::TimeZone;

    use super::*;

    fn write_transcript(
        journal: &Path,
        day: &str,
        key: &str,
        left_out_s: f64,
        windows_scored: u64,
        with_original: bool,
    ) {
        let segment = journal.join(format!("chronicle/{day}/watch/{key}"));
        fs::create_dir_all(&segment).unwrap();
        let name = format!("{key}.m4a");
        if with_original {
            fs::write(segment.join(&name), b"audio").unwrap();
        }
        let header = json!({
            "raw": name,
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "empty",
                "reason_code": "no_speech",
                "handler": "transcribe",
                "attempted_at": "2026-10-20T10:00:00Z",
                "input_size": 5,
                "owner_voice_filter": true,
                "owner_voice": {
                    "version": 1,
                    "speech_s": 10.0,
                    "kept_s": 1.0,
                    "left_out_s": left_out_s,
                    "windows_scored": windows_scored,
                    "windows_kept": 0,
                    "original": {"name": name, "size": 5, "sha256": "a".repeat(64)},
                    "print": {"sha256": "b".repeat(64)},
                },
            },
        });
        fs::write(segment.join(format!("{key}.jsonl")), format!("{header}\n")).unwrap();
    }

    #[test]
    fn counts_today_in_the_owner_zone_and_pending_over_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path();
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(
            journal.join("config/journal.json"),
            r#"{"identity": {"timezone": "America/Denver"}}"#,
        )
        .unwrap();
        // 23:30 in Denver on 10-20 is already 10-21 in UTC.
        let now = Utc.with_ymd_and_hms(2026, 10, 21, 5, 30, 0).unwrap();
        write_transcript(journal, "20261020", "120000_300", 4.0, 3, true);
        write_transcript(journal, "20261020", "130000_300", 2.0, 2, true);
        write_transcript(journal, "20261020", "140000_300", 5.0, 0, true); // no scored window
        write_transcript(journal, "20261019", "120000_300", 9.0, 3, true); // yesterday
        write_transcript(journal, "20260921", "120000_300", 1.0, 1, true); // 29 days back
        write_transcript(journal, "20260920", "120000_300", 1.0, 1, true); // 30 days back
        write_transcript(journal, "20261020", "150000_300", 3.0, 1, false); // original gone

        let status = status_at(journal, now);
        assert_eq!(status["available"], json!(false));
        // Today's records count whether or not their original is still there.
        assert_eq!(status["today"]["recordings"], json!(3));
        assert_eq!(status["today"]["left_out_s"], json!(9.0));
        // Owed: today's three with originals, yesterday's, and the one 29 days back; not the
        // one 30 days back, and not the one whose original is gone.
        assert_eq!(status["pending_deletions"], json!(5));
    }
}
