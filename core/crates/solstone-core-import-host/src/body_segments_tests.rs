// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods)]

use std::fs;

use serde_json::json;
use solstone_core_format::content::body_records::decode_rows;
use solstone_core_journal_io::{HealthMarkerKind, HealthMarkerState, read_health_marker};
use solstone_core_retention::door;
use solstone_core_retention::tombstone::RemovalReason;

use super::*;

const SOURCE: &str = r#"sourceName="Synthetic Watch" creationDate="2026-10-10 08:00:00 -0600""#;

fn heart(at: &str, value: &str) -> String {
    format!(
        r#"<Record type="HKQuantityTypeIdentifierHeartRate" {SOURCE} unit="count/min" startDate="{at}" endDate="{at}" value="{value}"/>"#
    )
}

fn export(directory: &Path, records: &[String]) -> PathBuf {
    fs::create_dir_all(directory).unwrap();
    fs::write(
        directory.join("export.xml"),
        format!("<HealthData>{}</HealthData>", records.join("")),
    )
    .unwrap();
    directory.to_path_buf()
}

fn journal() -> tempfile::TempDir {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        json!({"identity": {"timezone": "America/Denver"}}).to_string(),
    )
    .unwrap();
    for facet in ["health", "other"] {
        let dir = root.join("facets").join(facet);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("facet.json"),
            json!({"title": facet, "description": facet}).to_string(),
        )
        .unwrap();
    }
    journal
}

fn run(root: &Path, source: &Path, timestamp: &str, facet: Option<&str>) -> AppleSegmentsOutcome {
    run_apple_health_segments(root, &AppleSegmentsRequest::new(source, timestamp, facet)).unwrap()
}

fn window(root: &Path, day: &str, key: &str) -> PathBuf {
    root.join("chronicle").join(day).join(STREAM).join(key)
}

fn body_bytes(root: &Path, day: &str, key: &str) -> Vec<u8> {
    fs::read(window(root, day, key).join(BODY_RECORDS_FILE)).unwrap()
}

fn stream_marker_absent(root: &Path, day: &str) -> bool {
    matches!(
        read_health_marker(root, day, HealthMarkerKind::Stream).unwrap(),
        HealthMarkerState::Absent
    )
}

#[test]
fn an_export_publishes_windows_that_ask_for_no_thinking() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(
        scratch.path(),
        &[
            heart("2026-10-10 06:31:10 -0600", "70"),
            heart("2019-01-02 06:31:10 -0700", "71"),
        ],
    );
    let outcome = run(root, &source, "20261012_120000", None);
    assert!(outcome.finished);
    assert_eq!(outcome.published, 2);
    assert!(outcome.summary.contains("no facet"));
    assert!(!root.join(RUN_DIR_REL).exists());

    // The real writer's own lock file doesn't stop it being body-only.
    let recent = window(root, "20261010", "060000_3600");
    assert!(
        recent.join("body.jsonl.lock").exists(),
        "the writer leaves a lock"
    );
    assert!(recent.join("stream.json").is_file());
    assert!(solstone_core_segment::is_body_only_dir(STREAM, &recent));
    assert!(solstone_core_segment::is_body_only_dir(
        STREAM,
        &window(root, "20190102", "060000_3600")
    ));

    // Recent and old alike: no thinking scope, no stream marker, a body marker.
    let imports = solstone_core_import::thinking::read(root).unwrap();
    assert!(imports.imports.is_empty() && imports.unreadable.is_empty());
    let record = read_publication_record(&root.join("imports").join(&outcome.import_id))
        .unwrap()
        .unwrap();
    assert_eq!(record.status, PublicationStatus::Success);
    assert!(record.segments.iter().all(|segment| segment.body_only));
    for day in ["20261010", "20190102"] {
        assert!(stream_marker_absent(root, day));
        assert!(matches!(
            read_health_marker(root, day, HealthMarkerKind::Body).unwrap(),
            HealthMarkerState::Versioned { .. }
        ));
    }
    assert!(
        root.join("imports")
            .join(&outcome.import_id)
            .join("summary.md")
            .is_file()
    );
}

