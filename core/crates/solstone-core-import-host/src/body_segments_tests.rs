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
    run_apple_health_segments(
        root,
        &AppleSegmentsRequest {
            source,
            timestamp,
            facet,
        },
    )
    .unwrap()
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
    assert!(!root.join(PUBLISHING_MARKER_REL).exists());

    // The real writer's own lock file doesn't stop it being body-only.
    let recent = window(root, "20261010", "063000_300");
    assert!(
        recent.join("body.jsonl.lock").exists(),
        "the writer leaves a lock"
    );
    assert!(recent.join("stream.json").is_file());
    assert!(solstone_core_segment::is_body_only_dir(STREAM, &recent));
    assert!(solstone_core_segment::is_body_only_dir(
        STREAM,
        &window(root, "20190102", "063000_300")
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
    let before = body_bytes(root, "20261010", "063000_300");
    let marker = read_health_marker(root, "20261010", HealthMarkerKind::Body).unwrap();

    let again = run(root, &source, "20261012_120001", None);
    assert_eq!(again.published, 0, "{}", again.summary);
    assert_eq!(body_bytes(root, "20261010", "063000_300"), before);
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
    let rows = decode_rows(&String::from_utf8(body_bytes(root, "20261010", "063000_300")).unwrap())
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
        window(root, "20261010", "070000_300")
            .join(BODY_RECORDS_FILE)
            .is_file()
    );

    // Deleted through the owner's door, it stays deleted and the summary survives.
    let outcome = door::remove_segments(
        root,
        &[solstone_core_retention::Target {
            day: "20261010".to_owned(),
            stream: STREAM.to_owned(),
            dir: "063000_300".to_owned(),
        }],
        &chrono::Utc::now().to_rfc3339(),
        RemovalReason::OwnerSegmentDelete,
        "test",
    );
    assert!(outcome.halted.is_none());
    let tombstoned = window(root, "20261010", "063000_300");
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
        2
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
    let first = body_bytes(root, "20261010", "235000_300");

    let fewer = export(&scratch.path().join("fewer"), &whole[..1]);
    assert_eq!(run(root, &fewer, "20261012_120001", None).published, 0);
    assert_eq!(body_bytes(root, "20261010", "235000_300"), first);

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
    for (day, key) in [("20261010", "235500_300"), ("20261011", "001500_300")] {
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
        assert_eq!(tiles[0].count, Some(8));
        assert_eq!(tiles[0].end.as_deref(), Some("2026-10-11 00:30:00 -0600"));
    }
}

#[test]
fn a_chosen_facet_is_written_refused_when_muted_and_a_change_republishes() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let source = export(scratch.path(), &[heart("2026-10-10 06:31:10 -0600", "70")]);
    let refused = run_apple_health_segments(
        root,
        &AppleSegmentsRequest {
            source: &source,
            timestamp: "20261012_120000",
            facet: Some("absent"),
        },
    );
    assert!(refused.is_err());
    assert!(
        !root.join("chronicle").exists(),
        "refused before anything is written"
    );

    let first = run(root, &source, "20261012_120001", Some("health"));
    assert_eq!(first.published, 1);
    let dir = window(root, "20261010", "063000_300");
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
        fs::read(window(root, "20261010", "063000_300").join(BODY_RECORDS_FILE)).unwrap();
    fs::write(orphan.join(BODY_RECORDS_FILE), &orphan_row).unwrap();
    fs::write(root.join(PUBLISHING_MARKER_REL), b"").unwrap();

    // The next export doesn't mention it, and repair binds it anyway.
    let other = export(
        &scratch.path().join("other"),
        &[heart("2026-10-11 06:31:10 -0600", "72")],
    );
    let repaired = run(root, &other, "20261012_120001", None);
    assert!(repaired.finished);
    assert_eq!(repaired.published, 3, "{}", repaired.summary);
    assert!(orphan.join("stream.json").is_file());
    assert!(!root.join(PUBLISHING_MARKER_REL).exists());

    // With an unbound window and no marker, the window itself asks to bind.
    fs::remove_file(window(root, "20261011", "063000_300").join("stream.json")).unwrap();
    let rebound = run(root, &other, "20261012_120002", None);
    assert_eq!(rebound.published, 1);
    assert!(
        window(root, "20261011", "063000_300")
            .join("stream.json")
            .is_file()
    );
}

#[test]
fn a_publication_that_failed_leaves_the_marker_and_the_next_run_repairs_it() {
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
    assert!(root.join(PUBLISHING_MARKER_REL).exists());

    fs::remove_file(&blocked).unwrap();
    let repaired = run(root, &source, "20261012_120001", None);
    assert!(repaired.finished);
    assert_eq!(repaired.published, 1, "same bytes, but it publishes again");
    assert!(!root.join(PUBLISHING_MARKER_REL).exists());
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
        &AppleSegmentsRequest {
            source: &source,
            timestamp: "20261012_120000",
            facet: None,
        },
    );
    assert!(failed.is_err());
    assert!(root.join(PUBLISHING_MARKER_REL).exists());
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
        window(root, "20261010", "063000_300")
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
