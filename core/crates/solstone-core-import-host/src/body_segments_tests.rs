// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods)]

use std::fs;

use serde_json::json;
use solstone_core_format::content::body_records::decode_rows;
use solstone_core_import_sources::apple_health::STREAM;
use solstone_core_journal_io::{HealthMarkerKind, HealthMarkerState, read_health_marker};
use solstone_core_retention::door;
use solstone_core_retention::tombstone::RemovalReason;

use super::*;

const SOURCE: &str = r#"sourceName="Synthetic Watch" creationDate="2026-10-10 08:00:00 -0600""#;

#[cfg(feature = "full-tests")]
#[path = "body_segments_facet_tests.rs"]
pub(super) mod facet_tests;

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

/// A run the owner answered for, so a facet it names moves what's filed elsewhere.
fn run(root: &Path, source: &Path, timestamp: &str, facet: Option<&str>) -> AppleSegmentsOutcome {
    let mut request = AppleSegmentsRequest::new(source, timestamp, facet);
    request.listed = ListedMoves::Move;
    run_apple_health_segments(root, &request).unwrap()
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
        failed.summary.contains("2026-10-10: day marker"),
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

#[cfg(unix)]
#[test]
fn a_failed_write_binds_nothing_and_the_next_run_can_start() {
    use std::os::unix::fs::PermissionsExt;
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(scratch.path(), &[heart("2026-10-10 06:31:10 -0600", "70")]);
    // A day folder that takes no stream makes every window's write fail.
    let blocked = root.join("chronicle/20261010");
    fs::create_dir_all(&blocked).unwrap();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o500)).unwrap();
    if fs::write(blocked.join("probe"), "").is_ok() {
        // Root writes anywhere, so there's no refusal to see.
        return;
    }
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

    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
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
        added
            .summary
            .contains("1 part of your journal already carries a facet"),
        "{}",
        added.summary
    );

    let third = export(
        &scratch.path().join("third"),
        &[heart("2026-10-10 06:33:10 -0600", "72")],
    );
    let moved = run(root, &third, "20261012_120002", Some("other"));
    assert!(
        moved
            .summary
            .contains("1 part of your journal already held records"),
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

/// Four days: readings around midnight and both morning hours, a run each morning
/// with its events when `runs`, and a night across each midnight. Keep readings
/// inside and outside each experience without filling unrelated hours.
fn days_records(runs: bool) -> Vec<String> {
    let mut records = Vec::new();
    for day in 10..14 {
        for minute in [0, 23, 368, 391, 414, 437, 460, 736, 1357, 1380] {
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
    request_for(source, timestamp, None)
}

fn request_for<'a>(
    source: &'a Path,
    timestamp: &'a str,
    facet: Option<&'a str>,
) -> AppleSegmentsRequest<'a> {
    let mut request = AppleSegmentsRequest::new(source, timestamp, facet);
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
    // And every activity record, so a run with a facet is held to its records.
    // A facet's destination id is the journal's own, minted when it's first
    // written to, so it's left out.
    for facet in fs::read_dir(root.join("facets"))
        .into_iter()
        .flatten()
        .flatten()
    {
        for file in fs::read_dir(facet.path().join("activities"))
            .into_iter()
            .flatten()
            .flatten()
        {
            // One separator on every platform, so the key reads the same on Windows.
            let relative = file
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let mut text = String::new();
            for line in fs::read_to_string(file.path()).unwrap().lines() {
                let mut record: serde_json::Value = serde_json::from_str(line).unwrap();
                record.as_object_mut().unwrap().remove("destination_id");
                text.push_str(&format!("{record}\n"));
            }
            files.insert(relative, text.into_bytes());
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

fn assert_resume_spills(root: &Path, point: &str) {
    let folder = match point {
        "scanned" => "scan",
        "placed" => "placed",
        _ => return,
    };
    let runs = runs_in(&root.join(RUN_DIR_REL).join(folder)).unwrap();
    assert!(runs.len() > 1, "{point}: resume merges multiple runs");
}

#[test]
fn a_run_stopped_anywhere_resumes_to_what_an_uninterrupted_run_writes() {
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    // Without a facet, and with one, so a stop also falls among records.
    for facet in [None, Some("health")] {
        let whole = journal();
        let done = run_apple_health_segments(
            whole.path(),
            &request_for(&source, "20261104_120000", facet),
        )
        .unwrap();
        assert!(done.finished);
        let expected = snapshot(whole.path());
        assert!(expected.len() > 50, "the export spans many windows");
        if facet.is_some() {
            assert!(
                expected
                    .keys()
                    .any(|path| path.starts_with("facets/health/activities/")),
                "the export makes records"
            );
        }
        assert_chain(whole.path());

        // Without a facet, everywhere; with one, the stops on either side of
        // a day's records, which are written once the day is published.
        let everywhere = [
            ("scanned", 1),
            ("placed", 1),
            ("pending", 1),
            ("pending", 3),
            ("written", 2),
            ("published", 1),
            ("published", 4),
            ("written-all", 1),
            ("finished", 1),
        ];
        let around_records = [
            ("published", 1),
            ("pending", 3),
            ("published", 4),
            ("written-all", 1),
            ("finished", 1),
        ];
        let points: &[(&str, usize)] = if facet.is_some() {
            &around_records
        } else {
            &everywhere
        };
        for &(point, nth) in points {
            let journal = journal();
            let root = journal.path();
            let stop = stopping_at(point, nth);
            let mut stopped = request_for(&source, "20261104_120000", facet);
            stopped.stop_at = Some(&stop);
            let error = run_apple_health_segments(root, &stopped).unwrap_err();
            assert!(error.contains(point), "{point}: {error}");
            assert_resume_spills(root, point);
            // The owner runs it again later, under a new timestamp.
            let resumed =
                run_apple_health_segments(root, &request_for(&source, "20261104_130000", facet))
                    .unwrap();
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
        partly.summary.contains("2026-10-11: bind"),
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
    assert!(again.summary.contains("2026-10-11: bind"));
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
    // Stopped after its first window of that day: the others it meant to write
    // weren't reached, so they aren't counted.
    let written = state["totals"]["written"].as_u64().unwrap() + 1;
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
    // The half-done day's rows name the old import, so its record says so.
    let listing =
        fs::read_to_string(root.join("imports/20261104_120000/body-touched.jsonl")).unwrap();
    assert!(
        listing.lines().any(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["day"] == day.as_str()
        }),
        "{listing}"
    );
    let summary = fs::read_to_string(root.join("imports/20261104_120000/summary.md")).unwrap();
    assert!(summary.contains("replaced by a later one"), "{summary}");
    assert!(
        summary.contains(&format!("{written} parts of your journal written")),
        "the half-done day counts: {written}: {summary}"
    );
    let mut named = BTreeSet::new();
    for (path, bytes) in snapshot(root) {
        if path.ends_with(BODY_RECORDS_FILE)
            && decode_rows(&String::from_utf8(bytes).unwrap())
                .unwrap()
                .iter()
                .any(|row| row.run.as_deref() == Some("20261104_120000"))
        {
            named.insert(path.replace('\\', "/"));
        }
    }
    let listed: Vec<String> = listing
        .lines()
        .flat_map(|line| {
            let line: serde_json::Value = serde_json::from_str(line).unwrap();
            let day = line["day"].as_str().unwrap().to_owned();
            line["windows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|window| format!("{day}/{STREAM}/{}/", window[0].as_str().unwrap()))
                .collect::<Vec<_>>()
        })
        .collect();
    for path in &named {
        assert!(
            listed.iter().any(|window| path.contains(window.as_str())),
            "{path} names the import and isn't listed"
        );
    }
    assert!(!named.is_empty());
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
fn deleting_a_workouts_tile_keeps_what_was_saved_and_holds_back_what_is_new_inside_it() {
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
    assert_eq!(body_bytes(root, TEN, "060000_3600"), hour);
    assert!(holding(root, TEN, "81").is_empty(), "{}", again.summary);

    // The reading saved on the deleted tile was one the export still holds, so it's
    // counted there; a new phone reading inside the deleted slot is held back too.
    let mut more = records.to_vec();
    more.push(phone("2026-10-10 06:42:10 -0600", "93"));
    let three = run(
        root,
        &export(&scratch.path().join("3"), &more),
        "20261012_120002",
        None,
    );
    assert!(
        three.summary.contains("1 reading wasn't added"),
        "{}",
        three.summary
    );
    assert!(holding(root, TEN, "93").is_empty());
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
        assert_resume_spills(root, point);
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

fn strava_workout(day: u32) -> String {
    format!(
        r#"<Workout workoutActivityType="HKWorkoutActivityTypeRunning" sourceName="Synthetic Strava" creationDate="2026-10-10 08:00:00 -0600" startDate="2026-10-{day} 06:31:00 -0600" endDate="2026-10-{day} 07:20:00 -0600"/>"#
    )
}

#[test]
fn a_reading_whose_tile_was_deleted_stays_out_though_another_sources_session_covers_it() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let reading = heart("2026-10-10 06:40:30 -0600", "81");
    run(
        root,
        &export(
            &scratch.path().join("1"),
            &[workout(10), strava_workout(10), reading.clone()],
        ),
        "20261012_120000",
        None,
    );
    assert_eq!(holding(root, TEN, "81"), ["064000_300"]);
    delete(root, TEN, "064000_300");
    assert!(holding(root, TEN, "81").is_empty());
    // The watch's own workout is gone from the later export; Strava's still covers.
    let two = run(
        root,
        &export(&scratch.path().join("2"), &[strava_workout(10), reading]),
        "20261012_120001",
        None,
    );
    assert!(
        holding(root, TEN, "81").is_empty(),
        "a reading the owner deleted came back in {:?}",
        holding(root, TEN, "81")
    );
    assert!(
        two.summary.contains("1 reading wasn't added"),
        "{}",
        two.summary
    );
}

#[test]
fn the_same_bytes_at_the_same_time_leave_the_first_imports_record_alone() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(scratch.path(), &[heart("2026-10-10 06:31:10 -0600", "70")]);
    let one = run(root, &source, "20261012_120000", None);
    let dir = root.join("imports").join(&one.import_id);
    let record = |name: &str| fs::read(dir.join(name)).unwrap();
    let before = (
        record("body-touched.jsonl"),
        record("manifest.json"),
        record("summary.md"),
    );
    let two = run(root, &source, "20261012_120000", None);
    assert_ne!(one.import_id, two.import_id);
    assert_eq!(
        before,
        (
            record("body-touched.jsonl"),
            record("manifest.json"),
            record("summary.md"),
        ),
        "the first import's record was replaced"
    );
    assert!(
        two.summary.contains("0 parts of your journal written"),
        "{}",
        two.summary
    );
    // A row still names the import that first wrote it.
    let rows =
        decode_rows(&String::from_utf8(body_bytes(root, TEN, "060000_3600")).unwrap()).unwrap();
    assert_eq!(rows[0].run.as_deref(), Some(one.import_id.as_str()));
    // The retry path takes a new record too, and leaves the first one as it was.
    let three = run(root, &source, "20261012_120000", None);
    assert!(three.import_id != one.import_id && three.import_id != two.import_id);
}

/// Every time in these records rendered two hours on at `-0400`, as an export from
/// another zone writes the same instants.
fn at_another_offset(records: &[String]) -> Vec<String> {
    records
        .iter()
        .map(|record| {
            let mut out = String::new();
            let mut rest = record.as_str();
            while let Some(at) = rest.find(" -0600") {
                let (head, tail) = rest.split_at(at);
                let (head, time) = head.split_at(head.len() - 19);
                let time = chrono::NaiveDateTime::parse_from_str(time, "%Y-%m-%d %H:%M:%S")
                    .unwrap()
                    + chrono::Duration::hours(2);
                out.push_str(head);
                out.push_str(&time.format("%Y-%m-%d %H:%M:%S -0400").to_string());
                rest = &tail[" -0600".len()..];
            }
            out.push_str(rest);
            out
        })
        .collect()
}

fn every_kind(day: u32) -> Vec<String> {
    let mut records = days_records(true);
    // Readings share the Count and Logged windows in the offset comparisons.
    for minute in [552, 621] {
        records.push(heart(
            &format!(
                "2026-10-{day} {:02}:{:02}:10 -0600",
                minute / 60,
                minute % 60
            ),
            &format!("{}", 60 + minute % 20),
        ));
    }
    records.push(format!(
        r#"<Record type="HKQuantityTypeIdentifierStepCount" {SOURCE} unit="count" startDate="2026-10-{day} 09:00:00 -0600" endDate="2026-10-{day} 09:05:00 -0600" value="300"/>"#
    ));
    records.push(format!(
        r#"<Record type="HKCategoryTypeIdentifierHeadache" {SOURCE} startDate="2026-10-{day} 10:00:00 -0600" endDate="2026-10-{day} 10:30:00 -0600" value="HKCategoryValueSeverityMild"/>"#
    ));
    records
}

#[test]
fn an_export_at_another_offset_changes_nothing_and_doubles_nothing() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let records = every_kind(10);
    run(
        root,
        &export(&scratch.path().join("1"), &records),
        "20261012_120000",
        None,
    );
    let before = snapshot(root);
    let two = run(
        root,
        &export(&scratch.path().join("2"), &at_another_offset(&records)),
        "20261012_120001",
        None,
    );
    assert!(
        two.summary.contains("0 parts of your journal written"),
        "{}",
        two.summary
    );
    assert_eq!(two.published, 0, "{}", two.summary);
    let mut after = snapshot(root);
    after.retain(|path, _| !path.contains("20261012_120001"));
    assert_eq!(before, after);
}

#[test]
fn a_first_import_at_another_offset_lands_where_its_instants_belong() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let records = every_kind(10);
    let one = run(
        root,
        &export(&scratch.path().join("1"), &at_another_offset(&records)),
        "20261012_120000",
        None,
    );
    assert!(one.finished, "{}", one.summary);
    let elsewhere = tempfile::tempdir().unwrap();
    journal_in(elsewhere.path());
    run(
        elsewhere.path(),
        &export(&scratch.path().join("2"), &records),
        "20261012_120000",
        None,
    );
    let windows = |root: &Path| -> BTreeSet<String> {
        snapshot(root)
            .into_keys()
            .filter(|path| path.ends_with(BODY_RECORDS_FILE))
            .collect()
    };
    assert_eq!(windows(root), windows(elsewhere.path()));
}

fn journal_in(root: &Path) {
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        json!({"identity": {"timezone": "America/Denver"}}).to_string(),
    )
    .unwrap();
}

#[test]
fn a_readings_other_place_that_isnt_text_holds_it_back_and_the_run_goes_on() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(
        scratch.path(),
        &[
            heart("2026-10-10 06:35:10 -0600", "81"),
            heart("2026-10-11 06:35:10 -0600", "82"),
        ],
    );
    run(root, &source, "20261012_120000", None);
    fs::write(
        window(root, TEN, "060000_3600").join(BODY_RECORDS_FILE),
        [0xff, 0xfe, b'\n'],
    )
    .unwrap();
    // The reading now goes on a tile, and its other place can't be read as rows.
    let two = run(
        root,
        &export(
            &scratch.path().join("2"),
            &[
                heart("2026-10-10 06:35:10 -0600", "81"),
                heart("2026-10-11 06:35:10 -0600", "83"),
                workout(10),
            ],
        ),
        "20261012_120001",
        None,
    );
    assert!(
        two.summary.contains("1 reading wasn't added"),
        "{}",
        two.summary
    );
    assert_eq!(holding(root, "20261011", "83"), ["060000_3600"]);
}

fn stuck_window(root: &Path, day: &str, key: &str) {
    let stuck = window(root, day, key);
    fs::create_dir_all(&stuck).unwrap();
    fs::write(
        stuck.join("stream.json"),
        json!({"stream": STREAM, "prev_day": null, "prev_segment": null, "seq": 999}).to_string(),
    )
    .unwrap();
}

#[test]
fn two_adjacent_windows_that_wont_bind_dont_stop_the_days_after_them() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    for key in ["063000_300", "063500_300"] {
        stuck_window(root, "20261011", key);
    }
    let one = run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap();
    assert!(!one.finished);
    assert!(one.summary.contains("2026-10-11"), "{}", one.summary);
    assert!(!one.summary.contains("063000_300"), "{}", one.summary);
    assert!(
        window(root, "20261013", "063000_300")
            .join("stream.json")
            .is_file(),
        "the later day landed"
    );
    // Once the fault is gone, the next run saves them.
    for key in ["063000_300", "063500_300"] {
        fs::remove_file(window(root, "20261011", key).join("stream.json")).unwrap();
    }
    let two = run_apple_health_segments(root, &request(&source, "20261104_130000")).unwrap();
    assert!(two.finished, "{}", two.summary);
    assert_chain(root);
}

#[test]
fn a_stream_that_takes_no_window_stops_the_run_after_a_bounded_number_of_tries() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    // One window an hour, more of them than the run tries.
    let mut records = Vec::new();
    for day in 1..=4 {
        for hour in 0..24 {
            records.push(heart(
                &format!("2026-10-{day:02} {hour:02}:10:00 -0600"),
                "70",
            ));
        }
    }
    let source = export(scratch.path(), &records);
    let record = root.join("streams").join(format!("{STREAM}.json"));
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    fs::write(&record, "not a record").unwrap();
    let error = run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap_err();
    assert!(error.contains("in a row"), "{error}");
    assert!(
        !window(root, "20261004", "230000_3600").exists(),
        "the run stopped before the last day"
    );
    // Once the stream is whole, the same export resumes and finishes.
    fs::remove_file(&record).unwrap();
    let two = run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap();
    assert!(two.finished, "{}", two.summary);
    assert!(window(root, "20261004", "230000_3600").is_dir());
}