#[test]
fn the_same_export_changes_nothing_an_edit_updates_and_a_deletion_stays() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let records = [
        heart("2026-10-10 06:31:10 -0600", "70"),
        heart("2026-10-10 07:01:10 -0600", "75"),
    ];
    let source = export(scratch.path(), &records);
    run(root, &source, "20261012_120000", None);
    let before = body_bytes(root, "20261010", "060000_3600");
    let marker = read_health_marker(root, "20261010", HealthMarkerKind::Body).unwrap();

    let again = run(root, &source, "20261012_120001", None);
    assert_eq!(again.published, 0, "{}", again.summary);
    assert_eq!(body_bytes(root, "20261010", "060000_3600"), before);
    assert_eq!(
        read_health_marker(root, "20261010", HealthMarkerKind::Body).unwrap(),
        marker
    );

    // An edited value: same record, new value, one window publishes.
    let edited = export(
        &scratch.path().join("edited"),
        &[
            heart("2026-10-10 06:31:10 -0600", "99"),
            heart("2026-10-10 07:01:10 -0600", "75"),
        ],
    );
    let updated = run(root, &edited, "20261012_120002", None);
    assert_eq!(updated.published, 1);
    let rows =
        decode_rows(&String::from_utf8(body_bytes(root, "20261010", "060000_3600")).unwrap())
            .unwrap();
    assert_eq!(rows.len(), 1, "replaced, not added beside");
    assert_eq!(rows[0].value.as_deref(), Some("99"));

    // An export that omits a record isn't a deletion.
    let partial = export(
        &scratch.path().join("partial"),
        &[heart("2026-10-10 06:31:10 -0600", "99")],
    );
    assert_eq!(run(root, &partial, "20261012_120003", None).published, 0);
    assert!(
        window(root, "20261010", "070000_3600")
            .join(BODY_RECORDS_FILE)
            .is_file()
    );

    // Deleted through the owner's door, it stays deleted and the summary survives.
    let outcome = door::remove_segments(
        root,
        &[solstone_core_retention::Target {
            day: "20261010".to_owned(),
            stream: STREAM.to_owned(),
            dir: "060000_3600".to_owned(),
        }],
        &chrono::Utc::now().to_rfc3339(),
        RemovalReason::OwnerSegmentDelete,
        "test",
    );
    assert!(outcome.halted.is_none());
    let tombstoned = window(root, "20261010", "060000_3600");
    assert!(solstone_core_segment::is_body_only_dir(STREAM, &tombstoned));
    let after = run(root, &source, "20261012_120004", None);
    assert_eq!(after.published, 0);
    assert!(
        after.summary.contains("1 stayed deleted"),
        "{}",
        after.summary
    );
    assert!(after.summary.contains("heart rate: 2"), "{}", after.summary);
    assert!(!tombstoned.join(BODY_RECORDS_FILE).exists());
}

#[test]
fn a_deleted_card_stays_deleted_and_new_rows_land_beside_it() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let delete = |key: &str| {
        let outcome = door::remove_segments(
            root,
            &[solstone_core_retention::Target {
                day: "20261010".to_owned(),
                stream: STREAM.to_owned(),
                dir: key.to_owned(),
            }],
            &chrono::Utc::now().to_rfc3339(),
            RemovalReason::OwnerSegmentDelete,
            "test",
        );
        assert!(outcome.halted.is_none());
    };
    let card = window(root, "20261010", "000000_300");
    fs::create_dir_all(&card).unwrap();
    fs::write(card.join("day_summary_transcript.md"), "a card\n").unwrap();
    delete("000000_300");
    assert!(card.join("tombstone.json").is_file());
    assert!(!solstone_core_segment::is_body_only_dir(STREAM, &card));
    assert!(!solstone_core_segment::day_holds_nothing_the_owner_did(
        root, "20261010"
    ));

    let source = export(
        scratch.path(),
        &[
            // A session at midnight puts itself and its own readings on the slot
            // the card held.
            format!(
                r#"<Record type="HKCategoryTypeIdentifierMindfulSession" {SOURCE} startDate="2026-10-10 00:00:30 -0600" endDate="2026-10-10 00:04:30 -0600" value="HKCategoryValueNotApplicable"/>"#
            ),
            heart("2026-10-10 00:01:00 -0600", "70"),
            heart("2026-10-10 00:04:00 -0600", "71"),
        ],
    );
    let first = run(root, &source, "20261012_120000", None);
    assert_eq!(first.published, 1);
    assert!(
        first.summary.contains("0 stayed deleted"),
        "{}",
        first.summary
    );
    let beside = window(root, "20261010", "000000_301");
    assert_eq!(
        decode_rows(&fs::read_to_string(beside.join(BODY_RECORDS_FILE)).unwrap())
            .unwrap()
            .len(),
        3
    );
    assert!(!card.join(BODY_RECORDS_FILE).exists());

    // Nothing moves on the same export, and the deleted card stays what it was.
    let again = run(root, &source, "20261012_120001", None);
    assert_eq!(again.published, 0);
    assert!(!card.join(BODY_RECORDS_FILE).exists());

    // The window beside it, once deleted, stays deleted too.
    delete("000000_301");
    assert!(solstone_core_segment::is_body_only_dir(STREAM, &beside));
    let gone = run(root, &source, "20261012_120002", None);
    assert_eq!(gone.published, 0);
    assert!(
        gone.summary.contains("1 stayed deleted"),
        "{}",
        gone.summary
    );
    assert!(!beside.join(BODY_RECORDS_FILE).exists());
    assert!(!card.join(BODY_RECORDS_FILE).exists());
}

#[test]
fn a_night_never_shrinks_and_its_tail_rewrites_every_tile() {
    let stage = |start: &str, end: &str, value: &str| {
        format!(
            r#"<Record type="HKCategoryTypeIdentifierSleepAnalysis" {SOURCE} startDate="{start}" endDate="{end}" value="HKCategoryValueSleepAnalysis{value}"/>"#
        )
    };
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let whole = [
        stage(
            "2026-10-10 23:50:00 -0600",
            "2026-10-11 00:10:00 -0600",
            "AsleepCore",
        ),
        stage(
            "2026-10-11 00:10:00 -0600",
            "2026-10-11 00:20:00 -0600",
            "AsleepDeep",
        ),
    ];
    let source = export(scratch.path(), &whole);
    run(root, &source, "20261012_120000", None);
    let first = body_bytes(root, "20261010", "230000_3600");

    let fewer = export(&scratch.path().join("fewer"), &whole[..1]);
    assert_eq!(run(root, &fewer, "20261012_120001", None).published, 0);
    assert_eq!(body_bytes(root, "20261010", "230000_3600"), first);

    let longer = [
        whole[0].clone(),
        whole[1].clone(),
        stage(
            "2026-10-11 00:20:00 -0600",
            "2026-10-11 00:30:00 -0600",
            "AsleepREM",
        ),
    ];
    let longer = export(&scratch.path().join("longer"), &longer);
    run(root, &longer, "20261012_120002", None);
    // A night sits on hours: its tile past midnight is rewritten to the new end.
    let (day, key) = ("20261011", "000000_3600");
    let rows = decode_rows(&String::from_utf8(body_bytes(root, day, key)).unwrap()).unwrap();
    let tiles: Vec<_> = rows
        .iter()
        .filter(|row| row.of.is_some() && row.index.is_some())
        .collect();
    assert_eq!(
        tiles.len(),
        1,
        "{day}/{key}: no old tile left beside the new one"
    );
    assert_eq!(tiles[0].count, Some(2));
    assert_eq!(tiles[0].end.as_deref(), Some("2026-10-11 00:30:00 -0600"));
}

