// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A stream's activities are written once its evidence has settled, on a
//! journal clock: segments arrive and are thought at set times, and every
//! file carries the time it was written.

use super::*;

const DAY: &str = "20260813";
const MIDNIGHT: i64 = 1_786_579_200_000;
const MINUTE: i64 = 60_000;
const STREAM: &str = "watch";

fn at(hour: i64, minute: i64) -> i64 {
    MIDNIGHT + (hour * 60 + minute) * MINUTE
}

fn work() -> Value {
    serde_json::json!({"density":"active","content_type":"work","activity_summary":"work","facets":[{"facet":"work","level":"high","activity":"work"}]})
}

fn meeting() -> Value {
    serde_json::json!({"density":"active","content_type":"meeting","activity_summary":"meeting","facets":[{"facet":"work","level":"high","activity":"meeting"}]})
}

struct Bed {
    _dir: tempfile::TempDir,
    _roots: tempfile::TempDir,
    journal: std::path::PathBuf,
    talent_root: std::path::PathBuf,
    apps_root: std::path::PathBuf,
    recorder: Arc<Recorder>,
}

/// A journal in UTC with one facet and one activity talent; the window is on.
fn bed() -> Bed {
    crate::settle::settle_at_once(false);
    let dir = tempdir().unwrap();
    let roots = tempdir().unwrap();
    let journal = dir.path().to_path_buf();
    fs::create_dir_all(journal.join("config")).unwrap();
    fs::write(
        journal.join("config/journal.json"),
        r#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"openai","model":"test-model"}}}"#,
    )
    .unwrap();
    solstone_core_facets::create_facet(&journal, "work", "Work", "", "", "", None).unwrap();
    let (talent_root, apps_root) = talent_roots(
        roots.path(),
        &[(
            "participation",
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"activity\",\"priority\":1,\"output\":\"json\",\"activities\":[\"work\"]\n}",
        )],
    );
    Bed {
        _dir: dir,
        _roots: roots,
        journal,
        talent_root,
        apps_root,
        recorder: Arc::new(Recorder::default()),
    }
}

impl Bed {
    fn context(&self, day: &str, now: i64) -> context::ThinkContext {
        let day_dir = day::create_day(&self.journal, day).unwrap();
        context::ThinkContext::new(&self.journal, day.to_owned(), day_dir, now)
            .unwrap()
            .with_boundary(self.recorder.clone())
            .with_talent_roots(self.talent_root.clone(), self.apps_root.clone())
    }

    fn segment(&self, day: &str, key: &str) -> std::path::PathBuf {
        self.journal
            .join("chronicle")
            .join(day)
            .join(STREAM)
            .join(key)
    }

    /// The segment arrives at `arrived`, is thought a minute later, and its
    /// live tail runs then.
    fn deliver(&self, day: &str, key: &str, sense: &Value, arrived: i64) {
        let dir = self.segment(day, key);
        fs::create_dir_all(dir.join("talents")).unwrap();
        let input = dir.join("audio.flac");
        fs::write(&input, b"audio").unwrap();
        set_ms(&input, arrived);
        let thought = arrived + MINUTE;
        let sense_path = dir.join("talents/sense.json");
        fs::write(&sense_path, serde_json::to_vec(sense).unwrap()).unwrap();
        set_ms(&sense_path, thought);
        let context = self.context(day, thought);
        let mut log = test_log(&context, "segment");
        segment::replay_activity_state(
            &context,
            &mut log,
            &[(key.to_owned(), Some(STREAM.to_owned()))],
            false,
            1,
            false,
            true,
        )
        .unwrap();
    }

    /// The supervisor's settle check at `now`.
    fn settle(&self, now: i64) {
        let context = self.context(DAY, now);
        let mut log = test_log(&context, "settle");
        crate::settle::run(&context, &mut log, Some(STREAM), 1, false).unwrap();
    }

    /// Every record of the day: its ID and segments.
    fn records(&self, day: &str) -> Vec<(String, Vec<String>)> {
        solstone_core_facets::load_activity_records(&self.journal, "work", day, true)
            .unwrap()
            .into_iter()
            .map(|record| {
                (
                    record["id"].as_str().unwrap().to_owned(),
                    record["segments"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|segment| segment.as_str().unwrap().to_owned())
                        .collect(),
                )
            })
            .collect()
    }

    fn talent_requests(&self) -> usize {
        self.recorder
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.name == "participation")
            .count()
    }
}

fn set_ms(path: &Path, ms: i64) {
    filetime::set_file_mtime(
        path,
        filetime::FileTime::from_unix_time(ms / 1000, ((ms % 1000) * 1_000_000) as u32),
    )
    .unwrap();
}

fn keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| (*key).to_owned()).collect()
}