/// Every window folder without records.
fn folders_without_records(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for day in fs::read_dir(root.join("chronicle")).unwrap().flatten() {
        let Ok(windows) = fs::read_dir(day.path().join(STREAM)) else {
            continue;
        };
        for window in windows.flatten() {
            if window.path().is_dir() && !window.path().join(BODY_RECORDS_FILE).exists() {
                found.push(window.path());
            }
        }
    }
    found
}

#[test]
fn a_faceted_run_stopped_partway_leaves_no_folder_holding_only_a_facet() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    let stop = stopping_at("written", 1);
    let mut stopped = AppleSegmentsRequest::new(&source, "20261104_120000", Some("health"));
    stopped.spill_bytes = 4096;
    stopped.stop_at = Some(&stop);
    run_apple_health_segments(root, &stopped).unwrap_err();
    assert_eq!(folders_without_records(root), Vec::<PathBuf>::new());

    // Another export publishes the half-done day, facet and all.
    let other = export(
        &scratch.path().join("o"),
        &[heart("2026-10-20 06:31:10 -0600", "70")],
    );
    run(root, &other, "20261104_130000", None);
    assert_eq!(folders_without_records(root), Vec::<PathBuf>::new());
    let written: Vec<PathBuf> = fs::read_dir(root.join("chronicle").join(TEN).join(STREAM))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join(BODY_RECORDS_FILE).is_file())
        .collect();
    assert!(!written.is_empty());
    assert!(
        written
            .iter()
            .all(|path| path.join("talents/facets.json").is_file()),
        "{written:?}"
    );
}