#[test]
fn a_chosen_facet_is_written_refused_when_muted_and_a_change_republishes() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(scratch.path(), &[heart("2026-10-10 06:31:10 -0600", "70")]);
    let refused = run_apple_health_segments(
        root,
        &AppleSegmentsRequest::new(&source, "20261012_120000", Some("absent")),
    );
    assert!(refused.is_err());
    assert!(
        !root.join("chronicle").exists(),
        "refused before anything is written"
    );

    let first = run(root, &source, "20261012_120001", Some("health"));
    assert_eq!(first.published, 1);
    let dir = window(root, "20261010", "060000_3600");
    assert_eq!(
        fs::read_to_string(dir.join("talents/facets.json")).unwrap(),
        facet_file_text("health")
    );
    assert!(solstone_core_segment::is_body_only_dir(STREAM, &dir));

    // No facet named: what's there stays.
    assert_eq!(run(root, &source, "20261012_120002", None).published, 0);
    assert!(dir.join("talents/facets.json").is_file());

    // Another facet: identical rows, but it publishes so search reclassifies it.
    let changed = run(root, &source, "20261012_120003", Some("other"));
    assert_eq!(changed.published, 1);
    assert_eq!(
        fs::read_to_string(dir.join("talents/facets.json")).unwrap(),
        facet_file_text("other")
    );
}

#[test]
fn a_run_that_didnt_finish_is_repaired_by_the_next_whatever_it_holds() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(scratch.path(), &[heart("2026-10-10 06:31:10 -0600", "70")]);
    run(root, &source, "20261012_120000", None);

    // A window a stopped run wrote: on disk, never bound.
    let orphan = window(root, "20261009", "120000_300");
    fs::create_dir_all(&orphan).unwrap();
    let orphan_row =
        fs::read(window(root, "20261010", "060000_3600").join(BODY_RECORDS_FILE)).unwrap();
    fs::write(orphan.join(BODY_RECORDS_FILE), &orphan_row).unwrap();
    // A run whose state can't be read.
    fs::create_dir_all(root.join(RUN_DIR_REL)).unwrap();
    fs::write(root.join(RUN_DIR_REL).join("state.json"), b"{not json").unwrap();

    // The next export doesn't mention it, and the repair binds it anyway.
    let other = export(
        &scratch.path().join("other"),
        &[heart("2026-10-11 06:31:10 -0600", "72")],
    );
    let repaired = run(root, &other, "20261012_120001", None);
    assert!(repaired.finished);
    assert_eq!(repaired.published, 1, "{}", repaired.summary);
    assert!(
        repaired.summary.contains("published again"),
        "{}",
        repaired.summary
    );
    assert!(orphan.join("stream.json").is_file());
    assert!(!root.join(RUN_DIR_REL).exists());

    // With an unbound window and no marker, the window itself asks to bind.
    fs::remove_file(window(root, "20261011", "060000_3600").join("stream.json")).unwrap();
    let rebound = run(root, &other, "20261012_120002", None);
    assert_eq!(rebound.published, 1);
    assert!(
        window(root, "20261011", "060000_3600")
            .join("stream.json")
            .is_file()
    );
}

#[test]
fn a_day_whose_marker_failed_stays_owed_and_the_next_run_moves_it() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(scratch.path(), &[heart("2026-10-10 06:31:10 -0600", "70")]);
    // A file where the day's health directory goes makes the marker write fail, so
    // the record really says failure while the attempt still applies.
    let blocked = root.join("chronicle/20261010/health");
    fs::create_dir_all(blocked.parent().unwrap()).unwrap();
    fs::write(&blocked, "in the way").unwrap();
    let failed = run(root, &source, "20261012_120000", None);
    assert!(!failed.finished);
    let record = read_publication_record(&root.join("imports").join(&failed.import_id))
        .unwrap()
        .unwrap();
    assert_eq!(record.status, PublicationStatus::Failure);
    assert!(
        failed.summary.contains("20261010: day marker"),
        "{}",
        failed.summary
    );
    assert!(root.join(RUN_DIR_REL).join("state.json").is_file());

    fs::remove_file(&blocked).unwrap();
    let repaired = run(root, &source, "20261012_120001", None);
    assert!(repaired.finished, "{}", repaired.summary);
    assert!(!root.join(RUN_DIR_REL).exists());
    assert!(matches!(
        read_health_marker(root, "20261010", HealthMarkerKind::Body).unwrap(),
        HealthMarkerState::Versioned { .. }
    ));
}

