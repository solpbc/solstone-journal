// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner links attached only after a tool has admitted a row. These are
//! pointers, never credentials or tool references.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_link::{Place, seal};
use solstone_core_transcripts_web::{DaySegmentRef, day_segment_list};

pub(crate) fn attach(journal: &Path, row: &mut Value, place: Place) {
    if let Some(sealed) = seal(journal, &place) {
        row["link"] = Value::String(format!("https://go.solstone.app/j#{sealed}"));
    }
}

pub(crate) fn activity(journal: &Path, row: &mut Value, facet: &str, day: &str, id: &str) {
    attach(
        journal,
        row,
        Place::Activity {
            facet: facet.to_owned(),
            day: day.to_owned(),
            id: id.to_owned(),
        },
    );
}

/// One response's view of the same listing that the link resolver uses.
/// Failed days have no optional recording links, without losing their rows.
#[derive(Default)]
pub(crate) struct Recordings {
    days: BTreeMap<String, Vec<DaySegmentRef>>,
}

impl Recordings {
    pub(crate) fn attach(
        &mut self,
        journal: &Path,
        row: &mut Value,
        day: &str,
        stream: &str,
        segment: &str,
    ) {
        let listed = self.days.entry(day.to_owned()).or_insert_with(|| {
            match day_segment_list(journal, day, chrono::Utc::now()) {
                Ok(listed) => listed,
                Err(_) => {
                    log::warn!(
                        "journal link recording list unavailable; optional recording links omitted"
                    );
                    Vec::new()
                }
            }
        });
        if listed
            .iter()
            .any(|item| item.stream == stream && item.key == segment)
        {
            attach(
                journal,
                row,
                Place::Recording {
                    day: day.to_owned(),
                    stream: stream.to_owned(),
                    segment: segment.to_owned(),
                },
            );
        }
    }

    /// A search hit's admitted activity identity, or the recording its path
    /// names. Day documents and memory notes are not recording places.
    pub(crate) fn entry(
        &mut self,
        journal: &Path,
        row: &mut Value,
        path: &str,
        activity_id: Option<&str>,
    ) {
        let parts: Vec<_> = path
            .strip_prefix("chronicle/")
            .unwrap_or(path)
            .split('/')
            .collect();
        match parts.as_slice() {
            ["facets", facet, "activities", day_file] => {
                if let (Some(day), Some(id)) = (day_file.strip_suffix(".jsonl"), activity_id) {
                    activity(journal, row, facet, day, id);
                }
            }
            [day, stream, segment, ..]
                if solstone_core_journal_io::is_day_key(day)
                    && solstone_core_transcripts::segment_clock(segment).is_some()
                    && !stream.starts_with("agent-memory-") =>
            {
                self.attach(journal, row, day, stream, segment);
            }
            [day, segment, ..]
                if solstone_core_journal_io::is_day_key(day)
                    && solstone_core_transcripts::segment_clock(segment).is_some() =>
            {
                self.attach(
                    journal,
                    row,
                    day,
                    solstone_core_journal_io::paths::DEFAULT_STREAM,
                    segment,
                );
            }
            _ => {}
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn journal_links_failed_day_preserves_rows_and_other_days_links() {
        let journal = tempfile::tempdir_in(crate::test_scratch()).unwrap();
        let root = journal.path();
        // A named _default cannot be represented as a recording identity.
        // The page's scan refuses the day, even though one row is readable.
        for (day, stream) in [
            ("20260304", "desk"),
            ("20260304", "_default"),
            ("20260305", "desk"),
        ] {
            let dir = root
                .join("chronicle")
                .join(day)
                .join(stream)
                .join("090000_300");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("meeting_transcript.md"), "a readable line").unwrap();
        }
        assert!(day_segment_list(root, "20260304", chrono::Utc::now()).is_err());
        let mut links = Recordings::default();
        let original = json!({"reference":"kept", "entries":["a readable line"]});
        let mut failed = original.clone();
        links.attach(root, &mut failed, "20260304", "desk", "090000_300");
        assert_eq!(failed, original);
        let mut good = original;
        links.attach(root, &mut good, "20260305", "desk", "090000_300");
        let sealed = good["link"].as_str().unwrap().split_once('#').unwrap().1;
        assert_eq!(
            solstone_core_journal_link::open(root, sealed),
            Ok(Place::Recording {
                day: "20260305".into(),
                stream: "desk".into(),
                segment: "090000_300".into(),
            })
        );
    }

    #[test]
    fn journal_links_only_listed_recordings_get_urls_beside_body_windows_and_workouts() {
        let journal = tempfile::tempdir_in(crate::test_scratch()).unwrap();
        let root = journal.path();
        for (stream, file) in [
            ("desk", "meeting_transcript.md"),
            ("import.apple_health", "body.jsonl"),
            ("import.strava", "workout.json"),
        ] {
            let dir = root
                .join("chronicle/20260304")
                .join(stream)
                .join("090000_300");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(file), "{}\n").unwrap();
        }
        let mut links = Recordings::default();
        let original = json!({"reference":"kept"});
        for stream in ["import.apple_health", "import.strava", "absent"] {
            let mut row = original.clone();
            links.attach(root, &mut row, "20260304", stream, "090000_300");
            assert_eq!(row, original, "{stream}");
        }
        let mut good = original;
        links.attach(root, &mut good, "20260304", "desk", "090000_300");
        assert!(good["link"].is_string());
    }

    #[test]
    fn journal_links_damaged_root_does_not_replace_key_or_drop_row() {
        let journal = tempfile::tempdir_in(crate::test_scratch()).unwrap();
        fs::create_dir(journal.path().join("link")).unwrap();
        fs::write(journal.path().join("link/root.key"), "damaged").unwrap();
        let original = json!({"reference":"kept", "title":"an activity"});
        let mut row = original.clone();
        activity(journal.path(), &mut row, "work", "20260304", "meeting");
        assert_eq!(row, original);
        assert_eq!(
            fs::read_to_string(journal.path().join("link/root.key")).unwrap(),
            "damaged"
        );
    }
}