#[test]
fn a_faceted_run_resumed_gives_every_window_its_facet() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    let stop = stopping_at("written", 2);
    let mut stopped = AppleSegmentsRequest::new(&source, "20261104_120000", Some("health"));
    stopped.spill_bytes = 4096;
    stopped.stop_at = Some(&stop);
    run_apple_health_segments(root, &stopped).unwrap_err();
    let mut again = AppleSegmentsRequest::new(&source, "20261104_120000", Some("health"));
    again.spill_bytes = 4096;
    let done = run_apple_health_segments(root, &again).unwrap();
    assert!(done.finished, "{}", done.summary);
    assert_eq!(folders_without_records(root), Vec::<PathBuf>::new());
    for day in fs::read_dir(root.join("chronicle")).unwrap().flatten() {
        let Ok(windows) = fs::read_dir(day.path().join(STREAM)) else {
            continue;
        };
        for window in windows.flatten() {
            if window.path().is_dir() {
                assert!(
                    window.path().join("talents/facets.json").is_file(),
                    "{:?}",
                    window.path()
                );
            }
        }
    }
}

#[test]
fn a_run_missing_from_staging_is_not_trusted_and_no_row_goes_missing() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    let whole = tempfile::tempdir().unwrap();
    journal_in(whole.path());
    run_apple_health_segments(whole.path(), &request(&source, "20261104_120000")).unwrap();

    let stop = stopping_at("pending", 2);
    let mut stopped = request(&source, "20261104_120000");
    stopped.stop_at = Some(&stop);
    run_apple_health_segments(root, &stopped).unwrap_err();
    // Power loss: a run's folder entry never reached the disk.
    let runs = runs_in(&root.join(RUN_DIR_REL).join("placed")).unwrap();
    assert!(runs.len() > 1, "the placed rows spilled");
    fs::remove_file(runs.last().unwrap()).unwrap();

    let done = run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap();
    assert!(done.finished, "{}", done.summary);
    assert!(done.summary.contains("published again"), "{}", done.summary);
    let ids = |root: &Path| -> BTreeMap<String, Vec<String>> {
        snapshot(root)
            .into_iter()
            .filter(|(path, _)| path.ends_with(BODY_RECORDS_FILE))
            .map(|(path, bytes)| {
                let rows = decode_rows(&String::from_utf8(bytes).unwrap()).unwrap();
                (path, rows.into_iter().map(|row| row.id).collect())
            })
            .collect()
    };
    assert_eq!(ids(root), ids(whole.path()));
    assert_chain(root);
}

#[test]
fn a_kill_during_a_repair_leaves_no_import_reading_as_running() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    let stop = stopping_at("pending", 3);
    let mut stopped = request(&source, "20261104_120000");
    stopped.stop_at = Some(&stop);
    run_apple_health_segments(root, &stopped).unwrap_err();
    fs::remove_dir_all(root.join(RUN_DIR_REL).join("placed")).unwrap();
    let stop2 = stopping_at("repaired", 1);
    let mut repairing = request(&source, "20261104_130000");
    repairing.stop_at = Some(&stop2);
    run_apple_health_segments(root, &repairing).unwrap_err();
    let done = run_apple_health_segments(root, &request(&source, "20261104_140000")).unwrap();
    assert!(done.finished, "{}", done.summary);
    assert_chain(root);
    for id in ["20261104_120000", "20261104_130000"] {
        let metadata = read_provenance(root, id).unwrap().unwrap();
        let AttemptRead::Present(facts) = read_attempt_facts(&metadata) else {
            panic!("{id} has an attempt");
        };
        assert_ne!(facts.state, AttemptState::Running, "{id}");
    }
}

#[test]
fn failures_with_a_published_day_between_them_arent_in_a_row() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    // Every other day holds one stuck window, more of them than stop a run if they
    // were in a row; the days between publish whole.
    let days: Vec<chrono::NaiveDate> = (0..140)
        .map(|n| chrono::NaiveDate::from_ymd_opt(2026, 3, 1).unwrap() + chrono::Duration::days(n))
        .collect();
    let records: Vec<String> = days
        .iter()
        .map(|day| heart(&format!("{} 06:31:10 -0600", day.format("%Y-%m-%d")), "70"))
        .collect();
    for day in days.iter().step_by(2) {
        stuck_window(root, &day.format("%Y%m%d").to_string(), "060000_3600");
    }
    let source = export(scratch.path(), &records);
    let one = run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap();
    assert!(!one.finished);
    let last = days.last().unwrap().format("%Y%m%d").to_string();
    assert!(
        window(root, &last, "060000_3600")
            .join("stream.json")
            .is_file(),
        "the last day landed"
    );
}

#[test]
fn a_stream_that_takes_no_window_stops_the_run_though_each_day_holds_only_one() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let days: Vec<chrono::NaiveDate> = (0..70)
        .map(|n| chrono::NaiveDate::from_ymd_opt(2026, 3, 1).unwrap() + chrono::Duration::days(n))
        .collect();
    let records: Vec<String> = days
        .iter()
        .map(|day| heart(&format!("{} 06:31:10 -0600", day.format("%Y-%m-%d")), "70"))
        .collect();
    let source = export(scratch.path(), &records);
    let record = root.join("streams").join(format!("{STREAM}.json"));
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    fs::write(&record, "not a record").unwrap();
    let error = run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap_err();
    assert!(error.contains("64 segments in a row"), "{error}");
    assert!(error.contains("carry on"), "{error}");
}

fn raw_export(directory: &Path, bytes: &[u8]) -> PathBuf {
    fs::create_dir_all(directory).unwrap();
    fs::write(directory.join("export.xml"), bytes).unwrap();
    directory.to_path_buf()
}

fn saved_windows(root: &Path) -> usize {
    if !root.join("chronicle").exists() {
        return 0;
    }
    snapshot(root)
        .keys()
        .filter(|path| path.ends_with(BODY_RECORDS_FILE))
        .count()
}