/// Three live segments, then the seven that follow them relayed hours late,
/// newest first, three minutes apart: they span longer than the window, and
/// until the fifth arrives the stream shows a gap after the live three.
fn newest_first_backlog(bed: &Bed, check: &dyn Fn(&Bed, &str)) {
    for (key, arrived) in [
        ("090000_300", at(9, 6)),
        ("090500_300", at(9, 11)),
        ("091000_300", at(9, 16)),
    ] {
        bed.deliver(DAY, key, &work(), arrived);
    }
    for (key, arrived) in [
        ("094500_300", at(10, 30)),
        ("094000_300", at(10, 33)),
        ("093500_300", at(10, 36)),
        ("093000_300", at(10, 39)),
        ("092500_300", at(10, 42)),
        ("092000_300", at(10, 45)),
        ("091500_300", at(10, 48)),
    ] {
        bed.deliver(DAY, key, &work(), arrived);
        check(bed, key);
    }
}

#[test]
fn a_newest_first_backlog_across_a_gap_is_written_as_one_activity() {
    let bed = bed();
    newest_first_backlog(&bed, &|bed, key| {
        assert!(
            bed.records(DAY).is_empty(),
            "nothing is written once {key} is thought"
        );
    });
    // Once the backlog is in, nothing is written while the stream is not yet
    // quiet: the activity is still open.
    bed.settle(at(10, 55));
    assert!(bed.records(DAY).is_empty());
    // An hour after the stream went quiet, with no further segment, it is.
    bed.settle(at(11, 50));
    assert_eq!(
        bed.records(DAY),
        [(
            "work_090000_300".to_owned(),
            keys(&[
                "090000_300",
                "090500_300",
                "091000_300",
                "091500_300",
                "092000_300",
                "092500_300",
                "093000_300",
                "093500_300",
                "094000_300",
                "094500_300"
            ])
        )]
    );
    assert_eq!(bed.talent_requests(), 1);
}

/// With no window the same backlog writes the activity before its backlog
/// has arrived: the newest segment shows a gap the backlog is about to fill.
#[test]
fn with_no_window_the_backlog_is_written_before_it_has_arrived() {
    let bed = bed();
    crate::settle::settle_at_once(true);
    let first = std::cell::RefCell::new(None);
    newest_first_backlog(&bed, &|bed, _| {
        first.borrow_mut().get_or_insert_with(|| bed.records(DAY));
    });
    assert_eq!(
        first.into_inner().unwrap(),
        [(
            "work_090000_300".to_owned(),
            keys(&["090000_300", "090500_300", "091000_300"])
        )]
    );
}

#[test]
fn a_lost_pending_file_restarts_the_clocks_and_drops_nothing() {
    let bed = bed();
    newest_first_backlog(&bed, &|_, _| {});
    fs::remove_dir_all(bed.journal.join("awareness/activity_settle")).unwrap();
    bed.settle(at(11, 50));
    assert_eq!(bed.records(DAY).len(), 1);
    assert_eq!(bed.records(DAY)[0].1.len(), 10);
}

#[test]
fn the_supervisor_is_told_when_the_next_check_is_due() {
    let bed = bed();
    bed.deliver(DAY, "090000_300", &work(), at(9, 6));
    // The stream's open activity closes an hour after it goes quiet.
    assert!(crate::activity_settle_due(&bed.journal, at(10, 6)).is_empty());
    assert_eq!(
        crate::activity_settle_due(&bed.journal, at(10, 8)),
        [STREAM]
    );
    bed.settle(at(10, 8));
    assert_eq!(bed.records(DAY).len(), 1);
    assert!(crate::activity_settle_due(&bed.journal, i64::MAX).is_empty());
}

#[test]
fn a_backlog_of_the_previous_day_is_written_once_it_settles() {
    let bed = bed();
    let yesterday = "20260812";
    bed.deliver(DAY, "000500_300", &work(), at(0, 11));
    for (key, arrived) in [
        ("231000_300", at(8, 0)),
        ("230500_300", at(8, 2)),
        ("230000_300", at(8, 4)),
    ] {
        bed.deliver(yesterday, key, &work(), arrived);
        assert!(bed.records(yesterday).is_empty());
    }
    bed.settle(at(8, 11));
    assert_eq!(
        bed.records(yesterday),
        [(
            "work_230000_300".to_owned(),
            keys(&["230000_300", "230500_300", "231000_300"])
        )]
    );
}

#[test]
fn continuous_capture_does_not_hold_back_an_activity_that_has_settled() {
    let bed = bed();
    let mut minute = 0;
    while minute <= 90 {
        let key = format!("{:02}{:02}00_300", 9 + minute / 60, minute % 60);
        let sense = if minute < 30 { work() } else { meeting() };
        bed.deliver(DAY, &key, &sense, at(9, minute + 6));
        minute += 5;
    }
    // The work activity ended at 09:41, when the second meeting segment was
    // thought, and has been written while capture went on.
    let records = bed.records(DAY);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].0, "work_090000_300");
    assert_eq!(records[0].1.len(), 7);
    let written =
        solstone_core_facets::get_activity_record(&bed.journal, "work", DAY, "work_090000_300")
            .unwrap()
            .unwrap();
    let settled_at = written["settled_at"].as_i64().unwrap();
    assert!(
        settled_at <= at(9, 56),
        "written {} min after 09:41",
        (settled_at - at(9, 41)) / MINUTE
    );
}

