// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_mcp_endpoint::{PermissionStore, ReadPermission, ReadScope, run_mcp_probe};

const WORK_ID: &str = "123e4567-e89b-42d3-a456-426614174000";
const BODY_ID: &str = "223e4567-e89b-42d3-a456-426614174000";

fn facets(root: &Path) {
    for (name, id) in [("work", WORK_ID), ("body", BODY_ID)] {
        let dir = root.join("facets").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("facet.json"),
            json!({"id": id, "title": name, "description": name}).to_string(),
        )
        .unwrap();
    }
}

fn listing_reaches_its_day(root: &Path, import_id: &str, day: &str) {
    // Exercise the actual existing listing consumer with this writer's bytes.
    // With no chronicle or span manifest, only its listing can mark the day.
    let reader = tempfile::tempdir().unwrap();
    let copied = reader.path().join("imports").join(import_id);
    fs::create_dir_all(&copied).unwrap();
    for name in [LISTING_FILE, "import.json"] {
        fs::copy(
            root.join("imports").join(import_id).join(name),
            copied.join(name),
        )
        .unwrap();
    }
    let old = reader.path().join("imports/20200101_120000");
    fs::create_dir_all(old.join("normalized")).unwrap();
    fs::write(
        old.join("manifest.json"),
        json!({"source_type": "apple_health", "days_affected": [day]}).to_string(),
    )
    .unwrap();
    let covered = || {
        solstone_core_location::old_imports::covered_days_among(reader.path(), [day].into_iter())
    };
    assert!(
        covered().is_empty(),
        "the actual split-key listing reached its day"
    );
    fs::write(copied.join(LISTING_FILE), "").unwrap();
    assert!(
        covered().contains_key(day),
        "without the listing the old day is still waiting"
    );
}