#[test]
fn an_export_that_doesnt_close_saves_nothing_and_says_it_looks_incomplete() {
    let good = heart("2026-10-10 06:31:10 -0600", "70");
    let cut = heart("2026-10-10 07:31:10 -0600", "71");
    let open_record = good.replace("/>", "><MetadataEntry key=\"a\" value=\"b\"/>");
    for bytes in [
        // Between records, inside a tag, inside a record with children.
        format!("<HealthData>{good}\n"),
        format!("<HealthData>{good}{}", &cut[..40]),
        format!("<HealthData>{good}{open_record}"),
        // Nothing at all, and XML that isn't an export.
        String::new(),
        "<cda:ClinicalDocument/>".to_owned(),
        format!("{good}{cut}"),
    ] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        let source = raw_export(scratch.path(), bytes.as_bytes());
        let error =
            run_apple_health_segments(root, &request(&source, "20261012_120000")).unwrap_err();
        assert!(error.contains("looks incomplete"), "{bytes}: {error}");
        assert_eq!(saved_windows(root), 0, "{bytes}");
        // The whole export, taken again, goes in.
        let whole = raw_export(
            &scratch.path().join("whole"),
            format!("<HealthData>{good}</HealthData>").as_bytes(),
        );
        let done = run(root, &whole, "20261012_130000", None);
        assert!(done.finished, "{bytes}: {}", done.summary);
        assert_eq!(saved_windows(root), 1, "{bytes}");
    }
}

/// What an owner holds is a zip, and a copy of one that stopped short has no end.
#[test]
fn a_zip_cut_off_in_copying_saves_nothing_and_says_it_looks_incomplete() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("export.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    zip.start_file(
        "apple_health_export/export.xml",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    std::io::Write::write_all(
        &mut zip,
        format!(
            "<HealthData>{}</HealthData>",
            heart("2026-10-10 06:31:10 -0600", "70")
        )
        .as_bytes(),
    )
    .unwrap();
    zip.finish().unwrap();
    let whole = fs::read(&path).unwrap();
    fs::write(&path, &whole[..whole.len() / 2]).unwrap();
    let error = run_apple_health_segments(root, &request(&path, "20261012_120000")).unwrap_err();
    assert!(error.contains("looks incomplete"), "{error}");
    assert_eq!(saved_windows(root), 0);
}

#[test]
fn an_export_that_closes_with_nothing_in_it_is_a_finished_import_of_nothing() {
    for bytes in ["<HealthData></HealthData>", "<HealthData/>"] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        let source = raw_export(scratch.path(), bytes.as_bytes());
        let done = run(root, &source, "20261012_120000", None);
        assert!(done.finished, "{bytes}: {}", done.summary);
        assert_eq!(saved_windows(root), 0);
    }
}

#[test]
fn one_record_that_cant_be_read_is_counted_and_the_rest_are_saved() {
    let child = |attributes: &str| {
        format!(
            r#"<Record type="HKQuantityTypeIdentifierHeartRate" {SOURCE} startDate="2026-10-10 08:00:00 -0600" endDate="2026-10-10 08:00:00 -0600" value="1"><MetadataEntry {attributes}/></Record>"#
        )
        .into_bytes()
    };
    let cases: [Vec<u8>; 7] = [
        // An unescaped ampersand, a repeated attribute, bytes that aren't text.
        br#"<Record type="HKQuantityTypeIdentifierHeartRate" sourceName="A&B" startDate="2026-10-10 08:00:00 -0600" endDate="2026-10-10 08:00:00 -0600" value="1"/>"#.to_vec(),
        br#"<Record type="HKQuantityTypeIdentifierHeartRate" value="2" startDate="2026-10-10 08:00:00 -0600" endDate="2026-10-10 08:00:00 -0600" value="1"/>"#.to_vec(),
        b"<Record type=\"HKQuantityTypeIdentifierHeartRate\" sourceName=\"A\xff\" startDate=\"2026-10-10 08:00:00 -0600\" endDate=\"2026-10-10 08:00:00 -0600\" value=\"1\"/>".to_vec(),
        // The same on an element with children, whose children go with it.
        br#"<Record sourceName="A&B"><MetadataEntry key="a" value="b"/><HealthData/></Record>"#.to_vec(),
        // What's inside it goes unread, even what would be read outside it.
        br#"<Record sourceName="A&B"><Record type="HKQuantityTypeIdentifierHeartRate" sourceName="w" startDate="2026-10-10 08:00:00 -0600" endDate="2026-10-10 08:00:00 -0600" value="999"/></Record>"#.to_vec(),
        // On a child, which takes its record with it.
        child(r#"key="a&b" value="c""#),
        child(r#"key="a" key="b" value="c""#),
    ];
    for bad in cases {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        let mut xml =
            format!("<HealthData>{}", heart("2026-10-10 06:31:10 -0600", "70")).into_bytes();
        xml.extend_from_slice(&bad);
        xml.extend_from_slice(
            format!("{}</HealthData>", heart("2026-10-11 06:31:10 -0600", "71")).as_bytes(),
        );
        let source = raw_export(scratch.path(), &xml);
        let done = run(root, &source, "20261012_120000", None);
        let shown = String::from_utf8_lossy(&bad).into_owned();
        assert!(done.finished, "{shown}: {}", done.summary);
        assert_eq!(saved_windows(root), 2, "{shown}");
        assert!(
            done.summary.contains("## couldn't be read"),
            "{}",
            done.summary
        );
        assert!(done.summary.contains("attributes"), "{}", done.summary);
        // Twice over, the same.
        let again = run(root, &source, "20261012_130000", None);
        assert!(again.finished);
        assert_eq!(saved_windows(root), 2, "{shown}");
    }
}

#[test]
fn a_deleted_window_that_gains_a_stray_file_stays_deleted() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(scratch.path(), &[heart("2026-10-10 06:31:10 -0600", "70")]);
    run(root, &source, "20261012_120000", None);
    delete(root, "20261010", "060000_3600");
    let gone = window(root, "20261010", "060000_3600");
    assert!(gone.join("tombstone.json").is_file());
    // A sync client's conflict copy, say.
    fs::write(gone.join("stream (conflicted copy).json"), "{}").unwrap();
    let after = run(root, &source, "20261012_130000", None);
    assert!(
        !window(root, "20261010", "060000_3601").exists(),
        "the deleted reading came back beside its tombstone"
    );
    assert!(!gone.join(BODY_RECORDS_FILE).exists());
    assert!(
        after.summary.contains("1 stayed deleted"),
        "{}",
        after.summary
    );
}

#[test]
fn another_folder_export_of_the_same_size_is_read_not_taken_for_the_old_one() {
    for (point, elsewhere) in [("scanned", false), ("pending", true), ("written", true)] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        let source = export(scratch.path(), &[heart("2026-10-10 06:31:10 -0600", "70")]);
        let stop = stopping_at(point, 1);
        let mut stopped = request(&source, "20261104_120000");
        stopped.stop_at = Some(&stop);
        run_apple_health_segments(root, &stopped).unwrap_err();
        // The same length, in the same folder or another.
        let other = if elsewhere {
            export(
                &scratch.path().join("elsewhere"),
                &[heart("2026-10-20 06:31:10 -0600", "71")],
            )
        } else {
            export(scratch.path(), &[heart("2026-10-20 06:31:10 -0600", "71")])
        };
        let done = run(root, &other, "20261104_130000", None);
        assert!(done.finished, "{point}: {}", done.summary);
        assert_ne!(done.import_id, "20261104_120000", "{point}");
        assert!(
            window(root, "20261020", "060000_3600")
                .join(BODY_RECORDS_FILE)
                .is_file(),
            "{point}: the other export reported finished without being read"
        );
    }
}

#[test]
fn the_same_folder_export_still_carries_on_under_its_import() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    let stop = stopping_at("written", 2);
    let mut stopped = request(&source, "20261104_120000");
    stopped.stop_at = Some(&stop);
    run_apple_health_segments(root, &stopped).unwrap_err();
    let done = run(root, &source, "20261104_130000", None);
    assert!(done.finished, "{}", done.summary);
    assert_eq!(done.import_id, "20261104_120000");
}

/// A file the run isn't allowed to read says nothing of what it holds.
#[cfg(unix)]
#[test]
fn a_window_that_refuses_reading_stops_the_run_and_the_next_carries_on() {
    use std::os::unix::fs::PermissionsExt;
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(scratch.path(), &[heart("2026-10-10 06:31:10 -0600", "70")]);
    run(root, &source, "20261012_120000", None);
    let file = window(root, "20261010", "060000_3600").join(BODY_RECORDS_FILE);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&file).is_ok() {
        // Root reads anything, so there's no refusal to see.
        return;
    }
    let more = export(
        &scratch.path().join("more"),
        &[
            heart("2026-10-10 06:31:10 -0600", "70"),
            heart("2026-10-10 06:45:10 -0600", "77"),
        ],
    );
    let refused = run_apple_health_segments(root, &request(&more, "20261012_130000"));
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    let error = refused.unwrap_err();
    assert!(error.contains(BODY_RECORDS_FILE), "{error}");
    assert!(
        root.join(RUN_DIR_REL).join("state.json").is_file(),
        "its state was kept"
    );
    let done = run(root, &more, "20261012_140000", None);
    assert!(done.finished, "{}", done.summary);
    assert_eq!(done.import_id, "20261012_130000");
    let rows = decode_rows(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        !done.summary.contains("unreadable and left alone")
            || done.summary.contains("0 unreadable"),
        "{}",
        done.summary
    );
}

