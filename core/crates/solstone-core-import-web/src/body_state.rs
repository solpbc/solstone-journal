// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What an Apple Health import reads as on the import page, worked out when the
//! row is read so nothing says `failed` where its data is there.
//!
//! It reads the import's own record (its attempt, its publication record and
//! any `body-stop.json`) and where it stands among the journal's other Apple
//! Health imports, in the order this journal admitted their runs
//! (`body_runs`), never by the clock. It reads no run state. An import that
//! didn't run here, such as a merged one, is never ordered, so no later run
//! reads as having saved it. A row it can't place keeps the import page's own.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_import::metadata::{AttemptRead, AttemptState, read_attempt_facts};
use solstone_core_import::publish::{
    DayMarkerStatus, SegmentBindingOutcome, read_publication_record,
};
use solstone_core_import::{ProjectionStatus, read_import_metadata};
use solstone_core_import_sources::body_runs;
use solstone_core_import_sources::body_status::{self, StopClass};

const APPLE: &str = "apple_health";

/// One Apple Health import, as the others are measured against it.
#[derive(Clone, Debug)]
pub(crate) struct Seen {
    id: String,
    started_ms: Option<u64>,
    /// Its place in this journal's order of runs; a live start not yet
    /// admitted comes after every admitted one.
    order: Option<u64>,
    running: bool,
    /// `Completed` with a readable record listing no failing window.
    clean: bool,
}

/// Every Apple Health import in the journal, read once per list.
pub(crate) fn apple_imports(root: &Path) -> Vec<Seen> {
    let Ok(entries) = std::fs::read_dir(root.join("imports")) else {
        return Vec::new();
    };
    let order = body_runs::Order::read(root);
    entries
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().into_owned();
            if id.starts_with('.') || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                return None;
            }
            let metadata = read_import_metadata(root, &id).ok()?;
            if metadata.get("source_hint").and_then(Value::as_str) != Some(APPLE) {
                return None;
            }
            let attempt = read_attempt_facts(&metadata);
            let started_ms = match &attempt {
                AttemptRead::Present(facts) => Some(facts.started_at_ms),
                _ => metadata
                    .get("task_id")
                    .and_then(Value::as_str)
                    .and_then(|task| task.parse::<u64>().ok()),
            };
            let running =
                solstone_core_import::project_import_result(root, &id).status == ProjectionStatus::Running;
            let clean = matches!(&attempt, AttemptRead::Present(facts) if facts.state == AttemptState::Completed)
                && failing_windows(root, &id) == Some(0);
            let order = order
                .of(root, &id)
                .or_else(|| running.then_some(u64::MAX));
            Some(Seen {
                id,
                started_ms,
                order,
                running,
                clean,
            })
        })
        .collect()
}

/// How many windows an import's terminal record lists as failing: a failed
/// binding, or a bound window whose file search couldn't take or whose day's
/// marker failed. `None` when the record is missing or can't be read. A search
/// error on a file no window holds, such as the summary, isn't a window.
pub(crate) fn failing_windows(root: &Path, import_id: &str) -> Option<usize> {
    let record = match read_publication_record(&root.join("imports").join(import_id)) {
        Ok(Some(record)) => record,
        // A body run always writes its terminal record: one that isn't there
        // can't say what it saved.
        Ok(None) | Err(_) => return None,
    };
    let failed_days: BTreeSet<&str> = record
        .day_markers
        .iter()
        .filter(|marker| matches!(marker.outcome, DayMarkerStatus::Failed { .. }))
        .map(|marker| marker.day.as_str())
        .collect();
    let searched_away = |day: &str, stream: &str, segment: &str| {
        record.indexing.errored.iter().any(|error| {
            let parts: Vec<String> = error
                .path
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            parts
                .windows(3)
                .any(|three| three[0] == day && three[1] == stream && three[2] == segment)
        })
    };
    let failing = record
        .segments
        .iter()
        .filter(|segment| {
            matches!(
                segment.outcome,
                SegmentBindingOutcome::FailedAtAdvance { .. }
                    | SegmentBindingOutcome::FailedAtMarkerWrite { .. }
            ) || (matches!(segment.outcome, SegmentBindingOutcome::Bound { .. })
                && (failed_days.contains(segment.day.as_str())
                    || searched_away(&segment.day, &segment.stream, &segment.segment)))
        })
        .map(|segment| (segment.day.as_str(), segment.segment.as_str()))
        .collect::<BTreeSet<_>>();
    Some(failing.len())
}