#[test]
fn a_failed_write_binds_nothing_and_the_next_run_can_start() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(scratch.path(), &[heart("2026-10-10 06:31:10 -0600", "70")]);
    // A file where the stream's directory goes makes every window's write fail.
    let blocked = root.join("chronicle/20261010").join(STREAM);
    fs::create_dir_all(blocked.parent().unwrap()).unwrap();
    fs::write(&blocked, "in the way").unwrap();
    let failed = run_apple_health_segments(
        root,
        &AppleSegmentsRequest::new(&source, "20261012_120000", None),
    );
    assert!(failed.is_err());
    assert!(root.join(RUN_DIR_REL).join("state.json").is_file());
    assert!(
        solstone_core_import::thinking::read(root)
            .unwrap()
            .imports
            .is_empty()
    );
    assert!(stream_marker_absent(root, "20261010"));

    fs::remove_file(&blocked).unwrap();
    let repaired = run(root, &source, "20261012_120000", None);
    assert!(repaired.finished);
    assert!(
        window(root, "20261010", "060000_3600")
            .join("stream.json")
            .is_file()
    );
}

#[test]
fn the_summary_says_when_a_window_keeps_or_takes_on_another_imports_facet() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let first = export(scratch.path(), &[heart("2026-10-10 06:31:10 -0600", "70")]);
    run(root, &first, "20261012_120000", Some("health"));
    let second = export(
        &scratch.path().join("second"),
        &[heart("2026-10-10 06:32:10 -0600", "71")],
    );
    let added = run(root, &second, "20261012_120001", None);
    assert!(
        added.summary.contains("1 window already carries a facet"),
        "{}",
        added.summary
    );

    let third = export(
        &scratch.path().join("third"),
        &[heart("2026-10-10 06:33:10 -0600", "72")],
    );
    let moved = run(root, &third, "20261012_120002", Some("other"));
    assert!(
        moved.summary.contains("1 window already held records"),
        "{}",
        moved.summary
    );
}

#[test]
fn a_filled_old_record_keeps_its_bytes() {
    use solstone_core_import::publish::SegmentPublicationOutcome;
    let text = r#"{"day":"20260101","segment":"120000_300","stream":"import.strava","outcome":{"status":"owner_deleted"}}"#;
    let outcome: SegmentPublicationOutcome = serde_json::from_str(text).unwrap();
    assert!(!outcome.body_only);
    assert_eq!(serde_json::to_string(&outcome).unwrap(), text);
}

// ---- L2: resume, failure and repair ------------------------------------------

fn stage_record(start: &str, end: &str, value: &str) -> String {
    format!(
        r#"<Record type="HKCategoryTypeIdentifierSleepAnalysis" {SOURCE} startDate="{start}" endDate="{end}" value="HKCategoryValueSleepAnalysis{value}"/>"#
    )
}

/// Four days: readings every few minutes, a run each morning with its events
/// when `runs`, and a night across each midnight.
fn days_records(runs: bool) -> Vec<String> {
    let mut records = Vec::new();
    for day in 10..14 {
        for minute in (0..1440).step_by(23) {
            records.push(heart(
                &format!(
                    "2026-10-{day} {:02}:{:02}:10 -0600",
                    minute / 60,
                    minute % 60
                ),
                &format!("{}", 60 + minute % 20),
            ));
        }
        if runs {
            records.push(workout(day));
        }
        records.push(stage_record(
            &format!("2026-10-{day} 22:40:00 -0600"),
            &format!("2026-10-{} 01:00:00 -0600", day + 1),
            "AsleepCore",
        ));
    }
    records
}

fn workout(day: u32) -> String {
    format!(
        r#"<Workout workoutActivityType="HKWorkoutActivityTypeRunning" {SOURCE} startDate="2026-10-{day} 06:31:00 -0600" endDate="2026-10-{day} 07:20:00 -0600"><MetadataEntry key="HKWorkoutBrandName" value="Loop {day}"/><WorkoutEvent type="HKWorkoutEventTypeSegment" date="2026-10-{day} 06:41:00 -0600"/></Workout>"#
    )
}

fn days_export(directory: &Path) -> PathBuf {
    export(directory, &days_records(true))
}

fn request<'a>(source: &'a Path, timestamp: &'a str) -> AppleSegmentsRequest<'a> {
    let mut request = AppleSegmentsRequest::new(source, timestamp, None);
    // Small enough that every phase spills into several runs.
    request.spill_bytes = 4_096;
    request
}

/// Every file that says what the journal holds, byte for byte. Locks, health
/// markers (whose versions count moves, not content) and attempt records (which
/// carry times) are left out.
fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.join("chronicle")];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != "health" {
                    stack.push(path);
                }
            } else if !name.ends_with(".lock") {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                files.insert(relative, fs::read(&path).unwrap());
            }
        }
    }
    for import in fs::read_dir(root.join("imports")).unwrap().flatten() {
        let dir = import.path();
        for name in ["summary.md", "body-touched.jsonl"] {
            if let Ok(bytes) = fs::read(dir.join(name)) {
                let id = dir.file_name().unwrap().to_string_lossy().into_owned();
                files.insert(format!("imports/{id}/{name}"), bytes);
            }
        }
        if let Ok(bytes) = fs::read(dir.join("manifest.json")) {
            let manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let id = dir.file_name().unwrap().to_string_lossy().into_owned();
            files.insert(
                format!("imports/{id}/manifest totals"),
                json!([manifest["entry_count"], manifest["days_affected"]])
                    .to_string()
                    .into_bytes(),
            );
        }
    }
    files
}