/// Read through the real MCP permission and source-span dispatch, not just
/// the facet file written by the importer.
pub(in crate::body_segments) fn entries(
    root: &Path,
    facet_id: &str,
    day: &str,
    source: &str,
) -> Vec<String> {
    let connection = format!("bearer:facet-{facet_id}");
    PermissionStore::open(root)
        .set_permission(
            &connection,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![facet_id.to_owned()],
                },
            },
        )
        .unwrap();
    let list = run_mcp_probe(
        root,
        &connection,
        "list_transcripts",
        &json!({
            "day": day, "source": source, "time_from": "06:00", "time_to": "07:00", "limit": 1
        }),
    )
    .unwrap();
    let Some(span) = list["span_reference"].as_str() else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    let mut cursor = None;
    loop {
        let mut args = json!({"reference": span});
        if let Some(cursor) = &cursor {
            args["cursor"] = json!(cursor);
        }
        let result = run_mcp_probe(root, &connection, "get_transcript", &args).unwrap();
        entries.extend(
            result["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry.as_str().unwrap().to_owned()),
        );
        cursor = result["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    entries
}

#[test]
fn equivalent_facet_files_do_not_lose_updated_entry_counts_when_named() {
    let journal = journal();
    facets(journal.path());
    let mut state = RunState::default();
    let work = facet_file_text("work");
    state.totals.retained_revisions.insert(work.clone(), 2);
    state
        .totals
        .retained_revisions
        .insert(format!(" {work}"), 3);
    let counts = summary_counts(journal.path(), &state);
    assert_eq!(counts.retained_revisions["your work facet"], 5);
}

#[test]
fn new_apple_identities_in_a_foreign_hour_are_readable_only_under_the_run_facet() {
    for listed in [ListedMoves::Keep, ListedMoves::MoveAtMost(0)] {
        let journal = journal();
        let root = journal.path();
        facets(root);
        let scratch = tempfile::tempdir().unwrap();
        let old = heart("2026-10-10 06:11:10 -0600", "7111");
        let first = export(&scratch.path().join("first"), &[old.clone(), workout(11)]);
        let original = run(root, &first, "20261012_120000", Some("work"));
        let old_bytes = body_bytes(root, "20261010", "060000_3600");
        let next = export(
            &scratch.path().join("next"),
            &[old, heart("2026-10-10 06:42:00 -0600", "7222"), workout(11)],
        );
        let mut request = AppleSegmentsRequest::new(&next, "20261012_130000", Some("body"));
        request.listed = listed;
        let result = run_apple_health_segments(root, &request).unwrap();
        assert!(result.finished, "{}", result.summary);
        assert!(
            result.summary.contains("saved separately"),
            "{}",
            result.summary
        );
        listing_reaches_its_day(root, &result.import_id, "20261010");
        assert_eq!(body_bytes(root, "20261010", "060000_3600"), old_bytes);
        let work = entries(root, WORK_ID, "20261010", "Apple Health · imported");
        let body = entries(root, BODY_ID, "20261010", "Apple Health · imported");
        assert_eq!(work.len(), 1, "{work:?}");
        assert_eq!(body.len(), 1, "{body:?}");
        assert!(work[0].contains("7111") && !work[0].contains("7222"));
        assert!(body[0].contains("7222") && !body[0].contains("7111"));
        let rows = decode_rows(
            &fs::read_to_string(window(root, "20261010", "060000_3602").join(BODY_RECORDS_FILE))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].run.as_deref(), Some(result.import_id.as_str()));
        assert_ne!(original.import_id, result.import_id);
        let repeat = run_apple_health_segments(root, &request).unwrap();
        assert!(repeat.finished, "{}", repeat.summary);
        assert_eq!(repeat.published, 0, "{}", repeat.summary);
        assert_eq!(
            entries(root, BODY_ID, "20261010", "Apple Health · imported"),
            body
        );
    }
}

#[test]
fn a_directory_with_only_a_foreign_facet_cannot_receive_new_rows_under_it() {
    for listed in [ListedMoves::Keep, ListedMoves::Move] {
        let journal = journal();
        let root = journal.path();
        facets(root);
        let scratch = tempfile::tempdir().unwrap();
        let original = window(root, "20261010", "060000_3600");
        fs::create_dir_all(original.join("talents")).unwrap();
        fs::write(
            original.join("talents/facets.json"),
            facet_file_text("work"),
        )
        .unwrap();
        let source = export(
            scratch.path(),
            &[heart("2026-10-10 06:42:00 -0600", "7222")],
        );
        let stop = stopping_at("written", 1);
        let mut request = AppleSegmentsRequest::new(&source, "20261012_130000", Some("body"));
        request.listed = listed;
        request.stop_at = Some(&stop);
        run_apple_health_segments(root, &request).unwrap_err();
        assert!(
            entries(root, WORK_ID, "20261010", "Apple Health · imported").is_empty(),
            "an interrupted first write inherited the old metadata-only facet"
        );
        if matches!(listed, ListedMoves::Move) {
            assert_eq!(
                fs::read_to_string(original.join("talents/facets.json")).unwrap(),
                facet_file_text("body")
            );
        } else {
            assert!(!original.join(BODY_RECORDS_FILE).exists());
            assert_eq!(
                fs::read_to_string(original.join("talents/facets.json")).unwrap(),
                facet_file_text("work")
            );
        }
        request.stop_at = None;
        let done = run_apple_health_segments(root, &request).unwrap();
        assert!(done.finished, "{}", done.summary);
        assert!(
            !done.summary.contains("saved separately"),
            "{}",
            done.summary
        );
        assert_eq!(
            entries(root, BODY_ID, "20261010", "Apple Health · imported").len(),
            1
        );
        assert!(entries(root, WORK_ID, "20261010", "Apple Health · imported").is_empty());
    }
}

#[test]
fn a_resume_respects_a_changed_facet_even_before_its_first_body_write() {
    let journal = journal();
    let root = journal.path();
    facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let source = export(
        scratch.path(),
        &[heart("2026-10-10 06:42:00 -0600", "7222")],
    );
    let stop = stopping_at("pending", 1);
    let mut request = AppleSegmentsRequest::new(&source, "20261012_130000", Some("body"));
    request.stop_at = Some(&stop);
    run_apple_health_segments(root, &request).unwrap_err();
    let Read::Trusted(state) = read_state(root, &APPLE) else {
        panic!("no saved run");
    };
    let planned = &state.pending.as_ref().unwrap().windows[0];
    let dir = window(root, "20261010", &planned.key);
    fs::create_dir_all(dir.join("talents")).unwrap();
    fs::write(dir.join("talents/facets.json"), facet_file_text("work")).unwrap();
    request.stop_at = None;
    let error = run_apple_health_segments(root, &request).unwrap_err();
    assert!(error.contains("now filed elsewhere"), "{error}");
    assert!(!dir.join(BODY_RECORDS_FILE).exists());
    assert_eq!(
        fs::read_to_string(dir.join("talents/facets.json")).unwrap(),
        facet_file_text("work")
    );
}

#[test]
fn the_remembered_default_command_splits_a_foreign_hour_without_an_answer() {
    let journal = journal();
    let root = journal.path();
    facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let old = heart("2026-10-10 06:11:10 -0600", "7111");
    let first = export(&scratch.path().join("first"), std::slice::from_ref(&old));
    run(root, &first, "20261012_120000", Some("work"));
    solstone_core_journal_config_write::record_body_facet(
        root,
        &solstone_core_journal_config::BodyFacetChoice::Facet("body".to_owned()),
    )
    .unwrap();
    let next = export(
        &scratch.path().join("next"),
        &[old, heart("2026-10-10 06:42:00 -0600", "7222")],
    );
    let command = crate::body_commands::BodyCommand {
        import_id: None,
        facet: crate::cli_argv::ImportFacet::Unattended,
        listed_moves_shown: None,
        scheduled: true,
        json: false,
        confirm: true,
        date_from: None,
        date_to: None,
        window_days: None,
        force: false,
        save: true,
    };
    let result = crate::body_commands::save_apple(next.to_str().unwrap(), &command, root);
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert!(
        result.stdout.contains("saved separately"),
        "{}",
        result.stdout
    );
    let body = entries(root, BODY_ID, "20261010", "Apple Health · imported");
    let work = entries(root, WORK_ID, "20261010", "Apple Health · imported");
    assert_eq!(body.len(), 1);
    assert_eq!(work.len(), 1);
    assert!(body[0].contains("7222") && !body[0].contains("7111"));
    assert!(work[0].contains("7111") && !work[0].contains("7222"));
}

#[test]
fn a_same_plan_resume_replays_the_saved_sibling_and_keeps_its_counts() {
    for point in ["pending", "written", "published"] {
        let journal = journal();
        let root = journal.path();
        facets(root);
        let scratch = tempfile::tempdir().unwrap();
        let old = heart("2026-10-10 06:11:10 -0600", "7111");
        let first = export(&scratch.path().join("first"), std::slice::from_ref(&old));
        run(root, &first, "20261012_120000", Some("work"));
        let next = export(
            &scratch.path().join("next"),
            &[old, heart("2026-10-10 06:42:00 -0600", "7222")],
        );
        let stop = stopping_at(point, 1);
        let mut saving = AppleSegmentsRequest::new(&next, "20261012_130000", Some("body"));
        saving.stop_at = Some(&stop);
        run_apple_health_segments(root, &saving).unwrap_err();
        let Read::Trusted(state) = read_state(root, &APPLE) else {
            panic!("no saved run");
        };
        let pending = state.pending.as_ref().unwrap();
        assert!(pending.filing_plan);
        assert_eq!(pending.counts.split_rows, 1);
        assert!(
            pending
                .windows
                .iter()
                .any(|window| window.key == "060000_3602")
        );
        saving.stop_at = None;
        let result = run_apple_health_segments(root, &saving).unwrap();
        assert_eq!(result.import_id, state.import_id);
        assert_eq!(
            entries(root, BODY_ID, "20261010", "Apple Health · imported").len(),
            1
        );
        assert_eq!(
            entries(root, WORK_ID, "20261010", "Apple Health · imported").len(),
            1
        );
        assert!(!window(root, "20261010", "060000_3603").exists());
        let listing = fs::read_to_string(
            root.join("imports")
                .join(&result.import_id)
                .join(LISTING_FILE),
        )
        .unwrap();
        let listing: serde_json::Value =
            serde_json::from_str(listing.lines().next().unwrap()).unwrap();
        assert!(listing.to_string().contains("060000_3602"), "{listing}");
    }
}

#[test]
fn a_resume_never_writes_unplanned_rows_after_a_formerly_unreadable_window_is_repaired() {
    let journal = journal();
    let root = journal.path();
    facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let first = export(
        &scratch.path().join("first"),
        &[heart("2026-10-10 06:11:10 -0600", "7111")],
    );
    run(root, &first, "20261012_120000", Some("work"));
    let path = window(root, "20261010", "060000_3600").join(BODY_RECORDS_FILE);
    let before = fs::read(&path).unwrap();
    fs::write(&path, "not rows").unwrap();
    let next = export(
        &scratch.path().join("next"),
        &[
            heart("2026-10-10 06:42:00 -0600", "7222"),
            heart("2026-10-10 08:42:00 -0600", "7333"),
        ],
    );
    let stop = stopping_at("pending", 1);
    let mut request = AppleSegmentsRequest::new(&next, "20261012_130000", Some("body"));
    request.stop_at = Some(&stop);
    run_apple_health_segments(root, &request).unwrap_err();
    let Read::Trusted(state) = read_state(root, &APPLE) else {
        panic!("no saved run");
    };
    assert_eq!(state.pending.as_ref().unwrap().counts.unreadable, 1);
    fs::write(&path, &before).unwrap();
    request.stop_at = None;
    let resumed = run_apple_health_segments(root, &request).unwrap();
    assert_eq!(resumed.import_id, state.import_id);
    let work = entries(root, WORK_ID, "20261010", "Apple Health · imported");
    assert_eq!(work.len(), 1, "{work:?}");
    assert!(!work[0].contains("7222"));
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "the saved unreadable decision is not silently replaced by a new write"
    );
    let fresh = run_apple_health_segments(root, &request).unwrap();
    assert_ne!(fresh.import_id, resumed.import_id);
    let body = entries(root, BODY_ID, "20261010", "Apple Health · imported");
    assert_eq!(body.len(), 1, "{body:?}");
    assert!(body[0].contains("7222"));
    assert_eq!(
        entries(root, WORK_ID, "20261010", "Apple Health · imported"),
        work
    );
}