/// A start time as the owner reads it, in the journal's zone.
fn when(root: &Path, started_ms: u64) -> String {
    let zone = solstone_core_journal_config::owner_zone(root);
    chrono::DateTime::from_timestamp_millis(i64::try_from(started_ms).unwrap_or(i64::MAX))
        .map(|at| {
            at.with_timezone(&zone)
                .format("%b %-d, %-I:%M %p")
                .to_string()
        })
        .unwrap_or_default()
}

/// The row's state and words, or `None` to keep the import page's own reading.
pub(crate) fn apple_row(
    root: &Path,
    import_id: &str,
    all: &[Seen],
) -> Option<(&'static str, String)> {
    let metadata = read_import_metadata(root, import_id).ok()?;
    if metadata.get("source_hint").and_then(Value::as_str) != Some(APPLE) {
        return None;
    }
    let attempt = read_attempt_facts(&metadata);
    let projection = solstone_core_import::project_import_result(root, import_id);
    if matches!(attempt, AttemptRead::Malformed)
        || projection.status == ProjectionStatus::Unavailable
    {
        return None;
    }
    let latest = match &attempt {
        AttemptRead::Present(facts) => Some(facts.generation),
        _ => None,
    };
    // A recorded reason, for the attempt as it stands.
    let stop = body_status::read(root, import_id).filter(|(_, generation)| match latest {
        Some(latest) => *generation == latest,
        None => *generation == 1,
    });
    let classed = |code: &str| -> (&'static str, String) {
        match body_status::class_of(code) {
            StopClass::Refused => ("failed", body_status::words(code).to_owned()),
            StopClass::DidntStart => (
                "didnt_start",
                format!(
                    "{} once it finishes, import the export again.",
                    body_status::words(code)
                ),
            ),
            StopClass::Stopped => ("stopped", String::new()),
        }
    };
    if let (AttemptRead::Absent, Some((code, _))) = (&attempt, &stop)
        && body_status::class_of(code) != StopClass::Stopped
    {
        return Some(classed(code));
    }
    if matches!(
        projection.status,
        ProjectionStatus::Pending | ProjectionStatus::Running
    ) {
        return None;
    }
    let ours = body_runs::Order::read(root).of(root, import_id);
    let later = |seen: &&Seen| {
        seen.id != import_id
            && match (seen.order, ours) {
                (Some(theirs), Some(ours)) => theirs > ours,
                _ => false,
            }
    };
    if let AttemptRead::Present(facts) = &attempt
        && facts.state == AttemptState::Completed
    {
        let failing = failing_windows(root, import_id);
        if failing == Some(0) || all.iter().filter(later).any(|seen| seen.clean) {
            return Some(("saved", String::new()));
        }
        let words = match failing {
            Some(1) => "1 part of your journal didn't finish saving. importing any Apple Health export again tries it.".to_owned(),
            Some(n) => format!("{n} parts of your journal didn't finish saving. importing any Apple Health export again tries them."),
            None => "some parts of your journal didn't finish saving. importing any Apple Health export again tries them.".to_owned(),
        };
        return Some(("partly_saved", words));
    }
    if let Some((code, _)) = &stop {
        let (state, words) = classed(code);
        if state != "stopped" {
            return Some((state, words));
        }
    }
    // The run that carried this one on, as that run recorded it on the attempt
    // it ended (a takeover does); otherwise the run order says which ran after.
    let carried_by = match &attempt {
        AttemptRead::Present(facts) => facts.continued_by.as_deref(),
        _ => None,
    }
    .and_then(|id| all.iter().find(|seen| seen.id == id));
    if let Some(seen) = carried_by {
        let at = when(root, seen.started_ms.unwrap_or_default());
        let words = if seen.running {
            format!(
                "stopped before it finished. carried on by the import of {at}, which is running now."
            )
        } else {
            format!("stopped before it finished. carried on by the import of {at}.")
        };
        return Some(("stopped", words));
    }
    let after = all
        .iter()
        .filter(later)
        .filter(|seen| seen.clean || seen.running)
        .min_by_key(|seen| seen.order);
    let words = match after {
        Some(seen) if seen.clean => format!(
            "stopped before it finished. your import of {} ran after it.",
            when(root, seen.started_ms.unwrap_or_default())
        ),
        Some(seen) => format!(
            "stopped before it finished. your import of {} is running now.",
            when(root, seen.started_ms.unwrap_or_default())
        ),
        None => match stop.as_ref() {
            Some((code, _)) => body_status::stopped_words(code),
            None => "it stopped before it finished. import the export again to finish.".to_owned(),
        },
    };
    Some(("stopped", words))
}