/// The stream's chain: every seq once, and each `prev` naming the marker at seq-1.
fn assert_chain(root: &Path) {
    let mut by_seq: BTreeMap<u64, (String, String, serde_json::Value)> = BTreeMap::new();
    for day in fs::read_dir(root.join("chronicle")).unwrap().flatten() {
        let stream = day.path().join(STREAM);
        let Ok(windows) = fs::read_dir(&stream) else {
            continue;
        };
        for window in windows.flatten() {
            let Ok(bytes) = fs::read(window.path().join("stream.json")) else {
                continue;
            };
            let marker: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let seq = marker["seq"].as_u64().unwrap();
            let key = (
                day.file_name().to_string_lossy().into_owned(),
                window.file_name().to_string_lossy().into_owned(),
                marker,
            );
            assert!(
                by_seq.insert(seq, key).is_none(),
                "seq {seq} is claimed twice"
            );
        }
    }
    for (seq, (_, _, marker)) in &by_seq {
        if *seq > 1
            && let Some((day, key, _)) = by_seq.get(&(seq - 1))
        {
            assert_eq!(
                marker["prev_day"].as_str(),
                Some(day.as_str()),
                "prev of {seq}"
            );
            assert_eq!(
                marker["prev_segment"].as_str(),
                Some(key.as_str()),
                "prev of {seq}"
            );
        }
    }
}

/// Stop at the `nth` time `point` is reached.
fn stopping_at(point: &'static str, nth: usize) -> impl Fn(&str) -> bool {
    let seen = std::cell::Cell::new(0);
    move |name| {
        if name == point {
            seen.set(seen.get() + 1);
            seen.get() == nth
        } else {
            false
        }
    }
}

#[test]
fn a_run_stopped_anywhere_resumes_to_what_an_uninterrupted_run_writes() {
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    let whole = journal();
    let done =
        run_apple_health_segments(whole.path(), &request(&source, "20261104_120000")).unwrap();
    assert!(done.finished);
    let expected = snapshot(whole.path());
    assert!(expected.len() > 50, "the export spans many windows");
    assert_chain(whole.path());

    for (point, nth) in [
        ("scanned", 1),
        ("placed", 1),
        ("pending", 1),
        ("pending", 3),
        ("written", 2),
        ("published", 1),
        ("published", 4),
        ("written-all", 1),
        ("finished", 1),
    ] {
        let journal = journal();
        let root = journal.path();
        let stop = stopping_at(point, nth);
        let mut stopped = request(&source, "20261104_120000");
        stopped.stop_at = Some(&stop);
        let error = run_apple_health_segments(root, &stopped).unwrap_err();
        assert!(error.contains(point), "{point}: {error}");
        // The owner runs it again later, under a new timestamp.
        let resumed =
            run_apple_health_segments(root, &request(&source, "20261104_130000")).unwrap();
        assert!(resumed.finished, "{point} {nth}: {}", resumed.summary);
        assert_eq!(
            resumed.import_id, done.import_id,
            "{point} {nth}: resumed under its import"
        );
        assert!(
            !root.join("imports/20261104_130000").exists(),
            "{point} {nth}: no second import holds these bytes"
        );
        assert!(!root.join(RUN_DIR_REL).exists());
        let got = snapshot(root);
        assert_eq!(
            got.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>(),
            "{point} {nth}"
        );
        for (path, bytes) in &expected {
            assert_eq!(got.get(path), Some(bytes), "{point} {nth}: {path}");
        }
        assert_chain(root);
    }
}

#[test]
fn every_row_carries_the_run_that_first_wrote_it_and_the_listing_says_what_each_touched() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let first = export(scratch.path(), &[heart("2026-10-10 06:31:10 -0600", "70")]);
    let one = run(root, &first, "20261012_120000", None);
    let second = export(
        &scratch.path().join("second"),
        &[
            heart("2026-10-10 06:31:10 -0600", "71"),
            heart("2026-10-10 06:32:10 -0600", "72"),
        ],
    );
    let two = run(root, &second, "20261012_120001", None);
    let rows =
        decode_rows(&String::from_utf8(body_bytes(root, "20261010", "060000_3600")).unwrap())
            .unwrap();
    let runs: Vec<_> = rows.iter().map(|row| row.run.clone().unwrap()).collect();
    // The edited row keeps the run that first wrote it; the new one carries the second.
    assert_eq!(runs, vec![one.import_id.clone(), two.import_id.clone()]);
    let listing = fs::read_to_string(
        root.join("imports")
            .join(&two.import_id)
            .join("body-touched.jsonl"),
    )
    .unwrap();
    assert_eq!(
        listing.trim(),
        r#"{"day":"20261010","windows":[["060000_3600",1,1,0]]}"#
    );
    // The same export again changes nothing, and its listing says so.
    let again = run(root, &second, "20261012_120002", None);
    let listing = fs::read_to_string(
        root.join("imports")
            .join(&again.import_id)
            .join("body-touched.jsonl"),
    )
    .unwrap();
    assert_eq!(
        listing.trim(),
        r#"{"day":"20261010","windows":[["060000_3600",0,0,2]]}"#
    );
}