#[test]
fn a_resume_whose_move_bound_is_lost_takes_over_before_splitting_new_rows() {
    let journal = journal();
    let root = journal.path();
    facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let old = heart("2026-10-10 06:11:10 -0600", "7111");
    let first = export(&scratch.path().join("first"), &[old.clone(), workout(11)]);
    run(root, &first, "20261012_120000", Some("work"));
    let next = export(
        &scratch.path().join("next"),
        &[old, heart("2026-10-10 06:42:00 -0600", "7222"), workout(11)],
    );
    let stop = stopping_at("pending", 1);
    let mut saving = AppleSegmentsRequest::new(&next, "20261012_130000", Some("body"));
    saving.listed = ListedMoves::MoveAtMost(1);
    saving.stop_at = Some(&stop);
    run_apple_health_segments(root, &saving).unwrap_err();
    let Read::Trusted(old_state) = read_state(root, &APPLE) else {
        panic!("no saved run");
    };
    assert!(old_state.move_listed);
    add_listed_elsewhere(root, "20261001", "arrived-later");
    saving.stop_at = None;
    let result = run_apple_health_segments(root, &saving).unwrap();
    assert_ne!(result.import_id, old_state.import_id);
    let body = entries(root, BODY_ID, "20261010", "Apple Health · imported");
    let work = entries(root, WORK_ID, "20261010", "Apple Health · imported");
    assert_eq!(body.len(), 1, "{body:?}");
    assert_eq!(work.len(), 1, "{work:?}");
    assert!(body[0].contains("7222") && work[0].contains("7111"));
    let previous = root.join("imports").join(&old_state.import_id);
    assert!(
        !previous.join(LISTING_FILE).exists(),
        "no planned-but-unwritten window is listed"
    );
}