/// A workout of `slots` five-minute slots on 10-10, with a day either side of it.
fn long_workout_export(directory: &Path, slots: u32) -> PathBuf {
    let minutes = 6 * 60 + slots * 5;
    export(
        directory,
        &[
            heart("2026-10-09 06:31:10 -0600", "70"),
            format!(
                r#"<Workout workoutActivityType="HKWorkoutActivityTypeHiking" {SOURCE} startDate="2026-10-10 06:00:00 -0600" endDate="2026-10-10 {:02}:{:02}:00 -0600"/>"#,
                minutes / 60,
                minutes % 60
            ),
            heart("2026-10-12 06:31:10 -0600", "71"),
        ],
    )
}

fn stick_slot(root: &Path, slot: u32) {
    let minutes = 6 * 60 + slot * 5;
    stuck_window(
        root,
        "20261010",
        &format!("{:02}{:02}00_300", minutes / 60, minutes % 60),
    );
}

fn later_day_landed(root: &Path) -> bool {
    window(root, "20261012", "060000_3600")
        .join(BODY_RECORDS_FILE)
        .is_file()
}

#[test]
fn one_short_of_the_bound_of_stuck_windows_lets_the_later_day_land_in_the_same_run() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = long_workout_export(scratch.path(), 63);
    (0..63).for_each(|slot| stick_slot(root, slot));
    let one = run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap();
    assert!(!one.finished);
    assert!(later_day_landed(root));
}

#[test]
fn a_day_with_more_stuck_windows_than_the_bound_is_recorded_and_the_next_run_goes_past_it() {
    for slots in [64_u32, 72] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        let source = long_workout_export(scratch.path(), slots);
        (0..slots).for_each(|slot| stick_slot(root, slot));
        let error =
            run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap_err();
        assert!(error.contains("carry on"), "{error}");
        assert!(error.contains("after 2026-10-10"), "{error}");
        assert!(
            error.contains(&format!("{slots} segments in a row")),
            "{error}"
        );
        // It stopped having recorded the day, and said so truly: the next run is past it.
        assert!(!later_day_landed(root));
        let state: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(RUN_DIR_REL).join("state.json")).unwrap())
                .unwrap();
        assert_eq!(state["done_through"], "20261010");
        assert_eq!(state["failed"].as_array().unwrap().len(), slots as usize);
        let two = run_apple_health_segments(root, &request(&source, "20261104_130000")).unwrap();
        assert!(later_day_landed(root));
        assert!(!two.finished);
        assert_eq!(two.import_id, "20261104_120000");
        assert!(two.summary.contains("not saved yet"), "{}", two.summary);
        // Unstuck, a later run with the same export saves them.
        (0..slots).for_each(|slot| {
            let minutes = 6 * 60 + slot * 5;
            fs::remove_file(
                window(
                    root,
                    "20261010",
                    &format!("{:02}{:02}00_300", minutes / 60, minutes % 60),
                )
                .join("stream.json"),
            )
            .unwrap();
        });
        let three = run(root, &source, "20261104_140000", None);
        assert!(three.finished, "{}", three.summary);
    }
}

#[test]
fn a_window_bound_between_stuck_ones_means_they_arent_in_a_row() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = long_workout_export(scratch.path(), 81);
    (0..81)
        .filter(|slot| *slot != 40)
        .for_each(|slot| stick_slot(root, slot));
    let one = run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap();
    assert!(!one.finished);
    assert!(
        later_day_landed(root),
        "forty and forty, with one bound between them, stopped the run"
    );
}

/// The privacy order: a kill between a window's new rows and its new facet mustn't
/// leave those rows under the old one.
#[test]
fn a_kill_never_leaves_new_rows_under_a_windows_old_facet() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(scratch.path(), &[heart("2026-10-10 06:31:10 -0600", "70")]);
    run(root, &source, "20261012_120000", Some("health"));
    let more = export(
        &scratch.path().join("more"),
        &[
            heart("2026-10-10 06:31:10 -0600", "70"),
            heart("2026-10-10 06:45:10 -0600", "77"),
        ],
    );
    let stop = stopping_at("written", 1);
    let mut stopped = request(&more, "20261012_130000");
    stopped.facet = Some("other");
    stopped.listed = ListedMoves::Move;
    stopped.stop_at = Some(&stop);
    run_apple_health_segments(root, &stopped).unwrap_err();
    let dir = window(root, "20261010", "060000_3600");
    let rows = decode_rows(&fs::read_to_string(dir.join(BODY_RECORDS_FILE)).unwrap()).unwrap();
    assert_eq!(rows.len(), 2, "the kill came after the new row landed");
    assert_eq!(
        fs::read_to_string(dir.join("talents/facets.json")).unwrap(),
        facet_file_text("other")
    );
}

#[test]
fn a_finished_import_isnt_told_it_was_replaced_before_it_finished() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(scratch.path(), &[heart("2026-10-10 06:31:10 -0600", "70")]);
    let stop = stopping_at("finished", 1);
    let mut stopped = request(&source, "20261104_120000");
    stopped.stop_at = Some(&stop);
    run_apple_health_segments(root, &stopped).unwrap_err();
    let summary = root.join("imports/20261104_120000/summary.md");
    let before = fs::read_to_string(&summary).unwrap();
    let other = export(
        &scratch.path().join("other"),
        &[heart("2026-10-20 06:31:10 -0600", "700")],
    );
    run(root, &other, "20261104_130000", None);
    assert_eq!(before, fs::read_to_string(&summary).unwrap());
}

fn stage(start: &str, end: &str, value: &str) -> String {
    format!(
        r#"<Record type="HKCategoryTypeIdentifierSleepAnalysis" {SOURCE} startDate="{start}" endDate="{end}" value="HKCategoryValueSleepAnalysis{value}"/>"#
    )
}

/// A night's stored end is read to keep it from shrinking, so a read that fails has
/// to stop the run, not stage a shorter night to write once the file reads again.
#[cfg(unix)]
#[test]
fn a_stop_on_an_unreadable_window_doesnt_shorten_a_night_on_resume() {
    use std::os::unix::fs::PermissionsExt;
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
    let file = window(root, "20261010", "230000_3600").join(BODY_RECORDS_FILE);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&file).is_ok() {
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }
    let refused = run_apple_health_segments(root, &request(&fewer, "20261012_130000"));
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(refused.is_err());
    let resumed = run(root, &fewer, "20261012_140000", None);
    assert!(resumed.finished, "{}", resumed.summary);
    assert_eq!(
        body_bytes(root, "20261010", "230000_3600"),
        first,
        "the night shrank"
    );
}

/// Running again changes nothing about a folder where a file should be, so it's
/// counted and the days after it land. On Windows too, where opening one reads as
/// access denied.
#[test]
fn a_window_whose_file_is_a_folder_is_counted_and_later_days_land() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(
        scratch.path(),
        &[
            heart("2026-10-10 06:31:10 -0600", "70"),
            heart("2026-10-12 06:31:10 -0600", "71"),
        ],
    );
    fs::create_dir_all(window(root, "20261010", "060000_3600").join(BODY_RECORDS_FILE)).unwrap();
    let done = run(root, &source, "20261104_120000", None);
    assert!(done.finished, "{}", done.summary);
    assert!(done.summary.contains("1 unreadable"), "{}", done.summary);
    assert!(later_day_landed(root));
}