#[test]
fn one_window_that_wont_bind_is_named_and_the_rest_of_the_import_lands() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    // A marker already claiming a place this stream hasn't reached: that window
    // can't be bound, whatever runs.
    let stuck = window(root, "20261011", "063000_300");
    fs::create_dir_all(&stuck).unwrap();
    fs::write(
        stuck.join("stream.json"),
        json!({"stream": STREAM, "prev_day": null, "prev_segment": null, "seq": 999}).to_string(),
    )
    .unwrap();
    let partly = run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap();
    assert!(!partly.finished);
    assert!(
        partly.summary.contains("20261011 063000_300: bind"),
        "{}",
        partly.summary
    );
    assert!(
        window(root, "20261013", "063000_300")
            .join("stream.json")
            .is_file(),
        "later days land"
    );
    let record = read_publication_record(&root.join("imports").join(&partly.import_id))
        .unwrap()
        .unwrap();
    assert_eq!(record.status, PublicationStatus::Failure);
    assert_eq!(
        record.segments.len(),
        1,
        "it lists only the window still failing"
    );
    assert_ne!(
        solstone_core_import::project_import_result(root, &partly.import_id).status,
        solstone_core_import::ProjectionStatus::Success
    );
    // Another run retries only that window, which still fails, and blocks nothing.
    let again = run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap();
    assert!(!again.finished);
    assert!(again.summary.contains("20261011 063000_300: bind"));
    // Once the stray marker is gone, it lands.
    fs::remove_file(stuck.join("stream.json")).unwrap();
    let fixed = run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap();
    assert!(fixed.finished, "{}", fixed.summary);
    assert!(stuck.join("stream.json").is_file());
    assert!(!root.join(RUN_DIR_REL).exists());
}

#[test]
fn a_marker_that_cant_be_written_ends_the_day_and_the_next_run_carries_on() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    // A directory where a window's marker goes: its record moves and its marker
    // can't follow, which every later advance in the stream would repeat.
    let blocked = window(root, "20261012", "000000_3600").join("stream.json");
    fs::create_dir_all(&blocked).unwrap();
    let error = run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap_err();
    assert!(error.contains("marker"), "{error}");
    let state = fs::read_to_string(root.join(RUN_DIR_REL).join("state.json")).unwrap();
    assert!(
        state.contains("\"pending\":{\"day\":\"20261012\""),
        "{state}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&state).unwrap();
    assert_eq!(
        parsed["failed"],
        json!([]),
        "it fanned no window into failed"
    );
    fs::remove_dir(&blocked).unwrap();
    let resumed = run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap();
    assert!(resumed.finished, "{}", resumed.summary);
    assert_chain(root);
}

#[test]
fn a_restored_run_with_its_record_behind_its_markers_repairs_without_forking() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    let stop = stopping_at("pending", 3);
    let mut stopped = request(&source, "20261104_120000");
    stopped.stop_at = Some(&stop);
    run_apple_health_segments(root, &stopped).unwrap_err();
    // A restore: staging is left out of copies, and the stream record came from
    // earlier than some window markers.
    fs::remove_dir_all(root.join(RUN_DIR_REL).join("placed")).unwrap();
    let record_path = root.join("streams").join(format!("{STREAM}.json"));
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    let seq = record["seq"].as_u64().unwrap();
    assert!(seq > 5);
    record["seq"] = json!(seq - 5);
    fs::write(&record_path, record.to_string()).unwrap();
    let repaired = run_apple_health_segments(root, &request(&source, "20261104_130000")).unwrap();
    assert!(repaired.finished, "{}", repaired.summary);
    assert!(
        repaired.summary.contains("published again"),
        "{}",
        repaired.summary
    );
    assert_chain(root);
    // Every window holds what an uninterrupted run would hold.
    let whole = self::journal();
    run_apple_health_segments(whole.path(), &request(&source, "20261104_120000")).unwrap();
    let bodies = |root: &Path| {
        snapshot(root)
            .into_iter()
            .filter(|(path, _)| path.ends_with("body.jsonl"))
            .map(|(path, bytes)| {
                // Rows' runs differ when a second import wrote them.
                let rows = decode_rows(&String::from_utf8(bytes).unwrap()).unwrap();
                let rows: Vec<_> = rows
                    .into_iter()
                    .map(|mut row| {
                        row.run = None;
                        row
                    })
                    .collect();
                (path, rows)
            })
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(bodies(root), bodies(whole.path()));
}

#[test]
fn another_export_after_a_stop_publishes_the_old_half_day_and_starts_fresh() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    let stop = stopping_at("written", 2);
    let mut stopped = request(&source, "20261104_120000");
    stopped.stop_at = Some(&stop);
    run_apple_health_segments(root, &stopped).unwrap_err();
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(RUN_DIR_REL).join("state.json")).unwrap())
            .unwrap();
    let day = state["pending"]["day"].as_str().unwrap().to_owned();
    let other = export(
        &scratch.path().join("other"),
        &[heart("2026-10-20 06:31:10 -0600", "70")],
    );
    let fresh = run(root, &other, "20261104_130000", None);
    assert!(fresh.finished, "{}", fresh.summary);
    assert_ne!(fresh.import_id, "20261104_120000");
    for window in fs::read_dir(root.join("chronicle").join(&day).join(STREAM))
        .unwrap()
        .flatten()
    {
        if window.path().join(BODY_RECORDS_FILE).is_file() {
            assert!(
                window.path().join("stream.json").is_file(),
                "{:?} was left unbound",
                window.path()
            );
        }
    }
    let old = read_provenance(root, "20261104_120000").unwrap().unwrap();
    let AttemptRead::Present(facts) = read_attempt_facts(&old) else {
        panic!("the old import has an attempt");
    };
    assert_ne!(
        facts.state,
        AttemptState::Running,
        "the old import was closed"
    );
}