#[test]
fn pre_fix_pending_bytes_take_over_with_only_the_rows_the_old_run_reached() {
    for reached in [false, true] {
        let journal = journal();
        let root = journal.path();
        facets(root);
        let scratch = tempfile::tempdir().unwrap();
        let old = heart("2026-10-10 06:11:10 -0600", "7111");
        let first = export(&scratch.path().join("first"), std::slice::from_ref(&old));
        run(root, &first, "20261012_120000", Some("work"));
        let next = export(
            &scratch.path().join("next"),
            &[old, heart("2026-10-10 06:42:00 -0600", "7222")],
        );
        let stop = stopping_at("pending", 1);
        let mut saving = AppleSegmentsRequest::new(&next, "20261012_130000", Some("body"));
        saving.stop_at = Some(&stop);
        run_apple_health_segments(root, &saving).unwrap_err();
        let Read::Trusted(state) = read_state(root, &APPLE) else {
            panic!("no saved run");
        };
        let refused = BTreeSet::new();
        let mut days = Days::open(
            root,
            &APPLE,
            &root.join(RUN_DIR_REL).join("placed"),
            &refused,
            None,
        )
        .unwrap();
        let (_, windows) = days.next_day().unwrap().unwrap();
        let (id, rows) = windows.into_iter().next().unwrap();
        // The pre-fix merge primitive, outside the new day placer, recreates
        // the old window's exact intended bytes. The checkpoint is a manually
        // authored schema-3 fixture, not today's pending serializer.
        let old_decision = solstone_core_import_sources::apple_health::decide(
            root,
            &APPLE,
            &id,
            rows,
            Some("body"),
            false,
            Some(&state.import_id),
        )
        .unwrap();
        let contents = old_decision.contents.unwrap();
        let mut pending: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/body_facet_pending_v3.json"))
                .unwrap();
        pending["windows"][0]["written_sha"] = json!(text_sha(&contents));
        let state_path = root.join(RUN_DIR_REL).join(STATE_FILE);
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
        legacy["pending"] = pending;
        fs::write(&state_path, legacy.to_string()).unwrap();
        if reached {
            fs::write(
                window(root, "20261010", "060000_3600").join(BODY_RECORDS_FILE),
                contents,
            )
            .unwrap();
        }
        saving.stop_at = None;
        let result = run_apple_health_segments(root, &saving).unwrap();
        assert_ne!(result.import_id, state.import_id);
        let previous = root.join("imports").join(&state.import_id);
        let metadata = read_provenance(root, &state.import_id).unwrap().unwrap();
        let AttemptRead::Present(facts) = read_attempt_facts(&metadata) else {
            panic!("no former attempt");
        };
        assert_eq!(
            facts.continued_by.as_deref(),
            Some(result.import_id.as_str())
        );
        assert_eq!(previous.join(LISTING_FILE).exists(), reached);
        if reached {
            let rows = decode_rows(
                &fs::read_to_string(
                    window(root, "20261010", "060000_3600").join(BODY_RECORDS_FILE),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(rows.len(), 2);
            assert!(
                rows.iter()
                    .any(|row| row.run.as_deref() == Some(state.import_id.as_str()))
            );
        } else {
            assert_eq!(
                entries(root, BODY_ID, "20261010", "Apple Health · imported").len(),
                1
            );
            assert_eq!(
                entries(root, WORK_ID, "20261010", "Apple Health · imported").len(),
                1
            );
        }
    }
}

#[test]
fn a_new_covering_workout_does_not_duplicate_readings_in_facet_siblings() {
    let journal = journal();
    let root = journal.path();
    facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let old = heart("2026-10-10 06:32:00 -0600", "7111");
    let new = heart("2026-10-10 06:34:00 -0600", "7222");
    let first = export(&scratch.path().join("first"), std::slice::from_ref(&old));
    run(root, &first, "20261012_120000", Some("work"));
    let next = export(&scratch.path().join("next"), &[old.clone(), new.clone()]);
    run_apple_health_segments(
        root,
        &AppleSegmentsRequest::new(&next, "20261012_130000", Some("body")),
    )
    .unwrap();
    let covering = export(&scratch.path().join("covering"), &[old, new, workout(10)]);
    let result = run_apple_health_segments(
        root,
        &AppleSegmentsRequest::new(&covering, "20261012_140000", Some("body")),
    )
    .unwrap();
    assert!(result.finished, "{}", result.summary);
    let rows = fs::read_dir(root.join("chronicle/20261010").join(STREAM))
        .unwrap()
        .flatten()
        .filter_map(|entry| fs::read_to_string(entry.path().join(BODY_RECORDS_FILE)).ok())
        .flat_map(|text| decode_rows(&text).unwrap())
        .filter(|row| row.kind == RowKind::Reading)
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(
        rows.iter()
            .map(|row| &row.id)
            .collect::<BTreeSet<_>>()
            .len(),
        2
    );
    let body = entries(root, BODY_ID, "20261010", "Apple Health · imported");
    let work = entries(root, WORK_ID, "20261010", "Apple Health · imported");
    assert!(body.iter().all(|entry| !entry.contains("7111")));
    assert!(body.iter().any(|entry| entry.contains("7222")));
    assert_eq!(work.len(), 1);
    assert!(work[0].contains("7111"));
}

#[test]
fn an_apple_night_keeps_its_longest_span_when_its_first_row_is_in_a_facet_sibling() {
    let journal = journal();
    let root = journal.path();
    facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let old = heart("2026-10-10 22:55:00 -0600", "7111");
    let first = export(&scratch.path().join("first"), &[old]);
    run(root, &first, "20261012_120000", Some("work"));
    let stage = stage_record(
        "2026-10-10 22:40:00 -0600",
        "2026-10-10 23:10:00 -0600",
        "AsleepCore",
    );
    let short = export(&scratch.path().join("short"), std::slice::from_ref(&stage));
    run_apple_health_segments(
        root,
        &AppleSegmentsRequest::new(&short, "20261012_130000", Some("body")),
    )
    .unwrap();
    let long = export(
        &scratch.path().join("long"),
        &[
            stage,
            stage_record(
                "2026-10-10 23:10:00 -0600",
                "2026-10-10 23:50:00 -0600",
                "AsleepDeep",
            ),
        ],
    );
    run_apple_health_segments(
        root,
        &AppleSegmentsRequest::new(&long, "20261012_140000", Some("body")),
    )
    .unwrap();
    run_apple_health_segments(
        root,
        &AppleSegmentsRequest::new(&short, "20261012_150000", Some("body")),
    )
    .unwrap();
    let rows = decode_rows(
        &fs::read_to_string(window(root, "20261010", "220000_3602").join(BODY_RECORDS_FILE))
            .unwrap(),
    )
    .unwrap();
    let night = rows
        .iter()
        .find(|row| row.kind == RowKind::Experience)
        .unwrap();
    assert_eq!(night.end.as_deref(), Some("2026-10-10 23:50:00 -0600"));
    assert_eq!(
        fs::read_to_string(window(root, "20261010", "220000_3600").join("talents/facets.json"))
            .unwrap(),
        facet_file_text("work")
    );
}