/// A moment's unreadable export isn't another export: the run stops before touching
/// the earlier one, whose record stays true and which carries on after.
#[cfg(unix)]
#[test]
fn an_export_that_cant_be_read_for_a_moment_leaves_the_earlier_run_to_carry_on() {
    use std::os::unix::fs::PermissionsExt;
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    let stop = stopping_at("written", 2);
    let mut stopped = request(&source, "20261104_120000");
    stopped.stop_at = Some(&stop);
    run_apple_health_segments(root, &stopped).unwrap_err();
    let state = fs::read(root.join(RUN_DIR_REL).join("state.json")).unwrap();
    let export_file = source.join("export.xml");
    fs::set_permissions(&export_file, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&export_file).is_ok() {
        fs::set_permissions(&export_file, fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }
    let refused = run_apple_health_segments(root, &request(&source, "20261104_130000"));
    fs::set_permissions(&export_file, fs::Permissions::from_mode(0o600)).unwrap();
    refused.unwrap_err();
    assert_eq!(
        fs::read(root.join(RUN_DIR_REL).join("state.json")).unwrap(),
        state
    );
    assert!(!root.join("imports/20261104_130000").exists());
    let summary = root.join("imports/20261104_120000/summary.md");
    assert!(
        fs::read_to_string(&summary).map_or(true, |text| !text.contains("replaced")),
        "the earlier import was told it was replaced"
    );
    let done = run(root, &source, "20261104_140000", None);
    assert_eq!(done.import_id, "20261104_120000");
    assert!(done.finished, "{}", done.summary);
}

/// The other side of what a read error stops: a window that truly isn't rows, in the
/// place its reading goes, is counted, left alone, and the days after it land.
#[test]
fn a_window_that_isnt_rows_where_its_reading_goes_is_counted_and_the_run_goes_on() {
    for bad in [&[0xff_u8, 0xfe, b'\n'][..], &b"{not json\n"[..]] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        let source = export(
            scratch.path(),
            &[
                heart("2026-10-10 06:35:10 -0600", "81"),
                heart("2026-10-12 06:35:10 -0600", "82"),
            ],
        );
        let file = window(root, "20261010", "060000_3600").join(BODY_RECORDS_FILE);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, bad).unwrap();
        let done = run(root, &source, "20261012_120000", None);
        assert!(done.summary.contains("1 unreadable"), "{}", done.summary);
        assert_eq!(fs::read(&file).unwrap(), bad);
        assert!(
            window(root, "20261012", "060000_3600")
                .join(BODY_RECORDS_FILE)
                .is_file()
        );
    }
}

fn two_facets(root: &Path) {
    for facet in ["work", "body"] {
        let dir = root.join("facets").join(facet);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("facet.json"),
            json!({"title": facet, "description": facet}).to_string(),
        )
        .unwrap();
    }
}

fn records_in(root: &Path, facet: &str, day: &str) -> usize {
    fs::read_to_string(root.join(format!("facets/{facet}/activities/{day}.jsonl")))
        .unwrap_or_default()
        .lines()
        .count()
}

#[test]
fn a_run_nobody_answered_for_moves_nothing_already_filed() {
    let journal = journal();
    let root = journal.path();
    two_facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let source = export(&scratch.path().join("1"), &[workout(10)]);
    run(root, &source, "20261012_120000", Some("work"));
    let quiet = AppleSegmentsRequest::new(&source, "20261012_130000", Some("body"));
    let outcome = run_apple_health_segments(root, &quiet).unwrap();
    assert_eq!(
        records_in(root, "work", "20261010"),
        1,
        "{}",
        outcome.summary
    );
    assert_eq!(records_in(root, "body", "20261010"), 0);
    assert!(
        !outcome.summary.contains("moved to this import"),
        "{}",
        outcome.summary
    );
    let stream = root.join("chronicle/20261010").join(STREAM);
    for entry in fs::read_dir(&stream).unwrap().flatten() {
        let facets = entry.path().join("talents/facets.json");
        if facets.is_file() {
            assert_eq!(
                fs::read_to_string(&facets).unwrap(),
                facet_file_text("work"),
                "{}",
                entry.path().display()
            );
        }
    }
}

#[test]
fn the_owners_answer_moves_the_record_with_its_window_and_says_how_many_from_where() {
    let journal = journal();
    let root = journal.path();
    two_facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let source = export(&scratch.path().join("1"), &[workout(10)]);
    run(root, &source, "20261012_120000", Some("work"));
    let outcome = run(root, &source, "20261012_130000", Some("body"));
    assert_eq!(
        records_in(root, "work", "20261010"),
        0,
        "{}",
        outcome.summary
    );
    assert_eq!(records_in(root, "body", "20261010"), 1);
    assert!(
        outcome.summary.contains(
            "1 record already listed on the activities page moved to this import's facet, from your work facet."
        ),
        "{}",
        outcome.summary
    );
}

#[test]
fn more_than_the_owner_was_told_moves_none_and_says_so() {
    let journal = journal();
    let root = journal.path();
    two_facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let source = export(&scratch.path().join("1"), &[workout(10), workout(11)]);
    run(root, &source, "20261012_120000", Some("work"));
    let mut told = AppleSegmentsRequest::new(&source, "20261012_130000", Some("body"));
    told.listed = ListedMoves::MoveAtMost(1);
    let outcome = run_apple_health_segments(root, &told).unwrap();
    assert_eq!(
        records_in(root, "work", "20261010"),
        1,
        "{}",
        outcome.summary
    );
    assert!(
        outcome
            .summary
            .contains("nothing already listed moved: 2 records would have, and you were told 1."),
        "{}",
        outcome.summary
    );
}

fn add_listed_elsewhere(root: &Path, day: &str, key: &str) {
    let path = root.join(format!("facets/work/activities/{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!(
        "{}\n",
        json!({"id": format!("body_{key}"), "source": "body", "activity": "workout",
            "account": {"key": format!("import.apple_health\u{1f}{key}")}})
    ));
    fs::write(path, text).unwrap();
}

#[test]
fn a_resumed_run_counts_again_and_moves_no_more_than_the_owner_was_told() {
    let journal = journal();
    let root = journal.path();
    two_facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let source = export(&scratch.path().join("1"), &[workout(10), workout(11)]);
    run(root, &source, "20261012_120000", Some("work"));
    // Told two, and stopped once the first day's record has moved.
    let stop = stopping_at("pending", 2);
    let mut told = AppleSegmentsRequest::new(&source, "20261012_130000", Some("body"));
    told.listed = ListedMoves::MoveAtMost(2);
    told.stop_at = Some(&stop);
    run_apple_health_segments(root, &told).unwrap_err();
    assert_eq!(records_in(root, "body", "20261010"), 1);
    let Read::Trusted(prior) = read_state(root, &APPLE) else {
        panic!("no saved run");
    };
    let mut kept = (*prior).clone();
    keep_half_done_day_filed(root, &APPLE, &mut kept);
    for window in &kept.pending.as_ref().unwrap().windows {
        assert!(!window.publish);
        let counts = window.counts.as_ref().unwrap();
        assert_eq!(
            (counts.refiled, counts.published, counts.unchanged),
            (0, 0, 1)
        );
    }
    // Something else lands elsewhere while it's stopped, and the re-run is unattended.
    add_listed_elsewhere(root, "20261001", "later");
    let quiet = AppleSegmentsRequest::new(&source, "20261012_130000", Some("body"));
    let outcome = run_apple_health_segments(root, &quiet).unwrap();
    assert_eq!(
        records_in(root, "work", "20261011"),
        1,
        "{}",
        outcome.summary
    );
    assert_ne!(
        outcome.import_id, prior.import_id,
        "a lost move bound takes over"
    );
    let earlier_summary = fs::read_to_string(
        root.join("imports")
            .join(&prior.import_id)
            .join("summary.md"),
    )
    .unwrap();
    assert!(
        earlier_summary.contains(
            "the rest of what's already listed stayed where it was: 3 records would have moved in all, and you were told 2."
        ),
        "{}",
        earlier_summary
    );
    let metadata = read_provenance(root, &prior.import_id).unwrap().unwrap();
    let AttemptRead::Present(facts) = read_attempt_facts(&metadata) else {
        panic!("no former attempt");
    };
    assert_eq!(
        facts.continued_by.as_deref(),
        Some(outcome.import_id.as_str())
    );
    // The half-done day was decided with moving on, and still stays where it is.
    let stream = root.join("chronicle/20261011").join(STREAM);
    for entry in fs::read_dir(&stream).unwrap().flatten() {
        let facets = entry.path().join("talents/facets.json");
        if facets.is_file() {
            assert_eq!(
                fs::read_to_string(&facets).unwrap(),
                facet_file_text("work"),
                "{}",
                entry.path().display()
            );
        }
    }
}

#[test]
fn a_resumed_run_within_what_the_owner_was_told_finishes_moving() {
    let journal = journal();
    let root = journal.path();
    two_facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let source = export(&scratch.path().join("1"), &[workout(10), workout(11)]);
    run(root, &source, "20261012_120000", Some("work"));
    let stop = stopping_at("pending", 2);
    let mut told = AppleSegmentsRequest::new(&source, "20261012_130000", Some("body"));
    told.listed = ListedMoves::MoveAtMost(2);
    told.stop_at = Some(&stop);
    run_apple_health_segments(root, &told).unwrap_err();
    let quiet = AppleSegmentsRequest::new(&source, "20261012_130000", Some("body"));
    let outcome = run_apple_health_segments(root, &quiet).unwrap();
    assert_eq!(
        records_in(root, "body", "20261011"),
        1,
        "{}",
        outcome.summary
    );
    assert!(
        outcome.summary.contains("2 records already listed"),
        "{}",
        outcome.summary
    );
}