// Re-import when a reading's place depends on a session: it has two places on its
// day, its hour and its session's slot, and an export can move it between them.

fn delete(root: &Path, day: &str, key: &str) {
    let outcome = door::remove_segments(
        root,
        &[solstone_core_retention::Target {
            day: day.to_owned(),
            stream: STREAM.to_owned(),
            dir: key.to_owned(),
        }],
        &chrono::Utc::now().to_rfc3339(),
        RemovalReason::OwnerSegmentDelete,
        "test",
    );
    assert!(outcome.halted.is_none());
}

/// Every window on `day` whose file holds a reading of `value`.
fn holding(root: &Path, day: &str, value: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let Ok(windows) = fs::read_dir(root.join("chronicle").join(day).join(STREAM)) else {
        return keys;
    };
    for window in windows.flatten() {
        if let Ok(text) = fs::read_to_string(window.path().join(BODY_RECORDS_FILE))
            && text.contains(&format!(r#""value":"{value}""#))
        {
            keys.push(window.file_name().to_string_lossy().into_owned());
        }
    }
    keys.sort();
    keys
}

fn phone(at: &str, value: &str) -> String {
    format!(
        r#"<Record type="HKQuantityTypeIdentifierHeartRate" sourceName="Synthetic iPhone" unit="count/min" startDate="{at}" endDate="{at}" value="{value}"/>"#
    )
}

const TEN: &str = "20261010";

#[test]
fn a_session_arriving_later_leaves_readings_where_they_were_saved() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let readings = [
        heart("2026-10-10 06:35:10 -0600", "81"),
        heart("2026-10-10 06:40:10 -0600", "82"),
    ];
    run(
        root,
        &export(&scratch.path().join("1"), &readings),
        "20261012_120000",
        None,
    );
    assert_eq!(holding(root, TEN, "81"), ["060000_3600"]);

    let mut with_run = readings.to_vec();
    with_run.push(workout(10));
    let source = export(&scratch.path().join("2"), &with_run);
    let two = run(root, &source, "20261012_120001", None);
    assert!(!two.summary.contains("wasn't added") && !two.summary.contains("weren't added"));
    for value in ["81", "82"] {
        assert_eq!(holding(root, TEN, value), ["060000_3600"], "{value}");
    }
    let listing = fs::read_to_string(
        root.join("imports")
            .join(&two.import_id)
            .join("body-touched.jsonl"),
    )
    .unwrap();
    assert!(listing.contains(r#"["060000_3600",0,0,2]"#), "{listing}");
    assert!(listing.contains(r#"["063000_300",1,0,0]"#), "{listing}");

    // The same export again changes nothing.
    assert_eq!(run(root, &source, "20261012_120002", None).published, 0);
}

#[test]
fn a_session_dropped_later_leaves_its_series_on_its_tiles_and_makes_no_empty_hour() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let readings = [heart("2026-10-10 06:35:10 -0600", "81")];
    let mut with_run = readings.to_vec();
    with_run.push(workout(10));
    run(
        root,
        &export(&scratch.path().join("1"), &with_run),
        "20261012_120000",
        None,
    );
    assert_eq!(holding(root, TEN, "81"), ["063500_300"]);

    let source = export(&scratch.path().join("2"), &readings);
    let two = run(root, &source, "20261012_120001", None);
    assert_eq!(two.published, 0, "{}", two.summary);
    assert_eq!(holding(root, TEN, "81"), ["063500_300"]);
    assert!(
        !window(root, TEN, "060000_3600").exists(),
        "no hour is written with nothing in it"
    );
    assert!(!two.summary.contains("added"), "{}", two.summary);
}

#[test]
fn a_reading_whose_tile_the_owner_deleted_stays_out_when_its_session_goes() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let readings = [heart("2026-10-10 06:35:10 -0600", "81")];
    let mut with_run = readings.to_vec();
    with_run.push(workout(10));
    run(
        root,
        &export(&scratch.path().join("1"), &with_run),
        "20261012_120000",
        None,
    );
    delete(root, TEN, "063500_300");

    let two = run(
        root,
        &export(&scratch.path().join("2"), &readings),
        "20261012_120001",
        None,
    );
    assert!(
        two.summary.contains("1 reading wasn't added"),
        "{}",
        two.summary
    );
    assert!(holding(root, TEN, "81").is_empty());
    assert!(!window(root, TEN, "060000_3600").exists());
}

#[test]
fn readings_in_an_hour_the_owner_deleted_stay_out_when_a_session_arrives() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let readings = [
        heart("2026-10-10 06:35:10 -0600", "81"),
        heart("2026-10-10 06:40:10 -0600", "82"),
    ];
    run(
        root,
        &export(&scratch.path().join("1"), &readings),
        "20261012_120000",
        None,
    );
    delete(root, TEN, "060000_3600");

    let mut with_run = readings.to_vec();
    with_run.push(workout(10));
    let two = run(
        root,
        &export(&scratch.path().join("2"), &with_run),
        "20261012_120001",
        None,
    );
    assert!(
        two.summary.contains("2 readings weren't added"),
        "{}",
        two.summary
    );
    for value in ["81", "82"] {
        assert!(holding(root, TEN, value).is_empty(), "{value}");
    }
    // The session itself lands on its tiles.
    assert!(
        window(root, TEN, "063000_300")
            .join(BODY_RECORDS_FILE)
            .is_file()
    );
}

#[test]
fn deleting_a_workouts_tile_leaves_another_sources_readings_in_their_hour() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let records = [
        workout(10),
        heart("2026-10-10 06:40:30 -0600", "81"),
        phone("2026-10-10 06:40:10 -0600", "91"),
        phone("2026-10-10 06:45:10 -0600", "92"),
    ];
    run(
        root,
        &export(&scratch.path().join("1"), &records),
        "20261012_120000",
        None,
    );
    assert_eq!(holding(root, TEN, "91"), ["060000_3600"]);
    let hour = body_bytes(root, TEN, "060000_3600");
    delete(root, TEN, "064000_300");

    // The same export: the phone's readings are where they were, nothing held back.
    let again = run(
        root,
        &export(&scratch.path().join("1"), &records),
        "20261012_120001",
        None,
    );
    assert!(!again.summary.contains("added"), "{}", again.summary);
    assert_eq!(body_bytes(root, TEN, "060000_3600"), hour);

    // A new phone reading inside the deleted slot still lands: the session that
    // slot belonged to is still in the export.
    let mut more = records.to_vec();
    more.push(phone("2026-10-10 06:42:10 -0600", "93"));
    let three = run(
        root,
        &export(&scratch.path().join("3"), &more),
        "20261012_120002",
        None,
    );
    assert!(!three.summary.contains("added"), "{}", three.summary);
    assert_eq!(holding(root, TEN, "93"), ["060000_3600"]);
}

#[test]
fn a_reading_whose_other_place_cant_be_read_is_held_back() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let readings = [heart("2026-10-10 06:35:10 -0600", "81")];
    run(
        root,
        &export(&scratch.path().join("1"), &readings),
        "20261012_120000",
        None,
    );
    fs::write(
        window(root, TEN, "060000_3600").join(BODY_RECORDS_FILE),
        "{broken\n",
    )
    .unwrap();

    let mut with_run = readings.to_vec();
    with_run.push(workout(10));
    let two = run(
        root,
        &export(&scratch.path().join("2"), &with_run),
        "20261012_120001",
        None,
    );
    assert!(
        two.summary.contains("1 reading wasn't added"),
        "{}",
        two.summary
    );
    assert!(
        holding(root, TEN, "81").is_empty(),
        "not copied beside the unreadable hour"
    );
    assert_eq!(
        fs::read_to_string(window(root, TEN, "060000_3600").join(BODY_RECORDS_FILE)).unwrap(),
        "{broken\n"
    );
}