/// Put the reading on a list or detail row: its state, its words, and the
/// generic status the page already knows how to draw.
pub(crate) fn overlay(root: &Path, import_id: &str, values: &mut Map<String, Value>, all: &[Seen]) {
    // An Apple Health upload is named for what it is in every state, running and
    // pending included, not by the generic kind its upload was saved as.
    let apple = read_import_metadata(root, import_id)
        .is_ok_and(|metadata| metadata.get("source_hint").and_then(Value::as_str) == Some(APPLE));
    if apple {
        values.insert("source_type".into(), json!(APPLE));
        values.insert(
            "source_display".into(),
            json!(
                crate::imports::source_display(APPLE).unwrap_or_else(|| "Apple Health".to_owned())
            ),
        );
    }
    let Some((state, words)) = apple_row(root, import_id, all) else {
        return;
    };
    let status = match state {
        "saved" => "success",
        "failed" | "didnt_start" => "failed",
        _ => "noted",
    };
    values.insert("status".into(), json!(status));
    values.insert("body_state".into(), json!(state));
    values.insert("body_words".into(), json!(words));
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_import::publish::{
        DayMarkerOutcome, IndexPublicationOutcomes, IndexedFileError, PUBLICATION_SCHEMA,
        PublicationRecord, PublicationStatus, SegmentPublicationOutcome, write_publication_record,
    };
    use std::fs;

    const STREAM: &str = "import.apple_health";

    fn journal() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        fs::create_dir_all(journal.path().join("imports")).unwrap();
        journal
    }

    fn staged(root: &Path, id: &str) {
        let mut metadata = solstone_core_import::ImportMetadata::new();
        metadata.insert("source_hint".into(), json!(APPLE));
        solstone_core_import::write_import_metadata(root, id, &metadata).unwrap();
    }

    fn now_ms() -> u64 {
        u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
    }

    /// An import that ran, started at `started`, ended `Completed` or not, with
    /// its terminal record holding these segments.
    fn ran(
        root: &Path,
        id: &str,
        started: u64,
        completed: bool,
        record: Option<PublicationRecord>,
    ) {
        staged(root, id);
        let facts =
            solstone_core_import::admit_running_attempt(root, id, started, Some(APPLE)).unwrap();
        body_runs::stamp(root, id);
        if let Some(record) = record {
            write_publication_record(Some(&root.join("imports").join(id)), &record).unwrap();
        }
        if completed {
            solstone_core_import::record_completed_attempt(
                root,
                id,
                facts.generation,
                started + 1,
                None,
                None,
            )
            .unwrap();
        } else {
            solstone_core_import::record_unconfirmed_attempt(
                root,
                id,
                facts.generation,
                started + 1,
                Some(solstone_core_import::IMPORT_FAILED_REASON.to_owned()),
            )
            .unwrap();
        }
        solstone_core_import::metadata::release_attempt(root, id);
    }

    /// "your import of Mon D, H:MM AM ran after it", with no ISO date in it.
    fn names_the_import_as_history_does(words: &str) -> bool {
        let date = words
            .split("your import of ")
            .nth(1)
            .and_then(|rest| rest.split(" ran after it").next())
            .unwrap_or_default();
        !date.contains('-')
            && (date.ends_with(" AM") || date.ends_with(" PM"))
            && date.contains(", ")
    }

    fn segment(day: &str, key: &str, outcome: SegmentBindingOutcome) -> SegmentPublicationOutcome {
        SegmentPublicationOutcome {
            day: day.to_owned(),
            segment: key.to_owned(),
            stream: STREAM.to_owned(),
            outcome,
            body_only: true,
        }
    }

    fn bound() -> SegmentBindingOutcome {
        SegmentBindingOutcome::Bound {
            prev_day: None,
            prev_segment: None,
            seq: 1,
        }
    }

    fn record(
        segments: Vec<SegmentPublicationOutcome>,
        errored: Vec<&str>,
        failed_days: Vec<&str>,
    ) -> PublicationRecord {
        let failing = !errored.is_empty()
            || !failed_days.is_empty()
            || segments
                .iter()
                .any(|segment| !matches!(segment.outcome, SegmentBindingOutcome::Bound { .. }));
        PublicationRecord {
            schema: PUBLICATION_SCHEMA.to_owned(),
            status: if failing {
                PublicationStatus::Failure
            } else {
                PublicationStatus::Success
            },
            segments,
            indexing: IndexPublicationOutcomes {
                errored: errored
                    .into_iter()
                    .map(|path| IndexedFileError {
                        path: path.into(),
                        error: "database or disk is full".into(),
                    })
                    .collect(),
                ..IndexPublicationOutcomes::default()
            },
            day_markers: failed_days
                .into_iter()
                .map(|day| DayMarkerOutcome {
                    day: day.to_owned(),
                    outcome: DayMarkerStatus::Failed { error: "x".into() },
                })
                .collect(),
        }
    }

    fn clean() -> PublicationRecord {
        record(vec![], vec![], vec![])
    }

    fn row(root: &Path, id: &str) -> Option<(&'static str, String)> {
        apple_row(root, id, &apple_imports(root))
    }

    #[test]
    fn a_finished_import_reads_saved_or_partly_saved_by_its_own_failing_windows() {
        let journal = journal();
        let root = journal.path();
        // Carried windows that bound at finish aren't failing.
        ran(
            root,
            "20261010_100000",
            1_000,
            true,
            Some(record(
                vec![segment("20261009", "060000_3600", bound())],
                vec![],
                vec![],
            )),
        );
        assert_eq!(row(root, "20261010_100000").unwrap().0, "saved");
        // A failed binding, a search failure and a failed day marker each count.
        let failed = SegmentBindingOutcome::FailedAtAdvance { error: "x".into() };
        ran(
            root,
            "20261010_110000",
            2_000,
            true,
            Some(record(
                vec![segment("20261009", "070000_3600", failed)],
                vec![],
                vec![],
            )),
        );
        ran(
            root,
            "20261010_120000",
            3_000,
            true,
            Some(record(
                vec![segment("20261009", "080000_3600", bound())],
                vec!["chronicle/20261009/import.apple_health/080000_3600/body.jsonl"],
                vec![],
            )),
        );
        ran(
            root,
            "20261010_130000",
            4_000,
            true,
            Some(record(
                vec![
                    segment("20261008", "080000_3600", bound()),
                    segment("20261008", "090000_3600", bound()),
                ],
                vec![],
                vec!["20261008"],
            )),
        );
        let (state, words) = row(root, "20261010_110000").unwrap();
        assert_eq!(state, "partly_saved");
        assert!(
            words.starts_with("1 part of your journal didn't finish saving"),
            "{words}"
        );
        assert_eq!(row(root, "20261010_120000").unwrap().0, "partly_saved");
        let (state, words) = row(root, "20261010_130000").unwrap();
        assert_eq!(state, "partly_saved");
        assert!(words.starts_with("2 parts"), "{words}");
    }

    #[test]
    fn a_later_clean_import_saves_what_an_earlier_one_left_and_nothing_else_does() {
        let journal = journal();
        let root = journal.path();
        let failed = || SegmentBindingOutcome::FailedAtMarkerWrite { error: "x".into() };
        // A clean import that ran here before it, though its clock reads later
        // and its id sorts later: the clock went back between the two runs.
        ran(root, "20261010_130000", 9_000, true, Some(clean()));
        ran(
            root,
            "20261010_120000",
            5_000,
            true,
            Some(record(
                vec![segment("20261009", "070000_3600", failed())],
                vec![],
                vec![],
            )),
        );
        assert_eq!(row(root, "20261010_120000").unwrap().0, "partly_saved");
        // A clean import merged in from another journal, started later, its
        // own mark copied with it: it didn't run here, so it carried nothing.
        // Its number there is higher than anything here, so only the journal
        // it carries tells them apart.
        let other = super::tests::journal();
        for earlier in ["20261010_131000", "20261010_132000", "20261010_133000"] {
            ran(other.path(), earlier, 6_000, true, Some(clean()));
        }
        ran(other.path(), "20261010_135000", 6_500, true, Some(clean()));
        let merged = root.join("imports/20261010_135000");
        fs::create_dir_all(&merged).unwrap();
        for entry in fs::read_dir(other.path().join("imports/20261010_135000")).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                fs::copy(entry.path(), merged.join(entry.file_name())).unwrap();
            }
        }
        assert_eq!(
            row(root, "20261010_135000").unwrap().0,
            "saved",
            "merged in whole"
        );
        assert_eq!(
            row(root, "20261010_120000").unwrap().0,
            "partly_saved",
            "a merged clean import saves nothing here"
        );
        // A later one that itself left hours doesn't either.
        ran(
            root,
            "20261010_140000",
            6_000,
            true,
            Some(record(
                vec![segment("20261009", "090000_3600", failed())],
                vec![],
                vec![],
            )),
        );
        assert_eq!(row(root, "20261010_120000").unwrap().0, "partly_saved");
        // Nor one whose record can't be read.
        ran(root, "20261010_150000", 7_000, true, Some(clean()));
        fs::write(
            root.join("imports/20261010_150000/imported.json"),
            b"not json",
        )
        .unwrap();
        assert_eq!(row(root, "20261010_120000").unwrap().0, "partly_saved");
        assert_eq!(
            row(root, "20261010_150000"),
            None,
            "its own unreadable record: HEAD's status unavailable"
        );
        // A later clean one does: every run retries what earlier runs left.
        ran(root, "20261010_160000", 8_000, true, Some(clean()));
        assert_eq!(row(root, "20261010_120000").unwrap().0, "saved");
        assert_eq!(row(root, "20261010_140000").unwrap().0, "saved");
    }

    #[test]
    fn a_partly_saved_import_ends_as_finished_and_a_stopped_one_in_its_words() {
        use crate::lifecycle::{ExitEvent, exit_event_for};
        let journal = journal();
        let root = journal.path();
        let failed = || SegmentBindingOutcome::FailedAtAdvance { error: "x".into() };
        ran(
            root,
            "20261010_120000",
            1_000,
            true,
            Some(record(
                vec![segment("20261009", "070000_3600", failed())],
                vec![],
                vec![],
            )),
        );
        assert_eq!(row(root, "20261010_120000").unwrap().0, "partly_saved");
        assert!(
            matches!(
                exit_event_for(root, "20261010_120000"),
                Some(ExitEvent::Completed(_))
            ),
            "what it saved is there: the page isn't told it failed"
        );
        ran(root, "20261010_130000", 2_000, false, None);
        body_status::record(root, "20261010_130000", "permission", 1);
        match exit_event_for(root, "20261010_130000") {
            Some(ExitEvent::Error(error)) => {
                assert_eq!(error.stage, "stopped");
                assert_eq!(
                    error.error,
                    body_status::words("permission"),
                    "no remedy it can't keep"
                );
            }
            _ => panic!("a stop is told as one"),
        }
    }

    #[test]
    fn an_import_that_didnt_finish_never_reads_failed_unless_it_was_refused() {
        let journal = journal();
        let root = journal.path();
        // Stopped, with nothing recorded: the general words.
        ran(root, "20261010_120000", 5_000, false, None);
        let (state, words) = row(root, "20261010_120000").unwrap();
        assert_eq!(state, "stopped");
        assert_eq!(
            words,
            "it stopped before it finished. import the export again to finish."
        );
        // Stopped on a full disk, for this attempt.
        body_status::record(root, "20261010_120000", "disk_full", 1);
        assert!(
            row(root, "20261010_120000")
                .unwrap()
                .1
                .starts_with("the disk filled up")
        );
        // A reason from another generation is ignored.
        body_status::record(root, "20261010_120000", "export_incomplete", 2);
        assert_eq!(row(root, "20261010_120000").unwrap().0, "stopped");
        // A later finished import names itself.
        ran(root, "20261010_130000", 6_000, true, Some(clean()));
        let (state, words) = row(root, "20261010_120000").unwrap();
        assert_eq!(state, "stopped");
        assert!(words.contains("ran after it"), "{words}");
        // In the form the history's own column uses.
        assert!(names_the_import_as_history_does(&words), "{words}");
        // Refused after it was admitted: failed, in the refusal's words.
        ran(root, "20261010_140000", 7_000, false, None);
        body_status::record(root, "20261010_140000", "export_incomplete", 1);
        let (state, words) = row(root, "20261010_140000").unwrap();
        assert_eq!(state, "failed");
        assert_eq!(words, body_status::words("export_incomplete"));
    }

    #[test]
    fn a_refusal_before_it_ran_reads_as_itself_even_while_its_task_reads_running() {
        let journal = journal();
        let root = journal.path();
        for (id, code, state) in [
            ("20261010_120000", "go_ahead", "failed"),
            ("20261010_130000", "busy", "didnt_start"),
            ("20261010_140000", "no_room", "failed"),
        ] {
            staged(root, id);
            let mut metadata = read_import_metadata(root, id).unwrap();
            metadata.insert("task_id".into(), json!(now_ms().to_string()));
            solstone_core_import::write_import_metadata(root, id, &metadata).unwrap();
            assert_eq!(row(root, id), None, "{id}: queued, the page's own reading");
            body_status::record(root, id, code, 1);
            assert_eq!(row(root, id).unwrap().0, state, "{id}");
        }
    }

    #[test]
    fn a_stopped_import_names_the_run_that_carried_it_on_before_any_inference() {
        let journal = journal();
        let root = journal.path();
        ran(root, "20261010_120000", 5_000, false, None);
        // A clean import ran after it in this journal's order...
        ran(root, "20261010_130000", 6_000, true, Some(clean()));
        // ...but the takeover that carried it on, recorded on its attempt, is
        // another, started at a later time.
        ran(root, "20261010_140000", 9_000_000, true, Some(clean()));
        let carried = |successor: &str| {
            solstone_core_import::metadata::record_continued_attempt_unlocked(
                root,
                "20261010_120000",
                1,
                5_001,
                successor,
            )
            .unwrap();
        };
        carried("20261010_140000");
        let (state, words) = row(root, "20261010_120000").unwrap();
        assert_eq!(state, "stopped");
        assert!(
            words.starts_with("stopped before it finished. carried on by the import of "),
            "{words}"
        );
        assert!(
            words.contains(&when(root, 9_000_000)),
            "names the recorded successor: {words}"
        );
        // A successor that isn't here any more falls back to the run order.
        carried("20261010_150000");
        assert!(
            row(root, "20261010_120000")
                .unwrap()
                .1
                .contains("ran after it")
        );
    }

    #[test]
    fn an_apple_upload_is_named_for_what_it_is_while_it_runs_or_waits() {
        let journal = journal();
        let root = journal.path();
        staged(root, "20261010_120000");
        // The row as the list builds it: no hint, the upload's generic kind.
        let mut values = serde_json::Map::new();
        values.insert("source_type".into(), json!("text"));
        values.insert("source_display".into(), json!("Text"));
        overlay(root, "20261010_120000", &mut values, &apple_imports(root));
        assert_eq!(values["source_type"], APPLE);
        assert_eq!(values["source_display"], "Apple Health");
        assert!(
            values.get("body_state").is_none(),
            "pending keeps the page's own reading"
        );
    }

    #[test]
    fn rows_it_cant_place_keep_the_pages_own_reading() {
        let journal = journal();
        let root = journal.path();
        staged(root, "20261010_120000");
        assert_eq!(row(root, "20261010_120000"), None, "staged, never started");
        let mut metadata = solstone_core_import::ImportMetadata::new();
        metadata.insert("source_hint".into(), json!("strava"));
        solstone_core_import::write_import_metadata(root, "20261010_130000", &metadata).unwrap();
        assert_eq!(row(root, "20261010_130000"), None, "not Apple Health");
        let mut values = Map::new();
        values.insert("status".into(), json!("pending"));
        overlay(root, "20261010_120000", &mut values, &apple_imports(root));
        assert_eq!(values["status"], "pending");
    }

    #[test]
    fn what_isnt_a_window_doesnt_count_and_a_missing_record_says_nothing_is_saved() {
        let journal = journal();
        let root = journal.path();
        // Only the summary couldn't be searched: the record says failure, the hours are saved.
        let mut summary_only = clean();
        summary_only.status = PublicationStatus::Failure;
        summary_only.indexing.errored = vec![IndexedFileError {
            path: "imports/20261010_120000/summary.md".into(),
            error: "x".into(),
        }];
        ran(
            root,
            "20261010_110000",
            1_000,
            true,
            Some(record(
                vec![segment(
                    "20261009",
                    "070000_3600",
                    SegmentBindingOutcome::FailedAtAdvance { error: "x".into() },
                )],
                vec![],
                vec![],
            )),
        );
        ran(root, "20261010_120000", 2_000, true, Some(summary_only));
        assert_eq!(row(root, "20261010_120000").unwrap().0, "saved");
        assert_eq!(
            row(root, "20261010_110000").unwrap().0,
            "saved",
            "and it counts as clean"
        );
        // A search failure named from another journal's root is still that window's.
        ran(
            root,
            "20261010_130000",
            3_000,
            true,
            Some(record(
                vec![segment("20261009", "080000_3600", bound())],
                vec![
                    "/elsewhere/journal/chronicle/20261009/import.apple_health/080000_3600/body.jsonl",
                ],
                vec![],
            )),
        );
        assert_eq!(row(root, "20261010_130000").unwrap().0, "partly_saved");
        // A window the owner deleted on a day whose marker failed isn't counted.
        ran(
            root,
            "20261010_140000",
            4_000,
            true,
            Some(record(
                vec![
                    segment("20261008", "080000_3600", bound()),
                    segment(
                        "20261008",
                        "090000_3600",
                        SegmentBindingOutcome::OwnerDeleted,
                    ),
                ],
                vec![],
                vec!["20261008"],
            )),
        );
        assert!(
            row(root, "20261010_140000")
                .unwrap()
                .1
                .starts_with("1 part")
        );
        // A finished import with no record at all can't say it saved anything.
        ran(root, "20261010_150000", 5_000, true, None);
        let (state, words) = row(root, "20261010_150000").unwrap();
        assert_eq!(state, "partly_saved");
        assert!(words.starts_with("some parts"), "{words}");
    }
}