#[test]
fn a_takeover_counts_again_and_moves_no_more_than_the_owner_was_told() {
    let journal = journal();
    let root = journal.path();
    two_facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let source = export(&scratch.path().join("1"), &[workout(10), workout(11)]);
    run(root, &source, "20261012_120000", Some("work"));
    let stop = stopping_at("written", 1);
    let mut told = AppleSegmentsRequest::new(&source, "20261012_130000", Some("body"));
    told.listed = ListedMoves::MoveAtMost(2);
    told.stop_at = Some(&stop);
    let more = export(
        &scratch.path().join("2"),
        &[
            workout(10),
            workout(11),
            heart("2026-10-10 06:40:10 -0600", "70"),
        ],
    );
    told.source = &more;
    run_apple_health_segments(root, &told).unwrap_err();
    add_listed_elsewhere(root, "20261001", "later-a");
    add_listed_elsewhere(root, "20261002", "later-b");
    let other = export(
        &scratch.path().join("3"),
        &[heart("2026-10-22 06:31:10 -0600", "70")],
    );
    run(root, &other, "20261012_140000", None);
    let old = fs::read_to_string(root.join("imports/20261012_130000/summary.md")).unwrap();
    assert_eq!(records_in(root, "work", "20261010"), 1, "{old}");
    assert_eq!(records_in(root, "body", "20261010"), 0, "{old}");
    assert!(
        old.contains("nothing already listed moved: 4 records would have, and you were told 2."),
        "{old}"
    );
}

#[test]
fn a_takeovers_moves_and_only_the_windows_it_reached_reach_the_old_imports_summary() {
    let journal = journal();
    let root = journal.path();
    two_facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let source = export(&scratch.path().join("1"), &[workout(10)]);
    run(root, &source, "20261012_120000", Some("work"));
    // Rewrites two windows of the day, then stops after the first.
    let more = export(
        &scratch.path().join("2"),
        &[
            workout(10),
            heart("2026-10-10 06:40:10 -0600", "70"),
            heart("2026-10-10 09:40:10 -0600", "71"),
        ],
    );
    let stop = stopping_at("written", 1);
    let mut moving = AppleSegmentsRequest::new(&more, "20261012_130000", Some("body"));
    moving.listed = ListedMoves::Move;
    moving.stop_at = Some(&stop);
    run_apple_health_segments(root, &moving).unwrap_err();
    let other = export(
        &scratch.path().join("3"),
        &[heart("2026-10-22 06:31:10 -0600", "70")],
    );
    run(root, &other, "20261012_140000", None);
    let old = fs::read_to_string(root.join("imports/20261012_130000/summary.md")).unwrap();
    assert!(
        old.contains(
            "1 record already listed on the activities page moved to this import's facet, from your work facet."
        ),
        "{old}"
    );
    assert!(old.contains("1 part of your journal written"), "{old}");
}

#[test]
fn a_new_record_that_joins_a_workout_the_owner_hid_says_so() {
    let journal = journal();
    let root = journal.path();
    two_facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let watch = export(&scratch.path().join("1"), &[workout(10)]);
    run(root, &watch, "20261012_120000", Some("work"));
    let file = root.join("facets/work/activities/20261010.jsonl");
    let hidden = fs::read_to_string(&file)
        .unwrap()
        .replace(r#""hidden":false"#, r#""hidden":true"#);
    fs::write(&file, hidden).unwrap();
    let band = export(
        &scratch.path().join("2"),
        &[workout(10).replace("Synthetic Watch", "Synthetic Band")],
    );
    let outcome = run(root, &band, "20261012_130000", Some("work"));
    assert!(
        outcome
            .summary
            .contains("1 record joined a workout you'd hidden, so it's hidden too."),
        "{}",
        outcome.summary
    );
}

#[test]
fn an_exports_manifest_says_the_days_it_reached() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(
        scratch.path(),
        &[
            heart("2026-10-10 06:31:10 -0600", "70"),
            heart("2026-10-14 06:31:10 -0600", "71"),
        ],
    );
    let outcome = run(root, &source, "20261015_120000", None);
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(
            root.join("imports")
                .join(&outcome.import_id)
                .join("manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["span"], json!([["20261010", "20261014"]]));
}

#[test]
fn a_summary_holds_no_stage_code_or_journal_key() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    stuck_window(root, "20261011", "063000_300");
    let outcome = run_apple_health_segments(root, &request(&source, "20261104_120000")).unwrap();
    for raw in [
        "HKQuantity",
        "063000_300",
        "end_before_start",
        "window",
        "connection",
    ] {
        assert!(!outcome.summary.contains(raw), "{raw}: {}", outcome.summary);
    }
}

/// A takeover lists the windows the stopped run got to and no others, because a
/// day in a listing counts as read again for the old-import line.
#[test]
fn a_takeovers_listing_names_only_the_windows_its_run_reached() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(
        scratch.path(),
        &[
            heart("2026-10-20 06:31:10 -0600", "70"),
            heart("2026-10-20 07:31:10 -0600", "71"),
            heart("2026-10-20 08:31:10 -0600", "72"),
        ],
    );
    let stop = stopping_at("written", 1);
    let mut stopped = request(&source, "20261104_120000");
    stopped.stop_at = Some(&stop);
    run_apple_health_segments(root, &stopped).unwrap_err();

    let other = export(
        &scratch.path().join("other"),
        &[heart("2026-10-22 06:31:10 -0600", "70")],
    );
    run(root, &other, "20261104_130000", None);
    let listing = root.join("imports/20261104_120000/body-touched.jsonl");
    let lines = fs::read_to_string(&listing).unwrap();
    let entries: Vec<serde_json::Value> = lines
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(entries.len(), 1, "{lines}");
    assert_eq!(
        entries[0]["windows"].as_array().unwrap().len(),
        1,
        "{lines}"
    );
    let listed = entries[0]["windows"][0][0].as_str().unwrap();
    let day = entries[0]["day"].as_str().unwrap();
    assert!(
        root.join("chronicle")
            .join(day)
            .join(STREAM)
            .join(listed)
            .join(BODY_RECORDS_FILE)
            .is_file(),
        "{lines}"
    );
}

/// A takeover of a run stopped before its first window names no day at all.
#[test]
fn a_takeover_of_a_run_that_wrote_nothing_lists_nothing() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(scratch.path(), &[heart("2026-10-20 06:31:10 -0600", "70")]);
    let stop = stopping_at("pending", 1);
    let mut stopped = request(&source, "20261104_120000");
    stopped.stop_at = Some(&stop);
    run_apple_health_segments(root, &stopped).unwrap_err();

    let other = export(
        &scratch.path().join("other"),
        &[heart("2026-10-22 06:31:10 -0600", "70")],
    );
    run(root, &other, "20261104_130000", None);
    let listing = root.join("imports/20261104_120000/body-touched.jsonl");
    assert_eq!(fs::read_to_string(&listing).unwrap_or_default(), "");
}

/// A takeover killed after its listing line landed and before its state moved: the
/// next one cuts back to the length the state knows, so no day is listed twice.
#[test]
fn a_takeovers_listing_line_isnt_doubled_after_a_kill() {
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
    let listing = root.join("imports/20261104_120000/body-touched.jsonl");
    // Longer than the line the takeover writes, so only cutting back removes it all.
    let mut torn = fs::read_to_string(&listing).unwrap_or_default();
    let windows: Vec<(String, u64, u64, u64)> = (0..200)
        .map(|index| (format!("{index:06}_3600"), 1, 0, 0))
        .collect();
    torn.push_str(&format!("{}\n", json!({"day": day, "windows": windows})));
    fs::write(&listing, torn).unwrap();
    let other = export(
        &scratch.path().join("other"),
        &[heart("2026-10-20 06:31:10 -0600", "70")],
    );
    run(root, &other, "20261104_130000", None);
    let days: Vec<String> = fs::read_to_string(&listing)
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["day"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    let unique: BTreeSet<&String> = days.iter().collect();
    assert_eq!(unique.len(), days.len(), "{days:?}");
    assert!(days.contains(&day), "{days:?}");
}

/// The bound holds in a repair too: a stuck stream stops it after a recorded day, and
/// each run gets further.
#[test]
fn a_stuck_stream_stops_a_repair_after_a_recorded_day_and_each_run_gets_further() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let mut records = Vec::new();
    for day in 1..=6 {
        for hour in 0..24 {
            records.push(heart(
                &format!("2026-10-{day:02} {hour:02}:10:00 -0600"),
                "70",
            ));
        }
    }
    let source = export(scratch.path(), &records);
    run(root, &source, "20261104_120000", None);
    fs::create_dir_all(root.join(RUN_DIR_REL)).unwrap();
    fs::write(root.join(RUN_DIR_REL).join("state.json"), "garbage").unwrap();
    let record = root.join("streams").join(format!("{STREAM}.json"));
    fs::write(&record, "not a record").unwrap();
    let done_through = |root: &Path| {
        let state: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(RUN_DIR_REL).join("state.json")).unwrap())
                .unwrap();
        (state["phase"].clone(), state["done_through"].clone())
    };
    let one = run_apple_health_segments(root, &request(&source, "20261105_120000")).unwrap_err();
    assert!(one.contains("carry on"), "{one}");
    let (phase, first) = done_through(root);
    assert_eq!(phase, "repair");
    let two = run_apple_health_segments(root, &request(&source, "20261105_120001")).unwrap_err();
    assert!(two.contains("carry on"), "{two}");
    let (phase, second) = done_through(root);
    assert_eq!(phase, "repair");
    assert!(second.as_str().unwrap() > first.as_str().unwrap());
}