#[test]
fn a_record_written_before_this_publisher_is_never_extended_or_overlapped() {
    let bed = bed();
    // A fragment the earlier publisher wrote, under an ID the rebuilt
    // activity does not have.
    let mut seeded = Map::new();
    seeded.insert("id".to_owned(), Value::from("work_090500_300"));
    seeded.insert("activity".to_owned(), Value::from("work"));
    seeded.insert("facet".to_owned(), Value::from("work"));
    seeded.insert("stream".to_owned(), Value::from(STREAM));
    seeded.insert(
        "segments".to_owned(),
        serde_json::json!(["090500_300", "091000_300"]),
    );
    let _ =
        solstone_core_facets::append_activity_record(&bed.journal, "work", DAY, seeded).unwrap();
    let path = bed
        .journal
        .join(format!("facets/work/activities/{DAY}.jsonl"));
    let before = fs::read(&path).unwrap();
    for (key, arrived) in [
        ("090000_300", at(9, 6)),
        ("090500_300", at(9, 11)),
        ("091000_300", at(9, 16)),
    ] {
        bed.deliver(DAY, key, &work(), arrived);
    }
    bed.settle(at(11, 0));
    bed.settle(at(23, 59));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(bed.talent_requests(), 0);
}

#[test]
fn an_activity_this_publisher_wrote_grows_when_later_evidence_extends_it() {
    let bed = bed();
    // Two segments, a capture gap, then the stream's open activity.
    bed.deliver(DAY, "090000_300", &work(), at(9, 6));
    bed.deliver(DAY, "090500_300", &work(), at(9, 11));
    bed.deliver(DAY, "093000_300", &work(), at(9, 36));
    bed.settle(at(9, 43));
    assert_eq!(
        bed.records(DAY),
        [(
            "work_090000_300".to_owned(),
            keys(&["090000_300", "090500_300"])
        )]
    );
    assert_eq!(bed.talent_requests(), 1);
    // The gap's segments arrive two hours later and join both stretches into
    // one activity under the same ID.
    for (key, arrived) in [
        ("092500_300", at(11, 0)),
        ("092000_300", at(11, 1)),
        ("091500_300", at(11, 2)),
        ("091000_300", at(11, 3)),
    ] {
        bed.deliver(DAY, key, &work(), arrived);
    }
    bed.settle(at(12, 5));
    let records = bed.records(DAY);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].0, "work_090000_300");
    assert_eq!(records[0].1.len(), 7);
    let grown =
        solstone_core_facets::get_activity_record(&bed.journal, "work", DAY, "work_090000_300")
            .unwrap()
            .unwrap();
    let edit = grown["edits"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(edit["actor"], "activity");
    let mut fields = edit["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|field| field.as_str().unwrap())
        .collect::<Vec<_>>();
    fields.sort_unstable();
    assert_eq!(fields, ["active_entities", "level_avg", "segments"]);
    // Its talents read the new evidence.
    assert_eq!(bed.talent_requests(), 2);
}

#[test]
fn a_check_that_writes_nothing_logs_nothing() {
    let bed = bed();
    bed.deliver(DAY, "090000_300", &work(), at(9, 6));
    bed.deliver(DAY, "090500_300", &work(), at(9, 11));
    bed.deliver(DAY, "093000_300", &work(), at(9, 36));
    for minute in [43, 44, 50, 59] {
        bed.settle(at(9, minute));
    }
    let detected = oplog_records(&bed.journal, DAY, "settle")
        .into_iter()
        .filter(|event| event["event"] == "activity.detected")
        .count();
    assert_eq!(detected, 1);
    assert_eq!(bed.records(DAY).len(), 1);
}

#[test]
fn a_burst_of_late_pieces_grows_an_activity_once() {
    let bed = bed();
    let idle = serde_json::json!({"density":"idle","content_type":"idle","facets":[]});
    bed.deliver(DAY, "090000_300", &work(), at(9, 6));
    bed.deliver(DAY, "091500_300", &work(), at(9, 21));
    bed.deliver(DAY, "092000_300", &idle, at(9, 26));
    bed.settle(at(9, 33));
    assert_eq!(bed.records(DAY)[0].1, keys(&["090000_300", "091500_300"]));
    assert_eq!(bed.talent_requests(), 1);
    // The two missing pieces arrive two minutes apart, long after.
    bed.deliver(DAY, "091000_300", &work(), at(11, 0));
    bed.deliver(DAY, "090500_300", &work(), at(11, 2));
    bed.settle(at(11, 10));
    assert_eq!(
        bed.records(DAY)[0].1,
        keys(&["090000_300", "090500_300", "091000_300", "091500_300"])
    );
    assert_eq!(bed.talent_requests(), 2, "one revision for the whole burst");
}