#[test]
fn a_deleted_card_at_midnight_holds_back_no_reading() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let card = window(root, TEN, "000000_300");
    fs::create_dir_all(&card).unwrap();
    fs::write(card.join("day_summary_transcript.md"), "a card\n").unwrap();
    delete(root, TEN, "000000_300");
    let one = run(
        root,
        &export(scratch.path(), &[heart("2026-10-10 00:02:10 -0600", "81")]),
        "20261012_120000",
        None,
    );
    assert!(!one.summary.contains("added"), "{}", one.summary);
    assert_eq!(holding(root, TEN, "81"), ["000000_3600"]);
}

#[test]
fn a_run_over_an_earlier_import_stopped_anywhere_resumes_to_the_same_bytes() {
    let scratch = tempfile::tempdir().unwrap();
    let first = export(&scratch.path().join("1"), &days_records(false));
    let second = days_export(&scratch.path().join("2"));
    // The earlier import, then the owner deletes one morning's hour, so the runs
    // arriving with the second export find readings both saved and deleted.
    let earlier = |root: &Path| {
        run_apple_health_segments(root, &request(&first, "20261104_110000")).unwrap();
        delete(root, "20261011", "060000_3600");
    };
    let whole = journal();
    earlier(whole.path());
    let done =
        run_apple_health_segments(whole.path(), &request(&second, "20261104_120000")).unwrap();
    assert!(done.finished);
    assert!(
        done.summary.contains("readings weren't added"),
        "{}",
        done.summary
    );
    // A tombstone carries the moment of its delete, which each journal did apart.
    let settled = |root: &Path| {
        let mut files = snapshot(root);
        files.retain(|path, _| !path.ends_with("tombstone.json"));
        files
    };
    let expected = settled(whole.path());
    assert_chain(whole.path());

    for (point, nth) in [
        ("scanned", 1),
        ("placed", 1),
        ("pending", 1),
        ("pending", 3),
        ("written", 2),
        ("published", 1),
        ("published", 4),
        ("written-all", 1),
        ("finished", 1),
    ] {
        let journal = journal();
        let root = journal.path();
        earlier(root);
        let stop = stopping_at(point, nth);
        let mut stopped = request(&second, "20261104_120000");
        stopped.stop_at = Some(&stop);
        let error = run_apple_health_segments(root, &stopped).unwrap_err();
        assert!(error.contains(point), "{point}: {error}");
        let resumed =
            run_apple_health_segments(root, &request(&second, "20261104_130000")).unwrap();
        assert!(resumed.finished, "{point} {nth}: {}", resumed.summary);
        let got = settled(root);
        assert_eq!(
            got.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>(),
            "{point} {nth}"
        );
        for (path, bytes) in &expected {
            assert_eq!(got.get(path), Some(bytes), "{point} {nth}: {path}");
        }
        assert_chain(root);
    }
}