/// A file where the stream's folder goes: no window can ever be read or written there,
/// and running again changes nothing, so it's counted. Windows answers a path through a
/// file as not found, so there the write is what fails, and the run stops and says so.
#[cfg(unix)]
#[test]
fn a_file_where_the_streams_folder_goes_is_counted_and_later_days_land() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(
        scratch.path(),
        &[
            heart("2026-10-10 06:31:10 -0600", "70"),
            heart("2026-10-12 06:31:10 -0600", "71"),
        ],
    );
    let blocked = root.join("chronicle/20261010").join(STREAM);
    fs::create_dir_all(blocked.parent().unwrap()).unwrap();
    fs::write(&blocked, "in the way").unwrap();
    let done = run(root, &source, "20261104_120000", None);
    assert!(done.finished, "{}", done.summary);
    assert!(
        done.summary.contains("1 reading wasn't added"),
        "{}",
        done.summary
    );
    assert!(later_day_landed(root));
    assert_eq!(fs::read(&blocked).unwrap(), b"in the way");
}

#[test]
fn an_import_adds_one_activity_per_workout_and_again_adds_none() {
    let journal = journal();
    let root = journal.path();
    let dir = root.join("facets/body");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("facet.json"),
        json!({"title": "body", "description": "body"}).to_string(),
    )
    .unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(&scratch.path().join("1"), &[workout(10), workout(11)]);
    let one = run(root, &source, "20261012_120000", Some("body"));
    assert!(
        one.summary.contains(
            "2 of the workouts, nights of sleep and mindfulness sessions now show on their days."
        ),
        "{}",
        one.summary
    );
    let files = ["20261010", "20261011"]
        .map(|day| root.join(format!("facets/body/activities/{day}.jsonl")));
    let before = files.clone().map(|file| fs::read(file).unwrap());
    for bytes in &before {
        let text = String::from_utf8_lossy(bytes);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains(r#""source":"body""#), "{text}");
        assert!(
            !text.contains("Loop"),
            "nothing of the row is copied: {text}"
        );
    }
    let two = run(root, &source, "20261012_120001", Some("body"));
    assert!(
        !two.summary.contains("now show on their days"),
        "{}",
        two.summary
    );
    assert_eq!(files.map(|file| fs::read(file).unwrap()), before);

    // The same export with no facet gives no record and says why.
    let fresh = tempfile::tempdir().unwrap();
    fs::create_dir_all(fresh.path().join("config")).unwrap();
    fs::write(
        fresh.path().join("config/journal.json"),
        json!({"identity": {"timezone": "America/Denver"}}).to_string(),
    )
    .unwrap();
    let three = run(fresh.path(), &source, "20261012_120002", None);
    assert!(
        three.summary.contains("2 of the workouts, nights of sleep and mindfulness sessions aren't listed on the activities page, because those parts of your journal have no facet."),
        "{}",
        three.summary
    );
    assert!(!fresh.path().join("facets").exists());
}

/// The one activity file line for `record_id` on `day`, under `facet`.
fn record_line(root: &Path, facet: &str, day: &str, record_id: &str) -> serde_json::Value {
    let text =
        fs::read_to_string(root.join(format!("facets/{facet}/activities/{day}.jsonl"))).unwrap();
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|record| record["id"] == record_id)
        .unwrap()
}

#[test]
fn a_record_follows_its_night_when_a_later_export_ends_it_later_and_keeps_the_owners_word() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let night = [stage_record(
        "2026-10-10 22:00:00 -0600",
        "2026-10-11 02:00:00 -0600",
        "AsleepCore",
    )];
    run(
        root,
        &export(scratch.path(), &night),
        "20261012_120000",
        Some("health"),
    );
    let text = fs::read_to_string(root.join("facets/health/activities/20261010.jsonl")).unwrap();
    let id = serde_json::from_str::<serde_json::Value>(text.lines().next().unwrap()).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // The owner titles, describes, details and hides it; a pair is kept apart.
    let mut patch = serde_json::Map::new();
    for (field, value) in [
        ("title", "Long night"),
        ("description", "after the flight"),
        ("details", "slept in"),
        ("playback", "none"),
    ] {
        patch.insert(field.to_owned(), json!(value));
    }
    solstone_core_facets::update_activity_record(
        root,
        "health",
        "20261010",
        &id,
        &patch,
        "owner:web",
        "",
        "2026-10-12T13:00:00Z",
    )
    .unwrap()
    .expect("edited");
    solstone_core_facets::set_activity_hidden(
        root,
        "health",
        "20261010",
        &id,
        true,
        "owner:web",
        None,
        "2026-10-12T13:01:00Z",
    )
    .unwrap();
    let path = root.join("facets/health/activities/20261010.jsonl");
    let mut record = record_line(root, "health", "20261010", &id);
    record["apart"] = json!(["body_0000000000000000"]);
    fs::write(&path, format!("{record}\n")).unwrap();
    let before = record_line(root, "health", "20261010", &id);

    // A later export ends the night four hours later.
    let longer = [
        night[0].clone(),
        stage_record(
            "2026-10-11 02:00:00 -0600",
            "2026-10-11 06:00:00 -0600",
            "AsleepDeep",
        ),
    ];
    let outcome = run(
        root,
        &export(&scratch.path().join("longer"), &longer),
        "20261012_120001",
        Some("health"),
    );
    assert!(
        outcome.summary.contains("changed its times or type"),
        "{}",
        outcome.summary
    );
    let after = record_line(root, "health", "20261010", &id);
    assert_eq!(after["account"]["end"], "2026-10-11 06:00:00 -0600");
    // Nothing but the account's end moved: the owner's word stands.
    let mut expected = before.clone();
    expected["account"]["end"] = json!("2026-10-11 06:00:00 -0600");
    assert_eq!(after, expected);

    // The same export again changes nothing, and says nothing.
    let bytes = fs::read(&path).unwrap();
    let again = run(
        root,
        &export(&scratch.path().join("again"), &longer),
        "20261012_120002",
        Some("health"),
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(
        !again.summary.contains("changed its times or type"),
        "{}",
        again.summary
    );
}

/// Moving Oura's documents changes nothing Apple writes or keeps:
/// its tiles keep their index ids, and a run's state holds no field of the moves.
#[test]
fn an_apple_runs_state_holds_nothing_of_moves_and_its_tiles_keep_their_index_ids() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = days_export(scratch.path());
    let stop = stopping_at("pending", 2);
    let mut stopped = request(&source, "20261104_120000");
    stopped.stop_at = Some(&stop);
    run_apple_health_segments(root, &stopped).unwrap_err();
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(RUN_DIR_REL).join(STATE_FILE)).unwrap())
            .unwrap();
    for field in ["drops", "unfinished", "findings"] {
        assert!(state.get(field).is_none(), "{field}: {state}");
    }
    for field in ["moved", "unfinished", "holes"] {
        assert!(state["pending"].get(field).is_none(), "{field}: {state}");
    }
    run_apple_health_segments(root, &request(&source, "20261104_130000")).unwrap();
    let mut tiles = 0;
    for path in snapshot(root)
        .into_keys()
        .filter(|path| path.ends_with("body.jsonl"))
    {
        for row in decode_rows(&fs::read_to_string(root.join(&path)).unwrap()).unwrap() {
            if row.kind == RowKind::ExperienceTile {
                tiles += 1;
                let of = row.of.as_deref().unwrap();
                assert_eq!(row.id, format!("{of}-{}", row.index.unwrap()), "{path}");
            }
        }
    }
    assert!(tiles > 0);
}
